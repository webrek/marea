//! Verificación adversarial del índice de líneas (`LineIndex`).
//!
//! Ataca el mapeo byte ⇄ UTF-16, el punto donde más fallan los servidores LSP.
//! Cada prueba está construida para FALLAR si la columna se midiera por bytes o
//! por caracteres Unicode en vez de por unidades de código UTF-16, y para
//! confirmar que offsets límite (fin de archivo, media secuencia, fuera de
//! rango) no provocan pánico.

use lsp_types::Position;
use marea_lsp::line_index::LineIndex;
use marea_syntax::span::Span;

/// Atajo: posición LSP a partir de un offset de byte.
fn pos(text: &str, off: usize) -> Position {
    LineIndex::new(text).offset_to_position(off, text)
}

/// 𝐀 (U+1D400, MATHEMATICAL BOLD CAPITAL A): fuera del BMP.
/// UTF-8: 4 bytes. Unicode: 1 carácter. UTF-16: 2 unidades (par suplente).
/// Sirve para distinguir las tres unidades sin ambigüedad.
#[test]
fn fuera_del_bmp_da_dos_code_units_no_bytes_ni_chars() {
    let text = "a𝐀b"; // a(0) 𝐀(1..5) b(5)
    assert_eq!(text.len(), 6, "control: 1 + 4 + 1 bytes");

    // Inicio del carácter: columna 1 (tras la 'a').
    assert_eq!(pos(text, 1), Position::new(0, 1));

    // Tras 𝐀 (byte 5): columna = 1 ('a') + 2 (par suplente) = 3.
    // Si fuera por bytes daría 5; si fuera por chars daría 2. Debe ser 3.
    let p = pos(text, 5);
    assert_eq!(
        p,
        Position::new(0, 3),
        "columna por UTF-16 esperada 3; por bytes sería 5, por chars 2; obtenido {p:?}"
    );

    // Fin del texto (byte 6): la 'b' añade 1 → columna 4.
    assert_eq!(pos(text, 6), Position::new(0, 4));
}

/// Round-trip sobre 𝐀 sin partir el par suplente: pedir la columna 2
/// (a media unidad del par) debe quedarse al inicio del carácter.
#[test]
fn fuera_del_bmp_round_trip_y_no_parte_par_suplente() {
    let text = "a𝐀b";
    let index = LineIndex::new(text);

    // Columna 2 cae a mitad del par suplente → offset al inicio de 𝐀 (byte 1).
    assert_eq!(index.position_to_offset(Position::new(0, 2), text), 1);
    // Columna 3 = justo después de 𝐀 (byte 5).
    assert_eq!(index.position_to_offset(Position::new(0, 3), text), 5);

    // Round-trip en cada límite de carácter.
    for o in [0usize, 1, 5, 6] {
        let p = index.offset_to_position(o, text);
        assert_eq!(index.position_to_offset(p, text), o, "round-trip en offset {o}");
    }
}

/// Una línea con varios caracteres fuera del BMP seguidos: las columnas deben
/// avanzar de 2 en 2 (UTF-16), no de 1 en 1 (chars) ni de 4 en 4 (bytes).
#[test]
fn varios_no_bmp_seguidos_columnas_de_dos_en_dos() {
    // 🌊 y 𝐀, ambos pares suplentes.
    let text = "🌊𝐀🌊"; // 🌊(0..4) 𝐀(4..8) 🌊(8..12)
    assert_eq!(text.len(), 12);
    assert_eq!(pos(text, 0), Position::new(0, 0));
    assert_eq!(pos(text, 4), Position::new(0, 2)); // tras el 1.º
    assert_eq!(pos(text, 8), Position::new(0, 4)); // tras el 2.º
    assert_eq!(pos(text, 12), Position::new(0, 6)); // tras el 3.º
}

/// Saltos de línea mixtos `\n`, `\r\n` y un `\r` solitario (que NO abre línea
/// en LSP por defecto). El conteo de líneas se rige solo por `\n`.
#[test]
fn saltos_mixtos_solo_lf_abre_linea() {
    // "a\nb\r\nc\rd"
    //  a(0) \n(1) b(2) \r(3) \n(4) c(5) \r(6) d(7)
    let text = "a\nb\r\nc\rd";

    // Línea 0: "a".
    assert_eq!(pos(text, 0), Position::new(0, 0));
    // Tras el primer \n: inicio de línea 1.
    assert_eq!(pos(text, 2), Position::new(1, 0)); // 'b'
    // El \r del CRLF es columna 1 de la línea 1.
    assert_eq!(pos(text, 3), Position::new(1, 1));
    // Tras el \n del CRLF: inicio de línea 2.
    assert_eq!(pos(text, 5), Position::new(2, 0)); // 'c'
    // El \r solitario NO abre línea: 'd' sigue en la línea 2.
    // 'c'(col 0) \r(col 1) 'd'(col 2), todos en línea 2.
    assert_eq!(pos(text, 6), Position::new(2, 1)); // el \r solitario
    assert_eq!(pos(text, 7), Position::new(2, 2)); // 'd'
    assert_eq!(pos(text, text.len()), Position::new(2, 3)); // fin de línea 2
}

/// Round-trip con saltos mixtos y un `\r` solitario en medio.
#[test]
fn round_trip_saltos_mixtos() {
    let text = "a\nb\r\nc\rd𝐀\n🌊";
    let index = LineIndex::new(text);
    for o in 0..=text.len() {
        if !text.is_char_boundary(o) {
            continue;
        }
        let p = index.offset_to_position(o, text);
        let back = index.position_to_offset(p, text);
        assert_eq!(back, o, "round-trip falló en offset {o} (pos {p:?})");
    }
}

/// Fin de archivo con multibyte: offset == text_len justo tras un par suplente,
/// sin salto final. La posición debe reflejar las unidades UTF-16 acumuladas.
#[test]
fn fin_de_archivo_tras_par_suplente_sin_salto() {
    let text = "x𝐀"; // x(0) 𝐀(1..5), sin '\n' final
    assert_eq!(text.len(), 5);
    // Fin del texto: columna = 1 ('x') + 2 (par) = 3, línea 0.
    assert_eq!(pos(text, text.len()), Position::new(0, 3));
    // Round-trip del fin de archivo.
    let index = LineIndex::new(text);
    let p = index.offset_to_position(text.len(), text);
    assert_eq!(index.position_to_offset(p, text), text.len());
}

/// Span que termina exactamente en el fin del archivo sobre un par suplente.
#[test]
fn span_hasta_fin_de_archivo_con_par_suplente() {
    let text = "ab🌊"; // a(0) b(1) 🌊(2..6)
    let index = LineIndex::new(text);
    let range = index.span_to_range(Span::new(2, 6), text);
    assert_eq!(range.start, Position::new(0, 2)); // inicio del emoji
    assert_eq!(range.end, Position::new(0, 4)); // 2 + 2 unidades UTF-16
}

/// Offset a media secuencia UTF-8 de un par suplente fuera del BMP: cada byte
/// interno debe retroceder al inicio del carácter sin pánico.
#[test]
fn media_secuencia_no_bmp_retrocede_sin_panico() {
    let text = "𝐀z"; // 𝐀(0..4) z(4)
    // Bytes 1, 2, 3 caen dentro del par suplente → retroceden a 0 → columna 0.
    for off in [1usize, 2, 3] {
        assert_eq!(pos(text, off), Position::new(0, 0), "byte interno {off}");
    }
    // Byte 4 = inicio de 'z' → columna 2 (par suplente + nada aún de 'z').
    assert_eq!(pos(text, 4), Position::new(0, 2));
}

/// Offsets disparatadamente grandes y `usize::MAX` no entran en pánico: clamp.
#[test]
fn offset_gigante_sin_panico() {
    let text = "ñ\n🌊"; // ñ(0..2) \n(2) 🌊(3..7)
    let index = LineIndex::new(text);
    // usize::MAX hace clamp al final del texto (línea 1, tras el emoji).
    let p = index.offset_to_position(usize::MAX, text);
    assert_eq!(p, Position::new(1, 2));
    // Posición disparatada en línea y columna hace clamp dentro del texto.
    let off = index.position_to_offset(Position::new(u32::MAX, u32::MAX), text);
    assert_eq!(off, text.len());
}

/// La posición de fin de línea ante una columna que rebasa NO debe saltar a la
/// línea siguiente ni partir un carácter multibyte previo al `\n`.
#[test]
fn columna_que_rebasa_se_queda_antes_del_salto() {
    let text = "a🌊\nz"; // a(0) 🌊(1..5) \n(5) z(6)
    let index = LineIndex::new(text);
    // Columna 99 en línea 0: clamp al fin de línea 0 (justo antes del \n, byte 5).
    let off = index.position_to_offset(Position::new(0, 99), text);
    assert_eq!(off, 5);
    // Y esa posición de vuelta es la columna 3 (a + emoji), no la línea 1.
    assert_eq!(index.offset_to_position(off, text), Position::new(0, 3));
}

/// Combinación caracteres BMP de 3 bytes (no suplentes) + ASCII: 3 bytes pero
/// 1 unidad UTF-16. Distingue bytes de UTF-16 en el rango intermedio.
#[test]
fn bmp_tres_bytes_una_unidad_utf16() {
    // '中' U+4E2D: 3 bytes UTF-8, 1 unidad UTF-16.
    let text = "中文x"; // 中(0..3) 文(3..6) x(6)
    assert_eq!(text.len(), 7);
    assert_eq!(pos(text, 3), Position::new(0, 1)); // tras '中'
    assert_eq!(pos(text, 6), Position::new(0, 2)); // tras '文'
    assert_eq!(pos(text, 7), Position::new(0, 3)); // tras 'x'
}
