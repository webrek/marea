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
    assert!(p.client.contains("const x = __memo(() => 1)"), "{}", p.client);
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
    let p = build("@client fn main() { print(1); }");
    assert!(p.runtime.contains("export async function __rpc"));
    assert!(p.runtime.contains("export function startServer"));
}

#[test]
fn reactivo_genera_signal_memo_y_effect() {
    let p = build(
        "@client fn main() { reactive mut n = 0; reactive doble = n * 2; effect { print(doble); } n = n + 1; }",
    );
    assert!(p.client.contains("const n = __signal(0)"), "{}", p.client);
    assert!(p.client.contains("const doble = __memo(() => (n.get() * 2))"), "{}", p.client);
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
    // JS '/' daría flotante; debe truncar para no romper el contrato Int.
    let p = build("fn d() -> Int { return 7 / 2; }");
    assert!(p.client.contains("Math.trunc"), "{}", p.client);
}

#[test]
fn variante_como_valor_es_etiqueta() {
    let p = build("@client fn f(n: Int) -> A | B { if n > 0 { return A; } return B; }");
    assert!(p.client.contains("return \"A\""), "{}", p.client);
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
    let p = build("@client fn main() { reactive mut n = 0; effect { let n = 99; print(n); } n = n + 1; }");
    assert!(p.client.contains("const n = 99"), "{}", p.client);
    assert!(p.client.contains("print(n)") && !p.client.contains("print(n.get())"), "{}", p.client);
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
    assert!(glue.contains("decodificarCadena(exports.vista())"), "glue: {glue}");
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
    assert!(!glue.contains("decodificarCadena(exports.vista())"), "glue: {glue}");
}

// --- app web (marea build-app): RPC + reactivo + DOM ---

fn app(src: &str) -> marea_codegen::AppProject {
    marea_codegen::emit_app(&parse(src).unwrap())
}

#[test]
fn app_reactiva_de_modulo_es_signal() {
    // Una `reactive mut` de nivel superior se vuelve un signal de módulo.
    let a = app("reactive mut posts = [];\n@client fn vista() -> String { let p = posts; return \"x\"; }");
    assert!(a.client_js.contains("const posts = __signal([]);"), "{}", a.client_js);
    // La vista lee el signal con .get().
    assert!(a.client_js.contains("posts.get()"), "{}", a.client_js);
}

#[test]
fn app_cliente_es_js_sin_tipos_ni_imports_node() {
    let a = app("@server fn feed() -> List<Int> { return todos(); }\n@client fn main() { feed(); }");
    // El cliente de navegador NO importa Node ni lleva anotaciones de tipo.
    assert!(!a.client_js.contains("node:http"), "{}", a.client_js);
    assert!(!a.client_js.contains(": number"), "{}", a.client_js);
    assert!(!a.client_js.contains(": string"), "{}", a.client_js);
    // El @server se vuelve un stub fetch al mismo origen.
    assert!(a.client_js.contains("__rpc(\"feed\""), "{}", a.client_js);
    assert!(a.client_js.contains("fetch(\"/__marea\""), "{}", a.client_js);
}

#[test]
fn app_arranca_main_y_monta_vista() {
    let a = app("@client fn vista() -> String { return \"hola\"; }\n@client fn main() { print(\"hi\"); }");
    assert!(a.client_js.contains("await main();"), "{}", a.client_js);
    assert!(a.client_js.contains("__mount(vista);"), "{}", a.client_js);
    // Expone las funciones en window.marea para los onclick del HTML.
    assert!(a.client_js.contains("globalThis.marea = {"), "{}", a.client_js);
    // index.html tiene el contenedor #app y carga client.js como módulo.
    assert!(a.index_html.contains("id=\"app\""), "{}", a.index_html);
    assert!(a.index_html.contains("src=\"./client.js\""), "{}", a.index_html);
}

#[test]
fn app_servidor_sirve_estaticos() {
    let a = app("@server fn feed() -> List<Int> { return todos(); }");
    // El runtime Node sirve estáticos (la app vive en el mismo origen que el RPC).
    assert!(a.runtime.contains("__serveStatic"), "{}", a.runtime);
    assert!(a.runtime.contains("MAREA_WEB_ROOT"), "{}", a.runtime);
    // El entry deja el servidor vivo y fija la raíz estática.
    assert!(a.serve.contains("startServer()"), "{}", a.serve);
    assert!(a.serve.contains("MAREA_WEB_ROOT"), "{}", a.serve);
}

#[test]
fn store_builtins_son_async_y_se_awaitan() {
    let p = build("@server fn g(x: Int) { guardar(x); } @server fn t() -> List<Int> { return todos(); }");
    // Con backends de BD, las operaciones del store son asíncronas (I/O).
    assert!(p.runtime.contains("export async function guardar"), "{}", p.runtime);
    assert!(p.runtime.contains("export async function todos"), "{}", p.runtime);
    // El call site las espera.
    assert!(p.server.contains("(await guardar(x))"), "{}", p.server);
    assert!(p.server.contains("(await todos())"), "{}", p.server);
}

#[test]
fn store_persiste_a_disco() {
    let p = build("@server fn g(x: Int) { guardar(x); }");
    // El runtime carga el store del archivo y lo reescribe en cada guardar.
    assert!(p.runtime.contains("readFileSync"), "{}", p.runtime);
    assert!(p.runtime.contains("writeFileSync"), "{}", p.runtime);
    assert!(p.runtime.contains("MAREA_STORE"), "{}", p.runtime);
}

#[test]
fn indexado_usa_bounds_check() {
    let p = build("@client fn f(xs: List) { let a = xs[0]; print(a); }");
    assert!(p.client.contains("__index(xs, 0)"), "{}", p.client);
    assert!(p.runtime.contains("export function __index"), "{}", p.runtime);
}

// --- endurecimiento de seguridad (auditoría 2026-06-18) ---

#[test]
fn handler_valida_aridad_de_args() {
    // M1: el wrapper RPC exige la aridad exacta antes de invocar la función,
    // para que un argumento faltante no se cuele como undefined.
    let p = build("@server fn pub(a: String, b: Int) {}");
    assert!(p.server.contains(r#"if (args.length !== 2) throw new Error("aridad inválida")"#), "{}", p.server);
}

#[test]
fn transporte_rpc_esta_endurecido() {
    // H1/L1/M2: bind a loopback, tope de cuerpo, tabla de handlers sin prototipo,
    // y errores genéricos al cliente (sin reflejar String(e) ni el nombre de fn).
    let p = build("@server fn f() {}");
    assert!(p.runtime.contains("MAREA_MAX_BODY"), "falta tope de cuerpo");
    assert!(p.runtime.contains("statusCode = 413"), "falta rechazo 413");
    assert!(p.runtime.contains("Object.create(null)"), "handlers deben ir sin prototipo");
    assert!(p.runtime.contains("listen(MAREA_PORT, MAREA_HOST"), "debe bindear host explícito");
    assert!(p.runtime.contains(r#"error: "error interno""#), "el error al cliente debe ser genérico");
    // No debe filtrar el error crudo ni hacer eco del nombre de función.
    assert!(!p.runtime.contains("error: String(e)"), "no debe reflejar String(e)");
    assert!(!p.runtime.contains("función desconocida: ${fn}"), "no debe hacer eco del fn");
}

#[test]
fn backends_sql_comillan_identificadores() {
    // L2: tabla/columnas se comillan por dialecto, así un campo con nombre de
    // palabra reservada (from, order…) no rompe el SQL generado.
    let p = build("type Fila = { from: String, order: Int };\nstore Fila;\n@server fn g() { guardar(Fila { from: \"a\", order: 1 }); }");
    assert!(p.runtime.contains("function __quoteId"), "{}", p.runtime);
    assert!(p.runtime.contains("function __idCol"), "{}", p.runtime);
    // sqlite/pg usan comilla doble; mysql usa backtick (constantes por backend).
    assert!(p.runtime.contains("const q = '\"'"), "sqlite/pg deben comillar con \"");
    assert!(p.runtime.contains("const q = \"`\""), "mysql debe comillar con backtick");
}

#[test]
fn persistencia_a_archivo_es_incremental_y_atomica() {
    // H2/L5: el backend de archivo usa un log append-only (O(1) por mutación) y
    // compacta atómicamente (temporal + rename). Sin reescritura completa por op.
    let p = build("store Int;\n@server fn g(x: Int) { guardar(x); }");
    assert!(p.runtime.contains("appendFileSync"), "debe escribir incremental (append)");
    assert!(p.runtime.contains("renameSync"), "la compactación debe ser atómica");
    assert!(p.runtime.contains(".tmp"), "debe compactar vía temporal");
}

#[test]
fn backends_son_incrementales_por_id() {
    // H2: la interfaz del backend es insert/update/remove por id, no saveAll;
    // las mutaciones tocan una sola fila.
    let p = build("store Int;\n@server fn g(x: Int) { guardar(x); }");
    assert!(p.runtime.contains("insert(id: number, item: unknown)"), "{}", p.runtime);
    assert!(p.runtime.contains("remove(id: number)"), "{}", p.runtime);
    assert!(!p.runtime.contains("saveAll"), "saveAll (reescritura total) debe haber desaparecido");
    // El esquema SQL gana una clave primaria interna.
    assert!(p.runtime.contains("INTEGER PRIMARY KEY"), "{}", p.runtime);
}

#[test]
fn placeholders_se_sustituyen_en_una_pasada() {
    // L3: un campo cuyo nombre coincide con un centinela del template no debe
    // corromper la sustitución del otro. Tras emitir no quedan centinelas crudos.
    let p = build("type T = { __MAREA_STORE_SCHEMA__: Int };\nstore T;\n@server fn g() { guardar(T { __MAREA_STORE_SCHEMA__: 1 }); }");
    assert!(!p.runtime.contains("__MAREA_STORE_DEFAULT__"), "centinela DEFAULT sin sustituir");
    // El único '__MAREA_STORE_SCHEMA__' admisible es el nombre de columna inyectado,
    // no un placeholder del template suelto en una posición de código.
    assert!(p.runtime.contains("const __STORE_SCHEMA: __Schema | null = {"), "el esquema debe quedar sustituido");
}

#[test]
fn store_inyecta_esquema_de_columnas() {
    let p = build("type Post = { texto: String, likes: Int };\nstore Post;\n@server fn g() { guardar(Post { texto: \"a\", likes: 0 }); }");
    // El esquema inyectado describe tabla + columnas tipadas para los backends SQL.
    assert!(p.runtime.contains("table: \"post\""), "{}", p.runtime);
    assert!(p.runtime.contains("{ name: \"texto\", kind: \"text\" }"), "{}", p.runtime);
    assert!(p.runtime.contains("{ name: \"likes\", kind: \"int\" }"), "{}", p.runtime);
    assert!(!p.runtime.contains("__MAREA_STORE_SCHEMA__"), "placeholder sin sustituir");
}

#[test]
fn store_escalar_usa_columna_doc() {
    let p = build("store Int;\n@server fn g(x: Int) { guardar(x); }");
    // Un store no-registro guarda el valor entero como una sola columna JSON.
    assert!(p.runtime.contains("{ name: \"__doc\", kind: \"json\" }"), "{}", p.runtime);
}

#[test]
fn sin_store_el_esquema_es_null() {
    let p = build("@client fn f() { print(\"hola\"); }");
    assert!(p.runtime.contains("const __STORE_SCHEMA: __Schema | null = null;"), "{}", p.runtime);
}

#[test]
fn store_tiene_backends_de_base_de_datos() {
    let p = build("store Int;\n@server fn g(x: Int) { guardar(x); }");
    // Los cinco backends conviven; el driver se elige con MAREA_DB.
    for marca in ["__sqliteBackend", "__postgresBackend", "__mysqlBackend", "__mongoBackend", "MAREA_DB"] {
        assert!(p.runtime.contains(marca), "falta {marca} en runtime");
    }
    // Los drivers externos se importan de forma perezosa (no rompen si no están).
    assert!(p.runtime.contains("await import(\"node:sqlite\")"), "{}", p.runtime);
    assert!(p.runtime.contains("await import(\"pg\")"), "{}", p.runtime);
}

#[test]
fn store_file_lleva_la_firma_del_esquema() {
    let p = build("type Post = { a: Int };\nstore Post;\n@server fn g() { guardar(Post { a: 1 }); }");
    // El archivo por defecto incluye nombre+campos para no colisionar entre apps.
    assert!(p.runtime.contains("marea-store.Post-a.log"), "{}", p.runtime);
    assert!(!p.runtime.contains("__MAREA_STORE_DEFAULT__"), "placeholder sin sustituir");
}
