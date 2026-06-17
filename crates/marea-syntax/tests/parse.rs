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
