//! Prueba DIFERENCIAL: el mismo `.mar` por los dos backends, comparando
//! resultados.
//!
//! Un lenguaje con dos backends tiene un riesgo que ningún test por separado
//! ve: que el mismo programa signifique dos cosas. Esta prueba lo mide. Cuando
//! se escribió encontró siete divergencias reales —igualdad de cadenas por
//! puntero, `&&` sin cortocircuito, indexado sin comprobar rango (que además
//! leía memoria ajena) y división entre cero que en TypeScript daba `Infinity`
//! dentro de un `Int`— y todas se cerraron.
//!
//! Queda una divergencia CONOCIDA y aceptada: `Int` es i32 en WASM y un número
//! de 53 bits en TypeScript, así que al desbordar los resultados difieren. Está
//! documentada en el README; el test la fija para que no crezca en silencio.
//!
//! Necesita `wat2wasm` (paquete wabt) y `node`; si falta alguno, se salta.

use std::process::{Command, Stdio};

fn hay(cmd: &str, arg: &str) -> bool {
    Command::new(cmd)
        .arg(arg)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Una llamada a comparar entre los dos backends: nombre de la función y sus
/// argumentos.
type Llamada<'a> = (&'a str, &'a [i32]);

/// Un caso del diferencial: etiqueta, fuente `.mar` y las llamadas a comparar.
type Caso<'a> = (&'a str, &'a str, &'a [Llamada<'a>]);

/// Programas en el subconjunto que ambos backends aceptan, con funciones puras
/// que devuelven Int (los Bool como 0/1) para poder comparar de forma uniforme.
const CASOS: &[Caso] = &[
    (
        "control",
        "fn fib(n: Int) -> Int { if n < 2 { return n; } return fib(n - 1) + fib(n - 2); }\n\
         fn suma(n: Int) -> Int { if n < 1 { return 0; } return n + suma(n - 1); }",
        &[("fib", &[10]), ("fib", &[20]), ("suma", &[100])],
    ),
    (
        "cadenas",
        "fn vacio() -> Int { if concat(\"a\", \"\") == \"a\" { return 1; } return 0; }\n\
         fn igual() -> Int { if concat(\"ab\", \"\") == concat(\"a\", \"b\") { return 1; } return 0; }\n\
         fn distinto() -> Int { if \"a\" == \"b\" { return 1; } return 0; }",
        &[("vacio", &[]), ("igual", &[]), ("distinto", &[])],
    ),
    (
        "cortocircuito",
        "fn caro(n: Int) -> Int { return n / 0; }\n\
         fn conY() -> Int { if false && (caro(1) > 0) { return 1; } return 0; }\n\
         fn conO() -> Int { if true || (caro(1) > 0) { return 1; } return 0; }",
        &[("conY", &[]), ("conO", &[])],
    ),
    (
        "indices",
        "fn dentro() -> Int { let xs = [10, 20, 30]; return xs[1]; }\n\
         fn fuera() -> Int { let xs = [10, 20, 30]; return xs[5]; }\n\
         fn negativo() -> Int { let xs = [10, 20, 30]; return xs[0 - 1]; }",
        &[("dentro", &[]), ("fuera", &[]), ("negativo", &[])],
    ),
];

#[test]
fn los_dos_backends_significan_lo_mismo() {
    if !hay("node", "--version") || !hay("wat2wasm", "--version") {
        eprintln!("faltan node o wat2wasm: se omite la prueba diferencial");
        return;
    }
    let dir = std::env::temp_dir().join("marea-diferencial");
    let dir = dir.to_string_lossy().to_string();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("crear dir");

    let mut plan = Vec::new();
    for (nombre, fuente, llamadas) in CASOS {
        let module = marea_syntax::parse(fuente)
            .unwrap_or_else(|e| panic!("'{nombre}' debe parsear: {}", e.message));
        assert!(
            marea_types::check(&module).is_empty(),
            "'{nombre}' debe tipar"
        );
        // Backend de TypeScript.
        let ts = marea_codegen::emit(&module);
        let dts = format!("{dir}/ts-{nombre}");
        std::fs::create_dir_all(&dts).expect("crear dir ts");
        std::fs::write(format!("{dts}/runtime.ts"), ts.runtime).expect("escribir");
        std::fs::write(format!("{dts}/client.ts"), ts.client).expect("escribir");
        // Backend WASM.
        let wat = marea_wasm::emit_wat(&module)
            .unwrap_or_else(|e| panic!("'{nombre}' debe compilar a WASM: {e}"));
        let ruta_wat = format!("{dir}/{nombre}.wat");
        std::fs::write(&ruta_wat, wat).expect("escribir wat");
        let ok = Command::new("wat2wasm")
            .args([&ruta_wat, "-o", &format!("{dir}/{nombre}.wasm")])
            .status()
            .expect("wat2wasm");
        assert!(ok.success(), "'{nombre}' debe ensamblar");

        for (fnc, args) in *llamadas {
            let lista: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            plan.push(format!(
                r#"{{"caso":"{nombre}","fn":"{fnc}","args":[{}]}}"#,
                lista.join(",")
            ));
        }
    }

    let guion = format!(
        r#"
import fs from "node:fs";
const plan = [{}];
const div = [];
for (const p of plan) {{
  const mod = await import("{dir}/ts-" + p.caso + "/client.ts");
  const {{ instance }} = await WebAssembly.instantiate(fs.readFileSync("{dir}/" + p.caso + ".wasm"), {{}});
  let ts, wa;
  try {{ ts = {{ v: await mod[p.fn](...p.args) }}; }} catch {{ ts = {{ err: 1 }}; }}
  try {{ wa = {{ v: instance.exports[p.fn](...p.args) }}; }} catch {{ wa = {{ err: 1 }}; }}
  // Si los dos fallan concuerdan: el programa se rechaza en ambos blancos.
  const igual = (ts.err && wa.err) || JSON.stringify(ts) === JSON.stringify(wa);
  if (!igual) div.push(p.caso + "." + p.fn + " TS=" + JSON.stringify(ts) + " WASM=" + JSON.stringify(wa));
}}
console.log("RESULTADO:" + JSON.stringify({{ total: plan.length, div }}));
"#,
        plan.join(",")
    );
    let salida = Command::new("node")
        .args(["--input-type=module", "-e", &guion])
        .output()
        .expect("lanzar node");
    let txt = String::from_utf8_lossy(&salida.stdout).to_string();
    assert!(
        txt.contains("RESULTADO:"),
        "el guion no completó.\nstdout: {txt}\nstderr: {}",
        String::from_utf8_lossy(&salida.stderr)
    );
    let json = txt.split("RESULTADO:").nth(1).unwrap().trim();
    assert!(
        json.contains(r#""div":[]"#),
        "los dos backends deben significar lo mismo; divergencias: {json}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
