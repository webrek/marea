//! Conversiones entre el modelo del compilador (`Span` en bytes, errores) y el
//! modelo del protocolo (`Range`, `Position`, `Diagnostic`).
//!
//! Este es el **único** puente del crate hacia `lsp_types`: el módulo `analysis`
//! produce datos neutrales y aquí se visten con los tipos del protocolo, usando
//! [`LineIndex`] para traducir offsets de byte a posiciones LSP.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::{
    Diagnostic, DiagnosticSeverity, DocumentSymbol, Location, NumberOrString, SymbolKind, Uri,
};
use marea_syntax::span::Span;

use crate::analysis::{NeutralDiag, Severity, Symbol, SymbolClass};
use crate::documents::DocumentStore;
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
        // No hay `SymbolKind::STORE`; el almacén es el estado persistente del
        // módulo, así que se enseña como una propiedad suya.
        SymbolClass::Store => SymbolKind::PROPERTY,
    }
}

/// Construye una [`Location`] del protocolo para un span dentro de un documento.
pub fn span_to_location(uri: Uri, span: Span, index: &LineIndex, text: &str) -> Location {
    Location {
        uri,
        range: index.span_to_range(span, text),
    }
}

// ===================== URIs y rutas =====================
//
// Con varios archivos el servidor deja de hablar sólo del documento abierto:
// publica diagnósticos en otro archivo y devuelve saltos hacia él, así que hay
// que traducir en las dos direcciones entre el URI del protocolo y la ruta del
// sistema de archivos.
//
// La traducción se hace sobre el TEXTO del URI, obtenido por serde. `lsp-types`
// ha cambiado de biblioteca de URIs entre versiones (antes `url`, ahora
// `fluent-uri`) y con ella los métodos del tipo; lo que no cambia es que el
// protocolo lo transporta como una cadena JSON, que es de donde se parte aquí.

/// El URI como texto.
pub fn uri_texto(uri: &Uri) -> String {
    match serde_json::to_value(uri) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

/// La ruta del sistema de archivos de un URI `file:`, o `None` si el documento
/// no vive en el disco (`untitled:`, por ejemplo). No comprueba que exista.
pub fn ruta_de_uri(uri: &Uri) -> Option<PathBuf> {
    let texto = uri_texto(uri);
    let resto = texto.strip_prefix("file://")?;
    // Autoridad vacía (`file:///x`) o el `localhost` que algún cliente escribe.
    let resto = resto.strip_prefix("localhost").unwrap_or(resto);
    if !resto.starts_with('/') {
        return None;
    }
    let decodificada = percent_decode(resto)?;
    // `file:///C:/x` en Windows: ante la letra de unidad, la barra inicial sobra.
    let bytes = decodificada.as_bytes();
    if cfg!(windows) && bytes.len() >= 3 && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
        return Some(PathBuf::from(&decodificada[1..]));
    }
    Some(PathBuf::from(decodificada))
}

/// El URI `file:` de una ruta absoluta.
pub fn uri_de_ruta(ruta: &Path) -> Option<Uri> {
    let ruta = ruta.to_str()?;
    let mut texto = String::from("file://");
    if !ruta.starts_with('/') {
        texto.push('/');
    }
    for ch in ruta.chars() {
        match ch {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '/' | '-' | '.' | '_' | '~' | ':' => texto.push(ch),
            otro => {
                let mut buf = [0u8; 4];
                for b in otro.encode_utf8(&mut buf).as_bytes() {
                    texto.push_str(&format!("%{b:02X}"));
                }
            }
        }
    }
    Uri::from_str(&texto).ok()
}

fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            match (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                (Some(alto), Some(bajo)) => {
                    out.push(alto * 16 + bajo);
                    i += 3;
                    continue;
                }
                _ => return None,
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

fn hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Traduce rutas canónicas a URIs, prefiriendo aquel con el que el editor abrió
/// el archivo.
///
/// Importa: si para un archivo YA abierto fabricásemos un URI distinto del que
/// usó el cliente (otro escapado, otro enlace simbólico), el editor lo trataría
/// como un segundo documento y los diagnósticos se verían duplicados en uno y
/// fantasma en el otro.
pub struct Uris {
    por_ruta: HashMap<PathBuf, Uri>,
}

impl Uris {
    /// Construye el mapa a partir de los documentos abiertos.
    pub fn de(store: &DocumentStore) -> Self {
        let mut por_ruta = HashMap::new();
        for (uri, _) in store.iter() {
            if let Some(canonica) = ruta_de_uri(uri).and_then(|p| p.canonicalize().ok()) {
                por_ruta.insert(canonica, uri.clone());
            }
        }
        Uris { por_ruta }
    }

    /// El URI de una ruta canónica: el del editor si el archivo está abierto, y
    /// si no uno construido.
    pub fn uri(&self, ruta: &Path) -> Option<Uri> {
        match self.por_ruta.get(ruta) {
            Some(uri) => Some(uri.clone()),
            None => uri_de_ruta(ruta),
        }
    }
}
