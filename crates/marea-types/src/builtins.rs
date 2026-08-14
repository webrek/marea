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
        // El modelo de eventos: `on("click", fn() { ... })` devuelve el ATRIBUTO
        // que ata un elemento a su manejador, así que su tipo es `Html` y entra
        // en un hueco crudo de plantilla como cualquier otro marcado. El resto
        // de reglas —qué eventos existen, qué forma tiene un manejador, dónde
        // vale ponerlo— vive en `crate::eventos`.
        "on" => Some(Ty::Fn {
            params: vec![
                Ty::String,
                Ty::Fn {
                    params: vec![],
                    ret: Box::new(Ty::Unit),
                    location: None,
                },
            ],
            ret: Box::new(Ty::Html),
            location: None,
        }),
        // Estado del servidor: 'save(a, x)' añade al almacén; 'all(a)' lo lee.
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
        // --- páginas: lo que no es HTML, y el JSON que no se puede escapar ---
        // Confianza explícita, igual que `html`: marca un texto como JSON ya
        // construido. Es la única puerta a `Json`, y existe porque el camino
        // fácil —reusar `Html`— corrompe el JSON-LD escapando el `&`.
        "json" => Some(Ty::Fn {
            params: vec![Ty::String],
            ret: Box::new(Ty::Json),
            location: None,
        }),
        // `robots.txt` y compañía: no hay marcado que escapar, así que entra
        // texto tal cual.
        "textoPlano" => Some(Ty::Fn {
            params: vec![Ty::String],
            ret: Box::new(Ty::Respuesta),
            location: None,
        }),
        // El sitemap EXIGE `Html`, y no es un capricho: XML escapa los mismos
        // cinco caracteres que HTML, así que la garantía que ya existe vale tal
        // cual y un nombre con `&` no puede romper el documento. No hace falta
        // un tipo `Xml`: sería `Html` con otro nombre.
        "documentoXml" => Some(Ty::Fn {
            params: vec![Ty::Html],
            ret: Box::new(Ty::Respuesta),
            location: None,
        }),
        // La query string de la petición. Devuelve cadena vacía si no está: una
        // query es opcional por definición, y un `String | NoEstá` obligaría a
        // un match en cada lectura para acabar poniendo "" en la otra rama.
        "consulta" => Some(Ty::Fn {
            params: vec![Ty::String],
            ret: Box::new(Ty::String),
            location: None,
        }),
        // Convertir texto a número puede fallar, así que el tipo lo dice y el
        // `match` obliga a decidir el valor por defecto. Tapa además un hueco
        // que va más allá del enrutado: hasta ahora no había forma de leer un
        // número de un texto.
        "entero" => Some(Ty::Fn {
            params: vec![Ty::String],
            ret: Box::new(Ty::Union(vec!["Int".to_string(), "NoEsNumero".to_string()])),
            location: None,
        }),
        // Variante nominal usable como etiqueta en uniones / patrones.
        "NotFound" => Some(Ty::Union(vec!["NotFound".to_string()])),
        _ => None,
    }
}

/// Builtins síncronos que la lista canónica —`marea_syntax::builtins::SINCRONOS`,
/// que consumen también el emisor y este crate— todavía no lista, porque ese
/// crate es de la otra mitad del reparto y no se toca desde aquí. Es la misma
/// excepción provisional que ya tenía `on`. Cuando se pueda tocar, estos nombres
/// van allí y esta lista se borra.
///
/// Ninguno pega a la red ni al disco: son cómputo puro sobre valores que ya
/// están en la mano (el texto de una query, una cadena que se marca como JSON).
const SINCRONOS_DE_PAGINA: &[&str] = &["json", "textoPlano", "documentoXml", "consulta", "entero"];

/// ¿Es `name` un builtin síncrono, contando los que aún no están en la lista
/// canónica? Lo que decide es si la llamada puede aparecer donde no hay dónde
/// esperar un `await` (el memo de una `reactive`, una global de módulo).
pub(crate) fn es_sincrono_local(name: &str) -> bool {
    marea_syntax::builtins::es_sincrono(name)
        || name == crate::eventos::ON
        || SINCRONOS_DE_PAGINA.contains(&name)
}

/// Los campos FIJOS de los registros builtin. Fijos porque son los que el
/// compilador sabe armar en el `<head>`: un registro abierto no se podría
/// emitir, y una etiqueta que no está aquí no es una etiqueta que exista.
///
/// `Pagina` es el retorno de una ruta que sirve HTML; `Meta` es una etiqueta
/// suelta (`og:type`, `twitter:card`) para lo que no tiene campo propio.
pub fn record_lookup(name: &str) -> Option<Vec<(String, Ty)>> {
    let campos: Vec<(&str, Ty)> = match name {
        "Pagina" => vec![
            ("titulo", Ty::String),
            ("descripcion", Ty::String),
            ("canonica", Ty::String),
            ("metas", Ty::List(Box::new(Ty::Named("Meta".to_string())))),
            ("jsonld", Ty::List(Box::new(Ty::Json))),
            ("cuerpo", Ty::Html),
        ],
        "Meta" => vec![("clave", Ty::String), ("valor", Ty::String)],
        _ => return None,
    };
    Some(campos.into_iter().map(|(n, t)| (n.to_string(), t)).collect())
}

/// Los campos de un registro builtin que se pueden OMITIR al construirlo.
///
/// En `Pagina` son cuatro, y la lista es la decisión: el vacío de una
/// descripción, de las metas, del JSON-LD o del cuerpo significa algo —no hay—.
/// El de un título o el de una canónica no significa nada: es el fallo de SEO
/// más caro que existe y no lo avisa nadie, así que hay que escribirlos.
pub fn campos_omitibles(name: &str) -> &'static [&'static str] {
    match name {
        "Pagina" => &["descripcion", "metas", "jsonld", "cuerpo"],
        _ => &[],
    }
}

/// Por qué hay que escribir un campo builtin que no se puede omitir. Va tal cual
/// en el error: la regla sola no convence a nadie, el motivo sí.
pub fn porque_obligatorio(tipo: &str, campo: &str) -> &'static str {
    match (tipo, campo) {
        ("Pagina", "titulo") => "es lo que un buscador enseña como enlace",
        ("Pagina", "canonica") => "sin ella, dos URLs con el mismo contenido compiten entre sí",
        _ => "el tipo no tiene un valor por defecto para él",
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
        // Los tipos de una página. `Pagina` y `Meta` son registros de campos
        // fijos (sus campos están en `record_lookup`), así que su tipo es el
        // nombre y se resuelven como cualquier otro registro nombrado.
        "Pagina" => Some(Ty::Named("Pagina".to_string())),
        "Meta" => Some(Ty::Named("Meta".to_string())),
        "Json" => Some(Ty::Json),
        "Respuesta" => Some(Ty::Respuesta),
        // `Record` es el tipo registro abierto: acceso a campo → Unknown.
        "Record" => Some(Ty::Unknown),
        // Marcador de política: `@server(Public)` es decir "aquí no exijo
        // identidad", explícitamente. No tiene valores —no hay forma de
        // construir uno— así que sólo sirve donde va una política.
        "Public" => Some(Ty::Named("Public".to_string())),
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
    "on",
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
    "json",
    "textoPlano",
    "documentoXml",
    "consulta",
    "entero",
    "NotFound",
];

/// Todos los nombres de tipo builtin. `List` no está en `type_lookup` (se trata
/// como caso especial por su argumento) pero sí es un nombre de tipo válido.
pub const TYPE_NAMES: &[&str] = &[
    "Int",
    "Float",
    "Bool",
    "String",
    "Unit",
    "Html",
    "Record",
    "List",
    "Public",
    "Pagina",
    "Meta",
    "Json",
    "Respuesta",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// La lista exportada y la tabla real no pueden divergir.
    #[test]
    fn value_names_coincide_con_lookup() {
        for n in VALUE_NAMES {
            assert!(
                lookup(n).is_some(),
                "'{n}' está en VALUE_NAMES pero no en lookup()"
            );
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

    /// Un campo omitible mal escrito no rompe nada visible: simplemente deja de
    /// coincidir con el campo real, que pasa a ser obligatorio sin que nadie lo
    /// haya decidido. Es el tipo de deriva que sólo se ve cuando ya molesta.
    #[test]
    fn los_campos_omitibles_existen_en_su_registro() {
        for tipo in ["Pagina", "Meta"] {
            let campos = record_lookup(tipo).expect("es un registro builtin");
            for omitible in campos_omitibles(tipo) {
                assert!(
                    campos.iter().any(|(n, _)| n == omitible),
                    "'{omitible}' se puede omitir en '{tipo}' pero no es un campo suyo"
                );
            }
        }
    }

    /// Los dos campos que hay que escribir sí o sí son el título y la canónica:
    /// es la decisión de diseño de `Pagina`, y aquí queda fijada.
    #[test]
    fn en_una_pagina_solo_titulo_y_canonica_son_obligatorios() {
        let campos = record_lookup("Pagina").expect("Pagina es un registro builtin");
        let obligatorios: Vec<&str> = campos
            .iter()
            .map(|(n, _)| n.as_str())
            .filter(|n| !campos_omitibles("Pagina").contains(n))
            .collect();
        assert_eq!(obligatorios, vec!["titulo", "canonica"]);
    }
}
