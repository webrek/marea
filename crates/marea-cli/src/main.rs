//! CLI del lenguaje Marea.
//!
//!   marea tokens <archivo.mar>   muestra los tokens del lexer
//!   marea parse  <archivo.mar>   muestra el AST del parser

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
        other => {
            eprintln!("error: comando desconocido '{}'", other);
            print_usage();
            ExitCode::FAILURE
        }
    }
}

fn print_usage() {
    eprintln!("Marea — compilador del lenguaje (v0)\n");
    eprintln!("uso:");
    eprintln!("  marea tokens <archivo.mar>   muestra los tokens");
    eprintln!("  marea parse  <archivo.mar>   muestra el AST");
}
