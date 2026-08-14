//! Pruebas del verificador de tipos `marea-types`.
//!
//! Cubre: golden de los ejemplos reales, un caso negativo por cada código de
//! error, narrowing dentro de la rama de un `match`, clasificación de cruces de
//! frontera y acumulación de múltiples errores en un mismo archivo.

use marea_types::{check, check_with_boundaries};

/// Parsea y chequea, devolviendo los errores de tipo.
fn check_src(src: &str) -> Vec<marea_types::TypeError> {
    let module = marea_syntax::parse(src).expect("el fuente debe parsear");
    check(&module)
}

/// ¿Aparece este código de error en la lista?
fn has_code(errs: &[marea_types::TypeError], code: &str) -> bool {
    errs.iter().any(|e| e.code == code)
}

fn codes(errs: &[marea_types::TypeError]) -> Vec<String> {
    errs.iter().map(|e| e.code.clone()).collect()
}

// ============================ GOLDEN: ejemplos ============================

/// TODOS los ejemplos del repositorio deben tipar. Antes esta lista tenía cinco
/// nombres escritos a mano y los diez restantes no tenían red: uno de ellos
/// (`user.mar`) llegó a contener un patrón que generaba JavaScript inválido sin
/// que ningún test lo notara. Ahora se recorre el directorio, así que un ejemplo
/// nuevo entra en la red automáticamente.
#[test]
fn ejemplos_reales_tipan_sin_errores() {
    let dir = format!("{}/../../examples", env!("CARGO_MANIFEST_DIR"));
    let mut vistos = 0;
    for entrada in std::fs::read_dir(&dir).expect("no se pudo leer examples/") {
        let ruta = entrada.expect("entrada ilegible").path();
        if ruta.extension().and_then(|e| e.to_str()) != Some("mar") {
            continue;
        }
        let src = std::fs::read_to_string(&ruta)
            .unwrap_or_else(|_| panic!("no se pudo leer {}", ruta.display()));
        let errs = check_src(&src);
        assert!(
            errs.is_empty(),
            "el ejemplo '{}' debería tipar pero produjo: {:?}",
            ruta.display(),
            codes(&errs)
        );
        vistos += 1;
    }
    assert!(
        vistos >= 14,
        "se esperaban al menos 14 ejemplos, se vieron {vistos}"
    );
}

/// Los ejemplos de `examples/check_fail/` existen para FALLAR: cada uno ilustra
/// una regla del verificador. Si alguno dejara de dar error, la regla se habría
/// perdido sin que nada lo avisara.
#[test]
fn los_ejemplos_de_check_fail_fallan() {
    let dir = format!("{}/../../examples/check_fail", env!("CARGO_MANIFEST_DIR"));
    let mut vistos = 0;
    for entrada in std::fs::read_dir(&dir).expect("no se pudo leer examples/check_fail/") {
        let ruta = entrada.expect("entrada ilegible").path();
        if ruta.extension().and_then(|e| e.to_str()) != Some("mar") {
            continue;
        }
        let src = std::fs::read_to_string(&ruta).expect("no se pudo leer el ejemplo");
        let module = marea_syntax::parse(&src).expect("el fuente debe parsear");
        let errs = check(&module);
        assert!(
            !errs.is_empty(),
            "'{}' debería producir un error de tipos y no produjo ninguno",
            ruta.display()
        );
        vistos += 1;
    }
    assert!(
        vistos >= 3,
        "se esperaban al menos 3 casos negativos, se vieron {vistos}"
    );
}

/// La demo desplegada en `site/` también entra en la red: es el artefacto que
/// está en producción.
#[test]
fn la_demo_del_sitio_tipa() {
    let ruta = format!("{}/../../site/marea-demo.mar", env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(&ruta).expect("no se pudo leer site/marea-demo.mar");
    let errs = check_src(&src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// ============================ RESOLUCIÓN ============================

#[test]
fn e_unresolved_name() {
    let errs = check_src("fn f() { print(noExiste); }");
    assert!(has_code(&errs, "E_UNRESOLVED_NAME"), "{:?}", codes(&errs));
}

#[test]
fn e_duplicate_item_fn() {
    let errs = check_src("fn f() {} fn f() {}");
    assert!(has_code(&errs, "E_DUPLICATE_ITEM"), "{:?}", codes(&errs));
    // La nota apunta a la primera declaración.
    let e = errs.iter().find(|e| e.code == "E_DUPLICATE_ITEM").unwrap();
    assert!(!e.notes.is_empty(), "debe llevar nota al primero");
}

#[test]
fn e_duplicate_item_type() {
    let errs = check_src("type T = Int; type T = Bool;");
    assert!(has_code(&errs, "E_DUPLICATE_ITEM"), "{:?}", codes(&errs));
}

#[test]
fn e_duplicate_param() {
    let errs = check_src("fn f(a: Int, a: Int) -> Int { return a; }");
    assert!(has_code(&errs, "E_DUPLICATE_PARAM"), "{:?}", codes(&errs));
}

#[test]
fn e_duplicate_binding_mismo_scope() {
    let errs = check_src("fn f() { let x = 1; let x = 2; }");
    assert!(has_code(&errs, "E_DUPLICATE_BINDING"), "{:?}", codes(&errs));
}

#[test]
fn shadowing_en_scope_interno_si_permitido() {
    // Re-declarar en un bloque interno (dentro de un if) NO es error.
    let src = "fn f(b: Bool) { let x = 1; if b { let x = 2; print(x); } }";
    let errs = check_src(src);
    assert!(
        !has_code(&errs, "E_DUPLICATE_BINDING"),
        "el shadowing interno debe permitirse: {:?}",
        codes(&errs)
    );
}

#[test]
fn e_unknown_type() {
    let errs = check_src("fn f(x: Inexistente) { print(x); }");
    assert!(has_code(&errs, "E_UNKNOWN_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn e_cyclic_type() {
    let errs = check_src("type A = B; type B = A;");
    assert!(has_code(&errs, "E_CYCLIC_TYPE"), "{:?}", codes(&errs));
}

// Regresión C-1: un tipo registro *estructuralmente* recursivo (lista enlazada)
// es válido y NO debe ser un ciclo transparente. Antes desbordaba la pila al
// expandir el campo recursivo en `ty_from_syntax`; ahora la referencia queda
// opaca y se re-resuelve un nivel por vez.
#[test]
fn tipo_registro_recursivo_directo_no_crashea() {
    let errs = check_src(
        "type Nodo = { valor: Int, siguiente: Nodo };\n\
         fn cabeza(n: Nodo) -> Int { return n.valor; }",
    );
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

// Regresión C-1: recursión mutua entre registros. Antes desbordaba la pila en
// `is_subtype`, que alternaba desplegando A→B→A…; ahora el subtipado es
// coinductivo sobre los nombres en despliegue.
#[test]
fn tipos_registro_mutuamente_recursivos_no_crashean() {
    let errs = check_src(
        "type A = { x: B };\n\
         type B = { y: A };\n\
         fn f(a: A) -> B { return a.x; }",
    );
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

// Regresión C-1: un `store` de tipo recursivo también entraba por la misma vía.
#[test]
fn store_recursivo_no_crashea() {
    let errs = check_src(
        "type Nodo = { v: Int, sig: Nodo };\n\
         store almacen: Nodo;\n\
         @server fn poner(n: Nodo) { save(almacen, n); }",
    );
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

// ============================ TIPOS ============================

#[test]
fn e_arith_type_mezcla() {
    let errs = check_src("fn f() -> Int { return 1 + true; }");
    assert!(has_code(&errs, "E_ARITH_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn e_cond_not_bool() {
    let errs = check_src("fn f() { if 3 { print(1); } }");
    assert!(has_code(&errs, "E_COND_NOT_BOOL"), "{:?}", codes(&errs));
}

#[test]
fn e_let_type_mismatch() {
    let errs = check_src("fn f() { let x: Int = \"hola\"; print(x); }");
    assert!(has_code(&errs, "E_LET_TYPE_MISMATCH"), "{:?}", codes(&errs));
}

#[test]
fn e_return_type_mismatch() {
    let errs = check_src("fn f() -> Int { return \"x\"; }");
    assert!(
        has_code(&errs, "E_RETURN_TYPE_MISMATCH"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_missing_return() {
    let errs = check_src("fn f() -> Int { let x = 1; }");
    assert!(has_code(&errs, "E_MISSING_RETURN"), "{:?}", codes(&errs));
}

#[test]
fn retorno_en_todas_las_ramas_no_es_error() {
    let src = "fn f(b: Bool) -> Int { if b { return 1; } else { return 2; } }";
    let errs = check_src(src);
    assert!(
        !has_code(&errs, "E_MISSING_RETURN"),
        "if/else con return en ambas ramas termina: {:?}",
        codes(&errs)
    );
}

#[test]
fn e_not_callable() {
    let errs = check_src("fn f() { let x = 1; x(); }");
    assert!(has_code(&errs, "E_NOT_CALLABLE"), "{:?}", codes(&errs));
}

#[test]
fn e_arity() {
    let errs = check_src("fn g(a: Int) -> Int { return a; } fn f() { g(1, 2); }");
    assert!(has_code(&errs, "E_ARITY"), "{:?}", codes(&errs));
}

#[test]
fn e_arg_type_escalar() {
    let errs = check_src("fn g(a: Int) -> Int { return a; } fn f() { g(true); }");
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

// ============================ MEMBER ============================

#[test]
fn e_no_field_en_escalar() {
    let errs = check_src("fn f() { let x = 1; print(x.campo); }");
    assert!(has_code(&errs, "E_NO_FIELD"), "{:?}", codes(&errs));
}

#[test]
fn e_field_on_union_sin_match() {
    // u: User | NotFound; render(u.nombre) sin haber hecho match.
    let src = r#"
        type User = Record;
        @server
        fn getUser() -> User | NotFound { return NotFound; }
        @client
        fn perfil() {
            let u = getUser();
            render(u.nombre);
        }
    "#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_FIELD_ON_UNION"), "{:?}", codes(&errs));
}

// ============================ UBICACIÓN / FRONTERA ============================

#[test]
fn e_call_client_from_server() {
    let src = r#"
        @client
        fn ui() { print("hola"); }
        @server
        fn handler() { ui(); }
    "#;
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_CALL_CLIENT_FROM_SERVER"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn cruce_de_frontera_valido_se_registra() {
    // @client llamando @server: cruce válido, sin error, y aparece en la lista.
    let src = r#"
        @server
        fn saludar(nombre: String) -> String { return concat("Hola ", nombre); }
        @client
        fn main() { let m = saludar("Marea"); print(m); }
    "#;
    let module = marea_syntax::parse(src).unwrap();
    let (errs, crossings) = check_with_boundaries(&module);
    assert!(errs.is_empty(), "no debe haber errores: {:?}", codes(&errs));
    assert_eq!(crossings.len(), 1, "un solo cruce");
    assert_eq!(crossings[0].callee, "saludar");
    assert_eq!(crossings[0].from, Some(marea_syntax::ast::Location::Client));
    assert_eq!(crossings[0].to, Some(marea_syntax::ast::Location::Server));
}

#[test]
fn e_boundary_not_serializable() {
    // Un parámetro de tipo función no es serializable al cruzar la frontera.
    // Construimos el caso con un parámetro Record que contiene una función no
    // es posible en la sintaxis; en su lugar probamos vía retorno de función.
    // Aquí: @client llama @server cuyo parámetro es un alias a función — no
    // expresable; usamos una unión con variante no serializable no aplica.
    // Caso real soportado: retorno de función. Validamos vía un parámetro
    // que el verificador trate como Fn no es expresable por sintaxis, así que
    // verificamos que tipos escalares/registro SÍ pasan (negativo controlado).
    let src = r#"
        @server
        fn ok(n: Int) -> String { return concat("", "x"); }
        @client
        fn main() { let r = ok(1); print(r); }
    "#;
    let errs = check_src(src);
    assert!(
        !has_code(&errs, "E_BOUNDARY_NOT_SERIALIZABLE"),
        "Int/String son serializables: {:?}",
        codes(&errs)
    );
}

// ============================ UNIÓN + MATCH (corazón) ============================

#[test]
fn e_non_exhaustive_match_falta_variante() {
    // Unión de tres variantes, el match sólo cubre dos y sin comodín.
    let src = r#"
        type User = Record;
        @server
        fn getUser() -> User | NotFound | Error { return NotFound; }
        @client
        fn perfil() {
            let u = getUser();
            match u {
                User => render("ok"),
                NotFound => render("no"),
            }
        }
    "#;
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_NON_EXHAUSTIVE_MATCH"),
        "{:?}",
        codes(&errs)
    );
    // El mensaje nombra la variante faltante.
    let e = errs
        .iter()
        .find(|e| e.code == "E_NON_EXHAUSTIVE_MATCH")
        .unwrap();
    assert!(e.message.contains("Error"), "mensaje: {}", e.message);
}

#[test]
fn e_unknown_variant() {
    let src = r#"
        type User = Record;
        @server
        fn getUser() -> User | NotFound { return NotFound; }
        @client
        fn perfil() {
            let u = getUser();
            match u {
                Fantasma => render("?"),
                _ => render("ok"),
            }
        }
    "#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_UNKNOWN_VARIANT"), "{:?}", codes(&errs));
}

#[test]
fn narrowing_ok_dentro_de_la_rama() {
    // Dentro de la rama de la variante User, 'u.nombre' SÍ vale (sin error).
    let src = r#"
        type User = Record;
        @server
        fn getUser() -> User | NotFound { return NotFound; }
        @client
        fn perfil() {
            let u = getUser();
            match u {
                NotFound => print("no encontrado"),
                _ => print(u.nombre),
            }
        }
    "#;
    let errs = check_src(src);
    assert!(
        errs.is_empty(),
        "el narrowing en la rama _ debe permitir u.nombre: {:?}",
        codes(&errs)
    );
}

#[test]
fn e_arg_type_pasando_union_donde_se_espera_user() {
    // Pasar User | NotFound donde se espera User concreto.
    let src = r#"
        type User = { nombre: String };
        @server
        fn getUser() -> User | NotFound { return NotFound; }
        fn saluda(u: User) -> String { return u.nombre; }
        @client
        fn perfil() {
            let u = getUser();
            let s = saluda(u);
            print(s);
        }
    "#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

// ============================ STRUCTS ============================

#[test]
fn record_literal_correcto_no_es_error() {
    let src = r#"
        type Punto = { x: Int, y: Int };
        fn f() { let p = Punto { x: 1, y: 2 }; print(p.x); }
    "#;
    let errs = check_src(src);
    assert!(
        errs.is_empty(),
        "el literal de registro válido no debe fallar: {:?}",
        codes(&errs)
    );
}

#[test]
fn record_literal_campo_inexistente() {
    let src = r#"
        type Punto = { x: Int, y: Int };
        fn f() { let p = Punto { x: 1, y: 2, z: 3 }; print(p.x); }
    "#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_NO_FIELD"), "{:?}", codes(&errs));
}

#[test]
fn record_literal_campo_faltante_y_tipo_malo() {
    // Falta 'y' (E_ARG_TYPE de campo faltante) y 'x' tiene tipo equivocado.
    let src = r#"
        type Punto = { x: Int, y: Int };
        fn f() { let p = Punto { x: "no" }; print(p.x); }
    "#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn record_literal_tipo_no_registro() {
    let src = r#"
        type T = Int;
        fn f() { let p = T { x: 1 }; print(1); }
    "#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_UNKNOWN_TYPE"), "{:?}", codes(&errs));
}

// ============================ ACUMULACIÓN ============================

#[test]
fn acumula_multiples_errores_en_un_archivo() {
    // Tres errores distintos en un mismo módulo; el verificador no aborta.
    let src = r#"
        fn f() -> Int {
            let x = noExiste;
            if 5 { print(1); }
            return true;
        }
    "#;
    let errs = check_src(src);
    assert!(
        errs.len() >= 3,
        "esperaba >=3 errores, hubo {:?}",
        codes(&errs)
    );
    assert!(has_code(&errs, "E_UNRESOLVED_NAME"), "{:?}", codes(&errs));
    assert!(has_code(&errs, "E_COND_NOT_BOOL"), "{:?}", codes(&errs));
    assert!(
        has_code(&errs, "E_RETURN_TYPE_MISMATCH"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn render_muestra_codigo_linea_y_cursor() {
    let errs = check_src("fn f() { print(noExiste); }");
    let e = &errs[0];
    let salida = e.render("fn f() { print(noExiste); }");
    assert!(salida.contains("E_UNRESOLVED_NAME"), "{salida}");
    assert!(salida.contains("línea"), "{salida}");
    assert!(salida.contains('^'), "{salida}");
}

// ============================ LISTAS ============================

#[test]
fn lista_indexada_por_int_tipa() {
    let errs = check_src("@client fn f() { let xs = [1, 2, 3]; let a: Int = xs[0]; print(a); }");
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn indice_no_int_es_error() {
    let errs = check_src("@client fn f() { let xs = [1, 2, 3]; let a = xs[true]; print(a); }");
    assert!(
        has_code(&errs, "E_INDEX_NOT_INT"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn indexar_un_no_lista_es_error() {
    let errs = check_src("@client fn f() { let n = 5; let a = n[0]; print(a); }");
    assert!(
        has_code(&errs, "E_INDEX_NOT_LIST"),
        "códigos: {:?}",
        codes(&errs)
    );
}

// ============================ REGRESIÓN (bug hunt) ============================

#[test]
fn alias_ciclico_no_paniquea() {
    // Antes: stack overflow. Ahora: reporta E_CYCLIC_TYPE sin crashear.
    let errs = check_src("type A = B;\ntype B = A;\nfn f(p: A) {}");
    assert!(
        has_code(&errs, "E_CYCLIC_TYPE"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn alias_autoreferente_no_paniquea() {
    let errs = check_src("type A = A;\nfn g(p: A) {}");
    assert!(has_code(&errs, "E_CYCLIC_TYPE"));
}

#[test]
fn reasignar_inmutable_es_error() {
    let errs = check_src("@client fn f() { let x = 1; x = 2; print(x); }");
    assert!(
        has_code(&errs, "E_ASSIGN_IMMUTABLE"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn reasignar_mut_es_valido() {
    let errs = check_src("@client fn f() { let mut x = 1; x = 2; print(x); }");
    assert!(
        !has_code(&errs, "E_ASSIGN_IMMUTABLE"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn edge_llamando_client_es_error() {
    let errs = check_src("@client fn c() {}\n@edge fn e() { c(); }");
    assert!(
        has_code(&errs, "E_CALL_CLIENT_FROM_SERVER"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn lista_vacia_es_subtipo_de_list() {
    let errs = check_src("@client fn f() { let xs: List<Int> = []; print(xs); }");
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn variante_como_valor_tipa() {
    // 'errores como valores': una variante Mayúscula es valor de su unión.
    let errs = check_src("@client fn f(n: Int) -> A | B { if n > 0 { return A; } return B; }");
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn ident_minuscula_inexistente_sigue_siendo_error() {
    let errs = check_src("@client fn f() { print(noExiste); }");
    assert!(
        has_code(&errs, "E_UNRESOLVED_NAME"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn match_como_expresion_infiere_tipo() {
    // Antes: el match valía Unit -> E_RETURN_TYPE_MISMATCH. Ahora infiere String.
    let errs = check_src(
        "@client fn f(n: Int) -> String { return match n { 0 => \"cero\", _ => \"otro\" }; }",
    );
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn redefinir_builtin_es_error() {
    let errs = check_src("@client fn print(x: Int) { return; }");
    assert!(
        has_code(&errs, "E_REDEFINE_BUILTIN"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn len_de_lista_es_int() {
    let errs = check_src("@client fn f() { let xs = [1, 2, 3]; let n: Int = len(xs); print(n); }");
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn lista_heterogenea_es_error() {
    let errs = check_src(r#"fn f() -> Int { let xs = ["n", 99]; return len(xs[0]); }"#);
    assert!(
        has_code(&errs, "E_LIST_HETEROGENEOUS"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn store_del_servidor_tipa() {
    let errs = check_src(
        "type P = { t: String };\n\
         store almacen: P;\n\
         @server fn pub2(t: String) { save(almacen, P { t: t }); }\n\
         @server fn feed() -> List<P> { return all(almacen); }",
    );
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn estado_fuera_de_server_es_error() {
    // 'all(almacen)'/'save(almacen, )' desde @client tocarían el store del proceso
    // equivocado: el typechecker lo rechaza.
    let errs = check_src("@client fn main() { let d = all(almacen); print(len(d)); }");
    assert!(
        has_code(&errs, "E_STATE_OFF_SERVER"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn estado_en_server_es_valido() {
    let errs = check_src("type P = { t: String };\n@server fn s() -> List<P> { save(almacen, P { t: \"a\" }); return all(almacen); }");
    assert!(
        !has_code(&errs, "E_STATE_OFF_SERVER"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn store_tipado_cierra_el_lavado_de_tipos() {
    // guardar un Int cuando el store es Post -> error (antes 'lavaba' tipos).
    let errs = check_src("type Post = { a: String };\nstore almacen: Post;\n@server fn m() -> List<Post> { save(almacen, 99); return all(almacen); }");
    assert!(has_code(&errs, "E_ARG_TYPE"), "códigos: {:?}", codes(&errs));
}

#[test]
fn guardar_sin_store_declarado_es_error() {
    // Con almacenes con nombre, usar uno no declarado es un nombre sin resolver:
    // más preciso que el antiguo "no hay store".
    let errs = check_src("@server fn f() -> List<Int> { save(almacen, 1); return all(almacen); }");
    assert!(
        has_code(&errs, "E_UNRESOLVED_NAME"),
        "códigos: {:?}",
        codes(&errs)
    );
}

// Pasar algo que no es un almacén donde va uno.
#[test]
fn el_primer_argumento_debe_ser_un_almacen() {
    let errs = check_src("@server fn f(n: Int) { save(n, 1); }");
    assert!(has_code(&errs, "E_NO_STORE"), "códigos: {:?}", codes(&errs));
}

// Varios almacenes en el mismo módulo, cada uno con su tipo.
#[test]
fn varios_almacenes_conviven() {
    let errs = check_src(
        "type P = { a: Int };\ntype O = { b: String };\n\
         store productos: P;\nstore ordenes: O;\n\
         @server fn f() { save(productos, P { a: 1 }); save(ordenes, O { b: \"x\" }); }\n\
         @server fn g() -> List<O> { return all(ordenes); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Y no se pueden confundir entre sí.
#[test]
fn no_se_puede_guardar_en_el_almacen_equivocado() {
    let errs = check_src(
        "type P = { a: Int };\ntype O = { b: String };\n\
         store productos: P;\nstore ordenes: O;\n\
         @server fn f() { save(ordenes, P { a: 1 }); }",
    );
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn dos_almacenes_con_el_mismo_nombre_es_error() {
    let errs = check_src("type P = { a: Int };\nstore x: P;\nstore x: P;");
    assert!(has_code(&errs, "E_DUPLICATE_STORE"), "{:?}", codes(&errs));
}

#[test]
fn store_tipado_correcto_no_es_error() {
    let errs = check_src("type Post = { a: String };\nstore almacen: Post;\n@server fn pub(a: String) { save(almacen, Post { a: a }); }\n@server fn feed() -> List<Post> { return all(almacen); }");
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn actualizar_y_borrar_tipados() {
    // update(i, x): i Int, x del tipo del store; remove(almacen, i): i Int.
    let ok = check_src("type P = { a: Int };\nstore almacen: P;\n@server fn f() { update(almacen, 0, P { a: 1 }); remove(almacen, 1); }");
    assert!(ok.is_empty(), "no debería haber errores: {:?}", codes(&ok));
    // Valor de tipo equivocado en actualizar.
    let bad = check_src(
        "type P = { a: Int };\nstore almacen: P;\n@server fn f() { update(almacen, 0, 99); }",
    );
    assert!(has_code(&bad, "E_ARG_TYPE"), "{:?}", codes(&bad));
    // Índice no-Int en borrar.
    let bad2 = check_src(
        "type P = { a: Int };\nstore almacen: P;\n@server fn f() { remove(almacen, \"x\"); }",
    );
    assert!(has_code(&bad2, "E_ARG_TYPE"), "{:?}", codes(&bad2));
}

#[test]
fn actualizar_fuera_de_server_es_error() {
    let errs = check_src("type P = { a: Int };\nstore almacen: P;\n@client fn f() { update(almacen, 0, P { a: 1 }); }");
    assert!(has_code(&errs, "E_STATE_OFF_SERVER"), "{:?}", codes(&errs));
}

#[test]
fn atexto_es_string() {
    let errs = check_src("@client fn f() { let s: String = text(42); print(s); }");
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn atexto_de_no_escalar_es_error() {
    let errs =
        check_src("type P = { a: Int };\n@client fn f() { let s = text(P { a: 1 }); print(s); }");
    assert!(has_code(&errs, "E_ARG_TYPE"), "códigos: {:?}", codes(&errs));
}

// ===================== estado reactivo de nivel superior =====================

#[test]
fn reactiva_de_modulo_se_resuelve_en_funciones() {
    // Una `reactive mut` de nivel superior es visible desde las funciones y se
    // le puede reasignar (es mutable).
    let errs = check_src(
        "reactive mut n = 0;\n\
         @client fn leer() -> Int { return n; }\n\
         @client fn subir() { n = n + 1; }",
    );
    assert!(
        errs.is_empty(),
        "no debería haber errores: {:?}",
        codes(&errs)
    );
}

#[test]
fn reactiva_de_modulo_derivada_es_inmutable() {
    // `reactive` (sin mut) de módulo es una derivada de solo lectura.
    let errs = check_src("reactive base = 10;\n@client fn f() { base = 1; }");
    assert!(
        has_code(&errs, "E_ASSIGN_IMMUTABLE"),
        "códigos: {:?}",
        codes(&errs)
    );
}

#[test]
fn variable_inexistente_sigue_siendo_error() {
    // Sin declaración de módulo, el nombre sigue sin resolverse.
    let errs = check_src("@client fn f() -> Int { return fantasma; }");
    assert!(
        has_code(&errs, "E_UNRESOLVED_NAME"),
        "códigos: {:?}",
        codes(&errs)
    );
}

// A-4: un `let` de primer nivel que redeclara un parámetro generaba
// `function f(x) { const x = 2; }` → SyntaxError en JS (y dos locales con el
// mismo nombre en WASM). El cuerpo comparte scope con los parámetros.
#[test]
fn let_que_redeclara_un_parametro_es_error() {
    let errs = check_src("@client fn f(x: Int) -> Int { let x = 2; return x; }");
    assert!(has_code(&errs, "E_DUPLICATE_BINDING"), "{:?}", codes(&errs));
}

// Pero el shadowing en un bloque ANIDADO sigue siendo legal, igual que en JS.
#[test]
fn shadowing_de_parametro_en_bloque_anidado_es_valido() {
    let errs =
        check_src("@client fn f(x: Int) -> Int { if true { let x = 2; print(x); } return x; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// El builtin de escapado de HTML existe y tipa como String.
#[test]
fn escapar_es_un_builtin() {
    let errs = check_src("@client fn f(s: String) -> String { return escape(s); }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// A-2b: una global `reactive` es estado de UI y solo existe en el bundle del
// cliente. Leerla desde @server compilaba a un ReferenceError en cada RPC;
// ahora es un error de ubicación, simétrico a E_STATE_OFF_SERVER.
#[test]
fn e_reactive_off_client() {
    let errs =
        check_src("reactive mut contador = 0;\n@server fn leer() -> Int { return contador; }");
    assert!(
        has_code(&errs, "E_REACTIVE_OFF_CLIENT"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn leer_una_reactiva_desde_client_es_valido() {
    let errs =
        check_src("reactive mut contador = 0;\n@client fn leer() -> Int { return contador; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Una global NO reactiva sí es visible desde el servidor (es una constante).
#[test]
fn una_global_no_reactiva_si_es_visible_desde_server() {
    let errs = check_src("let saludo = \"hola\";\n@server fn dime() -> String { return saludo; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Fuga 1 de la auditoría: el patrón insignia `reactive x = llamadaRemota()` se
// compilaba a `__memo(() => (await f()))` —await en una arrow no-async, o sea
// un SyntaxError que impedía cargar el módulo entero—. Ahora es un error claro
// del verificador en vez de código generado inválido.
// `reactive x = llamadaRemota()` es ahora un RECURSO: la composición de las dos
// fronteras. Antes se compilaba a `__memo(() => (await f()))` —un await en una
// arrow no-async— y se acabó prohibiendo; ahora arranca en `Loading` y se
// resuelve solo, y el tipo obliga a cubrir los tres estados.
#[test]
fn un_reactive_con_llamada_es_un_recurso() {
    let errs = check_src(
        "type User = { nombre: String };\n\
         @server fn getUser(id: Int) -> User | NotFound { return NotFound; }\n\
         @client fn perfil(id: Int) -> Html {\n\
           reactive u = getUser(id);\n\
           return match u {\n\
             Loading => \"cargando\",\n\
             Failed => \"error\",\n\
             NotFound => \"no existe\",\n\
             otro => escape(otro.nombre),\n\
           };\n\
         }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Y el tipo del recurso obliga de verdad: si no se cubren `Loading` y `Failed`,
// lo que queda en el comodín sigue siendo una unión opaca —`Loading | User |
// Failed`— y no se le puede leer un campo. La garantía no es un aviso: es que el
// programa incompleto no compila.
#[test]
fn el_recurso_obliga_a_cubrir_cargando_y_fallo() {
    let errs = check_src(
        "type User = { nombre: String };\n\
         @server fn getUser(id: Int) -> User | NotFound { return NotFound; }\n\
         @client fn perfil(id: Int) -> Html {\n\
           reactive u = getUser(id);\n\
           return match u { NotFound => \"no\", otro => escape(otro.nombre) };\n\
         }",
    );
    assert!(has_code(&errs, "E_FIELD_ON_UNION"), "{:?}", codes(&errs));
}

// Una global de módulo se evalúa al importar, antes de que exista el servidor:
// el RPC fallaba con ECONNREFUSED nada más cargar.
#[test]
fn e_boundary_in_init_global() {
    let errs =
        check_src("@server fn suma(a: Int, b: Int) -> Int { return a + b; }\nlet x = suma(1, 2);");
    assert!(has_code(&errs, "E_BOUNDARY_IN_INIT"), "{:?}", codes(&errs));
}

// El patrón correcto (cruzar dentro del cuerpo) sigue siendo válido.
#[test]
fn cruzar_la_frontera_en_el_cuerpo_es_valido() {
    let errs = check_src(
        "@server fn getUser(id: Int) -> Int { return 1; }\n\
         @client fn perfil(id: Int) { let u = getUser(id); print(u); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Una reactiva local que NO cruza la red sigue siendo válida.
#[test]
fn reactive_sin_cruce_de_frontera_es_valida() {
    let errs =
        check_src("@client fn f() { reactive mut n = 0; reactive doble = n * 2; print(doble); }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Una global no puede llamarse como un builtin: el bundle importa los builtins
// del runtime y el `const` generado los redeclaraba (SyntaxError al cargar).
#[test]
fn e_redefine_builtin_en_global() {
    let errs = check_src("let render = 1;\n@client fn m() { print(render); }");
    assert!(has_code(&errs, "E_REDEFINE_BUILTIN"), "{:?}", codes(&errs));
}

// --- regresiones de la segunda ronda de revisión ---

// El subtipado coinductivo usaba un conjunto de NOMBRES, así que al reencontrar
// un alias aceptaba contra cualquier cosa: el mismo par se rechazaba a
// profundidad 3 y se aceptaba a 4. Ahora la clave es el par (sub, sup).
#[test]
fn el_subtipado_recursivo_no_depende_de_la_profundidad() {
    let p3 = check_src(
        "type A = { x: A };\n\
         fn f(a: A) -> Int { let t: { x: { x: { x: Int } } } = a; return 1; }",
    );
    let p4 = check_src(
        "type A = { x: A };\n\
         fn f(a: A) -> Int { let t: { x: { x: { x: { x: Int } } } } = a; return 1; }",
    );
    assert!(!p3.is_empty(), "profundidad 3 debe rechazarse");
    assert!(
        !p4.is_empty(),
        "profundidad 4 debe rechazarse igual que la 3"
    );
}

// Un registro estructural vale donde se espera una unión que lo contiene: es el
// patrón de devolver un elemento del store desde una función que puede fallar.
#[test]
fn un_registro_es_subtipo_de_la_union_que_lo_contiene() {
    let errs = check_src(
        "type User = { nombre: String };\n\
         store almacen: User;\n\
         @server fn buscar(i: Int) -> User | NotFound {\n\
             let us = all(almacen);\n\
             if i < len(us) { return us[i]; }\n\
             return NotFound;\n\
         }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// E_BOUNDARY_IN_INIT solo miraba cruces de red, pero CUALQUIER función del
// usuario se compila a async: `reactive t = doble(n)` daba el mismo await en
// arrow no-async.
// Una llamada local en un inicializador reactivo también es un recurso: el
// codegen la emite con `await` igual, así que un memo síncrono no la aguanta.
#[test]
fn una_llamada_local_en_un_init_reactive_tambien_es_recurso() {
    let errs = check_src(
        "fn doble(n: Int) -> Int { return n * 2; }\n\
         @client fn main() -> Int {\n\
           reactive mut n = 0;\n\
           reactive t = doble(n);\n\
           return match t { Loading => 0, Failed => 0, otro => otro };\n\
         }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// El inicializador de una global NO reactiva sigue sin poder cruzar: se evalúa
// al importar y no tiene dónde esperar.
#[test]
fn una_global_no_reactiva_sigue_sin_poder_llamar() {
    let errs =
        check_src("@server fn suma(a: Int, b: Int) -> Int { return a + b; }\nlet x = suma(1, 2);");
    assert!(has_code(&errs, "E_BOUNDARY_IN_INIT"), "{:?}", codes(&errs));
}

// Los builtins síncronos sí pueden usarse ahí (no generan await).
#[test]
fn los_builtins_sincronos_si_valen_en_un_init_reactive() {
    let errs = check_src(
        "@client fn main() { reactive mut n = 0; reactive t = concat(\"n=\", text(n)); print(t); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Escribir una reactiva tiene la misma restricción de ubicación que leerla.
#[test]
fn escribir_una_reactive_desde_server_es_error() {
    let errs = check_src("reactive mut n = 0;\n@server fn poner() { n = 1; }");
    assert!(
        has_code(&errs, "E_REACTIVE_OFF_CLIENT"),
        "{:?}",
        codes(&errs)
    );
}

// Una función sin anotación se emite también en el servidor, donde el estado
// reactivo no existe.
#[test]
fn usar_una_reactive_desde_una_fn_sin_anotacion_es_error() {
    let errs = check_src("reactive mut posts = [];\nfn cuantos() -> Int { return len(posts); }");
    assert!(
        has_code(&errs, "E_REACTIVE_OFF_CLIENT"),
        "{:?}",
        codes(&errs)
    );
}

// Una global no puede chocar con una función ni con un identificador que el
// bundle importa del runtime: ambos producen archivos que no cargan.
#[test]
fn una_global_no_puede_chocar_con_una_funcion() {
    let errs = check_src("fn saluda() -> Int { return 1; }\nlet saluda = 2;");
    assert!(has_code(&errs, "E_DUPLICATE_ITEM"), "{:?}", codes(&errs));
}

#[test]
fn una_global_no_puede_llamarse_como_un_interno_del_runtime() {
    let errs = check_src("let __rpc = 1;\n@client fn m() { print(__rpc); }");
    assert!(has_code(&errs, "E_REDEFINE_BUILTIN"), "{:?}", codes(&errs));
}

// El codegen descarta las ramas posteriores a un atrapa-todo; borrar código en
// silencio es peor que avisar.
#[test]
fn e_unreachable_arm() {
    let errs = check_src(
        "@client fn f(x: Int) { match x { 1 => print(\"uno\"), _ => print(\"otro\"), 2 => print(\"dos\") } }",
    );
    assert!(has_code(&errs, "E_UNREACHABLE_ARM"), "{:?}", codes(&errs));
}

// `reactive` derivada es de solo lectura, igual que las globales: antes el
// navegador lanzaba y Node lo ignoraba en silencio para el mismo .mar.
#[test]
fn una_reactive_derivada_local_es_inmutable() {
    let errs = check_src(
        "@client fn f() { reactive mut n = 1; reactive doble = n * 2; doble = 99; print(doble); }",
    );
    assert!(has_code(&errs, "E_ASSIGN_IMMUTABLE"), "{:?}", codes(&errs));
}

// --- el tipo Html: el escapado deja de ser opcional ---

// Un dato del store incrustado en el DOM sin escapar es ahora un error de
// compilación, no un XSS que hay que acordarse de evitar.
#[test]
fn un_dato_sin_escapar_no_llega_al_dom() {
    let errs = check_src(
        "type Post = { texto: String };\n\
         @client fn vista(p: Post) { render(concat(\"<li>\", p.texto)); }",
    );
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn el_mismo_dato_escapado_si_llega() {
    let errs = check_src(
        "type Post = { texto: String };\n\
         @client fn vista(p: Post) { render(concat(\"<li>\", escape(p.texto))); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Un literal del propio fuente lo escribió el programador: es de confianza y no
// necesita conversión, ni como argumento ni como retorno.
#[test]
fn los_literales_del_fuente_valen_como_html() {
    let errs = check_src("@client fn f() { render(\"no existe\"); }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
    let errs = check_src("fn f() -> Html { return \"\"; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// La confianza explícita existe, pero se ve en el fuente (y en una revisión).
#[test]
fn html_marca_una_cadena_como_segura() {
    let errs = check_src("@client fn f(s: String) { render(html(s)); }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Un número no puede contener marcado: su texto es seguro y el idioma
// `concat(literal, text(n))` sigue funcionando sin conversiones.
#[test]
fn el_texto_de_un_numero_es_seguro() {
    let errs = check_src("@client fn f(n: Int) { render(concat(\"n=\", text(n))); }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Pero el de un String no: puede traer marcado.
#[test]
fn el_texto_de_un_string_no_es_seguro() {
    let errs = check_src("@client fn f(s: String) { render(concat(\"x=\", text(s))); }");
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

// Html vale donde se espera texto; lo contrario no, que es la garantía.
#[test]
fn html_es_subtipo_de_string_pero_no_al_reves() {
    let errs = check_src("@client fn f(s: String) -> String { return escape(s); }");
    assert!(
        errs.is_empty(),
        "Html debe valer como String: {:?}",
        codes(&errs)
    );
    let errs = check_src("@client fn f(s: String) -> Html { return s; }");
    assert!(
        has_code(&errs, "E_RETURN_TYPE_MISMATCH"),
        "{:?}",
        codes(&errs)
    );
}

// --- agujeros del tipo Html que encontró la revisión ---

// `Unknown` es comodín para no encadenar errores, pero si además se colara en
// `Html` entonces un Record, un campo abierto o un match heterogéneo lavarían
// cualquier dato hasta el DOM. Es el único tipo que no acepta "no sé".
#[test]
fn unknown_no_se_cuela_en_html() {
    let errs = check_src("@client fn vista(r: Record) { render(r); }");
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn un_campo_de_tipo_abierto_no_es_html() {
    let errs = check_src(
        "store almacen: Record;\n\
         @server fn primero() -> Record { return all(almacen)[0]; }\n\
         @client fn f() { let p = primero(); let x: Html = p.t; render(x); }",
    );
    assert!(!errs.is_empty(), "un campo abierto no puede ser Html");
}

// Un match cuyas ramas no unifican degrada a Unknown; eso no puede ser Html.
#[test]
fn un_match_heterogeneo_no_produce_html() {
    let errs = check_src(
        "@client fn f(s: String, n: Int) { let x = match n { 1 => escape(s), _ => s }; render(x); }",
    );
    assert!(
        !errs.is_empty(),
        "el match heterogéneo no puede lavar a Html"
    );
}

// `reactive` es laxo con la inferencia, pero no puede serlo con Html.
#[test]
fn reactive_no_relaja_el_subtipado_de_html() {
    let errs = check_src("@client fn f(s: String) { reactive h: Html = s; render(h); }");
    assert!(!errs.is_empty(), "reactive no puede saltarse Html");
}

// `build-app` monta `vista` en el DOM: su retorno es un sumidero igual que
// render, así que la garantía tiene que cubrir la ruta por defecto.
#[test]
fn la_vista_montada_debe_devolver_html() {
    let errs = check_src(
        "type P = { t: String };\n\
         store almacen: P;\n\
         @server fn feed() -> List<P> { return all(almacen); }\n\
         reactive mut posts = [];\n\
         @client fn vista() -> String { let ps = posts; return concat(\"<ul>\", ps[0].t); }",
    );
    assert!(has_code(&errs, "E_VISTA_NO_HTML"), "{:?}", codes(&errs));
}

#[test]
fn una_vista_que_devuelve_html_es_valida() {
    let errs = check_src("@client fn vista() -> Html { return \"<p>hola</p>\"; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// La confianza no se serializa: al otro lado del cable la reconstruye quien
// mande el JSON, y el atacante no usa el cliente generado.
#[test]
fn html_no_vale_como_parametro_remoto() {
    let errs = check_src("@server fn publicar(c: Html) { print(c); }");
    assert!(
        has_code(&errs, "E_BOUNDARY_NOT_SERIALIZABLE"),
        "{:?}",
        codes(&errs)
    );
}

// --- listas y texto: sin esto no se puede escribir una búsqueda ---

// Construir una lista en runtime era imposible: no había concat de listas ni
// append, así que una función no podía devolver un subconjunto filtrado. Con
// `unir`/`agregar` el tipo del elemento se conserva (no hay genéricos: la firma
// se calcula desde los argumentos).
#[test]
fn unir_conserva_el_tipo_del_elemento() {
    let errs = check_src(
        "type P = { t: String };\n\
         fn f(a: List<P>, b: List<P>) -> List<P> { return concat(a, b); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn unir_listas_de_tipos_distintos_es_error() {
    let errs =
        check_src("fn f(a: List<Int>, b: List<String>) -> List<Int> { return concat(a, b); }");
    assert!(
        has_code(&errs, "E_LIST_HETEROGENEOUS"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn agregar_exige_que_el_elemento_encaje() {
    let ok = check_src("fn f(xs: List<Int>, x: Int) -> List<Int> { return append(xs, x); }");
    assert!(ok.is_empty(), "{:?}", codes(&ok));
    let mal = check_src("fn f(xs: List<Int>, s: String) -> List<Int> { return append(xs, s); }");
    assert!(has_code(&mal, "E_LIST_HETEROGENEOUS"), "{:?}", codes(&mal));
}

#[test]
fn unir_sobre_algo_que_no_es_lista_es_error() {
    let errs = check_src("fn f(a: Int, b: List<Int>) -> List<Int> { return concat(a, b); }");
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

// Búsqueda de texto: sin `contiene`/`minusculas`/`largo` no se podía comparar
// cadenas más allá de la igualdad exacta.
#[test]
fn los_builtins_de_texto_tipan() {
    let errs = check_src(
        "fn f(t: String, q: String) -> Bool { \
           if len(q) < 1 { return true; } \
           return contains(lower(t), lower(q)); \
         }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Un almacén es un asa del SERVIDOR: solo se declara en ese bundle. Nombrarla
// desde @client tipaba limpio y reventaba con ReferenceError al cargar, porque
// E_STATE_OFF_SERVER solo cubría las llamadas a los builtins y no la referencia
// suelta. Mismo fallo que ya se había cerrado para las globales reactivas.
#[test]
fn referenciar_un_almacen_desde_client_es_error() {
    let errs = check_src(
        "type P = { a: Int };\nstore cosas: P;\n@client fn f() { let x = cosas; print(\"h\"); }",
    );
    assert!(has_code(&errs, "E_STATE_OFF_SERVER"), "{:?}", codes(&errs));
}

#[test]
fn referenciar_un_almacen_desde_una_fn_sin_anotacion_es_error() {
    let errs = check_src("type P = { a: Int };\nstore cosas: P;\nfn f() { let x = cosas; }");
    assert!(has_code(&errs, "E_STATE_OFF_SERVER"), "{:?}", codes(&errs));
}

#[test]
fn usar_el_almacen_desde_server_es_valido() {
    let errs = check_src(
        "type P = { a: Int };\nstore cosas: P;\n@server fn f() -> Int { return len(all(cosas)); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// --- almacenes: colisiones con la capa de persistencia ---

// `__id` y `__doc` son columnas internas: un registro que las declare produce
// una tabla con la columna repetida y todo `guardar` revienta en runtime
// ("duplicate column name"). El verificador lo aceptaba.
#[test]
fn un_campo_id_reservado_es_error() {
    let errs = check_src("type T = { __id: String };\nstore cosas: T;");
    assert!(has_code(&errs, "E_CAMPO_RESERVADO"), "{:?}", codes(&errs));
    let errs = check_src("type T = { __doc: Int };\nstore cosas: T;");
    assert!(has_code(&errs, "E_CAMPO_RESERVADO"), "{:?}", codes(&errs));
}

// Dos almacenes que solo difieren en mayúsculas caían en la misma tabla (el
// nombre se pasa a minúsculas) y en el mismo archivo (macOS y Windows no
// distinguen caja), mezclando datos de tipos distintos sin avisar.
#[test]
fn dos_almacenes_que_difieren_solo_en_caja_es_error() {
    let errs = check_src("type T = { a: Int };\nstore Datos: T;\nstore datos: T;");
    assert!(has_code(&errs, "E_DUPLICATE_STORE"), "{:?}", codes(&errs));
}

// Un campo que coincide con una palabra reservada de SQL sí es válido: los
// identificadores se entrecomillan por dialecto.
#[test]
fn los_campos_con_palabras_reservadas_de_sql_son_validos() {
    let errs = check_src("type T = { order: Int, group: String, select: Bool };\nstore cosas: T;");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// --- almacenes PRESTADOS: `store p: T from "tabla";` ---
//
// Un almacén propio POSEE su tabla: Marea la crea, manda en su esquema y por eso
// puede garantizarlo. Con `from` la toma PRESTADA de otro programa, que es lo que
// permite migrar una web cuyas tablas escribe otro motor. Lo que cambia no es el
// tipo —leer sigue dando `List<T>`— sino quién manda en la tabla, y de ahí salen
// las reglas de abajo.

// Leer un almacén prestado es exactamente leer uno propio: mismo tipo, misma
// ubicación. Es el caso que existe para funcionar.
#[test]
fn leer_un_almacen_prestado_tipa() {
    let errs = check_src(
        "type Producto = { nombre: String, precio: Int };\n\
         store productos: Producto from \"products\";\n\
         @server fn catalogo() -> List<Producto> { return all(productos); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// El tipo sigue mandando en la lectura: `all` de un prestado devuelve `List<T>`,
// no un comodín. Sin esto el préstamo sería un agujero por el que entra `?` al
// resto del programa.
#[test]
fn el_tipo_manda_en_la_lectura_de_un_prestado() {
    let errs = check_src(
        "type Producto = { nombre: String, precio: Int };\n\
         store productos: Producto from \"products\";\n\
         @server fn f() -> List<Int> { return all(productos); }",
    );
    assert!(
        has_code(&errs, "E_RETURN_TYPE_MISMATCH"),
        "{:?}",
        codes(&errs)
    );
}

// Escribir en la tabla de otro a ciegas es escribir en la base de datos de otra
// aplicación: Marea no manda en su esquema ni en sus invariantes. Sólo lectura.
#[test]
fn e_store_prestado_solo_lectura_save() {
    let errs = check_src(
        "type Producto = { nombre: String };\n\
         store productos: Producto from \"products\";\n\
         @server fn f() { save(productos, Producto { nombre: \"x\" }); }",
    );
    assert!(
        has_code(&errs, "E_STORE_PRESTADO_SOLO_LECTURA"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_store_prestado_solo_lectura_update_y_remove() {
    let errs = check_src(
        "type Producto = { nombre: String };\n\
         store productos: Producto from \"products\";\n\
         @server fn f() { update(productos, 0, Producto { nombre: \"x\" }); }",
    );
    assert!(
        has_code(&errs, "E_STORE_PRESTADO_SOLO_LECTURA"),
        "{:?}",
        codes(&errs)
    );
    let errs = check_src(
        "type Producto = { nombre: String };\n\
         store productos: Producto from \"products\";\n\
         @server fn f() { remove(productos, 0); }",
    );
    assert!(
        has_code(&errs, "E_STORE_PRESTADO_SOLO_LECTURA"),
        "{:?}",
        codes(&errs)
    );
}

// La regla es del PRÉSTAMO, no del builtin: en un almacén propio se sigue
// escribiendo igual que siempre.
#[test]
fn escribir_en_un_almacen_propio_sigue_valiendo() {
    let errs = check_src(
        "type Producto = { nombre: String };\n\
         store productos: Producto;\n\
         @server fn f() { save(productos, Producto { nombre: \"x\" }); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Una tabla sin nombre no es ninguna tabla: `from \"\"` (o sólo espacios) no dice
// de dónde leer, y el error tiene que salir aquí y no en la primera consulta.
#[test]
fn e_tabla_externa_vacia() {
    let errs = check_src(
        "type P = { a: Int };\n\
         store productos: P from \"\";",
    );
    assert!(
        has_code(&errs, "E_TABLA_EXTERNA_VACIA"),
        "{:?}",
        codes(&errs)
    );
    let errs = check_src(
        "type P = { a: Int };\n\
         store productos: P from \"   \";",
    );
    assert!(
        has_code(&errs, "E_TABLA_EXTERNA_VACIA"),
        "{:?}",
        codes(&errs)
    );
}

// Una tabla ajena tiene COLUMNAS y el tipo es lo único que dice a qué campo va
// cada una. Un escalar no tiene campos: no hay mapeo posible.
#[test]
fn e_store_prestado_no_registro() {
    let errs = check_src("store contadores: Int from \"counters\";");
    assert!(
        has_code(&errs, "E_STORE_PRESTADO_NO_REGISTRO"),
        "{:?}",
        codes(&errs)
    );
    // Una lista tampoco: la tabla ya es la colección.
    let errs = check_src("type P = { a: Int };\nstore productos: List<P> from \"products\";");
    assert!(
        has_code(&errs, "E_STORE_PRESTADO_NO_REGISTRO"),
        "{:?}",
        codes(&errs)
    );
}

// Un almacén PROPIO sí admite un escalar: Marea manda en ese esquema y lo guarda
// en una columna suya. La regla nueva no puede llevarse eso por delante.
#[test]
fn un_almacen_propio_escalar_sigue_valiendo() {
    let errs = check_src("store contadores: Int;");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Dos almacenes sobre la misma tabla serían dos vistas del mismo sitio con dos
// tipos: el día que una se quede atrás respecto al esquema real, nadie lo vería.
#[test]
fn e_tabla_externa_duplicada() {
    let errs = check_src(
        "type P = { a: Int };\ntype Q = { b: String };\n\
         store uno: P from \"products\";\n\
         store otro: Q from \"products\";",
    );
    assert!(
        has_code(&errs, "E_TABLA_EXTERNA_DUPLICADA"),
        "{:?}",
        codes(&errs)
    );
}

// También cuando la tabla prestada es la que un almacén PROPIO ya iba a crear:
// la de uno propio se deriva del nombre en minúsculas, así que el choque es el
// mismo aunque sólo uno lleve `from`.
#[test]
fn una_tabla_prestada_no_puede_pisar_la_de_un_almacen_propio() {
    let errs = check_src(
        "type P = { a: Int };\ntype Q = { b: String };\n\
         store productos: P;\n\
         store ajenos: Q from \"Productos\";",
    );
    assert!(
        has_code(&errs, "E_TABLA_EXTERNA_DUPLICADA"),
        "{:?}",
        codes(&errs)
    );
}

// Dos prestados a tablas distintas son dos almacenes normales y corrientes.
#[test]
fn dos_prestados_a_tablas_distintas_tipan() {
    let errs = check_src(
        "type P = { a: Int };\ntype L = { b: String };\n\
         store productos: P from \"products\";\n\
         store anuncios: L from \"listings\";\n\
         @server fn f() -> Int { return len(all(productos)); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// --- red saliente ---

// Salir a la red es del servidor: desde el navegador la petición la haría el
// cliente (otro origen, otras credenciales, CORS decidiendo), que no es lo que
// el programa dice; y la lista blanca de destinos vive en el servidor.
#[test]
fn pedir_fuera_de_server_es_error() {
    let errs = check_src("@client fn f() -> String { return fetch(\"https://ejemplo.com\"); }");
    assert!(has_code(&errs, "E_RED_OFF_SERVER"), "{:?}", codes(&errs));
    let errs = check_src("fn f() -> String { return post(\"https://x.com\", \"{}\"); }");
    assert!(has_code(&errs, "E_RED_OFF_SERVER"), "{:?}", codes(&errs));
}

#[test]
fn pedir_desde_server_es_valido() {
    let errs = check_src("@server fn f() -> String { return fetch(\"https://ejemplo.com\"); }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Leer JSON sí puede hacerse en cualquier lado: es cómputo puro sobre un texto
// que ya se tiene.
#[test]
fn leer_json_vale_en_el_cliente() {
    let errs = check_src(
        "@client fn f(c: String) -> Int { \
           if jsonText(c, \"a.b\") != \"\" { return jsonInt(c, \"n\"); } \
           return jsonLen(c, \"lista\"); \
         }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// --- uniones con representación de runtime ---

// Una variante que resuelve a un REGISTRO no lleva etiqueta en runtime, así que
// nombrarla en una rama dejaba esa rama muerta en silencio: el match no
// ejecutaba ninguna. Ahora se dice al compilar.
#[test]
fn una_variante_que_es_registro_no_puede_nombrarse_en_un_match() {
    let errs = check_src(
        "type User = { nombre: String };\n\
         @client fn f(u: User | NotFound) -> String { \
           return match u { User => u.nombre, NotFound => \"no\", _ => \"\" }; }",
    );
    assert!(
        has_code(&errs, "E_VARIANTE_SIN_ETIQUETA"),
        "{:?}",
        codes(&errs)
    );
}

// El patrón correcto —comodín para el caso del registro— sigue siendo válido.
#[test]
fn el_comodin_cubre_el_caso_del_registro() {
    let errs = check_src(
        "type User = { nombre: String };\n\
         @client fn f(u: User | NotFound) -> String { \
           return match u { NotFound => \"no\", otro => otro.nombre }; }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// --- plantillas: el escapado deja de poder olvidarse ---

// Una plantilla SIEMPRE es Html: los huecos se escapan al emitir.
#[test]
fn una_plantilla_es_html() {
    let errs = check_src("@client fn f(s: String) -> Html { return `hola {s}`; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Y como Html es subtipo de String, vale también donde se espera texto.
#[test]
fn una_plantilla_vale_como_string() {
    let errs = check_src("fn f(n: Int) -> String { return `n = {text(n)}`; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// La puerta cruda sólo admite lo que YA es Html, así que no puede colar texto
// sin escapar: es la razón de que la plantilla sea segura por construcción.
#[test]
fn la_interpolacion_cruda_exige_html() {
    let mal = check_src("@client fn f(s: String) -> Html { return `x {!s} y`; }");
    assert!(
        has_code(&mal, "E_INTERP_CRUDA_NO_HTML"),
        "{:?}",
        codes(&mal)
    );
    let bien = check_src(
        "fn trozo() -> Html { return \"<b>x</b>\"; }\n\
         @client fn f() -> Html { return `x {!trozo()} y`; }",
    );
    assert!(bien.is_empty(), "{:?}", codes(&bien));
}

// Un dato del store interpolado normal se escapa, así que llega al DOM seguro.
#[test]
fn un_dato_interpolado_llega_escapado_al_dom() {
    let errs = check_src(
        "type P = { texto: String };\n\
         @client fn vista(p: P) -> Html { return `<li>{p.texto}</li>`; }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// --- el bucle for ---

// Sin bucle, todo recorrido era una función recursiva con índice: en la tienda
// eran cinco de veintiocho funciones, todas con la misma forma.
#[test]
fn el_for_recorre_una_lista() {
    let errs = check_src(
        "fn suma(xs: List<Int>) -> Int { let mut n = 0; for x in xs { n = n + x; } return n; }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn el_for_con_indice_liga_un_int() {
    let errs = check_src(
        "type P = { a: Int };\n\
         fn f(xs: List<P>) -> Int { let mut n = 0; for p, i in xs { n = n + p.a + i; } return n; }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn el_for_solo_recorre_listas() {
    let errs = check_src("fn f(n: Int) { for x in n { print(x); } }");
    assert!(has_code(&errs, "E_FOR_NO_LISTA"), "{:?}", codes(&errs));
}

// El elemento y el índice son inmutables: reasignarlos no cambiaría la lista,
// así que permitirlo solo crearía una expectativa falsa.
#[test]
fn el_elemento_y_el_indice_son_inmutables() {
    let a = check_src("fn f(xs: List<Int>) { for x in xs { x = 1; } }");
    assert!(has_code(&a, "E_ASSIGN_IMMUTABLE"), "{:?}", codes(&a));
    let b = check_src("fn f(xs: List<Int>) { for x, i in xs { i = 1; } }");
    assert!(has_code(&b, "E_ASSIGN_IMMUTABLE"), "{:?}", codes(&b));
}

// Y no escapan del bucle.
#[test]
fn el_elemento_no_escapa_del_bucle() {
    let errs = check_src("fn f(xs: List<Int>) -> Int { for x in xs { print(x); } return x; }");
    assert!(has_code(&errs, "E_UNRESOLVED_NAME"), "{:?}", codes(&errs));
}

// Un `for` no garantiza retorno: la lista puede estar vacía y el cuerpo no
// ejecutarse ni una vez.
#[test]
fn un_for_no_cuenta_como_retorno() {
    let errs = check_src("fn f(xs: List<Int>) -> Int { for x in xs { return x; } }");
    assert!(has_code(&errs, "E_MISSING_RETURN"), "{:?}", codes(&errs));
}

#[test]
fn los_for_anidados_valen() {
    let errs = check_src(
        "fn f(xs: List<List<Int>>) -> Int { \
           let mut n = 0; for fila in xs { for c in fila { n = n + c; } } return n; }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// ==================== POLÍTICA: quién cruza la frontera ====================
//
// La regla está ACOTADA a propósito: sin `@session` no se exige nada, porque no
// se le puede pedir identidad a un programa que no ha dicho qué es una
// identidad. En cuanto la declara, `@server` a secas deja de compilar.

/// Un programa con identidad y todos sus handlers decididos.
const CON_SESSION: &str = "\
type Usuario = { nombre: String };
@session fn quien(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }
@server(Usuario) fn borrar(i: Int) { print(i); }
@server(Public) fn feed() -> Int { return 1; }
";

#[test]
fn un_programa_con_politicas_completas_tipa() {
    let errs = check_src(CON_SESSION);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn sin_session_no_se_exige_politica() {
    // Es lo que hace que la regla no cueste una migración: los 27 handlers que
    // ya existen en el repo no declaran identidad y siguen compilando igual.
    let errs = check_src("@server fn feed() -> Int { return 1; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn e_server_sin_politica() {
    let src = "\
type Usuario = { nombre: String };
@session fn quien(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }
@server fn ventas() -> Int { return 1; }
";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_SERVER_SIN_POLITICA"),
        "{:?}",
        codes(&errs)
    );
    // El mensaje propone las dos salidas, con el tipo real del programa.
    let e = errs
        .iter()
        .find(|e| e.code == "E_SERVER_SIN_POLITICA")
        .unwrap();
    assert!(e.message.contains("@server(Usuario)"), "{}", e.message);
    assert!(e.message.contains("@server(Public)"), "{}", e.message);
}

#[test]
fn public_es_una_decision_valida_siempre() {
    // Incluso sin @session: decir "esto es público" nunca es un error, porque
    // el punto de la regla es que se escriba, no que se exija identidad.
    let errs = check_src("@server(Public) fn feed() -> Int { return 1; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn e_politica_off_server() {
    // (No se llama `vista`: esa tiene su propia regla, debe devolver Html.)
    let errs = check_src("@client fn contar() -> Int { return 1; }");
    assert!(errs.is_empty(), "{:?}", codes(&errs));
    let errs = check_src("@client(Public) fn contar() -> Int { return 1; }");
    assert!(
        has_code(&errs, "E_POLITICA_OFF_SERVER"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_politica_sin_session() {
    let src = "\
type Usuario = { nombre: String };
@server(Usuario) fn borrar(i: Int) { print(i); }
";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_POLITICA_SIN_SESSION"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_politica_no_coincide() {
    let src = "\
type Usuario = { nombre: String };
type Admin = { nivel: Int };
@session fn quien(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }
@server(Admin) fn borrar(i: Int) { print(i); }
";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_POLITICA_NO_COINCIDE"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_session_duplicada() {
    let src = "\
type Usuario = { nombre: String };
@session fn quien(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }
@session fn otro(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }
";
    assert!(has_code(&check_src(src), "E_SESSION_DUPLICADA"));
}

#[test]
fn e_session_firma() {
    let base = "type Usuario = { nombre: String };\n";
    // Sin unión de retorno: no obliga a cubrir el fallo.
    let src = format!(
        "{base}@session fn quien(t: String) -> Usuario {{ return Usuario {{ nombre: \"a\" }}; }}"
    );
    assert!(
        has_code(&check_src(&src), "E_SESSION_FIRMA"),
        "retorno no-unión"
    );
    // Sin parámetro de token.
    let src =
        format!("{base}@session fn quien() -> Usuario | NoAutorizado {{ return NoAutorizado; }}");
    assert!(has_code(&check_src(&src), "E_SESSION_FIRMA"), "sin token");
    // El token no es String.
    let src = format!(
        "{base}@session fn quien(t: Int) -> Usuario | NoAutorizado {{ return NoAutorizado; }}"
    );
    assert!(
        has_code(&check_src(&src), "E_SESSION_FIRMA"),
        "token no String"
    );
}

#[test]
fn e_session_no_invocable() {
    // Llamarla sería elegir tu propia identidad pasando el token que quieras.
    let src = format!(
        "{CON_SESSION}@server(Public) fn colar() -> Int {{ quien(\"loquesea\"); return 1; }}"
    );
    let errs = check_src(&src);
    assert!(
        has_code(&errs, "E_SESSION_NO_INVOCABLE"),
        "{:?}",
        codes(&errs)
    );
}

// --------- La identidad ligada: `@server(u: Usuario)` ---------
//
// Exigir identidad y no poder mirarla dejaría la política en una etiqueta. El
// nombre entra en el scope de la función como un binding más —inmutable, del
// tipo de la política— pero NO como parámetro: no viaja en la llamada, la
// inyecta el runtime tras resolver el token.

/// Programa mínimo con identidad, al que se le cuelga el handler de cada caso.
fn con_identidad(handler: &str) -> Vec<marea_types::TypeError> {
    let src = format!(
        "type Usuario = {{ nombre: String }};\n\
         @session fn quien(t: String) -> Usuario | NoAutorizado {{ return NoAutorizado; }}\n\
         {handler}"
    );
    check_src(&src)
}

#[test]
fn la_identidad_nombrada_esta_en_scope() {
    let errs = con_identidad(
        "@server(u: Usuario) fn publicar(texto: String) { print(u.nombre); print(texto); }",
    );
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn la_identidad_lleva_el_tipo_de_la_politica() {
    // Si el binding no tuviera el tipo de la política, un campo inventado
    // pasaría desapercibido y el `u.nombre` de arriba sería casualidad.
    let errs = con_identidad("@server(u: Usuario) fn publicar() { print(u.telefono); }");
    assert!(has_code(&errs, "E_NO_FIELD"), "{:?}", codes(&errs));
}

#[test]
fn la_identidad_es_inmutable() {
    // Quién eres no se reasigna: lo decidió la @session con el token.
    let errs =
        con_identidad("@server(u: Usuario) fn publicar() { u = Usuario { nombre: \"otro\" }; }");
    assert!(has_code(&errs, "E_ASSIGN_IMMUTABLE"), "{:?}", codes(&errs));
}

#[test]
fn la_politica_sin_nombre_no_liga_nada() {
    // `@server(Usuario)` exige identidad sin usarla; no introduce ninguna
    // variable mágica que el cuerpo pueda leer por accidente.
    let errs = con_identidad("@server(Usuario) fn borrar(i: Int) { print(u); }");
    assert!(has_code(&errs, "E_UNRESOLVED_NAME"), "{:?}", codes(&errs));
}

#[test]
fn e_identidad_choca_param() {
    // El cliente manda los parámetros; la identidad la pone el servidor. Que
    // compartieran nombre sería no poder decir cuál de los dos estás leyendo.
    let errs = con_identidad("@server(u: Usuario) fn publicar(u: String) { print(u); }");
    assert!(
        has_code(&errs, "E_IDENTIDAD_CHOCA_PARAM"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_identidad_choca_con_el_nombre_reservado() {
    // Aunque la política no la nombre, el generador la inyecta como primer
    // parámetro con un nombre reservado: un parámetro homónimo daría dos
    // parámetros iguales, o sea un archivo generado que ni siquiera carga.
    let errs = con_identidad("@server(Usuario) fn borrar(__identidad: Int) { print(1); }");
    assert!(
        has_code(&errs, "E_IDENTIDAD_CHOCA_PARAM"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_identidad_en_public() {
    // `Public` es la decisión de no exigir identidad: no hay ninguna que ligar,
    // y el runtime no pasaría nada, así que `u` sería 'undefined' en ejecución.
    let errs = check_src("@server(u: Public) fn feed() -> Int { return 1; }");
    assert!(
        has_code(&errs, "E_IDENTIDAD_EN_PUBLIC"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn la_identidad_no_puede_llamarse_como_un_builtin() {
    // Se vuelve el primer parámetro de la función emitida: llamarla `print`
    // sombrearía al builtin dentro del cuerpo y el archivo generado moriría.
    let errs = con_identidad("@server(print: Usuario) fn publicar() { print(1); }");
    assert!(has_code(&errs, "E_REDEFINE_BUILTIN"), "{:?}", codes(&errs));
}

// ===========================================================================
// CIERRES
//
// Un cierre es un valor de tipo `Ty::Fn`, que ya existía porque una función
// declarada también lo tiene. Lo nuevo aquí es tipar el cuerpo en su propio
// scope, deducir el retorno cuando no se escribe, decir qué se puede capturar
// y poder LLAMAR a un valor de ese tipo (antes sólo se llamaba por nombre).
// ===========================================================================

#[test]
fn un_cierre_tipa_y_se_puede_llamar() {
    let src = "@client fn v() { let menor = fn(a: Int, b: Int) -> Bool { return a < b; }; print(menor(1, 2)); }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn un_cierre_se_puede_llamar_en_el_acto() {
    let src = "@client fn v() { let n: Int = fn() -> Int { return 7; }(); print(n); }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn llamar_un_cierre_chequea_la_aridad() {
    let src = "@client fn v() { let f = fn(a: Int) -> Int { return a; }; print(f(1, 2)); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ARITY"), "{:?}", codes(&errs));
}

#[test]
fn llamar_un_cierre_chequea_los_tipos_de_los_argumentos() {
    let src = "@client fn v() { let f = fn(a: Int) -> Int { return a; }; print(f(\"x\")); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn el_retorno_de_un_cierre_se_usa_con_su_tipo() {
    // Si el `Ty::Fn` no llevara bien el retorno, esto pasaría desapercibido.
    let src =
        "@client fn v() { let f = fn() -> Int { return 1; }; let s: String = f(); print(s); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_LET_TYPE_MISMATCH"), "{:?}", codes(&errs));
}

#[test]
fn el_cuerpo_del_cierre_se_chequea() {
    let src = "@client fn v() { let f = fn(a: Int) -> Int { return a + \"x\"; }; print(f(1)); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ARITH_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn los_parametros_del_cierre_no_escapan_de_su_cuerpo() {
    let src = "@client fn v() { let f = fn(a: Int) -> Int { return a; }; print(a); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_UNRESOLVED_NAME"), "{:?}", codes(&errs));
}

#[test]
fn un_cierre_con_retorno_declarado_debe_devolverlo_en_todos_los_caminos() {
    let src = "@client fn v() { let f = fn(a: Int) -> Int { print(a); }; print(f(1)); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_MISSING_RETURN"), "{:?}", codes(&errs));
}

#[test]
fn un_cierre_con_retorno_declarado_lo_respeta() {
    let src = "@client fn v() { let f = fn() -> Int { return \"x\"; }; print(f()); }";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_RETURN_TYPE_MISMATCH"),
        "{:?}",
        codes(&errs)
    );
}

// --- deducción del retorno ---

#[test]
fn el_retorno_se_deduce_del_cuerpo() {
    // Sin `-> T`: el `return` es lo único que dice el tipo, y basta.
    let src =
        "@client fn v() { let f = fn(a: Int) { return a * 2; }; let n: Int = f(3); print(n); }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn un_cierre_sin_return_deduce_unit() {
    let src = "@client fn v() { let f = fn(a: Int) { print(a); }; f(1); }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn el_retorno_deducido_es_el_de_verdad() {
    // Se deduce Int; usarlo como String tiene que doler igual que si se hubiera
    // escrito `-> Int`, o la deducción sería un agujero en vez de una comodidad.
    let src = "@client fn v() { let f = fn() { return 1; }; let s: String = f(); print(s); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_LET_TYPE_MISMATCH"), "{:?}", codes(&errs));
}

#[test]
fn e_cierre_retorno_ambiguo() {
    // Dos `return` que no coinciden: no hay nada que deducir, y adivinarlo
    // costaría una regla más larga que escribir `-> Tipo`.
    let src = "@client fn v() { let f = fn(a: Bool) { if a { return 1; } else { return \"x\"; } }; f(true); }";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_CIERRE_RETORNO_AMBIGUO"),
        "{:?}",
        codes(&errs)
    );
}

// --- captura ---

#[test]
fn capturar_un_let_inmutable_esta_bien() {
    let src = "@client fn v() { let base = 10; let f = fn(a: Int) -> Int { return a + base; }; print(f(1)); }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn e_captura_mutable_al_leer() {
    // Copiar una `mut` crea la expectativa falsa de que reasignarla fuera se
    // vea dentro. Mejor decirlo que sorprender.
    let src = "@client fn v() { let mut n = 0; let f = fn() -> Int { return n; }; print(f()); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_CAPTURA_MUTABLE"), "{:?}", codes(&errs));
}

#[test]
fn e_captura_mutable_al_asignar() {
    // La misma trampa en su forma más nítida: escribiría en la copia.
    let src = "@client fn v() { let mut n = 0; let f = fn() { n = 1; }; f(); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_CAPTURA_MUTABLE"), "{:?}", codes(&errs));
}

#[test]
fn una_mut_declarada_dentro_del_cierre_no_es_una_captura() {
    // Es local suya: no hay dos mitades del programa creyendo tener la misma
    // variable, así que no hay nada que avisar.
    let src = "@client fn v() { let f = fn() -> Int { let mut n = 0; n = n + 1; return n; }; print(f()); }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn un_cierre_anidado_tambien_delata_la_captura_mutable() {
    let src = "@client fn v() { let f = fn() -> Int { let mut n = 0; let g = fn() -> Int { return n; }; return g(); }; print(f()); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_CAPTURA_MUTABLE"), "{:?}", codes(&errs));
}

// --- ubicación heredada ---

#[test]
fn un_cierre_de_una_server_puede_tocar_el_store() {
    // La ubicación se hereda del `fn` que lo crea, así que las reglas de estado
    // le caen encima solas: no hace falta anotar el cierre.
    let src = "type P = { a: Int };\nstore almacen: P;\n@server fn f() -> Int { let g = fn() -> Int { return len(all(almacen)); }; return g(); }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn un_cierre_de_una_client_no_puede_tocar_el_store() {
    let src = "type P = { a: Int };\nstore almacen: P;\n@client fn f() -> Int { let g = fn() -> Int { return len(all(almacen)); }; return g(); }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_STATE_OFF_SERVER"), "{:?}", codes(&errs));
}

#[test]
fn un_cierre_de_una_client_sigue_cruzando_la_frontera_al_llamar_una_server() {
    let src = "@server fn guardar(n: Int) -> Int { return n; }\n@client fn v() { let f = fn() -> Int { return guardar(1); }; print(f()); }";
    let module = marea_syntax::parse(src).expect("el fuente debe parsear");
    let (errs, cruces) = check_with_boundaries(&module);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
    assert_eq!(cruces.len(), 1, "{cruces:?}");
    assert_eq!(cruces[0].callee, "guardar");
}

// --- la frontera de red: un cierre es código, no un dato ---

#[test]
fn un_cierre_no_cruza_la_frontera_de_red() {
    // `is_serializable` ya decía que `Ty::Fn` no viaja; con cierres eso deja de
    // ser teórico, porque ahora sí hay forma de escribir uno en un argumento.
    // Es la razón de que un `sort(xs, criterio)` se quede de este lado.
    let src = "@server fn guardar(n: Int) -> Int { return n; }\n@client fn v() { print(guardar(fn(a: Int) -> Int { return a; })); }";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_BOUNDARY_NOT_SERIALIZABLE"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn una_funcion_declarada_tampoco_cruza_la_frontera_de_red() {
    // El mismo agujero por la otra puerta: pasar el NOMBRE de una función es
    // pasar un `Ty::Fn` igualmente.
    let src = "@server fn guardar(n: Int) -> Int { return n; }\nfn doble(x: Int) -> Int { return x * 2; }\n@client fn v() { print(guardar(doble)); }";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_BOUNDARY_NOT_SERIALIZABLE"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn un_cierre_que_no_cruza_la_red_no_molesta_a_nadie() {
    // La prohibición es de la RED, no de los cierres: del mismo lado de la
    // frontera son un valor más, y ahí es donde tienen que seguir siendo
    // cómodos —que es todo el sentido de tenerlos.
    let src = "@server fn f(n: Int) -> Int { let doble = fn(x: Int) -> Int { return x * 2; }; return doble(n); }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// --- el modelo de eventos: 'on' -------------------------------------------
//
// Un manejador es la otra mitad de la frontera del tiempo: el estado ya llegaba
// al DOM, pero el DOM no podía tocar el estado, y lo único que había era un
// `onclick="marea.f(3)"` escrito dentro de una cadena, que no miraba nadie.
// Estas reglas son exactamente lo que aquella cadena no podía comprobar.

#[test]
fn on_engancha_un_manejador_en_client() {
    let src = "\
reactive mut n = 0;
@client fn vista() -> Html { return `<b {!on(\"click\", fn() { n = n + 1; })}>+</b>`; }
";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Sin anotación también vale: esa función se emite en los dos bundles y en el
// del servidor `on` sencillamente no llega a llamarse.
#[test]
fn on_vale_en_una_funcion_sin_anotacion() {
    let src = "fn boton() -> Html { return `<b {!on(\"click\", fn() { print(1); })}>x</b>`; }";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Un manejador no puede vivir en el servidor: allí no hay DOM al que engancharlo.
#[test]
fn e_on_off_client() {
    let src = "\
@server fn boton() -> Html { return `<b {!on(\"click\", fn() { print(1); })}>x</b>`; }
";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ON_OFF_CLIENT"), "{:?}", codes(&errs));
}

// Un evento mal escrito no da error en ningún sitio: el navegador acepta
// `addEventListener("clcik", ...)` sin rechistar y lo que se ve es un botón
// mudo. Aquí corta, y con la lista de los válidos delante.
#[test]
fn e_evento_desconocido() {
    let src = "\
@client fn v() -> Html { return `<b {!on(\"clcik\", fn() { print(1); })}>x</b>`; }
";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_EVENTO_DESCONOCIDO"),
        "{:?}",
        codes(&errs)
    );
    let e = errs
        .iter()
        .find(|e| e.code == "E_EVENTO_DESCONOCIDO")
        .unwrap();
    assert!(e.message.contains("click"), "{}", e.message);
    assert!(e.message.contains("pointerleave"), "{}", e.message);
}

#[test]
fn e_evento_no_literal() {
    let src = "\
@client fn v(e: String) -> Html { return `<b {!on(e, fn() { print(1); })}>x</b>`; }
";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_EVENTO_NO_LITERAL"), "{:?}", codes(&errs));
}

// El manejador no recibe el evento como valor: esa forma todavía no existe en
// el lenguaje, así que un cierre con parámetros es una expectativa falsa.
#[test]
fn e_manejador_con_params() {
    let src = "\
@client fn v() -> Html { return `<b {!on(\"click\", fn(x: Int) { print(x); })}>x</b>`; }
";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_MANEJADOR_CON_PARAMS"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_manejador_devuelve() {
    let src = "\
@client fn v() -> Html { return `<b {!on(\"click\", fn() -> Int { return 1; })}>x</b>`; }
";
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_MANEJADOR_DEVUELVE"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_manejador_no_fn() {
    let src = "@client fn v() -> Html { return `<b {!on(\"click\", 3)}>x</b>`; }";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_MANEJADOR_NO_FN"), "{:?}", codes(&errs));
}

// Escapado, el atributo llega al navegador como texto y el manejador no se
// engancha nunca: otro botón mudo sin un aviso. Es el hueco crudo o nada.
#[test]
fn e_on_escapado() {
    let src = "\
@client fn v() -> Html { return `<b {on(\"click\", fn() { print(1); })}>x</b>`; }
";
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ON_ESCAPADO"), "{:?}", codes(&errs));
}

// El caso de uso entero: asignar a una `reactive mut` desde dentro del
// manejador. Sin esto no hay contador que valga.
#[test]
fn un_manejador_puede_asignar_a_una_reactiva() {
    let src = "\
reactive mut cuenta = 0;
@client fn vista() -> Html {
    return `{text(cuenta)}<b {!on(\"click\", fn() { cuenta = cuenta + 1; })}>+</b>`;
}
";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// Un manejador puede cruzar la otra frontera: llamar a una @server desde dentro
// es lo que compone las dos mitades y hace útil el modelo entero.
#[test]
fn un_manejador_puede_cruzar_la_frontera_de_red() {
    let src = "\
@server(Public) fn guardar(n: Int) { print(n); }
@client fn vista() -> Html { return `<b {!on(\"click\", fn() { guardar(1); })}>x</b>`; }
";
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// 'on' es un builtin y el bundle lo importa del runtime: redeclararlo produce
// un archivo que ni siquiera carga.
#[test]
fn on_es_un_builtin_y_no_se_puede_redefinir() {
    let errs = check_src("fn on(a: Int) -> Int { return a; }");
    assert!(has_code(&errs, "E_REDEFINE_BUILTIN"), "{:?}", codes(&errs));
}

// ===========================================================================
// PÁGINAS: @page("/ruta")
//
// Lo que se fija aquí son las reglas que separan una página de una función
// cualquiera: que la ruta y la firma digan lo mismo, que sólo se devuelva
// `Page` o `Response`, y que una página corra donde corre un `@server` sin
// que haya que escribirlo. El 404 no aparece por ningún lado a propósito: es la
// variante de fallo del retorno, no un caso especial del enrutado.
// ===========================================================================

/// Una página con todo puesto: los seis campos, el hueco atado a su parámetro y
/// el fallo como variante de la unión.
#[test]
fn una_pagina_completa_tipa() {
    let src = r#"
@page("/modelo/:id")
fn modelo(id: Int) -> Page | NotFound {
    if id < 0 { return NotFound; }
    return Page {
        titulo: "Un modelo",
        descripcion: "Lo que cuesta en cada tienda",
        canonica: concat("https://ahorrame.mx/modelo/", text(id)),
        metas: [Meta { clave: "og:type", valor: "product" }],
        jsonld: [json("{\"@type\":\"Product\"}")],
        cuerpo: `<h1>Un modelo</h1>`,
    };
}
"#;
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

/// Los cuatro campos que se pueden omitir se pueden omitir de verdad: su vacío
/// significa algo. El título y la canónica no están entre ellos.
#[test]
fn una_pagina_solo_exige_titulo_y_canonica() {
    let src = r#"
@page("/")
fn portada() -> Page {
    return Page { titulo: "Ahórrame", canonica: "https://ahorrame.mx/" };
}
"#;
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

#[test]
fn e_campo_obligatorio_sin_titulo() {
    let src = r#"
@page("/")
fn portada() -> Page {
    return Page { canonica: "https://ahorrame.mx/" };
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_CAMPO_OBLIGATORIO"), "{:?}", codes(&errs));
}

#[test]
fn e_campo_obligatorio_sin_canonica() {
    let src = r#"
@page("/")
fn portada() -> Page {
    return Page { titulo: "Ahórrame" };
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_CAMPO_OBLIGATORIO"), "{:?}", codes(&errs));
}

// --------------------------- la ruta y la firma ---------------------------

#[test]
fn e_ruta_sin_barra() {
    let src = r#"
@page("precios")
fn precios() -> Page {
    return Page { titulo: "Precios", canonica: "c" };
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_RUTA_SIN_BARRA"), "{:?}", codes(&errs));
}

#[test]
fn e_ruta_param_sin_nombre() {
    let src = r#"
@page("/modelo/:")
fn modelo() -> Page {
    return Page { titulo: "x", canonica: "c" };
}
"#;
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_RUTA_PARAM_SIN_NOMBRE"),
        "{:?}",
        codes(&errs)
    );
}

#[test]
fn e_ruta_param_repetido() {
    let src = r#"
@page("/a/:id/b/:id")
fn dos(id: Int) -> Page {
    return Page { titulo: "x", canonica: "c" };
}
"#;
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_RUTA_PARAM_REPETIDO"),
        "{:?}",
        codes(&errs)
    );
}

/// La ruta promete un hueco que la firma no recoge: el valor de la URL no
/// tendría dónde entrar.
#[test]
fn e_ruta_segmento_sin_param() {
    let src = r#"
@page("/modelo/:id")
fn modelo() -> Page {
    return Page { titulo: "x", canonica: "c" };
}
"#;
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_RUTA_SEGMENTO_SIN_PARAM"),
        "{:?}",
        codes(&errs)
    );
}

/// Y al revés: a una página la invoca una URL, así que un parámetro que la ruta
/// no menciona no se lo pasa nadie.
#[test]
fn e_param_sin_segmento() {
    let src = r#"
@page("/modelo")
fn modelo(id: Int) -> Page {
    return Page { titulo: "x", canonica: "c" };
}
"#;
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_PARAM_SIN_SEGMENTO"),
        "{:?}",
        codes(&errs)
    );
}

/// En una URL sólo caben Int y String: un Float en un segmento no significa
/// nada, porque no hay una escritura suya que el lenguaje sepa deshacer.
#[test]
fn e_ruta_param_tipo() {
    let src = r#"
@page("/precio/:p")
fn precio(p: Float) -> Page {
    return Page { titulo: "x", canonica: "c" };
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_RUTA_PARAM_TIPO"), "{:?}", codes(&errs));
}

/// Un String en la ruta sí vale: un slug es texto.
#[test]
fn un_segmento_de_texto_vale() {
    let src = r#"
@page("/categoria/:slug")
fn categoria(slug: String) -> Page {
    return Page { titulo: slug, canonica: concat("https://x/", slug) };
}
"#;
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// ------------------------------- el retorno -------------------------------

/// El error que esta regla existe para atajar. El mensaje tiene que explicar la
/// diferencia entre el cuerpo y la página, no sólo negar el tipo.
#[test]
fn e_pagina_retorno_html() {
    let src = r#"
@page("/precios")
fn precios() -> Html {
    return `<h1>Precios</h1>`;
}
"#;
    let errs = check_src(src);
    let e = errs
        .iter()
        .find(|e| e.code == "E_PAGINA_RETORNO")
        .unwrap_or_else(|| panic!("{:?}", codes(&errs)));
    assert!(
        e.message.contains("cuerpo") && e.message.contains("canónica"),
        "el mensaje debe explicar qué le falta a un Html para ser una página: {}",
        e.message
    );
}

#[test]
fn e_pagina_retorno_otro_tipo() {
    let src = r#"
@page("/precios")
fn precios() -> Int {
    return 1;
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_PAGINA_RETORNO"), "{:?}", codes(&errs));
}

/// Una ruta sirve un documento o sirve otra cosa; el tipo de contenido se
/// decide al declararla, no en cada rama.
#[test]
fn e_pagina_retorno_pagina_y_respuesta_a_la_vez() {
    let src = r#"
@page("/precios")
fn precios() -> Page | Response {
    return plainText("x");
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_PAGINA_RETORNO"), "{:?}", codes(&errs));
}

/// Lo que no es HTML devuelve `Response`, que es donde se decide el tipo de
/// contenido.
#[test]
fn una_ruta_puede_servir_lo_que_no_es_html() {
    let src = r#"
@page("/robots.txt")
fn robots() -> Response {
    return plainText("User-agent: *\n");
}

@page("/sitemap.xml")
fn sitemap() -> Response {
    return xmlDoc(`<urlset></urlset>`);
}
"#;
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

/// Sin `Html` no hay sitemap: un texto sin escapar con un '&' dentro rompe el
/// documento, y esa es exactamente la garantía que el tipo ya da —XML escapa los
/// mismos cinco caracteres que HTML—.
#[test]
fn documento_xml_no_acepta_texto_sin_escapar() {
    let src = r#"
fn crudo() -> String { return "a & b"; }

@page("/sitemap.xml")
fn sitemap() -> Response {
    return xmlDoc(crudo());
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

// ---------------------------- Json no es Html ----------------------------

/// El motivo es de corrección y no de estilo: `Html` escapa el '&' a '&amp;', y
/// dentro de un bloque de JSON-LD eso corrompe el JSON. Los dos tipos no se
/// mezclan en ninguna de las dos direcciones.
#[test]
fn un_json_no_es_html() {
    let src = r#"
@page("/")
fn portada() -> Page {
    return Page { titulo: "x", canonica: "c", cuerpo: json("{}") };
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

#[test]
fn un_html_no_es_json() {
    let src = r#"
@page("/")
fn portada() -> Page {
    return Page { titulo: "x", canonica: "c", jsonld: [`<b>no</b>`] };
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_ARG_TYPE"), "{:?}", codes(&errs));
}

// ---------------------------- la query string ----------------------------

/// Convertir texto a número puede fallar, así que el tipo lo dice y el `match`
/// obliga a decidir el valor por defecto.
#[test]
fn entero_devuelve_una_union_que_hay_que_cubrir() {
    let src = r#"
fn pagina() -> Int {
    let n: Int = parseInt(query("pagina"));
    return n;
}
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_LET_TYPE_MISMATCH"), "{:?}", codes(&errs));
}

#[test]
fn entero_con_match_da_un_int() {
    let src = r#"
fn pagina() -> Int {
    return match parseInt(query("pagina")) {
        NotANumber => 1,
        n => n,
    };
}
"#;
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// ------------------------- dónde corre una página -------------------------

/// `@page` implica servidor sin escribirlo: puede leer el almacén directamente,
/// que es lo que hace falta para renderizar lo que va a indexarse.
#[test]
fn una_pagina_puede_leer_el_almacen() {
    let src = r#"
type Modelo = { nombre: String };
store modelos: Modelo;

@page("/")
fn portada() -> Page {
    return Page {
        titulo: "Modelos",
        canonica: "https://x/",
        cuerpo: `<p>{text(len(all(modelos)))}</p>`,
    };
}
"#;
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

/// La otra mitad de la misma regla, y la que de verdad importa: el estado
/// reactivo vive en el navegador y no existe cuando se renderiza la página.
#[test]
fn una_pagina_no_puede_tocar_estado_reactivo() {
    let src = r#"
reactive mut cuenta = 0;

@page("/")
fn portada() -> Page {
    return Page { titulo: text(cuenta), canonica: "c" };
}
"#;
    let errs = check_src(src);
    assert!(
        has_code(&errs, "E_REACTIVE_OFF_CLIENT"),
        "{:?}",
        codes(&errs)
    );
}

/// Una página no exige política aunque el programa declare identidad: es una URL
/// pública por construcción —se sirve para que la indexen—, así que no hay a
/// quién exigirle nada. Sin esto, un sitio dejaría de compilar entero el día que
/// se le añade una `@session`.
#[test]
fn una_pagina_no_exige_politica_aunque_haya_session() {
    let src = r#"
type Usuario = { nombre: String };

@session
fn quien(token: String) -> Usuario | NoAutorizado {
    return NoAutorizado;
}

@page("/")
fn portada() -> Page {
    return Page { titulo: "x", canonica: "c" };
}
"#;
    let errs = check_src(src);
    assert!(errs.is_empty(), "{:?}", codes(&errs));
}

// ---------------------------- rutas repetidas ----------------------------

#[test]
fn e_ruta_duplicada() {
    let src = r#"
@page("/precios")
fn precios() -> Page { return Page { titulo: "a", canonica: "c" }; }

@page("/precios")
fn otra() -> Page { return Page { titulo: "b", canonica: "c" }; }
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_RUTA_DUPLICADA"), "{:?}", codes(&errs));
}

/// Lo que choca es a qué URLs responde una ruta, no cómo se escribió: dos huecos
/// con nombres distintos atienden exactamente lo mismo.
#[test]
fn e_ruta_duplicada_aunque_el_hueco_se_llame_distinto() {
    let src = r#"
@page("/modelo/:id")
fn uno(id: Int) -> Page { return Page { titulo: "a", canonica: "c" }; }

@page("/modelo/:slug")
fn otro(slug: String) -> Page { return Page { titulo: "b", canonica: "c" }; }
"#;
    let errs = check_src(src);
    assert!(has_code(&errs, "E_RUTA_DUPLICADA"), "{:?}", codes(&errs));
}
