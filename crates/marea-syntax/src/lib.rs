//! `marea-syntax` — front-end del lenguaje Marea: lexer, AST y parser.
//!
//! Marea es un lenguaje enfocado a la web cuya tesis es volver primitivas dos
//! fronteras que hoy se cruzan a mano: la de red (cliente↔servidor) y la del
//! tiempo (reactividad). Este crate cubre el primer hito: de texto a AST.

pub mod ast;
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

/// Conveniencia: de fuente a módulo (lex + parse en un paso).
pub fn parse(src: &str) -> Result<Module, SyntaxError> {
    let tokens = Lexer::tokenize(src)?;
    Parser::parse_module(tokens)
}
