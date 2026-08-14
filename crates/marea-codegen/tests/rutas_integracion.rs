//! Prueba de INTEGRACIÓN del enrutado: se levanta el servidor generado y se le
//! piden URLs de verdad.
//!
//! Es a propósito una prueba de runtime y no de texto emitido. Que `server.ts`
//! contenga la línea `__ruta("/modelo/:id", ...)` no dice nada de lo que importa
//! —si `/modelo/7` responde con la ficha, si `/modelo/abc` es un 404 y no un
//! 500, si `robots.txt` sale con su content-type— y esas tres cosas viven en el
//! casado de la URL, en la conversión de los segmentos y en la escritura de la
//! respuesta. Un test que mirara la plantilla pasaría igual con el casado
//! invertido.
//!
//! Y la que no se puede hacer de ninguna otra forma: DOS PETICIONES A LA VEZ con
//! querys distintas. `query("q")` lee la query string de la petición en
//! curso, y el servidor atiende varias a la vez; con una variable de módulo
//! guardando "la petición actual", dos peticiones simultáneas se pisan y una
//! acaba leyendo los filtros de la otra. Con una sola petición eso no se ve
//! nunca —por eso llega a producción—, así que aquí se lanzan varias en paralelo
//! y cada página, ADEMÁS, espera a la red en medio: sin esa espera las dos
//! peticiones se atenderían uno detrás de otro y el fallo seguiría escondido.
//!
//! Se salta solo (sin fallar) si no hay `node` en el PATH.

use std::process::{Command, Stdio};

/// El sitio de prueba: las cuatro formas de ruta que el diseño distingue
/// (segmento tipado, ruta fija que compite con una dinámica, `Response` que no
/// es HTML, y query string), más una `@server` para comprobar que el endpoint
/// RPC sigue en su sitio.
const FUENTE: &str = r#"
@page("/modelo/:id")
fn modelo(id: Int) -> Page | NotFound {
    if id > 100 {
        return NotFound;
    }
    return Page {
        titulo: "TITULO-QUE-NO-SE-EMITE-TODAVIA",
        descripcion: "DESCRIPCION-QUE-TAMPOCO",
        canonica: "https://ejemplo.mx/CANONICA-QUE-TAMPOCO",
        cuerpo: `<h1>modelo {text(id)}</h1>`,
    };
}

@page("/tienda/:slug")
fn tienda(slug: String) -> Page {
    return Page { titulo: "t", cuerpo: `<h1>tienda {slug}</h1>` };
}

@page("/tienda/madrid")
fn madrid() -> Page {
    return Page { titulo: "t", cuerpo: `<h1>la de madrid</h1>` };
}

@page("/par/:x/:y")
fn par(y: String, x: String) -> Page {
    return Page { titulo: "t", cuerpo: `<p>x={x} y={y}</p>` };
}

@page("/robots.txt")
fn robots() -> Response {
    return plainText("User-agent: *");
}

@page("/sitemap.xml")
fn sitemap() -> Response {
    return xmlDoc(`<urlset><url>{"tuercas & tornillos"}</url></urlset>`);
}

@page("/buscar")
fn buscar() -> Page {
    let antes = query("q");
    let pagina = match parseInt(query("pagina")) {
        NotANumber => 1,
        n => n,
    };
    let eco = fetch("http://127.0.0.1:PUERTO/robots.txt");
    let despues = query("q");
    return Page {
        titulo: "t",
        cuerpo: `<p>{antes}|{despues}|{text(pagina)}|{text(len(eco))}</p>`,
    };
}

@server
fn saludar(n: String) -> String {
    return concat("hola ", n);
}
"#;

/// El guion que ataca al servidor. Vive aparte del `format!` de Rust a
/// propósito: lleva llaves de JavaScript en cada línea y doblarlas todas lo
/// volvería ilegible, que es como se cuela un error en un test.
const GUION: &str = r#"
process.env.PORT = "PUERTO";
process.env.MAREA_WEB_ROOT = "DIR";
// La página '/buscar' se pide a sí misma para tener una espera de red DE VERDAD
// en medio: es lo que garantiza que las peticiones concurrentes se solapen.
process.env.MAREA_HTTP_HOSTS = "127.0.0.1";
await import("DIR/server.ts");
const { startServer, stopServer } = await import("DIR/runtime.ts");
await startServer();

const base = "http://127.0.0.1:PUERTO";
const pedir = async (ruta) => {
  const r = await fetch(base + ruta);
  return { status: r.status, tipo: r.headers.get("content-type") ?? "", texto: await r.text() };
};
const si = (b) => (b ? 1 : 0);
const out = {};

// --- una página con segmento tipado ---
const m = await pedir("/modelo/7");
out.modeloStatus = m.status;
out.modeloEsHtml = si(m.tipo === "text/html; charset=utf-8");
out.modeloCuerpo = si(m.texto === "<h1>modelo 7</h1>");
// Los metadatos se tipan pero NO se emiten todavía: sólo sale el cuerpo.
out.sinMetadatos = si(!/TITULO-QUE-NO|DESCRIPCION-QUE|CANONICA-QUE/.test(m.texto));

// --- una URL con basura donde va un Int: 404, no 500 ---
const basura = await pedir("/modelo/abc");
out.basuraStatus = basura.status;
out.basuraTieneCuerpo = si(basura.texto.includes("No encontrado"));
// Ni el desbordado, ni el decimal, ni lo que `Number` sí aceptaría: "0x10" es
// 16 para JavaScript y no es ningún entero escrito en una URL.
out.desbordadoStatus = (await pedir("/modelo/99999999999999999999")).status;
out.decimalStatus = (await pedir("/modelo/7.5")).status;
out.hexStatus = (await pedir("/modelo/0x10")).status;
out.exponenteStatus = (await pedir("/modelo/7e2")).status;
// La barra final no es la misma dirección.
out.barraFinalStatus = (await pedir("/modelo/7/")).status;

// --- la rama de fallo del tipo de retorno es el 404 ---
out.noEncontradoStatus = (await pedir("/modelo/999")).status;

// --- la ruta fija gana a la dinámica, esté donde esté en el archivo ---
const madrid = await pedir("/tienda/madrid");
out.fijaGana = si(madrid.texto === "<h1>la de madrid</h1>");
out.dinamicaSirve = si((await pedir("/tienda/sevilla")).texto === "<h1>tienda sevilla</h1>");

// --- los segmentos se atan POR NOMBRE, no por posición ---
// 'par' declara (y, x) y la ruta es /par/:x/:y: si se ataran por posición, se
// cruzarían en silencio (los dos son String y nada fallaría).
out.porNombre = si((await pedir("/par/AA/BB")).texto === "<p>x=AA y=BB</p>");

// --- lo que no es HTML lleva su propio content-type ---
const rob = await pedir("/robots.txt");
out.robotsTipo = si(rob.tipo === "text/plain; charset=utf-8");
out.robotsCuerpo = si(rob.texto === "User-agent: *");
const sit = await pedir("/sitemap.xml");
out.sitemapTipo = si(sit.tipo === "application/xml; charset=utf-8");
// El hueco de plantilla escapa igual que en HTML, que es justo lo que hace que
// la garantía de Html valga para XML sin inventar un tipo nuevo.
out.sitemapEscapa = si(sit.texto === "<urlset><url>tuercas &amp; tornillos</url></urlset>");

// --- una dirección que no sirve nadie ---
out.inexistenteStatus = (await pedir("/no/existe")).status;

// --- las rutas conviven con el RPC y con los estáticos ---
const rpc = await fetch(base + "/__marea", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ fn: "saludar", args: ["marea"] }),
});
out.rpcStatus = rpc.status;
out.rpcCuerpo = si((await rpc.json()).ok === "hola marea");
out.estaticoStatus = (await pedir("/client.js")).status;
// Una @page no se expone además por RPC: es otra puerta.
const paginaPorRpc = await fetch(base + "/__marea", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ fn: "modelo", args: [7] }),
});
out.paginaNoEsRpc = paginaPorRpc.status;

// --- LA PRUEBA DE LA CONCURRENCIA ---
// Ocho peticiones a la vez, cada una con su 'q'. Cada página lee 'q', espera a
// la red y la vuelve a leer: con "la petición actual" en una variable de módulo,
// la segunda lectura sería la de otra petición.
const etiquetas = ["uno", "dos", "tres", "cuatro", "cinco", "seis", "siete", "ocho"];
const a_la_vez = await Promise.all(
  etiquetas.map((q, i) => pedir("/buscar?q=" + q + "&pagina=" + (i + 1))),
);
out.concurrentesOk = si(
  a_la_vez.every((r, i) => r.texto === "<p>" + etiquetas[i] + "|" + etiquetas[i] + "|" + (i + 1) + "|13</p>"),
);
out.concurrentesVistos = a_la_vez.map((r) => r.texto).join(" ");

// Un parámetro que no está en la query es la cadena vacía, no un error.
out.sinQuery = si((await pedir("/buscar")).texto === "<p>||1|13</p>");

console.log("RESULTADO:" + JSON.stringify(out));
await stopServer();
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

/// Genera el sitio, levanta el servidor y le pide URLs desde el propio proceso
/// de Node. Devuelve su stdout, con el JSON detrás de `RESULTADO:`.
///
/// Cada test trae SU directorio y SU puerto: cargo corre los tests de un mismo
/// binario en paralelo, y con el puerto compartido el segundo moriría con
/// EADDRINUSE de forma no determinista.
fn servir(nombre: &str, puerto: u16) -> String {
    let dir = std::env::temp_dir().join(nombre);
    let dir = dir.to_string_lossy().to_string();
    let _ = std::fs::remove_dir_all(&dir);

    let fuente = FUENTE.replace("PUERTO", &puerto.to_string());
    let module = marea_syntax::parse(&fuente).expect("el sitio debe parsear");
    let app = marea_codegen::emit_app(&module);
    std::fs::create_dir_all(&dir).expect("crear dir");
    for (n, c) in [
        ("runtime.ts", app.runtime),
        ("server.ts", app.server),
        ("serve.ts", app.serve),
        ("client.js", app.client_js),
        ("index.html", app.index_html),
    ] {
        std::fs::write(format!("{dir}/{n}"), c).expect("escribir");
    }

    let con_dir = GUION.replace("DIR", &dir);
    let guion = con_dir.replace("PUERTO", &puerto.to_string());
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
    let _ = std::fs::remove_dir_all(&dir);
    txt
}

fn campo(salida: &str, clave: &str) -> i64 {
    let json = salida.split("RESULTADO:").nth(1).unwrap().trim();
    let pat = format!("\"{clave}\":");
    let i = json.find(&pat).expect("clave ausente");
    let resto = &json[i + pat.len()..];
    let fin = resto
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(resto.len());
    resto[..fin].parse().expect("número")
}

/// Igual, para un campo de TEXTO. Sólo se usa para contar qué pasó cuando algo
/// falla: el veredicto lo dan los campos numéricos.
fn texto(salida: &str, clave: &str) -> String {
    let json = salida.split("RESULTADO:").nth(1).unwrap().trim();
    let pat = format!("\"{clave}\":\"");
    let Some(i) = json.find(&pat) else {
        return String::new();
    };
    let resto = &json[i + pat.len()..];
    let fin = resto.find('"').unwrap_or(resto.len());
    resto[..fin].to_string()
}

#[test]
fn el_servidor_sirve_el_sitio() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite la prueba de enrutado");
        return;
    }
    let s = servir("marea-rutas-sitio", 8806);

    // Una página, con su segmento convertido y su content-type.
    assert_eq!(campo(&s, "modeloStatus"), 200, "/modelo/7:\n{s}");
    assert_eq!(campo(&s, "modeloEsHtml"), 1, "una Page es HTML:\n{s}");
    assert_eq!(campo(&s, "modeloCuerpo"), 1, "el cuerpo de la ficha:\n{s}");
    assert_eq!(
        campo(&s, "sinMetadatos"),
        1,
        "los metadatos NO se emiten todavía:\n{s}"
    );

    // Una URL con basura donde va un Int es una URL que no existe.
    for (clave, que) in [
        ("basuraStatus", "/modelo/abc"),
        ("desbordadoStatus", "un parseInt que no cabe"),
        ("decimalStatus", "/modelo/7.5"),
        ("hexStatus", "/modelo/0x10"),
        ("exponenteStatus", "/modelo/7e2"),
        ("barraFinalStatus", "/modelo/7/"),
        ("noEncontradoStatus", "la variante de fallo"),
        ("inexistenteStatus", "una dirección de nadie"),
    ] {
        assert_eq!(campo(&s, clave), 404, "{que} debe ser 404 (no 500):\n{s}");
    }
    assert_eq!(
        campo(&s, "basuraTieneCuerpo"),
        1,
        "el 404 lleva cuerpo:\n{s}"
    );

    // El orden de la tabla lo decide la especificidad, no el archivo.
    assert_eq!(campo(&s, "fijaGana"), 1, "/tienda/madrid:\n{s}");
    assert_eq!(campo(&s, "dinamicaSirve"), 1, "/tienda/sevilla:\n{s}");
    assert_eq!(
        campo(&s, "porNombre"),
        1,
        "los segmentos se atan por nombre:\n{s}"
    );

    // Lo que no es HTML.
    assert_eq!(campo(&s, "robotsTipo"), 1, "robots.txt no es HTML:\n{s}");
    assert_eq!(
        campo(&s, "robotsCuerpo"),
        1,
        "el cuerpo de robots.txt:\n{s}"
    );
    assert_eq!(
        campo(&s, "sitemapTipo"),
        1,
        "sitemap.xml es application/xml:\n{s}"
    );
    assert_eq!(
        campo(&s, "sitemapEscapa"),
        1,
        "el XML escapa como el HTML:\n{s}"
    );

    // Y todo lo que ya servía sigue sirviendo.
    assert_eq!(campo(&s, "rpcStatus"), 200, "el RPC de /__marea:\n{s}");
    assert_eq!(campo(&s, "rpcCuerpo"), 1, "la respuesta del RPC:\n{s}");
    assert_eq!(campo(&s, "estaticoStatus"), 200, "client.js:\n{s}");
    assert_eq!(
        campo(&s, "paginaNoEsRpc"),
        400,
        "una @page no se expone por RPC:\n{s}"
    );
}

/// La parte que sólo se rompe con carga: `query` bajo peticiones
/// simultáneas. Va en su propio test para que, cuando falle, el nombre diga qué
/// se rompió sin tener que leer el resto.
#[test]
fn dos_peticiones_a_la_vez_no_se_pisan_la_query() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite la prueba de concurrencia");
        return;
    }
    let s = servir("marea-rutas-concurrencia", 8807);

    assert_eq!(
        campo(&s, "concurrentesOk"),
        1,
        "cada petición debe leer SU query string, no la de la de al lado.\n\
         Lo que devolvió cada una: {}\n{s}",
        texto(&s, "concurrentesVistos")
    );
    assert_eq!(
        campo(&s, "sinQuery"),
        1,
        "un parámetro ausente es la cadena vacía:\n{s}"
    );
}
