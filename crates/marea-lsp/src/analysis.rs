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
//!   - si `parse` falla, se reporta **exactamente un** diagnóstico y NO se
//!     corre el chequeo de tipos (no hay AST que chequear);
//!   - si `parse` tiene éxito, se corren todos los chequeos de tipo y cada
//!     `TypeError` se mapea a un [`NeutralDiag`].

use marea_syntax::ast::{
    Block, ElseBranch, Expr, Item, Module, Param, Pattern, Stmt, Type,
};
use marea_syntax::parse_recovering;
use marea_syntax::span::Span;
use marea_types::check;

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

/// Analiza una fuente Marea: parsea y, si parsea, chequea tipos.
///
/// - Error de sintaxis → exactamente un diagnóstico (Error, sin código) y
///   `module` en `None`; el chequeo de tipos NO corre.
/// - Parseo correcto → cada `TypeError` se mapea a un diagnóstico (con su
///   código y notas) y `module` queda en `Some`.
pub fn analyze(src: &str) -> Analysis {
    // Parser con recuperación: módulo parcial + TODOS los errores de sintaxis.
    let (module, syntax_errors) = parse_recovering(src);
    let mut diagnostics: Vec<NeutralDiag> = syntax_errors
        .into_iter()
        .map(|err| NeutralDiag {
            severity: Severity::Error,
            span: err.span,
            code: None,
            message: err.message,
            notes: Vec::new(),
        })
        .collect();

    // El chequeo de tipos sólo corre si la SINTAXIS está limpia: sobre un AST
    // parcial produciría diagnósticos de tipos confusos (nombres que faltan por
    // los items descartados).
    if diagnostics.is_empty() {
        for te in check(&module) {
            diagnostics.push(NeutralDiag {
                severity: Severity::Error,
                span: te.span,
                code: Some(te.code),
                message: te.message,
                notes: te.notes,
            });
        }
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
        .filter_map(|item| match item {
            Item::Fn(f) => Some(Symbol {
                class: SymbolClass::Fn,
                name: f.name.clone(),
                span: f.span,
            }),
            Item::Type(t) => Some(Symbol {
                class: SymbolClass::Type,
                name: t.name.clone(),
                span: t.span,
            }),
            Item::Let(l) => Some(Symbol {
                class: SymbolClass::Let,
                name: l.name.clone(),
                span: l.span,
            }),
            // El `store T;` no aporta un símbolo con nombre al esquema.
            Item::Store { .. } => None,
        })
        .collect()
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
        Stmt::Expr(e) => e.span(),
    }
}

/// Encuentra el nodo más interno cuyo span contiene `offset`, recorriendo el
/// AST en profundidad (items → stmts → exprs/types/patterns).
///
/// Devuelve `None` si el offset no cae dentro de ningún nodo (p. ej. en espacio
/// en blanco entre items, o fuera del texto).
pub fn find_node_at(module: &Module, offset: usize) -> Option<Node<'_>> {
    for item in &module.items {
        match item {
            Item::Fn(f) => {
                if contains(f.span, offset) {
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
            Node::Item(item) => match item {
                Item::Fn(f) => f.span,
                Item::Type(t) => t.span,
                Item::Let(l) => l.span,
                Item::Store { span, .. } => *span,
            },
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
                find_in_pattern(&arm.pattern, offset)
                    .or_else(|| find_in_expr(&arm.body, offset))
            })
        }),
        Expr::Record { fields, .. } => {
            fields.iter().find_map(|fi| find_in_expr(&fi.value, offset))
        }
        Expr::List { elements, .. } => {
            elements.iter().find_map(|e| find_in_expr(e, offset))
        }
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
        Type::Union { variants, .. } => {
            variants.iter().find_map(|v| find_in_type(v, offset))
        }
        Type::Record { fields, .. } => {
            fields.iter().find_map(|f| find_in_type(&f.ty, offset))
        }
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
