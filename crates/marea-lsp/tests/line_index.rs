//! Pruebas del índice de líneas (`LineIndex`).
//!
//! Cubren el mapeo byte ⇄ posición LSP cuidando las tres unidades que suelen
//! confundirse: bytes (los del `Span`), caracteres Unicode y unidades de código
//! UTF-16 (las que mide la columna LSP).

use lsp_types::Position;
use marea_lsp::line_index::LineIndex;
use marea_syntax::span::Span;

/// Atajo: posición LSP a partir de un offset de byte.
fn pos(text: &str, off: usize) -> Position {
    LineIndex::new(text).offset_to_position(off, text)
}

#[test]
fn ascii_multilinea() {
    let text = "let x = 1\nlet y = 2\n";
    // Offset 10 = primer byte de la segunda línea (tras el `\n` del byte 9).
    assert_eq!(pos(text, 10), Position::new(1, 0));
    // Offset == longitud: línea 2 (tras el segundo `\n`), columna 0.
    assert_eq!(pos(text, text.len()), Position::new(2, 0));
    // Inicio del documento.
    assert_eq!(pos(text, 0), Position::new(0, 0));
    // Mitad de la primera línea.
    assert_eq!(pos(text, 4), Position::new(0, 4));
}

#[test]
fn acentos_un_code_unit_por_caracter() {
    // 'ñ' y 'á' ocupan 2 bytes en UTF-8 pero 1 unidad de código UTF-16.
    let text = "ñá x"; // bytes: ñ(0..2) á(2..4) ' '(4) 'x'(5)
                       // Tras 'ñ' (offset de byte 2) la columna es 1.
    assert_eq!(pos(text, 2), Position::new(0, 1));
    // Tras 'á' (offset de byte 4) la columna es 2.
    assert_eq!(pos(text, 4), Position::new(0, 2));
    // 'x' está en la columna 3 (offset de byte 5).
    assert_eq!(pos(text, 5), Position::new(0, 3));
}

#[test]
fn emoji_dos_code_units() {
    // 🌊 = 4 bytes UTF-8, 2 unidades de código UTF-16 (par suplente).
    let text = "a🌊b"; // a(0) 🌊(1..5) b(5)
                       // El byte 5 es justo después del emoji: columna = 1 (la 'a') + 2 (el emoji).
    assert_eq!(pos(text, 5), Position::new(0, 3));
    // La 'a' está en la columna 0; el emoji empieza en la columna 1.
    assert_eq!(pos(text, 1), Position::new(0, 1));
}

#[test]
fn span_con_emoji_end_character() {
    // "fn 🌊": f(0) n(1) ' '(2) 🌊(3..7)
    let text = "fn 🌊";
    let index = LineIndex::new(text);
    let range = index.span_to_range(Span::new(3, 7), text);
    // El emoji empieza en la columna 3 y, al ocupar 2 unidades UTF-16,
    // termina en la columna 5.
    assert_eq!(range.start, Position::new(0, 3));
    assert_eq!(range.end, Position::new(0, 5));
}

#[test]
fn crlf() {
    // "a\r\nb": el `\r` (byte 1) cierra la primera línea; la segunda empieza
    // tras el `\n` (byte 3).
    let text = "a\r\nb";
    assert_eq!(pos(text, 3), Position::new(1, 0));
    // El `\r` sigue contando como columna 1 de la primera línea.
    assert_eq!(pos(text, 1), Position::new(0, 1));
    // 'b' es la columna 1 de la segunda línea.
    assert_eq!(pos(text, 4), Position::new(1, 1));
}

#[test]
fn span_vacio() {
    let text = "let x = 1\n";
    let index = LineIndex::new(text);
    let range = index.span_to_range(Span::new(5, 5), text);
    assert_eq!(range.start, range.end);
    assert_eq!(range.start, Position::new(0, 5));
}

#[test]
fn offset_fuera_de_rango_sin_panico() {
    let text = "hola";
    // Muy por encima de la longitud: clamp al final del texto.
    assert_eq!(pos(text, 9999), Position::new(0, 4));
}

#[test]
fn offset_a_media_secuencia_utf8_sin_panico() {
    // 🌊 ocupa los bytes 0..4; el offset 2 cae a media secuencia.
    let text = "🌊x";
    // Debe retroceder al inicio del emoji (offset 0) → columna 0.
    assert_eq!(pos(text, 2), Position::new(0, 0));
    // Offset 1, también dentro del emoji, retrocede igual a 0.
    assert_eq!(pos(text, 1), Position::new(0, 0));
}

#[test]
fn position_to_offset_basico() {
    let text = "let x = 1\nlet y = 2\n";
    let index = LineIndex::new(text);
    assert_eq!(index.position_to_offset(Position::new(1, 0), text), 10);
    assert_eq!(index.position_to_offset(Position::new(0, 4), text), 4);
    assert_eq!(
        index.position_to_offset(Position::new(2, 0), text),
        text.len()
    );
}

#[test]
fn position_to_offset_no_parte_par_suplente() {
    // "a🌊b": pedir la columna 2 (a media unidad del emoji) no debe partir el
    // par suplente; el offset se queda al inicio del emoji (byte 1).
    let text = "a🌊b";
    let index = LineIndex::new(text);
    assert_eq!(index.position_to_offset(Position::new(0, 2), text), 1);
    // Columna 3 = justo después del emoji (byte 5).
    assert_eq!(index.position_to_offset(Position::new(0, 3), text), 5);
}

#[test]
fn position_to_offset_clamp() {
    let text = "abc\ndef";
    let index = LineIndex::new(text);
    // Línea fuera de rango: clamp a la última línea.
    let off = index.position_to_offset(Position::new(99, 0), text);
    assert_eq!(off, 4); // inicio de "def"
                        // Columna que rebasa la línea: clamp al fin de línea, no al siguiente.
    let off = index.position_to_offset(Position::new(0, 99), text);
    assert_eq!(off, 3); // fin de "abc", antes del `\n`
}

#[test]
fn round_trip_en_cada_limite_de_caracter() {
    // Propiedad: para todo límite de carácter `o`,
    // position_to_offset(offset_to_position(o)) == o.
    let textos = [
        "let x = 1\nlet y = 2\n",
        "a🌊b\nñá\r\n🌊🌊",
        "",
        "\n\n\n",
        "sin salto final",
        "fn 🌊\ng ñ",
    ];
    for text in textos {
        let index = LineIndex::new(text);
        for o in 0..=text.len() {
            if !text.is_char_boundary(o) {
                continue;
            }
            let p = index.offset_to_position(o, text);
            let back = index.position_to_offset(p, text);
            assert_eq!(
                back, o,
                "round-trip falló en texto {text:?} offset {o} (pos {p:?})"
            );
        }
    }
}

#[test]
fn texto_vacio() {
    let text = "";
    let index = LineIndex::new(text);
    assert_eq!(index.offset_to_position(0, text), Position::new(0, 0));
    assert_eq!(index.offset_to_position(5, text), Position::new(0, 0));
    assert_eq!(index.position_to_offset(Position::new(0, 0), text), 0);
    assert_eq!(index.position_to_offset(Position::new(9, 9), text), 0);
}
