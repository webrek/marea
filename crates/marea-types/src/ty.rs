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
    /// Tipo desconocido: absorbe operaciones sin generar errores en cascada.
    Unknown,
}

impl Ty {
    /// ¿Es un escalar primitivo?
    pub fn is_scalar(&self) -> bool {
        matches!(self, Ty::Int | Ty::Float | Ty::Bool | Ty::String)
    }

    /// Nombre legible para mensajes de error.
    pub fn display(&self) -> String {
        match self {
            Ty::Int => "Int".to_string(),
            Ty::Float => "Float".to_string(),
            Ty::Bool => "Bool".to_string(),
            Ty::String => "String".to_string(),
            Ty::Unit => "Unit".to_string(),
            Ty::Named(n) => n.clone(),
            Ty::Union(vs) => vs.join(" | "),
            Ty::Fn { params, ret, .. } => {
                let ps: Vec<String> = params.iter().map(|p| p.display()).collect();
                format!("fn({}) -> {}", ps.join(", "), ret.display())
            }
            Ty::Record(fields) => {
                let fs: Vec<String> =
                    fields.iter().map(|(n, t)| format!("{}: {}", n, t.display())).collect();
                format!("{{ {} }}", fs.join(", "))
            }
            Ty::Unknown => "?".to_string(),
        }
    }
}
