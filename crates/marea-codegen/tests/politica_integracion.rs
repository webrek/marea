//! Prueba de INTEGRACIÓN de la política: se levanta el servidor generado y se
//! comprueba que la identidad se exige DE VERDAD.
//!
//! Que `@server(Usuario)` compile no protege nada por sí solo: la fase 1 dejó la
//! política como una garantía de compilación mientras el endpoint seguía
//! atendiendo a cualquiera. Y buscar cadenas en la plantilla del runtime pasaría
//! igual con la comprobación invertida. Así que aquí se ataca el servidor: sin
//! token, con un token inválido, con uno bueno por cabecera y por cookie, y
//! contra un handler declarado público.
//!
//! Se salta solo (sin fallar) si no hay `node` en el PATH.

use std::process::{Command, Stdio};

const FUENTE: &str = r#"
type Usuario = { nombre: String };
type Post = { autor: String, texto: String };
store posts: Post;

@session
fn quien(token: String) -> Usuario | NoAutorizado {
    if contains(token, "secreto") {
        return Usuario { nombre: "ada" };
    }
    return NoAutorizado;
}

@server(u: Usuario)
fn publicar(texto: String) {
    save(posts, Post { autor: u.nombre, texto: texto });
}

@server(Public)
fn feed() -> List<Post> {
    return all(posts);
}

@client
fn vista() -> Html {
    return `<p>x</p>`;
}
"#;

fn hay_node() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn generar(dir: &str) {
    let module = marea_syntax::parse(FUENTE).expect("la fuente debe parsear");
    let app = marea_codegen::emit_app(&module);
    std::fs::create_dir_all(dir).expect("crear dir");
    for (n, c) in [
        ("runtime.ts", app.runtime),
        ("server.ts", app.server),
        ("serve.ts", app.serve),
        ("client.js", app.client_js),
        ("index.html", app.index_html),
    ] {
        std::fs::write(format!("{dir}/{n}"), c).expect("escribir");
    }
}

/// Lanza el servidor y le hace las peticiones desde el propio proceso de Node
/// (sin curl ni puertos adivinados). Devuelve su stdout.
fn atacar(dir: &str, puerto: u16) -> String {
    let guion = format!(
        r#"
process.env.MAREA_STORE_DIR = "{dir}";
process.env.PORT = "{puerto}";
await import("{dir}/server.ts");
const {{ startServer, stopServer }} = await import("{dir}/runtime.ts");
await startServer();
const U = "http://127.0.0.1:{puerto}/__marea";
const J = {{ "content-type": "application/json" }};
const pedir = async (cuerpo, cab) => {{
  const r = await fetch(U, {{ method: "POST", headers: cab, body: JSON.stringify(cuerpo) }});
  return {{ status: r.status, texto: await r.text() }};
}};
const out = {{}};

// 1. Sin token: la política se aplica antes del cuerpo.
const sin = await pedir({{fn:"publicar",args:["sin token"]}}, J);
out.sinToken = sin.status;
// 2. Con un token que la @session rechaza.
const malo = await pedir({{fn:"publicar",args:["token malo"]}}, {{...J, authorization:"Bearer basura"}});
out.tokenMalo = malo.status;
// Las dos respuestas son la misma: distinguirlas sería un oráculo de tokens.
out.mismoCuerpo = sin.texto === malo.texto ? 1 : 0;
out.filtra = /basura|NoAutorizado|quien|session/i.test(sin.texto) ? 1 : 0;

// 3. Un cliente que intenta MANDAR la identidad como argumento: la aridad
//    válida sigue siendo la del cliente, así que sobra un argumento.
out.forjada = (await pedir(
  {{fn:"publicar",args:[{{nombre:"root"}},"colado"]}},
  {{...J, authorization:"Bearer el-secreto"}}
)).status;

// 4. Token bueno por cabecera y por cookie.
out.conBearer = (await pedir({{fn:"publicar",args:["con bearer"]}}, {{...J, authorization:"Bearer el-secreto"}})).status;
out.conCookie = (await pedir({{fn:"publicar",args:["con cookie"]}}, {{...J, cookie:"otra=1; marea_session=el-secreto"}})).status;

// 5. Lo declarado @server(Public) sigue respondiendo sin token.
const feed = await pedir({{fn:"feed",args:[]}}, J);
out.publicoSinToken = feed.status;

// 6. Lo que quedó guardado: sólo lo de las dos peticiones autorizadas, y con el
//    autor que puso el servidor (no el que mandó el cliente).
const filas = JSON.parse(feed.texto).ok;
out.filas = filas.length;
out.autoresOk = filas.every((p) => p.autor === "ada") ? 1 : 0;
out.textosOk = filas.map((p) => p.texto).join("|") === "con bearer|con cookie" ? 1 : 0;

console.log("RESULTADO:" + JSON.stringify(out));
await stopServer();
"#
    );
    let hijo = Command::new("node")
        .args(["--input-type=module", "-e", &guion])
        // Puerto propio: cargo corre los binarios de test EN PARALELO y todos
        // estos levantan el servidor. Con el 8787 por defecto, el segundo que
        // arranca muere con EADDRINUSE, y de forma no determinista.
        .env("MAREA_PORT", "8804")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanzar node");
    let salida = hijo.wait_with_output().expect("esperar node");
    let txt = String::from_utf8_lossy(&salida.stdout).to_string();
    if !txt.contains("RESULTADO:") {
        panic!(
            "el guion no completó.\nstdout: {txt}\nstderr: {}",
            String::from_utf8_lossy(&salida.stderr)
        );
    }
    txt
}

fn campo(salida: &str, clave: &str) -> i64 {
    let json = salida.split("RESULTADO:").nth(1).unwrap().trim();
    let pat = format!("\"{clave}\":");
    let resto = &json[json.find(&pat).expect("clave ausente") + pat.len()..];
    let fin = resto
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(resto.len());
    resto[..fin].parse().expect("número")
}

#[test]
fn el_runtime_aplica_la_politica() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite la prueba de integración");
        return;
    }
    let dir = std::env::temp_dir().join("marea-politica-int");
    let dir = dir.to_string_lossy().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    generar(&dir);
    let s = atacar(&dir, 9791);

    // Sin identidad no se entra, y da igual por qué.
    assert_eq!(campo(&s, "sinToken"), 401, "sin token:\n{s}");
    assert_eq!(campo(&s, "tokenMalo"), 401, "token inválido:\n{s}");
    assert_eq!(
        campo(&s, "mismoCuerpo"),
        1,
        "las dos negativas deben ser idénticas:\n{s}"
    );
    assert_eq!(
        campo(&s, "filtra"),
        0,
        "el 401 no debe decir qué falló:\n{s}"
    );

    // La identidad no se manda: intentarlo es un argumento de más.
    assert_eq!(campo(&s, "forjada"), 400, "identidad como argumento:\n{s}");

    // Con identidad sí, por cabecera o por cookie.
    assert_eq!(campo(&s, "conBearer"), 200, "token en authorization:\n{s}");
    assert_eq!(campo(&s, "conCookie"), 200, "token en cookie:\n{s}");

    // Lo público sigue siendo público.
    assert_eq!(
        campo(&s, "publicoSinToken"),
        200,
        "@server(Public) sin token:\n{s}"
    );

    // Y el cuerpo de las peticiones sin identidad NO llegó a ejecutarse.
    assert_eq!(campo(&s, "filas"), 2, "sólo las dos autorizadas:\n{s}");
    assert_eq!(
        campo(&s, "autoresOk"),
        1,
        "el autor lo pone el servidor:\n{s}"
    );
    assert_eq!(campo(&s, "textosOk"), 1, "los textos guardados:\n{s}");

    let _ = std::fs::remove_dir_all(&dir);
}
