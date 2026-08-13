//! Pruebas del análisis puro (`analysis`) y de su conversión al protocolo
//! (`conversions`), más el almacén de documentos (`documents`).
//!
//! Usan ejemplos `.mar` reales del repositorio como fixtures para no inventar fuentes
//! que se desincronicen del lenguaje.

use marea_lsp::analysis::{
    analyze, collect_symbols, find_node_at, top_level_index, Node, Severity, SymbolClass,
};
use marea_lsp::conversions::{neutral_to_diagnostic, span_to_location, symbol_to_document_symbol};
use marea_lsp::documents::DocumentStore;
use marea_lsp::line_index::LineIndex;

use lsp_types::{DiagnosticSeverity, NumberOrString, SymbolKind, Uri};
use std::str::FromStr;

/// Lee un ejemplo del directorio `examples/` del workspace.
fn ejemplo(rel: &str) -> String {
    let manifest = env!("CARGO_MANIFEST_DIR");
    let path = format!("{manifest}/../../examples/{rel}");
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("no se pudo leer {path}: {e}"))
}

// ===================== analyze: caminos felices y de error =====================

#[test]
fn codigo_valido_sin_diagnosticos_y_con_modulo() {
    let src = ejemplo("registro.mar");
    let analysis = analyze(&src);
    assert!(
        analysis.module.is_some(),
        "un programa válido debe producir un módulo"
    );
    assert!(
        analysis.diagnostics.is_empty(),
        "un programa válido no debe tener diagnósticos, hubo: {:?}",
        analysis.diagnostics
    );
}

#[test]
fn type_mismatch_un_diagnostico_con_codigo() {
    let src = ejemplo("check_fail/tipo_aritmetico.mar");
    let analysis = analyze(&src);
    // El parseo tuvo éxito: hay módulo.
    assert!(analysis.module.is_some());
    // Exactamente un error de tipo aritmético.
    assert_eq!(analysis.diagnostics.len(), 1, "{:?}", analysis.diagnostics);
    let diag = &analysis.diagnostics[0];
    assert_eq!(diag.severity, Severity::Error);
    assert_eq!(diag.code.as_deref(), Some("E_ARITH_TYPE"));
}

#[test]
fn error_de_sintaxis_no_corre_chequeo_de_tipos() {
    // Con recuperación, el parser produce un módulo parcial y los errores de
    // sintaxis; el chequeo de tipos NO corre mientras haya errores de sintaxis,
    // así que NO debe aparecer ningún diagnóstico con código E_... (de tipos).
    let src = "fn mal( {\n    return 1 + \"x\";\n}\n";
    let analysis = analyze(src);
    assert!(
        !analysis.diagnostics.is_empty(),
        "debe haber al menos un error de sintaxis"
    );
    // Todos los diagnósticos son de sintaxis (sin código E_...): los de tipos
    // no corren mientras la sintaxis esté rota.
    assert!(
        analysis.diagnostics.iter().all(|d| d.code.is_none()),
        "no debe haber diagnósticos de tipos sobre un AST parcial: {:?}",
        analysis.diagnostics
    );
    assert!(analysis
        .diagnostics
        .iter()
        .all(|d| d.severity == Severity::Error));
}

#[test]
fn match_no_exhaustivo_reporta_el_codigo_correcto() {
    let src = ejemplo("check_fail/match_no_exhaustivo.mar");
    let analysis = analyze(&src);
    assert!(analysis.module.is_some());
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.code.as_deref() == Some("E_NON_EXHAUSTIVE_MATCH")),
        "se esperaba E_NON_EXHAUSTIVE_MATCH, hubo: {:?}",
        analysis.diagnostics
    );
}

#[test]
fn frontera_cliente_desde_servidor_reporta_su_codigo() {
    let src = ejemplo("check_fail/frontera.mar");
    let analysis = analyze(&src);
    assert!(analysis.module.is_some());
    assert!(
        analysis
            .diagnostics
            .iter()
            .any(|d| d.code.as_deref() == Some("E_CALL_CLIENT_FROM_SERVER")),
        "se esperaba E_CALL_CLIENT_FROM_SERVER, hubo: {:?}",
        analysis.diagnostics
    );
}

// ===================== find_node_at =====================

#[test]
fn find_node_at_en_offsets_conocidos() {
    let src = ejemplo("hello.mar");
    let module = analyze(&src).module.expect("hello.mar parsea");

    // Dentro del literal de string del `let`: el nodo más interno es la
    // expresión `Str`. (El nombre `saludo` del binding no es una expresión:
    // ahí el nodo más interno sería el propio `Stmt::Let`.)
    let str_off = src.find("\"¡Hola").expect("hay literal de string");
    match find_node_at(&module, str_off + 1) {
        Some(Node::Expr(e)) => {
            assert!(
                matches!(e, marea_syntax::ast::Expr::Str { .. }),
                "se esperaba una expresión de string, fue {e:?}"
            );
        }
        otro => panic!("se esperaba Node::Expr(Str), fue {otro:?}"),
    }

    // Dentro de la llamada `print(saludo)`: el argumento `saludo` es un Ident.
    let arg_off = {
        let call = src.find("print(saludo)").expect("hay llamada print");
        call + "print(".len() // primer byte de `saludo` (el argumento)
    };
    match find_node_at(&module, arg_off) {
        Some(Node::Expr(marea_syntax::ast::Expr::Ident { name, .. })) => {
            assert_eq!(name, "saludo");
        }
        otro => panic!("se esperaba Node::Expr(Ident \"saludo\"), fue {otro:?}"),
    }

    // Un offset en el comentario inicial (antes de cualquier item) no cae en
    // ningún nodo del AST.
    assert!(
        find_node_at(&module, 0).is_none(),
        "el comentario inicial no pertenece a ningún nodo del AST"
    );
}

#[test]
fn find_node_at_desciende_hasta_el_tipo_de_retorno() {
    let src = ejemplo("registro.mar");
    let module = analyze(&src).module.expect("registro.mar parsea");

    // El tipo de retorno `Punto` de `fn origen() -> Punto`.
    let ret_off = {
        let sig = src.find("fn origen() -> Punto").expect("hay firma origen");
        sig + "fn origen() -> ".len()
    };
    match find_node_at(&module, ret_off) {
        Some(Node::Type(t)) => {
            assert!(matches!(
                t,
                marea_syntax::ast::Type::Name { name, .. } if name == "Punto"
            ));
        }
        otro => panic!("se esperaba Node::Type(Name Punto), fue {otro:?}"),
    }
}

// ===================== símbolos e índice de nivel superior =====================

#[test]
fn collect_symbols_lista_funciones_y_tipos() {
    let src = ejemplo("registro.mar");
    let module = analyze(&src).module.expect("registro.mar parsea");
    let symbols = collect_symbols(&module);

    // Hay un type `Punto` y varias funciones.
    assert!(symbols
        .iter()
        .any(|s| s.class == SymbolClass::Type && s.name == "Punto"));
    assert!(symbols
        .iter()
        .any(|s| s.class == SymbolClass::Fn && s.name == "origen"));
    assert!(
        symbols
            .iter()
            .filter(|s| s.class == SymbolClass::Fn)
            .count()
            >= 3
    );
}

#[test]
fn top_level_index_mapea_nombre_a_span() {
    let src = ejemplo("registro.mar");
    let module = analyze(&src).module.expect("registro.mar parsea");
    let index = top_level_index(&module);

    let span = index.get("origen").expect("origen está en el índice");
    // El span debe abarcar la firma de `origen`.
    let texto = &src[span.start..span.end];
    assert!(texto.starts_with("fn origen"), "span de origen = {texto:?}");
}

// ===================== conversions =====================

#[test]
fn neutral_to_diagnostic_lleva_codigo_source_y_rango() {
    let src = ejemplo("check_fail/tipo_aritmetico.mar");
    let analysis = analyze(&src);
    let neutral = &analysis.diagnostics[0];

    let line_index = LineIndex::new(&src);
    let diag = neutral_to_diagnostic(neutral, &line_index, &src);

    assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
    assert_eq!(diag.source.as_deref(), Some("marea"));
    assert_eq!(
        diag.code,
        Some(NumberOrString::String("E_ARITH_TYPE".to_string()))
    );
    // El rango debe coincidir con el del span original traducido.
    assert_eq!(diag.range, line_index.span_to_range(neutral.span, &src));
}

#[test]
fn neutral_to_diagnostic_concatena_notas() {
    // Dos `fn` con el mismo nombre → E_DUPLICATE_ITEM con una nota.
    let src = "fn f() {}\nfn f() {}\n";
    let analysis = analyze(src);
    let neutral = analysis
        .diagnostics
        .iter()
        .find(|d| d.code.as_deref() == Some("E_DUPLICATE_ITEM"))
        .expect("hay duplicado");
    assert!(!neutral.notes.is_empty(), "el duplicado trae una nota");

    let line_index = LineIndex::new(src);
    let diag = neutral_to_diagnostic(neutral, &line_index, src);
    assert!(
        diag.message.contains("nota:"),
        "el mensaje debe incluir la nota concatenada: {:?}",
        diag.message
    );
}

#[test]
fn symbol_to_document_symbol_mapea_clases() {
    let src = ejemplo("registro.mar");
    let module = analyze(&src).module.expect("registro.mar parsea");
    let line_index = LineIndex::new(&src);

    for sym in collect_symbols(&module) {
        let ds = symbol_to_document_symbol(&sym, &line_index, &src);
        assert_eq!(ds.name, sym.name);
        // range y selection_range coinciden (no hay span de nombre).
        assert_eq!(ds.range, ds.selection_range);
        let esperado = match sym.class {
            SymbolClass::Fn => SymbolKind::FUNCTION,
            SymbolClass::Type => SymbolKind::INTERFACE,
            SymbolClass::Let => SymbolKind::VARIABLE,
            SymbolClass::Store => SymbolKind::PROPERTY,
        };
        assert_eq!(ds.kind, esperado);
    }
}

#[test]
fn span_to_location_usa_la_uri_y_el_rango() {
    let src = ejemplo("registro.mar");
    let module = analyze(&src).module.expect("registro.mar parsea");
    let line_index = LineIndex::new(&src);
    let span = *top_level_index(&module).get("origen").unwrap();

    let uri = Uri::from_str("file:///registro.mar").unwrap();
    let loc = span_to_location(uri.clone(), span, &line_index, &src);
    assert_eq!(loc.uri, uri);
    assert_eq!(loc.range, line_index.span_to_range(span, &src));
}

// ===================== DocumentStore =====================

#[test]
fn document_store_abre_actualiza_y_cierra() {
    let mut store = DocumentStore::new();
    assert!(store.is_empty());

    let uri = Uri::from_str("file:///a.mar").unwrap();
    store.open(uri.clone(), "let x = 1\n".to_string(), 1);
    assert_eq!(store.len(), 1);

    let doc = store.get(&uri).expect("a.mar está abierto");
    assert_eq!(doc.version, 1);
    // El índice debe ubicar el inicio de la segunda línea tras el `\n`.
    let pos = doc.line_index.offset_to_position(10, &doc.text);
    assert_eq!(pos.line, 1);

    // Actualización: texto y versión nuevos, índice reconstruido.
    store.update(uri.clone(), "let x = 1\nlet y = 2\n".to_string(), 2);
    let doc = store.get(&uri).unwrap();
    assert_eq!(doc.version, 2);
    assert!(doc.text.contains("let y"));

    // Cierre.
    let cerrado = store.close(&uri);
    assert!(cerrado.is_some());
    assert!(store.is_empty());
}

#[test]
fn document_store_update_sin_apertura_crea() {
    let mut store = DocumentStore::new();
    let uri = Uri::from_str("file:///nuevo.mar").unwrap();
    store.update(uri.clone(), "fn f() {}\n".to_string(), 5);
    let doc = store
        .get(&uri)
        .expect("update crea el documento si no existía");
    assert_eq!(doc.version, 5);
}
