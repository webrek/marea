//! Parser de descenso recursivo con un sub-parser Pratt para expresiones.
//!
//! Consume la lista de tokens del lexer y produce un `Module`. Los niveles:
//!   - `parse_module` / `parse_item`  -> declaraciones de nivel superior
//!   - `parse_block` / `parse_stmt`   -> sentencias
//!   - `parse_bin_expr` (Pratt)       -> expresiones por precedencia
//!   - `parse_type`                   -> tipos, incluidas las uniones `A | B`

use crate::ast::*;
use crate::error::SyntaxError;
use crate::token::{Token, TokenKind};

type PResult<T> = Result<T, SyntaxError>;

pub struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    pub fn new(tokens: Vec<Token>) -> Self {
        Parser { tokens, pos: 0 }
    }

    /// Punto de entrada: tokens -> módulo.
    pub fn parse_module(tokens: Vec<Token>) -> PResult<Module> {
        let mut p = Parser::new(tokens);
        let mut items = Vec::new();
        while !p.at_eof() {
            items.push(p.parse_item()?);
        }
        Ok(Module { items })
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
            Err(SyntaxError::new(format!("se esperaba {}", what), self.peek().span))
        }
    }

    fn expect_ident(&mut self, what: &str) -> PResult<(String, crate::span::Span)> {
        match self.peek_kind().clone() {
            TokenKind::Ident(name) => {
                let span = self.advance().span;
                Ok((name, span))
            }
            _ => Err(SyntaxError::new(format!("se esperaba {}", what), self.peek().span)),
        }
    }

    // --- items ---

    fn parse_item(&mut self) -> PResult<Item> {
        let location = self.parse_location()?;
        match self.peek_kind() {
            TokenKind::Fn => Ok(Item::Fn(self.parse_fn(location)?)),
            TokenKind::Type if location.is_none() => Ok(Item::Type(self.parse_type_decl()?)),
            TokenKind::Let | TokenKind::Reactive if location.is_none() => {
                Ok(Item::Let(self.parse_let()?))
            }
            _ if location.is_some() => Err(SyntaxError::new(
                "los atributos de ubicación (@server/@client/@edge) solo aplican a funciones",
                self.peek().span,
            )),
            _ => Err(SyntaxError::new(
                "se esperaba un elemento de nivel superior: fn, type o let",
                self.peek().span,
            )),
        }
    }

    fn parse_location(&mut self) -> PResult<Option<Location>> {
        if !self.check(&TokenKind::At) {
            return Ok(None);
        }
        let at = self.advance();
        let (name, span) = self.expect_ident("nombre de ubicación tras '@'")?;
        let loc = match name.as_str() {
            "server" => Location::Server,
            "client" => Location::Client,
            "edge" => Location::Edge,
            other => {
                return Err(SyntaxError::new(
                    format!("ubicación desconocida '@{}'; usa @server, @client o @edge", other),
                    at.span.to(span),
                ))
            }
        };
        Ok(Some(loc))
    }

    fn parse_fn(&mut self, location: Option<Location>) -> PResult<FnDecl> {
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
            location,
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
        Ok(Param { name, ty, span })
    }

    fn parse_type_decl(&mut self) -> PResult<TypeDecl> {
        let kw = self.expect(&TokenKind::Type, "'type'")?;
        let (name, _) = self.expect_ident("nombre del tipo")?;
        self.expect(&TokenKind::Eq, "'=' en la declaración de tipo")?;
        let aliased = self.parse_type()?;
        let semi = self.expect(&TokenKind::Semicolon, "';' al final de la declaración de tipo")?;
        Ok(TypeDecl {
            name,
            aliased,
            span: kw.span.to(semi.span),
        })
    }

    // --- tipos ---

    fn parse_type(&mut self) -> PResult<Type> {
        let first = self.parse_type_primary()?;
        if !self.check(&TokenKind::Pipe) {
            return Ok(first);
        }
        let mut variants = vec![first];
        while self.eat(&TokenKind::Pipe) {
            variants.push(self.parse_type_primary()?);
        }
        let span = variants.first().unwrap().span().to(variants.last().unwrap().span());
        Ok(Type::Union { variants, span })
    }

    fn parse_type_primary(&mut self) -> PResult<Type> {
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
        match self.peek_kind() {
            TokenKind::Let | TokenKind::Reactive => Ok(Stmt::Let(self.parse_let()?)),
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

    fn parse_let(&mut self) -> PResult<LetStmt> {
        let (reactive, kw_span) = if self.check(&TokenKind::Reactive) {
            (true, self.advance().span)
        } else {
            (false, self.expect(&TokenKind::Let, "'let' o 'reactive'")?.span)
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
                            args.push(self.parse_expr()?);
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
                Ok(Expr::Int { value, span: tok.span })
            }
            TokenKind::Float(value) => {
                self.advance();
                Ok(Expr::Float { value, span: tok.span })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Expr::Str { value: s, span: tok.span })
            }
            TokenKind::Bool(value) => {
                self.advance();
                Ok(Expr::Bool { value, span: tok.span })
            }
            TokenKind::Ident(name) => {
                self.advance();
                Ok(Expr::Ident { name, span: tok.span })
            }
            TokenKind::LParen => {
                self.advance();
                let inner = self.parse_expr()?;
                self.expect(&TokenKind::RParen, "')'")?;
                Ok(inner)
            }
            TokenKind::If => self.parse_if(),
            TokenKind::Match => self.parse_match(),
            _ => Err(SyntaxError::new("se esperaba una expresión", tok.span)),
        }
    }

    fn parse_if(&mut self) -> PResult<Expr> {
        let kw = self.expect(&TokenKind::If, "'if'")?;
        let cond = self.parse_expr()?;
        let then_branch = self.parse_block()?;
        let mut span = kw.span.to(then_branch.span);

        let else_branch = if self.eat(&TokenKind::Else) {
            if self.check(&TokenKind::If) {
                let inner = self.parse_if()?;
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
        let scrutinee = self.parse_expr()?;
        self.expect(&TokenKind::LBrace, "'{' para abrir las ramas del match")?;

        let mut arms = Vec::new();
        while !self.check(&TokenKind::RBrace) && !self.at_eof() {
            let pattern = self.parse_pattern()?;
            self.expect(&TokenKind::FatArrow, "'=>' tras el patrón")?;
            let body = self.parse_expr()?;
            let span = pattern.span().to(body.span());
            arms.push(MatchArm { pattern, body, span });
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
                Ok(Pattern::Binding { name, span: tok.span })
            }
            TokenKind::Int(value) => {
                self.advance();
                Ok(Pattern::Int { value, span: tok.span })
            }
            TokenKind::Bool(value) => {
                self.advance();
                Ok(Pattern::Bool { value, span: tok.span })
            }
            TokenKind::Str(s) => {
                self.advance();
                Ok(Pattern::Str { value: s, span: tok.span })
            }
            _ => Err(SyntaxError::new("se esperaba un patrón", tok.span)),
        }
    }
}
