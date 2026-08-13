//! Símbolos del documento: enumera los items de nivel superior (funciones,
//! tipos, `let` de módulo y almacenes) para la vista de esquema y la navegación
//! del editor.
//!
//! Se deriva por completo del AST: [`collect_symbols`] recorre el módulo y aquí
//! cada símbolo neutral se viste con el tipo `DocumentSymbol` del protocolo. La
//! lista es plana (sin jerarquía): los items de nivel superior de Marea no
//! anidan otros items.
//!
//! Sólo se listan los del documento abierto, no los de todo el programa: el
//! esquema es del archivo, y para lo importado está el ir-a-definición.

use lsp_types::DocumentSymbol;

use crate::analysis::collect_symbols;
use crate::conversions::symbol_to_document_symbol;
use crate::documents::Document;
use crate::programa::Salida;

/// Construye los símbolos de nivel superior del documento para
/// `textDocument/documentSymbol`.
pub fn document_symbols(salida: &Salida, doc: &Document) -> Vec<DocumentSymbol> {
    collect_symbols(&salida.entrada().modulo)
        .iter()
        .map(|sym| symbol_to_document_symbol(sym, &doc.line_index, &doc.text))
        .collect()
}
