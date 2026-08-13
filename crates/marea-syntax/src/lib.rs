//! `marea-syntax` — front-end del lenguaje Marea: lexer, AST y parser.
//!
//! Marea es un lenguaje enfocado a la web cuya tesis es volver primitivas dos
//! fronteras que hoy se cruzan a mano: la de red (cliente↔servidor) y la del
//! tiempo (reactividad). Este crate cubre el primer hito: de texto a AST.

pub mod ast;
pub mod builtins;
pub mod error;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;

pub use ast::Module;
pub use error::SyntaxError;
pub use lexer::Lexer;
pub use parser::Parser;
pub use token::{Token, TokenKind};

/// Conveniencia: de fuente a módulo (lex + parse en un paso, fail-fast).
pub fn parse(src: &str) -> Result<Module, SyntaxError> {
    let tokens = Lexer::tokenize(src)?;
    Parser::parse_module(tokens)
}

/// De fuente a módulo PARCIAL + todos los errores de sintaxis (con recuperación).
///
/// Si el lexer falla (p. ej. cadena sin cerrar), devuelve un módulo vacío y ese
/// único error léxico. Si lexea, recupera a nivel de item y acumula los errores.
pub fn parse_recovering(src: &str) -> (Module, Vec<SyntaxError>) {
    match Lexer::tokenize(src) {
        Ok(tokens) => Parser::parse_module_recovering(tokens),
        Err(e) => (Module { items: Vec::new() }, vec![e]),
    }
}
