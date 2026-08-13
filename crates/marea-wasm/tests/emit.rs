//! Tests del backend WASM (generación de WAT).

use marea_syntax::parse;
use marea_wasm::emit_wat;

fn wat(src: &str) -> Result<String, String> {
    emit_wat(&parse(src).unwrap())
}

#[test]
fn funcion_se_exporta_con_params_i32() {
    let w = wat("fn append(a: Int, b: Int) -> Int { return a + b; }").unwrap();
    assert!(w.contains(r#"(func $append (export "append") (param $a i32) (param $b i32) (result i32)"#));
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
    let w = wat("fn append(a: Int, b: Int) -> Int { return a + b; }").unwrap();
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

// --- registros (structs) sobre memoria lineal ---

#[test]
fn construir_registro_reserva_y_guarda_campos() {
    let w = wat(
        "type Punto = { x: Int, y: Int };\n\
         fn p() -> Punto { let p: Punto = Punto { x: 1, y: 2 }; return p; }",
    )
    .unwrap();
    // Reserva el bloque de 2 campos * 4 bytes.
    assert!(w.contains("(call $__alloc (i32.const 8))"), "wat: {w}");
    // Guarda x en offset 0 e y en offset 4.
    assert!(w.contains("(i32.store offset=0 (local.get $__rec0) (i32.const 1))"), "wat: {w}");
    assert!(w.contains("(i32.store offset=4 (local.get $__rec0) (i32.const 2))"), "wat: {w}");
}

#[test]
fn acceso_a_campo_es_i32_load() {
    let w = wat(
        "type Punto = { x: Int, y: Int };\n\
         fn lee(p: Punto) -> Int { return p.y; }",
    )
    .unwrap();
    // El campo y está en índice 1 -> offset 4.
    assert!(w.contains("(i32.load offset=4 (local.get $p))"), "wat: {w}");
}

#[test]
fn orden_de_declaracion_no_de_uso() {
    // El literal pone los campos invertidos (y antes que x); el layout sigue
    // el orden de DECLARACIÓN, así que x debe ir a offset 0 e y a offset 4.
    let w = wat(
        "type Punto = { x: Int, y: Int };\n\
         fn p() -> Punto { return Punto { y: 2, x: 1 }; }",
    )
    .unwrap();
    assert!(w.contains("(i32.store offset=0 (local.get $__rec0) (i32.const 1))"), "wat: {w}");
    assert!(w.contains("(i32.store offset=4 (local.get $__rec0) (i32.const 2))"), "wat: {w}");
}

#[test]
fn campo_string_convive_con_runtime_de_string() {
    let w = wat(
        "type Persona = { nombre: String, edad: Int };\n\
         fn nueva() -> Persona { return Persona { nombre: \"Ana\", edad: 30 }; }",
    )
    .unwrap();
    // Runtime de memoria presente.
    assert!(w.contains(r#"(memory (export "memory") 1)"#), "wat: {w}");
    // El campo nombre guarda el puntero al literal (offset 0 en data).
    assert!(w.contains("(i32.store offset=0 (local.get $__rec0) (i32.const 0))"), "wat: {w}");
    // El literal "Ana" quedó en el data section.
    assert!(w.contains("(data (i32.const 0)"), "wat: {w}");
}

#[test]
fn campo_inexistente_es_error() {
    let err = wat(
        "type Punto = { x: Int, y: Int };\n\
         fn p() -> Punto { return Punto { x: 1, y: 2, z: 3 }; }",
    )
    .unwrap_err();
    assert!(err.contains("no tiene un campo 'z'"), "mensaje: {err}");
}

#[test]
fn campo_faltante_es_error() {
    let err = wat(
        "type Punto = { x: Int, y: Int };\n\
         fn p() -> Punto { return Punto { x: 1 }; }",
    )
    .unwrap_err();
    assert!(err.contains("falta el campo 'y'"), "mensaje: {err}");
}

#[test]
fn campo_float_en_tipo_es_error() {
    let err = wat(
        "type Caja = { peso: Float };\n\
         fn c() -> Caja { return Caja { peso: 1 }; }",
    )
    .unwrap_err();
    assert!(err.contains("Float") || err.contains("flotantes"), "mensaje: {err}");
}

#[test]
fn miembro_sobre_tipo_desconocido_es_error() {
    let err = wat("fn f(a: Int) -> Int { return a.x; }").unwrap_err();
    assert!(
        err.contains("no se conoce el tipo") || err.contains("miembro"),
        "mensaje: {err}"
    );
}

#[test]
fn ensambla_con_wat2wasm() {
    use std::io::Write;
    use std::process::Command;

    // Solo corre si wat2wasm está disponible en el entorno.
    if Command::new("wat2wasm").arg("--version").output().is_err() {
        eprintln!("wat2wasm no disponible; se omite la verificación E2E");
        return;
    }

    let w = wat(
        "type Punto = { x: Int, y: Int };\n\
         fn p() -> Int { let p: Punto = Punto { y: 2, x: 1 }; return p.x; }",
    )
    .unwrap();

    let dir = std::env::temp_dir();
    let wat_path = dir.join("marea_struct_test.wat");
    let wasm_path = dir.join("marea_struct_test.wasm");
    {
        let mut f = std::fs::File::create(&wat_path).unwrap();
        f.write_all(w.as_bytes()).unwrap();
    }
    let out = Command::new("wat2wasm")
        .arg(&wat_path)
        .arg("-o")
        .arg(&wasm_path)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "wat2wasm rechazó el módulo:\nstderr: {}\nwat:\n{w}",
        String::from_utf8_lossy(&out.stderr)
    );
    let _ = std::fs::remove_file(&wat_path);
    let _ = std::fs::remove_file(&wasm_path);
}

// --- listas sobre memoria lineal ---

#[test]
fn lista_construye_con_longitud_y_elementos() {
    let w = wat("fn f() -> Int { let xs = [10, 20, 30]; return xs[1]; }").unwrap();
    // Reserva 4*(3+1) = 16 bytes.
    assert!(w.contains("(call $__alloc (i32.const 16))"), "wat: {w}");
    // Longitud en la palabra 0.
    assert!(w.contains("(i32.store offset=0 (local.get $__rec0) (i32.const 3))"), "wat: {w}");
    // Primer elemento en la palabra 1 (offset 4).
    assert!(w.contains("(i32.store offset=4 (local.get $__rec0) (i32.const 10))"), "wat: {w}");
}

#[test]
fn indexado_comprueba_el_rango() {
    let w = wat("fn f(xs: List) -> Int { return xs[2]; }").unwrap();
    // El indexado pasa por un helper que trapea fuera de rango. Antes sumaba el
    // índice al puntero sin más: un índice negativo leía HACIA ATRÁS en el heap
    // —xs[-1] devolvía la longitud, xs[-2] datos de la estructura anterior—, o
    // sea divulgación de memoria, y además difería del backend de TypeScript,
    // que lanza.
    assert!(w.contains("(call $__index (local.get $xs) (i32.const 2))"), "wat: {w}");
    assert!(w.contains("(func $__index"), "falta el helper: {w}");
    assert!(w.contains("i32.lt_s (local.get $i) (i32.const 0)"), "sin cota inferior: {w}");
    assert!(w.contains("unreachable"), "debe trapear fuera de rango: {w}");
}

// `&&` y `||` cortocircuitan como en TypeScript: emitirlos con i32.and/i32.or
// evaluaba siempre los dos lados, así que `i < len(xs) && xs[i] > 0` indexaba
// igualmente y un operando que trapea mataba el programa.
#[test]
fn los_operadores_logicos_cortocircuitan() {
    let w = wat("fn f(a: Bool, b: Bool) -> Bool { return a && b; }").unwrap();
    assert!(w.contains("(if (result i32)"), "sin cortocircuito: {w}");
    assert!(!w.contains("i32.and"), "no debe usar i32.and: {w}");
    let w = wat("fn f(a: Bool, b: Bool) -> Bool { return a || b; }").unwrap();
    assert!(w.contains("(if (result i32)"), "sin cortocircuito: {w}");
}

// La igualdad de cadenas compara CONTENIDO. Con i32.eq comparaba punteros, así
// que concat("a","") == "a" era falso en WASM y verdadero en TypeScript.
#[test]
fn la_igualdad_de_cadenas_compara_contenido() {
    let w = wat("fn f(s: String) -> Bool { return s == \"hola\"; }").unwrap();
    assert!(w.contains("call $__streq"), "wat: {w}");
    assert!(w.contains("(func $__streq"), "falta el helper: {w}");
    // Los enteros siguen comparándose con i32.eq.
    let w = wat("fn f(a: Int) -> Bool { return a == 1; }").unwrap();
    assert!(w.contains("i32.eq"), "wat: {w}");
    assert!(!w.contains("call $__streq"), "wat: {w}");
}

#[test]
fn indexar_lista_param_emite_memoria() {
    // Antes: i32.load sin (memory) declarada -> WAT inválido.
    let w = wat("fn cabeza(xs: List) -> Int { return xs[0]; }").unwrap();
    assert!(w.contains(r#"(memory (export "memory") 1)"#), "wat: {w}");
}

#[test]
fn int_fuera_de_rango_i32_es_error() {
    let err = wat("fn big() -> Int { return 5000000000; }").unwrap_err();
    assert!(err.contains("i32"), "mensaje: {err}");
}

#[test]
fn if_else_terminal_emite_unreachable() {
    // Ambas ramas con return: el validador WASM exige marcar la caída inalcanzable.
    let w = wat("fn abs(n: Int) -> Int { if n > 0 { return n; } else { return 0 - n; } }").unwrap();
    assert!(w.contains("(unreachable)"), "wat: {w}");
}

#[test]
fn len_es_load_de_la_longitud() {
    let w = wat("fn n(xs: List) -> Int { return len(xs); }").unwrap();
    // len(xs) = i32.load del puntero (la longitud vive en la palabra 0).
    assert!(w.contains("(i32.load (local.get $xs))"), "wat: {w}");
}

#[test]
fn fn_con_nombre_reservado_es_error() {
    let err = wat(r#"fn memory() -> Int { return 1; } fn s() -> String { return concat("a","b"); }"#).unwrap_err();
    assert!(err.contains("reservado"), "mensaje: {err}");
}

#[test]
fn estado_servidor_no_existe_en_wasm() {
    let err = wat("fn f() -> Int { return len(all()); }").unwrap_err();
    assert!(err.contains("runtime de TypeScript"), "mensaje: {err}");
}
