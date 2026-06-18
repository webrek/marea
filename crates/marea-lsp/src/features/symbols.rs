//! Símbolos del documento: enumera los items de nivel superior (funciones,
//! tipos, lets) para la vista de esquema y la navegación del editor.
//!
//! Se deriva por completo del AST: [`collect_symbols`] recorre el módulo y
//! aquí cada símbolo neutral se viste con el tipo `DocumentSymbol` del
//! protocolo. La lista es plana (sin jerarquía): los items de nivel superior
//! de Marea no anidan otros items.

use lsp_types::DocumentSymbol;

use crate::analysis::collect_symbols;
use crate::conversions::symbol_to_document_symbol;
use crate::documents::Document;

/// Construye los símbolos de nivel superior del documento para
/// `textDocument/documentSymbol`.
///
/// Si el documento no parsea (error de sintaxis) no hay módulo del que extraer
/// símbolos y se devuelve una lista vacía.
pub fn document_symbols(doc: &Document) -> Vec<DocumentSymbol> {
    let Some(module) = crate::analyze(&doc.text).module else {
        return Vec::new();
    };
    collect_symbols(&module)
        .iter()
        .map(|sym| symbol_to_document_symbol(sym, &doc.line_index, &doc.text))
        .collect()
}
