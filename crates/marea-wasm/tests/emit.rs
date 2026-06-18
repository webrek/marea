//! Tests del backend WASM (generación de WAT).

use marea_syntax::parse;
use marea_wasm::emit_wat;

fn wat(src: &str) -> Result<String, String> {
    emit_wat(&parse(src).unwrap())
}

#[test]
fn funcion_se_exporta_con_params_i32() {
    let w = wat("fn add(a: Int, b: Int) -> Int { return a + b; }").unwrap();
    assert!(w.contains(r#"(func $add (export "add") (param $a i32) (param $b i32) (result i32)"#));
    assert!(w.contains("(i32.add (local.get $a) (local.get $b))"));
}

#[test]
fn if_como_sentencia_con_return() {
    let w = wat("fn f(n: Int) -> Int { if n < 2 { return n; } return 0; }").unwrap();
    assert!(w.contains("(if (i32.lt_s (local.get $n) (i32.const 2))"));
    assert!(w.contains("(then"));
}

#[test]
fn llamada_y_recursion() {
    let w = wat("fn fib(n: Int) -> Int { if n < 2 { return n; } return fib(n - 1); }").unwrap();
    assert!(w.contains("(call $fib (i32.sub (local.get $n) (i32.const 1)))"));
}

#[test]
fn let_se_vuelve_local() {
    let w = wat("fn f(a: Int) -> Int { let x = a + 1; return x; }").unwrap();
    assert!(w.contains("(local $x i32)"));
    assert!(w.contains("(local.set $x (i32.add (local.get $a) (i32.const 1)))"));
}

#[test]
fn bool_es_i32() {
    let w = wat("fn t() -> Bool { return true; }").unwrap();
    assert!(w.contains("(i32.const 1)"));
}

#[test]
fn modulo_y_operadores() {
    let w = wat("fn g(a: Int, b: Int) -> Int { return a % b; }").unwrap();
    assert!(w.contains("(i32.rem_s (local.get $a) (local.get $b))"));
}

// --- cadenas sobre memoria lineal ---

#[test]
fn cadenas_van_a_memoria_lineal() {
    let w = wat(r#"fn s() -> String { return concat("Hola, ", "Marea"); }"#).unwrap();
    // Se emite memoria, el builtin concat y el data section de los literales.
    assert!(w.contains(r#"(memory (export "memory") 1)"#));
    assert!(w.contains("(global $__heap"));
    assert!(w.contains(r#"(func $concat (export "concat")"#));
    assert!(w.contains("(data (i32.const 0)"));
    // El literal se referencia por su puntero (offset 0).
    assert!(w.contains("(call $concat (i32.const 0)"));
}

#[test]
fn sin_cadenas_no_se_emite_memoria() {
    // El slice numérico no debe arrastrar el runtime de memoria.
    let w = wat("fn add(a: Int, b: Int) -> Int { return a + b; }").unwrap();
    assert!(!w.contains("(memory"));
    assert!(!w.contains("$concat"));
}

// --- lo aún no soportado debe fallar con mensaje claro, no generar WAT roto ---

#[test]
fn flotantes_no_soportados_aun() {
    let err = wat("fn f() -> Int { let x = 1.5; return 1; }").unwrap_err();
    assert!(err.contains("flotantes"), "mensaje: {err}");
}

#[test]
fn tipo_desconocido_es_error() {
    let err = wat("fn f(u: User) -> Int { return 1; }").unwrap_err();
    assert!(err.contains("Int/Bool/String"), "mensaje: {err}");
}
