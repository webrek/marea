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
        // 'render' solo acepta marcado seguro: es el sumidero del DOM.
        "render" => Some(Ty::Fn {
            params: vec![Ty::Html],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        // Longitud de una lista.
        "len" => Some(Ty::Fn {
            params: vec![Ty::Unknown],
            ret: Box::new(Ty::Int),
            location: None,
        }),
        // Convierte cualquier valor a su representación textual.
        "text" => Some(Ty::Fn {
            params: vec![Ty::Unknown],
            ret: Box::new(Ty::String),
            location: None,
        }),
        // Neutraliza un texto para incrustarlo en HTML. Necesario porque el
        // HTML se construye concatenando y `render` lo inyecta tal cual: sin
        // esto, cualquier dato que haya cruzado el RPC (que no valida tipos)
        // se ejecuta como marcado en el navegador de todos los visitantes.
        "escape" => Some(Ty::Fn {
            params: vec![Ty::Unknown],
            ret: Box::new(Ty::Html),
            location: None,
        }),
        // Confianza explícita: marca una cadena como marcado ya seguro. Es la
        // única puerta de String a Html, y por eso se ve en la revisión.
        "html" => Some(Ty::Fn {
            params: vec![Ty::String],
            ret: Box::new(Ty::Html),
            location: None,
        }),
        // Estado del servidor: 'guardar(x)' añade al store; 'todos()' lo lee.
        "save" => Some(Ty::Fn {
            params: vec![Ty::Unknown],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        "all" => Some(Ty::Fn {
            params: vec![],
            ret: Box::new(Ty::List(Box::new(Ty::Unknown))),
            location: None,
        }),
        "update" => Some(Ty::Fn {
            params: vec![Ty::Int, Ty::Unknown],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        "remove" => Some(Ty::Fn {
            params: vec![Ty::Int],
            ret: Box::new(Ty::Unit),
            location: None,
        }),
        // --- listas ---
        // `concat` y `append` se tipan aparte en `check_call` para conservar el
        // tipo del elemento (el verificador no tiene genéricos). Esta firma es
        // la de reserva.
        "append" => Some(Ty::Fn {
            params: vec![Ty::List(Box::new(Ty::Unknown)), Ty::Unknown],
            ret: Box::new(Ty::List(Box::new(Ty::Unknown))),
            location: None,
        }),
        // --- texto ---
        "contains" => Some(Ty::Fn {
            params: vec![Ty::String, Ty::String],
            ret: Box::new(Ty::Bool),
            location: None,
        }),
        "lower" => Some(Ty::Fn {
            params: vec![Ty::String],
            ret: Box::new(Ty::String),
            location: None,
        }),
        // --- red saliente (solo @server) ---
        // `fetch(url)` hace un GET y devuelve el cuerpo; `post(url, cuerpo)`
        // manda JSON. Son el puente a servicios externos: sin ellos el lenguaje
        // solo sabe hablar con su propio store y su propio cliente.
        "fetch" => Some(Ty::Fn {
            params: vec![Ty::String],
            ret: Box::new(Ty::String),
            location: None,
        }),
        "post" => Some(Ty::Fn {
            params: vec![Ty::String, Ty::String],
            ret: Box::new(Ty::String),
            location: None,
        }),
        // --- lectura de JSON ---
        // El lenguaje no tiene tipos dinámicos, así que una respuesta se lee por
        // ruta: `jsonTexto(cuerpo, "current.time")`, `jsonNumero(c, "a.0.b")`.
        "jsonText" => Some(Ty::Fn {
            params: vec![Ty::String, Ty::String],
            ret: Box::new(Ty::String),
            location: None,
        }),
        "jsonInt" => Some(Ty::Fn {
            params: vec![Ty::String, Ty::String],
            ret: Box::new(Ty::Int),
            location: None,
        }),
        "jsonFloat" => Some(Ty::Fn {
            params: vec![Ty::String, Ty::String],
            ret: Box::new(Ty::Float),
            location: None,
        }),
        "jsonLen" => Some(Ty::Fn {
            params: vec![Ty::String, Ty::String],
            ret: Box::new(Ty::Int),
            location: None,
        }),
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
        "Html" => Some(Ty::Html),
        "Unit" => Some(Ty::Unit),
        // `Record` es el tipo registro abierto: acceso a campo → Unknown.
        "Record" => Some(Ty::Unknown),
        _ => None,
    }
}

/// Todos los nombres de valor builtin, para consumidores externos (el servidor
/// de lenguaje los ofrece en el completado). Es la ÚNICA fuente: antes el LSP
/// mantenía su propia copia "espejo" y se desincronizó —no ofrecía ninguno de
/// los builtins del store—, así que aquí se exporta y allá se consume.
pub const VALUE_NAMES: &[&str] = &[
    "print",
    "concat",
    "render",
    "len",
    "text",
    "escape",
    "html",
    "save",
    "all",
    "update",
    "remove",
    "concat",
    "append",
    "len",
    "contains",
    "lower",
    "fetch",
    "post",
    "jsonText",
    "jsonInt",
    "jsonFloat",
    "jsonLen",
    "NotFound",
];

/// Todos los nombres de tipo builtin. `List` no está en `type_lookup` (se trata
/// como caso especial por su argumento) pero sí es un nombre de tipo válido.
pub const TYPE_NAMES: &[&str] = &[
    "Int", "Float", "Bool", "String", "Unit", "Html", "Record", "List",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// La lista exportada y la tabla real no pueden divergir.
    #[test]
    fn value_names_coincide_con_lookup() {
        for n in VALUE_NAMES {
            assert!(lookup(n).is_some(), "'{n}' está en VALUE_NAMES pero no en lookup()");
        }
    }

    #[test]
    fn type_names_coincide_con_type_lookup() {
        for n in TYPE_NAMES {
            // `List` es el único caso especial sin entrada en type_lookup.
            if *n == "List" {
                continue;
            }
            assert!(
                type_lookup(n).is_some(),
                "'{n}' está en TYPE_NAMES pero no en type_lookup()"
            );
        }
    }
}
