//! Ir a definición. Salta a donde se declara el nombre bajo el cursor, esté
//! donde esté: en el mismo ámbito, en el mismo archivo, o al otro lado de un
//! `import`.
//!
//! El orden de búsqueda es el del verificador, y no es un detalle: primero los
//! ámbitos léxicos (parámetros —incluidos los de un cierre—, `let`, variable de
//! un `for`, nombre ligado por un `match`, la identidad de la política), luego
//! lo que declara el módulo, y sólo entonces lo que trajo un `import`. Un
//! parámetro llamado igual que una función tapa a la función, y el salto tiene
//! que ir donde iría el valor.

use lsp_types::{GotoDefinitionResponse, Location, Position, Uri};
use marea_syntax::ast::{Expr, Type};
use marea_syntax::span::Span;

use crate::analysis::{
    find_import_at, find_node_at, item_span, resolver_ligadura, top_level_index, Node,
};
use crate::conversions::{span_to_location, Uris};
use crate::documents::Document;
use crate::line_index::LineIndex;
use crate::programa::{Archivo, Salida};

/// Resuelve `textDocument/definition` en la posición dada.
///
/// Devuelve `None` si el cursor no cae sobre un nombre referenciable o si el
/// nombre no corresponde a ninguna declaración conocida (una variable de la que
/// no hay nada que decir, o un builtin, que no se declara en ningún `.mar`).
pub fn goto_definition(
    salida: &Salida,
    doc: &Document,
    uri: &Uri,
    uris: &Uris,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let entrada = salida.entrada();
    let offset = doc.line_index.position_to_offset(position, &doc.text);

    // Sobre el propio `import`: sobre un nombre, salta a su declaración; sobre
    // la ruta, al principio del archivo que nombra.
    if let Some((imp, nombre)) = find_import_at(&entrada.modulo, offset) {
        let (destino, span) = match nombre {
            Some(n) => {
                let (destino, item) = salida.declaracion_importada(entrada, &n.name)?;
                (destino, item_span(item))
            }
            None => (salida.destino(entrada, &imp.path)?, Span::new(0, 0)),
        };
        return localizar(uris, uri, destino, span).map(GotoDefinitionResponse::Scalar);
    }

    let name = match find_node_at(&entrada.modulo, offset)? {
        Node::Expr(Expr::Ident { name, .. }) => name,
        Node::Type(Type::Name { name, .. }) => name,
        _ => return None,
    };

    // 1. Ámbitos léxicos del propio archivo.
    if let Some(l) = resolver_ligadura(&entrada.modulo, offset, name) {
        return localizar(uris, uri, entrada, l.span).map(GotoDefinitionResponse::Scalar);
    }
    // 2. Declaraciones de nivel superior del propio archivo.
    if let Some(span) = top_level_index(&entrada.modulo).get(name) {
        return localizar(uris, uri, entrada, *span).map(GotoDefinitionResponse::Scalar);
    }
    // 3. Al otro lado de un `import`.
    let (destino, item) = salida.declaracion_importada(entrada, name)?;
    localizar(uris, uri, destino, item_span(item)).map(GotoDefinitionResponse::Scalar)
}

/// Construye la [`Location`] de un span dentro de `archivo`.
///
/// El índice de líneas se reconstruye a partir del fuente del archivo, que puede
/// no ser el documento abierto: un salto a otro módulo aterriza en un archivo
/// que el editor quizá ni tiene abierto.
fn localizar(uris: &Uris, doc_uri: &Uri, archivo: &Archivo, span: Span) -> Option<Location> {
    let uri = match &archivo.ruta {
        Some(ruta) => uris.uri(ruta)?,
        // Un documento sin archivo detrás: sólo puede ser el propio buffer.
        None => doc_uri.clone(),
    };
    let index = LineIndex::new(&archivo.fuente);
    Some(span_to_location(uri, span, &index, &archivo.fuente))
}
