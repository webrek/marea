//! Prueba de INTEGRACIÓN del backend SQLite.
//!
//! Hasta ahora todas las pruebas usaban el backend de archivo, así que la capa
//! SQL —esquema, tipos de columna, ida y vuelta de Bool y JSON, y el CRUD por
//! id— no se ejercitaba nunca. Dos fallos reales vivieron ahí sin que nadie los
//! notara: un campo llamado `__id` chocaba con la columna interna y dos
//! almacenes que solo diferían en mayúsculas caían en la misma tabla.
//!
//! Usa `node:sqlite`, integrado en Node desde la 22: cero dependencias. Se salta
//! solo (sin fallar) si no hay `node`.

use std::process::{Command, Stdio};

const FUENTE: &str = r#"
type Articulo = { sku: String, nombre: String, precio: Int, activo: Bool, etiquetas: List<String> };
type Movimiento = { sku: String, cantidad: Int };

store articulos: Articulo;
store movimientos: Movimiento;

@server fn alta(sku: String, nombre: String, precio: Int, etiquetas: List<String>) {
    guardar(articulos, Articulo { sku: sku, nombre: nombre, precio: precio, activo: true, etiquetas: etiquetas });
    guardar(movimientos, Movimiento { sku: sku, cantidad: 1 });
}
@server fn catalogo() -> List<Articulo> { return todos(articulos); }
@server fn bitacora() -> List<Movimiento> { return todos(movimientos); }
@server fn desactivar(i: Int) {
    let xs = todos(articulos);
    if i < len(xs) {
        let a = xs[i];
        actualizar(articulos, i, Articulo { sku: a.sku, nombre: a.nombre, precio: a.precio, activo: false, etiquetas: a.etiquetas });
    }
}
@server fn eliminar(i: Int) { borrar(articulos, i); }
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
    let app = marea_codegen::emit(&module);
    std::fs::create_dir_all(dir).expect("crear dir");
    for (n, c) in [
        ("runtime.ts", app.runtime),
        ("server.ts", app.server),
        ("client.ts", app.client),
    ] {
        std::fs::write(format!("{dir}/{n}"), c).expect("escribir");
    }
}

/// Corre el guion contra el servidor generado y devuelve su salida.
fn correr(dir: &str, db: &str, puerto: u16, guion: &str) -> String {
    let completo = format!(
        r#"
process.env.MAREA_DB = "sqlite";
process.env.MAREA_DB_URL = "{db}";
process.env.PORT = "{puerto}";
await import("{dir}/server.ts");
const {{ startServer, stopServer }} = await import("{dir}/runtime.ts");
await startServer();
const U = "http://127.0.0.1:{puerto}/__marea";
const c = (fn, ...args) => fetch(U, {{
  method: "POST", headers: {{ "content-type": "application/json" }},
  body: JSON.stringify({{ fn, args }}),
}}).then((r) => r.json());
{guion}
await stopServer();
"#
    );
    let hijo = Command::new("node")
        .args(["--input-type=module", "-e", &completo])
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
    txt.split("RESULTADO:").nth(1).unwrap().trim().to_string()
}

#[test]
fn el_backend_sqlite_persiste_y_recarga() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite la prueba de SQLite");
        return;
    }
    let base = std::env::temp_dir().join("marea-sqlite-int");
    let dir = base.to_string_lossy().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    generar(&dir);
    let db = format!("{dir}/prueba.sqlite");

    // Primer proceso: escribe.
    let s1 = correr(
        &dir,
        &db,
        9788,
        r#"
await c("alta", "SKU-1", "Taladro", 189900, ["herramienta", "electrico"]);
await c("alta", "SKU-2", "Brocas", 34900, ["accesorio"]);
await c("desactivar", 1);
const cat = (await c("catalogo")).ok;
console.log("RESULTADO:" + JSON.stringify({
  n: cat.length,
  activo0: cat[0].activo, activo1: cat[1].activo,
  etiquetas0: cat[0].etiquetas, movs: (await c("bitacora")).ok.length,
}));
"#,
    );
    // Cada almacén va a su tabla y el CRUD funciona.
    assert!(s1.contains("\"n\":2"), "dos artículos: {s1}");
    assert!(s1.contains("\"movs\":2"), "el otro almacén es independiente: {s1}");
    // Bool ida y vuelta (SQLite no tiene booleanos: se guardan como 0/1).
    assert!(s1.contains("\"activo0\":true"), "{s1}");
    assert!(s1.contains("\"activo1\":false"), "{s1}");
    // Una lista viaja como columna JSON y vuelve como lista.
    assert!(
        s1.contains(r#""etiquetas0":["herramienta","electrico"]"#),
        "la columna json debe recuperarse: {s1}"
    );

    // Segundo proceso: solo lee. Si el esquema o los tipos no cuadraran, aquí
    // se vería (es la recarga desde disco, no la copia en memoria).
    let s2 = correr(
        &dir,
        &db,
        9789,
        r#"
const cat = (await c("catalogo")).ok;
console.log("RESULTADO:" + JSON.stringify({ n: cat.length, sku0: cat[0].sku, activo1: cat[1].activo }));
"#,
    );
    assert!(s2.contains("\"n\":2"), "debe recargar de la base: {s2}");
    assert!(s2.contains("\"sku0\":\"SKU-1\""), "{s2}");
    assert!(s2.contains("\"activo1\":false"), "el Bool sobrevive al reinicio: {s2}");

    // Tercer proceso: borra por índice, que en la base es un DELETE por id.
    let s3 = correr(
        &dir,
        &db,
        9790,
        r#"
await c("eliminar", 0);
const cat = (await c("catalogo")).ok;
console.log("RESULTADO:" + JSON.stringify({ n: cat.length, sku0: cat[0].sku }));
"#,
    );
    assert!(s3.contains("\"n\":1"), "{s3}");
    assert!(s3.contains("\"sku0\":\"SKU-2\""), "debe quedar el otro: {s3}");

    let _ = std::fs::remove_dir_all(&dir);
}
