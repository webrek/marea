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
fn reactive_se_marca_para_v3() {
    let p = build("@client fn f() { reactive x = 1; print(x); }");
    assert!(p.client.contains("/* reactive"));
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
