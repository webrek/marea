//! Representación interna de tipos del verificador.
//!
//! No es el `Type` sintáctico de `marea-syntax`: aquí los alias ya se pueden
//! resolver, las uniones llevan sus variantes nominales y los registros llevan
//! sus campos. `Unknown` es el comodín que silencia cascadas de errores.

use marea_syntax::ast::Location;

/// Un tipo del verificador.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    Int,
    Float,
    Bool,
    String,
    Unit,
    /// Marcado seguro para incrustar en el DOM. En runtime es una cadena; la
    /// distinción es puramente estática y existe para que el escapado deje de
    /// ser opcional: `render` solo acepta `Html`, y a `Html` solo se llega por
    /// `escapar(...)`, por `html("...")` (confianza explícita) o desde un
    /// literal del propio fuente, que es de confianza por construcción.
    Html,
    /// Un fragmento de JSON ya construido, para el `<script
    /// type="application/ld+json">` de una página. NO es `Html` con otro nombre,
    /// y la diferencia es de corrección: `Html` escapa `&` a `&amp;`, y dentro
    /// de un bloque de JSON-LD eso corrompe el JSON —el `&` de un nombre de
    /// producto deja de leerse—. Su única puerta es `json("...")`, confianza
    /// explícita como la de `html(...)`.
    Json,
    /// Lo que sirve una ruta que no es HTML: `sitemap.xml`, `robots.txt`. Es
    /// opaco a propósito —no tiene campos ni operaciones— y sólo se construye
    /// con `plainText(...)` o `xmlDoc(...)`, que es donde se decide el
    /// tipo de contenido.
    Response,
    /// Un almacén declarado con `store nombre: T;`. Lleva su nombre (para los
    /// mensajes) y el tipo de sus elementos.
    Store(String, Box<Ty>),
    /// Referencia a un alias de tipo declarado por el usuario (`User`).
    Named(String),
    /// Unión de variantes nominales (`User | NotFound`). Valor opaco: sólo se
    /// consume con `match`.
    Union(Vec<String>),
    /// Tipo de una función, con su ubicación de ejecución.
    Fn {
        params: Vec<Ty>,
        ret: Box<Ty>,
        location: Option<Location>,
    },
    /// Registro estructural con campos `nombre -> tipo`.
    Record(Vec<(String, Ty)>),
    /// Lista homogénea de elementos de un mismo tipo (`List<T>`).
    List(Box<Ty>),
    /// Tipo desconocido: absorbe operaciones sin generar errores en cascada.
    Unknown,
}

impl Ty {
    /// ¿Es un escalar primitivo?
    pub fn is_scalar(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float | Ty::Bool | Ty::String | Ty::Html)
    }

    /// Nombre legible para mensajes de error.
    pub fn display(&self) -> String {
        match self {
            Ty::Int => "Int".to_string(),
            Ty::Float => "Float".to_string(),
            Ty::Bool => "Bool".to_string(),
            Ty::String => "String".to_string(),
            Ty::Unit => "Unit".to_string(),
            Ty::Html => "Html".to_string(),
            Ty::Json => "Json".to_string(),
            Ty::Response => "Response".to_string(),
            Ty::Store(n, e) => format!("store {n}: {}", e.display()),
            Ty::Named(n) => n.clone(),
            Ty::Union(vs) => vs.join(" | "),
            Ty::Fn { params, ret, .. } => {
                let ps: Vec<String> = params.iter().map(|p| p.display()).collect();
                format!("fn({}) -> {}", ps.join(", "), ret.display())
            }
            Ty::Record(fields) => {
                let fs: Vec<String> = fields
                    .iter()
                    .map(|(n, t)| format!("{}: {}", n, t.display()))
                    .collect();
                format!("{{ {} }}", fs.join(", "))
            }
            Ty::List(elem) => format!("List<{}>", elem.display()),
            Ty::Unknown => "?".to_string(),
        }
    }
}
