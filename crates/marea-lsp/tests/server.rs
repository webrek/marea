//! Prueba headless del servidor JSON-RPC completo: arranca [`marea_lsp::run`]
//! en un hilo sobre una conexión en memoria y, desde el hilo principal, actúa
//! como cliente del editor.
//!
//! Cubre el ciclo de vida real: handshake `initialize`/`initialized`, apertura
//! de un documento con error de tipos (se reciben diagnósticos con el código
//! `E_...` y un rango válido), apertura de un documento válido (diagnósticos
//! vacíos) y cierre limpio con `shutdown`/`exit`.

use std::thread;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidOpenTextDocument, Exit, Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{Initialize, Request as _, Shutdown};
use lsp_types::{
    DidOpenTextDocumentParams, InitializeParams, InitializedParams, NumberOrString,
    PublishDiagnosticsParams, TextDocumentItem, Uri,
};
use std::str::FromStr;

/// Lee un ejemplo del directorio `examples/` del workspace.
fn ejemplo(rel: &str) -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest}/../../examples/{rel}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("no se pudo leer {path}: {e}"))
}

/// Espera y devuelve el siguiente `publishDiagnostics` del servidor, ignorando
/// cualquier otro mensaje (p. ej. notificaciones de registro).
fn recibir_diagnosticos(client: &Connection) -> PublishDiagnosticsParams {
    loop {
        let msg = client
            .receiver
            .recv()
            .expect("el servidor debe enviar diagnósticos");
        if let Message::Notification(note) = msg {
            if note.method == PublishDiagnostics::METHOD {
                return note
                    .extract(PublishDiagnostics::METHOD)
                    .expect("params de publishDiagnostics válidos");
            }
        }
    }
}

#[test]
fn ciclo_de_vida_completo_con_diagnosticos() {
    // `memory()` devuelve dos extremos conectados: uno para el servidor, otro
    // para el cliente de la prueba.
    let (server_conn, client) = Connection::memory();

    // Hilo del servidor: corre el bucle real hasta el cierre.
    let server = thread::spawn(move || marea_lsp::run(server_conn));

    // --- Handshake: initialize -> (capacidades) -> initialized ---
    let init_id = RequestId::from(1);
    let init_params = serde_json::to_value(InitializeParams::default()).unwrap();
    client
        .sender
        .send(Message::Request(Request::new(
            init_id.clone(),
            Initialize::METHOD.to_string(),
            init_params,
        )))
        .unwrap();

    // El servidor responde con sus capacidades.
    match client.receiver.recv().unwrap() {
        Message::Response(resp) => {
            assert_eq!(resp.id, init_id);
            assert!(resp.error.is_none(), "initialize no debe fallar: {resp:?}");
            let result = resp.result.expect("initialize devuelve capacidades");
            assert!(
                result.get("capabilities").is_some(),
                "la respuesta debe traer capabilities: {result}"
            );
        }
        otro => panic!("se esperaba la respuesta de initialize, fue {otro:?}"),
    }

    client
        .sender
        .send(Message::Notification(Notification::new(
            Initialized::METHOD.to_string(),
            serde_json::to_value(InitializedParams {}).unwrap(),
        )))
        .unwrap();

    // --- didOpen de un .mar con error de tipos conocido ---
    let uri_malo = Uri::from_str("file:///malo.mar").unwrap();
    let src_malo = ejemplo("check_fail/tipo_aritmetico.mar");
    client
        .sender
        .send(Message::Notification(Notification::new(
            DidOpenTextDocument::METHOD.to_string(),
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri_malo.clone(),
                    language_id: "marea".to_string(),
                    version: 1,
                    text: src_malo,
                },
            })
            .unwrap(),
        )))
        .unwrap();

    let diag_malo = recibir_diagnosticos(&client);
    assert_eq!(diag_malo.uri, uri_malo);
    assert!(
        !diag_malo.diagnostics.is_empty(),
        "un documento con error de tipos debe traer >=1 diagnóstico"
    );
    let d = &diag_malo.diagnostics[0];
    assert_eq!(
        d.code,
        Some(NumberOrString::String("E_ARITH_TYPE".to_string())),
        "el código del diagnóstico debe ser E_ARITH_TYPE"
    );
    // Rango válido: start <= end.
    assert!(
        (d.range.start.line, d.range.start.character) <= (d.range.end.line, d.range.end.character),
        "el rango del diagnóstico debe ser válido: {:?}",
        d.range
    );

    // --- didOpen de un .mar válido: diagnósticos vacíos ---
    let uri_ok = Uri::from_str("file:///ok.mar").unwrap();
    let src_ok = ejemplo("registro.mar");
    client
        .sender
        .send(Message::Notification(Notification::new(
            DidOpenTextDocument::METHOD.to_string(),
            serde_json::to_value(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri_ok.clone(),
                    language_id: "marea".to_string(),
                    version: 1,
                    text: src_ok,
                },
            })
            .unwrap(),
        )))
        .unwrap();

    let diag_ok = recibir_diagnosticos(&client);
    assert_eq!(diag_ok.uri, uri_ok);
    assert!(
        diag_ok.diagnostics.is_empty(),
        "un documento válido no debe traer diagnósticos, hubo: {:?}",
        diag_ok.diagnostics
    );

    // --- Cierre limpio: shutdown -> exit ---
    let shutdown_id = RequestId::from(2);
    client
        .sender
        .send(Message::Request(Request::new(
            shutdown_id.clone(),
            Shutdown::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();

    match client.receiver.recv().unwrap() {
        Message::Response(resp) => {
            assert_eq!(resp.id, shutdown_id);
            assert!(resp.error.is_none(), "shutdown no debe fallar: {resp:?}");
        }
        otro => panic!("se esperaba la respuesta de shutdown, fue {otro:?}"),
    }

    client
        .sender
        .send(Message::Notification(Notification::new(
            Exit::METHOD.to_string(),
            serde_json::Value::Null,
        )))
        .unwrap();

    // El servidor debe terminar sin error.
    let resultado = server
        .join()
        .expect("el hilo del servidor no debe entrar en pánico");
    assert!(resultado.is_ok(), "run debe terminar limpio: {resultado:?}");
}
