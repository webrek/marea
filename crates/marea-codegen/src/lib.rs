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
use std::collections::HashSet;

/// El runtime TypeScript embebido (transporte RPC + builtins).
pub const RUNTIME_TS: &str = include_str!("runtime.ts");

/// El runtime de NAVEGADOR embebido (cliente RPC + reactivo + DOM, sin Node).
pub const BROWSER_RT: &str = include_str!("browser.js");

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

/// Builtins provistos por el runtime; no se transpilan ni se registran.
const BUILTINS: &str = "{ __register, __malFormado, __rpc, print, concat, render, len, aTexto, escapar, __index, \
     guardar, todos, actualizar, borrar, __marea_is, __signal, __memo, __effect }";

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

/// Una función con `@server` o `@edge` corre "remota": handler + stub RPC.
fn is_remote(f: &FnDecl) -> bool {
    matches!(f.location, Some(Location::Server) | Some(Location::Edge))
}

pub fn emit(module: &Module) -> Project {
    let mut remote = Vec::new();
    let mut local = Vec::new();
    // Sin anotación = local desde cualquier lado: va a AMBOS bundles.
    let mut shared = Vec::new();
    for item in &module.items {
        if let Item::Fn(f) = item {
            if is_remote(f) {
                remote.push(f);
            } else {
                if f.location.is_none() {
                    shared.push(f);
                }
                local.push(f);
            }
        }
    }
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
    let store_file = match store_signature(module) {
        Some(sig) => format!("marea-store.{sig}.log"),
        None => "marea-store.log".to_string(),
    };
    // Sustitución en UNA pasada: cada centinela del template se reemplaza una
    // vez y el contenido inyectado nunca se vuelve a escanear. Así un nombre de
    // tipo/campo que coincida con otro centinela (p.ej. un campo llamado como el
    // placeholder) no puede corromper la sustitución siguiente.
    let runtime = fill_template(
        RUNTIME_TS,
        &[
            ("__MAREA_STORE_DEFAULT__", &store_file),
            ("__MAREA_STORE_SCHEMA__", &store_schema_literal(module)),
        ],
    );

    Project {
        runtime,
        server: emit_server(&remote, &shared, &plain_globals, &type_aliases(module)),
        client: emit_client(&remote, &local, &plain_globals),
        demo: emit_demo(&local),
    }
}

/// Sustituye varios centinelas en `tpl` en una sola pasada de izquierda a
/// derecha. A diferencia de encadenar `str::replace`, el texto inyectado se
/// copia a la salida y no se vuelve a inspeccionar, así que un reemplazo no
/// puede introducir (ni chocar con) el centinela de otro.
fn fill_template(tpl: &str, subs: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(tpl.len());
    let mut rest = tpl;
    while !rest.is_empty() {
        // El centinela que aparezca más a la izquierda en lo que queda.
        let next = subs
            .iter()
            .filter_map(|(pat, val)| rest.find(pat).map(|idx| (idx, pat.len(), *val)))
            .min_by_key(|(idx, _, _)| *idx);
        match next {
            Some((idx, pat_len, val)) => {
                out.push_str(&rest[..idx]);
                out.push_str(val);
                rest = &rest[idx + pat_len..];
            }
            None => {
                out.push_str(rest);
                break;
            }
        }
    }
    out
}

/// El runtime Node con los centinelas del store sustituidos (compartido por
/// `emit` y `emit_app`).
fn build_node_runtime(module: &Module) -> String {
    let store_file = match store_signature(module) {
        Some(sig) => format!("marea-store.{sig}.log"),
        None => "marea-store.log".to_string(),
    };
    fill_template(
        RUNTIME_TS,
        &[
            ("__MAREA_STORE_DEFAULT__", &store_file),
            ("__MAREA_STORE_SCHEMA__", &store_schema_literal(module)),
        ],
    )
}

/// Genera una app web COMPLETA a partir de un módulo: servidor Node (runtime +
/// handlers `@server` + entry) y cliente de navegador (HTML + JS con cliente RPC,
/// núcleo reactivo y render al DOM). Las `reactive` de nivel superior se vuelven
/// signals de módulo: el estado de la app vive ahí y la vista se re-pinta sola.
pub fn emit_app(module: &Module) -> AppProject {
    let mut remote = Vec::new();
    let mut local = Vec::new();
    let mut shared = Vec::new();
    for item in &module.items {
        if let Item::Fn(f) = item {
            if is_remote(f) {
                remote.push(f);
            } else {
                if f.location.is_none() {
                    shared.push(f);
                }
                local.push(f);
            }
        }
    }
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
    let reactive_names: HashSet<String> =
        top_reactives.iter().map(|l| l.name.clone()).collect();

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

    AppProject {
        runtime: build_node_runtime(module),
        server: emit_server(&remote, &shared, &plain_globals, &type_aliases(module)),
        serve,
        client_js: emit_client_js(&remote, &local, &top_reactives, &reactive_names, &plain_globals),
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
    s.push_str(BROWSER_RT);
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
            s.push_str(&format!("const {} = __memo(() => {init});\n", l.name));
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
    s.push_str(&format!("globalThis.marea = {{ {} }};\n", exposed.join(", ")));
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
fn store_signature(module: &Module) -> Option<String> {
    let store_ty = module.items.iter().find_map(|it| match it {
        Item::Store { ty, .. } => Some(ty),
        _ => None,
    })?;
    Some(type_signature(store_ty, module))
}

fn type_signature(ty: &Type, module: &Module) -> String {
    match ty {
        Type::Record { fields, .. } => {
            let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
            format!("rec-{}", names.join("-"))
        }
        Type::Name { name, .. } => {
            // Resuelve un alias a registro para incluir sus campos en la firma.
            let aliased = module.items.iter().find_map(|it| match it {
                Item::Type(t) if &t.name == name => Some(&t.aliased),
                _ => None,
            });
            if let Some(Type::Record { fields, .. }) = aliased {
                let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
                format!("{name}-{}", names.join("-"))
            } else {
                name.clone()
            }
        }
        Type::Union { .. } => "union".to_string(),
    }
}

/// Literal JS del esquema del store (`{ table, columns: [{name,kind}] }`) para
/// el runtime, o `"null"` si no hay store. Los backends SQL/Mongo derivan de aquí
/// la tabla y las columnas.
fn store_schema_literal(module: &Module) -> String {
    let store_ty = match module.items.iter().find_map(|it| match it {
        Item::Store { ty, .. } => Some(ty),
        _ => None,
    }) {
        Some(t) => t,
        None => return "null".to_string(),
    };
    let (table, columns) = resolve_store_fields(store_ty, module);
    let cols: Vec<String> = columns
        .iter()
        .map(|(n, k)| format!("{{ name: {}, kind: {} }}", js_string(n), js_string(k)))
        .collect();
    format!(
        "{{ table: {}, columns: [{}] }}",
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
fn js_validator(ty: &Type, aliases: &std::collections::HashMap<String, Type>, expr: &str) -> String {
    match ty {
        Type::Name { name, args, .. } => match name.as_str() {
            "Int" => format!("Number.isInteger({expr})"),
            "Float" => format!("(typeof {expr} === \"number\" && Number.isFinite({expr}))"),
            "Bool" => format!("typeof {expr} === \"boolean\""),
            "String" => format!("typeof {expr} === \"string\""),
            "List" => {
                let elem = match args.first() {
                    Some(a) => js_validator(a, aliases, "__e"),
                    None => "true".to_string(),
                };
                format!("(Array.isArray({expr}) && {expr}.every((__e) => {elem}))")
            }
            // `Record` es el registro abierto: cualquier objeto vale.
            "Record" => format!("({expr} !== null && typeof {expr} === \"object\")"),
            otro => match aliases.get(otro) {
                Some(t) => js_validator(t, aliases, expr),
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
                partes.push(js_validator(&f.ty, aliases, &acceso));
            }
            format!("({})", partes.join(" && "))
        }
        // Una unión no tiene discriminante de runtime todavía: se acepta.
        Type::Union { .. } => "true".to_string(),
    }
}

fn emit_server(
    remote: &[&FnDecl],
    shared: &[&FnDecl],
    globals: &[&LetStmt],
    aliases: &std::collections::HashMap<String, Type>,
) -> String {
    let mut s = String::new();
    s.push_str("// Generado por Marea — lado servidor.\n");
    s.push_str(&format!("import {} from \"./runtime.ts\";\n\n", BUILTINS));
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
        s.push_str(&emit_fn_def(f, false));
        s.push('\n');
    }
    for f in remote {
        s.push_str(&emit_fn_def(f, false));
        s.push('\n');
        // El transporte ya garantiza que 'args' es un arreglo; aquí exigimos la
        // aridad exacta para que un argumento faltante no se cuele como undefined.
        let pass: Vec<String> = (0..f.params.len()).map(|i| format!("args[{i}]")).collect();
        let n = f.params.len();
        // Guarda por argumento derivado del tipo declarado.
        let mut checks = String::new();
        for (i, p) in f.params.iter().enumerate() {
            let v = js_validator(&p.ty, aliases, &format!("args[{i}]"));
            if v == "true" {
                continue;
            }
            checks.push_str(&format!(
                " if (!({v})) __malFormado(\"argumento {}\");",
                i + 1
            ));
        }
        s.push_str(&format!(
            "__register(\"{}\", (args) => {{ if (args.length !== {n}) __malFormado(\"aridad\");{checks} return {}({}); }});\n\n",
            f.name,
            f.name,
            pass.join(", ")
        ));
    }
    s
}

fn emit_client(remote: &[&FnDecl], local: &[&FnDecl], globals: &[&LetStmt]) -> String {
    let mut s = String::new();
    s.push_str("// Generado por Marea — lado cliente.\n");
    s.push_str(&format!("import {} from \"./runtime.ts\";\n\n", BUILTINS));
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
        s.push_str(&emit_fn_def(f, true));
        s.push('\n');
    }
    s
}

fn emit_demo(local: &[&FnDecl]) -> String {
    let has_main = local.iter().any(|f| f.name == "main");
    let mut s = String::new();
    s.push_str("// Generado por Marea — orquestador de la demo.\n");
    s.push_str("import { startServer, stopServer } from \"./runtime.ts\";\n");
    s.push_str("import \"./server.ts\";\n");
    if has_main {
        s.push_str("import { main } from \"./client.ts\";\n");
    }
    s.push_str("\nawait startServer();\ntry {\n");
    if has_main {
        s.push_str("  await main();\n");
    } else {
        s.push_str("  console.log(\"[marea] no hay función main() en @client\");\n");
    }
    s.push_str("} finally {\n  await stopServer();\n}\n");
    s
}

// --- funciones ---

fn emit_fn_def(f: &FnDecl, export: bool) -> String {
    let params = ts_params(f);
    let kw = if export {
        "export async function"
    } else {
        "async function"
    };
    // El conjunto de variables reactivas se construye incrementalmente por
    // bloque (respetando el alcance léxico), arrancando vacío.
    let body = emit_block_inner(&f.body, 1, &HashSet::new());
    format!(
        "{}\n{} {}({}) {{\n{}\n}}\n",
        signature_comment(f),
        kw,
        f.name,
        params,
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
        .unwrap_or_else(|| "unknown".to_string());
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

fn ts_params(f: &FnDecl) -> String {
    f.params
        .iter()
        .map(|p| format!("{}: {}", p.name, map_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
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

fn emit_stmt(stmt: &Stmt, indent: usize, reactive: &HashSet<String>) -> String {
    let p = pad(indent);
    match stmt {
        Stmt::Let(l) if l.reactive => {
            // Fuente reactiva (mutable) = signal; derivada (inmutable) = memo.
            let init = emit_expr(&l.value, reactive);
            if l.mutable {
                format!("{p}const {} = __signal({init});", l.name)
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
                        s.push_str(&format!(
                            "{pin}{{ const {name} = __m;\n{body}\n{pin}}}\n"
                        ));
                    }
                    caught_all = true;
                }
            }
            Pattern::Int { value, .. } => {
                let kw = if chained { "else if" } else { "if" };
                s.push_str(&format!("{pin}{kw} (__m === {value}) {{\n{body}\n{pin}}}\n"));
                chained = true;
            }
            Pattern::Bool { value, .. } => {
                let kw = if chained { "else if" } else { "if" };
                s.push_str(&format!("{pin}{kw} (__m === {value}) {{\n{body}\n{pin}}}\n"));
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
        Expr::Int { value, .. } => value.to_string(),
        Expr::Float { value, .. } => value.to_string(),
        Expr::Str { value, .. } => js_string(value),
        Expr::Bool { value, .. } => value.to_string(),
        Expr::Ident { name, .. } => {
            if reactive.contains(name) {
                // Leer una reactiva = su getter (rastrea dependencias).
                format!("{name}.get()")
            } else if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                // Variante nominal usada como valor (errores como valores): se
                // representa por su etiqueta, que '__marea_is' reconoce en match.
                js_string(name)
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
                BinOp::Div => format!("Math.trunc({l} / {r})"),
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
            // Builtins SÍNCRONOS (no se 'await'). Los de estado (guardar/todos/
            // actualizar/borrar) son async (pegan a la BD) y SÍ se awaitan.
            let is_sync_builtin = matches!(
                callee.as_ref(),
                Expr::Ident { name, .. }
                    if matches!(name.as_str(), "print" | "concat" | "render" | "len" | "aTexto" | "escapar")
            );
            if is_sync_builtin {
                format!("{}({})", callee_ts, a.join(", "))
            } else {
                format!("(await {}({}))", callee_ts, a.join(", "))
            }
        }
        Expr::Member { object, field, .. } => format!("{}.{}", emit_expr(object, reactive), field),
        // 'match' en posición de expresión: IIFE que RETORNA el valor de la rama.
        Expr::Match { scrutinee, arms, .. } => {
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
        // `List<T>` -> `T[]` (arreglo TS). Sin args, una lista genérica.
        Type::Name { name, args, .. } if name == "List" => match args.first() {
            Some(elem) => format!("{}[]", map_type(elem)),
            None => "unknown[]".to_string(),
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
    let loc = match f.location {
        Some(Location::Server) => "@server ",
        Some(Location::Client) => "@client ",
        Some(Location::Edge) => "@edge ",
        None => "",
    };
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
    use super::fill_template;

    #[test]
    fn fill_template_no_reescanea_lo_inyectado() {
        // El valor inyectado por A contiene el centinela de B: en una sola pasada
        // NO debe volver a sustituirse (a diferencia de encadenar str::replace).
        let tpl = "x=A; y=B;";
        let out = fill_template(tpl, &[("A", "B"), ("B", "Z")]);
        assert_eq!(out, "x=B; y=Z;");
    }

    #[test]
    fn fill_template_respeta_el_orden_izquierda_a_derecha() {
        let out = fill_template("__P1__ medio __P2__", &[("__P1__", "uno"), ("__P2__", "dos")]);
        assert_eq!(out, "uno medio dos");
    }
}
