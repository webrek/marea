//! `marea-types` — verificador de tipos de Marea (v1.5).
//!
//! Dos fases:
//!   - **Fase A (recolección global):** registra todas las firmas de `fn` y los
//!     alias de `type` antes de chequear cuerpos, lo que permite recursión mutua
//!     y orden libre de declaraciones.
//!   - **Fase B (chequeo de cuerpos):** recorre cada función con una pila de
//!     scopes léxicos, validando resolución, tipos, fronteras de red y uniones.
//!
//! La regla protagonista es la unión + `match`: un valor `A | B` es opaco
//! (no asignable a `A`, sin campos) y sólo se consume con `match`, que estrecha
//! (narrowing) cada rama a su variante.

pub mod builtins;

// El verificador se implementa a lo largo de estos módulos: cada uno
// aporta su bloque `impl Checker`.
mod collect;
mod error;
mod expr;
mod frontera;
mod stmt;
mod subtipado;
mod ty;
mod uniones;

pub use error::TypeError;
pub use ty::Ty;

use marea_syntax::ast::{
    BinOp, Block, ElseBranch, Expr, FnDecl, Item, Location, Module, Pattern, Stmt, Type, UnaryOp,
};
use marea_syntax::builtins::es_sincrono;
use marea_syntax::span::Span;
use std::collections::HashMap;

/// Un cruce de frontera de red detectado en una llamada.
///
/// Se expone para poder testear la clasificación de ubicaciones.
#[derive(Debug, Clone, PartialEq)]
pub struct BoundaryCrossing {
    /// Nombre de la función llamada.
    pub callee: String,
    /// Ubicación desde la que se llama (la del `fn` actual).
    pub from: Option<Location>,
    /// Ubicación de la función llamada.
    pub to: Option<Location>,
    pub span: Span,
}

/// Firma global de una función (Fase A).
#[derive(Debug, Clone)]
struct FnSig {
    params: Vec<Ty>,
    ret: Ty,
    location: Option<Location>,
    /// Nombres del tipo de retorno tal como se escribieron (`User`,
    /// `User | NotFound`). El `Ty` ya viene con los alias expandidos, y para
    /// construir el tipo de un recurso —`Cargando | User | Fallo`— hace falta el
    /// nombre, no la forma. Vacío si el retorno no es nombrable (un registro
    /// escrito en línea, por ejemplo).
    ret_nombres: Vec<String>,
}

/// Punto de entrada: chequea un módulo y acumula TODOS los errores.
/// Un `Vec` vacío significa que el módulo tipa.
pub fn check(module: &Module) -> Vec<TypeError> {
    let mut checker = Checker::new();
    checker.collect(module);
    checker.collect_globals(module);
    checker.check_bodies(module);
    checker.errors
}

/// Variante de [`check`] que además devuelve los cruces de frontera detectados.
pub fn check_with_boundaries(module: &Module) -> (Vec<TypeError>, Vec<BoundaryCrossing>) {
    let mut checker = Checker::new();
    checker.collect(module);
    checker.collect_globals(module);
    checker.check_bodies(module);
    (checker.errors, checker.crossings)
}

struct Checker {
    /// Firmas de funciones por nombre (Fase A).
    fns: HashMap<String, FnSig>,
    /// Alias de tipo por nombre (Fase A): `type T = ...`.
    aliases: HashMap<String, Type>,
    /// Alias detectados como cíclicos (Fase A): cortan la resolución recursiva
    /// en Fase B para no desbordar la pila.
    cyclic: std::collections::HashSet<String>,
    /// Tipo de elemento del store del servidor (Fase A), declarado con `store T;`.
    /// Tipa `guardar(T)` y `todos() -> List<T>`. `None` si no se declaró.
    /// Almacenes declarados con `store nombre: T;`, por nombre. Un módulo puede
    /// tener varios: el nombre se pasa como primer argumento a los builtins de
    /// estado, así que `todos(productos)` y `todos(ordenes)` son distintos.
    stores: HashMap<String, Ty>,
    /// Variables de nivel superior (`let`/`reactive` de módulo) y su mutabilidad.
    /// Visibles desde cualquier función; son el estado reactivo de la app.
    globals: HashMap<String, (Ty, bool)>,
    /// Nombres de las globales declaradas `reactive`. Son estado de UI que vive
    /// en el cliente, así que leerlas desde `@server` es un error de ubicación
    /// (simétrico a `E_STATE_OFF_SERVER` para el store).
    reactive_globals: std::collections::HashSet<String>,
    /// Pila de scopes léxicos de variables (Fase B). El bool es la mutabilidad
    /// (`true` si se declaró `mut`/`reactive`); se usa para rechazar reasignar
    /// un binding inmutable.
    scopes: Vec<HashMap<String, (Ty, bool)>>,
    /// Ubicación de la función que se está chequeando (Fase B).
    current_location: Option<Location>,
    /// Contexto de inicializador que NO puede cruzar la red: el de una variable
    /// `reactive` (se compila a un memo síncrono) o el de una global de módulo
    /// (se evalúa al importar). Lleva la etiqueta para el mensaje de error.
    init_context: Option<&'static str>,
    /// Tipo de retorno declarado de la función actual.
    current_return: Ty,
    errors: Vec<TypeError>,
    crossings: Vec<BoundaryCrossing>,
}

impl Checker {
    pub(crate) fn new() -> Self {
        Checker {
            fns: HashMap::new(),
            aliases: HashMap::new(),
            cyclic: std::collections::HashSet::new(),
            stores: HashMap::new(),
            globals: HashMap::new(),
            reactive_globals: std::collections::HashSet::new(),
            scopes: Vec::new(),
            current_location: None,
            init_context: None,
            current_return: Ty::Unit,
            errors: Vec::new(),
            crossings: Vec::new(),
        }
    }

    pub(crate) fn error(&mut self, e: TypeError) {
        self.errors.push(e);
    }

    /// Comprueba el acceso (lectura o escritura) a una global `reactive`. Es
    /// estado de UI: solo se emite en el bundle del cliente. Se prohíbe desde
    /// `@server`/`@edge` —que compilaba a un `ReferenceError` en cada RPC— y
    /// también desde una función SIN anotación, porque el codegen la duplica en
    /// los dos bundles y el del servidor no tiene el estado. Simétrico a
    /// `E_STATE_OFF_SERVER`, que exige lo contrario para el store.
    pub(crate) fn check_reactive_access(&mut self, name: &str, span: Span) {
        if !self.reactive_globals.contains(name) {
            return;
        }
        let fuera_del_cliente = !matches!(self.current_location, Some(Location::Client));
        if fuera_del_cliente {
            let donde = match self.current_location {
                Some(Location::Server) => "una función @server",
                Some(Location::Edge) => "una función @edge",
                _ => "una función sin anotación (se emite también en el servidor)",
            };
            self.error(TypeError::new(
                "E_REACTIVE_OFF_CLIENT",
                format!(
                    "'{name}' es estado reactivo del cliente y no existe en el servidor; \
                     no puede usarse desde {donde}: márcala @client o pásalo como argumento"
                ),
                span,
            ));
        }
    }
}

/// Nombre textual de un callee (`getUser`, `todos`). Lo usan tanto el tipado de
/// llamadas como la clasificación de cruces de frontera, así que vive aquí.
pub(crate) fn callee_name(callee: &Expr) -> String {
    match callee {
        Expr::Ident { name, .. } => name.clone(),
        Expr::Member { object, field, .. } => format!("{}.{}", callee_name(object), field),
        _ => "<expr>".to_string(),
    }
}

/// Identificadores que el bundle importa del runtime. No son builtins del
/// lenguaje (no se pueden llamar desde un `.mar`), pero sí ocupan el espacio de
/// nombres del archivo generado: declarar uno produce un `const`/`function` que
/// redeclara el import y el archivo entero deja de cargar.
pub(crate) fn es_interno_del_runtime(name: &str) -> bool {
    matches!(
        name,
        "__register"
            | "__rpc"
            | "__index"
            | "__marea_is"
            | "__signal"
            | "__memo"
            | "__effect"
            | "startServer"
            | "stopServer"
            | "puerto"
    )
}
