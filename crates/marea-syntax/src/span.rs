//! Posiciones dentro del código fuente.
//!
//! Un `Span` son dos offsets de byte `[start, end)`. Guardamos offsets (no
//! línea/columna) porque es barato y exacto; la traducción a línea/columna
//! solo se hace cuando hay que mostrar un error.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Span { start, end }
    }

    /// Une dos spans en el rango que los cubre a ambos.
    pub fn to(self, other: Span) -> Span {
        Span::new(self.start.min(other.start), self.end.max(other.end))
    }
}

/// Traduce un offset de byte a `(línea, columna)` 1-indexado, para diagnósticos.
pub fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in src.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}
