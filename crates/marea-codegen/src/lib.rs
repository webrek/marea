//! `marea-codegen` — transpilador de Marea a TypeScript.
//!
//! La rebanada angosta ("bala trazadora") que demuestra la tesis: una función
//! `@server` se vuelve (a) un handler registrado en el servidor y (b) un *stub*
//! en el cliente que cruza la frontera por RPC. El `@client` que la llama no
//! nota la diferencia: la sigue invocando como si fuera local.
//!
//! No hace chequeo de tipos (eso es v1.5). El objetivo es ver un `.mar` corriendo
//! end-to-end y validar el diseño.

use marea_syntax::ast::*;
use marea_syntax::builtins::es_sincrono;
use std::collections::HashSet;
use std::sync::LazyLock;

/// El NÚCLEO COMPARTIDO: la única implementación del núcleo reactivo y de los
/// builtins puros. Se inserta en los dos runtimes, así que hay un solo texto que
/// arreglar cuando algo está mal. Ver la cabecera de `nucleo.js`.
const NUCLEO_JS: &str = include_str!("nucleo.js");

/// Las plantillas tal cual viven en el repo: con el bloque `@marea:nucleo-*` sin
/// sustituir. No se exponen; lo que se emite es siempre la versión compuesta.
const RUNTIME_PLANTILLA: &str = include_str!("runtime.ts");
const BROWSER_PLANTILLA: &str = include_str!("browser.js");

/// El runtime TypeScript embebido (transporte RPC + builtins), ya con el núcleo
/// compartido dentro y sus anotaciones destapadas: es TypeScript de verdad.
pub fn runtime_ts() -> &'static str {
    static COMPUESTO: LazyLock<String> =
        LazyLock::new(|| con_nucleo(RUNTIME_PLANTILLA, &destapar_tipos(NUCLEO_JS)));
    &COMPUESTO
}

/// El runtime de NAVEGADOR embebido (cliente RPC + reactivo + DOM, sin Node), ya
/// con el núcleo compartido dentro y SIN anotaciones: JavaScript plano.
pub fn browser_rt() -> &'static str {
    static COMPUESTO: LazyLock<String> =
        LazyLock::new(|| con_nucleo(BROWSER_PLANTILLA, &quitar_tipos(NUCLEO_JS)));
    &COMPUESTO
}

/// Marca de apertura de una anotación de tipo dentro del núcleo compartido.
const MARCA: &str = "/*ts";

/// Recorre el núcleo separando lo que va dentro de `/*ts ... */` de lo que no, y
/// deja que quien llame decida qué hacer con cada trozo. Es la ÚNICA regla que
/// traduce entre los dos blancos, y por eso vive en un solo sitio: el contenido
/// de la marca es TypeScript literal, no hay conversión que pueda equivocarse.
///
/// Una marca sin cerrar sería un error de edición del núcleo que produciría
/// TypeScript inválido en silencio, así que revienta aquí, al construir.
fn recorrer_nucleo(js: &str, mut con_tipo: impl FnMut(&mut String, &str)) -> String {
    let mut out = String::with_capacity(js.len());
    let mut resto = js;
    while let Some(i) = resto.find(MARCA) {
        out.push_str(&resto[..i]);
        let tras = &resto[i + MARCA.len()..];
        let j = tras
            .find("*/")
            .expect("nucleo.js: una marca '/*ts' se quedó sin cerrar");
        con_tipo(&mut out, &tras[..j]);
        resto = &tras[j + 2..];
    }
    out.push_str(resto);
    out
}

/// El núcleo con sus anotaciones DESTAPADAS: `s/*ts: string*/` pasa a
/// `s: string`. Es lo que se inserta en runtime.ts, que es TypeScript y pasa por
/// `tsc --strict`.
fn destapar_tipos(js: &str) -> String {
    recorrer_nucleo(js, |out, tipo| out.push_str(tipo))
}

/// El núcleo con sus anotaciones BORRADAS: `s/*ts: string*/` pasa a `s`. Es lo
/// que se inserta en client.js, que el navegador ejecuta tal cual — y así el
/// archivo que llega al navegador no lleva ni rastro de tipos.
fn quitar_tipos(js: &str) -> String {
    recorrer_nucleo(js, |_out, _tipo| {})
}

/// Sustituye el bloque `@marea:nucleo-inicio/fin` de una plantilla por el núcleo
/// ya adaptado. Los marcadores hablan con el codegen, así que no se copian.
fn con_nucleo(plantilla: &str, nucleo: &str) -> String {
    let mut out = String::with_capacity(plantilla.len() + nucleo.len());
    let mut dentro = false;
    let mut sustituido = false;
    for linea in plantilla.lines() {
        if dentro {
            if linea.starts_with("// @marea:nucleo-fin") {
                dentro = false;
            }
            continue;
        }
        if linea.starts_with("// @marea:nucleo-inicio") {
            dentro = true;
            sustituido = true;
            out.push_str(nucleo.trim_end_matches('\n'));
            out.push('\n');
            continue;
        }
        out.push_str(linea);
        out.push('\n');
    }
    assert!(
        sustituido,
        "la plantilla perdió su bloque '@marea:nucleo-inicio': el núcleo compartido no se insertó"
    );
    out
}

/// Los cuatro archivos TypeScript que produce el transpilador.
pub struct Project {
    pub runtime: String,
    pub server: String,
    pub client: String,
    pub demo: String,
}

/// Los archivos de una app web completa (`marea build-app`): servidor Node
/// (runtime + handlers + entry) y cliente de navegador (HTML + JS).
pub struct AppProject {
    pub runtime: String,    // runtime.ts — servidor Node (RPC + store + estáticos)
    pub server: String,     // server.ts — handlers @server registrados
    pub serve: String,      // serve.ts — arranca el servidor y lo deja vivo
    pub client_js: String,  // client.js — cliente de navegador (RPC + reactivo + DOM)
    pub index_html: String, // index.html — la página
}

/// Builtins que existen SIEMPRE: cómputo puro sobre valores que ya se tienen.
/// Ninguno toca la red, el disco ni `process`, así que viven igual en Node, en
/// el navegador y en el edge.
const BUILTINS_PURO: &[&str] = &[
    "__badRequest",
    "print",
    "concat",
    "render",
    "len",
    "text",
    "escape",
    "html",
    // El modelo de eventos. Va en los builtins de SIEMPRE porque el despachador
    // no trae nada de Node: sin DOM (en el servidor) `on` registra el cierre y
    // no engancha nada, igual que `render` escribe en consola.
    "on",
    "__div",
    "__rem",
    "append",
    "contains",
    "lower",
    "jsonText",
    "jsonInt",
    "jsonFloat",
    "jsonLen",
    "__index",
    "__marea_is",
    "__signal",
    "__memo",
    "__resource",
    "__effect",
    // Texto a número. Vive en el núcleo compartido (es cómputo puro), así que
    // existe en los dos blancos y no depende del enrutado.
    "parseInt",
];

/// Los que sólo existen si el módulo SIRVE PÁGINAS (`@page`). Un módulo sin
/// rutas no tiene tabla que registrar ni petición en curso que consultar, y
/// pedir estos nombres le importaría cosas que su runtime no exporta.
const BUILTINS_PAGINA: &[&str] = &["__ruta", "query", "plainText", "xmlDoc"];

/// Los que sólo existen si el módulo CRUZA la frontera de red: el registro de
/// handlers, el cliente RPC y la red saliente. Un módulo que sólo calcula y
/// devuelve marcado no los usa, y pedirlos ataría su bundle a Node.
const BUILTINS_SERVIDOR: &[&str] = &["__register", "__rpc", "fetch", "post"];

/// Los que sólo existen si el módulo declara algún `store`. Importarlos cuando
/// no los hay traería un import a nombres que el runtime recortado ya no exporta.
const BUILTINS_STORE: &[&str] = &["__store", "save", "all", "update", "remove"];

/// Los de ESCRITURA. Un almacén prestado (`from "tabla"`) es de sólo lectura: la
/// tabla es de otro. Si TODOS los almacenes del módulo son prestados, estos tres
/// no se importan siquiera — el programa no tiene nada que escribir, y el
/// nombre no está ahí para llamarlo por descuido.
const BUILTINS_STORE_ESCRITURA: &[&str] = &["save", "update", "remove"];

/// ¿Este módulo puede vivir sin Node?
///
/// Sí cuando no cruza la frontera de red ni guarda estado: entonces todo lo que
/// hace es cómputo sobre sus argumentos, y el bundle resultante se puede
/// importar desde un componente de cliente o desde el edge. Es la diferencia
/// entre generar marcado y ser una aplicación.
///
/// Una página (`@page("/ruta")`) es lo contrario de puro: implica servidor —se
/// renderiza para que un buscador la lea— y necesita el `node:http` que sirve la
/// ruta. Sin esta condición, un módulo que sólo declarara páginas se emitía con
/// el runtime recortado y se quedaba sin servidor que las sirviera.
fn es_puro(module: &Module) -> bool {
    store_decls(module).is_empty()
        && !module
            .items
            .iter()
            .any(|it| matches!(it, Item::Fn(f) if is_remote(f) || f.es_session || es_pagina(f)))
}

/// La lista de importación del runtime para este módulo.
fn builtins_de(module: &Module) -> String {
    let mut ns: Vec<&str> = BUILTINS_PURO.to_vec();
    if !es_puro(module) {
        ns.extend_from_slice(BUILTINS_SERVIDOR);
    }
    if !paginas(module).is_empty() {
        ns.extend_from_slice(BUILTINS_PAGINA);
    }
    if !store_decls(module).is_empty() {
        let escribe = hay_almacen_propio(module);
        for &n in BUILTINS_STORE {
            if escribe || !BUILTINS_STORE_ESCRITURA.contains(&n) {
                ns.push(n);
            }
        }
    }
    format!("{{ {} }}", ns.join(", "))
}

/// El runtime que le toca a este módulo.
///
/// Sin `store`, se recorta todo lo marcado con `@marea:store-inicio` /
/// `@marea:store-fin`. No es una optimización de tamaño: ahí dentro hay tres
/// `import("pg" | "mysql2" | "mongodb")`, paquetes de npm que un consumidor sin
/// base de datos no instala, y un empaquetador (Next, Vite) RESUELVE ese
/// especificador aunque el código sea inalcanzable. Con ellos, el `.ts` generado
/// no se puede empaquetar: "Module not found: Can't resolve 'mongodb'".
fn runtime_de(module: &Module) -> String {
    let mut recortar: Vec<&str> = Vec::new();
    if store_decls(module).is_empty() {
        recortar.push("store");
    }
    if es_puro(module) {
        recortar.push("servidor");
    }
    let completo = runtime_ts();
    let mut out = String::with_capacity(completo.len());
    let mut dentro = false;
    for linea in completo.lines() {
        // Un marcador nunca se copia a la salida, se recorte su región o no:
        // habla con el codegen, no con quien lee el archivo generado.
        let es_marcador = linea.starts_with("// @marea:");
        if !dentro {
            if let Some(cual) = recortar
                .iter()
                .find(|c| linea.starts_with(&format!("// @marea:{c}-inicio")))
            {
                let _ = cual;
                dentro = true;
                continue;
            }
        } else if recortar
            .iter()
            .any(|c| linea.starts_with(&format!("// @marea:{c}-fin")))
        {
            dentro = false;
            continue;
        }
        if !dentro && !es_marcador {
            out.push_str(linea);
            out.push('\n');
        }
    }
    out
}

/// Los alias de `type` del módulo, para resolver los tipos declarados al emitir
/// los validadores del límite de red.
fn type_aliases(module: &Module) -> std::collections::HashMap<String, Type> {
    module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Type(t) => Some((t.name.clone(), t.aliased.clone())),
            _ => None,
        })
        .collect()
}

/// Las declaraciones `export type` del módulo.
///
/// Sin esto, el `.ts` generado referencia nombres que nunca declara: `map_type`
/// traduce `List<Post>` a `Post[]`, pero `Post` no existía en ninguna parte y
/// `tsc --strict` cortaba con "Cannot find name 'Post'". Se notó al meter la
/// salida en un proyecto con TypeScript estricto de verdad.
///
/// Una variante nominal se emite con la forma que YA tiene en runtime —el
/// `{ $tag: "NotFound" }` que el codegen construye—, así que el tipo describe el
/// valor real y no una aproximación.
/// Las declaraciones `export type` de un módulo y los nombres que declaran.
/// Van juntos porque uno no sirve sin el otro: una firma sólo puede citar un
/// tipo si el archivo lo declara.
struct Tipos {
    decls: String,
    nombres: HashSet<String>,
}

fn emit_type_decls(module: &Module) -> Tipos {
    fn ts_de(t: &Type, declarados: &HashSet<String>) -> String {
        match t {
            Type::Union { variants, .. } => variants
                .iter()
                .map(|v| match v {
                    // Un nombre que el módulo no declara es una variante
                    // nominal: en runtime es su etiqueta.
                    Type::Name { name, args, .. }
                        if args.is_empty()
                            && !declarados.contains(name)
                            && !matches!(
                                name.as_str(),
                                "Int" | "Float" | "Bool" | "String" | "Html" | "Unit" | "Record"
                            ) =>
                    {
                        format!("{{ $tag: \"{name}\" }}")
                    }
                    otro => ts_de(otro, declarados),
                })
                .collect::<Vec<_>>()
                .join(" | "),
            Type::Record { fields, .. } => {
                let ps: Vec<String> = fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, ts_de(&f.ty, declarados)))
                    .collect();
                format!("{{ {} }}", ps.join("; "))
            }
            otro => map_type(otro),
        }
    }
    let declarados: HashSet<String> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Type(t) => Some(t.name.clone()),
            _ => None,
        })
        .collect();
    let mut s = String::new();
    for it in &module.items {
        if let Item::Type(t) = it {
            s.push_str(&format!(
                "export type {} = {};\n",
                t.name,
                ts_de(&t.aliased, &declarados)
            ));
        }
    }
    if !s.is_empty() {
        s.push('\n');
    }
    Tipos {
        decls: s,
        nombres: declarados,
    }
}

/// Una función con `@server` o `@edge` corre "remota": handler + stub RPC.
fn is_remote(f: &FnDecl) -> bool {
    // Una `@session` NO se registra como handler: la invoca el runtime con el
    // token de la petición. Exponerla por RPC dejaría que cualquiera pidiera la
    // identidad de un token arbitrario, que es un oráculo de sesiones.
    !f.es_session && matches!(f.location, Some(Location::Server) | Some(Location::Edge))
}

/// ¿Esta función sirve una ruta? `@page("/modelo/:id")`.
fn es_pagina(f: &FnDecl) -> bool {
    f.ruta.is_some()
}

/// Reparte las funciones del módulo en los tres cubos que consumen los emisores:
/// `(remotas, locales, compartidas)`.
///
/// La `@session` va SÓLO al bundle del servidor aunque no lleve anotación de
/// ubicación: su cuerpo es quien decide qué token vale, y eso no tiene por qué
/// viajar al navegador. Va en `shared` porque el servidor sí la necesita —el
/// runtime la invoca en cada petición con política—, y fuera de `local`, que es
/// lo que se emite para el cliente.
///
/// Una `@page` va al mismo sitio y por una razón parecida: implica servidor —se
/// renderiza para que un buscador la lea—. Sin esto viajaba también al bundle
/// del navegador, que es donde no puede correr, y de paso mandaba al cliente el
/// cuerpo de una función que consulta la base de datos.
fn repartir(module: &Module) -> (Vec<&FnDecl>, Vec<&FnDecl>, Vec<&FnDecl>) {
    let mut remote = Vec::new();
    let mut local = Vec::new();
    // Sin anotación = local desde cualquier lado: va a AMBOS bundles.
    let mut shared = Vec::new();
    for item in &module.items {
        if let Item::Fn(f) = item {
            if f.es_session || es_pagina(f) {
                shared.push(f);
            } else if is_remote(f) {
                remote.push(f);
            } else {
                if f.location.is_none() {
                    shared.push(f);
                }
                local.push(f);
            }
        }
    }
    (remote, local, shared)
}

/// El nombre de la función `@session` del módulo, que es la que el runtime
/// invoca para traducir un token en una identidad. Hay como mucho una (el
/// verificador rechaza la segunda).
fn session_fn(module: &Module) -> Option<String> {
    module.items.iter().find_map(|it| match it {
        Item::Fn(f) if f.es_session => Some(f.name.clone()),
        _ => None,
    })
}

/// Las páginas del módulo, EN EL ORDEN EN QUE HAY QUE PROBARLAS.
///
/// La tabla se arma al compilar —la ruta es un literal justamente para eso— y
/// también se ORDENA al compilar, de más específica a más general: `/modelo/nuevo`
/// antes que `/modelo/:id`. Si mandara el orden del archivo, mover una función
/// treinta líneas más arriba cambiaría qué página sirve una URL, y eso es un
/// cambio de comportamiento que no se ve en el diff de la línea que se tocó.
///
/// El criterio, segmento a segmento: un literal es más específico que un
/// `:nombre`. Entre dos igual de específicas gana la que se declaró antes (el
/// orden es estable), que sólo puede pasar si sirven el mismo patrón.
fn paginas(module: &Module) -> Vec<&FnDecl> {
    let mut ps: Vec<&FnDecl> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Fn(f) if es_pagina(f) => Some(f),
            _ => None,
        })
        .collect();
    ps.sort_by_key(|f| especificidad(f.ruta.as_deref().unwrap_or("")));
    ps
}

/// La clave de orden de un patrón: 0 por segmento literal, 1 por dinámico. Se
/// compara elemento a elemento, así que decide el PRIMER segmento en que dos
/// patrones difieren, que es el mismo criterio con que se leen.
fn especificidad(ruta: &str) -> Vec<u8> {
    let dinamico = |s: &str| u8::from(s.starts_with(':'));
    ruta.split('/').map(dinamico).collect()
}

/// El tipo con el que el runtime convierte un segmento de URL. Sólo los
/// escalares tienen una lectura evidente desde texto; lo demás se pasa tal cual
/// (el verificador es quien decide si eso es legal).
fn tipo_segmento(t: &Type) -> &'static str {
    match t {
        Type::Name { name, args, .. } if args.is_empty() => match name.as_str() {
            "Int" => "Int",
            "Float" => "Float",
            "Bool" => "Bool",
            _ => "String",
        },
        _ => "String",
    }
}

/// La línea de tabla de una página: el patrón, los tipos de sus segmentos y el
/// envoltorio que reparte los segmentos —que llegan POR NOMBRE— a los
/// parámetros de la función en el orden en que ésta los declara.
///
/// Por nombre y no por posición porque es lo único que no miente: `@page(
/// "/a/:x/:y") fn f(y: Int, x: Int)` tiene que llamar a `f` con `y` primero, y
/// una tabla posicional los cruzaría en silencio —dos `Int`, ningún error—.
fn emit_ruta(f: &FnDecl) -> String {
    let patron = f.ruta.as_deref().unwrap_or("");
    let mut params: Vec<String> = Vec::new();
    for segmento in patron.split('/') {
        let Some(nombre) = segmento.strip_prefix(':') else {
            continue;
        };
        // Un `:nombre` que ningún parámetro recoge se lee como texto y no llega
        // a ninguna parte. Es un programa que el verificador rechaza; aquí sólo
        // hace falta que no invente un tipo que nadie declaró.
        let mut tipo = "String";
        for p in &f.params {
            if p.name == nombre {
                tipo = tipo_segmento(&p.ty);
            }
        }
        let n = js_string(nombre);
        params.push(format!("{{ nombre: {n}, tipo: \"{tipo}\" }}"));
    }
    // El envoltorio afirma el tipo de cada segmento porque el runtime los
    // entrega como `unknown` —no conoce las firmas— y aquí sí se sabe: acaba de
    // convertirlos con el tipo que dice esta misma tabla.
    let mut args: Vec<String> = Vec::new();
    for p in &f.params {
        let nombre = js_string(&p.name);
        args.push(format!("__p[{nombre}] as {}", map_type(&p.ty)));
    }
    let recibe = if f.params.is_empty() { "()" } else { "(__p)" };
    format!(
        "__ruta({}, [{}], async {recibe} => {}({}));\n",
        js_string(patron),
        params.join(", "),
        f.name,
        args.join(", ")
    )
}

/// La política del handler CUANDO exige identidad. `@server(Public)` —y la
/// ausencia de política— no exigen nada, así que no producen nada emitido: el
/// handler se registra igual que siempre.
fn exige_identidad(f: &FnDecl) -> Option<&Type> {
    match &f.politica {
        Some(Type::Name { name, .. }) if name == "Public" => None,
        otra => otra.as_ref(),
    }
}

/// Nombre con el que la identidad entra en la función emitida. `@server(u: T)`
/// lo dice; `@server(T)` exige identidad sin usarla, y entonces se usa un nombre
/// reservado: el runtime la pasa igual, sólo que el cuerpo no la mira.
fn nombre_identidad(f: &FnDecl) -> String {
    f.identidad_bind
        .clone()
        .unwrap_or_else(|| "__identidad".to_string())
}

pub fn emit(module: &Module) -> Project {
    let (remote, local, shared) = repartir(module);
    // Las globales no reactivas son constantes de módulo: el verificador las
    // declara visibles desde cualquier función, así que deben existir en los dos
    // bundles (antes se descartaban y daban ReferenceError).
    let plain_globals: Vec<&LetStmt> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Let(l) if !l.reactive => Some(l),
            _ => None,
        })
        .collect();

    // El archivo del store por defecto lleva la firma del esquema de `store T;`,
    // para que dos apps con esquemas distintos no compartan archivo.
    // Extensión .log: el backend de archivo es un log append-only JSONL (no un
    // arreglo JSON), una línea por mutación.

    // Sustitución en UNA pasada: cada centinela del template se reemplaza una
    // vez y el contenido inyectado nunca se vuelve a escanear. Así un nombre de
    // tipo/campo que coincida con otro centinela (p.ej. un campo llamado como el
    // placeholder) no puede corromper la sustitución siguiente.
    let runtime = runtime_de(module);
    let builtins = builtins_de(module);
    let tipos = emit_type_decls(module);
    let sesion = session_fn(module);
    let paginas = paginas(module);

    Project {
        runtime,
        server: emit_server(&Servidor {
            paginas: &paginas,
            remote: &remote,
            shared: &shared,
            globals: &plain_globals,
            aliases: &type_aliases(module),
            stores: &store_decls(module),
            sesion: sesion.as_deref(),
            builtins: &builtins,
            tipos: &tipos,
        }),
        client: emit_client(&remote, &local, &plain_globals, &builtins, &tipos),
        demo: emit_demo(&local, es_puro(module)),
    }
}

/// Genera una app web COMPLETA a partir de un módulo: servidor Node (runtime +
/// handlers `@server` + entry) y cliente de navegador (HTML + JS con cliente RPC,
/// núcleo reactivo y render al DOM). Las `reactive` de nivel superior se vuelven
/// signals de módulo: el estado de la app vive ahí y la vista se re-pinta sola.
pub fn emit_app(module: &Module) -> AppProject {
    let (remote, local, shared) = repartir(module);
    let plain_globals: Vec<&LetStmt> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Let(l) if !l.reactive => Some(l),
            _ => None,
        })
        .collect();
    let top_reactives: Vec<&LetStmt> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Let(l) if l.reactive => Some(l),
            _ => None,
        })
        .collect();
    let reactive_names: HashSet<String> = top_reactives.iter().map(|l| l.name.clone()).collect();

    let serve = "// Generado por Marea — arranca el servidor web y lo deja vivo.\n\
        import { startServer, puerto } from \"./runtime.ts\";\n\
        import \"./server.ts\";\n\
        // Sirve los estáticos (index.html, client.js) desde esta carpeta.\n\
        process.env.MAREA_WEB_ROOT ??= import.meta.dirname;\n\
        await startServer();\n\
        // El puerto se lee del runtime, que es quien lo resolvió y validó:\n\
        // recalcularlo aquí haría que el mensaje mintiera si el valor es basura.\n\
        console.log(`[marea] app web en http://127.0.0.1:${puerto()}`);\n"
        .to_string();
    let sesion = session_fn(module);
    let builtins = builtins_de(module);
    let tipos = emit_type_decls(module);
    let paginas = paginas(module);

    AppProject {
        runtime: runtime_de(module),
        server: emit_server(&Servidor {
            paginas: &paginas,
            remote: &remote,
            shared: &shared,
            globals: &plain_globals,
            aliases: &type_aliases(module),
            stores: &store_decls(module),
            sesion: sesion.as_deref(),
            builtins: &builtins,
            tipos: &tipos,
        }),
        serve,
        client_js: emit_client_js(
            &remote,
            &local,
            &top_reactives,
            &reactive_names,
            &plain_globals,
        ),
        index_html: APP_HTML.to_string(),
    }
}

/// El cliente de navegador: runtime de navegador + estado reactivo de módulo +
/// stubs RPC + funciones `@client`/locales (como JS sin tipos) + arranque.
fn emit_client_js(
    remote: &[&FnDecl],
    local: &[&FnDecl],
    top_reactives: &[&LetStmt],
    reactive: &HashSet<String>,
    globals: &[&LetStmt],
) -> String {
    let mut s = String::new();
    s.push_str("// Generado por Marea — cliente de navegador (no editar).\n");
    s.push_str(browser_rt());
    s.push_str("\n// --- programa ---\n");

    // Constantes de módulo (globales no reactivas). Van antes que el estado
    // reactivo porque éste puede leerlas en su inicializador.
    {
        let no_reactive: HashSet<String> = HashSet::new();
        for l in globals {
            s.push_str(&format!(
                "const {} = {};\n",
                l.name,
                emit_expr(&l.value, &no_reactive)
            ));
        }
    }

    // Estado reactivo de nivel superior: signal (mutable) o memo (derivado).
    for l in top_reactives {
        let init = emit_expr(&l.value, reactive);
        if l.mutable {
            s.push_str(&format!("const {} = __signal({init});\n", l.name));
        } else {
            if es_recurso(&l.value) {
                s.push_str(&format!(
                    "const {} = __resource(async () => {init});\n",
                    l.name
                ));
            } else {
                s.push_str(&format!("const {} = __memo(() => {init});\n", l.name));
            }
        }
    }
    s.push('\n');

    // Stubs RPC para las @server (cruzan la red por fetch al mismo origen).
    for f in remote {
        s.push_str(&emit_stub_js(f));
    }
    // Funciones @client y locales, como JS sin anotaciones de tipo.
    for f in local {
        s.push_str(&emit_fn_def_js(f, reactive));
    }

    // Arranque: expone las funciones en window.marea (para onclick), corre main()
    // y monta vista() en el DOM si existen.
    s.push_str("\n// --- arranque ---\n");
    let exposed: Vec<String> = local.iter().map(|f| f.name.clone()).collect();
    s.push_str(&format!(
        "globalThis.marea = {{ {} }};\n",
        exposed.join(", ")
    ));
    if local.iter().any(|f| f.name == "main") {
        s.push_str("await main();\n");
    }
    if local
        .iter()
        .any(|f| f.name == "vista" && f.params.is_empty())
    {
        s.push_str("__mount(vista);\n");
    }
    s
}

/// Una función @client/local como JS de navegador: sin tipos, parámetros por
/// nombre, cuerpo idéntico al del transpilador (que ya no emite tipos).
fn emit_fn_def_js(f: &FnDecl, reactive: &HashSet<String>) -> String {
    let params: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
    let body = emit_block_inner(&f.body, 1, reactive);
    format!(
        "async function {}({}) {{\n{}\n}}\n",
        f.name,
        params.join(", "),
        body
    )
}

/// Stub RPC en JS de navegador para una @server: serializa y manda por fetch.
fn emit_stub_js(f: &FnDecl) -> String {
    let names: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
    format!(
        "async function {name}({params}) {{\n  return await __rpc(\"{name}\", [{args}]);\n}}\n",
        name = f.name,
        params = names.join(", "),
        args = names.join(", ")
    )
}

/// La página de la app web: un #app donde se monta la vista reactiva.
const APP_HTML: &str = "<!doctype html>\n\
    <html lang=\"es\">\n\
    <head>\n\
    <meta charset=\"utf-8\">\n\
    <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
    <title>Marea — app web</title>\n\
    <style>\n\
      body { font-family: system-ui, sans-serif; margin: 2rem; color: #0b1b3a; }\n\
      #app { font-size: 1.1rem; }\n\
      button { cursor: pointer; }\n\
    </style>\n\
    </head>\n\
    <body>\n\
    <h1>Marea en el navegador 🌊</h1>\n\
    <div id=\"app\">cargando…</div>\n\
    <script type=\"module\" src=\"./client.js\"></script>\n\
    </body>\n\
    </html>\n";

/// Firma del esquema del `store T;` del módulo (nombre + campos), para nombrar
/// el archivo de persistencia. `None` si no hay store.
/// Los almacenes del módulo: (nombre, literal JS de su esquema).
fn store_decls(module: &Module) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for it in &module.items {
        if let Item::Store { name, ty, .. } = it {
            let tabla = tabla_externa_de(it);
            let esquema = schema_literal_de(ty, name, tabla, module);
            out.push((name.clone(), esquema));
        }
    }
    out
}

/// La tabla que un almacén toma PRESTADA (`store x: T from "tabla"`), si la
/// toma. `None` es el almacén de siempre: Marea lo posee y lo crea.
fn tabla_externa_de(it: &Item) -> Option<&str> {
    match it {
        Item::Store { tabla_externa, .. } => tabla_externa.as_deref(),
        _ => None,
    }
}

/// ¿Este elemento es un almacén que Marea POSEE?
fn es_almacen_propio(it: &Item) -> bool {
    matches!(it, Item::Store { .. }) && tabla_externa_de(it).is_none()
}

/// ¿El módulo tiene algún almacén propio? Sólo entonces escribe: uno prestado
/// lee una tabla ajena y nada más.
fn hay_almacen_propio(module: &Module) -> bool {
    module.items.iter().any(es_almacen_propio)
}

/// Literal JS del esquema del store (`{ table, columns: [{name,kind}] }`) para
/// el runtime, o `"null"` si no hay store. Los backends SQL/Mongo derivan de aquí
/// la tabla y las columnas.
///
/// Un almacén PRESTADO añade `prestado: true`, y eso cambia lo que el runtime
/// hace con el esquema: deja de ser una orden (crea esta tabla con estas
/// columnas) y pasa a ser una expectativa (esta tabla ya existe y debe tener
/// estas columnas). Va en el literal, y no en una variable de entorno ni en una
/// convención de nombres, porque es una propiedad del programa: se lee en el
/// fuente y viaja hasta el runtime sin que nadie tenga que acordarse.
fn schema_literal_de(
    store_ty: &Type,
    nombre: &str,
    tabla_externa: Option<&str>,
    module: &Module,
) -> String {
    let (_, columns) = resolve_store_fields(store_ty, module);
    // Uno propio: la tabla toma el NOMBRE del almacén, no el del tipo (dos
    // almacenes del mismo tipo son dos tablas distintas, que es lo que se
    // quiere). Uno prestado: el nombre EXACTO que dice el fuente, sin pasarlo a
    // minúsculas — la tabla es de otro y su nombre no es nuestro para
    // normalizarlo.
    let table = match tabla_externa {
        Some(t) => t.to_string(),
        None => nombre.to_lowercase(),
    };
    let cols: Vec<String> = columns
        .iter()
        .map(|(n, k)| format!("{{ name: {}, kind: {} }}", js_string(n), js_string(k)))
        .collect();
    let prestado = if tabla_externa.is_some() {
        ", prestado: true"
    } else {
        ""
    };
    format!(
        "{{ table: {}, columns: [{}]{prestado} }}",
        js_string(&table),
        cols.join(", ")
    )
}

/// Resuelve el store a (nombre de tabla, columnas [(campo, kind)]). Un store de
/// registro produce una columna por campo; uno escalar/lista/unión, una sola
/// columna JSON `__doc`.
fn resolve_store_fields(ty: &Type, module: &Module) -> (String, Vec<(String, String)>) {
    let (name, record_fields): (Option<String>, Option<&[FieldDef]>) = match ty {
        Type::Record { fields, .. } => (None, Some(fields)),
        Type::Name { name, .. } => {
            let aliased = module.items.iter().find_map(|it| match it {
                Item::Type(t) if &t.name == name => Some(&t.aliased),
                _ => None,
            });
            match aliased {
                Some(Type::Record { fields, .. }) => (Some(name.clone()), Some(fields)),
                _ => (Some(name.clone()), None),
            }
        }
        _ => (None, None),
    };
    let table = name.unwrap_or_else(|| "store".to_string()).to_lowercase();
    match record_fields {
        Some(fields) => (
            table,
            fields
                .iter()
                .map(|f| (f.name.clone(), type_kind(&f.ty)))
                .collect(),
        ),
        None => (table, vec![("__doc".to_string(), "json".to_string())]),
    }
}

/// Tipo de columna de un campo, para el esquema del backend.
fn type_kind(ty: &Type) -> String {
    match ty {
        Type::Name { name, .. } => match name.as_str() {
            "Int" => "int",
            "Float" => "real",
            "Bool" => "bool",
            "String" => "text",
            _ => "json",
        },
        _ => "json",
    }
    .to_string()
}

/// Harness web para correr un módulo WebAssembly de Marea en el navegador:
/// `(index.html, glue.mjs)`. El glue carga `module.wasm`, expone sus funciones
/// en `window.marea` (decodificando cadenas desde la memoria) y, si existe una
/// entrada `vista()`/`main()` que devuelve String, la renderiza en `#salida`.
pub fn emit_web(module: &Module) -> (String, String) {
    let fns: Vec<&str> = module
        .items
        .iter()
        .filter_map(|it| match it {
            Item::Fn(f) => Some(f.name.as_str()),
            _ => None,
        })
        .collect();
    let lista = if fns.is_empty() {
        "(ninguna)".to_string()
    } else {
        fns.join(", ")
    };

    let html = format!(
        "<!doctype html>\n\
         <html lang=\"es\">\n\
         <head>\n\
         <meta charset=\"utf-8\">\n\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
         <title>Marea — app WebAssembly</title>\n\
         <style>\n\
           body {{ font-family: system-ui, sans-serif; margin: 2rem; color: #0b1b3a; }}\n\
           #salida {{ font-size: 1.4rem; padding: 1rem 1.2rem; background: #eaf1ff;\n\
                      border-radius: 10px; display: inline-block; }}\n\
           code {{ background: #f3f3f3; padding: .1rem .3rem; border-radius: 4px; }}\n\
         </style>\n\
         </head>\n\
         <body>\n\
         <h1>Marea en el navegador 🌊</h1>\n\
         <div id=\"salida\">cargando…</div>\n\
         <p>Funciones disponibles en <code>window.marea</code>: {lista}.</p>\n\
         <script type=\"module\" src=\"./glue.mjs\"></script>\n\
         </body>\n\
         </html>\n"
    );

    // Entrada web por convención: `vista` o `main` SIN parámetros. El render se
    // genera según su tipo de retorno: si es String se decodifica desde memoria;
    // si es otro valor se muestra tal cual; si no hay entrada válida, un aviso.
    // (Antes, el glue asumía siempre String y crasheaba si devolvía un Int.)
    let entry = module.items.iter().find_map(|it| match it {
        Item::Fn(f) if (f.name == "vista" || f.name == "main") && f.params.is_empty() => Some(f),
        _ => None,
    });
    let render = match entry {
        Some(f) => {
            let call = format!("exports.{}()", f.name);
            match &f.return_type {
                Some(Type::Name { name, .. }) if name == "String" => {
                    format!("  salida.textContent = decodificarCadena({call});")
                }
                Some(_) => format!("  salida.textContent = String({call});"),
                None => format!("  {call};\n  salida.textContent = \"(listo)\";"),
            }
        }
        None => "  salida.textContent = \"(define vista() o main() sin argumentos)\";".to_string(),
    };

    let glue = format!(
        "{WEB_GLUE}\n\
         const salida = typeof document !== \"undefined\" && document.getElementById(\"salida\");\n\
         if (salida) {{\n{render}\n}}\n"
    );
    (html, glue)
}

/// Pegamento JS que conecta el navegador con el módulo WASM (DOM ↔ WASM).
const WEB_GLUE: &str = r#"// Glue generado por Marea: carga el módulo WASM, expone sus funciones en
// window.marea (decodificando cadenas) y renderiza vista()/main() en #salida.

const bytes = await (await fetch("./module.wasm")).arrayBuffer();
const { instance } = await WebAssembly.instantiate(bytes);
const exports = instance.exports;
const memory = exports.memory;

// Una cadena de Marea es un puntero a [longitud:i32][bytes UTF-8].
export function decodificarCadena(ptr) {
  if (!memory) return String(ptr);
  const dv = new DataView(memory.buffer);
  const len = dv.getUint32(ptr, true);
  return new TextDecoder().decode(new Uint8Array(memory.buffer, ptr + 4, len));
}

export const marea = {};
for (const [nombre, fn] of Object.entries(exports)) {
  if (typeof fn === "function") marea[nombre] = fn;
}
globalThis.marea = marea;
globalThis.mareaCadena = decodificarCadena;
"#;

/// El lado servidor: handlers `@server`/`@edge` MÁS las funciones sin anotación
/// y las globales no reactivas que aquéllos puedan usar. Sin esto, extraer un
/// helper compartido (el refactor más común que existe) dejaba `server.ts` con
/// una llamada a algo que no está declarado: `ReferenceError` en cada RPC, sobre
/// código que el verificador había aprobado.
/// Expresión JS que valida que `expr` cumpla el tipo declarado `ty`.
///
/// El verificador garantiza los tipos DENTRO del programa, pero el límite de red
/// recibe JSON arbitrario: sin esto la garantía terminaba en el `fetch` y un
/// `String` declarado podía llegar como objeto o arreglo y persistirse así. El
/// compilador ya conoce las firmas, de modo que emitir el guarda es mecánico.
///
/// Es deliberadamente estricto en escalares, listas y registros, y permisivo en
/// uniones y tipos abiertos (`Record`, alias sin resolver), donde el lenguaje
/// todavía no tiene una representación de runtime con la que discriminar.
fn js_validator(
    ty: &Type,
    aliases: &std::collections::HashMap<String, Type>,
    expr: &str,
) -> String {
    js_validator_guarded(ty, aliases, expr, &mut HashSet::new())
}

/// Igual, con un conjunto de alias en expansión. Un tipo registro recursivo
/// (`type Nodo = { sig: Nodo }`) es válido en el lenguaje, y sin esta guarda el
/// generador recursaba hasta desbordar la pila: `check` pasaba y `build` moría.
/// Al reencontrar un alias ya en expansión se deja de profundizar y se acepta
/// —validar una estructura de profundidad arbitraria en el límite no es el
/// objetivo; el objetivo es que un String no llegue donde se declaró un Int—.
fn js_validator_guarded(
    ty: &Type,
    aliases: &std::collections::HashMap<String, Type>,
    expr: &str,
    expanding: &mut HashSet<String>,
) -> String {
    match ty {
        Type::Name { name, args, .. } => match name.as_str() {
            // Acotado a entero seguro: `Number.isInteger` solo deja pasar 1e21
            // sin más, y el backend WASM del mismo lenguaje trata Int como i32.
            "Int" => format!("Number.isSafeInteger({expr})"),
            "Float" => format!("(typeof {expr} === \"number\" && Number.isFinite({expr}))"),
            "Bool" => format!("typeof {expr} === \"boolean\""),
            // `Html` es una cadena en runtime: se valida como tal. Sin esta
            // entrada caía al brazo genérico y se emitía "true", es decir un
            // parámetro sin ningún guarda.
            "String" | "Html" => format!("typeof {expr} === \"string\""),
            "List" => {
                let elem = match args.first() {
                    Some(a) => js_validator_guarded(a, aliases, "__e", expanding),
                    None => "true".to_string(),
                };
                format!("(Array.isArray({expr}) && {expr}.every((__e) => {elem}))")
            }
            // `Record` es el registro abierto: cualquier objeto vale, pero un
            // arreglo NO es un registro (el `Type::Record` estructural ya lo
            // excluía; sin esto los dos caminos discrepaban y `p.campo` salía
            // undefined aguas abajo).
            "Record" => format!(
                "({expr} !== null && typeof {expr} === \"object\" && !Array.isArray({expr}))"
            ),
            otro => match aliases.get(otro) {
                Some(t) => {
                    // Ya en expansión → referencia recursiva: no profundizar.
                    if !expanding.insert(otro.to_string()) {
                        return "true".to_string();
                    }
                    let v = js_validator_guarded(t, aliases, expr, expanding);
                    expanding.remove(otro);
                    v
                }
                // Variante nominal (NotFound, …): sin representación de runtime
                // fiable, se acepta.
                None => "true".to_string(),
            },
        },
        Type::Record { fields, .. } => {
            let mut partes = vec![format!(
                "({expr} !== null && typeof {expr} === \"object\" && !Array.isArray({expr}))"
            )];
            for f in fields {
                let acceso = format!("{expr}[{}]", js_string(&f.name));
                partes.push(js_validator_guarded(&f.ty, aliases, &acceso, expanding));
            }
            format!("({})", partes.join(" && "))
        }
        // Una unión no tiene discriminante de runtime todavía, así que no se
        // puede saber QUÉ variante llegó; pero sí se puede exigir que sea algo
        // representable (etiqueta, escalar u objeto) y descartar `null` y
        // `undefined`, que era lo que pasaba entero antes.
        Type::Union { .. } => format!("({expr} !== null && {expr} !== undefined)"),
    }
}

/// Lo que necesita el emisor del bundle de servidor. Es un struct y no ocho
/// parámetros sueltos porque ya iba por ocho: cada frontera nueva del lenguaje
/// —el store con nombre, la política, los tipos emitidos— le añadió el suyo, y
/// en una llamada posicional de ocho el siguiente error es mudo.
struct Servidor<'a> {
    remote: &'a [&'a FnDecl],
    shared: &'a [&'a FnDecl],
    globals: &'a [&'a LetStmt],
    aliases: &'a std::collections::HashMap<String, Type>,
    stores: &'a [(String, String)],
    /// Nombre de la `@session` del programa, si la hay.
    sesion: Option<&'a str>,
    /// La lista de importación del runtime, ya ajustada a este módulo.
    builtins: &'a str,
    /// Las declaraciones `export type` del módulo y sus nombres.
    tipos: &'a Tipos,
    /// Las `@page` del módulo, ya ordenadas de más específica a más general.
    /// Están además en `shared`: la tabla las REGISTRA, no las define.
    paginas: &'a [&'a FnDecl],
}

fn emit_server(s_: &Servidor) -> String {
    let Servidor {
        remote,
        shared,
        globals,
        aliases,
        stores,
        sesion,
        builtins,
        tipos,
        paginas,
    } = *s_;
    let mut s = String::new();
    s.push_str("// Generado por Marea — lado servidor.\n");
    s.push_str(&format!("import {} from \"./runtime.ts\";\n\n", builtins));
    s.push_str(&tipos.decls);
    // Un almacén por `store nombre: T;`. Vive solo aquí: el verificador ya
    // impide usar el estado del servidor fuera de @server.
    for (nombre, esquema) in stores {
        s.push_str(&format!(
            "const {nombre} = __store({}, {esquema});\n",
            js_string(nombre)
        ));
    }
    if !stores.is_empty() {
        s.push('\n');
    }
    let no_reactive: HashSet<String> = HashSet::new();
    for l in globals {
        s.push_str(&format!(
            "const {} = {};\n",
            l.name,
            emit_expr(&l.value, &no_reactive)
        ));
    }
    if !globals.is_empty() {
        s.push('\n');
    }
    for f in shared {
        s.push_str(&emit_fn_def(f, false, &tipos.nombres));
        s.push('\n');
    }
    for f in remote {
        s.push_str(&emit_fn_def(f, false, &tipos.nombres));
        s.push('\n');
        // El transporte ya garantiza que 'args' es un arreglo; aquí exigimos la
        // aridad exacta para que un argumento faltante no se cuele como undefined.
        // La aridad y los guardas cuentan sólo lo que manda el CLIENTE: la
        // identidad no viaja en la llamada, así que exigirla aquí haría fallar a
        // todo cliente honesto y la haría, encima, falsificable.
        let mut pass: Vec<String> = (0..f.params.len())
            .map(|i| format!("__args[{i}]"))
            .collect();
        let n = f.params.len();
        // Guarda por argumento derivado del tipo declarado.
        let mut checks = String::new();
        for (i, p) in f.params.iter().enumerate() {
            let v = js_validator(&p.ty, aliases, &format!("__args[{i}]"));
            if v == "true" {
                continue;
            }
            checks.push_str(&format!(
                " if (!({v})) __badRequest(\"argumento {}\");",
                i + 1
            ));
        }
        // Un handler con política se registra con el resolutor de identidad. El
        // runtime lo llama con el token de la petición ANTES que al handler, y si
        // no resuelve responde 401 sin ejecutar el cuerpo; cuando resuelve, pasa
        // la identidad como segundo parámetro del envoltorio, que la reenvía como
        // PRIMER argumento de la función del usuario. Así el `u` de
        // `@server(u: Usuario)` no puede venir del cliente.
        let (recibe, delante, resolutor) = match exige_identidad(f) {
            Some(_) => {
                let arg = match sesion {
                    Some(nombre) => nombre.to_string(),
                    // Sin @session el verificador ya rechaza la política; si aun
                    // así se llegara aquí, se falla CERRADO en vez de abrir.
                    None => "() => ({ $tag: \"NoAutorizado\" })".to_string(),
                };
                ("__args, __identidad", true, format!(", {arg}"))
            }
            None => ("__args", false, String::new()),
        };
        if delante {
            // El transporte la tipa `unknown` —viene de un resolutor que el
            // runtime no conoce—, pero AQUÍ sí se sabe qué es: la política lo
            // dice, y el runtime ya comprobó que resolvió antes de llamar. Sin
            // la afirmación, `tsc --strict` corta con "Argument of type
            // 'unknown' is not assignable".
            let como = exige_identidad(f)
                .map(|t| format!(" as {}", map_type(t)))
                .unwrap_or_default();
            pass.insert(0, format!("__identidad{como}"));
        }
        s.push_str(&format!(
            "__register(\"{}\", ({recibe}) => {{ if (__args.length !== {n}) __badRequest(\"aridad\");{checks} return {}({}); }}{resolutor});\n\n",
            f.name,
            f.name,
            pass.join(", ")
        ));
    }
    // La TABLA DE RUTAS. Va al final porque cita a las funciones de arriba, y
    // se escribe entera aquí, al compilar: en runtime no hay nada que registrar
    // ni orden que dependa de qué se importó primero. Sale ya ordenada de más
    // específica a más general (ver `paginas`).
    if !paginas.is_empty() {
        s.push_str("// La tabla de rutas, armada al COMPILAR.\n");
        for f in paginas {
            s.push_str(&emit_ruta(f));
        }
    }
    s
}

fn emit_client(
    remote: &[&FnDecl],
    local: &[&FnDecl],
    globals: &[&LetStmt],
    builtins: &str,
    tipos: &Tipos,
) -> String {
    let mut s = String::new();
    s.push_str("// Generado por Marea — lado cliente.\n");
    s.push_str(&format!("import {} from \"./runtime.ts\";\n\n", builtins));
    s.push_str(&tipos.decls);
    let no_reactive: HashSet<String> = HashSet::new();
    for l in globals {
        s.push_str(&format!(
            "const {} = {};\n",
            l.name,
            emit_expr(&l.value, &no_reactive)
        ));
    }
    if !globals.is_empty() {
        s.push('\n');
    }
    for f in remote {
        s.push_str(&emit_stub(f));
        s.push('\n');
    }
    for f in local {
        s.push_str(&emit_fn_def(f, true, &tipos.nombres));
        s.push('\n');
    }
    s
}

fn emit_demo(local: &[&FnDecl], puro: bool) -> String {
    let has_main = local.iter().any(|f| f.name == "main");
    let mut s = String::new();
    s.push_str("// Generado por Marea — orquestador de la demo.\n");
    // Un módulo que no cruza la frontera no tiene servidor que arrancar, y su
    // runtime ya no exporta `startServer`: importarlo sería un error de carga.
    if !puro {
        s.push_str("import { startServer, stopServer } from \"./runtime.ts\";\n");
        s.push_str("import \"./server.ts\";\n");
    }
    if has_main {
        s.push_str("import { main } from \"./client.ts\";\n");
    }
    s.push('\n');
    if puro {
        if has_main {
            s.push_str("await main();\n");
        } else {
            s.push_str("console.log(\"[marea] módulo sin main(): nada que ejecutar\");\n");
        }
        return s;
    }
    s.push_str("await startServer();\ntry {\n");
    if has_main {
        s.push_str("  await main();\n");
    } else {
        s.push_str("  console.log(\"[marea] no hay función main() en @client\");\n");
    }
    s.push_str("} finally {\n  await stopServer();\n}\n");
    s
}

// --- funciones ---

/// El tipo de retorno para la firma TypeScript, **sólo si es seguro emitirlo**.
///
/// TypeScript infiere el retorno de una función normal, pero de una RECURSIVA no
/// puede: sin anotación da TS7023 ("implicitly has return type 'any'"), y eso es
/// un error en modo estricto.
///
/// Se anota cuando el tipo tiene una traducción que EXISTE en el archivo
/// generado: los primitivos, y los `type` declarados por el módulo, que ahora se
/// emiten como `export type`. Una unión de variantes se queda fuera: produciría
/// nombres que nadie declara, y anotar mal es peor que no anotar —cambiaría un
/// TS7023 por un "Cannot find name"—.
fn ts_return_type(f: &FnDecl, declarados: &HashSet<String>) -> Option<String> {
    fn traducir(t: &Type, declarados: &HashSet<String>) -> Option<String> {
        match t {
            Type::Name { name, args, .. } if name == "List" => {
                let e = traducir(args.first()?, declarados)?;
                Some(format!("{e}[]"))
            }
            Type::Name { name, args, .. } if args.is_empty() => match name.as_str() {
                "Int" | "Float" => Some("number".to_string()),
                // `Html` es una cadena en runtime: la distinción es estática y
                // no tiene contrapartida en TS.
                "String" | "Html" => Some("string".to_string()),
                "Bool" => Some("boolean".to_string()),
                "Unit" => Some("void".to_string()),
                // Un `type` del módulo: existe en la salida porque se emite su
                // `export type`. Una variante nominal, no.
                otro if declarados.contains(otro) => Some(otro.to_string()),
                _ => None,
            },
            // Un registro escrito en línea se traduce a su type-literal, que no
            // depende de que nadie lo declare.
            Type::Record { .. } => Some(map_type(t)),
            _ => None,
        }
    }
    match &f.return_type {
        None => Some("void".to_string()),
        Some(t) => traducir(t, declarados),
    }
}

fn emit_fn_def(f: &FnDecl, export: bool, declarados: &HashSet<String>) -> String {
    let params = ts_params_handler(f);
    let kw = if export {
        "export async function"
    } else {
        "async function"
    };
    // El conjunto de variables reactivas se construye incrementalmente por
    // bloque (respetando el alcance léxico), arrancando vacío.
    let body = emit_block_inner(&f.body, 1, &HashSet::new());
    // La función se emite `async`, así que lo anotado es la promesa.
    let ret = match ts_return_type(f, declarados) {
        Some(t) => format!(": Promise<{t}>"),
        None => String::new(),
    };
    format!(
        "{}\n{} {}({}){} {{\n{}\n}}\n",
        signature_comment(f),
        kw,
        f.name,
        params,
        ret,
        body
    )
}

fn emit_stub(f: &FnDecl) -> String {
    let params = ts_params(f);
    let arg_names: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
    let ret = f
        .return_type
        .as_ref()
        .map(map_type)
        // Sin retorno declarado: `void` es lo que la función devuelve de verdad.
        .unwrap_or_else(|| "void".to_string());
    format!(
        "// stub generado para la función @server '{name}' (cruza la frontera por RPC)\n\
         async function {name}({params}): Promise<{ret}> {{\n  \
         return (await __rpc(\"{name}\", [{args}])) as {ret};\n}}\n",
        name = f.name,
        params = params,
        ret = ret,
        args = arg_names.join(", ")
    )
}

/// Los parámetros que el CLIENTE manda: la firma que ve quien llama. La
/// identidad no está aquí a propósito —no viaja en la llamada—, así que el stub
/// RPC no cambia porque un handler pase a exigirla.
fn ts_params(f: &FnDecl) -> String {
    f.params
        .iter()
        .map(|p| format!("{}: {}", p.name, map_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Los parámetros de la función tal como se DEFINE en el servidor: la identidad
/// primero, si la política la exige, y luego los del cliente. Es el mismo orden
/// que usa el envoltorio de `__register`.
fn ts_params_handler(f: &FnDecl) -> String {
    match exige_identidad(f) {
        Some(politica) => {
            let mut ps = vec![format!("{}: {}", nombre_identidad(f), map_type(politica))];
            ps.extend(
                f.params
                    .iter()
                    .map(|p| format!("{}: {}", p.name, map_type(&p.ty))),
            );
            ps.join(", ")
        }
        None => ts_params(f),
    }
}

// --- sentencias ---

fn emit_block_inner(block: &Block, indent: usize, reactive: &HashSet<String>) -> String {
    // `current` arranca con las reactivas del alcance externo y se actualiza al
    // procesar cada sentencia: un `reactive` añade el nombre; un `let`/binding
    // no-reactivo del mismo nombre lo SOMBREA (lo quita) para el resto del
    // bloque. Así una lectura del nombre sombreado no emite `.get()` erróneo.
    let mut current = reactive.clone();
    let mut lines = Vec::with_capacity(block.stmts.len());
    for stmt in &block.stmts {
        // La sentencia se emite con el alcance ANTERIOR a su propio binding
        // (el RHS de `let n = ...` ve la `n` externa, no la que declara).
        lines.push(emit_stmt(stmt, indent, &current));
        if let Stmt::Let(l) = stmt {
            if l.reactive {
                current.insert(l.name.clone());
            } else {
                current.remove(&l.name);
            }
        }
    }
    lines.join("\n")
}

/// Neutraliza el texto literal de una plantilla para incrustarlo en una
/// plantilla de JavaScript: el backtick la cerraría antes de tiempo y `${`
/// abriría una interpolación que el programa no escribió.
fn escapar_plantilla(t: &str) -> String {
    t.replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${")
}

/// `on` es SÍNCRONO: registra un cierre en la tabla del runtime y devuelve el
/// atributo, sin tocar la red ni el disco. Emitirlo con `await` sería, además de
/// una espera de nada, romper el rastreo de dependencias reactivas justo donde
/// más duele: `on` se llama DENTRO de la plantilla que pinta la vista, así que
/// un await ahí suspende el effect antes de que termine de leer sus signals.
///
/// La lista canónica de síncronos es `marea_syntax::builtins::SINCRONOS`, que
/// pertenece a la otra mitad del reparto; hasta que `on` entre ahí la excepción
/// vive aquí —y otra igual en el verificador—. Cuando se pueda tocar ese crate,
/// esto se borra y la lista vuelve a ser una sola.
fn es_sincrono_aqui(name: &str) -> bool {
    es_sincrono(name) || name == "on"
}

/// ¿El inicializador de una `reactive` es un recurso? Lo es cuando llama a una
/// función del usuario: el codegen emite `await` para ésas, y un `await` no cabe
/// en el cuerpo síncrono de un memo. Los builtins síncronos no cuentan.
fn es_recurso(e: &Expr) -> bool {
    match e {
        Expr::Call { callee, .. } => matches!(
            callee.as_ref(),
            Expr::Ident { name, .. } if !es_sincrono_aqui(name)
        ),
        _ => false,
    }
}

fn emit_stmt(stmt: &Stmt, indent: usize, reactive: &HashSet<String>) -> String {
    let p = pad(indent);
    match stmt {
        Stmt::Let(l) if l.reactive => {
            // Fuente reactiva (mutable) = signal; derivada (inmutable) = memo.
            // Si el inicializador es una LLAMADA, es un recurso: la llamada es
            // asíncrona, así que el valor arranca en `Cargando` y se resuelve
            // solo. Un memo no serviría —su cuerpo es síncrono— y era justo lo
            // que antes generaba `__memo(() => (await f()))`, un await dentro de
            // una arrow no-async, o sea un SyntaxError.
            let init = emit_expr(&l.value, reactive);
            if l.mutable {
                format!("{p}const {} = __signal({init});", l.name)
            } else if es_recurso(&l.value) {
                format!("{p}const {} = __resource(async () => {init});", l.name)
            } else {
                format!("{p}const {} = __memo(() => {init});", l.name)
            }
        }
        Stmt::Let(l) => {
            let kw = if l.mutable { "let" } else { "const" };
            format!("{p}{kw} {} = {};", l.name, emit_expr(&l.value, reactive))
        }
        // Asignar a una variable reactiva = invocar su setter; si no, asignación normal.
        Stmt::Assign { name, value, .. } => {
            let v = emit_expr(value, reactive);
            if reactive.contains(name) {
                format!("{p}{name}.set({v});")
            } else {
                format!("{p}{name} = {v};")
            }
        }
        // Efecto: se re-ejecuta cuando cambian las reactivas que lee.
        Stmt::For {
            var,
            index,
            iter,
            body,
            ..
        } => {
            // Bucle clásico sobre índice; el elemento y el índice son `const`
            // porque dentro del bucle son inmutables.
            let it = emit_expr(iter, reactive);
            let i = index.clone().unwrap_or_else(|| "__i".to_string());
            let cuerpo = emit_block_inner(body, indent + 2, reactive);
            format!(
                "{p}{{\n{p}  const __xs = {it};\n{p}  for (let {i} = 0; {i} < __xs.length; {i}++) {{\n{p}    const {var} = __xs[{i}];\n{cuerpo}\n{p}  }}\n{p}}}"
            )
        }
        Stmt::Effect { body, .. } => {
            let inner = emit_block_inner(body, indent + 1, reactive);
            format!("{p}__effect(async () => {{\n{inner}\n{p}}});")
        }
        Stmt::Return { value, .. } => match value {
            Some(v) => format!("{p}return {};", emit_expr(v, reactive)),
            None => format!("{p}return;"),
        },
        Stmt::Expr(e) => match e {
            Expr::If { .. } | Expr::Match { .. } => emit_control(e, indent, reactive),
            _ => format!("{p}{};", emit_expr(e, reactive)),
        },
    }
}

fn emit_control(e: &Expr, indent: usize, reactive: &HashSet<String>) -> String {
    let p = pad(indent);
    match e {
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let mut s = format!("{p}if ({}) {{\n", emit_expr(cond, reactive));
            s.push_str(&emit_block_inner(then_branch, indent + 1, reactive));
            s.push_str(&format!("\n{p}}}"));
            if let Some(eb) = else_branch {
                match eb.as_ref() {
                    ElseBranch::Block(b) => {
                        s.push_str(" else {\n");
                        s.push_str(&emit_block_inner(b, indent + 1, reactive));
                        s.push_str(&format!("\n{p}}}"));
                    }
                    ElseBranch::If(inner) => {
                        s.push_str(" else ");
                        s.push_str(emit_control(inner, indent, reactive).trim_start());
                    }
                }
            }
            s
        }
        Expr::Match {
            scrutinee, arms, ..
        } => emit_match(scrutinee, arms, indent, reactive, false),
        _ => format!("{p}{};", emit_expr(e, reactive)),
    }
}

/// `returning`: si es `true`, el cuerpo de cada rama se emite como `return <v>;`
/// (match en posición de EXPRESIÓN, dentro de un IIFE que produce el valor); si
/// es `false`, como sentencia (`<v>;`).
fn emit_match(
    scrut: &Expr,
    arms: &[MatchArm],
    indent: usize,
    reactive: &HashSet<String>,
    returning: bool,
) -> String {
    let p = pad(indent);
    let pin = pad(indent + 1);
    let mut s = format!("{p}{{\n{pin}const __m = {};\n", emit_expr(scrut, reactive));
    let mut chained = false; // ¿ya emitimos algún if/else-if?
    let mut caught_all = false; // ¿ya se emitió una rama atrapa-todo?
    for arm in arms {
        let body = match &arm.body {
            Expr::If { .. } | Expr::Match { .. } => emit_control(&arm.body, indent + 2, reactive),
            _ if returning => format!(
                "{}return {};",
                pad(indent + 2),
                emit_expr(&arm.body, reactive)
            ),
            _ => format!("{}{};", pad(indent + 2), emit_expr(&arm.body, reactive)),
        };
        // Tras una rama atrapa-todo el resto es inalcanzable: emitirlas daría
        // `else if` después de un `else` (JS inválido).
        if caught_all {
            continue;
        }
        match &arm.pattern {
            Pattern::Wildcard { .. } => {
                // Sin `if` previo no puede haber `else`: se emite un bloque plano.
                if chained {
                    s.push_str(&format!("{pin}else {{\n{body}\n{pin}}}\n"));
                } else {
                    s.push_str(&format!("{pin}{{\n{body}\n{pin}}}\n"));
                }
                caught_all = true;
            }
            Pattern::Binding { name, .. } => {
                if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                    // Variante (Mayúscula): comparación best-effort.
                    let kw = if chained { "else if" } else { "if" };
                    s.push_str(&format!(
                        "{pin}{kw} (__marea_is(__m, \"{name}\")) {{\n{body}\n{pin}}}\n"
                    ));
                    chained = true;
                } else {
                    // Enlace (minúscula): captura todo y nombra el valor.
                    if chained {
                        s.push_str(&format!(
                            "{pin}else {{ const {name} = __m;\n{body}\n{pin}}}\n"
                        ));
                    } else {
                        s.push_str(&format!("{pin}{{ const {name} = __m;\n{body}\n{pin}}}\n"));
                    }
                    caught_all = true;
                }
            }
            Pattern::Int { value, .. } => {
                let kw = if chained { "else if" } else { "if" };
                s.push_str(&format!(
                    "{pin}{kw} (__m === {value}) {{\n{body}\n{pin}}}\n"
                ));
                chained = true;
            }
            Pattern::Bool { value, .. } => {
                let kw = if chained { "else if" } else { "if" };
                s.push_str(&format!(
                    "{pin}{kw} (__m === {value}) {{\n{body}\n{pin}}}\n"
                ));
                chained = true;
            }
            Pattern::Str { value, .. } => {
                let kw = if chained { "else if" } else { "if" };
                s.push_str(&format!(
                    "{pin}{kw} (__m === {}) {{\n{body}\n{pin}}}\n",
                    js_string(value)
                ));
                chained = true;
            }
        }
    }
    s.push_str(&format!("{p}}}"));
    s
}

// --- expresiones ---

fn emit_expr(e: &Expr, reactive: &HashSet<String>) -> String {
    match e {
        // Un cierre se emite como función flecha, y `async` como todas las del
        // usuario: su cuerpo puede llamar a cualquier cosa, y el emisor de
        // llamadas ya pone el `await` a todo lo que no sea un builtin síncrono.
        // La captura sale gratis porque JavaScript ya cierra sobre el entorno
        // léxico; el verificador es quien impone que sea POR VALOR, prohibiendo
        // capturar una variable mutable.
        Expr::Fn { params, body, .. } => {
            let ps: Vec<String> = params
                .iter()
                .map(|p| format!("{}: {}", p.name, map_type(&p.ty)))
                .collect();
            format!(
                "(async ({}) => {{\n{}\n}})",
                ps.join(", "),
                emit_block_inner(body, 1, reactive)
            )
        }
        Expr::Int { value, .. } => value.to_string(),
        Expr::Float { value, .. } => value.to_string(),
        Expr::Str { value, .. } => js_string(value),
        Expr::Bool { value, .. } => value.to_string(),
        Expr::Ident { name, .. } => {
            if reactive.contains(name) {
                // Leer una reactiva = su getter (rastrea dependencias).
                format!("{name}.get()")
            } else if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                // Variante nominal usada como valor (errores como valores). Se
                // representa con una etiqueta en el campo reservado `$tag`: el
                // lexer no admite `$` en un identificador, así que ningún
                // registro del usuario puede tener ese campo y confundirse con
                // una variante. Antes era una cadena desnuda, y entonces el
                // String "NotFound" era indistinguible de la variante.
                format!("{{ $tag: {} }}", js_string(name))
            } else {
                name.clone()
            }
        }
        Expr::Unary { op, expr, .. } => {
            let inner = emit_expr(expr, reactive);
            match op {
                UnaryOp::Neg => format!("-({inner})"),
                UnaryOp::Not => format!("!({inner})"),
            }
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = emit_expr(left, reactive);
            let r = emit_expr(right, reactive);
            match op {
                // División entera: trunca hacia cero, igual que i32.div_s de WASM.
                // JS '/' daría flotante (7/2=3.5) y rompería el contrato Int.
                // División y módulo van por helpers que cortan el divisor cero:
                // en JS darían Infinity/NaN dentro de un Int, y el backend WASM
                // trapea. El mismo programa no puede tener dos finales.
                BinOp::Div => format!("__div({l}, {r})"),
                BinOp::Rem => format!("__rem({l}, {r})"),
                _ => format!("({} {} {})", l, map_binop(*op), r),
            }
        }
        // Las llamadas a funciones del usuario y stubs RPC son async (se
        // 'await'); los builtins síncronos (print/concat/render) NO se awaitan,
        // porque un 'await' espurio rompe el rastreo de dependencias reactivas
        // (el cuerpo del effect se suspendería y __currentSub se restauraría
        // antes de leer los signals).
        Expr::Call { callee, args, .. } => {
            let a: Vec<String> = args.iter().map(|x| emit_expr(x, reactive)).collect();
            let callee_ts = emit_expr(callee, reactive);
            // Builtins SÍNCRONOS (no se 'await'). Los de estado (save/all/
            // update/remove) son async (pegan a la BD) y SÍ se awaitan.
            let is_sync_builtin = matches!(
                callee.as_ref(),
                Expr::Ident { name, .. } if es_sincrono_aqui(name)
            );
            if is_sync_builtin {
                format!("{}({})", callee_ts, a.join(", "))
            } else {
                format!("(await {}({}))", callee_ts, a.join(", "))
            }
        }
        Expr::Member { object, field, .. } => format!("{}.{}", emit_expr(object, reactive), field),
        // 'match' en posición de expresión: IIFE que RETORNA el valor de la rama.
        Expr::Match {
            scrutinee, arms, ..
        } => {
            format!(
                "(await (async () => {{\n{}\n}})())",
                emit_match(scrutinee, arms, 1, reactive, true)
            )
        }
        // 'if' en posición de expresión: IIFE (las ramas-bloque no tienen valor
        // de cola en la gramática; queda como sentencia, valor diferido).
        Expr::If { .. } => {
            format!(
                "(await (async () => {{\n{}\n}})())",
                emit_control(e, 1, reactive)
            )
        }
        // Literal de registro -> objeto JS.
        Expr::Record { fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, emit_expr(&f.value, reactive)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        // Literal de lista -> arreglo JS.
        Expr::List { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|x| emit_expr(x, reactive)).collect();
            format!("[{}]", parts.join(", "))
        }
        // Indexado -> acceso con verificación de rango (lanza si está fuera).
        Expr::Template { parts, .. } => {
            // Se emite como plantilla de JS. Cada hueco `{x}` pasa por
            // `escape(...)`; `{!x}` va tal cual, y el verificador ya garantizó
            // que es Html. El texto literal se neutraliza para que un backtick
            // o un `${` del propio contenido no rompan la plantilla generada.
            let mut out = String::from("`");
            for parte in parts {
                match parte {
                    TemplatePart::Lit(t) => out.push_str(&escapar_plantilla(t)),
                    TemplatePart::Interp { expr, raw } => {
                        let e = emit_expr(expr, reactive);
                        if *raw {
                            out.push_str(&format!("${{{e}}}"));
                        } else {
                            out.push_str(&format!("${{escape({e})}}"));
                        }
                    }
                }
            }
            out.push('`');
            out
        }
        Expr::Index { object, index, .. } => {
            format!(
                "__index({}, {})",
                emit_expr(object, reactive),
                emit_expr(index, reactive)
            )
        }
    }
}

// --- utilidades ---

fn pad(n: usize) -> String {
    "  ".repeat(n)
}

fn map_binop(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "===",
        BinOp::Ne => "!==",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn map_type(t: &Type) -> String {
    match t {
        // `List<T>` -> `T[]` (arreglo TS). Sin args, el elemento es el `Unknown`
        // de Marea, que se traduce `any` y NO `unknown`: son opuestos. El
        // `Unknown` de Marea es el comodín que absorbe operaciones —subtipo de
        // todo, para no encadenar errores—, mientras que el `unknown` de TS es
        // el tipo cima, al que no se le puede asignar nada. Con `unknown[]`, un
        // `fn cabeza(xs: List) -> Int` emitía un retorno que no encajaba con su
        // propia firma.
        Type::Name { name, args, .. } if name == "List" => match args.first() {
            Some(elem) => format!("{}[]", map_type(elem)),
            None => "any[]".to_string(),
        },
        // Otros genéricos: traducir los argumentos en vez de descartarlos.
        Type::Name { name, args, .. } if !args.is_empty() => {
            let inner: Vec<String> = args.iter().map(map_type).collect();
            format!("{}<{}>", name, inner.join(", "))
        }
        Type::Name { name, .. } => match name.as_str() {
            "Int" | "Float" => "number".to_string(),
            "String" => "string".to_string(),
            "Bool" => "boolean".to_string(),
            // Tipos sin definir: conservamos el nombre (Node lo ignora al correr).
            _ => name.clone(),
        },
        Type::Union { variants, .. } => variants
            .iter()
            .map(map_type)
            .collect::<Vec<_>>()
            .join(" | "),
        // Tipo registro -> type-literal TS preciso.
        Type::Record { fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, map_type(&f.ty)))
                .collect();
            format!("{{ {} }}", parts.join("; "))
        }
    }
}

fn signature_comment(f: &FnDecl) -> String {
    // La política forma parte de la firma tanto como la ubicación: quien lea el
    // archivo generado tiene que ver a quién deja entrar cada handler.
    let mut loc = match f.location {
        Some(Location::Server) => "@server".to_string(),
        Some(Location::Client) => "@client".to_string(),
        Some(Location::Edge) => "@edge".to_string(),
        None if f.es_session => "@session".to_string(),
        None => String::new(),
    };
    if let Some(politica) = &f.politica {
        let nombrada = match &f.identidad_bind {
            Some(n) => format!("{n}: "),
            None => String::new(),
        };
        loc.push_str(&format!("({nombrada}{})", type_to_src(politica)));
    }
    if !loc.is_empty() {
        loc.push(' ');
    }
    let params = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, type_to_src(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = f
        .return_type
        .as_ref()
        .map(|t| format!(" -> {}", type_to_src(t)))
        .unwrap_or_default();
    format!("// {loc}fn {}({}){}", f.name, params, ret)
}

fn type_to_src(t: &Type) -> String {
    match t {
        Type::Name { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                format!(
                    "{}<{}>",
                    name,
                    args.iter().map(type_to_src).collect::<Vec<_>>().join(", ")
                )
            }
        }
        Type::Union { variants, .. } => variants
            .iter()
            .map(type_to_src)
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Record { fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, type_to_src(&f.ty)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

/// Literal de cadena JS con escapes. Conserva UTF-8 (emojis, acentos) tal cual.
fn js_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El emisor y el verificador tienen que estar de acuerdo en qué builtin es
    /// síncrono: si discrepan, o se emite `await` donde no toca o se rechaza
    /// código válido. Ahora leen la misma lista, y esto lo fija.
    #[test]
    fn el_await_lo_decide_la_lista_compartida() {
        let no_reactive = HashSet::new();
        let m = marea_syntax::parse("fn f() -> Int { return len(concat(\"a\", \"b\")); }").unwrap();
        let Item::Fn(f) = &m.items[0] else { panic!() };
        let Stmt::Return { value: Some(e), .. } = &f.body.stmts[0] else {
            panic!()
        };
        assert_eq!(emit_expr(e, &no_reactive), "len(concat(\"a\", \"b\"))");

        let m =
            marea_syntax::parse("@server fn g() { print(1); }\n@client fn h() { g(); }").unwrap();
        let Item::Fn(h) = &m.items[1] else { panic!() };
        let Stmt::Expr(e) = &h.body.stmts[0] else {
            panic!()
        };
        assert!(emit_expr(e, &no_reactive).starts_with("(await "));
    }
}
