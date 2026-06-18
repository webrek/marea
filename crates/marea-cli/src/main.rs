//! CLI del lenguaje Marea.
//!
//!   marea tokens <archivo.mar>   muestra los tokens del lexer
//!   marea parse  <archivo.mar>   muestra el AST del parser
//!   marea check  <archivo.mar>   verifica los tipos del módulo

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        print_usage();
        return ExitCode::FAILURE;
    }

    let cmd = args[1].as_str();
    let path = args[2].as_str();

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
        "check" => match marea_syntax::parse(&src) {
            Ok(module) => {
                let errores = marea_types::check(&module);
                if errores.is_empty() {
                    println!("  {} tipa sin errores", path);
                    ExitCode::SUCCESS
                } else {
                    for e in &errores {
                        eprintln!("{}\n", e.render(&src));
                    }
                    let n = errores.len();
                    eprintln!(
                        "{} error{} de tipos",
                        n,
                        if n == 1 { "" } else { "es" }
                    );
                    ExitCode::FAILURE
                }
            }
            Err(e) => {
                eprintln!("{}", e.render(&src));
                ExitCode::FAILURE
            }
        },
        "build" => {
            let out_dir = args.get(3).map(String::as_str).unwrap_or("marea-out");
            match marea_syntax::parse(&src) {
                Ok(module) => build(&module, out_dir),
                Err(e) => {
                    eprintln!("{}", e.render(&src));
                    ExitCode::FAILURE
                }
            }
        }
        "build-wasm" => {
            let out = args.get(3).map(String::as_str).unwrap_or("module.wat");
            match marea_syntax::parse(&src) {
                Ok(module) => match marea_wasm::emit_wat(&module) {
                    Ok(wat) => match std::fs::write(out, wat) {
                        Ok(()) => {
                            println!("  escrito {}", out);
                            println!("\nEnsambla y corre con:\n  wat2wasm {out} -o module.wasm", out = out);
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
                Err(e) => {
                    eprintln!("{}", e.render(&src));
                    ExitCode::FAILURE
                }
            }
        }
        other => {
            eprintln!("error: comando desconocido '{}'", other);
            print_usage();
            ExitCode::FAILURE
        }
    }
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
    println!("\nlisto. Para correr la demo end-to-end:\n  node {}/demo.ts", out_dir);
    ExitCode::SUCCESS
}

fn print_usage() {
    eprintln!("Marea — compilador del lenguaje (v0)\n");
    eprintln!("uso:");
    eprintln!("  marea tokens     <archivo.mar>        muestra los tokens");
    eprintln!("  marea parse      <archivo.mar>        muestra el AST");
    eprintln!("  marea check      <archivo.mar>        verifica los tipos");
    eprintln!("  marea build      <archivo.mar> [dir]  transpila a TypeScript");
    eprintln!("  marea build-wasm <archivo.mar> [out]  compila a WebAssembly (WAT)");
}
