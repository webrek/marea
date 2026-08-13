//! Contrato del runtime: los comportamientos a los que hay código de terceros
//! clavado.
//!
//! Vigía genera con Marea el SVG de su gráfica de precios y lo tiene en
//! producción, validado contra la versión anterior en TypeScript. Su dibujo
//! depende de tres cosas que hasta ahora eran detalle de implementación nuestro:
//! cómo imprime `text` un entero, hacia dónde trunca la división y qué escapa
//! `escape`. Un cambio en cualquiera de las tres no le rompería el build: le
//! movería el dibujo EN SILENCIO, que es peor.
//!
//! Pidió que le avisáramos antes de tocarlas. Un aviso depende de que alguien se
//! acuerde, así que en vez de prometerlo se fija aquí: si alguna cambia, esto se
//! pone rojo y el aviso sale solo. Cambiarlas sigue siendo legítimo —hay que
//! actualizar este archivo a propósito y decírselo—; lo que deja de ser posible
//! es cambiarlas sin enterarse.
//!
//! Son comprobaciones de RUNTIME, así que se ejercitan con node: buscar cadenas
//! en la plantilla no probaría nada.

use std::process::{Command, Stdio};

fn hay_node() -> bool {
    Command::new("node")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Escribe el runtime y un script que lo ejercita, y devuelve su salida.
fn correr(nombre: &str, script: &str) -> String {
    let dir = std::env::temp_dir().join(format!("marea-contrato-{nombre}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("no se pudo crear el temporal");
    std::fs::write(dir.join("runtime.ts"), marea_codegen::RUNTIME_TS).unwrap();
    std::fs::write(dir.join("t.ts"), script).unwrap();
    let salida = Command::new("node")
        .arg("t.ts")
        .current_dir(&dir)
        .output()
        .expect("no se pudo lanzar node");
    let texto = String::from_utf8_lossy(&salida.stdout).to_string();
    let err = String::from_utf8_lossy(&salida.stderr).to_string();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        salida.status.success(),
        "el script falló:\nstdout: {texto}\nstderr: {err}"
    );
    texto
}

/// `text(Int)` da dígitos planos: sin separador de miles, sin notación
/// científica y sin depender de la configuración regional. Va dentro de
/// atributos SVG (`cy="1435"`), donde un `1,435` no da error: dibuja mal.
#[test]
fn text_de_un_entero_son_digitos_planos() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite");
        return;
    }
    let out = correr(
        "text",
        r#"import { text } from "./runtime.ts";
const casos = [0, 7, 1435, -250, 1000000, 10000000, 2147483647, -2147483648];
console.log(casos.map((n) => text(n)).join("|"));
// -0 aparece al truncar hacia cero (__div(-1, 2)); debe imprimirse "0", no "-0".
console.log(text(-0));
"#,
    );
    let mut lineas = out.lines();
    assert_eq!(
        lineas.next().unwrap(),
        "0|7|1435|-250|1000000|10000000|2147483647|-2147483648"
    );
    assert_eq!(lineas.next().unwrap(), "0", "-0 debe imprimirse sin signo");
}

/// La división entera trunca HACIA CERO, no hacia abajo. `-7/2` es -3, no -4.
/// Las escalas de Vigía dependen de ello, y además es lo que hace `i32.div_s`
/// en el backend WASM: si aquí se redondeara distinto, el mismo programa daría
/// dos dibujos según a qué blanco compiles.
#[test]
fn la_division_entera_trunca_hacia_cero() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite");
        return;
    }
    let out = correr(
        "div",
        r#"import { __div, __rem } from "./runtime.ts";
const pares = [[7,2],[-7,2],[7,-2],[-7,-2],[1,2],[-1,2],[10000,3]];
console.log(pares.map(([a,b]) => __div(a,b)).join("|"));
console.log(pares.map(([a,b]) => __rem(a,b)).join("|"));
"#,
    );
    let mut lineas = out.lines();
    // Hacia cero: -7/2 = -3 (si truncara hacia abajo sería -4).
    assert_eq!(lineas.next().unwrap(), "3|-3|-3|3|0|0|3333");
    // El resto acompaña al truncado: conserva el signo del dividendo.
    assert_eq!(lineas.next().unwrap(), "1|-1|1|-1|1|-1|1");
}

/// `escape` hace exactamente cinco reemplazos, y el `&` va PRIMERO: si fuera
/// después, `<` produciría `&lt;` y esa `&` se volvería a escapar.
#[test]
fn escape_hace_cinco_reemplazos_con_el_ampersand_primero() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite");
        return;
    }
    let out = correr(
        "escape",
        r#"import { escape } from "./runtime.ts";
console.log(escape(`&<>"'`));
// El orden: si '&' no fuera el primero, esto saldría con la entidad rota.
console.log(escape("<"));
// No es idempotente, y no debe serlo: escapar dos veces escapa la entidad.
console.log(escape(escape("&")));
// Lo que NO toca: el resto pasa tal cual (acentos, barras, espacios).
console.log(escape("á / \\ ` = ñ"));
"#,
    );
    let esperado = [
        "&amp;&lt;&gt;&quot;&#39;",
        "&lt;",
        "&amp;amp;",
        "á / \\ ` = ñ",
    ];
    for (i, linea) in out.lines().enumerate() {
        assert_eq!(linea, esperado[i], "línea {i}");
    }
    // La comilla simple es `&#39;`, NO `&apos;`: Vigía ajustó su línea base a
    // esto, así que cambiarlo le movería el dibujo.
    assert!(out.contains("&#39;"));
    assert!(!out.contains("&apos;"));
}
