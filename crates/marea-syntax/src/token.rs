//! Tokens léxicos de Marea.

use crate::span::Span;

/// Una pieza de plantilla: texto literal, o un hueco con el FUENTE de su
/// expresión y el desplazamiento absoluto donde empieza (para que los errores
/// dentro del hueco apunten al sitio correcto del archivo).
#[derive(Debug, Clone, PartialEq)]
pub enum TemplatePiece {
    Lit(String),
    /// `{expr}` escapa; `{!expr}` inserta tal cual (y exige que sea Html).
    Hueco { fuente: String, offset: usize, crudo: bool },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    // Literales
    Int(i64),
    Float(f64),
    Str(String),
    /// Plantilla de texto: `` `hola {nombre}` ``. Se lexea entera en un token
    /// que lleva sus piezas; el parser vuelve a parsear el fuente de cada hueco.
    Template(Vec<TemplatePiece>),
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
    Effect,
    Store,

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
