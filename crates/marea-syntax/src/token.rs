//! Tokens léxicos de Marea.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literales
    Int(i64),
    Float(f64),
    Str(String),
    Bool(bool),
    Ident(String),

    // Palabras clave
    Fn,
    Let,
    Mut,
    Reactive,
    Type,
    If,
    Else,
    Match,
    Return,
    Import,

    // '@' de los atributos de ubicación: @server, @client, @edge
    At,

    // Delimitadores
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,

    // Puntuación
    Comma,
    Colon,
    Semicolon,
    Dot,
    Arrow,    // ->
    FatArrow, // =>

    // Operadores
    Eq,       // =
    EqEq,     // ==
    BangEq,   // !=
    Lt,       // <
    Gt,       // >
    Le,       // <=
    Ge,       // >=
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AmpAmp,   // &&
    PipePipe, // ||
    Pipe,     // |  (uniones de tipos: User | NotFound)
    Bang,     // !

    // Patrón comodín
    Underscore, // _

    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Token { kind, span }
    }
}
