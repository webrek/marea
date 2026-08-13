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
//!      - Las peticiones de funciones se resuelven contra el PROGRAMA del
//!        documento, no contra el documento suelto.
//!
//! El estado del bucle son tres cosas: los documentos abiertos, la caché de
//! programa ([`crate::programa::Cache`]) y el conjunto de archivos a los que se
//! publicó la última vez, que hace falta para limpiar los diagnósticos de un
//! archivo cuando deja de formar parte de cualquier programa abierto.

use std::collections::{HashMap, HashSet};

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

use crate::analysis::NeutralDiag;
use crate::capabilities::server_capabilities;
use crate::conversions::{neutral_to_diagnostic, ruta_de_uri, uri_texto, Uris};
use crate::documents::{Document, DocumentStore};
use crate::features::{completion, goto, hover, symbols};
use crate::line_index::LineIndex;
use crate::programa::{Abiertos, Cache, Salida};

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
    let mut cache = Cache::new();
    let mut publicados: HashSet<Uri> = HashSet::new();

    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                // `handle_shutdown` responde al `shutdown` y espera el `exit`;
                // devuelve `true` cuando el cierre se completó.
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }
                handle_request(connection, &store, &mut cache, req)?;
            }
            Message::Notification(note) => {
                handle_notification(connection, &mut store, &mut cache, &mut publicados, note)?;
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
    cache: &mut Cache,
    publicados: &mut HashSet<Uri>,
    note: lsp_server::Notification,
) -> Result<(), BoxError> {
    match note.method.as_str() {
        DidOpenTextDocument::METHOD => {
            let params: DidOpenTextDocumentParams = note.extract(DidOpenTextDocument::METHOD)?;
            let doc = params.text_document;
            let uri = doc.uri.clone();
            store.open(doc.uri, doc.text, doc.version);
            publicar(connection, store, cache, publicados, Some(&uri))?;
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
                publicar(connection, store, cache, publicados, Some(&uri))?;
            }
        }
        DidCloseTextDocument::METHOD => {
            let params: DidCloseTextDocumentParams = note.extract(DidCloseTextDocument::METHOD)?;
            let uri = params.text_document.uri;
            store.close(&uri);
            cache.olvidar(&uri_texto(&uri));
            // Republicar es lo que limpia: el archivo cerrado ya no es la
            // entrada de ningún programa, así que si nadie más lo cubre se le
            // manda la lista vacía.
            publicar(connection, store, cache, publicados, None)?;
        }
        _ => {}
    }
    Ok(())
}

/// Analiza el documento de `uri` con la caché de programa.
///
/// Devuelve `None` si el documento no está abierto. La ruta se canonicaliza
/// porque es la identidad de un módulo: dos URIs distintos que llegan al mismo
/// archivo tienen que resolver al mismo nodo del grafo.
fn analizar<'c, 's>(
    store: &'s DocumentStore,
    cache: &'c mut Cache,
    abiertos: &Abiertos<'_>,
    uri: &Uri,
) -> Option<(&'c Salida, &'s Document)> {
    let doc = store.get(uri)?;
    let ruta = ruta_de_uri(uri).and_then(|p| p.canonicalize().ok());
    let salida = cache.analizar(
        &uri_texto(uri),
        ruta.as_deref(),
        &doc.text,
        doc.version,
        abiertos,
    );
    Some((salida, doc))
}

/// Los buffers abiertos, por ruta canónica. Ganan al disco: es lo que hace que
/// los diagnósticos vayan al ritmo de lo que se teclea y no al de lo que se
/// guarda.
fn buffers(store: &DocumentStore) -> Abiertos<'_> {
    let mut abiertos = Abiertos::new();
    for (uri, doc) in store.iter() {
        if let Some(ruta) = ruta_de_uri(uri).and_then(|p| p.canonicalize().ok()) {
            abiertos.insertar(ruta, &doc.text, doc.version);
        }
    }
    abiertos
}

/// Despacha una petición de función a su manejador. Cada manejador deriva su
/// respuesta del programa del documento abierto. El `shutdown` ya se trató antes
/// de llegar aquí.
fn handle_request(
    connection: &Connection,
    store: &DocumentStore,
    cache: &mut Cache,
    req: Request,
) -> Result<(), BoxError> {
    let abiertos = buffers(store);
    let uris = Uris::de(store);

    let req = match dispatch::<DocumentSymbolRequest, _>(connection, req, |params| {
        let uri = params.text_document.uri;
        let (salida, doc) = analizar(store, &mut *cache, &abiertos, &uri)?;
        let simbolos = symbols::document_symbols(salida, doc);
        Some(DocumentSymbolResponse::Nested(simbolos))
    })? {
        Ok(()) => return Ok(()),
        Err(req) => req,
    };
    let req = match dispatch::<Completion, _>(connection, req, |params| {
        let pos = params.text_document_position;
        let (salida, doc) = analizar(store, &mut *cache, &abiertos, &pos.text_document.uri)?;
        let items = completion::completion(salida, doc, pos.position);
        Some(CompletionResponse::Array(items))
    })? {
        Ok(()) => return Ok(()),
        Err(req) => req,
    };
    let req = match dispatch::<GotoDefinition, _>(connection, req, |params| {
        let pos = params.text_document_position_params;
        let uri = pos.text_document.uri;
        let (salida, doc) = analizar(store, &mut *cache, &abiertos, &uri)?;
        goto::goto_definition(salida, doc, &uri, &uris, pos.position)
    })? {
        Ok(()) => return Ok(()),
        Err(req) => req,
    };
    let req = match dispatch::<HoverRequest, _>(connection, req, |params| {
        let pos = params.text_document_position_params;
        let (salida, doc) = analizar(store, &mut *cache, &abiertos, &pos.text_document.uri)?;
        hover::hover(salida, doc, pos.position)
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

/// Publica los diagnósticos de TODOS los documentos abiertos y de los archivos
/// que sus `import` arrastran.
///
/// Se recalculan todos y no sólo el que cambió porque un programa es un grafo:
/// tocar `usuarios.mar` puede romper `catalogo.mar`, y dejar los errores del
/// segundo como estaban hasta que alguien lo abra sería peor que no darlos. La
/// caché hace que los documentos que no dependen de lo que se tocó no cuesten
/// nada: sus huellas no cambiaron y se devuelve el resultado tal cual.
///
/// Cuando dos programas abiertos comparten un archivo, sus diagnósticos se UNEN
/// en vez de pisarse: si no, el que mira el archivo suelto borraría los errores
/// que sólo se ven mirando el programa entero (dos módulos que declaran el mismo
/// nombre, dos `@session`), y los diagnósticos parpadearían según qué documento
/// se tocara el último.
fn publicar(
    connection: &Connection,
    store: &DocumentStore,
    cache: &mut Cache,
    publicados: &mut HashSet<Uri>,
    disparador: Option<&Uri>,
) -> Result<(), BoxError> {
    let abiertos = buffers(store);
    let uris = Uris::de(store);

    // El documento que disparó la publicación va primero: es el que el editor
    // está esperando.
    let mut docs: Vec<(&Uri, &Document)> = store.iter().collect();
    docs.sort_by_key(|(uri, _)| match disparador {
        Some(d) => *uri != d,
        None => true,
    });

    let mut orden: Vec<Uri> = Vec::new();
    let mut acumulado: HashMap<Uri, (String, Vec<NeutralDiag>)> = HashMap::new();

    for (uri, doc) in docs {
        let ruta = ruta_de_uri(uri).and_then(|p| p.canonicalize().ok());
        let salida = cache.analizar(
            &uri_texto(uri),
            ruta.as_deref(),
            &doc.text,
            doc.version,
            &abiertos,
        );
        for archivo in &salida.archivos {
            let destino = match &archivo.ruta {
                Some(ruta) => match uris.uri(ruta) {
                    Some(u) => u,
                    None => continue,
                },
                None => uri.clone(),
            };
            if let Some((_, diags)) = acumulado.get_mut(&destino) {
                for d in &archivo.diags {
                    if !diags.contains(d) {
                        diags.push(d.clone());
                    }
                }
                continue;
            }
            orden.push(destino.clone());
            let propios = (archivo.fuente.clone(), archivo.diags.clone());
            acumulado.insert(destino, propios);
        }
    }

    let mut nuevos: HashSet<Uri> = HashSet::new();
    for uri in orden {
        let Some((fuente, diags)) = acumulado.remove(&uri) else {
            continue;
        };
        let index = LineIndex::new(&fuente);
        let lsp: Vec<Diagnostic> = diags
            .iter()
            .map(|d| neutral_to_diagnostic(d, &index, &fuente))
            .collect();
        // La versión sólo se declara para los archivos que el editor tiene
        // abiertos; de los demás no tenemos ninguna que citar.
        let version = store.get(&uri).map(|d| d.version);
        nuevos.insert(uri.clone());
        send_diagnostics(connection, uri, version, lsp)?;
    }

    // Archivos que ya no forman parte de ningún programa abierto: se limpian,
    // o se quedarían subrayados para siempre.
    for viejo in publicados.iter() {
        if !nuevos.contains(viejo) {
            send_diagnostics(connection, viejo.clone(), None, Vec::new())?;
        }
    }
    *publicados = nuevos;
    Ok(())
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
