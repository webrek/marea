//! CLI del lenguaje Marea.
//!
//!   marea tokens     <archivo.mar>        muestra los tokens del lexer
//!   marea parse      <archivo.mar>        muestra el AST del parser
//!   marea deps       <archivo.mar>        resuelve el grafo de módulos
//!   marea check      <archivo.mar>        verifica los tipos del módulo
//!   marea build      <archivo.mar> [dir]  transpila a TypeScript
//!   marea build-wasm <archivo.mar> [out]  compila a WebAssembly (WAT)
//!   marea build-web  <archivo.mar> [dir]  genera una app web (WASM + DOM)
//!   marea build-app  <archivo.mar> [dir]  app web completa (RPC + reactivo + DOM)
//!
//! Todos los `build*` verifican tipos antes de emitir; `--no-check` lo omite.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    // Separa banderas (`--flag`) de argumentos posicionales, para que
    // `--no-check` pueda ir en cualquier posición sin desplazar la ruta ni el dir.
    let positional: Vec<&str> = args
        .iter()
        .skip(1)
        .map(String::as_str)
        .filter(|a| !a.starts_with("--"))
        .collect();
    let no_check = args.iter().any(|a| a == "--no-check");

    if positional.len() < 2 {
        print_usage();
        return ExitCode::FAILURE;
    }

    let cmd = positional[0];
    let path = positional[1];
    let out_arg = positional.get(2).copied();

    let src = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: no se pudo leer '{}': {}", path, e);
            return ExitCode::FAILURE;
        }
    };

    match cmd {
        "tokens" => match marea_syntax::Lexer::tokenize(&src) {
            Ok(tokens) => {
                for t in &tokens {
                    println!("{:>4}..{:<4} {:?}", t.span.start, t.span.end, t.kind);
                }
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}", e.render(&src));
                ExitCode::FAILURE
            }
        },
        "parse" => match marea_syntax::parse(&src) {
            Ok(module) => {
                println!("{:#?}", module);
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}", e.render(&src));
                ExitCode::FAILURE
            }
        },
        "deps" => deps(path),
        // Un archivo con `import` ya no es un programa completo: hay que
        // resolver el grafo y chequear todos sus módulos, porque un error puede
        // estar en un archivo que no es el que nombraste.
        "check" if tiene_imports(&src) => check_programa(path),
        "check" => match frontend(&src, false) {
            Ok(_) => {
                println!("  {} tipa sin errores", path);
                ExitCode::SUCCESS
            }
            Err(code) => code,
        },
        // Con `import`, el front-end es otro: hay que resolver el grafo, chequear
        // todos los módulos y APLANARLOS. La salida de Marea es un bundle plano,
        // así que emitir módulo a módulo daría una copia del runtime por archivo
        // y ninguna forma de que una función llame a otra.
        "build" => {
            let out_dir = out_arg.unwrap_or("marea-out");
            match front(path, &src, no_check) {
                Ok(module) => build(&module, out_dir),
                Err(code) => code,
            }
        }
        "build-wasm" => {
            let out = out_arg.unwrap_or("module.wat");
            match front(path, &src, no_check) {
                Ok(module) => match marea_wasm::emit_wat(&module) {
                    Ok(wat) => match std::fs::write(out, wat) {
                        Ok(()) => {
                            println!("  escrito {}", out);
                            println!(
                                "\nEnsambla y corre con:\n  wat2wasm {out} -o module.wasm",
                                out = out
                            );
                            ExitCode::SUCCESS
                        }
                        Err(e) => {
                            eprintln!("error: no se pudo escribir '{}': {}", out, e);
                            ExitCode::FAILURE
                        }
                    },
                    Err(e) => {
                        eprintln!("error de codegen WASM: {}", e);
                        ExitCode::FAILURE
                    }
                },
                Err(code) => code,
            }
        }
        "build-web" => {
            let out_dir = out_arg.unwrap_or("marea-web");
            match front(path, &src, no_check) {
                Ok(module) => build_web(&module, out_dir),
                Err(code) => code,
            }
        }
        "build-app" => {
            let out_dir = out_arg.unwrap_or("marea-app");
            match front(path, &src, no_check) {
                Ok(module) => build_app(&module, out_dir),
                Err(code) => code,
            }
        }
        other => {
            eprintln!("error: comando desconocido '{}'", other);
            print_usage();
            ExitCode::FAILURE
        }
    }
}

/// El front-end que le toca a este archivo: si declara `import`, hay que
/// resolver el grafo y aplanarlo; si no, basta con parsearlo.
///
/// Existe para que no haya que acordarse comando por comando. Cuando el grafo se
/// cableó, `check`, `build` y `build-app` lo aprendieron y `build-web` y
/// `build-wasm` no, así que el mismo archivo compilaba en uno y fallaba en otro
/// con "'pesos' no está definido".
fn front(path: &str, src: &str, no_check: bool) -> Result<marea_syntax::Module, ExitCode> {
    if tiene_imports(src) {
        frontend_programa(path, no_check)
    } else {
        frontend(src, no_check)
    }
}

/// `marea deps` — resuelve el grafo de módulos y lo imprime en orden
/// topológico. Es la única forma, de momento, de ver el sistema de módulos
/// funcionando: el verificador y el codegen siguen trabajando sobre un solo
/// archivo, así que el grafo no llega todavía más allá de aquí.
fn deps(path: &str) -> ExitCode {
    let program = match marea_syntax::resolve_program(std::path::Path::new(path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.render());
            return ExitCode::FAILURE;
        }
    };
    let n = program.modulos.len();
    println!(
        "{} módulo{} en orden topológico (dependencias antes que dependientes):\n",
        n,
        if n == 1 { "" } else { "s" }
    );
    for m in &program.modulos {
        println!("  [{}] {}", m.id, m.nombre);
        for import in &m.modulo.imports {
            let nombres: Vec<&str> = import.names.iter().map(|n| n.name.as_str()).collect();
            println!(
                "        import {{ {} }} from \"{}\"",
                nombres.join(", "),
                import.path
            );
        }
    }
    println!("\nentrada: {}", program.entrada().nombre);
    ExitCode::SUCCESS
}

/// Front-end común de los comandos de compilación: parsea con recuperación
/// (reporta TODOS los errores de sintaxis a la vez, igual que `check`) y, salvo
/// `--no-check`, verifica los tipos antes de dejar pasar el módulo al codegen.
/// Así la garantía del verificador deja de ser opt-in: `build`/`build-app`/etc.
/// ya no emiten código que el checker habría rechazado. Devuelve `Err(código)`
/// con los diagnósticos ya impresos si algo falla.
fn frontend(src: &str, no_check: bool) -> Result<marea_syntax::Module, ExitCode> {
    let (module, syntax_errors) = marea_syntax::parse_recovering(src);
    if !syntax_errors.is_empty() {
        for e in &syntax_errors {
            eprintln!("{}\n", e.render(src));
        }
        let n = syntax_errors.len();
        eprintln!("{} error{} de sintaxis", n, if n == 1 { "" } else { "es" });
        return Err(ExitCode::FAILURE);
    }
    if !no_check {
        let errores = marea_types::check(&module);
        if !errores.is_empty() {
            for e in &errores {
                eprintln!("{}\n", e.render(src));
            }
            let n = errores.len();
            eprintln!(
                "{} error{} de tipos (usa --no-check para compilar de todos modos)",
                n,
                if n == 1 { "" } else { "es" }
            );
            return Err(ExitCode::FAILURE);
        }
    }
    Ok(module)
}

fn build_app(module: &marea_syntax::Module, out_dir: &str) -> ExitCode {
    let app = marea_codegen::emit_app(module);
    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!("error: no se pudo crear '{}': {}", out_dir, e);
        return ExitCode::FAILURE;
    }
    let files = [
        ("runtime.ts", app.runtime),
        ("server.ts", app.server),
        ("serve.ts", app.serve),
        ("client.js", app.client_js),
        ("index.html", app.index_html),
    ];
    for (name, contents) in files {
        let path = format!("{}/{}", out_dir, name);
        // El index.html es una plantilla mínima pensada para arrancar, no un
        // archivo de salida: si ya existe se respeta, porque suele estar escrito
        // a mano (la landing de site/ lo está). Los demás son código generado y
        // se sobrescriben siempre — así regenerar tras arreglar el compilador no
        // destruye la página.
        if name == "index.html" && std::path::Path::new(&path).exists() {
            println!("  conservado {} (ya existía)", path);
            continue;
        }
        if let Err(e) = std::fs::write(&path, contents) {
            eprintln!("error: no se pudo escribir '{}': {}", path, e);
            return ExitCode::FAILURE;
        }
        println!("  escrito {}", path);
    }
    println!(
        "\nlisto. Arranca la app web (servidor + estáticos en el mismo origen):\n  \
         node {}/serve.ts\n  \
         (abre http://127.0.0.1:8787 en el navegador)",
        out_dir
    );
    ExitCode::SUCCESS
}

fn build(module: &marea_syntax::Module, out_dir: &str) -> ExitCode {
    let project = marea_codegen::emit(module);
    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!("error: no se pudo crear '{}': {}", out_dir, e);
        return ExitCode::FAILURE;
    }
    let files = [
        ("runtime.ts", project.runtime),
        ("server.ts", project.server),
        ("client.ts", project.client),
        ("demo.ts", project.demo),
    ];
    for (name, contents) in files {
        let path = format!("{}/{}", out_dir, name);
        if let Err(e) = std::fs::write(&path, contents) {
            eprintln!("error: no se pudo escribir '{}': {}", path, e);
            return ExitCode::FAILURE;
        }
        println!("  escrito {}", path);
    }
    println!(
        "\nlisto. Para correr la demo end-to-end:\n  node {}/demo.ts",
        out_dir
    );
    ExitCode::SUCCESS
}

fn build_web(module: &marea_syntax::Module, out_dir: &str) -> ExitCode {
    let wat = match marea_wasm::emit_wat(module) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("error de codegen WASM: {}", e);
            return ExitCode::FAILURE;
        }
    };
    let (html, glue) = marea_codegen::emit_web(module);
    if let Err(e) = std::fs::create_dir_all(out_dir) {
        eprintln!("error: no se pudo crear '{}': {}", out_dir, e);
        return ExitCode::FAILURE;
    }
    let readme = "# App web de Marea\n\n\
        Generada con `marea build-web`. Para correrla:\n\n\
        ```sh\n\
        wat2wasm module.wat -o module.wasm   # ensambla el WASM\n\
        # sirve esta carpeta con un servidor estático y abre index.html, p.ej.:\n\
        python3 -m http.server 8000\n\
        ```\n\n\
        El glue carga `module.wasm`, expone las funciones en `window.marea`\n\
        (las cadenas se decodifican con `mareaCadena(ptr)`), y renderiza\n\
        `vista()`/`main()` en `#salida`.\n";
    let files = [
        ("module.wat", wat),
        ("index.html", html),
        ("glue.mjs", glue),
        ("README.md", readme.to_string()),
    ];
    for (name, contents) in files {
        let path = format!("{}/{}", out_dir, name);
        if let Err(e) = std::fs::write(&path, contents) {
            eprintln!("error: no se pudo escribir '{}': {}", path, e);
            return ExitCode::FAILURE;
        }
        println!("  escrito {}", path);
    }
    println!(
        "\nlisto. Ensambla y sirve:\n  \
         wat2wasm {dir}/module.wat -o {dir}/module.wasm\n  \
         (sirve {dir}/ con un servidor estático y abre index.html)",
        dir = out_dir
    );
    ExitCode::SUCCESS
}

fn print_usage() {
    eprintln!("Marea — compilador del lenguaje (v0)\n");
    eprintln!("uso:");
    eprintln!("  marea tokens     <archivo.mar>        muestra los tokens");
    eprintln!("  marea parse      <archivo.mar>        muestra el AST");
    eprintln!("  marea deps       <archivo.mar>        resuelve el grafo de módulos");
    eprintln!("  marea check      <archivo.mar>        verifica los tipos");
    eprintln!("  marea build      <archivo.mar> [dir]  transpila a TypeScript");
    eprintln!("  marea build-wasm <archivo.mar> [out]  compila a WebAssembly (WAT)");
    eprintln!("  marea build-web  <archivo.mar> [dir]  genera una app web (WASM + DOM)");
    eprintln!("  marea build-app  <archivo.mar> [dir]  app web completa (RPC + reactivo + DOM)");
    eprintln!("\nbanderas:");
    eprintln!(
        "  --no-check                            omite la verificación de tipos en los build*"
    );
}

/// ¿El fuente declara algún `import`? Se mira sobre los TOKENS y no con una
/// búsqueda de texto: "import" dentro de una cadena o de un comentario no
/// convierte un archivo en un programa de varios módulos.
fn tiene_imports(src: &str) -> bool {
    !marea_syntax::parse_recovering(src).0.imports.is_empty()
}

/// `marea check` sobre un programa de varios módulos: resuelve el grafo desde el
/// archivo de entrada y chequea todos, atribuyendo cada error a su archivo.
fn check_programa(path: &str) -> ExitCode {
    let programa = match marea_syntax::program::resolve_program(std::path::Path::new(path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.render());
            return ExitCode::FAILURE;
        }
    };
    // Los errores de sintaxis de CUALQUIER módulo van primero: sin ellos el
    // chequeo de tipos de ese archivo estaría mirando un AST incompleto.
    let mut sintaxis = 0usize;
    for m in &programa.modulos {
        for e in marea_syntax::parse_recovering(&m.fuente).1 {
            eprintln!("{}:", m.nombre);
            eprintln!("{}\n", e.render(&m.fuente));
            sintaxis += 1;
        }
    }
    if sintaxis > 0 {
        eprintln!(
            "{} error{} de sintaxis",
            sintaxis,
            if sintaxis == 1 { "" } else { "es" }
        );
        return ExitCode::FAILURE;
    }

    let errores = marea_types::check_program(&programa);
    if errores.is_empty() {
        println!(
            "  {} módulos tipan sin errores ({})",
            programa.modulos.len(),
            programa
                .modulos
                .iter()
                .map(|m| m.nombre.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        return ExitCode::SUCCESS;
    }
    for pe in &errores {
        let m = &programa.modulos[pe.modulo];
        eprintln!("{}:", m.nombre);
        eprintln!("{}\n", pe.error.render(&m.fuente));
    }
    eprintln!(
        "{} error{} de tipos en {} módulo{}",
        errores.len(),
        if errores.len() == 1 { "" } else { "es" },
        programa.modulos.len(),
        if programa.modulos.len() == 1 { "" } else { "s" }
    );
    ExitCode::FAILURE
}

/// Front-end de los comandos de compilación cuando el archivo tiene `import`:
/// resuelve el grafo, chequea TODOS los módulos y devuelve el programa aplanado
/// listo para el codegen.
fn frontend_programa(path: &str, no_check: bool) -> Result<marea_syntax::Module, ExitCode> {
    let programa = match marea_syntax::program::resolve_program(std::path::Path::new(path)) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{}", e.render());
            return Err(ExitCode::FAILURE);
        }
    };
    let mut sintaxis = 0usize;
    for m in &programa.modulos {
        for e in marea_syntax::parse_recovering(&m.fuente).1 {
            eprintln!("{}:", m.nombre);
            eprintln!("{}\n", e.render(&m.fuente));
            sintaxis += 1;
        }
    }
    if sintaxis > 0 {
        eprintln!(
            "{} error{} de sintaxis",
            sintaxis,
            if sintaxis == 1 { "" } else { "es" }
        );
        return Err(ExitCode::FAILURE);
    }
    if !no_check {
        let errores = marea_types::check_program(&programa);
        if !errores.is_empty() {
            for pe in &errores {
                let m = &programa.modulos[pe.modulo];
                eprintln!("{}:", m.nombre);
                eprintln!("{}\n", pe.error.render(&m.fuente));
            }
            let n = errores.len();
            eprintln!(
                "{} error{} de tipos (usa --no-check para compilar de todos modos)",
                n,
                if n == 1 { "" } else { "es" }
            );
            return Err(ExitCode::FAILURE);
        }
    }
    println!(
        "  {} módulos: {}",
        programa.modulos.len(),
        programa
            .modulos
            .iter()
            .map(|m| m.nombre.as_str())
            .collect::<Vec<_>>()
            .join(" -> ")
    );
    Ok(marea_syntax::program::aplanar(&programa))
}
