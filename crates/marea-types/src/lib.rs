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
/// El modelo de eventos (`on`). Público porque la lista de eventos válidos es
/// la única que hay: quien la necesite —el completado del editor, mañana— la
/// consume de aquí en vez de copiarla.
pub mod eventos;

// El verificador se implementa a lo largo de estos módulos: cada uno
// aporta su bloque `impl Checker`.
mod collect;
mod error;
mod expr;
mod frontera;
mod paginas;
mod programa;
mod stmt;
mod subtipado;
mod ty;
mod uniones;

pub use error::TypeError;
pub use programa::{check_program, check_program_with_boundaries, ProgramTypeError};
pub use ty::Ty;

use marea_syntax::ast::{
    BinOp, Block, ElseBranch, Expr, FnDecl, Item, Location, Module, Pattern, Stmt, Type, UnaryOp,
};
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

/// La función `@session` del programa (Fase A). Su presencia es lo que activa
/// la exigencia de política: sin identidad declarada no hay a quién exigirle
/// nada, y en cuanto la hay, `@server` a secas deja de compilar.
#[derive(Debug, Clone)]
struct Sesion {
    /// Nombre de la función `@session`. La invoca el runtime, no el programa.
    fn_name: String,
    /// Nombre del tipo de identidad que resuelve. Es el único que puede
    /// aparecer como política de un handler.
    identidad: String,
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
    // La identidad se recolecta después de los alias (necesita resolverlos) y
    // antes de las políticas, que se comparan contra ella.
    checker.collect_session(module);
    checker.check_politicas(module);
    checker.check_paginas(module);
    checker.check_bodies(module);
    checker.errors
}

/// Variante de [`check`] que además devuelve los cruces de frontera detectados.
pub fn check_with_boundaries(module: &Module) -> (Vec<TypeError>, Vec<BoundaryCrossing>) {
    let mut checker = Checker::new();
    checker.collect(module);
    checker.collect_globals(module);
    // La identidad se recolecta después de los alias (necesita resolverlos) y
    // antes de las políticas, que se comparan contra ella.
    checker.collect_session(module);
    checker.check_politicas(module);
    checker.check_paginas(module);
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
    /// Almacenes declarados con `store nombre: T;`, por nombre (Fase A). Un
    /// módulo puede tener varios: el nombre se pasa como primer argumento a los
    /// builtins de estado, así que `all(productos)` y `all(ordenes)` son
    /// distintos, y `save(ordenes, p)` con un Producto es un error de tipos.
    stores: HashMap<String, Ty>,
    /// Almacenes PRESTADOS (`store p: T from "tabla";`), por nombre, con la
    /// tabla ajena que mapean. Un almacén propio no aparece aquí.
    ///
    /// La marca vive aparte y no dentro de `Ty::Store` porque no es del TIPO:
    /// `all(p)` sigue dando `List<T>` se preste la tabla o no. Lo que cambia es
    /// quién manda en ella, y eso decide una sola cosa —que escribir está
    /// prohibido—, así que no tiene por qué contaminar cada comparación de
    /// tipos.
    stores_prestados: HashMap<String, String>,
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
    /// Índices dentro de `scopes` donde empieza el cuerpo de cada cierre que se
    /// está chequeando (el más interno al final). Una variable que se resuelve
    /// POR DEBAJO de la última marca no es local del cierre: la captura.
    closure_bases: Vec<usize>,
    /// Tipos de los `return` vistos hasta ahora en el cierre actual, cuando no
    /// escribió `-> T` y hay que deducirlo de ellos. `None` en cualquier otro
    /// caso, que es lo que distingue "acumular" de "comparar".
    inferring_return: Option<Vec<Ty>>,
    /// La `@session` del programa (Fase A), si la declara.
    session: Option<Sesion>,
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
            stores_prestados: HashMap::new(),
            globals: HashMap::new(),
            reactive_globals: std::collections::HashSet::new(),
            scopes: Vec::new(),
            current_location: None,
            init_context: None,
            current_return: Ty::Unit,
            closure_bases: Vec::new(),
            inferring_return: None,
            session: None,
            errors: Vec::new(),
            crossings: Vec::new(),
        }
    }

    pub(crate) fn error(&mut self, e: TypeError) {
        self.errors.push(e);
    }

    /// Un uso de `name` que se resolvió en el scope número `idx`: ¿es una
    /// captura, y de algo que no debería capturarse?
    ///
    /// Marea no tiene referencias —todo es un valor— así que un cierre COPIA lo
    /// que captura en el momento de crearse. Sobre algo `mut` eso monta una
    /// trampa silenciosa: dentro se trabaja sobre la copia, de modo que
    /// reasignar fuera no se ve dentro ni al revés, y las dos mitades del
    /// programa creen tener la misma variable. Decirlo aquí cuesta un error;
    /// callarlo cuesta una tarde de depuración.
    ///
    /// Las globales de módulo NO pasan por esta regla: no se copian al entorno
    /// del cierre, viven en el ámbito del módulo, así que leerlas desde un
    /// cierre es exactamente lo mismo que leerlas desde una función con nombre
    /// —y para el estado reactivo ya manda `E_REACTIVE_OFF_CLIENT`.
    pub(crate) fn check_captura(&mut self, name: &str, idx: usize, mutable: bool, span: Span) {
        // Fuera de un cierre no hay nada que capturar.
        let Some(&base) = self.closure_bases.last() else {
            return;
        };
        // Declarada dentro del propio cierre: es local suya, no una captura.
        if idx >= base || !mutable {
            return;
        }
        self.error(TypeError::new(
            "E_CAPTURA_MUTABLE",
            format!(
                "'{name}' se declaró 'mut' y un cierre captura por valor: aquí dentro sería \
                 una copia, así que reasignarla fuera no se vería dentro (ni al revés). \
                 Cópiala a un 'let' antes del cierre, o pásala como parámetro"
            ),
            span,
        ));
    }

    /// Busca un nombre en la pila de scopes, del más interno al más externo, y
    /// devuelve en qué scope estaba junto con su tipo y su mutabilidad. El
    /// índice es lo que permite saber si el uso cruza la frontera de un cierre.
    pub(crate) fn buscar_en_scopes(&self, name: &str) -> Option<(usize, Ty, bool)> {
        self.scopes
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, s)| s.get(name).map(|(t, m)| (i, t.clone(), *m)))
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

/// Dónde corre una función, contando lo que su anotación implica sin escribirlo.
///
/// `@page` no dice `@server`, pero una página se renderiza en el servidor: es lo
/// que lee un buscador, y la interactividad va encima con islas. Así que le
/// caen las mismas reglas que ya existen —puede tocar el almacén, no puede tocar
/// estado reactivo del cliente, no puede llamar a una `@client`— sin una sola
/// regla nueva que escribir ni recordar. Todo lo que mire la ubicación de una
/// función declarada tiene que pasar por aquí; leer `f.location` a pelo deja las
/// páginas fuera de la mitad de las reglas, que es justo el error que esto evita.
///
/// Lo que NO hereda es la política (`@server(Public)`): una página es una URL
/// pública por construcción —se sirve para que la indexen— así que no hay a
/// quién exigirle identidad. Por eso `check_politicas` sigue mirando
/// `f.location`, que en una página es `None`.
pub(crate) fn ubicacion_efectiva(f: &FnDecl) -> Option<Location> {
    match f.location {
        Some(l) => Some(l),
        None if f.ruta.is_some() => Some(Location::Server),
        None => None,
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
