//! Prueba de INTEGRACIÓN de la red saliente: las defensas contra SSRF.
//!
//! Dar acceso a la red desde el servidor abre SSRF —el atacante consigue que el
//! servidor pida por él, y el servidor sí alcanza metadatos de nube y servicios
//! internos—. Estas comprobaciones son de runtime, así que hay que ejercitarlas
//! en runtime; no basta con buscar cadenas en la plantilla.
//!
//! No necesita internet: todos los casos deben ser RECHAZADOS antes de salir.

use std::process::{Command, Stdio};

fn hay_node() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[test]
fn las_defensas_contra_ssrf_bloquean() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite la prueba de red");
        return;
    }
    let dir = std::env::temp_dir().join("marea-red-int");
    let dir = dir.to_string_lossy().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear dir");

    let module = marea_syntax::parse("@server fn f(u: String) -> String { return fetch(u); }")
        .expect("parsea");
    let app = marea_codegen::emit(&module);
    std::fs::write(format!("{dir}/runtime.ts"), app.runtime).expect("escribir");

    let guion = format!(
        r#"
const rt = await import("{dir}/runtime.ts");
const casos = [
  "http://localhost:8080/",
  "http://127.0.0.1/",
  "http://169.254.169.254/latest/meta-data/",
  "http://10.0.0.5/",
  "http://192.168.1.1/",
  "http://172.16.0.1/",
  "http://[::1]/",
  "http://metadata.internal/",
  "file:///etc/passwd",
  "gopher://evil/",
  "no-es-una-url",
];
let bloqueados = 0;
for (const u of casos) {{
  try {{ await rt.fetch(u); }} catch {{ bloqueados++; }}
}}
// Con lista blanca, un host público que no esté en ella también se rechaza.
process.env.MAREA_HTTP_HOSTS = "api.ejemplo.com";
let listaOk = 0;
try {{ await rt.fetch("https://api.github.com/"); }} catch {{ listaOk = 1; }}
console.log("RESULTADO:" + JSON.stringify({{ bloqueados, total: casos.length, listaOk }}));
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
    assert!(
        txt.contains("RESULTADO:"),
        "el guion no completó.\nstdout: {txt}\nstderr: {}",
        String::from_utf8_lossy(&salida.stderr)
    );
    let json = txt.split("RESULTADO:").nth(1).unwrap().trim();
    assert!(
        json.contains(r#""bloqueados":11"#) && json.contains(r#""total":11"#),
        "los once destinos deben rechazarse: {json}"
    );
    assert!(json.contains(r#""listaOk":1"#), "la lista blanca debe aplicar: {json}");

    let _ = std::fs::remove_dir_all(&dir);
}
