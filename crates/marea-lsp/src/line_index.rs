//! Índice de líneas: traduce entre offsets de byte (los que usa `Span`) y las
//! posiciones línea/carácter del protocolo LSP.
//!
//! El protocolo LSP, salvo negociación explícita de otra codificación, mide la
//! columna (`character`) en **unidades de código UTF-16**, no en bytes ni en
//! caracteres Unicode. Por eso este índice suma `len_utf16()` carácter a
//! carácter, no offsets de byte. Confundir estas tres unidades es el error
//! clásico al implementar un LSP.
//!
//! La aritmética interna trabaja en `usize`; al exponer al protocolo se hace
//! cast a `u32` saturado.
//!
//! No se reutiliza [`marea_syntax::span::line_col`] porque cuenta caracteres
//! 1-indexados (sirve para mensajes de consola, no para LSP).

use lsp_types::{Position, Range};
use marea_syntax::span::Span;

/// Índice de los inicios de línea de un texto, para mapear offsets de byte a
/// posiciones LSP y viceversa.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Offset de byte donde empieza cada línea. Siempre arranca con `0` y, por
    /// cada `\n`, contiene el offset del byte siguiente al salto.
    line_starts: Vec<usize>,
    /// Longitud total del texto en bytes; sirve de cota superior al hacer clamp.
    text_len: usize,
}

impl LineIndex {
    /// Construye el índice escaneando el texto una sola vez.
    ///
    /// `line_starts` queda como `[0, ...siguiente offset tras cada b'\n']`.
    /// Con CRLF (`\r\n`), el `\r` es el último byte de la línea previa y la
    /// nueva línea empieza justo después del `\n`.
    pub fn new(text: &str) -> Self {
        let bytes = text.as_bytes();
        let mut line_starts = Vec::with_capacity(bytes.len() / 16 + 1);
        line_starts.push(0);
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        LineIndex {
            line_starts,
            text_len: bytes.len(),
        }
    }

    /// Traduce un offset de byte a una [`Position`] LSP (0-indexada).
    ///
    /// Es tolerante: hace clamp del offset a `[0, text_len]` y, si cae a media
    /// secuencia UTF-8, retrocede al límite de carácter anterior. La columna se
    /// calcula sumando `len_utf16()` de cada carácter desde el inicio de línea,
    /// es decir en unidades de código UTF-16.
    pub fn offset_to_position(&self, offset: usize, text: &str) -> Position {
        let off = self.clamp_to_char_boundary(offset, text);

        // `partition_point` localiza la primera línea cuyo inicio es > off;
        // la línea de `off` es la anterior. Siempre hay al menos `[0]`, así que
        // el índice es >= 1 y la resta no se desborda.
        let line = self.line_starts.partition_point(|&start| start <= off) - 1;
        let line_start = self.line_starts[line];

        let mut character: usize = 0;
        for ch in text[line_start..off].chars() {
            character += ch.len_utf16();
        }

        Position {
            line: saturating_u32(line),
            character: saturating_u32(character),
        }
    }

    /// Traduce una [`Position`] LSP a un offset de byte.
    ///
    /// Avanza por los caracteres de la línea sumando `len_utf16()` hasta
    /// alcanzar `position.character` (sin partir un par suplente). Hace clamp de
    /// la línea al rango válido y del offset resultante a `text_len`.
    pub fn position_to_offset(&self, position: Position, text: &str) -> usize {
        if self.line_starts.is_empty() {
            return 0;
        }
        let last_line = self.line_starts.len() - 1;
        let line = (position.line as usize).min(last_line);
        let line_start = self.line_starts[line];

        // El final de la línea es el inicio de la siguiente o el final del
        // texto si es la última. Recortamos solo el `\n` final para que una
        // columna que rebase la línea haga clamp justo antes del salto y no
        // caiga en la línea siguiente. El `\r` de un CRLF se conserva: es un
        // carácter de la línea con su propia columna (ver la prueba `crlf`),
        // así que el round-trip sobre él debe respetarse.
        let mut line_end = self
            .line_starts
            .get(line + 1)
            .copied()
            .unwrap_or(self.text_len);
        if line_end > line_start && text.as_bytes()[line_end - 1] == b'\n' {
            line_end -= 1;
        }

        let target = position.character as usize;
        let mut utf16_seen: usize = 0;
        let mut offset = line_start;

        for ch in text[line_start..line_end].chars() {
            if utf16_seen >= target {
                break;
            }
            // No partir un par suplente: si añadir este carácter rebasa el
            // objetivo a media unidad, lo dejamos antes del carácter.
            let next = utf16_seen + ch.len_utf16();
            if next > target {
                break;
            }
            utf16_seen = next;
            offset += ch.len_utf8();
        }

        offset.min(self.text_len)
    }

    /// Traduce un [`Span`] de bytes a un [`Range`] LSP, con clamp en ambos
    /// extremos para no entrar en pánico ante spans fuera de rango.
    pub fn span_to_range(&self, span: Span, text: &str) -> Range {
        Range {
            start: self.offset_to_position(span.start, text),
            end: self.offset_to_position(span.end, text),
        }
    }

    /// Aplica clamp del offset a `[0, text_len]` y lo retrocede hasta el límite
    /// de carácter UTF-8 más cercano por debajo.
    fn clamp_to_char_boundary(&self, offset: usize, text: &str) -> usize {
        let mut off = offset.min(self.text_len);
        while off > 0 && !text.is_char_boundary(off) {
            off -= 1;
        }
        off
    }
}

/// Convierte un `usize` a `u32` saturando en `u32::MAX` en vez de desbordar.
fn saturating_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}
