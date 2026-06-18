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
