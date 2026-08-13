//! Tests del resolvedor del grafo de módulos.
//!
//! Los archivos que hace falta resolver se escriben en un directorio temporal y
//! se borran al terminar, incluso si el test falla: una prueba que sigue rutas
//! del disco no tiene por qué dejar rastro en el repositorio.

use marea_syntax::{resolve_program, Clase, Program};
use std::path::PathBuf;

/// Un directorio temporal con los módulos de un test dentro.
///
/// El nombre lo pone cada test, así que dos que corran a la vez no se pisan. El
/// `Drop` corre también cuando el test entra en pánico, que es justo el caso en
/// el que un `remove_dir_all` al final del cuerpo no llegaría a ejecutarse.
struct Caja {
    dir: PathBuf,
}

impl Caja {
    fn nueva(nombre: &str) -> Caja {
        let dir = std::env::temp_dir().join(format!("marea-modulos-{nombre}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear el directorio temporal");
        Caja { dir }
    }

    fn escribir(&self, rel: &str, contenido: &str) {
        let ruta = self.dir.join(rel);
        if let Some(padre) = ruta.parent() {
            std::fs::create_dir_all(padre).expect("crear el subdirectorio");
        }
        std::fs::write(&ruta, contenido).expect("escribir el módulo");
    }

    fn ruta(&self, rel: &str) -> PathBuf {
        self.dir.join(rel)
    }
}

impl Drop for Caja {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Los módulos del programa por su nombre, en el orden en que quedaron.
fn nombres(p: &Program) -> Vec<&str> {
    p.modulos.iter().map(|m| m.nombre.as_str()).collect()
}

#[test]
fn import_simple() {
    let c = Caja::nueva("import-simple");
    c.escribir(
        "usuarios.mar",
        r#"type Usuario = { nombre: String };
"#,
    );
    c.escribir(
        "main.mar",
        r#"import { Usuario } from "./usuarios.mar";
fn main() {}
"#,
    );

    let p = resolve_program(&c.ruta("main.mar")).expect("debería resolver");
    assert_eq!(nombres(&p), ["usuarios.mar", "main.mar"]);
    // La dependencia va antes que quien la usa, y el grafo lo dice.
    assert_eq!(p.modulos[0].deps, Vec::<usize>::new());
    assert_eq!(p.modulos[1].deps, vec![0]);
    assert_eq!(p.entrada().nombre, "main.mar");
    // El módulo importado llega parseado, no sólo listado.
    assert_eq!(p.modulos[0].modulo.items.len(), 1);
}

#[test]
fn dos_archivos_importando_el_mismo_tercero() {
    let c = Caja::nueva("diamante");
    c.escribir(
        "comun.mar",
        r#"type Id = { n: Int };
"#,
    );
    c.escribir(
        "a.mar",
        r#"import { Id } from "./comun.mar";
fn a() {}
"#,
    );
    c.escribir(
        "b.mar",
        r#"import { Id } from "./comun.mar";
fn b() {}
"#,
    );
    c.escribir(
        "main.mar",
        r#"import { a } from "./a.mar";
import { b } from "./b.mar";
fn main() {}
"#,
    );

    let p = resolve_program(&c.ruta("main.mar")).expect("debería resolver");
    // Cuatro módulos, no cinco: 'comun.mar' se parsea UNA vez por mucho que lo
    // importen dos, porque su identidad es la ruta canónica.
    assert_eq!(nombres(&p), ["comun.mar", "a.mar", "b.mar", "main.mar"]);
    assert_eq!(p.modulos[1].deps, vec![0]);
    assert_eq!(p.modulos[2].deps, vec![0]);
    assert_eq!(p.modulos[3].deps, vec![1, 2]);
}

// Llegar al mismo archivo por dos textos distintos ('./comun.mar' y
// './sub/../comun.mar') sigue siendo llegar al mismo módulo.
#[test]
fn dos_rutas_distintas_al_mismo_archivo_son_un_solo_modulo() {
    let c = Caja::nueva("misma-ruta");
    c.escribir(
        "comun.mar",
        r#"type Id = { n: Int };
"#,
    );
    c.escribir(
        "sub/nada.mar",
        r#"fn nada() {}
"#,
    );
    c.escribir(
        "a.mar",
        r#"import { Id } from "./sub/../comun.mar";
fn a() {}
"#,
    );
    c.escribir(
        "main.mar",
        r#"import { Id } from "./comun.mar";
import { a } from "./a.mar";
fn main() {}
"#,
    );

    let p = resolve_program(&c.ruta("main.mar")).expect("debería resolver");
    assert_eq!(nombres(&p), ["comun.mar", "a.mar", "main.mar"]);
}

#[test]
fn el_orden_es_topologico() {
    let c = Caja::nueva("topologico");
    c.escribir(
        "base.mar",
        r#"fn base() {}
"#,
    );
    c.escribir(
        "medio.mar",
        r#"import { base } from "./base.mar";
fn medio() {}
"#,
    );
    c.escribir(
        "alto.mar",
        r#"import { medio } from "./medio.mar";
import { base } from "./base.mar";
fn alto() {}
"#,
    );
    c.escribir(
        "main.mar",
        r#"import { alto } from "./alto.mar";
fn main() {}
"#,
    );

    let p = resolve_program(&c.ruta("main.mar")).expect("debería resolver");
    assert_eq!(
        nombres(&p),
        ["base.mar", "medio.mar", "alto.mar", "main.mar"]
    );
    // La propiedad que hace útil el orden: al llegar a un módulo, todo lo que
    // importa ya está resuelto. Con los ids, eso es exactamente dep < id.
    for (indice, m) in p.modulos.iter().enumerate() {
        assert_eq!(m.id, indice, "el id de un módulo es su sitio en el orden");
        for dep in &m.deps {
            assert!(
                *dep < m.id,
                "'{}' depende de '{}', que va después",
                m.nombre,
                p.modulos[*dep].nombre
            );
        }
    }
    // Y la entrada es la última, porque nada del programa la importa.
    assert_eq!(p.entrada().nombre, "main.mar");
}

#[test]
fn ciclo_directo() {
    let c = Caja::nueva("ciclo-directo");
    c.escribir(
        "a.mar",
        r#"import { b } from "./b.mar";
fn a() {}
"#,
    );
    c.escribir(
        "b.mar",
        r#"import { a } from "./a.mar";
fn b() {}
"#,
    );

    let e = resolve_program(&c.ruta("a.mar")).expect_err("un ciclo no puede resolver");
    assert_eq!(e.clase, Clase::Ciclo);
    assert!(
        e.mensaje.contains("a.mar -> b.mar -> a.mar"),
        "el error debe traer la cadena entera:\n{}",
        e.mensaje
    );
    // Y señalar el import que la cierra, en el archivo donde está escrito.
    assert!(e.mensaje.contains("b.mar, línea 1"), "{}", e.mensaje);
}

#[test]
fn ciclo_indirecto_de_tres() {
    let c = Caja::nueva("ciclo-tres");
    c.escribir(
        "a.mar",
        r#"import { b } from "./b.mar";
fn a() {}
"#,
    );
    c.escribir(
        "b.mar",
        r#"import { c } from "./c.mar";
fn b() {}
"#,
    );
    c.escribir(
        "c.mar",
        r#"import { a } from "./a.mar";
fn c() {}
"#,
    );

    let e = resolve_program(&c.ruta("a.mar")).expect_err("un ciclo no puede resolver");
    assert_eq!(e.clase, Clase::Ciclo);
    assert!(
        e.mensaje.contains("a.mar -> b.mar -> c.mar -> a.mar"),
        "{}",
        e.mensaje
    );
}

#[test]
fn un_modulo_que_se_importa_a_si_mismo_es_un_ciclo() {
    let c = Caja::nueva("ciclo-propio");
    c.escribir(
        "a.mar",
        r#"import { a } from "./a.mar";
fn a() {}
"#,
    );

    let e = resolve_program(&c.ruta("a.mar")).expect_err("un ciclo no puede resolver");
    assert_eq!(e.clase, Clase::Ciclo);
    assert!(e.mensaje.contains("a.mar -> a.mar"), "{}", e.mensaje);
}

// Un ciclo que no toca la entrada se reporta con la cadena del ciclo, no con el
// camino que se anduvo para llegar hasta él.
#[test]
fn el_ciclo_se_reporta_sin_el_camino_de_entrada() {
    let c = Caja::nueva("ciclo-lejos");
    c.escribir(
        "main.mar",
        r#"import { b } from "./b.mar";
fn main() {}
"#,
    );
    c.escribir(
        "b.mar",
        r#"import { c } from "./c.mar";
fn b() {}
"#,
    );
    c.escribir(
        "c.mar",
        r#"import { b } from "./b.mar";
fn c() {}
"#,
    );

    let e = resolve_program(&c.ruta("main.mar")).expect_err("un ciclo no puede resolver");
    assert_eq!(e.clase, Clase::Ciclo);
    assert!(
        e.mensaje.contains("b.mar -> c.mar -> b.mar"),
        "{}",
        e.mensaje
    );
    assert!(!e.mensaje.contains("main.mar ->"), "{}", e.mensaje);
}

#[test]
fn archivo_inexistente() {
    let c = Caja::nueva("inexistente");
    c.escribir(
        "main.mar",
        r#"import { x } from "./no-esta.mar";
fn main() {}
"#,
    );

    let e = resolve_program(&c.ruta("main.mar")).expect_err("no hay tal archivo");
    assert_eq!(e.clase, Clase::NoSePudoLeer);
    assert!(e.mensaje.contains("no-esta.mar"), "{}", e.mensaje);
    // El error señala el import, no el archivo que no existe: el import es lo
    // único de los dos que se puede abrir en un editor.
    assert!(e.mensaje.contains("main.mar, línea 1"), "{}", e.mensaje);
}

#[test]
fn archivo_de_entrada_inexistente() {
    let c = Caja::nueva("entrada-inexistente");
    let e = resolve_program(&c.ruta("no-esta.mar")).expect_err("no hay tal archivo");
    assert_eq!(e.clase, Clase::NoSePudoLeer);
    assert!(e.mensaje.contains("no-esta.mar"), "{}", e.mensaje);
}

#[test]
fn nombre_no_exportado() {
    let c = Caja::nueva("no-exportado");
    c.escribir(
        "usuarios.mar",
        r#"fn getUser() {}
"#,
    );
    c.escribir(
        "main.mar",
        r#"import { getUser, getAdmin } from "./usuarios.mar";
fn main() {}
"#,
    );

    let e = resolve_program(&c.ruta("main.mar")).expect_err("getAdmin no existe");
    assert_eq!(e.clase, Clase::NoExportado);
    assert!(e.mensaje.contains("no exporta 'getAdmin'"), "{}", e.mensaje);
    // El cursor cae sobre el nombre que falla, no sobre el import entero:
    // 'getAdmin' empieza en la columna 19 y mide ocho caracteres.
    assert!(e.mensaje.contains("línea 1, columna 19"), "{}", e.mensaje);
    assert!(
        e.mensaje.contains("^^^^^^^^") && !e.mensaje.contains("^^^^^^^^^"),
        "{}",
        e.mensaje
    );
}

// Todo elemento de nivel superior exporta: no hay `pub` en el lenguaje. Así que
// tipos, funciones, `let` y `store` valen igual como nombre importado.
#[test]
fn exportan_todos_los_elementos_de_nivel_superior() {
    let c = Caja::nueva("todo-exporta");
    c.escribir(
        "cosas.mar",
        r#"type T = { n: Int };
let v = 1;
store s: T;
fn f() {}
"#,
    );
    c.escribir(
        "main.mar",
        r#"import { T, v, s, f } from "./cosas.mar";
fn main() {}
"#,
    );

    let p = resolve_program(&c.ruta("main.mar")).expect("debería resolver");
    assert_eq!(nombres(&p), ["cosas.mar", "main.mar"]);
}

#[test]
fn un_modulo_que_no_parsea_da_error_de_sintaxis_con_su_archivo() {
    let c = Caja::nueva("sintaxis");
    c.escribir(
        "roto.mar",
        r#"fn f( {
"#,
    );
    c.escribir(
        "main.mar",
        r#"import { f } from "./roto.mar";
fn main() {}
"#,
    );

    let e = resolve_program(&c.ruta("main.mar")).expect_err("roto.mar no parsea");
    assert_eq!(e.clase, Clase::Sintaxis);
    // El error apunta al archivo que no parsea, que ya no se sobreentiende.
    assert!(e.mensaje.contains("roto.mar, línea 1"), "{}", e.mensaje);
}

#[test]
fn la_ruta_debe_empezar_por_punto() {
    let c = Caja::nueva("no-relativa");
    c.escribir(
        "usuarios.mar",
        r#"fn getUser() {}
"#,
    );
    c.escribir(
        "main.mar",
        r#"import { getUser } from "usuarios.mar";
fn main() {}
"#,
    );

    let e = resolve_program(&c.ruta("main.mar")).expect_err("falta el './'");
    assert_eq!(e.clase, Clase::RutaNoRelativa);
    assert!(e.mensaje.contains("paquetes"), "{}", e.mensaje);
}

#[test]
fn la_ruta_absoluta_se_rechaza() {
    let c = Caja::nueva("absoluta");
    // Se rechaza antes de tocar el disco, así que da igual que exista o no.
    c.escribir(
        "main.mar",
        r#"import { getUser } from "/tmp/usuarios.mar";
fn main() {}
"#,
    );

    let e = resolve_program(&c.ruta("main.mar")).expect_err("las absolutas no valen");
    assert_eq!(e.clase, Clase::RutaNoRelativa);
    assert!(e.mensaje.contains("portables"), "{}", e.mensaje);
}

// La raíz del programa es el directorio del archivo de entrada. Salirse de ella
// con '..' es un error: si no, dónde está guardado un programa cambiaría lo que
// ese programa significa.
#[test]
fn un_modulo_fuera_del_arbol_se_rechaza() {
    let c = Caja::nueva("fuera-arbol");
    c.escribir(
        "afuera.mar",
        r#"fn afuera() {}
"#,
    );
    c.escribir(
        "dentro/main.mar",
        r#"import { afuera } from "../afuera.mar";
fn main() {}
"#,
    );

    let e = resolve_program(&c.ruta("dentro/main.mar")).expect_err("queda fuera del árbol");
    assert_eq!(e.clase, Clase::FueraDelArbol);
    assert!(e.mensaje.contains("fuera del programa"), "{}", e.mensaje);
}

// Un enlace simbólico resuelve a su destino real, así que tampoco es una puerta
// trasera para sacar el programa de su árbol.
#[cfg(unix)]
#[test]
fn un_enlace_que_apunta_fuera_del_arbol_se_rechaza() {
    let c = Caja::nueva("symlink");
    c.escribir(
        "afuera.mar",
        r#"fn afuera() {}
"#,
    );
    c.escribir(
        "dentro/main.mar",
        r#"import { afuera } from "./enlace.mar";
fn main() {}
"#,
    );
    std::os::unix::fs::symlink(c.ruta("afuera.mar"), c.ruta("dentro/enlace.mar"))
        .expect("crear el enlace");

    let e = resolve_program(&c.ruta("dentro/main.mar")).expect_err("el enlace escapa");
    assert_eq!(e.clase, Clase::FueraDelArbol);
}

// La resolución es recursiva: una cadena patológica tiene que salir por un error
// ordinario y no por un desbordamiento de pila, igual que hace el parser con las
// expresiones demasiado anidadas.
#[test]
fn una_cadena_de_imports_demasiado_larga_no_desborda_la_pila() {
    let c = Caja::nueva("profundidad");
    let ultimo = 80;
    for i in 0..=ultimo {
        let mut contenido = String::new();
        if i < ultimo {
            let sig = i + 1;
            contenido.push_str(&format!("import {{ f{sig} }} from \"./m{sig}.mar\";\n"));
        }
        contenido.push_str(&format!("fn f{i}() {{}}\n"));
        c.escribir(&format!("m{i}.mar"), &contenido);
    }

    let e = resolve_program(&c.ruta("m0.mar")).expect_err("la cadena pasa del máximo");
    assert_eq!(e.clase, Clase::DemasiadoProfundo);
}

// El programa de examples/modulos/ es el ejemplo que enseña esta fase: si deja
// de resolver, el ejemplo miente.
#[test]
fn el_ejemplo_de_examples_modulos_resuelve() {
    let raiz = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let p = resolve_program(&raiz.join("../../examples/modulos/tienda.mar"))
        .expect("el ejemplo debería resolver");
    assert_eq!(nombres(&p), ["usuarios.mar", "catalogo.mar", "tienda.mar"]);
}
