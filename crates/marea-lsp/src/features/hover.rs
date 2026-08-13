//! Hover: muestra información SINTÁCTICA del nodo bajo el cursor, en markdown.
//!
//! No infiere tipos: solo reconstruye, a partir del AST, lo que ya está escrito
//! en la fuente. Tres formas de hover:
//!   - sobre una función (su nombre, su firma o un uso de ella) → la firma
//!     reconstruida `fn nombre(p: T, ...) -> R`, con `@ubicacion` si la tiene;
//!   - sobre un tipo (`Type::Name`) o una declaración `type` → `type N = ...`
//!     si N es un alias del módulo, o el tipo formateado en otro caso;
//!   - sobre un parámetro → `param: Tipo`.

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};
use marea_syntax::ast::{Expr, FnDecl, Item, Location, Module, Param, Type, TypeDecl};

use crate::analysis::{find_node_at, Node};
use crate::documents::Document;

/// Resuelve `textDocument/hover` en la posición dada.
///
/// Devuelve `None` si el documento no parsea o si el nodo bajo el cursor no
/// tiene una representación sintáctica útil (p. ej. un literal numérico).
pub fn hover(doc: &Document, position: Position) -> Option<Hover> {
    let module = crate::analyze(&doc.text).module?;
    let offset = doc.line_index.position_to_offset(position, &doc.text);
    let node = find_node_at(&module, offset)?;

    let markdown = hover_markdown(&module, node)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    })
}

/// Construye el cuerpo markdown del hover para `node`, o `None` si no aplica.
fn hover_markdown(module: &Module, node: Node<'_>) -> Option<String> {
    match node {
        // Uso de un nombre: si es una función o un alias de tipo de nivel
        // superior, se muestra su declaración.
        Node::Expr(Expr::Ident { name, .. }) => declaration_markdown(module, name),
        Node::Type(Type::Name { name, .. }) => {
            // Un alias de tipo del módulo muestra su definición; si no, el tipo
            // formateado tal cual (p. ej. un builtin o un genérico).
            declaration_markdown(module, name)
                .or_else(|| Some(code_block(&render_type(node_type(node)?))))
        }
        // Sobre el nombre de un parámetro → `nombre: Tipo`.
        Node::Param(p) => Some(code_block(&format!("{}: {}", p.name, render_type(&p.ty)))),
        // El propio item: función → firma; type → `type N = ...`.
        Node::Item(Item::Fn(f)) => Some(code_block(&fn_signature(f))),
        Node::Item(Item::Type(t)) => Some(code_block(&type_decl(t))),
        Node::Item(Item::Let(l)) => Some(code_block(&format!("let {}", l.name))),
        // Otros tipos compuestos (unión, registro) se formatean directos.
        Node::Type(t) => Some(code_block(&render_type(t))),
        _ => None,
    }
}

/// Tipo asociado a un `Node::Type`, para reusar `render_type`.
fn node_type<'a>(node: Node<'a>) -> Option<&'a Type> {
    match node {
        Node::Type(t) => Some(t),
        _ => None,
    }
}

/// Si `name` es una declaración de nivel superior del módulo, devuelve su hover
/// (firma de función o `type N = ...`); si no, `None`.
fn declaration_markdown(module: &Module, name: &str) -> Option<String> {
    for item in &module.items {
        match item {
            Item::Fn(f) if f.name == name => return Some(code_block(&fn_signature(f))),
            Item::Type(t) if t.name == name => return Some(code_block(&type_decl(t))),
            _ => {}
        }
    }
    None
}

/// Reconstruye la firma de una función: `@ubicacion fn nombre(p: T, ...) -> R`.
/// El cuerpo no se incluye (hover sintáctico de la firma).
fn fn_signature(f: &FnDecl) -> String {
    let mut sig = String::new();
    if let Some(loc) = f.location {
        sig.push('@');
        sig.push_str(location_name(loc));
        sig.push('\n');
    }
    sig.push_str("fn ");
    sig.push_str(&f.name);
    sig.push('(');
    sig.push_str(&render_params(&f.params));
    sig.push(')');
    if let Some(rt) = &f.return_type {
        sig.push_str(" -> ");
        sig.push_str(&render_type(rt));
    }
    sig
}

/// Renderiza la lista de parámetros como `n1: T1, n2: T2`.
fn render_params(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reconstruye una declaración de tipo: `type N = <tipo>`.
fn type_decl(t: &TypeDecl) -> String {
    format!("type {} = {}", t.name, render_type(&t.aliased))
}

/// Nombre textual de una ubicación.
fn location_name(loc: Location) -> &'static str {
    match loc {
        Location::Server => "server",
        Location::Client => "client",
        Location::Edge => "edge",
    }
}

/// Formatea un `Type` del AST como texto fuente aproximado.
///
/// - `Name { args }` → `N` o `N<A, B>`;
/// - `Union { variants }` → `A | B | C`;
/// - `Record { fields }` → `{ n: T, ... }`.
fn render_type(ty: &Type) -> String {
    match ty {
        Type::Name { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner = args.iter().map(render_type).collect::<Vec<_>>().join(", ");
                format!("{name}<{inner}>")
            }
        }
        Type::Union { variants, .. } => variants
            .iter()
            .map(render_type)
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Record { fields, .. } => {
            let inner = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, render_type(&f.ty)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {inner} }}")
        }
    }
}

/// Envuelve un fragmento de código en un bloque markdown con lenguaje `marea`.
fn code_block(code: &str) -> String {
    format!("```marea\n{code}\n```")
}
