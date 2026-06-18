//! Errores de tipado con renderizado amigable, al estilo de `SyntaxError`.

use marea_syntax::span::{line_col, Span};

/// Un error de tipado. `code` es un identificador estable (`E_...`); `notes`
/// son líneas extra de contexto (p. ej. "primera declaración aquí").
#[derive(Debug, Clone, PartialEq)]
pub struct TypeError {
    pub code: String,
    pub message: String,
    pub span: Span,
    pub notes: Vec<String>,
}

impl TypeError {
    pub fn new(code: impl Into<String>, message: impl Into<String>, span: Span) -> Self {
        TypeError {
            code: code.into(),
            message: message.into(),
            span,
            notes: Vec::new(),
        }
    }

    /// Variante con una nota de contexto.
    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    /// Renderiza el error con línea, columna y un cursor `^` bajo el código.
    pub fn render(&self, src: &str) -> String {
        let (line, col) = line_col(src, self.span.start);
        let line_str = src.lines().nth(line - 1).unwrap_or("");
        let width = self.span.end.saturating_sub(self.span.start).max(1);
        let caret = format!("{}{}", " ".repeat(col.saturating_sub(1)), "^".repeat(width));
        let mut out = format!(
            "error[{}]: {}\n  --> línea {}, columna {}\n{:>4} | {}\n{:>4} | {}",
            self.code, self.message, line, col, line, line_str, "", caret
        );
        for note in &self.notes {
            out.push_str(&format!("\n  nota: {note}"));
        }
        out
    }
}

impl std::fmt::Display for TypeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "[{}] {} (en {}..{})",
            self.code, self.message, self.span.start, self.span.end
        )
    }
}

impl std::error::Error for TypeError {}
