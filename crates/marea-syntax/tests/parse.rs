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
    assert_eq!(ks, vec![TokenKind::Int(1), TokenKind::Int(2), TokenKind::Eof]);
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
    let Item::Fn(f) = &m.items[0] else {
        panic!()
    };
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
fn ubicacion_invalida_es_error() {
    let err = parse("@servidor fn v() {}").unwrap_err();
    assert!(err.message.contains("ubicación desconocida"));
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
    let Expr::Record { type_name, fields, .. } = &l.value else {
        panic!("se esperaba un literal de registro")
    };
    assert_eq!(type_name.as_deref(), Some("Punto"));
    assert_eq!(fields.len(), 2);
}

#[test]
fn parse_literal_de_lista() {
    let m = parse("@client fn f() { let xs = [1, 2, 3]; print(xs); }").unwrap();
    let Item::Fn(f) = &m.items[0] else { panic!() };
    let Stmt::Let(l) = &f.body.stmts[0] else { panic!() };
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
    let Stmt::Let(l) = &f.body.stmts[0] else { panic!() };
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
    let Stmt::Let(l) = &f.body.stmts[0] else { panic!() };
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
    let Stmt::Let(l) = &f.body.stmts[0] else { panic!() };
    let Expr::Index { object, .. } = &l.value else {
        panic!("se esperaba un indexado")
    };
    assert!(matches!(object.as_ref(), Expr::List { .. }));
}

#[test]
fn parse_asignacion_y_efecto() {
    let m = parse("@client fn f() { reactive mut n = 0; effect { print(n); } n = n + 1; }").unwrap();
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
    let src = format!("fn f() -> Int {{ return {}1{}; }}", "(".repeat(2000), ")".repeat(2000));
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
    assert!(errors.len() >= 2, "errores: {:?}", errors.iter().map(|e| &e.message).collect::<Vec<_>>());
    // 'c' es válida y debe aparecer en el módulo parcial.
    assert!(
        module.items.iter().any(|it| matches!(it, Item::Fn(f) if f.name == "c")),
        "el item válido 'c' debe parsearse"
    );
}
