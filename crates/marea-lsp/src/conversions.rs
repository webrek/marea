//! Conversiones entre el modelo del compilador (`Span` en bytes, errores) y el
//! modelo del protocolo (`Range`, `Position`, `Diagnostic`).
//!
//! Este es el **único** puente del crate hacia `lsp_types`: el módulo `analysis`
//! produce datos neutrales y aquí se visten con los tipos del protocolo, usando
//! [`LineIndex`] para traducir offsets de byte a posiciones LSP.

use lsp_types::{
    Diagnostic, DiagnosticSeverity, DocumentSymbol, Location, NumberOrString, SymbolKind, Uri,
};
use marea_syntax::span::Span;

use crate::analysis::{NeutralDiag, Severity, Symbol, SymbolClass};
use crate::line_index::LineIndex;

/// Traduce un diagnóstico neutral a un [`Diagnostic`] del protocolo.
///
/// - `range` se calcula con [`LineIndex::span_to_range`];
/// - `source` se fija en `"marea"` para que el editor agrupe los diagnósticos;
/// - el `code` neutral (un `String` como `E_...`) se envuelve en
///   `NumberOrString::String`;
/// - las `notes` se concatenan al mensaje (el protocolo no tiene un canal
///   estándar para notas sueltas; `related_information` exige ubicaciones que
///   aquí no tenemos).
pub fn neutral_to_diagnostic(diag: &NeutralDiag, index: &LineIndex, text: &str) -> Diagnostic {
    let range = index.span_to_range(diag.span, text);

    let mut message = diag.message.clone();
    for note in &diag.notes {
        message.push_str("\nnota: ");
        message.push_str(note);
    }

    Diagnostic {
        range,
        severity: Some(severity_to_lsp(diag.severity)),
        code: diag.code.clone().map(NumberOrString::String),
        source: Some("marea".to_string()),
        message,
        ..Diagnostic::default()
    }
}

/// Traduce la severidad neutral a la del protocolo.
fn severity_to_lsp(severity: Severity) -> DiagnosticSeverity {
    match severity {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
        Severity::Information => DiagnosticSeverity::INFORMATION,
        Severity::Hint => DiagnosticSeverity::HINT,
    }
}

/// Traduce un símbolo neutral a un [`DocumentSymbol`] del protocolo.
///
/// El AST no guarda el span del nombre del item, así que `range` y
/// `selection_range` son ambos el span del item completo (deben ser iguales o
/// estar contenidos uno en otro, y aquí coinciden).
#[allow(deprecated)] // `DocumentSymbol::deprecated` está deprecado pero es un campo obligatorio del struct.
pub fn symbol_to_document_symbol(symbol: &Symbol, index: &LineIndex, text: &str) -> DocumentSymbol {
    let range = index.span_to_range(symbol.span, text);
    DocumentSymbol {
        name: symbol.name.clone(),
        detail: None,
        kind: symbol_kind(symbol.class),
        tags: None,
        deprecated: None,
        range,
        selection_range: range,
        children: None,
    }
}

/// Mapea la clase de símbolo de Marea al `SymbolKind` del protocolo.
fn symbol_kind(class: SymbolClass) -> SymbolKind {
    match class {
        SymbolClass::Fn => SymbolKind::FUNCTION,
        SymbolClass::Type => SymbolKind::INTERFACE,
        SymbolClass::Let => SymbolKind::VARIABLE,
    }
}

/// Construye una [`Location`] del protocolo para un span dentro de un documento.
pub fn span_to_location(uri: Uri, span: Span, index: &LineIndex, text: &str) -> Location {
    Location {
        uri,
        range: index.span_to_range(span, text),
    }
}
