//! Completado: ofrece sugerencias (palabras clave, builtins, tipos y nombres
//! de nivel superior del módulo) según la posición del cursor.
//!
//! La lista es **plana**: el servidor no filtra por el prefijo ya tecleado, eso
//! lo hace el cliente. La única decisión contextual es la ubicación tras `@`,
//! donde Marea solo admite `server`/`client`/`edge`.
//!
//! Builtins ESPEJO: los nombres se duplican aquí como listas propias (no se
//! importan de los crates del compilador) para que el servidor de lenguaje
//! quede aislado. Si el compilador cambia su tabla de builtins, estas listas
//! deben actualizarse a la par.

use lsp_types::{CompletionItem, CompletionItemKind, Position};

use crate::documents::Document;

/// Palabras clave del lenguaje (ESPEJO de la tabla del lexer).
const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "reactive", "type", "if", "else", "match", "return",
    "import", "effect",
];

/// Valores builtin (ESPEJO). Nota: incluye `NotFound` y NO incluye `Error`.
const BUILTIN_VALUES: &[&str] = &["print", "concat", "render", "db", "NotFound"];

/// Tipos builtin (ESPEJO).
const BUILTIN_TYPES: &[&str] = &["Int", "Float", "Bool", "String", "Record", "List"];

/// Ubicaciones válidas tras `@` (ESPEJO de [`marea_syntax::ast::Location`]).
const LOCATIONS: &[&str] = &["server", "client", "edge"];

/// Construye los items de completado para `textDocument/completion` en la
/// posición dada.
///
/// - Justo tras un `@` se ofrecen solo las ubicaciones (`server`/`client`/
///   `edge`), porque ahí el lenguaje no admite nada más.
/// - En cualquier otro punto se ofrece la lista plana: palabras clave +
///   builtins (valores y tipos) + nombres de nivel superior del módulo (si
///   parsea).
pub fn completion(doc: &Document, position: Position) -> Vec<CompletionItem> {
    let offset = doc.line_index.position_to_offset(position, &doc.text);

    if at_location_context(&doc.text, offset) {
        return LOCATIONS
            .iter()
            .map(|name| item(name, CompletionItemKind::ENUM_MEMBER))
            .collect();
    }

    let mut items = Vec::new();
    for kw in KEYWORDS {
        items.push(item(kw, CompletionItemKind::KEYWORD));
    }
    for value in BUILTIN_VALUES {
        items.push(item(value, CompletionItemKind::FUNCTION));
    }
    for ty in BUILTIN_TYPES {
        items.push(item(ty, CompletionItemKind::CLASS));
    }
    // Nombres de nivel superior del propio módulo (funciones y tipos), si el
    // documento parsea. No se filtran duplicados con los builtins: el cliente
    // ya deduplica visualmente y la lista es de referencia.
    if let Some(module) = crate::analyze(&doc.text).module {
        for sym in crate::analysis::collect_symbols(&module) {
            let kind = match sym.class {
                crate::analysis::SymbolClass::Fn => CompletionItemKind::FUNCTION,
                crate::analysis::SymbolClass::Type => CompletionItemKind::CLASS,
                crate::analysis::SymbolClass::Let => CompletionItemKind::VARIABLE,
            };
            items.push(item(&sym.name, kind));
        }
    }
    items
}

/// ¿El cursor está escribiendo una ubicación tras `@`?
///
/// Cierto cuando el byte inmediatamente anterior al cursor es `@`, o cuando
/// entre `@` y el cursor solo hay caracteres de identificador (la ubicación a
/// medio teclear, p. ej. `@ser|`). Trabaja sobre los bytes previos al offset.
fn at_location_context(text: &str, offset: usize) -> bool {
    let prefix = &text.as_bytes()[..offset.min(text.len())];
    // Retrocede sobre los caracteres de identificador ya tecleados.
    let mut i = prefix.len();
    while i > 0 && is_ident_byte(prefix[i - 1]) {
        i -= 1;
    }
    // Justo antes del identificador (posiblemente vacío) debe haber un `@`.
    i > 0 && prefix[i - 1] == b'@'
}

/// ¿El byte forma parte de un identificador (letra ASCII, dígito o `_`)?
/// Basta con ASCII: las ubicaciones de Marea son identificadores ASCII.
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Crea un `CompletionItem` mínimo: etiqueta + clase.
fn item(label: &str, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        ..CompletionItem::default()
    }
}
