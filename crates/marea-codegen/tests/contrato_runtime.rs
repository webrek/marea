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
//! LOS DOS RUNTIMES. Cada contrato se ejercita contra el runtime de Node Y el
//! del navegador. Antes solo se miraba el de Node y la copia del navegador no la
//! comprobaba nadie: la misma función podía irse de un lado y quedarse en el
//! otro sin que nada lo dijera —que es exactamente como llegó el `.append()`
//! sobre un `Set` a los seis sitios—. Hoy los dos salen del mismo texto
//! (`nucleo.js`), y esto es lo que lo fija: si alguien vuelve a partirlos, no
//! basta con que uno de los dos siga bien.
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

/// Corre el MISMO guion contra los dos runtimes y devuelve las dos salidas.
///
/// `{RUNTIME}` se sustituye por el módulo de cada uno. El del navegador se
/// escribe con extensión `.mjs`: node lo carga como JavaScript, sin quitar
/// tipos, que es justo como lo carga el navegador. Si alguna anotación se colara
/// en el archivo que servimos como JS plano, esto moriría con un SyntaxError en
/// vez de pasar de largo.
fn en_ambos(nombre: &str, guion: &str) -> Vec<(&'static str, String)> {
    let mut salidas = Vec::new();
    for (quien, runtime, modulo, entrada) in [
        ("node", marea_codegen::runtime_ts(), "runtime.ts", "t.ts"),
        (
            "navegador",
            marea_codegen::browser_rt(),
            "browser.mjs",
            "t.mjs",
        ),
    ] {
        let dir = std::env::temp_dir().join(format!("marea-contrato-{nombre}-{quien}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("no se pudo crear el temporal");
        std::fs::write(dir.join(modulo), runtime).unwrap();
        std::fs::write(
            dir.join(entrada),
            guion.replace("{RUNTIME}", &format!("./{modulo}")),
        )
        .unwrap();
        let salida = Command::new("node")
            .arg(entrada)
            .current_dir(&dir)
            .output()
            .expect("no se pudo lanzar node");
        let texto = String::from_utf8_lossy(&salida.stdout).to_string();
        let err = String::from_utf8_lossy(&salida.stderr).to_string();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            salida.status.success(),
            "el guion falló en el runtime de {quien}:\nstdout: {texto}\nstderr: {err}"
        );
        salidas.push((quien, texto));
    }
    salidas
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
    for (quien, out) in en_ambos(
        "text",
        r#"import { text } from "{RUNTIME}";
const casos = [0, 7, 1435, -250, 1000000, 10000000, 2147483647, -2147483648];
console.log(casos.map((n) => text(n)).join("|"));
// -0 aparece al truncar hacia cero (__div(-1, 2)); debe imprimirse "0", no "-0".
console.log(text(-0));
"#,
    ) {
        let mut lineas = out.lines();
        assert_eq!(
            lineas.next().unwrap(),
            "0|7|1435|-250|1000000|10000000|2147483647|-2147483648",
            "runtime de {quien}"
        );
        assert_eq!(
            lineas.next().unwrap(),
            "0",
            "-0 debe imprimirse sin signo (runtime de {quien})"
        );
    }
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
    for (quien, out) in en_ambos(
        "div",
        r#"import { __div, __rem } from "{RUNTIME}";
const pares = [[7,2],[-7,2],[7,-2],[-7,-2],[1,2],[-1,2],[10000,3]];
console.log(pares.map(([a,b]) => __div(a,b)).join("|"));
console.log(pares.map(([a,b]) => __rem(a,b)).join("|"));
// Y el divisor cero CORTA en los dos, en vez de colar un Infinity dentro de un Int.
try { __div(1, 0); console.log("no cortó"); } catch { console.log("cortó"); }
try { __rem(1, 0); console.log("no cortó"); } catch { console.log("cortó"); }
"#,
    ) {
        let mut lineas = out.lines();
        // Hacia cero: -7/2 = -3 (si truncara hacia abajo sería -4).
        assert_eq!(
            lineas.next().unwrap(),
            "3|-3|-3|3|0|0|3333",
            "runtime de {quien}"
        );
        // El resto acompaña al truncado: conserva el signo del dividendo.
        assert_eq!(
            lineas.next().unwrap(),
            "1|-1|1|-1|1|-1|1",
            "runtime de {quien}"
        );
        assert_eq!(lineas.next().unwrap(), "cortó", "runtime de {quien}");
        assert_eq!(lineas.next().unwrap(), "cortó", "runtime de {quien}");
    }
}

/// `escape` hace exactamente cinco reemplazos, y el `&` va PRIMERO: si fuera
/// después, `<` produciría `&lt;` y esa `&` se volvería a escapar.
#[test]
fn escape_hace_cinco_reemplazos_con_el_ampersand_primero() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite");
        return;
    }
    let esperado = [
        "&amp;&lt;&gt;&quot;&#39;",
        "&lt;",
        "&amp;amp;",
        "á / \\ ` = ñ",
    ];
    for (quien, out) in en_ambos(
        "escape",
        r#"import { escape } from "{RUNTIME}";
console.log(escape(`&<>"'`));
// El orden: si '&' no fuera el primero, esto saldría con la entidad rota.
console.log(escape("<"));
// No es idempotente, y no debe serlo: escapar dos veces escapa la entidad.
console.log(escape(escape("&")));
// Lo que NO toca: el resto pasa tal cual (acentos, barras, espacios).
console.log(escape("á / \\ ` = ñ"));
"#,
    ) {
        for (i, linea) in out.lines().enumerate() {
            assert_eq!(linea, esperado[i], "línea {i} del runtime de {quien}");
        }
        // La comilla simple es `&#39;`, NO `&apos;`: Vigía ajustó su línea base a
        // esto, así que cambiarlo le movería el dibujo.
        assert!(out.contains("&#39;"), "runtime de {quien}");
        assert!(!out.contains("&apos;"), "runtime de {quien}");
    }
}

/// El núcleo reactivo es UNO. Un signal invalida a quien lo leyó y el effect se
/// vuelve a ejecutar con el valor fresco; un memo es perezoso y se recalcula.
/// Es la parte que el `.append()` sobre un `Set` rompía entera, y la que ningún
/// test ejercitaba en el navegador.
#[test]
fn el_nucleo_reactivo_se_comporta_igual_en_los_dos() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite");
        return;
    }
    for (quien, out) in en_ambos(
        "reactivo",
        r#"import { __signal, __memo, __effect } from "{RUNTIME}";
const n = __signal(1);
const doble = __memo(() => n.get() * 2);
const visto = [];
__effect(() => { visto.push(doble.get()); });
n.set(5);
n.set(5);   // mismo valor: no debe re-ejecutar nada
n.set(7);
console.log(visto.join("|"));
// Un memo es derivado: asignarle avisa en vez de perder la escritura en silencio.
try { doble.set(0); console.log("no avisó"); } catch { console.log("avisó"); }
"#,
    ) {
        let mut lineas = out.lines();
        assert_eq!(lineas.next().unwrap(), "2|10|14", "runtime de {quien}");
        assert_eq!(lineas.next().unwrap(), "avisó", "runtime de {quien}");
    }
}
