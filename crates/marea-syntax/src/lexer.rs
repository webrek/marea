//! Lexer (tokenizador) escrito a mano, sin dependencias.
//!
//! Recorre el código por caracteres con sus offsets de byte y produce una
//! lista de tokens terminada en `Eof`. Salta espacios y comentarios
//! (`// línea` y `/* bloque */`).

use crate::error::SyntaxError;
use crate::span::Span;
use crate::token::{TemplatePiece, Token, TokenKind};

pub struct Lexer<'a> {
    src: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer {
            src,
            chars: src.char_indices().peekable(),
        }
    }

    /// Tokeniza toda la fuente. La lista siempre termina en `Eof`.
    pub fn tokenize(src: &'a str) -> Result<Vec<Token>, SyntaxError> {
        let mut lexer = Lexer::new(src);
        let mut tokens = Vec::new();
        loop {
            let tok = lexer.next_token()?;
            let is_eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if is_eof {
                break;
            }
        }
        Ok(tokens)
    }

    // --- utilidades de cursor ---

    fn peek_char(&mut self) -> Option<char> {
        self.chars.peek().map(|&(_, c)| c)
    }

    /// Offset de byte del próximo carácter, o el final de la fuente.
    fn peek_offset(&mut self) -> usize {
        match self.chars.peek() {
            Some(&(i, _)) => i,
            None => self.src.len(),
        }
    }

    fn bump(&mut self) -> Option<(usize, char)> {
        self.chars.next()
    }

    // --- saltar espacios y comentarios ---

    fn skip_trivia(&mut self) -> Result<(), SyntaxError> {
        loop {
            match self.peek_char() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') => {
                    // Lookahead de 2: ¿es comentario o el operador '/'?
                    let mut clone = self.chars.clone();
                    clone.next(); // consume '/'
                    match clone.peek().map(|&(_, c)| c) {
                        Some('/') => {
                            self.bump();
                            self.bump();
                            while let Some(c) = self.peek_char() {
                                if c == '\n' {
                                    break;
                                }
                                self.bump();
                            }
                        }
                        Some('*') => {
                            let start = self.peek_offset();
                            self.bump();
                            self.bump(); // consume '/*'
                            loop {
                                match self.bump() {
                                    Some((_, '*')) if self.peek_char() == Some('/') => {
                                        self.bump();
                                        break;
                                    }
                                    Some(_) => {}
                                    None => {
                                        return Err(SyntaxError::new(
                                            "comentario de bloque sin cerrar",
                                            Span::new(start, self.src.len()),
                                        ))
                                    }
                                }
                            }
                        }
                        // Es el operador de división: lo deja para next_token.
                        _ => break,
                    }
                }
                _ => break,
            }
        }
        Ok(())
    }

    // --- token siguiente ---

    fn next_token(&mut self) -> Result<Token, SyntaxError> {
        self.skip_trivia()?;
        let start = self.peek_offset();

        let c = match self.peek_char() {
            None => return Ok(Token::new(TokenKind::Eof, Span::new(start, start))),
            Some(c) => c,
        };

        if c.is_alphabetic() || c == '_' {
            return Ok(self.lex_ident(start));
        }
        if c.is_ascii_digit() {
            return self.lex_number(start);
        }
        if c == '`' {
            return self.lex_template(start);
        }
        if c == '"' {
            return self.lex_string(start);
        }

        self.bump(); // consume el carácter del operador/puntuación
        let kind = match c {
            '(' => TokenKind::LParen,
            ')' => TokenKind::RParen,
            '{' => TokenKind::LBrace,
            '}' => TokenKind::RBrace,
            '[' => TokenKind::LBracket,
            ']' => TokenKind::RBracket,
            ',' => TokenKind::Comma,
            ':' => TokenKind::Colon,
            ';' => TokenKind::Semicolon,
            '.' => TokenKind::Dot,
            '@' => TokenKind::At,
            '+' => TokenKind::Plus,
            '*' => TokenKind::Star,
            '/' => TokenKind::Slash,
            '%' => TokenKind::Percent,
            '-' => {
                if self.peek_char() == Some('>') {
                    self.bump();
                    TokenKind::Arrow
                } else {
                    TokenKind::Minus
                }
            }
            '=' => match self.peek_char() {
                Some('=') => {
                    self.bump();
                    TokenKind::EqEq
                }
                Some('>') => {
                    self.bump();
                    TokenKind::FatArrow
                }
                _ => TokenKind::Eq,
            },
            '!' => {
                if self.peek_char() == Some('=') {
                    self.bump();
                    TokenKind::BangEq
                } else {
                    TokenKind::Bang
                }
            }
            '<' => {
                if self.peek_char() == Some('=') {
                    self.bump();
                    TokenKind::Le
                } else {
                    TokenKind::Lt
                }
            }
            '>' => {
                if self.peek_char() == Some('=') {
                    self.bump();
                    TokenKind::Ge
                } else {
                    TokenKind::Gt
                }
            }
            '&' => {
                if self.peek_char() == Some('&') {
                    self.bump();
                    TokenKind::AmpAmp
                } else {
                    return Err(SyntaxError::new(
                        "se encontró '&' suelto; ¿quisiste decir '&&'?",
                        Span::new(start, self.peek_offset()),
                    ));
                }
            }
            '|' => {
                if self.peek_char() == Some('|') {
                    self.bump();
                    TokenKind::PipePipe
                } else {
                    TokenKind::Pipe
                }
            }
            other => {
                return Err(SyntaxError::new(
                    format!("carácter inesperado '{}'", other),
                    Span::new(start, self.peek_offset()),
                ))
            }
        };

        let end = self.peek_offset();
        Ok(Token::new(kind, Span::new(start, end)))
    }

    fn lex_ident(&mut self, start: usize) -> Token {
        while let Some(c) = self.peek_char() {
            if c.is_alphanumeric() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }
        let end = self.peek_offset();
        let text = &self.src[start..end];
        let kind = match text {
            "fn" => TokenKind::Fn,
            "let" => TokenKind::Let,
            "mut" => TokenKind::Mut,
            "reactive" => TokenKind::Reactive,
            "type" => TokenKind::Type,
            "if" => TokenKind::If,
            "else" => TokenKind::Else,
            "match" => TokenKind::Match,
            "return" => TokenKind::Return,
            "import" => TokenKind::Import,
            "effect" => TokenKind::Effect,
            "store" => TokenKind::Store,
            "true" => TokenKind::Bool(true),
            "false" => TokenKind::Bool(false),
            "_" => TokenKind::Underscore,
            _ => TokenKind::Ident(text.to_string()),
        };
        Token::new(kind, Span::new(start, end))
    }

    fn lex_number(&mut self, start: usize) -> Result<Token, SyntaxError> {
        while let Some(c) = self.peek_char() {
            if c.is_ascii_digit() || c == '_' {
                self.bump();
            } else {
                break;
            }
        }

        let mut is_float = false;
        // Un '.' seguido de dígito hace flotante; si no, es acceso a miembro.
        if self.peek_char() == Some('.') {
            let mut clone = self.chars.clone();
            clone.next(); // '.'
            if let Some(&(_, c)) = clone.peek() {
                if c.is_ascii_digit() {
                    is_float = true;
                    self.bump(); // '.'
                    while let Some(c) = self.peek_char() {
                        if c.is_ascii_digit() || c == '_' {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                }
            }
        }

        let end = self.peek_offset();
        let raw: String = self.src[start..end].chars().filter(|&c| c != '_').collect();
        let span = Span::new(start, end);

        if is_float {
            let value: f64 = raw
                .parse()
                .map_err(|_| SyntaxError::new("número flotante inválido", span))?;
            Ok(Token::new(TokenKind::Float(value), span))
        } else {
            let value: i64 = raw
                .parse()
                .map_err(|_| SyntaxError::new("entero fuera de rango para i64", span))?;
            Ok(Token::new(TokenKind::Int(value), span))
        }
    }

    /// Plantilla de texto: `` `hola {nombre}` ``. Se lexea entera y se guarda el
    /// FUENTE de cada hueco; el parser lo vuelve a parsear con su desplazamiento
    /// real, así que un error dentro de `{...}` apunta a su sitio del archivo.
    fn lex_template(&mut self, start: usize) -> Result<Token, SyntaxError> {
        self.bump(); // comilla invertida de apertura
        let mut piezas: Vec<TemplatePiece> = Vec::new();
        let mut lit = String::new();
        loop {
            match self.bump() {
                Some((_, '`')) => break,
                Some((off, '\\')) => match self.bump() {
                    Some((_, 'n')) => lit.push('\n'),
                    Some((_, 't')) => lit.push('\t'),
                    Some((_, '`')) => lit.push('`'),
                    Some((_, '{')) => lit.push('{'),
                    Some((_, '\\')) => lit.push('\\'),
                    Some((end, otro)) => {
                        return Err(SyntaxError::new(
                            format!("secuencia de escape inválida '\\{otro}'"),
                            Span::new(off, end + otro.len_utf8()),
                        ))
                    }
                    None => return Err(self.plantilla_sin_cerrar(start)),
                },
                Some((abre, '{')) => {
                    if !lit.is_empty() {
                        piezas.push(TemplatePiece::Lit(std::mem::take(&mut lit)));
                    }
                    // `{!expr}` inserta tal cual; `{expr}` escapa.
                    let crudo = matches!(self.peek_char(), Some('!'));
                    if crudo {
                        self.bump();
                    }
                    let ini = abre + 1 + usize::from(crudo);
                    let mut hondo = 0usize;
                    let mut en_cadena = false;
                    let fin;
                    loop {
                        match self.bump() {
                            Some((o, '}')) if hondo == 0 && !en_cadena => {
                                fin = o;
                                break;
                            }
                            Some((_, '}')) if !en_cadena => hondo -= 1,
                            Some((_, '{')) if !en_cadena => hondo += 1,
                            Some((_, '"')) => en_cadena = !en_cadena,
                            Some((_, '\\')) if en_cadena => {
                                self.bump();
                            }
                            Some(_) => {}
                            None => return Err(self.plantilla_sin_cerrar(start)),
                        }
                    }
                    let fuente = self.src[ini..fin].to_string();
                    if fuente.trim().is_empty() {
                        return Err(SyntaxError::new(
                            "hueco vacío en la plantilla",
                            Span::new(abre, fin + 1),
                        ));
                    }
                    piezas.push(TemplatePiece::Hueco { fuente, offset: ini, crudo });
                }
                Some((_, c)) => lit.push(c),
                None => return Err(self.plantilla_sin_cerrar(start)),
            }
        }
        if !lit.is_empty() {
            piezas.push(TemplatePiece::Lit(lit));
        }
        let end = self.peek_offset();
        Ok(Token::new(TokenKind::Template(piezas), Span::new(start, end)))
    }

    fn plantilla_sin_cerrar(&self, start: usize) -> SyntaxError {
        SyntaxError::new("plantilla sin cerrar", Span::new(start, self.src.len()))
    }

    fn lex_string(&mut self, start: usize) -> Result<Token, SyntaxError> {
        self.bump(); // consume la comilla de apertura
        let mut value = String::new();
        loop {
            match self.bump() {
                Some((_, '"')) => break,
                Some((off, '\\')) => match self.bump() {
                    Some((_, 'n')) => value.push('\n'),
                    Some((_, 't')) => value.push('\t'),
                    Some((_, 'r')) => value.push('\r'),
                    Some((_, '\\')) => value.push('\\'),
                    Some((_, '"')) => value.push('"'),
                    Some((_, '0')) => value.push('\0'),
                    Some((end, other)) => {
                        return Err(SyntaxError::new(
                            format!("secuencia de escape inválida '\\{}'", other),
                            Span::new(off, end + other.len_utf8()),
                        ))
                    }
                    None => {
                        return Err(SyntaxError::new(
                            "cadena sin cerrar",
                            Span::new(start, self.src.len()),
                        ))
                    }
                },
                Some((_, c)) => value.push(c),
                None => {
                    return Err(SyntaxError::new(
                        "cadena sin cerrar",
                        Span::new(start, self.src.len()),
                    ))
                }
            }
        }
        let end = self.peek_offset();
        Ok(Token::new(TokenKind::Str(value), Span::new(start, end)))
    }
}
