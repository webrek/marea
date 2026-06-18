//! Capacidades del servidor: describe al cliente qué funciones soporta el
//! servidor (diagnósticos, símbolos, completado, ir-a-definición, hover).
//!
//! Los diagnósticos no se anuncian con una capacidad: se publican de motu
//! propio (`textDocument/publishDiagnostics`) y siempre están disponibles.

use lsp_types::{
    CompletionOptions, HoverProviderCapability, OneOf, PositionEncodingKind, ServerCapabilities,
    TextDocumentSyncCapability, TextDocumentSyncKind,
};

/// Construye las capacidades que el servidor anuncia en el `initialize`.
///
/// - `position_encoding` = UTF-16: la columna (`character`) de cada posición se
///   mide en unidades de código UTF-16, lo mismo que asume [`crate::line_index`].
/// - `text_document_sync` = FULL: en cada cambio el cliente reenvía el texto
///   completo del documento (sin diffs incrementales).
/// - `document_symbol_provider`: árbol de símbolos del documento (esquema).
/// - `completion_provider`: completado disparado además por `@` y `.`.
/// - `definition_provider`: ir a definición.
/// - `hover_provider`: información al pasar el cursor.
pub fn server_capabilities() -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(PositionEncodingKind::UTF16),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
        document_symbol_provider: Some(OneOf::Left(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec!["@".to_string(), ".".to_string()]),
            ..CompletionOptions::default()
        }),
        definition_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        ..ServerCapabilities::default()
    }
}
