//! Almacén de documentos abiertos: mantiene el texto y la versión de cada
//! archivo que el editor tiene abierto, indexado por URI.
//!
//! Cada [`Document`] guarda un [`LineIndex`] precomputado para traducir spans a
//! posiciones LSP sin re-escanear el texto en cada petición; al actualizar el
//! contenido el índice se reconstruye.
//!
//! La clave es `lsp_types::Uri` (newtype sobre `fluent_uri`), el tipo que el
//! protocolo usa para identificar recursos; implementa `Hash`, `Eq` y `Clone`,
//! así que sirve de clave de `HashMap`.

use std::collections::HashMap;

use lsp_types::Uri;

use crate::line_index::LineIndex;

/// Un documento abierto: su texto completo, la versión que reportó el editor y
/// el índice de líneas derivado del texto.
#[derive(Debug, Clone)]
pub struct Document {
    pub text: String,
    pub version: i32,
    pub line_index: LineIndex,
}

impl Document {
    /// Crea un documento a partir de su texto y versión, construyendo el índice.
    pub fn new(text: String, version: i32) -> Self {
        let line_index = LineIndex::new(&text);
        Document {
            text,
            version,
            line_index,
        }
    }

    /// Reemplaza el texto y la versión, reconstruyendo el índice de líneas.
    pub fn update(&mut self, text: String, version: i32) {
        self.line_index = LineIndex::new(&text);
        self.text = text;
        self.version = version;
    }
}

/// Almacén de los documentos abiertos, indexado por URI.
#[derive(Debug, Default)]
pub struct DocumentStore {
    documents: HashMap<Uri, Document>,
}

impl DocumentStore {
    /// Crea un almacén vacío.
    pub fn new() -> Self {
        DocumentStore::default()
    }

    /// Inserta o reemplaza por completo el documento de `uri` (apertura o
    /// sincronización full).
    pub fn open(&mut self, uri: Uri, text: String, version: i32) {
        self.documents.insert(uri, Document::new(text, version));
    }

    /// Actualiza el documento de `uri` con texto y versión nuevos. Si no existía
    /// (cambio antes de la apertura), lo crea.
    pub fn update(&mut self, uri: Uri, text: String, version: i32) {
        match self.documents.get_mut(&uri) {
            Some(doc) => doc.update(text, version),
            None => {
                self.documents.insert(uri, Document::new(text, version));
            }
        }
    }

    /// Elimina el documento de `uri` (cierre). Devuelve el documento si existía.
    pub fn close(&mut self, uri: &Uri) -> Option<Document> {
        self.documents.remove(uri)
    }

    /// Referencia al documento de `uri`, si está abierto.
    pub fn get(&self, uri: &Uri) -> Option<&Document> {
        self.documents.get(uri)
    }

    /// Recorre los documentos abiertos. Lo necesita el análisis de programa: un
    /// archivo del grafo puede estar abierto y sin guardar, y entonces lo que
    /// vale es el buffer, no lo que hay en el disco.
    pub fn iter(&self) -> impl Iterator<Item = (&Uri, &Document)> {
        self.documents.iter()
    }

    /// Número de documentos abiertos.
    pub fn len(&self) -> usize {
        self.documents.len()
    }

    /// ¿No hay documentos abiertos?
    pub fn is_empty(&self) -> bool {
        self.documents.is_empty()
    }
}
