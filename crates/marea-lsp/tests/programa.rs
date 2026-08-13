//! Pruebas del servidor cuando el documento abierto es parte de un PROGRAMA:
//! varios archivos unidos por `import`.
//!
//! Se hacen contra el servidor JSON-RPC real y contra archivos reales del disco,
//! porque es justo lo que se está probando: el grafo se resuelve leyendo rutas
//! relativas, y una prueba con fuentes en memoria no diría nada de eso.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::thread;
use std::time::Duration;

use lsp_server::{Connection, Message, Notification, Request, RequestId};
use lsp_types::notification::{
    DidChangeTextDocument, DidOpenTextDocument, Initialized, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, GotoDefinition, HoverRequest, Initialize, Request as _, Shutdown,
};
use lsp_types::{
    CompletionParams, CompletionResponse, DidChangeTextDocumentParams, DidOpenTextDocumentParams,
    GotoDefinitionParams, GotoDefinitionResponse, Hover, HoverContents, HoverParams,
    InitializeParams, InitializedParams, PartialResultParams, Position, PublishDiagnosticsParams,
    TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    TextDocumentPositionParams, Uri, VersionedTextDocumentIdentifier, WorkDoneProgressParams,
};

use marea_lsp::conversions::uri_de_ruta;

/// Cuánto se espera un mensaje del servidor antes de dar la prueba por colgada.
const ESPERA: Duration = Duration::from_secs(20);

// ===================== cliente de prueba =====================

struct Cliente {
    conn: Connection,
    server: Option<thread::JoinHandle<Result<(), Box<dyn std::error::Error + Sync + Send>>>>,
}

impl Cliente {
    fn nuevo() -> Self {
        let (server_conn, client) = Connection::memory();
        let server = thread::spawn(move || marea_lsp::run(server_conn));

        let init_id = RequestId::from(1);
        client
            .sender
            .send(Message::Request(Request::new(
                init_id.clone(),
                Initialize::METHOD.to_string(),
                serde_json::to_value(InitializeParams::default()).unwrap(),
            )))
            .unwrap();
        match client.receiver.recv_timeout(ESPERA).unwrap() {
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
    }

    fn cambiar(&self, uri: &Uri, texto: String, version: i32) {
        self.conn
            .sender
            .send(Message::Notification(Notification::new(
                DidChangeTextDocument::METHOD.to_string(),
                serde_json::to_value(DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version,
                    },
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: texto,
                    }],
                })
                .unwrap(),
            )))
            .unwrap();
    }

    /// Espera hasta ver la publicación de diagnósticos DE ESE archivo. El
    /// servidor publica uno por archivo del programa, así que hay que esperar al
    /// que interesa y no al primero que llegue.
    fn diagnosticos_de(&self, uri: &Uri) -> PublishDiagnosticsParams {
        loop {
            let msg = self
                .conn
                .receiver
                .recv_timeout(ESPERA)
                .unwrap_or_else(|e| panic!("sin diagnósticos para {uri:?}: {e}"));
            if let Message::Notification(note) = msg {
                if note.method == PublishDiagnostics::METHOD {
                    let params: PublishDiagnosticsParams =
                        note.extract(PublishDiagnostics::METHOD).unwrap();
                    if params.uri == *uri {
                        return params;
                    }
                }
            }
        }
    }

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
            match self.conn.receiver.recv_timeout(ESPERA).unwrap() {
                Message::Response(resp) if resp.id == req_id => {
                    assert!(resp.error.is_none(), "la petición no debe fallar: {resp:?}");
                    let value = resp.result.unwrap_or(serde_json::Value::Null);
                    return serde_json::from_value(value).expect("resultado deserializable");
                }
                _ => {}
            }
        }
    }
}

impl Drop for Cliente {
    fn drop(&mut self) {
        use lsp_types::notification::Exit;
        let _ = self.conn.sender.send(Message::Request(Request::new(
            RequestId::from(9999),
            Shutdown::METHOD.to_string(),
            serde_json::Value::Null,
        )));
        let _ = self
            .conn
            .sender
            .send(Message::Notification(Notification::new(
                Exit::METHOD.to_string(),
                serde_json::Value::Null,
            )));
        if let Some(server) = self.server.take() {
            let _ = server.join();
        }
    }
}

// ===================== utilidades =====================

/// Crea un directorio de trabajo con los archivos dados y devuelve su ruta
/// CANÓNICA. Canonicalizar importa: en macOS el temporal cuelga de un enlace
/// simbólico, y el servidor compara rutas canónicas.
fn escribir(caso: &str, archivos: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("marea-lsp-{caso}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("se puede crear el directorio de la prueba");
    for (nombre, contenido) in archivos {
        std::fs::write(dir.join(nombre), contenido).expect("se puede escribir el módulo");
    }
    dir.canonicalize().expect("el directorio existe")
}

fn uri_de(ruta: &Path) -> Uri {
    uri_de_ruta(&ruta.canonicalize().expect("la ruta existe")).expect("URI construible")
}

fn ejemplo_modulos(nombre: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/modulos")
        .join(nombre)
}

/// Posición (línea/carácter 0-indexada) del primer byte de `aguja` en `texto`.
fn posicion_de(texto: &str, aguja: &str) -> Position {
    let off = texto
        .find(aguja)
        .unwrap_or_else(|| panic!("no se halló {aguja:?}"));
    let antes = &texto[..off];
    let line = antes.matches('\n').count() as u32;
    let col_start = antes.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let character = texto[col_start..off]
        .chars()
        .map(|c| c.len_utf16())
        .sum::<usize>() as u32;
    Position { line, character }
}

fn texto_del_hover(h: Hover) -> String {
    match h.contents {
        HoverContents::Markup(m) => m.value,
        otro => panic!("se esperaba markup, fue {otro:?}"),
    }
}

// ===================== diagnósticos de programa =====================

/// El caso que motivó todo esto: hasta ahora, abrir un archivo con `import`
/// dejaba el editor rojo entero porque cada nombre importado salía sin resolver.
#[test]
fn un_programa_real_con_imports_no_trae_ni_un_error() {
    let entrada = ejemplo_modulos("tienda.mar");
    let src = std::fs::read_to_string(&entrada).expect("examples/modulos/tienda.mar existe");
    let u = uri_de(&entrada);

    let cli = Cliente::nuevo();
    cli.abrir(&u, src);

    let diags = cli.diagnosticos_de(&u);
    assert!(
        diags.diagnostics.is_empty(),
        "tienda.mar importa de otros dos módulos y tipa: {:?}",
        diags.diagnostics
    );
}

/// Y los archivos que arrastra el `import` también reciben los suyos, aunque el
/// editor no los tenga abiertos: es lo que hace que el error se vea donde está.
#[test]
fn los_modulos_importados_tambien_reciben_diagnosticos() {
    let entrada = ejemplo_modulos("tienda.mar");
    let src = std::fs::read_to_string(&entrada).expect("examples/modulos/tienda.mar existe");
    let u = uri_de(&entrada);
    let u_usuarios = uri_de(&ejemplo_modulos("usuarios.mar"));

    let cli = Cliente::nuevo();
    cli.abrir(&u, src);

    let diags = cli.diagnosticos_de(&u_usuarios);
    assert!(
        diags.diagnostics.is_empty(),
        "usuarios.mar tipa: {:?}",
        diags.diagnostics
    );
}

/// Un error de tipos en el módulo importado se publica en SU archivo, con su
/// span, y no contamina al que se está editando.
#[test]
fn el_error_del_modulo_importado_va_a_su_propio_archivo() {
    let dir = escribir(
        "error-en-dependencia",
        &[
            (
                "b.mar",
                "fn mal(a: Int, b: String) -> Int {\n    return a + b;\n}\n",
            ),
            (
                "a.mar",
                "import { mal } from \"./b.mar\";\n\nfn usa() -> Int {\n    return mal(1, \"x\");\n}\n",
            ),
        ],
    );
    let u_a = uri_de(&dir.join("a.mar"));
    let u_b = uri_de(&dir.join("b.mar"));

    let cli = Cliente::nuevo();
    cli.abrir(&u_a, std::fs::read_to_string(dir.join("a.mar")).unwrap());

    let en_b = cli.diagnosticos_de(&u_b);
    assert_eq!(
        en_b.diagnostics
            .iter()
            .map(|d| d.code.clone())
            .collect::<Vec<_>>(),
        vec![Some(lsp_types::NumberOrString::String(
            "E_ARITH_TYPE".to_string()
        ))],
        "el error de b.mar se publica en b.mar"
    );

    let en_a = cli.diagnosticos_de(&u_a);
    assert!(
        en_a.diagnostics.is_empty(),
        "a.mar no tiene ningún error propio: {:?}",
        en_a.diagnostics
    );
}

/// Importar algo que el destino no declara se señala sobre EL NOMBRE, en el
/// archivo que lo escribió.
#[test]
fn importar_lo_que_el_otro_modulo_no_declara_senala_el_nombre() {
    let dir = escribir(
        "no-exportado",
        &[
            ("b.mar", "fn hay() -> Int {\n    return 1;\n}\n"),
            (
                "a.mar",
                "import { noHay } from \"./b.mar\";\n\nfn usa() -> Int {\n    return 1;\n}\n",
            ),
        ],
    );
    let u_a = uri_de(&dir.join("a.mar"));
    let src = std::fs::read_to_string(dir.join("a.mar")).unwrap();

    let cli = Cliente::nuevo();
    cli.abrir(&u_a, src.clone());

    let diags = cli.diagnosticos_de(&u_a);
    assert_eq!(diags.diagnostics.len(), 1, "{:?}", diags.diagnostics);
    let d = &diags.diagnostics[0];
    assert_eq!(
        d.code,
        Some(lsp_types::NumberOrString::String(
            "E_MODULO_NO_EXPORTA".to_string()
        ))
    );
    assert_eq!(
        d.range.start,
        posicion_de(&src, "noHay"),
        "el subrayado va sobre el nombre importado, no sobre la línea entera"
    );
}

/// Un `import` que no resuelve no debe arrastrar consigo un error por cada
/// nombre que traía: con el grafo roto, los tipos no se miran.
#[test]
fn un_import_que_no_resuelve_no_desata_una_cascada() {
    let dir = escribir(
        "import-roto",
        &[(
            "a.mar",
            "import { X } from \"./no-existe.mar\";\n\nfn usa(x: X) -> Int {\n    return 1;\n}\n",
        )],
    );
    let u_a = uri_de(&dir.join("a.mar"));

    let cli = Cliente::nuevo();
    cli.abrir(&u_a, std::fs::read_to_string(dir.join("a.mar")).unwrap());

    let diags = cli.diagnosticos_de(&u_a);
    assert_eq!(
        diags.diagnostics.len(),
        1,
        "sólo el import roto, sin errores derivados: {:?}",
        diags.diagnostics
    );
    assert_eq!(
        diags.diagnostics[0].code,
        Some(lsp_types::NumberOrString::String(
            "E_MODULO_NO_ENCONTRADO".to_string()
        ))
    );
}

/// Lo que vale es el buffer, no el disco: si no, los diagnósticos irían un
/// `Ctrl+S` por detrás de lo que se escribe.
#[test]
fn el_buffer_sin_guardar_manda_sobre_el_disco() {
    let dir = escribir(
        "buffer-manda",
        &[
            (
                "b.mar",
                "fn mal(a: Int, b: String) -> Int {\n    return a + b;\n}\n",
            ),
            (
                "a.mar",
                "import { mal } from \"./b.mar\";\n\nfn usa() -> Int {\n    return mal(1, \"x\");\n}\n",
            ),
        ],
    );
    let u_a = uri_de(&dir.join("a.mar"));
    let u_b = uri_de(&dir.join("b.mar"));

    let cli = Cliente::nuevo();
    cli.abrir(&u_a, std::fs::read_to_string(dir.join("a.mar")).unwrap());
    let _ = cli.diagnosticos_de(&u_b);

    // Se abre b.mar y se arregla EN EL EDITOR, sin tocar el disco.
    cli.abrir(&u_b, std::fs::read_to_string(dir.join("b.mar")).unwrap());
    let recien_abierto = cli.diagnosticos_de(&u_b);
    assert!(
        !recien_abierto.diagnostics.is_empty(),
        "al abrirlo, b.mar todavía trae el error que hay en el disco"
    );
    cli.cambiar(
        &u_b,
        "fn mal(a: Int, b: Int) -> Int {\n    return a + b;\n}\n".to_string(),
        2,
    );

    let en_b = cli.diagnosticos_de(&u_b);
    assert!(
        en_b.diagnostics.is_empty(),
        "el error se arregló en el buffer, aunque el disco siga como estaba: {:?}",
        en_b.diagnostics
    );
    // Y el archivo del disco sigue con la versión rota: la prueba no se está
    // engañando a sí misma.
    let en_disco = std::fs::read_to_string(dir.join("b.mar")).unwrap();
    assert!(en_disco.contains("b: String"));
}

// ===================== navegación a través del import =====================

#[test]
fn ir_a_definicion_salta_al_otro_archivo() {
    let dir = escribir(
        "goto-import",
        &[
            (
                "b.mar",
                "type Saludo = { texto: String };\n\nfn saluda(nombre: String) -> Saludo {\n    return Saludo { texto: nombre };\n}\n",
            ),
            (
                "a.mar",
                "import { Saludo, saluda } from \"./b.mar\";\n\nfn usa() -> Saludo {\n    return saluda(\"ada\");\n}\n",
            ),
        ],
    );
    let u_a = uri_de(&dir.join("a.mar"));
    let u_b = uri_de(&dir.join("b.mar"));
    let src_a = std::fs::read_to_string(dir.join("a.mar")).unwrap();
    let src_b = std::fs::read_to_string(dir.join("b.mar")).unwrap();

    let cli = Cliente::nuevo();
    cli.abrir(&u_a, src_a.clone());

    // Sobre el USO de `saluda`, que está dentro del `return`.
    let uso = posicion_de(&src_a, "saluda(\"ada\")");
    let resp = cli.pedir::<GotoDefinition>(
        2,
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u_a.clone() },
                position: uso,
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
    );
    let loc = match resp {
        Some(GotoDefinitionResponse::Scalar(loc)) => loc,
        otro => panic!("se esperaba una Location escalar, fue {otro:?}"),
    };
    assert_eq!(loc.uri, u_b, "el salto cruza el import");
    assert_eq!(
        loc.range.start,
        posicion_de(&src_b, "fn saluda"),
        "aterriza en la declaración de b.mar"
    );
}

#[test]
fn ir_a_definicion_desde_el_propio_import() {
    let dir = escribir(
        "goto-nombre-importado",
        &[
            ("b.mar", "type Saludo = { texto: String };\n"),
            (
                "a.mar",
                "import { Saludo } from \"./b.mar\";\n\nfn usa(s: Saludo) -> String {\n    return s.texto;\n}\n",
            ),
        ],
    );
    let u_a = uri_de(&dir.join("a.mar"));
    let u_b = uri_de(&dir.join("b.mar"));
    let src_a = std::fs::read_to_string(dir.join("a.mar")).unwrap();

    let cli = Cliente::nuevo();
    cli.abrir(&u_a, src_a.clone());

    // El cursor sobre el nombre DENTRO de las llaves del import.
    let resp = cli.pedir::<GotoDefinition>(
        2,
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u_a.clone() },
                position: posicion_de(&src_a, "Saludo"),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
            partial_result_params: PartialResultParams::default(),
        },
    );
    match resp {
        Some(GotoDefinitionResponse::Scalar(loc)) => assert_eq!(loc.uri, u_b),
        otro => panic!("se esperaba una Location escalar, fue {otro:?}"),
    }
}

#[test]
fn hover_de_un_nombre_importado_ensena_su_declaracion_y_de_donde_viene() {
    let dir = escribir(
        "hover-import",
        &[
            (
                "b.mar",
                "@server(Public)\nfn catalogo() -> Int {\n    return 1;\n}\n",
            ),
            (
                "a.mar",
                "import { catalogo } from \"./b.mar\";\n\nfn usa() -> Int {\n    return catalogo();\n}\n",
            ),
        ],
    );
    let u_a = uri_de(&dir.join("a.mar"));
    let src_a = std::fs::read_to_string(dir.join("a.mar")).unwrap();

    let cli = Cliente::nuevo();
    cli.abrir(&u_a, src_a.clone());

    let resp: Option<Hover> = cli.pedir::<HoverRequest>(
        2,
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u_a.clone() },
                position: posicion_de(&src_a, "catalogo();"),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
    );
    let texto = texto_del_hover(resp.expect("hay hover sobre el nombre importado"));
    assert!(texto.contains("fn catalogo"), "{texto:?}");
    assert!(
        texto.contains("@server(Public)"),
        "la política forma parte de la firma: {texto:?}"
    );
    assert!(
        texto.contains("b.mar"),
        "el hover dice de qué archivo viene: {texto:?}"
    );
}

#[test]
fn el_completado_ofrece_lo_que_traen_los_imports() {
    let dir = escribir(
        "completado-import",
        &[
            (
                "b.mar",
                "type Saludo = { texto: String };\n\nfn saluda() -> Int {\n    return 1;\n}\n",
            ),
            (
                "a.mar",
                "import { Saludo, saluda } from \"./b.mar\";\n\nfn usa() -> Int {\n    return 1;\n}\n",
            ),
        ],
    );
    let u_a = uri_de(&dir.join("a.mar"));
    let src_a = std::fs::read_to_string(dir.join("a.mar")).unwrap();

    let cli = Cliente::nuevo();
    cli.abrir(&u_a, src_a.clone());

    let resp = cli.pedir::<Completion>(
        2,
        CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u_a.clone() },
                position: posicion_de(&src_a, "return 1;"),
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
    assert!(labels.contains(&"saluda"), "falta lo importado: {labels:?}");
    assert!(labels.contains(&"Saludo"), "falta el tipo: {labels:?}");
    // Los builtins que se añadieron con las listas, el texto y el JSON.
    for builtin in ["append", "contains", "lower", "jsonText", "save", "all"] {
        assert!(labels.contains(&builtin), "falta {builtin}: {labels:?}");
    }
    // Y las palabras clave de módulos y de bucle.
    for kw in ["import", "from", "for", "in", "store"] {
        assert!(
            labels.contains(&kw),
            "falta la palabra clave {kw}: {labels:?}"
        );
    }

    let importado = items
        .iter()
        .find(|i| i.label == "saluda")
        .expect("saluda está");
    assert_eq!(
        importado.detail.as_deref(),
        Some("de b.mar"),
        "el completado dice de dónde viene cada nombre importado"
    );
}

// ===================== cierres =====================

/// El documento no existe en el disco a propósito: los cierres son cosa de UN
/// archivo, y así se prueba también que el camino sin programa sigue vivo.
const CON_CIERRE: &str = "\
@client
fn aplica(n: Int) -> Int {
    let doble = fn(a: Int) -> Int { return a + a; };
    return doble(n);
}
";

#[test]
fn hover_dentro_del_cierre_resuelve_su_parametro() {
    let cli = Cliente::nuevo();
    let u = Uri::from_str("file:///cierre.mar").unwrap();
    cli.abrir(&u, CON_CIERRE.to_string());

    // El uso de `a` dentro del cuerpo del cierre.
    let resp: Option<Hover> = cli.pedir::<HoverRequest>(
        2,
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: posicion_de(CON_CIERRE, "a + a"),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
    );
    let texto = texto_del_hover(resp.expect("hay hover sobre el parámetro del cierre"));
    assert!(
        texto.contains("a: Int"),
        "el parámetro del cierre se resuelve a su tipo: {texto:?}"
    );
}

#[test]
fn ir_a_definicion_dentro_del_cierre_lleva_a_su_parametro() {
    let cli = Cliente::nuevo();
    let u = Uri::from_str("file:///cierre.mar").unwrap();
    cli.abrir(&u, CON_CIERRE.to_string());

    let resp = cli.pedir::<GotoDefinition>(
        2,
        GotoDefinitionParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: posicion_de(CON_CIERRE, "a + a"),
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
    assert_eq!(
        loc.range.start,
        posicion_de(CON_CIERRE, "a: Int"),
        "salta al parámetro del cierre, no a la función que lo contiene"
    );
}

#[test]
fn hover_sobre_el_cierre_ensena_su_firma() {
    let cli = Cliente::nuevo();
    let u = Uri::from_str("file:///cierre.mar").unwrap();
    cli.abrir(&u, CON_CIERRE.to_string());

    // Sobre el `fn(` del cierre, no sobre su cuerpo.
    let resp: Option<Hover> = cli.pedir::<HoverRequest>(
        2,
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: posicion_de(CON_CIERRE, "fn(a: Int)"),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
    );
    let texto = texto_del_hover(resp.expect("hay hover sobre el cierre"));
    assert!(
        texto.contains("fn(a: Int) -> Int"),
        "la firma del cierre: {texto:?}"
    );
}

// ===================== política e identidad =====================

const CON_POLITICA: &str = "\
type Usuario = { nombre: String, admin: Bool };

@session
fn quien(token: String) -> Usuario | NoAutorizado {
    if len(token) > 0 {
        return Usuario { nombre: \"ada\", admin: true };
    }
    return NoAutorizado;
}

@server(u: Usuario)
fn saluda() -> String {
    return u.nombre;
}
";

#[test]
fn hover_de_la_identidad_ligada_por_la_politica() {
    let cli = Cliente::nuevo();
    let u = Uri::from_str("file:///politica.mar").unwrap();
    cli.abrir(&u, CON_POLITICA.to_string());

    let resp: Option<Hover> = cli.pedir::<HoverRequest>(
        2,
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: posicion_de(CON_POLITICA, "u.nombre"),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
    );
    let texto = texto_del_hover(resp.expect("hay hover sobre la identidad"));
    assert!(
        texto.contains("u: Usuario"),
        "la identidad se resuelve a su tipo: {texto:?}"
    );
}

#[test]
fn hover_de_la_session_dice_que_lo_es() {
    let cli = Cliente::nuevo();
    let u = Uri::from_str("file:///politica.mar").unwrap();
    cli.abrir(&u, CON_POLITICA.to_string());

    let resp: Option<Hover> = cli.pedir::<HoverRequest>(
        2,
        HoverParams {
            text_document_position_params: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier { uri: u.clone() },
                position: posicion_de(CON_POLITICA, "quien(token"),
            },
            work_done_progress_params: WorkDoneProgressParams::default(),
        },
    );
    let texto = texto_del_hover(resp.expect("hay hover sobre la @session"));
    assert!(texto.contains("@session"), "{texto:?}");
    assert!(texto.contains("fn quien"), "{texto:?}");
}
