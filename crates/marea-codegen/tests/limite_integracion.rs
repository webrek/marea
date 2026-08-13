//! Prueba de INTEGRACIÓN del límite de red: genera una app, levanta el servidor
//! de verdad y lo ataca.
//!
//! Los tests de `emit.rs` solo comprueban que ciertas cadenas aparecen en la
//! plantilla del runtime, así que pasarían igual con la comprobación de origen
//! invertida o con el `if` del content-type comentado. Las defensas son de
//! runtime; hay que ejercitarlas en runtime.
//!
//! Se salta solo (sin fallar) si no hay `node` en el PATH.

use std::process::{Command, Stdio};

const FUENTE: &str = r#"
type Post = { autor: String, likes: Int };
store posts: Post;
@server fn publicar(autor: String, likes: Int) { save(posts, Post { autor: autor, likes: likes }); }
@server fn feed() -> List<Post> { return all(posts); }
@client fn vista() -> Html { return "<p>x</p>"; }
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

/// Genera la app en un directorio temporal y devuelve su ruta.
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

/// Lanza el servidor, corre el guion de ataque en Node y devuelve su salida.
/// El guion hace las peticiones desde dentro del propio proceso para no
/// depender de curl ni de puertos fijos.
fn atacar(dir: &str, puerto: u16) -> String {
    let guion = format!(
        r#"
process.env.MAREA_STORE_DIR = "{dir}";
process.env.PORT = "{puerto}";
process.env.MAREA_WEB_ROOT = "{dir}";
await import("{dir}/server.ts");
const {{ startServer, stopServer }} = await import("{dir}/runtime.ts");
await startServer();
const U = "http://127.0.0.1:{puerto}/__marea";
const post = async (cuerpo, cab) => {{
  const r = await fetch(U, {{ method: "POST", headers: cab, body: cuerpo }});
  return r.status;
}};
const J = {{ "content-type": "application/json" }};
const out = {{}};
out.valido      = await post(JSON.stringify({{fn:"publicar",args:["ada",1]}}), J);
out.tipoMalo    = await post(JSON.stringify({{fn:"publicar",args:[{{"x":1}},1]}}), J);
out.aridad      = await post(JSON.stringify({{fn:"publicar",args:["ada"]}}), J);
out.intEnorme   = await post(JSON.stringify({{fn:"publicar",args:["ada",1e21]}}), J);
out.textPlain   = await post(JSON.stringify({{fn:"publicar",args:["ada",1]}}), {{"content-type":"text/plain"}});
out.ctMayus     = await post(JSON.stringify({{fn:"publicar",args:["ada",1]}}), {{"content-type":"Application/JSON"}});
out.origenMalo  = await post(JSON.stringify({{fn:"feed",args:[]}}), {{...J, origin:"http://evil.example"}});
out.sinHandler  = await post(JSON.stringify({{fn:"noExiste",args:[]}}), J);
const est = async (p) => (await fetch(`http://127.0.0.1:{puerto}${{p}}`)).status;
out.getServerTs = await est("/server.ts");
out.getIndex    = await est("/");
console.log("RESULTADO:" + JSON.stringify(out));
await stopServer();
"#
    );
    let hijo = Command::new("node")
        .args(["--input-type=module", "-e", &guion])
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
    let fin = resto.find(|c: char| !c.is_ascii_digit()).unwrap_or(resto.len());
    resto[..fin].parse().expect("número")
}

#[test]
fn el_limite_de_red_se_defiende_de_verdad() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite la prueba de integración");
        return;
    }
    let dir = std::env::temp_dir().join("marea-limite-int");
    let dir = dir.to_string_lossy().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    generar(&dir);
    let s = atacar(&dir, 9788);

    // Lo legítimo pasa.
    assert_eq!(campo(&s, "valido"), 200, "una llamada válida debe pasar:\n{s}");
    assert_eq!(campo(&s, "getIndex"), 200, "el index debe servirse:\n{s}");

    // Lo mal tipado es culpa del cliente: 400, no 500.
    assert_eq!(campo(&s, "tipoMalo"), 400, "objeto donde iba String:\n{s}");
    assert_eq!(campo(&s, "aridad"), 400, "aridad incorrecta:\n{s}");
    assert_eq!(campo(&s, "intEnorme"), 400, "1e21 no es un Int seguro:\n{s}");
    assert_eq!(campo(&s, "sinHandler"), 400, "handler inexistente:\n{s}");

    // CSRF: sin exigir JSON, un formulario cross-origin se salta el preflight.
    assert_eq!(campo(&s, "textPlain"), 415, "text/plain debe rechazarse:\n{s}");
    // …pero el media type es case-insensitive según la RFC.
    assert_eq!(campo(&s, "ctMayus"), 200, "Application/JSON debe aceptarse:\n{s}");

    // Origen ajeno.
    assert_eq!(campo(&s, "origenMalo"), 403, "origen ajeno:\n{s}");

    // El código fuente del servidor no se sirve (enumera los handlers).
    assert_eq!(campo(&s, "getServerTs"), 404, "server.ts no debe servirse:\n{s}");

    let _ = std::fs::remove_dir_all(&dir);
}
