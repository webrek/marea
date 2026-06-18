//! Pruebas headless por función, sobre el servidor JSON-RPC real corriendo en
//! un hilo con una conexión en memoria. Cada prueba hace el handshake, abre un
//! documento `.mar` de `examples/`, envía la petición de la función y verifica
//! la respuesta tipada.

use std::str::FromStr;
use std::thread;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{DidOpenTextDocument, Initialized, Notification as _};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, GotoDefinition, HoverRequest, Initialize, Request as _,
};
use lsp_types::{
    CompletionParams, CompletionResponse, DidOpenTextDocumentParams, DocumentSymbolParams,
    DocumentSymbolResponse, GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents,
    HoverParams, InitializeParams, InitializedParams, PartialResultParams, Position,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams, Uri,
    WorkDoneProgressParams,
};

/// Cliente de prueba: el extremo del cliente y el `join` del hilo del servidor.
struct Cliente {
    conn: Connection,
    server: Option<thread::JoinHandle<Result<(), Box<dyn std::error::Error + Sync + Send>>>>,
}

impl Cliente {
    /// Arranca el servidor en un hilo, hace el handshake y devuelve el cliente.
    fn nuevo() -> Self {
        let (server_conn, client) = Connection::memory();
        let server = thread::spawn(move || marea_lsp::run(server_conn));

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
        // Consume la respuesta de capacidades.
        match client.receiver.recv().unwrap() {
            Message::Response(resp) => assert_eq!(resp.id, init_id),
            otro => panic!("se esperaba la respuesta de initialize, fue {otro:?}"),
        }
        client
            .sender
            .send(Message::Notification(Notification::new(
                Initialized::METHOD.to_string(),
                serde_json::to_value(InitializedParams {}).unwrap(),
            )))
            .unwrap();

        Cliente {
            conn: client,
            server: Some(server),
        }
    }

    /// Abre un documento; consume el `publishDiagnostics` que dispara la apertura.
    fn abrir(&self, uri: &Uri, texto: String) {
        self.conn
            .sender
            .send(Message::Notification(Notification::new(
                DidOpenTextDocument::METHOD.to_string(),
                serde_json::to_value(DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri: uri.clone(),
                        language_id: "marea".to_string(),
                        version: 1,
                        text: texto,
                    },
                })
                .unwrap(),
            )))
            .unwrap();
        // La apertura publica diagnósticos; se descartan aquí.
        self.esperar_diagnosticos();
    }

    /// Envía una petición tipada y devuelve su resultado deserializado.
    fn pedir<R>(&self, id: i32, params: R::Params) -> R::Result
    where
        R: lsp_types::request::Request,
    {
        let req_id = RequestId::from(id);
        self.conn
            .sender
            .send(Message::Request(Request::new(
                req_id.clone(),
                R::METHOD.to_string(),
                serde_json::to_value(params).unwrap(),
            )))
            .unwrap();
        loop {
            match self.conn.receiver.recv().unwrap() {
                Message::Response(resp) if resp.id == req_id => {
                    assert!(resp.error.is_none(), "la petición no debe fallar: {resp:?}");
                    let value = resp.result.unwrap_or(serde_json::Value::Null);
                    return serde_json::from_value(value).expect("resultado deserializable");
                }
                // Otros mensajes (diagnósticos, etc.) se ignoran.
                _ => {}
            }
        }
    }

    fn esperar_diagnosticos(&self) {
        use lsp_types::notification::PublishDiagnostics;
        loop {
            match self.conn.receiver.recv().unwrap() {
                Message::Notification(note) if note.method == PublishDiagnostics::METHOD => return,
                _ => {}
            }
        }
    }
}

impl Drop for Cliente {
    fn drop(&mut self) {
        use lsp_types::notification::Exit;
        use lsp_types::request::Shutdown;
        // Cierre limpio para que el hilo del servidor termine.
        let _ = self.conn.sender.send(Message::Request(Request::new(
            RequestId::from(9999),
            Shutdown::METHOD.to_string(),
            serde_json::Value::Null,
        )));
        let _ = self.conn.sender.send(Message::Notification(Notification::new(
            Exit::METHOD.to_string(),
            serde_json::Value::Null,
        )));
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

/// Lee un ejemplo del directorio `examples/` del workspace.
fn ejemplo(rel: &str) -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest}/../../examples/{rel}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("no se pudo leer {path}: {e}"))
}

fn uri(nombre: &str) -> Uri {
    Uri::from_str(&format!("file:///{nombre}")).unwrap()
}

/// Posición (línea/carácter 0-indexada) del primer byte de `aguja` en `texto`.
fn posicion_de(texto: &str, aguja: &str) -> Position {
    let off = texto.find(aguja).unwrap_or_else(|| panic!("no se halló {aguja:?}"));
    let antes = &texto[..off];
    let line = antes.matches('\n').count() as u32;
    let col_start = antes.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = texto[col_start..off].chars().map(|c| c.len_utf16()).sum::<usize>() as u32;
    Position { line, character }
}

// ===================== documentSymbol =====================

#[test]
fn document_symbol_de_user_lista_items() {
    let cli = Cliente::nuevo();
    let u = uri("user.mar");
    let src = ejemplo("user.mar");
    cli.abrir(&u, src);

    let resp = cli.pedir::<DocumentSymbolRequest>(
        2,
        DocumentSymbolParams {
            text_document: TextDocumentIdentifier { uri: u.clone() },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
    );
    let symbols = match resp {
        Some(DocumentSymbolResponse::Nested(s)) => s,
        otro => panic!("se esperaba lista anidada de símbolos, fue {otro:?}"),
    };
    let nombres: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
    assert!(nombres.contains(&"UserId"), "falta UserId: {nombres:?}");
    assert!(nombres.contains(&"User"), "falta User: {nombres:?}");
    assert!(nombres.contains(&"getUser"), "falta getUser: {nombres:?}");
    assert!(nombres.contains(&"perfil"), "falta perfil: {nombres:?}");

    use lsp_types::SymbolKind;
    let get_user = symbols.iter().find(|s| s.name == "getUser").unwrap();
    assert_eq!(get_user.kind, SymbolKind::FUNCTION);
    let user_id = symbols.iter().find(|s| s.name == "UserId").unwrap();
    assert_eq!(user_id.kind, SymbolKind::INTERFACE);
}

// ===================== completion =====================

#[test]
fn completion_incluye_keyword_y_builtin() {
    let cli = Cliente::nuevo();
    let u = uri("user.mar");
    let src = ejemplo("user.mar");
    // Posición dentro del cuerpo de `perfil`, donde aplica la lista general.
    let pos = posicion_de(&src, "render");
    cli.abrir(&u, src);

    let resp = cli.pedir::<Completion>(
        3,
        CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: pos,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        },
    );
    let items = match resp {
        Some(CompletionResponse::Array(items)) => items,
        otro => panic!("se esperaba arreglo de completado, fue {otro:?}"),
    };
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"reactive"), "falta keyword reactive: {labels:?}");
    assert!(labels.contains(&"print"), "falta builtin print: {labels:?}");
    // Nombres de nivel superior del propio módulo.
    assert!(labels.contains(&"getUser"), "falta el nombre getUser: {labels:?}");
    // Builtin de tipo.
    assert!(labels.contains(&"Int"), "falta el tipo Int: {labels:?}");
}

#[test]
fn completion_tras_arroba_ofrece_ubicaciones() {
    let cli = Cliente::nuevo();
    let u = uri("arroba.mar");
    // Documento con un `@` recién tecleado antes de una función.
    let src = "@\nfn f() {}\n";
    let pos = posicion_de(src, "@");
    // El cursor va justo después del `@`: columna 1 en la línea 0.
    let pos = Position { line: pos.line, character: pos.character + 1 };
    cli.abrir(&u, src.to_string());

    let resp = cli.pedir::<Completion>(
        4,
        CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: pos,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
            context: None,
        },
    );
    let items = match resp {
        Some(CompletionResponse::Array(items)) => items,
        otro => panic!("se esperaba arreglo de completado, fue {otro:?}"),
    };
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels.len(),
        3,
        "tras `@` solo van las tres ubicaciones: {labels:?}"
    );
    assert!(labels.contains(&"server"));
    assert!(labels.contains(&"client"));
    assert!(labels.contains(&"edge"));
}

// ===================== definition =====================

#[test]
fn definition_de_uso_salta_a_la_declaracion() {
    let cli = Cliente::nuevo();
    let u = uri("user.mar");
    let src = ejemplo("user.mar");
    // El uso de `getUser` dentro de `perfil` (la llamada en el `reactive`).
    let uso = src.find("getUser(id)").expect("hay un uso de getUser");
    let antes = &src[..uso];
    let line = antes.matches('\n').count() as u32;
    let col_start = antes.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = src[col_start..uso].chars().map(|c| c.len_utf16()).sum::<usize>() as u32;
    let pos = Position { line, character };
    cli.abrir(&u, src.clone());

    let resp = cli.pedir::<GotoDefinition>(
        5,
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: pos,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
    );
    let loc = match resp {
        Some(GotoDefinitionResponse::Scalar(loc)) => loc,
        otro => panic!("se esperaba una Location escalar, fue {otro:?}"),
    };
    assert_eq!(loc.uri, u);
    // La declaración de `getUser` arranca en la línea de `@server` (su span
    // incluye la anotación de ubicación). Verificamos que el rango cubre la
    // firma `fn getUser`.
    let decl = src.find("fn getUser").expect("hay declaración");
    let antes_decl = &src[..decl];
    let line_decl = antes_decl.matches('\n').count() as u32;
    assert!(
        loc.range.start.line <= line_decl && line_decl <= loc.range.end.line,
        "el rango {:?} debe cubrir la línea {} de `fn getUser`",
        loc.range,
        line_decl
    );
}

// ===================== hover =====================

#[test]
fn hover_sobre_funcion_muestra_su_firma() {
    let cli = Cliente::nuevo();
    let u = uri("user.mar");
    let src = ejemplo("user.mar");
    // Hover sobre el nombre en la declaración `fn getUser`.
    let decl = src.find("getUser(id: UserId)").expect("hay firma");
    let antes = &src[..decl];
    let line = antes.matches('\n').count() as u32;
    let col_start = antes.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = src[col_start..decl].chars().map(|c| c.len_utf16()).sum::<usize>() as u32;
    let pos = Position { line, character };
    cli.abrir(&u, src);

    let resp: Option<Hover> = cli.pedir::<HoverRequest>(
        6,
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: pos,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
    );
    let hover = resp.expect("debe haber hover sobre la función");
    let texto = match hover.contents {
        HoverContents::Markup(m) => m.value,
        otro => panic!("se esperaba markup, fue {otro:?}"),
    };
    assert!(
        texto.contains("fn getUser"),
        "el hover debe mostrar la firma: {texto:?}"
    );
    assert!(
        texto.contains("UserId"),
        "la firma debe incluir el tipo del parámetro: {texto:?}"
    );
    assert!(
        texto.contains("@server"),
        "la firma debe incluir la ubicación: {texto:?}"
    );
}
