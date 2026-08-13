//! Análisis de una fuente Marea: orquesta el parseo (`marea_syntax`) y el
//! chequeo de tipos (`marea_types`) y reúne sus errores como diagnósticos.
//!
//! Este módulo es **puro**: no conoce `lsp_types`. Trabaja con `Span` en bytes
//! y un diagnóstico neutral ([`NeutralDiag`]) que el módulo `conversions`
//! traduce al modelo del protocolo. Así la lógica de análisis se prueba sin
//! arrastrar el protocolo.
//!
//! El flujo es fail-fast en sintaxis y acumulativo en tipos, espejo del
//! contrato del compilador:
//!   - si `parse` falla, se reportan los errores de sintaxis y NO se corre el
//!     chequeo de tipos (sobre un AST parcial produciría nombres sin resolver);
//!   - si `parse` tiene éxito, se corren todos los chequeos de tipo y cada
//!     `TypeError` se mapea a un [`NeutralDiag`].
//!
//! Aquí se mira UN archivo. Cuando el documento tiene `import`, el archivo deja
//! de ser el programa y quien manda es [`crate::programa`], que resuelve el
//! grafo y llama a `check_program`; este módulo le presta los helpers de AST
//! (búsqueda de nodos, ámbitos léxicos) y la traducción de errores.

use marea_syntax::ast::{
    Block, ElseBranch, Expr, Import, ImportName, Item, LetStmt, Module, Param, Pattern, Stmt,
    TemplatePart, Type,
};
use marea_syntax::parse_recovering;
use marea_syntax::span::Span;
use marea_syntax::SyntaxError;
use marea_types::{check, TypeError};

/// Severidad de un diagnóstico, independiente del protocolo. Se mapea 1:1 a
/// `lsp_types::DiagnosticSeverity` en `conversions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

/// Diagnóstico neutral: un error o aviso ubicado por `Span` (bytes), sin
/// dependencia del protocolo LSP.
#[derive(Debug, Clone, PartialEq)]
pub struct NeutralDiag {
    pub severity: Severity,
    /// Ubicación del diagnóstico en offsets de byte.
    pub span: Span,
    /// Código estable del error (`E_...`). Los errores de sintaxis no tienen.
    pub code: Option<String>,
    pub message: String,
    /// Líneas extra de contexto (heredadas de `TypeError::notes`).
    pub notes: Vec<String>,
}

/// Resultado de analizar una fuente: el AST (si parseó) y sus diagnósticos.
#[derive(Debug, Clone)]
pub struct Analysis {
    /// `Some` si el parseo tuvo éxito; `None` si hubo error de sintaxis.
    pub module: Option<Module>,
    pub diagnostics: Vec<NeutralDiag>,
}

/// Traduce un error de sintaxis a diagnóstico. No lleva código: el parser no
/// los numera.
pub fn diag_de_sintaxis(err: SyntaxError) -> NeutralDiag {
    NeutralDiag {
        severity: Severity::Error,
        span: err.span,
        code: None,
        message: err.message,
        notes: Vec::new(),
    }
}

/// Traduce un error de tipos a diagnóstico, conservando su código y sus notas.
pub fn diag_de_tipos(err: TypeError) -> NeutralDiag {
    NeutralDiag {
        severity: Severity::Error,
        span: err.span,
        code: Some(err.code),
        message: err.message,
        notes: err.notes,
    }
}

/// Analiza una fuente Marea: parsea y, si parsea, chequea tipos.
///
/// - Error de sintaxis → sus diagnósticos (Error, sin código) y el módulo
///   PARCIAL que el parser pudo recuperar; el chequeo de tipos NO corre.
/// - Parseo correcto → cada `TypeError` se mapea a un diagnóstico (con su
///   código y notas).
pub fn analyze(src: &str) -> Analysis {
    // Parser con recuperación: módulo parcial + TODOS los errores de sintaxis.
    let (module, syntax_errors) = parse_recovering(src);
    let mut diagnostics: Vec<NeutralDiag> =
        syntax_errors.into_iter().map(diag_de_sintaxis).collect();

    // El chequeo de tipos sólo corre si la SINTAXIS está limpia: sobre un AST
    // parcial produciría diagnósticos de tipos confusos (nombres que faltan por
    // los items descartados).
    if diagnostics.is_empty() {
        diagnostics.extend(check(&module).into_iter().map(diag_de_tipos));
    }

    Analysis {
        module: Some(module),
        diagnostics,
    }
}

// ===================== Helpers puros sobre el AST =====================

/// Clase de un símbolo de nivel superior, para el árbol de símbolos del editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolClass {
    Fn,
    Type,
    Let,
    /// `store nombre: Tipo;`. Tiene nombre desde que un módulo puede declarar
    /// varios almacenes, así que es un símbolo como los demás.
    Store,
}

/// Un símbolo recolectado del módulo: su clase, nombre y span del item completo.
///
/// El AST no guarda el span del *nombre* de un item, así que el span es el del
/// item entero (sirve tanto de rango como de rango de selección).
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    pub class: SymbolClass,
    pub name: String,
    pub span: Span,
}

/// Recolecta los símbolos de nivel superior del módulo en orden de aparición.
pub fn collect_symbols(module: &Module) -> Vec<Symbol> {
    module
        .items
        .iter()
        .map(|item| Symbol {
            class: symbol_class(item),
            name: item.name().to_string(),
            span: item_span(item),
        })
        .collect()
}

/// Clase de símbolo de un item.
pub fn symbol_class(item: &Item) -> SymbolClass {
    match item {
        Item::Fn(_) => SymbolClass::Fn,
        Item::Type(_) => SymbolClass::Type,
        Item::Let(_) => SymbolClass::Let,
        Item::Store { .. } => SymbolClass::Store,
    }
}

/// Span del item completo. `Item` no expone un `span()` propio.
pub fn item_span(item: &Item) -> Span {
    match item {
        Item::Fn(f) => f.span,
        Item::Type(t) => t.span,
        Item::Let(l) => l.span,
        Item::Store { span, .. } => *span,
    }
}

/// Mapa nombre → span de la declaración de nivel superior, para "ir a
/// definición". Ante nombres duplicados conserva la primera declaración, igual
/// que el chequeo de tipos (que registra solo la primera firma).
pub fn top_level_index(module: &Module) -> std::collections::HashMap<String, Span> {
    let mut index = std::collections::HashMap::new();
    for sym in collect_symbols(module) {
        index.entry(sym.name).or_insert(sym.span);
    }
    index
}

/// ¿El offset de byte cae dentro del span `[start, end)`?
fn contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset < span.end
}

/// Span de un `Stmt`. `Stmt` no expone un método `span()`, así que se hace por
/// match manual sobre cada variante (incluidas `Assign` y `Effect`).
fn stmt_span(stmt: &Stmt) -> Span {
    match stmt {
        Stmt::Let(l) => l.span,
        Stmt::Return { span, .. } => *span,
        Stmt::Assign { span, .. } => *span,
        Stmt::Effect { span, .. } => *span,
        Stmt::For { span, .. } => *span,
        Stmt::Expr(e) => e.span(),
    }
}

/// Encuentra el nodo más interno cuyo span contiene `offset`, recorriendo el
/// AST en profundidad (items → stmts → exprs/types/patterns).
///
/// Devuelve `None` si el offset no cae dentro de ningún nodo (p. ej. en espacio
/// en blanco entre items, o fuera del texto). Los `import` NO se ven desde
/// aquí: viven fuera de `Module::items`, y para ellos está [`find_import_at`].
pub fn find_node_at(module: &Module, offset: usize) -> Option<Node<'_>> {
    for item in &module.items {
        match item {
            Item::Fn(f) => {
                if contains(f.span, offset) {
                    // La política (`@server(u: Usuario)`) es un tipo escrito en
                    // la anotación: el cursor puede caer en ella igual que en el
                    // tipo de un parámetro.
                    if let Some(pol) = &f.politica {
                        if let Some(n) = find_in_type(pol, offset) {
                            return Some(n);
                        }
                    }
                    // Busca primero en los parámetros (nombre y tipo), el tipo de
                    // retorno y el cuerpo; si nada más interno encaja, el item.
                    for p in &f.params {
                        if contains(p.name_span, offset) {
                            return Some(Node::Param(p));
                        }
                        if let Some(n) = find_in_type(&p.ty, offset) {
                            return Some(n);
                        }
                    }
                    if let Some(rt) = &f.return_type {
                        if let Some(n) = find_in_type(rt, offset) {
                            return Some(n);
                        }
                    }
                    if let Some(n) = find_in_block(&f.body, offset) {
                        return Some(n);
                    }
                    return Some(Node::Item(item));
                }
            }
            Item::Type(t) => {
                if contains(t.span, offset) {
                    if let Some(n) = find_in_type(&t.aliased, offset) {
                        return Some(n);
                    }
                    return Some(Node::Item(item));
                }
            }
            Item::Let(l) => {
                if contains(l.span, offset) {
                    if let Some(ty) = &l.ty {
                        if let Some(n) = find_in_type(ty, offset) {
                            return Some(n);
                        }
                    }
                    if let Some(n) = find_in_expr(&l.value, offset) {
                        return Some(n);
                    }
                    return Some(Node::Item(item));
                }
            }
            Item::Store { ty, span, .. } => {
                if contains(*span, offset) {
                    if let Some(n) = find_in_type(ty, offset) {
                        return Some(n);
                    }
                    return Some(Node::Item(item));
                }
            }
        }
    }
    None
}

/// El `import` bajo el cursor y, si el cursor está sobre uno de los nombres
/// entre llaves, ese nombre.
///
/// Existe aparte de [`find_node_at`] porque los `import` viven en
/// `Module::imports`, no en `Module::items`: quien recorre un módulo suelto no
/// tiene por qué verlos, pero el editor sí.
pub fn find_import_at(module: &Module, offset: usize) -> Option<(&Import, Option<&ImportName>)> {
    let imp = module.imports.iter().find(|i| contains(i.span, offset))?;
    Some((imp, imp.names.iter().find(|n| contains(n.span, offset))))
}

/// Un nodo del AST localizado por [`find_node_at`]. Cubre las categorías que
/// tienen identidad propia para las features (hover, goto, símbolos).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Node<'a> {
    Item(&'a Item),
    Stmt(&'a Stmt),
    Expr(&'a Expr),
    Type(&'a Type),
    Pattern(&'a Pattern),
    Param(&'a Param),
}

impl Node<'_> {
    /// Span del nodo, sea cual sea su categoría.
    pub fn span(&self) -> Span {
        match self {
            Node::Item(item) => item_span(item),
            Node::Stmt(s) => stmt_span(s),
            Node::Expr(e) => e.span(),
            Node::Type(t) => t.span(),
            Node::Pattern(p) => p.span(),
            Node::Param(p) => p.span,
        }
    }
}

fn find_in_block(block: &Block, offset: usize) -> Option<Node<'_>> {
    if !contains(block.span, offset) {
        return None;
    }
    for stmt in &block.stmts {
        if contains(stmt_span(stmt), offset) {
            return Some(find_in_stmt(stmt, offset));
        }
    }
    None
}

/// Desciende por un `Stmt` hasta el nodo más interno; si nada más interno
/// encaja, devuelve el propio `Stmt`.
fn find_in_stmt(stmt: &Stmt, offset: usize) -> Node<'_> {
    match stmt {
        Stmt::For { iter, body, .. } => {
            if let Some(n) = find_in_expr(iter, offset) {
                return n;
            }
            for s in &body.stmts {
                if contains(stmt_span(s), offset) {
                    return find_in_stmt(s, offset);
                }
            }
            Node::Stmt(stmt)
        }
        Stmt::Let(l) => {
            if let Some(ty) = &l.ty {
                if let Some(n) = find_in_type(ty, offset) {
                    return n;
                }
            }
            if let Some(n) = find_in_expr(&l.value, offset) {
                return n;
            }
            Node::Stmt(stmt)
        }
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                if let Some(n) = find_in_expr(e, offset) {
                    return n;
                }
            }
            Node::Stmt(stmt)
        }
        Stmt::Assign { value, .. } => {
            if let Some(n) = find_in_expr(value, offset) {
                return n;
            }
            Node::Stmt(stmt)
        }
        Stmt::Effect { body, .. } => {
            if let Some(n) = find_in_block(body, offset) {
                return n;
            }
            Node::Stmt(stmt)
        }
        Stmt::Expr(e) => find_in_expr(e, offset).unwrap_or(Node::Stmt(stmt)),
    }
}

/// Desciende por una `Expr`. Devuelve `None` si el offset no cae en ella; si
/// cae pero ningún hijo es más interno, devuelve la propia expresión.
fn find_in_expr(expr: &Expr, offset: usize) -> Option<Node<'_>> {
    if !contains(expr.span(), offset) {
        return None;
    }
    let inner = match expr {
        Expr::Int { .. }
        | Expr::Float { .. }
        | Expr::Str { .. }
        | Expr::Bool { .. }
        | Expr::Ident { .. } => None,
        Expr::Template { parts, .. } => parts.iter().find_map(|p| match p {
            TemplatePart::Interp { expr, .. } => find_in_expr(expr, offset),
            _ => None,
        }),
        // Un cierre es una declaración sin nombre: sus parámetros, su tipo de
        // retorno y su cuerpo son nodos como los del `fn` que lo contiene, y el
        // cursor tiene que poder caer en cualquiera de los tres.
        Expr::Fn {
            params,
            return_type,
            body,
            ..
        } => {
            let en_params = params.iter().find_map(|p| {
                if contains(p.name_span, offset) {
                    Some(Node::Param(p))
                } else {
                    find_in_type(&p.ty, offset)
                }
            });
            en_params
                .or_else(|| return_type.as_ref().and_then(|rt| find_in_type(rt, offset)))
                .or_else(|| find_in_block(body, offset))
        }
        Expr::Unary { expr: inner, .. } => find_in_expr(inner, offset),
        Expr::Binary { left, right, .. } => {
            find_in_expr(left, offset).or_else(|| find_in_expr(right, offset))
        }
        Expr::Call { callee, args, .. } => find_in_expr(callee, offset)
            .or_else(|| args.iter().find_map(|a| find_in_expr(a, offset))),
        Expr::Member { object, .. } => find_in_expr(object, offset),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => find_in_expr(cond, offset)
            .or_else(|| find_in_block(then_branch, offset))
            .or_else(|| match else_branch {
                Some(eb) => find_in_else(eb, offset),
                None => None,
            }),
        Expr::Match {
            scrutinee, arms, ..
        } => find_in_expr(scrutinee, offset).or_else(|| {
            arms.iter().find_map(|arm| {
                find_in_pattern(&arm.pattern, offset).or_else(|| find_in_expr(&arm.body, offset))
            })
        }),
        Expr::Record { fields, .. } => fields.iter().find_map(|fi| find_in_expr(&fi.value, offset)),
        Expr::List { elements, .. } => elements.iter().find_map(|e| find_in_expr(e, offset)),
        Expr::Index { object, index, .. } => {
            find_in_expr(object, offset).or_else(|| find_in_expr(index, offset))
        }
    };
    Some(inner.unwrap_or(Node::Expr(expr)))
}

fn find_in_else(eb: &ElseBranch, offset: usize) -> Option<Node<'_>> {
    match eb {
        ElseBranch::Block(b) => find_in_block(b, offset),
        ElseBranch::If(e) => find_in_expr(e, offset),
    }
}

/// Desciende por un `Type`. Devuelve `None` si el offset no cae en él; si cae
/// pero ningún hijo es más interno, devuelve el propio tipo.
fn find_in_type(ty: &Type, offset: usize) -> Option<Node<'_>> {
    if !contains(ty.span(), offset) {
        return None;
    }
    let inner = match ty {
        Type::Name { args, .. } => args.iter().find_map(|a| find_in_type(a, offset)),
        Type::Union { variants, .. } => variants.iter().find_map(|v| find_in_type(v, offset)),
        Type::Record { fields, .. } => fields.iter().find_map(|f| find_in_type(&f.ty, offset)),
    };
    Some(inner.unwrap_or(Node::Type(ty)))
}

fn find_in_pattern(pat: &Pattern, offset: usize) -> Option<Node<'_>> {
    if contains(pat.span(), offset) {
        Some(Node::Pattern(pat))
    } else {
        None
    }
}

// ===================== Ámbitos léxicos =====================

/// Qué clase de nombre local es una ligadura. Cambia el texto del hover:
/// `param u: Usuario` no dice lo mismo que `for p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaseLigadura {
    /// Parámetro de una función con nombre.
    Param,
    /// Parámetro de un cierre.
    ParamCierre,
    /// La identidad de `@server(u: Usuario)`. NO es un parámetro: no viaja en
    /// la llamada, la inyecta el runtime tras resolver el token.
    Identidad,
    Let,
    Reactive,
    /// Variable de un `for x in xs`.
    For,
    /// Índice de un `for x, i in xs`.
    Indice,
    /// Nombre ligado por un patrón de `match`.
    Patron,
}

/// Un nombre ligado en algún ámbito léxico que envuelve al cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct Ligadura<'a> {
    pub nombre: &'a str,
    /// Span al que salta "ir a la definición". Es el del nombre cuando el AST lo
    /// guarda (parámetros, `for`, patrones) y el de la sentencia entera cuando
    /// no (un `let`).
    pub span: Span,
    /// El tipo TAL COMO SE ESCRIBIÓ, si se escribió. Este módulo no infiere: un
    /// `let x = 1;` se queda sin tipo, y es lo que se muestra.
    pub ty: Option<&'a Type>,
    pub clase: ClaseLigadura,
}

/// Todos los nombres locales visibles en `offset`, del más externo al más
/// interno. El orden es lo que permite que un parámetro de cierre tape al de la
/// función que lo contiene, igual que la pila de ámbitos del verificador.
pub fn ligaduras_en(module: &Module, offset: usize) -> Vec<Ligadura<'_>> {
    let mut out = Vec::new();
    for item in &module.items {
        match item {
            Item::Fn(f) if contains(f.span, offset) => {
                // `@server(u: Usuario)` liga `u` en todo el cuerpo. El AST no
                // guarda el span de ese nombre —sólo el del tipo—, así que el
                // salto aterriza en la política, que es donde se escribió.
                if let (Some(nombre), Some(pol)) = (&f.identidad_bind, &f.politica) {
                    out.push(Ligadura {
                        nombre: nombre.as_str(),
                        span: pol.span(),
                        ty: Some(pol),
                        clase: ClaseLigadura::Identidad,
                    });
                }
                for p in &f.params {
                    out.push(Ligadura {
                        nombre: p.name.as_str(),
                        span: p.name_span,
                        ty: Some(&p.ty),
                        clase: ClaseLigadura::Param,
                    });
                }
                ligaduras_en_bloque(&f.body, offset, &mut out);
            }
            // Una global de módulo puede inicializarse con un cierre, y dentro
            // de él vuelve a haber ámbitos.
            Item::Let(l) if contains(l.span, offset) => {
                ligaduras_en_expr(&l.value, offset, &mut out);
            }
            _ => {}
        }
    }
    out
}

/// La ligadura visible en `offset` que declara `nombre`, o `None` si el nombre
/// no es local (será una declaración de nivel superior, un import o un builtin).
pub fn resolver_ligadura<'a>(
    module: &'a Module,
    offset: usize,
    nombre: &str,
) -> Option<Ligadura<'a>> {
    ligaduras_en(module, offset)
        .into_iter()
        .rev()
        .find(|l| l.nombre == nombre)
}

fn ligadura_de_let(l: &LetStmt) -> Ligadura<'_> {
    Ligadura {
        nombre: l.name.as_str(),
        span: l.span,
        ty: l.ty.as_ref(),
        clase: if l.reactive {
            ClaseLigadura::Reactive
        } else {
            ClaseLigadura::Let
        },
    }
}

fn ligaduras_en_bloque<'a>(block: &'a Block, offset: usize, out: &mut Vec<Ligadura<'a>>) {
    if !contains(block.span, offset) {
        return;
    }
    for stmt in &block.stmts {
        let s = stmt_span(stmt);
        // Sentencia ya cerrada antes del cursor: sólo aporta lo que declara.
        // Que el `let` se añada AQUÍ y no al entrar en él es lo que hace que su
        // propio nombre no esté en ámbito dentro de su inicializador.
        if s.end <= offset {
            if let Stmt::Let(l) = stmt {
                out.push(ligadura_de_let(l));
            }
            continue;
        }
        if !contains(s, offset) {
            break; // sentencia posterior al cursor: no aporta nada
        }
        match stmt {
            Stmt::Let(l) => ligaduras_en_expr(&l.value, offset, out),
            Stmt::For {
                var,
                var_span,
                index,
                index_span,
                iter,
                body,
                ..
            } => {
                // La lista se evalúa FUERA del bucle: ahí `var` no existe aún.
                if contains(iter.span(), offset) {
                    ligaduras_en_expr(iter, offset, out);
                } else {
                    out.push(Ligadura {
                        nombre: var.as_str(),
                        span: *var_span,
                        ty: None,
                        clase: ClaseLigadura::For,
                    });
                    if let (Some(n), Some(sp)) = (index.as_ref(), index_span.as_ref()) {
                        out.push(Ligadura {
                            nombre: n.as_str(),
                            span: *sp,
                            ty: None,
                            clase: ClaseLigadura::Indice,
                        });
                    }
                    ligaduras_en_bloque(body, offset, out);
                }
            }
            Stmt::Return { value: Some(e), .. } => ligaduras_en_expr(e, offset, out),
            Stmt::Return { value: None, .. } => {}
            Stmt::Assign { value, .. } => ligaduras_en_expr(value, offset, out),
            Stmt::Effect { body, .. } => ligaduras_en_bloque(body, offset, out),
            Stmt::Expr(e) => ligaduras_en_expr(e, offset, out),
        }
        break;
    }
}

fn ligaduras_en_expr<'a>(expr: &'a Expr, offset: usize, out: &mut Vec<Ligadura<'a>>) {
    if !contains(expr.span(), offset) {
        return;
    }
    match expr {
        // Un cierre abre ámbito. Sin esto, el hover dentro de su cuerpo no
        // sabría de qué habla: sus parámetros no están en ningún otro sitio.
        Expr::Fn { params, body, .. } => {
            for p in params {
                out.push(Ligadura {
                    nombre: p.name.as_str(),
                    span: p.name_span,
                    ty: Some(&p.ty),
                    clase: ClaseLigadura::ParamCierre,
                });
            }
            ligaduras_en_bloque(body, offset, out);
        }
        Expr::Match {
            scrutinee, arms, ..
        } => {
            ligaduras_en_expr(scrutinee, offset, out);
            for arm in arms {
                if !contains(arm.span, offset) {
                    continue;
                }
                if let Pattern::Binding { name, span } = &arm.pattern {
                    out.push(Ligadura {
                        nombre: name.as_str(),
                        span: *span,
                        ty: None,
                        clase: ClaseLigadura::Patron,
                    });
                }
                ligaduras_en_expr(&arm.body, offset, out);
            }
        }
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            ligaduras_en_expr(cond, offset, out);
            ligaduras_en_bloque(then_branch, offset, out);
            if let Some(eb) = else_branch {
                ligaduras_en_else(eb, offset, out);
            }
        }
        Expr::Unary { expr: inner, .. } => ligaduras_en_expr(inner, offset, out),
        Expr::Binary { left, right, .. } => {
            ligaduras_en_expr(left, offset, out);
            ligaduras_en_expr(right, offset, out);
        }
        Expr::Call { callee, args, .. } => {
            ligaduras_en_expr(callee, offset, out);
            for a in args {
                ligaduras_en_expr(a, offset, out);
            }
        }
        Expr::Member { object, .. } => ligaduras_en_expr(object, offset, out),
        Expr::Record { fields, .. } => {
            for f in fields {
                ligaduras_en_expr(&f.value, offset, out);
            }
        }
        Expr::List { elements, .. } => {
            for e in elements {
                ligaduras_en_expr(e, offset, out);
            }
        }
        Expr::Index { object, index, .. } => {
            ligaduras_en_expr(object, offset, out);
            ligaduras_en_expr(index, offset, out);
        }
        Expr::Template { parts, .. } => {
            for p in parts {
                if let TemplatePart::Interp { expr, .. } = p {
                    ligaduras_en_expr(expr, offset, out);
                }
            }
        }
        _ => {}
    }
}

fn ligaduras_en_else<'a>(eb: &'a ElseBranch, offset: usize, out: &mut Vec<Ligadura<'a>>) {
    match eb {
        ElseBranch::Block(b) => ligaduras_en_bloque(b, offset, out),
        ElseBranch::If(e) => ligaduras_en_expr(e, offset, out),
    }
}
