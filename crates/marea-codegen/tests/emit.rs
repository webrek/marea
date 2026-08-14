//! Tests del transpilador a TypeScript.

use marea_codegen::emit;
use marea_syntax::parse;

fn build(src: &str) -> marea_codegen::Project {
    emit(&parse(src).unwrap())
}

#[test]
fn server_define_y_registra_handler() {
    let p = build(
        r#"
        @server
        fn saludar(nombre: String) -> String {
            return concat("hola ", nombre);
        }
        @client
        fn main() { let m = saludar("Marea"); print(m); }
        "#,
    );
    // El servidor define la función real y la registra.
    assert!(p.server.contains("async function saludar(nombre: string)"));
    assert!(p.server.contains(r#"__register("saludar""#));
    // El servidor NO contiene un fetch (es el productor, no el consumidor).
    assert!(!p.server.contains("__rpc("));
}

#[test]
fn cliente_genera_stub_rpc_para_funcion_servidor() {
    let p = build(
        r#"
        @server
        fn saludar(nombre: String) -> String { return nombre; }
        @client
        fn main() { let m = saludar("x"); print(m); }
        "#,
    );
    // El cliente recibe un stub con el mismo nombre que cruza por RPC.
    assert!(p.client.contains("async function saludar(nombre: string)"));
    assert!(p.client.contains(r#"__rpc("saludar", [nombre])"#));
    // 'main' la llama como si fuera local.
    assert!(p.client.contains("await saludar(\"x\")"));
    // Y 'main' se exporta para el orquestador.
    assert!(p.client.contains("export async function main()"));
}

#[test]
fn precedencia_se_conserva_con_parentesis() {
    let p = build("@client fn f() { let x = 1 + 2 * 3; print(x); }");
    assert!(p.client.contains("(1 + (2 * 3))"));
}

#[test]
fn reactive_derivada_es_memo() {
    let p = build("@client fn f() { reactive x = 1; print(x); }");
    // Una 'reactive' (no mut) compila a un memo, y su lectura a '.get()'.
    assert!(
        p.client.contains("const x = __memo(() => 1)"),
        "{}",
        p.client
    );
    assert!(p.client.contains("print(x.get())"), "{}", p.client);
}

#[test]
fn demo_orquesta_servidor_y_main() {
    let p = build("@server fn s() -> Int { return 1; } @client fn main() { print(s()); }");
    assert!(p.demo.contains("await startServer()"));
    assert!(p.demo.contains("await main()"));
    assert!(p.demo.contains("await stopServer()"));
}

#[test]
fn runtime_lleva_el_transporte() {
    // El fuente DEBE cruzar la frontera: un módulo que sólo calcula ya no lleva
    // el runtime de servidor (se recorta para que pueda vivir fuera de Node),
    // así que probar el transporte con un módulo puro no probaría nada.
    let p = build("@server fn s() -> Int { return 1; }\n@client fn main() { print(s()); }");
    assert!(p.runtime.contains("export async function __rpc"));
    assert!(p.runtime.contains("export function startServer"));
}

#[test]
fn reactivo_genera_signal_memo_y_effect() {
    let p = build(
        "@client fn main() { reactive mut n = 0; reactive doble = n * 2; effect { print(doble); } n = n + 1; }",
    );
    assert!(p.client.contains("const n = __signal(0)"), "{}", p.client);
    assert!(
        p.client
            .contains("const doble = __memo(() => (n.get() * 2))"),
        "{}",
        p.client
    );
    assert!(p.client.contains("__effect(async () =>"), "{}", p.client);
    // Lectura reactiva -> .get(); asignación -> .set()
    assert!(p.client.contains("print(doble.get())"), "{}", p.client);
    assert!(p.client.contains("n.set((n.get() + 1))"), "{}", p.client);
}

#[test]
fn runtime_lleva_el_nucleo_reactivo() {
    let p = build("@client fn main() { print(1); }");
    assert!(p.runtime.contains("export function __signal"));
    assert!(p.runtime.contains("export function __effect"));
    assert!(p.runtime.contains("export function __memo"));
}

#[test]
fn builtins_no_se_awaitan() {
    // Un 'await' espurio en print rompía el rastreo reactivo dentro de effect.
    let p = build("@client fn main() { reactive mut a = 1; effect { print(a); } a = 2; }");
    assert!(p.client.contains("print(a.get())"), "{}", p.client);
    assert!(!p.client.contains("(await print("), "{}", p.client);
}

#[test]
fn division_entera_trunca() {
    // JS '/' daría flotante; debe truncar para no romper el contrato Int. Va por
    // un helper que además corta el divisor cero: `7/0` daría Infinity dentro de
    // un Int, y el backend WASM trapea —el mismo programa no puede tener dos
    // finales—.
    let p = build("fn d() -> Int { return 7 / 2; }");
    assert!(p.client.contains("__div(7, 2)"), "{}", p.client);
    assert!(p.runtime.contains("export function __div"), "{}", p.runtime);
    assert!(p.runtime.contains("división entre cero"), "{}", p.runtime);
}

#[test]
fn el_modulo_tambien_corta_el_divisor_cero() {
    let p = build("fn m() -> Int { return 7 % 2; }");
    assert!(p.client.contains("__rem(7, 2)"), "{}", p.client);
    assert!(p.runtime.contains("export function __rem"), "{}", p.runtime);
}

#[test]
fn variante_como_valor_lleva_etiqueta_reservada() {
    let p = build("@client fn f(n: Int) -> A | B { if n > 0 { return A; } return B; }");
    // Con etiqueta en el campo reservado `$tag`, no como cadena desnuda: el
    // lexer no admite `$` en un identificador, así que ningún registro del
    // usuario puede tener ese campo y hacerse pasar por una variante. Antes, un
    // campo llamado `tag`, `kind` o `type` decidía qué rama del match corría.
    assert!(p.client.contains(r#"return { $tag: "A" }"#), "{}", p.client);
    assert!(p.client.contains(r#"return { $tag: "B" }"#), "{}", p.client);
}

#[test]
fn el_discriminante_solo_mira_el_campo_reservado() {
    let p = build("@client fn f(n: Int) -> A | B { if n > 0 { return A; } return B; }");
    assert!(p.runtime.contains(".$tag === tag"), "{}", p.runtime);
    // Ya no se consultan campos de datos del usuario.
    assert!(!p.runtime.contains("v.kind === tag"), "{}", p.runtime);
    assert!(!p.runtime.contains("v.type === tag"), "{}", p.runtime);
}

#[test]
fn match_como_expresion_retorna_valor() {
    let p = build("@client fn f(n: Int) -> String { return match n { 0 => \"c\", _ => \"o\" }; }");
    // El IIFE debe RETORNAR el valor de la rama, no quedar en undefined.
    assert!(p.client.contains("return \"c\""), "{}", p.client);
}

#[test]
fn local_sombrea_a_reactiva() {
    // Un 'let n' no-reactivo dentro de un bloque sombrea a la reactiva externa:
    // su lectura NO debe emitir .get().
    let p = build(
        "@client fn main() { reactive mut n = 0; effect { let n = 99; print(n); } n = n + 1; }",
    );
    assert!(p.client.contains("const n = 99"), "{}", p.client);
    assert!(
        p.client.contains("print(n)") && !p.client.contains("print(n.get())"),
        "{}",
        p.client
    );
    // La reactiva externa sigue siendo signal.
    assert!(p.client.contains("const n = __signal(0)"), "{}", p.client);
}

#[test]
fn emit_web_genera_html_y_glue() {
    let m = marea_syntax::parse("fn vista() -> String { return concat(\"a\", \"b\"); }").unwrap();
    let (html, glue) = marea_codegen::emit_web(&m);
    assert!(html.contains("id=\"salida\""), "html: {html}");
    assert!(html.contains("./glue.mjs"), "html: {html}");
    assert!(glue.contains("WebAssembly.instantiate"), "glue: {glue}");
    // vista() devuelve String: el render decodifica el puntero desde memoria.
    assert!(
        glue.contains("decodificarCadena(exports.vista())"),
        "glue: {glue}"
    );
}

#[test]
fn map_type_traduce_list_generica() {
    let p = build("@server fn feed() -> List<String> { return []; } @client fn main() { let x = feed(); print(x); }");
    // El stub del cliente debe tipar el retorno como string[], no como 'List'.
    assert!(p.client.contains("Promise<string[]>"), "{}", p.client);
}

#[test]
fn web_entry_no_string_no_decodifica() {
    let m = marea_syntax::parse("fn vista() -> Int { return 42; }").unwrap();
    let (_html, glue) = marea_codegen::emit_web(&m);
    assert!(glue.contains("String(exports.vista())"), "glue: {glue}");
    assert!(
        !glue.contains("decodificarCadena(exports.vista())"),
        "glue: {glue}"
    );
}

// --- app web (marea build-app): RPC + reactivo + DOM ---

fn app(src: &str) -> marea_codegen::AppProject {
    marea_codegen::emit_app(&parse(src).unwrap())
}

#[test]
fn app_reactiva_de_modulo_es_signal() {
    // Una `reactive mut` de nivel superior se vuelve un signal de módulo.
    let a = app(
        "reactive mut posts = [];\n@client fn vista() -> String { let p = posts; return \"x\"; }",
    );
    assert!(
        a.client_js.contains("const posts = __signal([]);"),
        "{}",
        a.client_js
    );
    // La vista lee el signal con .get().
    assert!(a.client_js.contains("posts.get()"), "{}", a.client_js);
}

#[test]
fn app_cliente_es_js_sin_tipos_ni_imports_node() {
    let a = app(
        "@server fn feed() -> List<Int> { return all(almacen); }\n@client fn main() { feed(); }",
    );
    // El cliente de navegador NO importa Node ni lleva anotaciones de tipo.
    assert!(!a.client_js.contains("node:http"), "{}", a.client_js);
    assert!(!a.client_js.contains(": number"), "{}", a.client_js);
    assert!(!a.client_js.contains(": string"), "{}", a.client_js);
    // El @server se vuelve un stub fetch al mismo origen.
    assert!(a.client_js.contains("__rpc(\"feed\""), "{}", a.client_js);
    assert!(
        a.client_js.contains("fetch(\"/__marea\""),
        "{}",
        a.client_js
    );
}

#[test]
fn app_arranca_main_y_monta_vista() {
    let a = app(
        "@client fn vista() -> String { return \"hola\"; }\n@client fn main() { print(\"hi\"); }",
    );
    assert!(a.client_js.contains("await main();"), "{}", a.client_js);
    assert!(a.client_js.contains("__mount(vista);"), "{}", a.client_js);
    // Expone las funciones en window.marea para los onclick del HTML.
    assert!(
        a.client_js.contains("globalThis.marea = {"),
        "{}",
        a.client_js
    );
    // index.html tiene el contenedor #app y carga client.js como módulo.
    assert!(a.index_html.contains("id=\"app\""), "{}", a.index_html);
    assert!(
        a.index_html.contains("src=\"./client.js\""),
        "{}",
        a.index_html
    );
}

#[test]
fn app_servidor_sirve_estaticos() {
    let a = app("@server fn feed() -> List<Int> { return all(almacen); }");
    // El runtime Node sirve estáticos (la app vive en el mismo origen que el RPC).
    assert!(a.runtime.contains("__serveStatic"), "{}", a.runtime);
    assert!(a.runtime.contains("MAREA_WEB_ROOT"), "{}", a.runtime);
    // El entry deja el servidor vivo y fija la raíz estática.
    assert!(a.serve.contains("startServer()"), "{}", a.serve);
    assert!(a.serve.contains("MAREA_WEB_ROOT"), "{}", a.serve);
}

#[test]
fn store_builtins_son_async_y_se_awaitan() {
    // El fuente DEBE declarar el almacén: sin `store`, el runtime emitido ya no
    // trae la capa de persistencia (se recorta para que un empaquetador no
    // tropiece con los drivers de npm). Antes esto pasaba con un fuente que ni
    // siquiera tipaba.
    let p = build(
        "store almacen: Int;\n@server fn g(x: Int) { save(almacen, x); }\n\
         @server fn t() -> List<Int> { return all(almacen); }",
    );
    // Con backends de BD, las operaciones del store son asíncronas (I/O).
    assert!(p.runtime.contains("export function save"), "{}", p.runtime);
    assert!(p.runtime.contains("export function all"), "{}", p.runtime);
    // El call site las espera.
    assert!(
        p.server.contains("(await save(almacen, x))"),
        "{}",
        p.server
    );
    assert!(p.server.contains("(await all(almacen))"), "{}", p.server);
}

#[test]
fn store_persiste_a_disco() {
    let p = build("store almacen: Int;\n@server fn g(x: Int) { save(almacen, x); }");
    // El runtime carga el store del archivo y lo reescribe en cada guardar.
    assert!(p.runtime.contains("readFileSync"), "{}", p.runtime);
    assert!(p.runtime.contains("writeFileSync"), "{}", p.runtime);
    assert!(p.runtime.contains("MAREA_STORE"), "{}", p.runtime);
}

#[test]
fn indexado_usa_bounds_check() {
    let p = build("@client fn f(xs: List) { let a = xs[0]; print(a); }");
    assert!(p.client.contains("__index(xs, 0)"), "{}", p.client);
    assert!(
        p.runtime.contains("export function __index"),
        "{}",
        p.runtime
    );
}

// --- endurecimiento de seguridad (auditoría 2026-06-18) ---

#[test]
fn handler_valida_aridad_de_args() {
    // M1: el wrapper RPC exige la aridad exacta antes de invocar la función,
    // para que un argumento faltante no se cuele como undefined.
    let p = build("@server fn pub(a: String, b: Int) {}");
    assert!(
        p.server
            .contains(r#"if (__args.length !== 2) __badRequest("aridad")"#),
        "{}",
        p.server
    );
}

// El límite de red recibe JSON arbitrario: sin validar los TIPOS, la garantía
// del verificador terminaba en el fetch y un String declarado podía llegar como
// objeto y persistirse así.
#[test]
fn el_handler_valida_los_tipos_de_los_argumentos() {
    let p = build("@server fn pub(a: String, b: Int) {}");
    assert!(
        p.server.contains(r#"typeof __args[0] === "string""#),
        "{}",
        p.server
    );
    assert!(
        p.server.contains("Number.isSafeInteger(__args[1])"),
        "{}",
        p.server
    );
}

#[test]
fn el_validador_recorre_listas_y_registros() {
    let p = build("type Post = { autor: String, likes: Int };\n@server fn g(ps: List<Post>) {}");
    assert!(
        p.server.contains("Array.isArray(__args[0])"),
        "{}",
        p.server
    );
    assert!(p.server.contains(r#"__e["autor"]"#), "{}", p.server);
    assert!(p.server.contains(r#"__e["likes"]"#), "{}", p.server);
}

// Un fallo de validación es culpa del cliente: 400, no 500, y sin eco del
// detalle (sería un oráculo de las firmas).
#[test]
fn los_errores_de_limite_responden_400() {
    let p = build("@server fn g(a: Int) {}");
    assert!(p.runtime.contains("__BoundaryError"), "{}", p.runtime);
    assert!(p.runtime.contains("res.statusCode = 400;"), "{}", p.runtime);
}

// Sin exigir JSON, un formulario cross-origin con enctype="text/plain" califica
// como petición simple, se salta el preflight y ejecuta el handler.
#[test]
fn el_endpoint_exige_json_y_valida_el_origen() {
    let p = build("@server fn g(a: Int) {}");
    assert!(p.runtime.contains("res.statusCode = 415;"), "{}", p.runtime);
    assert!(p.runtime.contains("MAREA_ALLOWED_ORIGINS"), "{}", p.runtime);
    assert!(p.runtime.contains("origen no permitido"), "{}", p.runtime);
}

#[test]
fn transporte_rpc_esta_endurecido() {
    // H1/L1/M2: bind a loopback, tope de cuerpo, tabla de handlers sin prototipo,
    // y errores genéricos al cliente (sin reflejar String(e) ni el nombre de fn).
    let p = build("@server fn f() {}");
    assert!(p.runtime.contains("MAREA_MAX_BODY"), "falta tope de cuerpo");
    assert!(p.runtime.contains("statusCode = 413"), "falta rechazo 413");
    assert!(
        p.runtime.contains("Object.create(null)"),
        "handlers deben ir sin prototipo"
    );
    assert!(
        p.runtime.contains("listen(MAREA_PORT, MAREA_HOST"),
        "debe bindear host explícito"
    );
    assert!(
        p.runtime.contains(r#"error: "error interno""#),
        "el error al cliente debe ser genérico"
    );
    // No debe filtrar el error crudo ni hacer eco del nombre de función.
    assert!(
        !p.runtime.contains("error: String(e)"),
        "no debe reflejar String(e)"
    );
    assert!(
        !p.runtime.contains("función desconocida: ${fn}"),
        "no debe hacer eco del fn"
    );
}

#[test]
fn backends_sql_comillan_identificadores() {
    // L2: tabla/columnas se comillan por dialecto, así un campo con nombre de
    // palabra reservada (from, order…) no rompe el SQL generado.
    let p = build("type Fila = { from: String, order: Int };\nstore almacen: Fila;\n@server fn g() { save(almacen, Fila { from: \"a\", order: 1 }); }");
    assert!(p.runtime.contains("function __quoteId"), "{}", p.runtime);
    assert!(p.runtime.contains("function __idCol"), "{}", p.runtime);
    // sqlite/pg usan comilla doble; mysql usa backtick (constantes por backend).
    assert!(
        p.runtime.contains("const q = '\"'"),
        "sqlite/pg deben comillar con \""
    );
    assert!(
        p.runtime.contains("const q = \"`\""),
        "mysql debe comillar con backtick"
    );
}

#[test]
fn persistencia_a_archivo_es_incremental_y_atomica() {
    // H2/L5: el backend de archivo usa un log append-only (O(1) por mutación) y
    // compacta atómicamente (temporal + rename). Sin reescritura completa por op.
    let p = build("store almacen: Int;\n@server fn g(x: Int) { save(almacen, x); }");
    assert!(
        p.runtime.contains("appendFileSync"),
        "debe escribir incremental (append)"
    );
    assert!(
        p.runtime.contains("renameSync"),
        "la compactación debe ser atómica"
    );
    assert!(p.runtime.contains(".tmp"), "debe compactar vía temporal");
}

#[test]
fn backends_son_incrementales_por_id() {
    // H2: la interfaz del backend es insert/update/remove por id, no saveAll;
    // las mutaciones tocan una sola fila.
    let p = build("store almacen: Int;\n@server fn g(x: Int) { save(almacen, x); }");
    assert!(
        p.runtime.contains("insert(id: number, item: unknown)"),
        "{}",
        p.runtime
    );
    assert!(p.runtime.contains("remove(id: number)"), "{}", p.runtime);
    assert!(
        !p.runtime.contains("saveAll"),
        "saveAll (reescritura total) debe haber desaparecido"
    );
    // El esquema SQL gana una clave primaria interna.
    assert!(p.runtime.contains("INTEGER PRIMARY KEY"), "{}", p.runtime);
}

#[test]
fn placeholders_se_sustituyen_en_una_pasada() {
    // L3: un campo cuyo nombre coincide con un centinela del template no debe
    // corromper la sustitución del otro. Tras emitir no quedan centinelas crudos.
    let p = build("type T = { __MAREA_STORE_SCHEMA__: Int };\nstore almacen: T;\n@server fn g() { save(almacen, T { __MAREA_STORE_SCHEMA__: 1 }); }");

    // El único '__MAREA_STORE_SCHEMA__' admisible es el nombre de columna inyectado,
    // no un placeholder del template suelto en una posición de código.
    // El nombre de columna viaja al literal del esquema sin corromper nada.
    assert!(p.server.contains("__MAREA_STORE_SCHEMA__"), "{}", p.server);
}

#[test]
fn store_inyecta_esquema_de_columnas() {
    let p = build("type Post = { texto: String, likes: Int };\nstore almacen: Post;\n@server fn g() { save(almacen, Post { texto: \"a\", likes: 0 }); }");
    // El esquema inyectado describe tabla + columnas tipadas para los backends SQL.
    // La tabla toma el NOMBRE del almacén (dos almacenes del mismo tipo son
    // dos tablas), y las columnas salen de los campos del registro.
    assert!(p.server.contains("table: \"almacen\""), "{}", p.server);
    assert!(
        p.server.contains("{ name: \"texto\", kind: \"text\" }"),
        "{}",
        p.server
    );
    assert!(
        p.server.contains("{ name: \"likes\", kind: \"int\" }"),
        "{}",
        p.server
    );
}

#[test]
fn store_escalar_usa_columna_doc() {
    let p = build("store almacen: Int;\n@server fn g(x: Int) { save(almacen, x); }");
    // Un store no-registro guarda el valor entero como una sola columna JSON.
    assert!(
        p.server.contains("{ name: \"__doc\", kind: \"json\" }"),
        "{}",
        p.server
    );
}

#[test]
fn sin_store_el_esquema_es_null() {
    let p = build("@client fn f() { print(\"hola\"); }");
    assert!(
        !p.server.contains("__store("),
        "sin store no debe declararse ninguno:\n{}",
        p.server
    );
}

#[test]
fn store_tiene_backends_de_base_de_datos() {
    let p = build("store almacen: Int;\n@server fn g(x: Int) { save(almacen, x); }");
    // Los cinco backends conviven; el driver se elige con MAREA_DB.
    for marca in [
        "__sqliteBackend",
        "__postgresBackend",
        "__mysqlBackend",
        "__mongoBackend",
        "MAREA_DB",
    ] {
        assert!(p.runtime.contains(marca), "falta {marca} en runtime");
    }
    // Los drivers externos se importan de forma perezosa (no rompen si no están).
    assert!(
        p.runtime.contains("await import(\"node:sqlite\")"),
        "{}",
        p.runtime
    );
    assert!(p.runtime.contains("await import(\"pg\")"), "{}", p.runtime);
}

// --- almacén prestado (`store x: T from "tabla"`) ---
//
// Lo que se comprueba de verdad —que Marea lee una tabla que no creó y que falla
// nombrando la columna que falta— vive en `prestado_integracion.rs`, contra una
// base SQLite real. Aquí sólo lo que es literalmente texto emitido: el esquema
// que viaja al runtime y la lista de importación.

/// La línea `import { ... } from "./runtime.ts"` del bundle de servidor. Se
/// extrae en vez de buscar el nombre suelto en todo el archivo: `save` aparece
/// también en cualquier comentario o identificador que lo contenga, y una
/// aserción así pasa (o falla) por la prosa, no por el código.
fn importados(server: &str) -> Vec<&str> {
    let linea = server
        .lines()
        .find(|l| l.starts_with("import {"))
        .expect("el bundle de servidor siempre importa del runtime");
    let tras = linea.split_once('{').expect("la importación abre").1;
    let cuerpo = tras.split_once('}').expect("la importación cierra").0;
    cuerpo.split(',').map(|n| n.trim()).collect()
}

#[test]
fn store_prestado_no_normaliza_el_nombre_de_la_tabla() {
    let p = build(
        r#"
        type Producto = { sku: String, precio: Int };
        store productos: Producto from "priceObservations";
        @server fn catalogo() -> List<Producto> { return all(productos); }
        "#,
    );
    // La tabla es la que dice el fuente, con sus mayúsculas: es de otro, y
    // adivinarle el nombre es justo donde esto se rompería en silencio.
    assert!(
        p.server.contains("table: \"priceObservations\""),
        "{}",
        p.server
    );
    assert!(p.server.contains("prestado: true"), "{}", p.server);
    // Nombre de campo = nombre de columna, tal cual (sin camelCase→snake_case).
    assert!(
        p.server.contains("{ name: \"sku\", kind: \"text\" }"),
        "{}",
        p.server
    );
    // Ni '__id' ni '__doc': esas columnas son de los almacenes propios.
    assert!(!p.server.contains("__doc"), "{}", p.server);
}

#[test]
fn store_propio_no_se_marca_como_prestado() {
    let p = build(
        r#"
        type Post = { texto: String };
        store almacen: Post;
        @server fn g() { save(almacen, Post { texto: "a" }); }
        "#,
    );
    assert!(p.server.contains("table: \"almacen\""), "{}", p.server);
    assert!(!p.server.contains("prestado"), "{}", p.server);
}

#[test]
fn con_solo_almacenes_prestados_no_se_importa_la_escritura() {
    let p = build(
        r#"
        type Producto = { sku: String };
        store productos: Producto from "products";
        @server fn catalogo() -> List<Producto> { return all(productos); }
        "#,
    );
    let ns = importados(&p.server);
    assert!(ns.contains(&"all"), "{ns:?}");
    assert!(ns.contains(&"__store"), "{ns:?}");
    // Sin nada que escribir, los builtins de escritura ni se nombran.
    for w in ["save", "update", "remove"] {
        assert!(!ns.contains(&w), "sobra '{w}': {ns:?}");
    }
}

#[test]
fn un_almacen_propio_basta_para_importar_la_escritura() {
    // Mezclar los dos no debe recortar de más: el propio sí se escribe.
    let p = build(
        r#"
        type Producto = { sku: String };
        type Nota = { t: String };
        store productos: Producto from "products";
        store notas: Nota;
        @server fn g() { save(notas, Nota { t: "a" }); }
        "#,
    );
    let ns = importados(&p.server);
    for w in ["save", "update", "remove"] {
        assert!(ns.contains(&w), "falta '{w}': {ns:?}");
    }
    assert!(p.server.contains("prestado: true"), "{}", p.server);
    assert!(p.server.contains("table: \"notas\""), "{}", p.server);
}

#[test]
fn store_file_lleva_la_firma_del_esquema() {
    let p = build("type Post = { a: Int };\nstore almacen: Post;\n@server fn g() { save(almacen, Post { a: 1 }); }");
    // El archivo por defecto incluye nombre+campos para no colisionar entre apps.
    assert!(p.server.contains("__store(\"almacen\""), "{}", p.server);
}

// --- regresiones de codegen (auditoría) ---

// A-1: una rama atrapa-todo en primera posición emitía `else` sin `if` previo,
// es decir JS que no parsea y tumba el módulo entero al cargarlo.
#[test]
fn match_con_catch_all_primero_no_emite_else_suelto() {
    let p = build("@client fn f(r: Int) { match r { x => print(x) } }");
    assert!(
        !p.client.contains("else {"),
        "no debe haber `else` sin `if` previo:\n{}",
        p.client
    );
}

#[test]
fn match_con_comodin_unico_no_emite_else_suelto() {
    let p = build("@client fn f(r: Int) { match r { _ => print(1) } }");
    assert!(!p.client.contains("else {"), "{}", p.client);
}

// Tras una rama atrapa-todo, el resto es inalcanzable: emitirla daría
// `else if` después de un `else`.
#[test]
fn match_descarta_ramas_tras_el_catch_all() {
    let p = build("@client fn g(r: Int) { match r { _ => print(1), A => print(2) } }");
    assert!(!p.client.contains("else if"), "{}", p.client);
    assert!(
        !p.client.contains("print(2)"),
        "rama inalcanzable emitida:\n{}",
        p.client
    );
}

// El caso que SÍ debe encadenar sigue haciéndolo.
#[test]
fn match_con_variantes_sigue_encadenando() {
    let p =
        build("@client fn f(r: Int) { match r { A => print(1), B => print(2), _ => print(3) } }");
    assert!(p.client.contains("else if"), "{}", p.client);
    assert!(p.client.contains("else {"), "{}", p.client);
}

// C-3: la carga del store se memoiza como promesa; sin eso dos RPC concurrentes
// creaban dos backends y el segundo pisaba al primero (ids duplicados / pérdida
// silenciosa de escrituras en el backend de archivo).
#[test]
fn ensure_store_memoiza_la_promesa_de_carga() {
    let p = build("store almacen: Int;\n@server fn g(x: Int) { save(almacen, x); }");
    assert!(
        p.runtime.contains("__loading"),
        "la carga debe memoizarse:\n{}",
        p.runtime
    );
}

// M-11: un valor de entorno mal formado daba NaN, y `size > NaN` es siempre
// false → el tope del cuerpo quedaba desactivado.
#[test]
fn los_limites_de_entorno_son_a_prueba_de_nan() {
    // El fuente DEBE cruzar la frontera: un módulo que sólo calcula ya no lleva
    // el runtime de servidor (se recorta para que pueda vivir fuera de Node),
    // así que probar el transporte con un módulo puro no probaría nada.
    let p = build("@server fn s() -> Int { return 1; }\n@client fn f() { print(s()); }");
    assert!(p.runtime.contains("__envInt"), "{}", p.runtime);
    assert!(
        !p.runtime.contains("Number(process.env.MAREA_MAX_BODY"),
        "MAREA_MAX_BODY no debe parsearse con Number() crudo"
    );
}

// C-2d: la raíz estática es el propio directorio de salida, así que sin lista
// blanca `GET /server.ts` filtraba los handlers y `GET /*.log` el store.
#[test]
fn los_estaticos_solo_sirven_extensiones_en_lista_blanca() {
    // El fuente DEBE cruzar la frontera: un módulo que sólo calcula ya no lleva
    // el runtime de servidor (se recorta para que pueda vivir fuera de Node),
    // así que probar el transporte con un módulo puro no probaría nada.
    let p = build("@server fn s() -> Int { return 1; }\n@client fn f() { print(s()); }");
    assert!(
        p.runtime.contains("const mime = __MIME[ext];"),
        "debe resolver el MIME antes de leer el archivo:\n{}",
        p.runtime
    );
    assert!(
        !p.runtime.contains("application/octet-stream"),
        "no debe haber tipo de reserva: eso servía .ts y .log"
    );
}

// C-2c: el builtin de escapado debe llegar a ambos runtimes.
#[test]
fn el_builtin_escape_esta_en_los_dos_runtimes() {
    let p = build("@client fn f() { print(escape(\"<b>\")); }");
    assert!(
        p.runtime.contains("export function escape"),
        "falta en runtime.ts"
    );
    assert!(p.client.contains("escape"), "{}", p.client);
}

// A-3: una función sin anotación es local desde cualquier lado, así que debe
// existir en AMBOS bundles. Antes solo se emitía en el cliente y extraer un
// helper usado por un @server daba ReferenceError en cada RPC.
#[test]
fn las_funciones_compartidas_llegan_al_servidor() {
    let p = build("fn ayuda(x: Int) -> Int { return x * 2; }\n@server fn calc(n: Int) -> Int { return ayuda(n); }");
    assert!(
        p.server.contains("function ayuda"),
        "falta el helper:\n{}",
        p.server
    );
}

// Una @client NO debe filtrarse al bundle del servidor.
#[test]
fn las_funciones_client_no_llegan_al_servidor() {
    let p = build("@client fn ui() { print(\"x\"); }\n@server fn s() { print(\"y\"); }");
    assert!(
        !p.server.contains("function ui"),
        "@client no debe ir al servidor:\n{}",
        p.server
    );
}

// A-2: una global no reactiva es una constante de módulo visible desde
// cualquier función; antes se descartaba y daba ReferenceError.
#[test]
fn las_globales_no_reactivas_se_emiten_en_ambos_bundles() {
    let p = build("let saludo = \"hola\";\n@server fn dime() -> String { return saludo; }\n@client fn m() { print(saludo); }");
    assert!(
        p.server.contains("const saludo"),
        "falta en server:\n{}",
        p.server
    );
    assert!(
        p.client.contains("const saludo"),
        "falta en client:\n{}",
        p.client
    );
}

// El comando insignia (build-app) no declaraba las globales no reactivas en
// client.js: se calculaban y solo se le pasaban al servidor, que no las usa.
#[test]
fn build_app_declara_las_globales_en_el_cliente() {
    let m = marea_syntax::parse(
        "let titulo = \"Marea\";\n@client fn vista() -> String { return escape(titulo); }",
    )
    .expect("parsea");
    let app = marea_codegen::emit_app(&m);
    assert!(app.client_js.contains("const titulo"), "{}", app.client_js);
}

// `escapar` es síncrono: emitirlo con await rompe el rastreo de dependencias
// reactivas en silencio.
#[test]
fn escapar_no_se_emite_con_await() {
    let p = build("@client fn f(s: String) -> String { return escape(s); }");
    assert!(!p.client.contains("await escapar"), "{}", p.client);
}

/// `site/app/` es un artefacto GENERADO que está commiteado, y el Dockerfile lo
/// copia tal cual: es literalmente lo que se despliega. Estuvo cuatro meses sin
/// regenerar y se desplegó con un XSS y una fuga de código fuente que el
/// compilador ya tenía arreglados. Este test falla si vuelve a divergir; el
/// arreglo es `marea build-app site/marea-demo.mar site/app`.
///
/// `index.html` queda fuera a propósito: es una landing escrita a mano, y por
/// eso `build-app` respeta el que ya exista.
#[test]
fn el_artefacto_desplegable_no_se_desincroniza() {
    let raiz = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
    let fuente = std::fs::read_to_string(format!("{raiz}/site/marea-demo.mar"))
        .expect("no se pudo leer site/marea-demo.mar");
    let module = marea_syntax::parse(&fuente).expect("la demo del sitio debe parsear");
    let app = marea_codegen::emit_app(&module);

    for (nombre, esperado) in [
        ("runtime.ts", &app.runtime),
        ("server.ts", &app.server),
        ("serve.ts", &app.serve),
        ("client.js", &app.client_js),
    ] {
        let ruta = format!("{raiz}/site/app/{nombre}");
        let actual = std::fs::read_to_string(&ruta).unwrap_or_else(|_| panic!("falta {ruta}"));
        assert_eq!(
            &actual, esperado,
            "site/app/{nombre} no coincide con el codegen actual; \
             regenera con: marea build-app site/marea-demo.mar site/app"
        );
    }
}

// Los validadores del límite recursan sobre los campos de un registro, así que
// un tipo recursivo (válido en el lenguaje) desbordaba la pila: `check` pasaba y
// `build` moría. Es la misma clase de fallo que ya se había cerrado en el
// verificador; aquí se cierra en el generador.
#[test]
fn el_validador_no_desborda_con_tipos_recursivos() {
    let p = build("type Nodo = { v: Int, sig: Nodo };\n@server fn append(n: Nodo) { print(n.v); }");
    assert!(p.server.contains("__register(\"append\""), "{}", p.server);
}

#[test]
fn el_validador_no_desborda_con_recursion_mutua() {
    let p = build("type A = { x: B };\ntype B = { y: A };\n@server fn g(a: A) { print(1); }");
    assert!(p.server.contains("__register(\"g\""), "{}", p.server);
}

#[test]
fn los_builtins_de_lista_y_texto_llegan_al_runtime() {
    let p = build("fn f(a: List<Int>, b: List<Int>) -> List<Int> { return concat(a, b); }");
    for n in [
        "export function concat",
        "export function append",
        "export function contains",
    ] {
        assert!(p.runtime.contains(n), "falta {n} en runtime.ts");
    }
    // Son síncronos: emitirlos con await rompería el rastreo reactivo.
    assert!(!p.client.contains("await append"), "{}", p.client);
}

// ================== Enrutado: lo que se decide AL COMPILAR ==================
//
// Lo que se decide al servir —qué responde `/modelo/7`, si `/modelo/abc` es un
// 404, si dos peticiones a la vez se pisan la query— se ejercita contra el
// servidor levantado, en `rutas_integracion.rs`. Aquí sólo lo que es una
// decisión del compilador y no se puede observar corriendo una sola URL: el
// ORDEN de la tabla y a qué bundle va cada función.

/// Las líneas de CÓDIGO del texto emitido, sin comentarios.
///
/// El filtro no es cosmético. La salida lleva un comentario con la firma de
/// cada función, así que una aserción sobre "el texto emitido" puede darse por
/// satisfecha con la prosa en vez de con la línea que importa. Ya ha pasado.
fn codigo(texto: &str) -> Vec<&str> {
    texto
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .collect()
}

/// Las líneas `__ruta(...)` de un bundle de servidor, en orden.
fn tabla(server: &str) -> Vec<&str> {
    codigo(server)
        .into_iter()
        .filter(|l| l.starts_with("__ruta("))
        .collect()
}

/// El orden de la tabla lo decide la especificidad, no el orden del archivo.
///
/// Si mandara el archivo, mover una función treinta líneas arriba cambiaría qué
/// página sirve una URL —y eso no se ve en el diff de la línea que se tocó—.
#[test]
fn la_tabla_va_de_mas_especifica_a_mas_general() {
    let p = build(
        r#"
        @page("/modelo/:id")
        fn ficha(id: Int) -> Pagina { return Pagina { cuerpo: `a` }; }
        @page("/modelo/nuevo")
        fn nuevo() -> Pagina { return Pagina { cuerpo: `b` }; }
        "#,
    );
    let rutas = tabla(&p.server);
    assert_eq!(rutas.len(), 2, "{}", p.server);
    assert!(rutas[0].contains("/modelo/nuevo"), "{}", p.server);
    assert!(rutas[1].contains("/modelo/:id"), "{}", p.server);
}

/// Los segmentos se atan a los parámetros POR NOMBRE. Por posición se cruzarían
/// en silencio cuando los dos son del mismo tipo: ningún error, otra página.
#[test]
fn los_segmentos_se_atan_por_nombre() {
    let p = build(
        r#"
        @page("/p/:x/:y")
        fn par(y: String, x: String) -> Pagina { return Pagina { cuerpo: `z` }; }
        "#,
    );
    let rutas = tabla(&p.server);
    let ruta = rutas.first().expect("debe haber una ruta");
    // La ruta declara :x antes que :y; la función pide (y, x) y así se la llama.
    assert!(ruta.contains("par(__p[\"y\"]"), "{ruta}");
    assert!(ruta.contains("nombre: \"x\", tipo: \"String\""), "{ruta}");
}

/// El tipo del parámetro viaja a la tabla: es lo que convierte el segmento y,
/// cuando no convierte, lo que hace que la URL no exista.
#[test]
fn el_tipo_del_segmento_viaja_a_la_tabla() {
    let p = build(
        r#"
        @page("/m/:id")
        fn ficha(id: Int) -> Pagina { return Pagina { cuerpo: `a` }; }
        "#,
    );
    let ruta = tabla(&p.server)[0];
    assert!(ruta.contains("nombre: \"id\", tipo: \"Int\""), "{ruta}");
    assert!(ruta.contains("ficha(__p[\"id\"] as number)"), "{ruta}");
}

/// Una página corre en el SERVIDOR. Sin esto viajaba también al bundle del
/// navegador —donde no puede correr—, y de paso le mandaba al cliente el cuerpo
/// de una función que consulta la base de datos.
#[test]
fn una_pagina_no_viaja_al_navegador() {
    let src = r#"
        @page("/x")
        fn pagina() -> Pagina { return Pagina { cuerpo: `x` }; }
        "#;
    let p = build(src);
    assert!(p.server.contains("async function pagina"), "{}", p.server);
    assert!(!p.client.contains("function pagina"), "{}", p.client);
    let a = app(src);
    assert!(!a.client_js.contains("function pagina"), "{}", a.client_js);
}

/// Un módulo que SÓLO sirve páginas no es puro: necesita el servidor que las
/// sirva. Antes se emitía con el runtime recortado —sin `node:http`— y se
/// quedaba sin nadie que atendiera sus propias rutas.
#[test]
fn un_modulo_de_solo_paginas_lleva_servidor() {
    let p = build(
        r#"
        @page("/")
        fn casa() -> Pagina { return Pagina { cuerpo: `h` }; }
        "#,
    );
    for n in [
        "node:http",
        "export function __ruta",
        "export function consulta",
        "export function textoPlano",
        "export function documentoXml",
    ] {
        assert!(p.runtime.contains(n), "falta '{n}' en runtime.ts");
    }
    // Y el bundle los importa: sin el import, el nombre no existe al ejecutar.
    assert!(p.server.contains("__ruta"), "{}", p.server);
}

/// Un módulo SIN páginas no paga nada: ni importa los builtins del enrutado ni
/// registra una tabla vacía.
#[test]
fn sin_paginas_no_hay_tabla_ni_importaciones_de_ruta() {
    let p = build("@server fn f() -> Int { return 1; }");
    assert!(tabla(&p.server).is_empty(), "{}", p.server);
    for n in ["__ruta", "consulta", "textoPlano", "documentoXml"] {
        assert!(!p.server.contains(n), "sobra '{n}' en server.ts");
    }
}

// ================== Un solo núcleo para los dos runtimes ==================

/// Lo que los dos runtimes comparten. Estuvo duplicado, con implementaciones
/// separadas que había que sincronizar a mano, y el precio fue real: el
/// `.append()` sobre un `Set` —un Set de JS tiene `.add`— estaba en los SEIS
/// sitios, tres por runtime, y rompía el núcleo reactivo entero.
///
/// `__rpc` está en los dos archivos y NO está en esta lista: comparte el nombre
/// pero no la función. Uno habla con un servidor por URL absoluta con el `fetch`
/// del entorno capturado; el otro va al mismo origen desde el navegador. Son dos
/// transportes, y juntarlos sería inventar una abstracción para tapar que el
/// nombre coincide.
const COMPARTIDAS: [&str; 19] = [
    "__div",
    "__effect",
    "__index",
    "__marea_is",
    "__memo",
    "__rem",
    "__resource",
    "__signal",
    "append",
    "concat",
    "contains",
    "entero",
    "escape",
    "html",
    "len",
    "lower",
    "print",
    "render",
    "text",
];

/// Las diecinueve salen del MISMO texto (`nucleo.js`), así que aparecen una vez
/// en cada runtime y ninguna se queda fuera. Esto es lo que impide volver a
/// partirlas: añadir una a un runtime y olvidarla en el otro se pone rojo aquí.
#[test]
fn el_nucleo_compartido_esta_una_sola_vez_en_cada_runtime() {
    let p = build("fn f(xs: List<Int>) -> Html { return `<p>x</p>`; }");
    let a = app("@client fn vista() -> String { return \"x\"; }");
    for (quien, texto) in [("runtime.ts", &p.runtime), ("client.js", &a.client_js)] {
        for n in COMPARTIDAS {
            // Sólo código: el núcleo ilustra la convención con una línea de
            // ejemplo dentro de un comentario, y eso no es una definición.
            let veces = texto
                .lines()
                .filter(|l| !l.trim_start().starts_with("//"))
                .filter(|l| l.contains(&format!("export function {n}")))
                .count();
            assert_eq!(
                veces, 1,
                "'{n}' aparece {veces} veces en {quien}, y tiene que aparecer 1"
            );
        }
    }
}

/// Y lo que va al navegador sigue siendo JavaScript plano. Las anotaciones del
/// núcleo viajan en `/*ts ... */` justamente para poder quitarlas aquí: si
/// llegaran al archivo que servimos, el navegador moriría en la primera línea
/// —no hay paso de compilación que las quite—. En runtime.ts es al revés:
/// llegan destapadas, y son las que `tsc --strict` comprueba.
#[test]
fn el_nucleo_llega_como_js_al_navegador_y_como_ts_a_node() {
    let a = app("@client fn vista() -> String { return \"x\"; }");
    for aguja in ["/*ts", ": unknown", ": string", ": number", "interface "] {
        assert!(
            !a.client_js.contains(aguja),
            "el cliente de navegador no puede llevar «{aguja}»:\n{}",
            a.client_js
        );
    }
    let p = build("fn f() -> Html { return `<p>x</p>`; }");
    assert!(!p.runtime.contains("/*ts"), "quedaron marcas sin destapar");
    assert!(
        p.runtime
            .contains("export function print(x: unknown): void"),
        "{}",
        p.runtime
    );
}
