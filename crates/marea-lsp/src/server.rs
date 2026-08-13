//! Bucle principal del servidor: negociación de capacidades y despacho de
//! peticiones y notificaciones a los manejadores de cada función.
//!
//! El servidor es síncrono al estilo de rust-analyzer: un solo hilo lee del
//! `receiver` de la [`Connection`], actualiza el estado y responde por el
//! `sender`. No hay `async`/`tokio`.
//!
//! Flujo:
//!   1. Handshake `initialize`/`initialized` (lo gestiona `Connection::initialize`,
//!      que responde con nuestras capacidades y espera el `initialized`).
//!   2. Bucle sobre los mensajes entrantes:
//!      - Notificaciones `didOpen`/`didChange`/`didClose` actualizan el
//!        [`DocumentStore`] y disparan la publicación de diagnósticos.
//!      - El `shutdown` (request) se atiende con `Connection::handle_shutdown`,
//!        que responde y espera el `exit` antes de cerrar el bucle.
//!      - Las peticiones de funciones (símbolos, completado, definición, hover)
//!        se delegan; en este paso devuelven vacío/`None` (el Paso 5 las llena).

use lsp_server::{Connection, ExtractError, Message, Request, RequestId, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _,
    PublishDiagnostics,
};
use lsp_types::request::{Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest};
use lsp_types::{
    CompletionResponse, Diagnostic, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentSymbolResponse, PublishDiagnosticsParams, Uri,
};
use serde::Serialize;

use crate::analysis::analyze;
use crate::capabilities::server_capabilities;
use crate::conversions::neutral_to_diagnostic;
use crate::documents::DocumentStore;
use crate::features::{completion, goto, hover, symbols};
use crate::line_index::LineIndex;

/// Error de tipo borrado que devuelven los puntos de entrada del crate.
type BoxError = Box<dyn std::error::Error + Sync + Send>;

/// Arranca el servidor sobre una [`Connection`] ya construida.
///
/// Negocia capacidades, atiende el bucle de mensajes y retorna cuando el
/// cliente cierra (tras `shutdown`/`exit` o al desconectarse el canal).
pub fn run(connection: Connection) -> Result<(), BoxError> {
    let caps = serde_json::to_value(server_capabilities())?;
    // Espera el `initialize`, responde con las capacidades y consume el
    // `initialized`. Los parámetros del cliente no se usan todavía.
    let _params = connection.initialize(caps)?;

    main_loop(&connection)?;
    Ok(())
}

/// Bucle principal: consume mensajes hasta que el canal se cierra o se completa
/// el `shutdown`.
fn main_loop(connection: &Connection) -> Result<(), BoxError> {
    let mut store = DocumentStore::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                // `handle_shutdown` responde al `shutdown` y espera el `exit`;
                // devuelve `true` cuando el cierre se completó.
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                handle_request(connection, &store, req)?;
            }
            Message::Notification(note) => {
                handle_notification(connection, &mut store, note)?;
            }
            // Las respuestas del cliente (a peticiones que hiciéramos nosotros)
            // no se esperan en este paso; se ignoran.
            Message::Response(_) => {}
        }
    }
    Ok(())
}

/// Despacha una notificación entrante. `didOpen`/`didChange`/`didClose`
/// actualizan el almacén y republican diagnósticos; el resto se ignora.
fn handle_notification(
    connection: &Connection,
    store: &mut DocumentStore,
    note: lsp_server::Notification,
) -> Result<(), BoxError> {
    match note.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = note.extract(DidOpenTextDocument::METHOD)?;
            let doc = params.text_document;
            store.open(doc.uri.clone(), doc.text, doc.version);
            publish_diagnostics(connection, store, &doc.uri, Some(doc.version))?;
        }
        DidChangeTextDocument::METHOD => {
            let params: DidChangeTextDocumentParams =
                note.extract(DidChangeTextDocument::METHOD)?;
            let uri = params.text_document.uri;
            let version = params.text_document.version;
            // Sincronización FULL: el último cambio trae el texto completo. Si
            // viniera vacío (cliente no conforme), se deja el texto previo.
            if let Some(change) = params.content_changes.into_iter().last() {
                store.update(uri.clone(), change.text, version);
                publish_diagnostics(connection, store, &uri, Some(version))?;
            }
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = note.extract(DidCloseTextDocument::METHOD)?;
            let uri = params.text_document.uri;
            store.close(&uri);
            // Al cerrar, se limpian los diagnósticos del documento: lista vacía.
            send_diagnostics(connection, uri, None, Vec::new())?;
        }
        _ => {}
    }
    Ok(())
}

/// Despacha una petición de función a su manejador. Cada manejador deriva su
/// respuesta del documento abierto (si lo está) y del AST. El `shutdown` ya se
/// trató antes de llegar aquí.
fn handle_request(
    connection: &Connection,
    store: &DocumentStore,
    req: Request,
) -> Result<(), BoxError> {
    let req = match dispatch::<DocumentSymbolRequest, _>(connection, req, |params| {
        let uri = params.text_document.uri;
        store
            .get(&uri)
            .map(|doc| DocumentSymbolResponse::Nested(symbols::document_symbols(doc)))
    })? {
        Ok(()) => return Ok(()),
        Err(req) => req,
    };
    let req = match dispatch::<Completion, _>(connection, req, |params| {
        let pos = params.text_document_position;
        store
            .get(&pos.text_document.uri)
            .map(|doc| CompletionResponse::Array(completion::completion(doc, pos.position)))
    })? {
        Ok(()) => return Ok(()),
        Err(req) => req,
    };
    let req = match dispatch::<GotoDefinition, _>(connection, req, |params| {
        let pos = params.text_document_position_params;
        let uri = pos.text_document.uri;
        store
            .get(&uri)
            .and_then(|doc| goto::goto_definition(doc, &uri, pos.position))
    })? {
        Ok(()) => return Ok(()),
        Err(req) => req,
    };
    let req = match dispatch::<HoverRequest, _>(connection, req, |params| {
        let pos = params.text_document_position_params;
        store
            .get(&pos.text_document.uri)
            .and_then(|doc| hover::hover(doc, pos.position))
    })? {
        Ok(()) => return Ok(()),
        Err(req) => req,
    };

    // Método no soportado: responde con error de método no encontrado para no
    // dejar la petición colgada.
    let resp = Response::new_err(
        req.id,
        lsp_server::ErrorCode::MethodNotFound as i32,
        format!("método no soportado: {}", req.method),
    );
    connection.sender.send(Message::Response(resp))?;
    Ok(())
}

/// Si `req` corresponde al método `R`, ejecuta `handler` con sus parámetros,
/// envía la respuesta serializada y devuelve `Ok(Ok(()))`. Si el método no
/// coincide, devuelve `Ok(Err(req))` para que el llamador pruebe el siguiente.
///
/// El `handler` devuelve un `Option<T>`: `None` serializa a `null` (resultado
/// vacío válido para estas respuestas opcionales, p. ej. documento no abierto o
/// nada que resolver).
fn dispatch<R, T>(
    connection: &Connection,
    req: Request,
    handler: impl FnOnce(R::Params) -> Option<T>,
) -> Result<Result<(), Request>, BoxError>
where
    R: lsp_types::request::Request,
    T: Serialize,
{
    // Captura el id ANTES de extract (en JsonError se pierde el request).
    let id = req.id.clone();
    match req.extract::<R::Params>(R::METHOD) {
        Ok((id, params)) => {
            respond(connection, id, handler(params))?;
            Ok(Ok(()))
        }
        Err(ExtractError::MethodMismatch(req)) => Ok(Err(req)),
        Err(ExtractError::JsonError { method, error }) => {
            // Params malformados: responde un error POR PETICIÓN y sigue vivo.
            // Antes esto tumbaba todo el servidor.
            let resp = Response::new_err(
                id,
                lsp_server::ErrorCode::InvalidParams as i32,
                format!("parámetros inválidos para {method}: {error}"),
            );
            connection.sender.send(Message::Response(resp))?;
            Ok(Ok(()))
        }
    }
}

/// Responde a una petición con el resultado serializado. `None` se envía como
/// `null` (resultado vacío válido para respuestas opcionales).
fn respond<T: Serialize>(
    connection: &Connection,
    id: RequestId,
    result: Option<T>,
) -> Result<(), BoxError> {
    let value = match result {
        Some(r) => serde_json::to_value(r)?,
        None => serde_json::Value::Null,
    };
    let resp = Response::new_ok(id, value);
    connection.sender.send(Message::Response(resp))?;
    Ok(())
}

/// Analiza el documento de `uri` (si está abierto) y publica sus diagnósticos.
fn publish_diagnostics(
    connection: &Connection,
    store: &DocumentStore,
    uri: &Uri,
    version: Option<i32>,
) -> Result<(), BoxError> {
    let Some(doc) = store.get(uri) else {
        return Ok(());
    };
    let analysis = analyze(&doc.text);
    let index = LineIndex::new(&doc.text);
    let diagnostics: Vec<Diagnostic> = analysis
        .diagnostics
        .iter()
        .map(|d| neutral_to_diagnostic(d, &index, &doc.text))
        .collect();
    send_diagnostics(connection, uri.clone(), version, diagnostics)
}

/// Envía una notificación `textDocument/publishDiagnostics` con la lista dada
/// (posiblemente vacía, para limpiar).
fn send_diagnostics(
    connection: &Connection,
    uri: Uri,
    version: Option<i32>,
    diagnostics: Vec<Diagnostic>,
) -> Result<(), BoxError> {
    let params = PublishDiagnosticsParams {
        uri,
        diagnostics,
        version,
    };
    let note = lsp_server::Notification::new(PublishDiagnostics::METHOD.to_string(), params);
    connection.sender.send(Message::Notification(note))?;
    Ok(())
}
