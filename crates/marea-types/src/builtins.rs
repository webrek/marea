//! Builtins del lenguaje, hardcodeados y explícitos.
//!
//! Todo nombre que no sea parámetro, `let`, función declarada o builtin de aquí
//! es un error de resolución (`E_UNRESOLVED_NAME`).

use crate::ty::Ty;

/// Devuelve el tipo de un identificador builtin, si existe.
///
/// - `print(x) -> Unit`
/// - `concat(String, String) -> String`
/// - `render(x) -> Unit`
/// - `db`: objeto abierto (acceso a campo y llamada → `Unknown`)
/// - `NotFound`: variante nominal (se modela como `Union(["NotFound"])`)
/// - `Record`: tipo abierto (acceso a campo → `Unknown`)
pub fn lookup(name: &str) -> Option<Ty> {
    match name {
        "print" => Some(Ty::Fn {
            params: vec![Ty::Unknown],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        "concat" => Some(Ty::Fn {
            params: vec![Ty::String, Ty::String],
            ret: Box::new(Ty::String),
            location: None,
        }),
        "render" => Some(Ty::Fn {
            params: vec![Ty::Unknown],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        // Longitud de una lista.
        "len" => Some(Ty::Fn {
            params: vec![Ty::List(Box::new(Ty::Unknown))],
            ret: Box::new(Ty::Int),
            location: None,
        }),
        // Convierte cualquier valor a su representación textual.
        "aTexto" => Some(Ty::Fn {
            params: vec![Ty::Unknown],
            ret: Box::new(Ty::String),
            location: None,
        }),
        // Estado del servidor: 'guardar(x)' añade al store; 'todos()' lo lee.
        "guardar" => Some(Ty::Fn {
            params: vec![Ty::Unknown],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        "todos" => Some(Ty::Fn {
            params: vec![],
            ret: Box::new(Ty::List(Box::new(Ty::Unknown))),
            location: None,
        }),
        "actualizar" => Some(Ty::Fn {
            params: vec![Ty::Int, Ty::Unknown],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        "borrar" => Some(Ty::Fn {
            params: vec![Ty::Int],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        // Objeto abierto: cualquier miembro o llamada se resuelve a Unknown.
        "db" => Some(Ty::Unknown),
        // Variante nominal usable como etiqueta en uniones / patrones.
        "NotFound" => Some(Ty::Union(vec!["NotFound".to_string()])),
        _ => None,
    }
}

/// ¿`name` es un nombre de tipo builtin (primitivo o abierto)?
pub fn type_lookup(name: &str) -> Option<Ty> {
    match name {
        "Int" => Some(Ty::Int),
        "Float" => Some(Ty::Float),
        "Bool" => Some(Ty::Bool),
        "String" => Some(Ty::String),
        "Unit" => Some(Ty::Unit),
        // `Record` es el tipo registro abierto: acceso a campo → Unknown.
        "Record" => Some(Ty::Unknown),
        _ => None,
    }
}
