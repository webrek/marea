//! Ir a definición: dado un identificador bajo el cursor, localiza el item de
//! nivel superior donde se declara, en el MISMO documento (single-file).
//!
//! Solo resuelve referencias a nombres de nivel superior: un `Expr::Ident` (uso
//! de una función o un `let` global) o un `Type::Name` (uso de un tipo). El
//! nombre se busca en [`top_level_index`]; si no está (variable local,
//! parámetro o builtin), se devuelve `None`.

use lsp_types::{GotoDefinitionResponse, Position, Uri};
use marea_syntax::ast::{Expr, Type};

use crate::analysis::{find_node_at, top_level_index, Node};
use crate::conversions::span_to_location;
use crate::documents::Document;

/// Resuelve `textDocument/definition` en la posición dada.
///
/// Devuelve `None` si el documento no parsea, si el cursor no cae sobre un
/// nombre referenciable, o si el nombre no corresponde a ninguna declaración de
/// nivel superior del documento.
pub fn goto_definition(
    doc: &Document,
    uri: &Uri,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let module = crate::analyze(&doc.text).module?;
    let offset = doc.line_index.position_to_offset(position, &doc.text);

    let name = match find_node_at(&module, offset)? {
        Node::Expr(Expr::Ident { name, .. }) => name,
        Node::Type(Type::Name { name, .. }) => name,
        _ => return None,
    };

    let index = top_level_index(&module);
    let span = *index.get(name)?;
    let location = span_to_location(uri.clone(), span, &doc.line_index, &doc.text);
    Some(GotoDefinitionResponse::Scalar(location))
}
