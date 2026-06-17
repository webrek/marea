//! Errores de sintaxis con renderizado amigable.

use crate::span::{line_col, Span};

#[derive(Debug, Clone, PartialEq)]
pub struct SyntaxError {
    pub message: String,
    pub span: Span,
}

impl SyntaxError {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        SyntaxError {
            message: message.into(),
            span,
        }
    }

    /// Renderiza el error con línea, columna y un cursor `^` bajo el código.
    pub fn render(&self, src: &str) -> String {
        let (line, col) = line_col(src, self.span.start);
        let line_str = src.lines().nth(line - 1).unwrap_or("");
        let width = self.span.end.saturating_sub(self.span.start).max(1);
        let caret = format!(
            "{}{}",
            " ".repeat(col.saturating_sub(1)),
            "^".repeat(width)
        );
        format!(
            "error: {}\n  --> línea {}, columna {}\n{:>4} | {}\n{:>4} | {}",
            self.message, line, col, line, line_str, "", caret
        )
    }
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} (en {}..{})", self.message, self.span.start, self.span.end)
    }
}

impl std::error::Error for SyntaxError {}
