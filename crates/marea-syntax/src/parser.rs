//! Parser de descenso recursivo con un sub-parser Pratt para expresiones.
//!
//! Consume la lista de tokens del lexer y produce un `Module`. Los niveles:
//!   - `parse_module` / `parse_item`  -> declaraciones de nivel superior
//!   - `parse_block` / `parse_stmt`   -> sentencias
//!   - `parse_bin_expr` (Pratt)       -> expresiones por precedencia
//!   - `parse_type`                   -> tipos, incluidas las uniones `A | B`

use crate::ast::*;
use crate::error::SyntaxError;
use crate::token::{TemplatePiece, Token, TokenKind};

type PResult<T> = Result<T, SyntaxError>;

/// Profundidad máxima de anidamiento de expresiones, para no desbordar la pila
/// nativa ante entradas como `((((...))))` y devolver un error ordinario.
/// Conservador para caber holgado en el hilo de 2 MB de los tests (cada nivel
/// son varios frames anidados); de sobra para cualquier expresión real.
const MAX_DEPTH: usize = 128;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    /// Cuando es `true`, un `Ident` seguido de `{` NO se lee como literal de
    /// registro (se deja el `{` para abrir un bloque). Se activa al parsear la
    /// condición de un `if` o el escrutinio de un `match`, y se resetea dentro
    /// de contextos delimitados `( )`, `[ ]` y argumentos de llamada.
    no_struct_literal: bool,
    /// Profundidad de recursión actual del parser de expresiones.
    depth: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser {
            tokens,
            pos: 0,
            no_struct_literal: false,
            depth: 0,
        }
    }

    /// Ejecuta `f` con `no_struct_literal = true` y restaura el valor previo.
    fn with_no_struct_literal<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let prev = self.no_struct_literal;
        self.no_struct_literal = true;
        let result = f(self);
        self.no_struct_literal = prev;
        result
    }

    /// Ejecuta `f` con `no_struct_literal = false` (contexto delimitado donde el
    /// `{` SÍ puede iniciar un literal de registro) y restaura el valor previo.
    fn allow_struct_literal<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        let prev = self.no_struct_literal;
        self.no_struct_literal = false;
        let result = f(self);
        self.no_struct_literal = prev;
        result
    }

    /// Punto de entrada (fail-fast): tokens -> módulo, abortando en el primer
    /// error. Lo usan `build`/`build-wasm`, que exigen un AST completo y válido.
    pub fn parse_module(tokens: Vec<Token>) -> PResult<Module> {
        let mut p = Parser::new(tokens);
        let mut items = Vec::new();
        while !p.at_eof() {
            items.push(p.parse_item()?);
        }
        Ok(Module { items })
    }

    /// Punto de entrada CON RECUPERACIÓN: devuelve el módulo PARCIAL (los items
    /// que sí parsearon) y TODOS los errores de sintaxis. Ante un error, salta
    /// hasta el inicio probable del siguiente item y continúa. Lo usa el LSP y
    /// `marea check` para reportar varios diagnósticos a la vez.
    pub fn parse_module_recovering(tokens: Vec<Token>) -> (Module, Vec<SyntaxError>) {
        let mut p = Parser::new(tokens);
        let mut items = Vec::new();
        let mut errors = Vec::new();
        while !p.at_eof() {
            let before = p.pos;
            match p.parse_item() {
                Ok(item) => items.push(item),
                Err(e) => {
                    errors.push(e);
                    p.recover_to_item();
                }
            }
            // Garantía anti-bucle: siempre avanzar al menos un token.
            if p.pos == before && !p.at_eof() {
                p.advance();
            }
        }
        (Module { items }, errors)
    }

    /// Salta tokens hasta el inicio probable del siguiente item (`@`, `fn`,
    /// `type`, `let`, `reactive`) o el fin del archivo.
    ///
    /// NO avanza incondicionalmente: si el error dejó el cursor justo al inicio
    /// de un item (p. ej. un `;` faltante reportado sobre la `fn` siguiente), no
    /// consume esa palabra clave, para no descartar un item válido. El progreso
    /// lo garantiza el guardia anti-bucle de `parse_module_recovering`.
    fn recover_to_item(&mut self) {
        while !self.at_eof() {
            if matches!(
                self.peek_kind(),
                TokenKind::At
                    | TokenKind::Fn
                    | TokenKind::Type
                    | TokenKind::Let
                    | TokenKind::Reactive
            ) {
                return;
            }
            self.advance();
        }
    }

    // --- utilidades ---

    fn peek(&self) -> &Token {
        &self.tokens[self.pos]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.tokens[self.pos].kind
    }

    fn at_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    fn advance(&mut self) -> Token {
        let tok = self.tokens[self.pos].clone();
        if !self.at_eof() {
            self.pos += 1;
        }
        tok
    }

    fn check(&self, kind: &TokenKind) -> bool {
        self.peek_kind() == kind
    }

    fn eat(&mut self, kind: &TokenKind) -> bool {
        if self.check(kind) {
            self.advance();
            true
        } else {
            false
        }
    }

    fn expect(&mut self, kind: &TokenKind, what: &str) -> PResult<Token> {
        if self.check(kind) {
            Ok(self.advance())
        } else {
            Err(SyntaxError::new(
                format!("se esperaba {}", what),
                self.peek().span,
            ))
        }
    }

    fn expect_ident(&mut self, what: &str) -> PResult<(String, crate::span::Span)> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                let span = self.advance().span;
                Ok((name, span))
            }
            _ => Err(SyntaxError::new(
                format!("se esperaba {}", what),
                self.peek().span,
            )),
        }
    }

    // --- items ---

    fn parse_item(&mut self) -> PResult<Item> {
        let anotacion = self.parse_anotacion()?;
        let sin_anotacion = anotacion.location.is_none() && !anotacion.es_session;
        match self.peek_kind() {
            TokenKind::Fn => Ok(Item::Fn(self.parse_fn(anotacion)?)),
            TokenKind::Type if sin_anotacion => Ok(Item::Type(self.parse_type_decl()?)),
            TokenKind::Let | TokenKind::Reactive if sin_anotacion => {
                Ok(Item::Let(self.parse_let()?))
            }
            TokenKind::Store if sin_anotacion => self.parse_store(),
            _ if !sin_anotacion => Err(SyntaxError::new(
                "las anotaciones (@server/@client/@edge/@session) solo aplican a funciones",
                self.peek().span,
            )),
            _ => Err(SyntaxError::new(
                "se esperaba un elemento de nivel superior: fn, type, let o store",
                self.peek().span,
            )),
        }
    }

    /// `store Post;` — declara el tipo de elemento del store del servidor.
    fn parse_store(&mut self) -> PResult<Item> {
        let kw = self.expect(&TokenKind::Store, "'store'")?;
        let (name, name_span) = self.expect_ident("el nombre del almacén")?;
        self.expect(
            &TokenKind::Colon,
            "':' entre el nombre y el tipo del almacén",
        )?;
        let ty = self.parse_type()?;
        let semi = self.expect(&TokenKind::Semicolon, "';' al final de 'store'")?;
        Ok(Item::Store {
            name,
            name_span,
            ty,
            span: kw.span.to(semi.span),
        })
    }

    /// La anotación de una función: `@server`, `@client`, `@edge` —con política
    /// opcional entre paréntesis, `@server(Usuario)`— o `@session`.
    ///
    /// La política va en el mismo hueco que un tipo cualquiera y no introduce
    /// palabras clave: `Public` es un tipo builtin más. Por eso escala a roles
    /// (`@server(Admin)`) sin tocar la gramática.
    fn parse_anotacion(&mut self) -> PResult<Anotacion> {
        if !self.check(&TokenKind::At) {
            return Ok(Anotacion::default());
        }
        let at = self.advance();
        let (name, span) = self.expect_ident("nombre de anotación tras '@'")?;
        if name == "session" {
            return Ok(Anotacion {
                location: None,
                politica: None,
                identidad_bind: None,
                es_session: true,
            });
        }
        let location = match name.as_str() {
            "server" => Location::Server,
            "client" => Location::Client,
            "edge" => Location::Edge,
            other => {
                return Err(SyntaxError::new(
                    format!(
                        "anotación desconocida '@{}'; usa @server, @client, @edge o @session",
                        other
                    ),
                    at.span.to(span),
                ))
            }
        };
        // Política: `@server(Usuario)`. Opcional en la gramática; que falte o no
        // sea aceptable lo decide el verificador, que es quien sabe si el
        // programa declaró identidad.
        let mut politica = None;
        let mut identidad_bind = None;
        if self.eat(&TokenKind::LParen) {
            // `@server(u: Usuario)` liga la identidad a un nombre utilizable en
            // el cuerpo; `@server(Usuario)` la exige sin nombrarla. Se distingue
            // con un token de adelanto: un IDENT seguido de ':' es el nombre.
            if matches!(self.peek_kind(), TokenKind::Ident(_))
                && matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::Colon)
                )
            {
                let (nombre, _) = self.expect_ident("nombre de la identidad")?;
                self.advance(); // ':'
                identidad_bind = Some(nombre);
            }
            politica = Some(self.parse_type()?);
            self.expect(&TokenKind::RParen, "')' para cerrar la política")?;
        }
        Ok(Anotacion {
            location: Some(location),
            politica,
            identidad_bind,
            es_session: false,
        })
    }

    fn parse_fn(&mut self, anotacion: Anotacion) -> PResult<FnDecl> {
        let fn_tok = self.expect(&TokenKind::Fn, "'fn'")?;
        let (name, _) = self.expect_ident("nombre de función")?;
        self.expect(&TokenKind::LParen, "'(' tras el nombre de la función")?;

        let mut params = Vec::new();
        if !self.check(&TokenKind::RParen) {
            loop {
                params.push(self.parse_param()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RParen) {
                    break; // coma final permitida
                }
            }
        }
        self.expect(&TokenKind::RParen, "')' para cerrar los parámetros")?;

        let return_type = if self.eat(&TokenKind::Arrow) {
            Some(self.parse_type()?)
        } else {
            None
        };

        let body = self.parse_block()?;
        let span = fn_tok.span.to(body.span);
        Ok(FnDecl {
            location: anotacion.location,
            politica: anotacion.politica,
            identidad_bind: anotacion.identidad_bind,
            es_session: anotacion.es_session,
            name,
            params,
            return_type,
            body,
            span,
        })
    }

    fn parse_param(&mut self) -> PResult<Param> {
        let (name, name_span) = self.expect_ident("nombre de parámetro")?;
        self.expect(&TokenKind::Colon, "':' tras el nombre del parámetro")?;
        let ty = self.parse_type()?;
        let span = name_span.to(ty.span());
        Ok(Param {
            name,
            name_span,
            ty,
            span,
        })
    }

    fn parse_type_decl(&mut self) -> PResult<TypeDecl> {
        let kw = self.expect(&TokenKind::Type, "'type'")?;
        let (name, _) = self.expect_ident("nombre del tipo")?;
        self.expect(&TokenKind::Eq, "'=' en la declaración de tipo")?;
        let aliased = self.parse_type()?;
        let semi = self.expect(
            &TokenKind::Semicolon,
            "';' al final de la declaración de tipo",
        )?;
        Ok(TypeDecl {
            name,
            aliased,
            span: kw.span.to(semi.span),
        })
    }

    // --- tipos ---

    fn parse_type(&mut self) -> PResult<Type> {
        // Los tipos también anidan (`List<List<...>>`, registros dentro de
        // registros) y también recursan, así que llevan la misma guarda que las
        // expresiones. Sin ella `List<` repetido desbordaba la pila del proceso
        // —y el servidor de lenguaje, que parsea texto sin terminar en cada
        // pulsación, moría con él.
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(SyntaxError::new("tipo demasiado anidado", self.peek().span));
        }
        let result = self.parse_type_inner();
        self.depth -= 1;
        result
    }

    fn parse_type_inner(&mut self) -> PResult<Type> {
        let first = self.parse_type_primary()?;
        if !self.check(&TokenKind::Pipe) {
            return Ok(first);
        }
        let mut variants = vec![first];
        while self.eat(&TokenKind::Pipe) {
            variants.push(self.parse_type_primary()?);
        }
        let span = variants
            .first()
            .unwrap()
            .span()
            .to(variants.last().unwrap().span());
        Ok(Type::Union { variants, span })
    }

    fn parse_type_primary(&mut self) -> PResult<Type> {
        if self.check(&TokenKind::LBrace) {
            return self.parse_record_type();
        }
        let (name, name_span) = self.expect_ident("un tipo")?;
        let mut args = Vec::new();
        let mut end = name_span;
        if self.check(&TokenKind::Lt) {
            self.advance(); // '<'
            loop {
                args.push(self.parse_type()?);
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
            }
            end = self
                .expect(&TokenKind::Gt, "'>' para cerrar los argumentos de tipo")?
                .span;
        }
        Ok(Type::Name {
            name,
            args,
            span: name_span.to(end),
        })
    }

    /// Tipo registro estructural: `{ campo: Tipo, ... }`.
    fn parse_record_type(&mut self) -> PResult<Type> {
        let open = self.expect(&TokenKind::LBrace, "'{' del tipo registro")?;
        let mut fields = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if !self.check(&TokenKind::RBrace) {
            loop {
                let (name, name_span) = self.expect_ident("nombre de campo")?;
                self.expect(&TokenKind::Colon, "':' tras el nombre del campo")?;
                let ty = self.parse_type()?;
                let span = name_span.to(ty.span());
                if !seen.insert(name.clone()) {
                    return Err(SyntaxError::new(
                        format!("campo '{name}' repetido en el tipo registro"),
                        name_span,
                    ));
                }
                fields.push(FieldDef { name, ty, span });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RBrace) {
                    break;
                }
            }
        }
        let close = self.expect(&TokenKind::RBrace, "'}' para cerrar el tipo registro")?;
        Ok(Type::Record {
            fields,
            span: open.span.to(close.span),
        })
    }

    // --- sentencias ---

    fn parse_block(&mut self) -> PResult<Block> {
        let open = self.expect(&TokenKind::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            stmts.push(self.parse_stmt()?);
        }
        let close = self.expect(&TokenKind::RBrace, "'}' para cerrar el bloque")?;
        Ok(Block {
            stmts,
            span: open.span.to(close.span),
        })
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        // Asignación `IDENT = expr;` — sólo si el token siguiente es '=' (no '==').
        if let TokenKind::Ident(_) = self.peek_kind() {
            if matches!(
                self.tokens.get(self.pos + 1).map(|t| &t.kind),
                Some(TokenKind::Eq)
            ) {
                return self.parse_assign();
            }
        }
        match self.peek_kind() {
            TokenKind::Let | TokenKind::Reactive => Ok(Stmt::Let(self.parse_let()?)),
            TokenKind::For => self.parse_for(),
            TokenKind::Effect => {
                let kw = self.advance();
                let body = self.parse_block()?;
                let span = kw.span.to(body.span);
                Ok(Stmt::Effect { body, span })
            }
            TokenKind::Return => {
                let kw = self.advance();
                let value = if self.check(&TokenKind::Semicolon) {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                let semi = self.expect(&TokenKind::Semicolon, "';' tras 'return'")?;
                Ok(Stmt::Return {
                    value,
                    span: kw.span.to(semi.span),
                })
            }
            _ => {
                let expr = self.parse_expr()?;
                // 'if' y 'match' como sentencia no exigen ';' (son tipo bloque).
                let needs_semi = !matches!(expr, Expr::If { .. } | Expr::Match { .. });
                if needs_semi {
                    self.expect(&TokenKind::Semicolon, "';' al final de la expresión")?;
                } else {
                    self.eat(&TokenKind::Semicolon);
                }
                Ok(Stmt::Expr(expr))
            }
        }
    }

    /// `for x in xs { ... }`, o `for x, i in xs { ... }` para llevar el índice.
    fn parse_for(&mut self) -> PResult<Stmt> {
        let kw = self.expect(&TokenKind::For, "'for'")?;
        let (var, var_span) = self.expect_ident("el nombre del elemento")?;
        let (index, index_span) = if self.eat(&TokenKind::Comma) {
            let (n, sp) = self.expect_ident("el nombre del índice")?;
            (Some(n), Some(sp))
        } else {
            (None, None)
        };
        self.expect(&TokenKind::In, "'in' tras el nombre del elemento")?;
        // Igual que en la condición de un `if`: aquí un `{` abre el cuerpo del
        // bucle, no un literal de registro.
        let iter = self.with_no_struct_literal(|p| p.parse_expr())?;
        let body = self.parse_block()?;
        let span = kw.span.to(body.span);
        Ok(Stmt::For {
            var,
            var_span,
            index,
            index_span,
            iter,
            body,
            span,
        })
    }

    fn parse_assign(&mut self) -> PResult<Stmt> {
        let (name, name_span) = self.expect_ident("nombre de variable")?;
        self.expect(&TokenKind::Eq, "'=' en la asignación")?;
        let value = self.parse_expr()?;
        let semi = self.expect(&TokenKind::Semicolon, "';' al final de la asignación")?;
        Ok(Stmt::Assign {
            name,
            name_span,
            value,
            span: name_span.to(semi.span),
        })
    }

    fn parse_let(&mut self) -> PResult<LetStmt> {
        let (reactive, kw_span) = if self.check(&TokenKind::Reactive) {
            (true, self.advance().span)
        } else {
            (
                false,
                self.expect(&TokenKind::Let, "'let' o 'reactive'")?.span,
            )
        };
        let mutable = self.eat(&TokenKind::Mut);
        let (name, _) = self.expect_ident("nombre de variable")?;
        let ty = if self.eat(&TokenKind::Colon) {
            Some(self.parse_type()?)
        } else {
            None
        };
        self.expect(&TokenKind::Eq, "'=' en la asignación")?;
        let value = self.parse_expr()?;
        let semi = self.expect(&TokenKind::Semicolon, "';' al final de la sentencia")?;
        Ok(LetStmt {
            reactive,
            mutable,
            name,
            ty,
            value,
            span: kw_span.to(semi.span),
        })
    }

    // --- expresiones (Pratt) ---

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_bin_expr(1)
    }

    /// `min_bp` es la potencia de enlace mínima aceptada. Mayor número = mayor
    /// precedencia. Todos los operadores son asociativos por la izquierda.
    fn parse_bin_expr(&mut self, min_bp: u8) -> PResult<Expr> {
        let mut left = self.parse_unary()?;
        loop {
            let (op, bp) = match self.peek_kind() {
                TokenKind::PipePipe => (BinOp::Or, 1),
                TokenKind::AmpAmp => (BinOp::And, 2),
                TokenKind::EqEq => (BinOp::Eq, 3),
                TokenKind::BangEq => (BinOp::Ne, 3),
                TokenKind::Lt => (BinOp::Lt, 4),
                TokenKind::Gt => (BinOp::Gt, 4),
                TokenKind::Le => (BinOp::Le, 4),
                TokenKind::Ge => (BinOp::Ge, 4),
                TokenKind::Plus => (BinOp::Add, 5),
                TokenKind::Minus => (BinOp::Sub, 5),
                TokenKind::Star => (BinOp::Mul, 6),
                TokenKind::Slash => (BinOp::Div, 6),
                TokenKind::Percent => (BinOp::Rem, 6),
                _ => break,
            };
            if bp < min_bp {
                break;
            }
            self.advance(); // consume el operador
            let right = self.parse_bin_expr(bp + 1)?;
            let span = left.span().to(right.span());
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
                span,
            };
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        // Todo nivel de anidamiento de expresión pasa por aquí (paréntesis,
        // listas y unarios); contamos la profundidad para no desbordar la pila.
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            self.depth -= 1;
            return Err(SyntaxError::new(
                "expresión demasiado anidada",
                self.peek().span,
            ));
        }
        let result = self.parse_unary_inner();
        self.depth -= 1;
        result
    }

    fn parse_unary_inner(&mut self) -> PResult<Expr> {
        let (op, op_span) = match self.peek_kind() {
            TokenKind::Minus => (UnaryOp::Neg, self.peek().span),
            TokenKind::Bang => (UnaryOp::Not, self.peek().span),
            _ => return self.parse_postfix(),
        };
        self.advance();
        let expr = self.parse_unary()?;
        let span = op_span.to(expr.span());
        Ok(Expr::Unary {
            op,
            expr: Box::new(expr),
            span,
        })
    }

    /// Sufijos: llamadas `f(...)` y acceso a miembro `x.campo`, encadenables.
    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut expr = self.parse_primary()?;
        loop {
            match self.peek_kind() {
                TokenKind::LParen => {
                    self.advance();
                    let mut args = Vec::new();
                    if !self.check(&TokenKind::RParen) {
                        loop {
                            // Dentro de la llamada el `{` SÍ puede iniciar un registro.
                            args.push(self.allow_struct_literal(|p| p.parse_expr())?);
                            if !self.eat(&TokenKind::Comma) {
                                break;
                            }
                            if self.check(&TokenKind::RParen) {
                                break;
                            }
                        }
                    }
                    let close = self.expect(&TokenKind::RParen, "')' para cerrar la llamada")?;
                    let span = expr.span().to(close.span);
                    expr = Expr::Call {
                        callee: Box::new(expr),
                        args,
                        span,
                    };
                }
                TokenKind::Dot => {
                    self.advance();
                    let (field, field_span) = self.expect_ident("nombre de campo tras '.'")?;
                    let span = expr.span().to(field_span);
                    expr = Expr::Member {
                        object: Box::new(expr),
                        field,
                        span,
                    };
                }
                TokenKind::LBracket => {
                    self.advance();
                    // Dentro de '[ ]' el '{' SÍ puede iniciar un registro.
                    let index = self.allow_struct_literal(|p| p.parse_expr())?;
                    let close = self.expect(&TokenKind::RBracket, "']' para cerrar el indexado")?;
                    let span = expr.span().to(close.span);
                    expr = Expr::Index {
                        object: Box::new(expr),
                        index: Box::new(index),
                        span,
                    };
                }
                _ => break,
            }
        }
        Ok(expr)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Int(value) => {
                self.advance();
                Ok(Expr::Int {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::Float(value) => {
                self.advance();
                Ok(Expr::Float {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::Str {
                    value: s,
                    span: tok.span,
                })
            }
            TokenKind::Template(piezas) => {
                self.advance();
                let mut parts = Vec::new();
                for pieza in piezas {
                    match pieza {
                        TemplatePiece::Lit(t) => parts.push(TemplatePart::Lit(t)),
                        TemplatePiece::Hueco {
                            fuente,
                            offset,
                            crudo,
                        } => {
                            // El hueco se parsea aparte, desplazando sus spans
                            // al sitio real del archivo: así un error dentro de
                            // `{...}` señala su columna, no la de la plantilla.
                            let expr = parse_hueco(&fuente, offset)?;
                            parts.push(TemplatePart::Interp {
                                expr: Box::new(expr),
                                raw: crudo,
                            });
                        }
                    }
                }
                Ok(Expr::Template {
                    parts,
                    span: tok.span,
                })
            }
            TokenKind::Bool(value) => {
                self.advance();
                Ok(Expr::Bool {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::Ident(name) => {
                // `Ident {` es un literal de registro, salvo en posición de
                // condición/escrutinio (donde `{` abre un bloque).
                let next_is_brace = matches!(
                    self.tokens.get(self.pos + 1).map(|t| &t.kind),
                    Some(TokenKind::LBrace)
                );
                if next_is_brace && !self.no_struct_literal {
                    self.parse_record_literal(name, tok.span)
                } else {
                    self.advance();
                    Ok(Expr::Ident {
                        name,
                        span: tok.span,
                    })
                }
            }
            TokenKind::LParen => {
                self.advance();
                // Dentro de paréntesis el `{` SÍ puede iniciar un registro.
                let inner = self.allow_struct_literal(|p| p.parse_expr())?;
                self.expect(&TokenKind::RParen, "')'")?;
                Ok(inner)
            }
            TokenKind::LBracket => {
                self.advance();
                let mut elements = Vec::new();
                if !self.check(&TokenKind::RBracket) {
                    loop {
                        elements.push(self.allow_struct_literal(|p| p.parse_expr())?);
                        if !self.eat(&TokenKind::Comma) {
                            break;
                        }
                        if self.check(&TokenKind::RBracket) {
                            break;
                        }
                    }
                }
                let close = self.expect(&TokenKind::RBracket, "']' para cerrar la lista")?;
                Ok(Expr::List {
                    elements,
                    span: tok.span.to(close.span),
                })
            }
            TokenKind::If => self.parse_if(),
            TokenKind::Match => self.parse_match(),
            _ => Err(SyntaxError::new("se esperaba una expresión", tok.span)),
        }
    }

    /// Literal de registro: `User { name: "x", age: 1 }`. El identificador y el
    /// `{` aún no se consumieron al entrar.
    fn parse_record_literal(
        &mut self,
        type_name: String,
        name_span: crate::span::Span,
    ) -> PResult<Expr> {
        self.advance(); // consume el identificador del tipo
        self.expect(&TokenKind::LBrace, "'{' del literal de registro")?;
        let mut fields = Vec::new();
        let mut seen = std::collections::HashSet::new();
        if !self.check(&TokenKind::RBrace) {
            loop {
                let (name, fname_span) = self.expect_ident("nombre de campo")?;
                self.expect(&TokenKind::Colon, "':' tras el nombre del campo")?;
                // El valor del campo es un contexto delimitado: registros OK.
                let value = self.allow_struct_literal(|p| p.parse_expr())?;
                let span = fname_span.to(value.span());
                if !seen.insert(name.clone()) {
                    return Err(SyntaxError::new(
                        format!("campo '{name}' repetido en el literal de registro"),
                        fname_span,
                    ));
                }
                fields.push(FieldInit { name, value, span });
                if !self.eat(&TokenKind::Comma) {
                    break;
                }
                if self.check(&TokenKind::RBrace) {
                    break;
                }
            }
        }
        let close = self.expect(&TokenKind::RBrace, "'}' para cerrar el literal de registro")?;
        Ok(Expr::Record {
            type_name: Some(type_name),
            type_name_span: Some(name_span),
            fields,
            span: name_span.to(close.span),
        })
    }

    fn parse_if(&mut self) -> PResult<Expr> {
        let kw = self.expect(&TokenKind::If, "'if'")?;
        // En la condición, un `Ident {` abre el bloque, no un registro.
        let cond = self.with_no_struct_literal(|p| p.parse_expr())?;
        let then_branch = self.parse_block()?;
        let mut span = kw.span.to(then_branch.span);

        let else_branch = if self.eat(&TokenKind::Else) {
            if self.check(&TokenKind::If) {
                // Una cadena larga de `else if` recursa aquí sin pasar por
                // parse_unary, así que cuenta su propia profundidad.
                self.depth += 1;
                if self.depth > MAX_DEPTH {
                    self.depth -= 1;
                    return Err(SyntaxError::new(
                        "cadena de 'else if' demasiado larga",
                        self.peek().span,
                    ));
                }
                let anidado = self.parse_if();
                self.depth -= 1;
                let inner = anidado?;
                span = span.to(inner.span());
                Some(Box::new(ElseBranch::If(inner)))
            } else {
                let blk = self.parse_block()?;
                span = span.to(blk.span);
                Some(Box::new(ElseBranch::Block(blk)))
            }
        } else {
            None
        };

        Ok(Expr::If {
            cond: Box::new(cond),
            then_branch,
            else_branch,
            span,
        })
    }

    fn parse_match(&mut self) -> PResult<Expr> {
        let kw = self.expect(&TokenKind::Match, "'match'")?;
        // En el escrutinio, un `Ident {` abre las ramas, no un registro.
        let scrutinee = self.with_no_struct_literal(|p| p.parse_expr())?;
        self.expect(&TokenKind::LBrace, "'{' para abrir las ramas del match")?;

        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::FatArrow, "'=>' tras el patrón")?;
            // El cuerpo de la rama es un contexto delimitado: un `Ident {` aquí
            // SÍ es un literal de registro (aunque el match esté en una posición
            // donde el flag no_struct_literal esté activo, p.ej. otro escrutinio).
            let body = self.allow_struct_literal(|p| p.parse_expr())?;
            let span = pattern.span().to(body.span());
            arms.push(MatchArm {
                pattern,
                body,
                span,
            });
            if !self.eat(&TokenKind::Comma) {
                break;
            }
        }

        let close = self.expect(&TokenKind::RBrace, "'}' para cerrar el match")?;
        Ok(Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            span: kw.span.to(close.span),
        })
    }

    fn parse_pattern(&mut self) -> PResult<Pattern> {
        let tok = self.peek().clone();
        match tok.kind {
            TokenKind::Underscore => {
                self.advance();
                Ok(Pattern::Wildcard { span: tok.span })
            }
            TokenKind::Ident(name) => {
                self.advance();
                Ok(Pattern::Binding {
                    name,
                    span: tok.span,
                })
            }
            TokenKind::Int(value) => {
                self.advance();
                Ok(Pattern::Int {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::Bool(value) => {
                self.advance();
                Ok(Pattern::Bool {
                    value,
                    span: tok.span,
                })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Pattern::Str {
                    value: s,
                    span: tok.span,
                })
            }
            _ => Err(SyntaxError::new("se esperaba un patrón", tok.span)),
        }
    }
}

/// Parsea el fuente de un hueco de plantilla como una expresión suelta,
/// desplazando los spans para que apunten a su posición real en el archivo.
fn parse_hueco(fuente: &str, offset: usize) -> PResult<Expr> {
    let tokens = crate::lexer::Lexer::tokenize(fuente).map_err(|e| desplazar(e, offset))?;
    let mut p = Parser::new(tokens);
    let mut expr = p.parse_expr().map_err(|e| desplazar(e, offset))?;
    // El sub-parser numera desde 0, así que hay que desplazar TODO el árbol: si
    // no, un error de tipos dentro del hueco señalaría el principio del archivo.
    desplazar_expr(&mut expr, offset);
    if !matches!(p.peek().kind, TokenKind::Eof) {
        return Err(SyntaxError::new(
            "sobra texto en el hueco de la plantilla",
            crate::span::Span::new(offset, offset + fuente.len()),
        ));
    }
    Ok(expr)
}

fn desplazar(e: SyntaxError, offset: usize) -> SyntaxError {
    SyntaxError::new(
        e.message,
        crate::span::Span::new(e.span.start + offset, e.span.end + offset),
    )
}

/// Suma `offset` a los spans de toda la expresión, en profundidad.
fn desplazar_expr(e: &mut Expr, offset: usize) {
    fn mover(s: &mut crate::span::Span, off: usize) {
        *s = crate::span::Span::new(s.start + off, s.end + off);
    }
    match e {
        Expr::Int { span, .. }
        | Expr::Float { span, .. }
        | Expr::Str { span, .. }
        | Expr::Bool { span, .. } => mover(span, offset),
        Expr::Ident { span, .. } => mover(span, offset),
        Expr::Unary { expr, span, .. } => {
            mover(span, offset);
            desplazar_expr(expr, offset);
        }
        Expr::Binary {
            left, right, span, ..
        } => {
            mover(span, offset);
            desplazar_expr(left, offset);
            desplazar_expr(right, offset);
        }
        Expr::Call { callee, args, span } => {
            mover(span, offset);
            desplazar_expr(callee, offset);
            for a in args {
                desplazar_expr(a, offset);
            }
        }
        Expr::Member { object, span, .. } => {
            mover(span, offset);
            desplazar_expr(object, offset);
        }
        Expr::Index {
            object,
            index,
            span,
        } => {
            mover(span, offset);
            desplazar_expr(object, offset);
            desplazar_expr(index, offset);
        }
        Expr::List { elements, span } => {
            mover(span, offset);
            for el in elements {
                desplazar_expr(el, offset);
            }
        }
        Expr::Record {
            fields,
            span,
            type_name_span,
            ..
        } => {
            mover(span, offset);
            if let Some(s) = type_name_span {
                mover(s, offset);
            }
            for f in fields {
                mover(&mut f.span, offset);
                desplazar_expr(&mut f.value, offset);
            }
        }
        Expr::Template { parts, span } => {
            mover(span, offset);
            for parte in parts {
                if let TemplatePart::Interp { expr, .. } = parte {
                    desplazar_expr(expr, offset);
                }
            }
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            span,
        } => {
            mover(span, offset);
            desplazar_expr(cond, offset);
            let _ = (then_branch, else_branch);
        }
        Expr::Match {
            scrutinee, span, ..
        } => {
            mover(span, offset);
            desplazar_expr(scrutinee, offset);
        }
    }
}
