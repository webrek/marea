//! Prueba de INTEGRACIÓN del ALMACÉN PRESTADO (`store x: T from "tabla"`).
//!
//! La propiedad que hay que demostrar no se ve en el texto generado: es que
//! Marea LEE una tabla que no creó ella. Así que aquí la tabla la crea el test
//! con SQL a mano —sin `__id`, sin `__doc`, con una columna de más que Marea no
//! conoce, como la tendría el motor en Go que la escribe de verdad— y lo que se
//! comprueba es que el programa Marea la lee sin haberla tocado.
//!
//! Y la contrapartida: prestar una tabla renuncia a la garantía de esquema, así
//! que lo que se puede exigir es que la deriva se DETECTE y se diga cuál es la
//! columna que falta, en vez de que el valor salga `undefined` tres capas más
//! abajo.
//!
//! Usa `node:sqlite`, integrado en Node desde la 22: cero dependencias. Se salta
//! solo (sin fallar) si no hay `node`.

use std::process::{Command, Stdio};

/// El programa: un almacén PRESTADO (la tabla `products` es de otro) y uno
/// PROPIO (`notas`, que Marea sí crea y escribe). Los dos juntos a propósito:
/// mezclarlos no debe recortar la escritura del propio ni abrirla en el prestado.
///
/// `colar` escribe en el prestado, que es un error de TIPOS —de la otra mitad
/// del compilador—; esta prueba no pasa por el verificador, así que la llamada
/// llega al runtime y sirve para comprobar la última línea de defensa: que falle
/// diciendo por qué en vez de escribir en la tabla de otro.
const FUENTE: &str = r#"
type Producto = { sku: String, nombre: String, precio: Int, activo: Bool, etiquetas: List<String> };
type Nota = { texto: String };

store productos: Producto from "products";
store notas: Nota;

@server fn catalogo() -> List<Producto> { return all(productos); }
@server fn anotar(t: String) { save(notas, Nota { texto: t }); }
@server fn colar() {
    save(productos, Producto { sku: "x", nombre: "y", precio: 0, activo: true, etiquetas: ["z"] });
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

/// Corre un módulo ESM suelto con node y devuelve (stdout, stderr).
fn node(guion: &str) -> (String, String) {
    let hijo = Command::new("node")
        .args(["--input-type=module", "-e", guion])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanzar node");
    let s = hijo.wait_with_output().expect("esperar node");
    (
        String::from_utf8_lossy(&s.stdout).to_string(),
        String::from_utf8_lossy(&s.stderr).to_string(),
    )
}

/// Ejecuta SQL contra una base SQLite SIN pasar por Marea: es el "otro
/// programa", el que es dueño de la tabla.
fn sql(db: &str, sentencias: &[&str]) {
    let mut g = format!(
        "const {{ DatabaseSync }} = await import(\"node:sqlite\");\n\
         const d = new DatabaseSync(\"{db}\");\n"
    );
    for s in sentencias {
        g.push_str(&format!("d.exec(`{s}`);\n"));
    }
    g.push_str("d.close();\nconsole.log(\"SQL-OK\");\n");
    let (out, err) = node(&g);
    assert!(out.contains("SQL-OK"), "SQL de preparación: {err}");
}

/// Consulta la base directamente y devuelve las filas como JSON.
fn consulta(db: &str, q: &str) -> String {
    let g = format!(
        "const {{ DatabaseSync }} = await import(\"node:sqlite\");\n\
         const d = new DatabaseSync(\"{db}\");\n\
         console.log(\"FILAS:\" + JSON.stringify(d.prepare(`{q}`).all()));\n"
    );
    let (out, err) = node(&g);
    assert!(out.contains("FILAS:"), "la consulta falló: {err}");
    out.split("FILAS:").nth(1).unwrap().trim().to_string()
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

/// Levanta el servidor generado, corre el guion contra él y devuelve
/// (stdout, stderr).
///
/// El fallo de un almacén prestado sale por dos sitios distintos según cuándo se
/// descubre: los de configuración revientan AL IMPORTAR (antes de que haya
/// servidor) y salen por stdout marcados `ARRANQUE:`; los de lectura revientan
/// dentro del handler, así que el cliente ve un 500 genérico y el motivo se
/// queda en el stderr del servidor. Por eso se devuelven los dos flujos.
fn correr(dir: &str, db: Option<&str>, puerto: u16, guion: &str) -> (String, String) {
    // Sin base de datos, el backend por defecto es el log de archivo: justo el
    // caso que un almacén prestado no puede atender.
    let backend = match db {
        Some(_) => "sqlite",
        None => "file",
    };
    let url = db.unwrap_or("");
    let completo = format!(
        r#"
process.env.MAREA_DB = "{backend}";
process.env.MAREA_DB_URL = "{url}";
process.env.MAREA_STORE_DIR = "{dir}";
process.env.PORT = "{puerto}";
let arranco = true;
try {{
  await import("{dir}/server.ts");
}} catch (e) {{
  arranco = false;
  console.log("ARRANQUE:" + e.message);
}}
if (arranco) {{
  const {{ startServer, stopServer }} = await import("{dir}/runtime.ts");
  await startServer();
  const U = "http://127.0.0.1:{puerto}/__marea";
  const c = (fn, ...args) => fetch(U, {{
    method: "POST", headers: {{ "content-type": "application/json" }},
    body: JSON.stringify({{ fn, args }}),
  }}).then((r) => r.json());
{guion}
  await stopServer();
}}
"#
    );
    let hijo = Command::new("node")
        .args(["--input-type=module", "-e", &completo])
        // Puerto propio: cargo corre los binarios de test EN PARALELO y todos
        // estos levantan un servidor.
        .env("MAREA_PORT", "8806")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("lanzar node");
    let s = hijo.wait_with_output().expect("esperar node");
    (
        String::from_utf8_lossy(&s.stdout).to_string(),
        String::from_utf8_lossy(&s.stderr).to_string(),
    )
}

fn temporal(nombre: &str) -> String {
    let d = std::env::temp_dir().join(format!("marea-prestado-{nombre}"));
    let s = d.to_string_lossy().to_string();
    let _ = std::fs::remove_dir_all(&s);
    s
}

/// Las tablas que existen ahora mismo en la base, como JSON.
fn tablas(db: &str) -> String {
    consulta(db, "SELECT name FROM sqlite_master WHERE type='table'")
}

#[test]
fn marea_lee_una_tabla_que_no_creo() {
    if !hay_node() {
        eprintln!("sin node: se omite la prueba del almacén prestado");
        return;
    }
    let dir = temporal("lee");
    generar(&dir);
    let db = format!("{dir}/ajena.sqlite");

    // La tabla la crea OTRO. Nótese: sin '__id' y sin '__doc' —una tabla ajena
    // no tiene las columnas internas de Marea— y con una columna 'interno' que
    // el tipo de Marea no menciona, como la tendría cualquier tabla de verdad.
    sql(
        &db,
        &[
            "CREATE TABLE products (sku TEXT, nombre TEXT, precio INTEGER, \
             activo INTEGER, etiquetas TEXT, interno TEXT)",
            "INSERT INTO products VALUES \
             ('SKU-1','Taladro',189900,1,'[\"a\",\"b\"]','no es de marea')",
            "INSERT INTO products VALUES ('SKU-2','Brocas',34900,0,'[]','tampoco')",
        ],
    );

    let (out, err) = correr(
        &dir,
        Some(&db),
        9821,
        r#"
  const cat = (await c("catalogo")).ok;
  console.log("RESULTADO:" + JSON.stringify(cat));
"#,
    );
    let Some(resto) = out.split("RESULTADO:").nth(1) else {
        panic!("el guion no completó.\nstdout: {out}\nstderr: {err}");
    };
    let cat = resto.trim();

    // Las filas que escribió el otro programa, leídas por Marea.
    assert!(cat.contains("\"sku\":\"SKU-1\""), "{cat}");
    assert!(cat.contains("\"nombre\":\"Taladro\""), "{cat}");
    assert!(cat.contains("\"precio\":189900"), "{cat}");
    assert!(cat.contains("\"sku\":\"SKU-2\""), "{cat}");
    // Bool y JSON se convierten igual que en un almacén propio.
    assert!(cat.contains("\"activo\":true"), "{cat}");
    assert!(cat.contains("\"activo\":false"), "{cat}");
    assert!(cat.contains(r#""etiquetas":["a","b"]"#), "{cat}");
    // La columna que Marea no declara NO se cuela en el valor.
    assert!(!cat.contains("interno"), "columna ajena filtrada: {cat}");
    assert!(!cat.contains("__id"), "{cat}");

    // Y lo central: Marea no creó nada. Si tratara el almacén como propio,
    // habría una tabla 'productos' (el nombre del almacén) junto a la ajena.
    let t = tablas(&db);
    assert!(!t.contains("\"productos\""), "Marea creó su tabla: {t}");
    assert!(t.contains("\"products\""), "{t}");
    let cols = consulta(&db, "PRAGMA table_info('products')");
    assert!(
        !cols.contains("__id") && !cols.contains("__doc"),
        "la tabla ajena no lleva columnas internas de Marea: {cols}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn una_columna_que_falta_se_dice_por_su_nombre() {
    if !hay_node() {
        eprintln!("sin node: se omite la prueba del almacén prestado");
        return;
    }
    let dir = temporal("falta");
    generar(&dir);
    let db = format!("{dir}/ajena.sqlite");

    // El dueño de la tabla le quitó 'precio' (o nunca la tuvo): es la deriva de
    // esquema que prestar una tabla hace posible.
    sql(
        &db,
        &["CREATE TABLE products (sku TEXT, nombre TEXT, activo INTEGER, etiquetas TEXT)"],
    );

    let (out, err) = correr(
        &dir,
        Some(&db),
        9822,
        r#"
  const r = await c("catalogo");
  console.log("RESULTADO:" + JSON.stringify(r));
"#,
    );
    assert!(out.contains("RESULTADO:"), "el guion no completó: {err}");
    // El cliente ve un error genérico (no se le da un oráculo del esquema)...
    assert!(out.contains("error"), "la lectura debe fallar: {out}");
    // ...y el servidor dice exactamente qué falta.
    assert!(
        err.contains("'precio'"),
        "el error debe nombrar la columna que falta:\n{err}"
    );
    assert!(err.contains("products"), "y la tabla:\n{err}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn una_tabla_que_no_existe_no_se_crea() {
    if !hay_node() {
        eprintln!("sin node: se omite la prueba del almacén prestado");
        return;
    }
    let dir = temporal("sin-tabla");
    generar(&dir);
    let db = format!("{dir}/ajena.sqlite");
    sql(&db, &["CREATE TABLE otra (x INTEGER)"]);

    let (out, err) = correr(
        &dir,
        Some(&db),
        9823,
        r#"
  const r = await c("catalogo");
  console.log("RESULTADO:" + JSON.stringify(r));
"#,
    );
    assert!(out.contains("RESULTADO:"), "el guion no completó: {err}");
    assert!(
        err.contains("no encuentra la tabla"),
        "debe decir que no la encuentra, no crearla:\n{err}"
    );
    // Lo que NO puede haber pasado: que Marea la haya creado por su cuenta.
    let t = tablas(&db);
    assert!(!t.contains("products"), "Marea creó la tabla ajena: {t}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn el_backend_de_archivo_no_puede_prestar() {
    if !hay_node() {
        eprintln!("sin node: se omite la prueba del almacén prestado");
        return;
    }
    let dir = temporal("archivo");
    generar(&dir);

    // Sin MAREA_DB el backend es el log JSONL que escribe Marea: ahí no hay
    // ninguna tabla ajena que leer, así que hay que decirlo AL ARRANCAR.
    let (out, err) = correr(&dir, None, 9824, "  console.log(\"NO-DEBERIA\");\n");
    assert!(
        out.contains("ARRANQUE:"),
        "debe fallar al importar, no a mitad de una petición:\nstdout: {out}\nstderr: {err}"
    );
    assert!(!out.contains("NO-DEBERIA"), "no debe arrancar: {out}");
    assert!(out.contains("MAREA_DB"), "debe nombrar MAREA_DB: {out}");
    assert!(out.contains("sqlite"), "y qué usar: {out}");
    // Y no debe haberse escrito un log para un almacén que no es suyo.
    let log = format!("{dir}/marea-store.productos.log");
    assert!(
        !std::path::Path::new(&log).exists(),
        "no debe escribir un log para un almacén prestado"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn escribir_en_un_prestado_no_toca_la_tabla_de_otro() {
    if !hay_node() {
        eprintln!("sin node: se omite la prueba del almacén prestado");
        return;
    }
    let dir = temporal("escritura");
    generar(&dir);
    let db = format!("{dir}/ajena.sqlite");
    sql(
        &db,
        &[
            "CREATE TABLE products (sku TEXT, nombre TEXT, precio INTEGER, \
             activo INTEGER, etiquetas TEXT)",
            "INSERT INTO products VALUES ('SKU-1','Taladro',189900,1,'[]')",
        ],
    );

    let (out, err) = correr(
        &dir,
        Some(&db),
        9825,
        r#"
  const malo = await c("colar");
  const bueno = await c("anotar", "va al almacén propio");
  console.log("RESULTADO:" + JSON.stringify({ malo, bueno }));
"#,
    );
    assert!(out.contains("RESULTADO:"), "el guion no completó: {err}");
    // El intento de escritura falla y dice por qué.
    assert!(
        err.contains("prestado"),
        "el runtime debe rechazar la escritura:\n{err}"
    );
    // La tabla de otro sigue exactamente como estaba.
    let filas = consulta(&db, "SELECT COUNT(*) AS n FROM products");
    assert_eq!(filas, r#"[{"n":1}]"#, "se escribió en la tabla ajena");
    // El almacén PROPIO del mismo módulo sigue escribiendo con normalidad: la
    // sólo-lectura es por almacén, no por módulo.
    let notas = consulta(&db, "SELECT COUNT(*) AS n FROM notas");
    assert_eq!(notas, r#"[{"n":1}]"#, "el propio debe seguir vivo");

    let _ = std::fs::remove_dir_all(&dir);
}
