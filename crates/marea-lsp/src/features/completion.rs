//! Completado: ofrece sugerencias según la posición del cursor.
//!
//! La lista es **plana**: el servidor no filtra por el prefijo ya tecleado, eso
//! lo hace el cliente. Lo contextual son las dos posiciones donde Marea no
//! admite cualquier cosa:
//!   - tras `@`, sólo `server`/`client`/`edge`/`session`;
//!   - dentro de `@server(...)`, sólo una política: `Public` o un tipo.
//!
//! Builtins: se consumen de `marea_types::builtins`, que es la fuente única.
//! Antes se duplicaban aquí como listas "espejo" y se desincronizaron (el
//! completado no ofrecía ninguno de los builtins del store), así que ahora se
//! importan del propio compilador y no pueden divergir.
//!
//! Nombres importados: salen del grafo ya resuelto, no de una heurística sobre
//! el texto. Ofrecer `getUser` sin saber si el módulo de al lado lo declara sería
//! ofrecer un error de compilación.

use lsp_types::{CompletionItem, CompletionItemKind, Position};

use crate::analysis::{collect_symbols, symbol_class, SymbolClass};
use crate::documents::Document;
use crate::programa::Salida;

/// Palabras clave del lenguaje, incluidas las de módulos (`import`/`from`) y las
/// del bucle (`for`/`in`).
const KEYWORDS: &[&str] = &[
    "fn", "let", "mut", "reactive", "type", "if", "else", "match", "return", "effect", "store",
    "for", "in", "import", "from", "true", "false",
];

/// Valores builtin: la tabla real del compilador.
const BUILTIN_VALUES: &[&str] = marea_types::builtins::VALUE_NAMES;

/// Tipos builtin: la tabla real del compilador.
const BUILTIN_TYPES: &[&str] = marea_types::builtins::TYPE_NAMES;

/// Anotaciones válidas tras `@`. `server`/`client`/`edge` son ESPEJO de
/// [`marea_syntax::ast::Location`]; `session` no es una ubicación sino la marca
/// de la función que traduce un token en una identidad, y el parser la acepta
/// en el mismo sitio.
const ANOTACIONES: &[&str] = &["server", "client", "edge", "session"];

/// Construye los items de completado para `textDocument/completion`.
pub fn completion(salida: &Salida, doc: &Document, position: Position) -> Vec<CompletionItem> {
    let offset = doc.line_index.position_to_offset(position, &doc.text);
    let entrada = salida.entrada();

    if en_anotacion(&doc.text, offset) {
        return ANOTACIONES
            .iter()
            .map(|name| item(name, CompletionItemKind::ENUM_MEMBER))
            .collect();
    }

    // Dentro de `@server(...)` va una política: `Public` —decir que no se exige
    // identidad, explícitamente— o el tipo que resuelve la `@session`. Se ofrecen
    // los tipos que el programa tiene a mano, que es de donde puede salir.
    if en_politica(&doc.text, offset) {
        let mut items = vec![item("Public", CompletionItemKind::CLASS)];
        for sym in collect_symbols(&entrada.modulo) {
            if sym.class == SymbolClass::Type {
                items.push(item(&sym.name, CompletionItemKind::CLASS));
            }
        }
        for (nombre, destino, item_ast) in salida.importados(entrada) {
            if matches!(symbol_class(item_ast), SymbolClass::Type) {
                items.push(importado(nombre, CompletionItemKind::CLASS, destino));
            }
        }
        return items;
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
    // Nombres de nivel superior del propio módulo. No se filtran duplicados con
    // los builtins: el cliente ya deduplica visualmente y la lista es de
    // referencia.
    for sym in collect_symbols(&entrada.modulo) {
        items.push(item(&sym.name, kind_de(sym.class)));
    }
    // Y lo que traen los `import`, con el archivo del que viene cada uno: sin
    // esto, escribir un programa de varios archivos es escribir de memoria.
    for (nombre, destino, item_ast) in salida.importados(entrada) {
        items.push(importado(nombre, kind_de(symbol_class(item_ast)), destino));
    }
    items
}

fn kind_de(class: SymbolClass) -> CompletionItemKind {
    match class {
        SymbolClass::Fn => CompletionItemKind::FUNCTION,
        SymbolClass::Type => CompletionItemKind::CLASS,
        SymbolClass::Let => CompletionItemKind::VARIABLE,
        SymbolClass::Store => CompletionItemKind::PROPERTY,
    }
}

/// ¿El cursor está escribiendo una anotación tras `@`?
///
/// Cierto cuando el byte inmediatamente anterior al cursor es `@`, o cuando
/// entre `@` y el cursor solo hay caracteres de identificador (la anotación a
/// medio teclear, p. ej. `@ser|`). Trabaja sobre los bytes previos al offset.
fn en_anotacion(text: &str, offset: usize) -> bool {
    let prefix = &text.as_bytes()[..offset.min(text.len())];
    // Retrocede sobre los caracteres de identificador ya tecleados.
    let mut i = prefix.len();
    while i > 0 && is_ident_byte(prefix[i - 1]) {
        i -= 1;
    }
    // Justo antes del identificador (posiblemente vacío) debe haber un `@`.
    if i == 0 || prefix[i - 1] != b'@' {
        return false;
    }
    abre_item(prefix, i - 1)
}

/// ¿El cursor está dentro de los paréntesis de una anotación, `@server(...|`?
fn en_politica(text: &str, offset: usize) -> bool {
    let prefix = &text.as_bytes()[..offset.min(text.len())];
    // Retrocede sobre lo que puede haber dentro del paréntesis antes del cursor:
    // el tipo a medio teclear y, si se ligó la identidad, el `u: ` de delante.
    let mut i = prefix.len();
    while i > 0 && es_de_la_politica(prefix[i - 1]) {
        i -= 1;
    }
    if i == 0 || prefix[i - 1] != b'(' {
        return false;
    }
    i -= 1;
    // Antes del paréntesis, el nombre de la anotación y su `@`.
    while i > 0 && is_ident_byte(prefix[i - 1]) {
        i -= 1;
    }
    if i == 0 || prefix[i - 1] != b'@' {
        return false;
    }
    abre_item(prefix, i - 1)
}

/// ¿El `@` en `pos` abre un item? Sólo si en su línea, antes de él, no hay más
/// que espacios. Así NO se dispara dentro de una cadena ("a@b") ni en medio de
/// una expresión.
fn abre_item(prefix: &[u8], pos: usize) -> bool {
    let mut j = pos;
    while j > 0 && prefix[j - 1] != b'\n' {
        if !prefix[j - 1].is_ascii_whitespace() {
            return false;
        }
        j -= 1;
    }
    true
}

/// ¿El byte forma parte de un identificador (letra ASCII, dígito o `_`)?
/// Basta con ASCII: las anotaciones de Marea son identificadores ASCII.
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// ¿El byte puede estar dentro del paréntesis de una anotación? Es el tipo, y
/// el `u: ` con el que se liga la identidad.
fn es_de_la_politica(b: u8) -> bool {
    is_ident_byte(b) || b == b' ' || b == b':'
}

/// Crea un `CompletionItem` mínimo: etiqueta + clase.
fn item(label: &str, kind: CompletionItemKind) -> CompletionItem {
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        ..CompletionItem::default()
    }
}

/// Un nombre que viene de otro módulo, con el archivo del que viene a la vista.
fn importado(
    label: &str,
    kind: CompletionItemKind,
    destino: &crate::programa::Archivo,
) -> CompletionItem {
    let de = destino
        .ruta
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "otro módulo".to_string());
    CompletionItem {
        label: label.to_string(),
        kind: Some(kind),
        detail: Some(format!("de {de}")),
        ..CompletionItem::default()
    }
}

#[cfg(test)]
mod tests {
    use super::{en_anotacion, en_politica};

    #[test]
    fn arroba_al_inicio_de_item_es_anotacion() {
        // '@' que abre un item (solo espacios antes en la línea).
        assert!(en_anotacion("@", 1));
        assert!(en_anotacion("@ser", 4));
        assert!(en_anotacion("  @cl", 5));
        assert!(en_anotacion("fn f() {}\n@ser", 14));
        assert!(en_anotacion("@sess", 5));
    }

    #[test]
    fn arroba_dentro_de_texto_no_es_anotacion() {
        // '@' precedido de no-espacios en la línea (cadena, email) NO dispara.
        assert!(!en_anotacion("let x = \"a@b", 12));
        assert!(!en_anotacion("correo@dominio", 14));
        assert!(!en_anotacion("print(x)", 8));
    }

    #[test]
    fn dentro_del_parentesis_de_la_anotacion_va_una_politica() {
        assert!(en_politica("@server(", 8));
        assert!(en_politica("@server(Pub", 11));
        assert!(en_politica("@server(u: Usua", 15));
        assert!(en_politica("@edge(", 6));
    }

    #[test]
    fn fuera_del_parentesis_no_va_una_politica() {
        assert!(!en_politica("@server", 7));
        assert!(!en_politica("print(x", 7));
        // Una llamada normal en medio de un cuerpo no es una anotación.
        assert!(!en_politica("    save(prod", 13));
    }
}
