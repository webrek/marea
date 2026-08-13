//! Tests de integración del lexer y el parser de Marea.

use marea_syntax::ast::*;
use marea_syntax::{parse, Lexer, TokenKind};

fn kinds(src: &str) -> Vec<TokenKind> {
    Lexer::tokenize(src)
        .unwrap()
        .into_iter()
        .map(|t| t.kind)
        .collect()
}

#[test]
fn lex_keywords_y_operadores() {
    let ks = kinds("fn let reactive -> => == != <= >= && || | @");
    assert_eq!(
        ks,
        vec![
            TokenKind::Fn,
            TokenKind::Let,
            TokenKind::Reactive,
            TokenKind::Arrow,
            TokenKind::FatArrow,
            TokenKind::EqEq,
            TokenKind::BangEq,
            TokenKind::Le,
            TokenKind::Ge,
            TokenKind::AmpAmp,
            TokenKind::PipePipe,
            TokenKind::Pipe,
            TokenKind::At,
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lex_numeros_cadenas_bools() {
    let ks = kinds(r#"42 2.5 1_000 "hola\n" true false"#);
    assert_eq!(
        ks,
        vec![
            TokenKind::Int(42),
            TokenKind::Float(2.5),
            TokenKind::Int(1000),
            TokenKind::Str("hola\n".to_string()),
            TokenKind::Bool(true),
            TokenKind::Bool(false),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn lex_ignora_comentarios() {
    let ks = kinds("1 // de línea\n /* de\n bloque */ 2");
    assert_eq!(
        ks,
        vec![TokenKind::Int(1), TokenKind::Int(2), TokenKind::Eof]
    );
}

#[test]
fn lex_punto_no_es_flotante() {
    // 'x.campo' no debe leerse como número flotante.
    let ks = kinds("x.campo");
    assert_eq!(
        ks,
        vec![
            TokenKind::Ident("x".to_string()),
            TokenKind::Dot,
            TokenKind::Ident("campo".to_string()),
            TokenKind::Eof,
        ]
    );
}

#[test]
fn parse_fn_servidor_con_tipo_union() {
    let m = parse(
        r#"
        @server
        fn getUser(id: UserId) -> User | NotFound {
            let u = db.users.find(id);
            return u;
        }
        "#,
    )
    .unwrap();

    assert_eq!(m.items.len(), 1);
    let Item::Fn(f) = &m.items[0] else {
        panic!("se esperaba una función");
    };
    assert_eq!(f.name, "getUser");
    assert_eq!(f.location, Some(Location::Server));
    assert_eq!(f.params.len(), 1);
    assert_eq!(f.params[0].name, "id");

    // El tipo de retorno es una unión de dos variantes.
    match f.return_type.as_ref().unwrap() {
        Type::Union { variants, .. } => assert_eq!(variants.len(), 2),
        other => panic!("se esperaba un tipo unión, se obtuvo {:?}", other),
    }
}

#[test]
fn parse_reactive_y_precedencia() {
    let m = parse("@client fn v() { reactive total = 1 + 2 * 3; }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let Stmt::Let(let_stmt) = &f.body.stmts[0] else {
        panic!("se esperaba un let")
    };
    assert!(let_stmt.reactive);
    assert_eq!(let_stmt.name, "total");

    // 1 + (2 * 3): la raíz debe ser '+', con '*' a la derecha.
    let Expr::Binary { op, right, .. } = &let_stmt.value else {
        panic!("se esperaba binaria")
    };
    assert_eq!(*op, BinOp::Add);
    assert!(matches!(
        right.as_ref(),
        Expr::Binary { op: BinOp::Mul, .. }
    ));
}

#[test]
fn parse_match_e_if() {
    let m = parse(
        r#"
        @client
        fn vista(u: User) {
            match u {
                NotFound => render("no existe"),
                _ => render(u.nombre),
            }
            if u.activo { ping(); } else { nada(); }
        }
        "#,
    )
    .unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert_eq!(f.body.stmts.len(), 2);
    assert!(matches!(&f.body.stmts[0], Stmt::Expr(Expr::Match { .. })));
    assert!(matches!(&f.body.stmts[1], Stmt::Expr(Expr::If { .. })));
}

#[test]
fn parse_error_reporta_posicion() {
    // Falta el ';' final.
    let err = parse("@client fn v() { let x = 1 }").unwrap_err();
    assert!(err.message.contains("';'"), "mensaje: {}", err.message);
}

#[test]
fn anotacion_invalida_es_error() {
    let err = parse("@servidor fn v() {}").unwrap_err();
    assert!(
        err.message.contains("anotación desconocida"),
        "{}",
        err.message
    );
    // El mensaje enumera las cuatro que sí existen, @session incluida.
    assert!(err.message.contains("@session"), "{}", err.message);
}

// --- política: quién puede cruzar la frontera ---

#[test]
fn parse_politica_en_server() {
    let m = parse("@server(Usuario) fn borrar(i: Int) {}").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert_eq!(f.location, Some(Location::Server));
    let Some(Type::Name { name, .. }) = &f.politica else {
        panic!("sin política: {:?}", f.politica)
    };
    assert_eq!(name, "Usuario");
}

#[test]
fn server_sin_politica_parsea_igual() {
    // La gramática la deja opcional; que falte lo juzga el verificador, que es
    // quien sabe si el programa declaró identidad.
    let m = parse("@server fn feed() {}").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert!(f.politica.is_none());
    assert!(!f.es_session);
}

#[test]
fn parse_session() {
    let m =
        parse("@session fn quien(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }")
            .unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert!(f.es_session);
    // No es una ubicación: no se anota como @server ni se registra por RPC.
    assert_eq!(f.location, None);
}

#[test]
fn politica_sin_cerrar_es_error() {
    let err = parse("@server(Usuario fn f() {}").unwrap_err();
    assert!(err.message.contains("')'"), "{}", err.message);
}

// --- registros y listas (Fase 0: contrato AST + desambiguación) ---

#[test]
fn parse_tipo_registro() {
    let m = parse("type Punto = { x: Int, y: Int };").unwrap();
    let Item::Type(td) = &m.items[0] else {
        panic!("se esperaba un type")
    };
    let Type::Record { fields, .. } = &td.aliased else {
        panic!("se esperaba un tipo registro")
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].name, "x");
    assert_eq!(fields[1].name, "y");
}

#[test]
fn parse_literal_de_registro() {
    let m = parse(r#"@client fn f() { let u = Punto { x: 1, y: 2 }; print(u); }"#).unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let Stmt::Let(l) = &f.body.stmts[0] else {
        panic!("se esperaba un let")
    };
    let Expr::Record {
        type_name, fields, ..
    } = &l.value
    else {
        panic!("se esperaba un literal de registro")
    };
    assert_eq!(type_name.as_deref(), Some("Punto"));
    assert_eq!(fields.len(), 2);
}

#[test]
fn parse_literal_de_lista() {
    let m = parse("@client fn f() { let xs = [1, 2, 3]; print(xs); }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let Stmt::Let(l) = &f.body.stmts[0] else {
        panic!()
    };
    let Expr::List { elements, .. } = &l.value else {
        panic!("se esperaba una lista")
    };
    assert_eq!(elements.len(), 3);
}

#[test]
fn regresion_if_no_es_registro() {
    // 'if ready {' debe abrir un bloque, NO leerse como 'ready { ... }' registro.
    let m = parse("@client fn f(ready: Bool) { if ready { ping(); } }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert!(matches!(&f.body.stmts[0], Stmt::Expr(Expr::If { .. })));
}

#[test]
fn registro_dentro_de_parentesis_y_member() {
    // El flag se resetea dentro de '(' : el registro se parsea y luego '.x'.
    let m = parse("@client fn f() { let v = (Punto { x: 1, y: 2 }).x; print(v); }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let Stmt::Let(l) = &f.body.stmts[0] else {
        panic!()
    };
    assert!(matches!(&l.value, Expr::Member { .. }));
}

#[test]
fn registro_como_argumento_de_llamada() {
    // El flag se resetea dentro de los argumentos: 'make(Punto { .. })' parsea.
    let m = parse("@client fn f() { make(Punto { x: 1, y: 2 }); }").unwrap();
    assert!(matches!(&m.items[0], Item::Fn(_)));
}

#[test]
fn campo_repetido_en_literal_es_error() {
    let err = parse("@client fn f() { let u = Punto { x: 1, x: 2 }; print(u); }").unwrap_err();
    assert!(err.message.contains("repetido"), "mensaje: {}", err.message);
}

#[test]
fn parse_indexado_de_lista() {
    let m = parse("@client fn f(xs: List) { let a = xs[0]; print(a); }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let Stmt::Let(l) = &f.body.stmts[0] else {
        panic!()
    };
    let Expr::Index { object, index, .. } = &l.value else {
        panic!("se esperaba un indexado")
    };
    assert!(matches!(object.as_ref(), Expr::Ident { .. }));
    assert!(matches!(index.as_ref(), Expr::Int { value: 0, .. }));
}

#[test]
fn parse_lista_literal_indexada() {
    // '[10, 20, 30][1]' = lista literal seguida de indexado.
    let m = parse("@client fn f() { let a = [10, 20, 30][1]; print(a); }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let Stmt::Let(l) = &f.body.stmts[0] else {
        panic!()
    };
    let Expr::Index { object, .. } = &l.value else {
        panic!("se esperaba un indexado")
    };
    assert!(matches!(object.as_ref(), Expr::List { .. }));
}

#[test]
fn parse_asignacion_y_efecto() {
    let m =
        parse("@client fn f() { reactive mut n = 0; effect { print(n); } n = n + 1; }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert!(matches!(&f.body.stmts[0], Stmt::Let(l) if l.reactive && l.mutable));
    assert!(matches!(&f.body.stmts[1], Stmt::Effect { .. }));
    assert!(matches!(&f.body.stmts[2], Stmt::Assign { .. }));
}

#[test]
fn igualdad_no_es_asignacion() {
    // 'n == 1' es comparación (expr-stmt), no asignación.
    let m = parse("@client fn f(n: Int) { n == 1; }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert!(matches!(&f.body.stmts[0], Stmt::Expr(Expr::Binary { .. })));
}

#[test]
fn registro_en_rama_de_match() {
    // El cuerpo de la rama es contexto delimitado: 'P { .. }' es un registro.
    let m = parse("type P = { x: Int };\n@client fn f(u: Int) { let r = match u { _ => P { x: 1 } }; print(r); }").unwrap();
    assert!(matches!(&m.items[1], Item::Fn(_)));
}

#[test]
fn anidamiento_profundo_no_paniquea() {
    // Antes: stack overflow (SIGABRT). Ahora: SyntaxError ordinario.
    let src = format!(
        "fn f() -> Int {{ return {}1{}; }}",
        "(".repeat(2000),
        ")".repeat(2000)
    );
    let err = parse(&src).unwrap_err();
    assert!(err.message.contains("anidada"), "mensaje: {}", err.message);
}

#[test]
fn recuperacion_reporta_varios_errores() {
    use marea_syntax::parse_recovering;
    // Dos funciones con errores + una válida: deben salir 2 errores y el item
    // válido debe parsearse (módulo parcial).
    let (module, errors) = parse_recovering(
        "@client\nfn a() { let x = ; }\nfn b() { return 1 }\nfn c() -> Int { return 5; }",
    );
    assert!(
        errors.len() >= 2,
        "errores: {:?}",
        errors.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    // 'c' es válida y debe aparecer en el módulo parcial.
    assert!(
        module
            .items
            .iter()
            .any(|it| matches!(it, Item::Fn(f) if f.name == "c")),
        "el item válido 'c' debe parsearse"
    );
}

#[test]
fn recuperacion_no_descarta_item_valido() {
    use marea_syntax::parse_recovering;
    // El error en 'a' (falta ';') no debe consumir la 'fn b' siguiente.
    let (module, errors) = parse_recovering("fn a() { let x = 1 }\nfn b() -> Int { return 5; }");
    assert!(!errors.is_empty());
    assert!(
        module
            .items
            .iter()
            .any(|it| matches!(it, Item::Fn(f) if f.name == "b")),
        "'b' válida debe parsearse, items: {:?}",
        module
            .items
            .iter()
            .filter_map(|it| match it {
                Item::Fn(f) => Some(&f.name),
                _ => None,
            })
            .collect::<Vec<_>>()
    );
}

#[test]
fn parse_store() {
    let m = parse("store almacen: Post;").unwrap();
    assert!(matches!(&m.items[0], Item::Store { .. }));
}

// Los tipos anidan igual que las expresiones y también recursan, pero la guarda
// de profundidad solo cubría `parse_unary`: `List<List<...>>` repetido miles de
// veces desbordaba la pila del proceso. Importa porque el servidor de lenguaje
// parsea texto sin terminar en cada pulsación, así que moría con él.
#[test]
fn un_tipo_demasiado_anidado_no_desborda_la_pila() {
    let src = format!(
        "fn f(x: {}Int{}) -> Int {{ return 1; }}",
        "List<".repeat(20_000),
        ">".repeat(20_000)
    );
    let e = marea_syntax::parse(&src).expect_err("debe rechazarse, no reventar");
    assert!(e.message.contains("anidado"), "mensaje: {}", e.message);
}

#[test]
fn un_registro_demasiado_anidado_no_desborda_la_pila() {
    let src = format!(
        "type T = {}Int{}; fn f(x: T) -> Int {{ return 1; }}",
        "{ a: ".repeat(20_000),
        " }".repeat(20_000)
    );
    let e = marea_syntax::parse(&src).expect_err("debe rechazarse, no reventar");
    assert!(e.message.contains("anidado"), "mensaje: {}", e.message);
}

// La cadena de `else if` recursa por parse_if directamente, saltándose la
// guarda de parse_unary.
#[test]
fn una_cadena_de_else_if_larguisima_no_desborda_la_pila() {
    let mut src = String::from("fn f(n: Int) -> Int { ");
    for _ in 0..20_000 {
        src.push_str("if n > 0 { return 1; } else ");
    }
    src.push_str("{ return 0; } }");
    let e = marea_syntax::parse(&src).expect_err("debe rechazarse, no reventar");
    assert!(e.message.contains("anidad"), "mensaje: {}", e.message);
}

// El anidamiento razonable sigue siendo válido.
#[test]
fn el_anidamiento_normal_de_tipos_sigue_valiendo() {
    let src = format!(
        "fn f(x: {}Int{}) -> Int {{ return 1; }}",
        "List<".repeat(100),
        ">".repeat(100)
    );
    assert!(marea_syntax::parse(&src).is_ok());
}

// --- plantillas de texto ---

#[test]
fn una_plantilla_separa_literales_y_huecos() {
    let m = marea_syntax::parse("fn f(n: Int) -> Html { return `a{n}b`; }").expect("parsea");
    let fuente = format!("{m:?}");
    assert!(fuente.contains("Template"), "{fuente}");
    assert!(fuente.contains("Interp"), "{fuente}");
}

// El hueco se parsea aparte, así que sus spans hay que desplazarlos: si no, un
// error dentro de `{...}` señalaría el principio del archivo.
#[test]
fn los_spans_del_hueco_apuntan_a_su_sitio() {
    let src = "fn f() -> Html {\n    return `hola {noExiste} adios`;\n}";
    let m = marea_syntax::parse(src).expect("parsea");
    let fuente = format!("{m:?}");
    // El identificador del hueco empieza en el byte 35 del archivo, no en 0.
    let pos = src.find("noExiste").unwrap();
    assert!(
        fuente.contains(&format!("start: {pos}")),
        "el span del hueco debe estar desplazado a {pos}: {fuente}"
    );
}

#[test]
fn una_plantilla_sin_cerrar_es_error() {
    let e = marea_syntax::parse("fn f() -> Html { return `hola; }").expect_err("debe fallar");
    assert!(e.message.contains("sin cerrar"), "{}", e.message);
}

#[test]
fn un_hueco_vacio_es_error() {
    let e = marea_syntax::parse("fn f() -> Html { return `a{}b`; }").expect_err("debe fallar");
    assert!(e.message.contains("vacío"), "{}", e.message);
}

// Las llaves y las cadenas dentro del hueco no lo cierran antes de tiempo.
#[test]
fn el_hueco_respeta_llaves_y_cadenas_anidadas() {
    assert!(marea_syntax::parse("fn f() -> Html { return `x{concat(\"}\", \"!\")}y`; }").is_ok());
    assert!(marea_syntax::parse(
        "type P = { a: Int };\nfn f() -> Html { return `x{P { a: 1 }.a}y`; }"
    )
    .is_ok());
}

#[test]
fn parse_politica_con_nombre() {
    // `@server(u: Usuario)` liga la identidad a un nombre usable en el cuerpo.
    let m = parse("@server(u: Usuario) fn publicar(t: String) {}").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert_eq!(f.identidad_bind.as_deref(), Some("u"));
    let Some(Type::Name { name, .. }) = &f.politica else {
        panic!()
    };
    assert_eq!(name, "Usuario");
    // El binding NO es un parámetro: no viaja en la llamada.
    assert_eq!(f.params.len(), 1);
    assert_eq!(f.params[0].name, "t");
}

#[test]
fn politica_sin_nombre_no_liga_nada() {
    let m = parse("@server(Usuario) fn publicar(t: String) {}").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    assert!(f.identidad_bind.is_none());
}
