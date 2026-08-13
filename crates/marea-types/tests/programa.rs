//! Verificación de PROGRAMAS: varios módulos unidos por `import`.
//!
//! Lo que se fija aquí es el aislamiento, que es la mitad del valor de tener
//! módulos: un módulo ve lo que importó y NO ve el resto. Y lo que es del
//! programa entero —la identidad, los nombres de almacén— se comprueba una vez
//! para todos, no módulo a módulo.

use marea_syntax::program::resolve_program;
use marea_types::check_program;

/// Escribe unos módulos en un temporal y chequea el programa desde el primero.
/// Devuelve `(nombre_del_módulo, código)` por cada error.
fn check_modulos(caso: &str, archivos: &[(&str, &str)]) -> Vec<(String, String)> {
    let dir = std::env::temp_dir().join(format!("marea-prog-{caso}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (nombre, src) in archivos {
        std::fs::write(dir.join(nombre), src).unwrap();
    }
    let programa = resolve_program(&dir.join(archivos[0].0)).expect("el grafo debe resolver");
    let errores = check_program(&programa);
    let out = errores
        .iter()
        .map(|e| {
            (
                programa.modulos[e.modulo].nombre.clone(),
                e.error.code.clone(),
            )
        })
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn codigos(errs: &[(String, String)]) -> Vec<&str> {
    errs.iter().map(|(_, c)| c.as_str()).collect()
}

#[test]
fn un_nombre_importado_se_usa_como_propio() {
    let errs = check_modulos(
        "ok",
        &[
            (
                "a.mar",
                "import { Usuario, saludo } from \"./b.mar\";\n\
                 fn hola(u: Usuario) -> String { return saludo(u.nombre); }\n",
            ),
            (
                "b.mar",
                "type Usuario = { nombre: String };\n\
                 fn saludo(n: String) -> String { return concat(\"hola \", n); }\n",
            ),
        ],
    );
    assert!(errs.is_empty(), "{:?}", codigos(&errs));
}

/// El aislamiento: lo que NO se importó no está. Sin esto, `import` sería
/// decorativo y los módulos compartirían un espacio de nombres global.
#[test]
fn lo_que_no_se_importa_no_se_ve() {
    let errs = check_modulos(
        "aislado",
        &[
            (
                "a.mar",
                // Importa Usuario pero NO 'saludo', y aun así la llama.
                "import { Usuario } from \"./b.mar\";\n\
                 fn hola(u: Usuario) -> String { return saludo(u.nombre); }\n",
            ),
            (
                "b.mar",
                "type Usuario = { nombre: String };\n\
                 fn saludo(n: String) -> String { return n; }\n",
            ),
        ],
    );
    assert_eq!(codigos(&errs), vec!["E_UNRESOLVED_NAME"]);
    assert_eq!(errs[0].0, "a.mar", "el error es de quien la llama");
}

/// Un error se atribuye al archivo donde está, no al que nombraste en la orden.
#[test]
fn el_error_lleva_su_archivo() {
    let errs = check_modulos(
        "atribucion",
        &[
            (
                "a.mar",
                "import { f } from \"./b.mar\";\nfn g() -> Int { return f(); }\n",
            ),
            ("b.mar", "fn f() -> Int { return \"no soy un Int\"; }\n"),
        ],
    );
    assert_eq!(codigos(&errs), vec!["E_RETURN_TYPE_MISMATCH"]);
    assert_eq!(errs[0].0, "b.mar");
}

/// La identidad es del PROGRAMA: el handler puede exigirla en un módulo y
/// resolverse en otro.
#[test]
fn la_session_cruza_los_modulos() {
    let errs = check_modulos(
        "sesion-ok",
        &[
            (
                "app.mar",
                "import { Usuario } from \"./auth.mar\";\n\
                 @server(u: Usuario) fn borrar(i: Int) { print(i); }\n\
                 @server(Public) fn feed() -> Int { return 1; }\n",
            ),
            (
                "auth.mar",
                "type Usuario = { nombre: String };\n\
                 @session fn quien(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }\n",
            ),
        ],
    );
    assert!(errs.is_empty(), "{:?}", codigos(&errs));
}

/// Y si la hay, exige política aunque el handler viva en otro archivo.
#[test]
fn la_session_de_otro_modulo_exige_politica_aqui() {
    let errs = check_modulos(
        "sesion-exige",
        &[
            (
                "app.mar",
                "import { Usuario } from \"./auth.mar\";\n@server fn ventas() -> Int { return 1; }\n",
            ),
            (
                "auth.mar",
                "type Usuario = { nombre: String };\n\
                 @session fn quien(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }\n",
            ),
        ],
    );
    assert_eq!(codigos(&errs), vec!["E_SERVER_SIN_POLITICA"]);
    assert_eq!(errs[0].0, "app.mar");
}

/// Dos `@session` en archivos distintos siguen siendo dos.
#[test]
fn dos_sessions_en_modulos_distintos_es_error() {
    let errs = check_modulos(
        "sesion-doble",
        &[
            (
                "a.mar",
                "import { Usuario } from \"./b.mar\";\n\
                 type Otro = { x: Int };\n\
                 @session fn quienA(t: String) -> Otro | NoAutorizado { return NoAutorizado; }\n",
            ),
            (
                "b.mar",
                "type Usuario = { nombre: String };\n\
                 @session fn quienB(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }\n",
            ),
        ],
    );
    assert!(
        codigos(&errs).contains(&"E_SESSION_DUPLICADA"),
        "{:?}",
        codigos(&errs)
    );
}

/// Dos módulos con el mismo nombre de almacén escribirían en la misma tabla.
#[test]
fn dos_almacenes_con_el_mismo_nombre_en_modulos_distintos() {
    let errs = check_modulos(
        "store-doble",
        &[
            (
                "a.mar",
                "import { P } from \"./b.mar\";\n\
                 type Q = { y: Int };\n\
                 store cosas: Q;\n\
                 @server fn f() -> List<Q> { return all(cosas); }\n",
            ),
            ("b.mar", "type P = { x: Int };\nstore cosas: P;\n"),
        ],
    );
    assert!(
        codigos(&errs).contains(&"E_DUPLICATE_STORE"),
        "{:?}",
        codigos(&errs)
    );
}

/// Importar un nombre que además se declara aquí: en el bundle serían la misma
/// declaración de primer nivel.
#[test]
fn importar_lo_que_ya_declaras_es_colision() {
    let errs = check_modulos(
        "colision",
        &[
            (
                "a.mar",
                "import { Usuario } from \"./b.mar\";\n\
                 type Usuario = { otra: Int };\n\
                 fn f(u: Usuario) -> Int { return u.otra; }\n",
            ),
            ("b.mar", "type Usuario = { nombre: String };\n"),
        ],
    );
    assert!(
        codigos(&errs).contains(&"E_IMPORT_COLISIONA"),
        "{:?}",
        codigos(&errs)
    );
}

/// Los ejemplos multi-módulo del repo tienen que tipar, igual que los de un
/// archivo. Estuvieron sin verificar porque el test dorado recorre
/// `examples/*.mar` y no entra en subdirectorios: `tienda.mar` llevaba un
/// `{!String}` que nadie había mirado.
#[test]
fn los_ejemplos_multimodulo_tipan() {
    let raiz = format!("{}/../../examples/modulos", env!("CARGO_MANIFEST_DIR"));
    let entrada = std::path::Path::new(&raiz).join("tienda.mar");
    let programa = resolve_program(&entrada).expect("el grafo de examples/modulos debe resolver");
    let errores = check_program(&programa);
    assert!(
        errores.is_empty(),
        "examples/modulos no tipa: {:?}",
        errores
            .iter()
            .map(|e| (programa.modulos[e.modulo].nombre.as_str(), &e.error.code))
            .collect::<Vec<_>>()
    );
    assert!(programa.modulos.len() >= 3);
}
