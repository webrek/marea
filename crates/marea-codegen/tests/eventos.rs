//! El modelo de eventos, EJECUTADO.
//!
//! Buscar `data-marea-on-click` en el texto generado no prueba nada: lo que hay
//! que demostrar es que un clic acaba moviendo una `reactive mut` y re-pintando
//! la vista, y que re-pintar no deja oyentes huérfanos —el fallo clásico de este
//! patrón, y el que más cuesta encontrar después—. Así que estas pruebas corren
//! con node y un DOM FALSO escrito a mano: cuarenta líneas de `addEventListener`
//! y `getAttribute`, sin una dependencia nueva en el repositorio.
//!
//! Son dos niveles. El primero ejercita el despachador contra los DOS runtimes
//! (Node y navegador), que salen del mismo `nucleo.js` y no pueden irse uno del
//! otro. El segundo coge el ejemplo tal como se publica —`contador-clic.mar`—,
//! lo pasa por `build-app` y le da clics de verdad.

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

/// Corre un guion en un directorio temporal propio y devuelve lo que imprimió
/// tras la marca `RESULTADO:`.
fn correr(caso: &str, archivos: &[(&str, &str)], entrada: &str) -> String {
    let dir = std::env::temp_dir().join(format!("marea-eventos-{caso}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("no se pudo crear el temporal");
    for (nombre, contenido) in archivos {
        std::fs::write(dir.join(nombre), contenido).expect("no se pudo escribir");
    }
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
        "el guion falló ({caso}):\nstdout: {texto}\nstderr: {err}"
    );
    texto
        .split("RESULTADO:")
        .nth(1)
        .unwrap_or_else(|| panic!("el guion no llegó al final ({caso}):\n{texto}\n{err}"))
        .trim()
        .to_string()
}

/// Lo que el emisor tiene que hacer con `on`, sin necesidad de node: emitirlo
/// SIN `await` y traérselo del runtime.
///
/// El `await` importa más de lo que parece: `on` se llama dentro de la plantilla
/// que pinta la vista, es decir dentro del effect que la monta, y un await ahí
/// suspende el cuerpo antes de que termine de leer sus signals —el rastreo de
/// dependencias se pierde y la vista deja de re-pintarse—.
#[test]
fn on_se_emite_sincrono_y_viene_del_runtime() {
    let src = "@client fn v() -> Html { return `<b {!on(\"click\", fn() { print(1); })}>x</b>`; }";
    let p = marea_codegen::emit(&marea_syntax::parse(src).expect("parsea"));
    assert!(
        p.client.contains(r#"on("click", (async () =>"#),
        "{}",
        p.client
    );
    assert!(!p.client.contains("await on("), "{}", p.client);
    // Viaja en la lista de builtins de SIEMPRE: el despachador no trae Node.
    assert!(p.client.contains("html, on, __div"), "{}", p.client);
    assert!(p.runtime.contains("export function on("), "{}", p.runtime);
    assert!(p.runtime.contains("data-marea-on-"), "{}", p.runtime);
}

/// Un DOM falso: lo justo para que el despachador tenga documento donde
/// engancharse y elementos a los que preguntarles un atributo.
const DOM_FALSO: &str = r#"
export const oyentes = new Map();
export const preventDefault = [];

globalThis.document = {
  addEventListener(tipo, fn, captura) {
    // Marea engancha en fase de CAPTURA: es la única que alcanza a los eventos
    // que no burbujean. Si algún día se cambia, esto lo dice.
    if (captura !== true) throw new Error("se esperaba un oyente en captura: " + tipo);
    if (!oyentes.has(tipo)) oyentes.set(tipo, []);
    oyentes.get(tipo).push(fn);
  },
  getElementById(id) {
    return id === "app" ? app : null;
  },
};

export const app = { innerHTML: "" };

/// Un elemento con los atributos que se le den y, opcionalmente, un padre.
export function elemento(atributos, padre) {
  return {
    getAttribute: (n) => (n in atributos ? atributos[n] : null),
    parentNode: padre ?? null,
  };
}

/// Dispara un evento como lo haría el navegador: los oyentes de la raíz reciben
/// el objetivo, y son ellos los que tienen que encontrar el manejador.
export function disparar(tipo, objetivo) {
  const ev = { type: tipo, target: objetivo, preventDefault: () => preventDefault.push(tipo) };
  for (const fn of oyentes.get(tipo) ?? []) fn(ev);
}

/// Deja correr las microtareas: el re-pintado cuelga de un effect asíncrono.
export function paso() {
  return new Promise((r) => setTimeout(r, 0));
}
"#;

/// Los once eventos se enganchan, se despachan al manejador correcto y respetan
/// las dos reglas que no son obvias: uno solo atiende el evento (el más cercano
/// al objetivo) y los que no burbujean no se buscan hacia arriba.
#[test]
fn el_despacho_por_delegacion_funciona_en_los_dos_runtimes() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite");
        return;
    }
    let guion = r#"
import { on, __podar } from "{RUNTIME}";
import { oyentes, preventDefault, elemento, disparar } from "./dom.mjs";

const EVENTOS = ["click", "submit", "change", "input", "keydown", "keyup", "blur",
                 "pointermove", "pointerdown", "pointerup", "pointerleave"];

const disparados = [];
const atributos = {};
for (const e of EVENTOS) {
  const attr = on(e, () => disparados.push(e));
  const m = /^data-marea-on-([a-z]+)="([^"]+)"$/.exec(attr);
  if (!m) throw new Error("atributo con forma inesperada: " + attr);
  if (m[1] !== e) throw new Error("el atributo no lleva su evento: " + attr);
  atributos["data-marea-on-" + e] = m[2];
}

const boton = elemento(atributos, null);
for (const e of EVENTOS) disparar(e, boton);

// Un hijo SIN manejador: el evento lo atiende el de arriba, como el burbujeo.
const hijo = elemento({}, boton);
disparar("click", hijo);
const trasBurbujeo = disparados.length;

// 'blur' no burbujea: el manejador del padre NO es suyo.
disparar("blur", hijo);
const trasNoBurbujeo = disparados.length;

// Un id que no está en la tabla no revienta: simplemente no hay a quién llamar.
disparar("click", elemento({ "data-marea-on-click": "h999" }, null));

// Podar con un marcado que solo nombra el manejador del clic tira los demás.
__podar(`<button data-marea-on-click="${atributos["data-marea-on-click"]}"></button>`);
disparar("change", boton);
const trasPodar = disparados.length;
disparar("click", boton);

console.log("RESULTADO:" + JSON.stringify({
  disparados,
  trasBurbujeo,
  trasNoBurbujeo,
  trasPodar,
  final: disparados.length,
  // UN oyente por tipo, pase lo que pase: es lo que evita el huérfano.
  oyentesPorTipo: EVENTOS.map((e) => (oyentes.get(e) ?? []).length),
  preventDefault,
}));
"#;

    // El runtime de Node se carga desde una entrada `.ts` y el del navegador
    // desde una `.mjs`: cada uno como lo carga su blanco de verdad. Si a
    // `client.js` se le colara una anotación de tipo, la segunda vuelta moriría
    // con un SyntaxError en vez de pasar de largo.
    for (quien, runtime, modulo, entrada) in [
        ("node", marea_codegen::runtime_ts(), "runtime.ts", "t.ts"),
        (
            "navegador",
            marea_codegen::browser_rt(),
            "browser.mjs",
            "t.mjs",
        ),
    ] {
        let texto = guion.replace("{RUNTIME}", &format!("./{modulo}"));
        let json = correr(
            &format!("despacho-{quien}"),
            &[
                (modulo, runtime),
                ("dom.mjs", DOM_FALSO),
                (entrada, texto.as_str()),
            ],
            entrada,
        );
        // Cada evento llegó a SU manejador, en orden.
        let orden = r#""disparados":["click","submit","change","input","keydown","keyup","blur","pointermove","pointerdown","pointerup","pointerleave""#;
        assert!(
            json.contains(orden),
            "el despacho no llegó a los once eventos ({quien}): {json}"
        );
        // El clic sobre el hijo sube; el blur sobre el hijo no.
        assert!(
            json.contains(r#""trasBurbujeo":12"#),
            "un clic en un hijo debe atenderlo el manejador de arriba ({quien}): {json}"
        );
        assert!(
            json.contains(r#""trasNoBurbujeo":12"#),
            "'blur' no burbujea: no debe subir al padre ({quien}): {json}"
        );
        // Tras podar, 'change' ya no tiene manejador; 'click' sigue vivo.
        assert!(
            json.contains(r#""trasPodar":12"#) && json.contains(r#""final":13"#),
            "podar debe dejar vivo solo lo que el marcado nombra ({quien}): {json}"
        );
        // Un oyente por tipo: doce despachos no añaden ni uno.
        assert!(
            json.contains(r#""oyentesPorTipo":[1,1,1,1,1,1,1,1,1,1,1]"#),
            "debe haber UN oyente por tipo de evento ({quien}): {json}"
        );
        // 'submit' navegaría y se llevaría por delante el estado de la app.
        assert!(
            json.contains(r#""preventDefault":["submit"]"#),
            "'submit' debe cancelar la navegación por defecto ({quien}): {json}"
        );
    }
}

/// El ejemplo publicado, tal cual, pasado por `build-app` y CLICADO: el círculo
/// entero (clic → manejador → asignación a la reactiva → re-pintado).
#[test]
fn el_ejemplo_del_contador_cuenta_clics_de_verdad() {
    if !hay_node() {
        eprintln!("sin node en el PATH: se omite");
        return;
    }
    let raiz = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
    let fuente = std::fs::read_to_string(format!("{raiz}/examples/contador-clic.mar"))
        .expect("no se pudo leer examples/contador-clic.mar");
    // Que TIPA lo fija `marea-types` (recorre todo `examples/`); aquí lo que se
    // demuestra es que además CORRE.
    let module = marea_syntax::parse(&fuente).expect("el ejemplo debe parsear");
    let app = marea_codegen::emit_app(&module);

    let guion = r#"
// El DOM falso va PRIMERO: client.mjs monta la vista al importarse.
import { app, disparar, elemento, oyentes, paso } from "./dom.mjs";
import "./client.mjs";

const ids = (html) => [...html.matchAll(/data-marea-on-click="([^"]+)"/g)].map((m) => m[1]);
const clic = async (id) => { disparar("click", elemento({ "data-marea-on-click": id })); await paso(); };

await paso();
const inicial = app.innerHTML;
const [sumar, reiniciar] = ids(inicial);

await clic(sumar);
const trasUno = app.innerHTML;

// El re-pintado trae manejadores NUEVOS: el botón de antes ya no está en el DOM.
const [sumarDos] = ids(trasUno);
await clic(sumarDos);
const trasDos = app.innerHTML;

// Y el viejo está muerto: si siguiera vivo, esto contaría un clic fantasma.
await clic(sumar);
const trasHuerfano = app.innerHTML;

// El segundo manejador de la misma vista (mismo evento, otro elemento).
await clic(ids(trasHuerfano)[1]);

console.log("RESULTADO:" + JSON.stringify({
  inicial, trasUno, trasDos, trasHuerfano,
  trasReiniciar: app.innerHTML,
  idsDistintos: sumar !== sumarDos && reiniciar !== undefined,
  oyentesClick: (oyentes.get("click") ?? []).length,
}));
"#;

    let json = correr(
        "contador",
        &[
            ("client.mjs", app.client_js.as_str()),
            ("dom.mjs", DOM_FALSO),
            ("t.mjs", guion),
        ],
        "t.mjs",
    );
    for (clave, esperado) in [
        ("inicial", "<b>0</b>"),
        ("trasUno", "<b>1</b>"),
        ("trasDos", "<b>2</b>"),
        // Un clic sobre el manejador del re-pintado anterior no hace nada.
        ("trasHuerfano", "<b>2</b>"),
        ("trasReiniciar", "<b>0</b>"),
    ] {
        // El valor va hasta el `","` que abre la clave siguiente: sin cortar
        // ahí, "<b>0</b>" del reinicio haría pasar la comprobación del inicial.
        let trozo = json
            .split(&format!("\"{clave}\":\""))
            .nth(1)
            .unwrap_or_else(|| panic!("falta '{clave}' en {json}"))
            .split("\",\"")
            .next()
            .unwrap();
        assert!(
            trozo.contains(esperado),
            "'{clave}' debería contener '{esperado}': {json}"
        );
    }
    assert!(
        json.contains(r#""idsDistintos":true"#),
        "cada pintado registra manejadores nuevos: {json}"
    );
    // CUATRO re-pintados y sigue habiendo un solo oyente: eso es la delegación.
    assert!(
        json.contains(r#""oyentesClick":1"#),
        "re-pintar no puede acumular oyentes: {json}"
    );
}
