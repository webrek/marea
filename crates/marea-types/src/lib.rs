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
mod error;
mod ty;

pub use error::TypeError;
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
    fn new() -> Self {
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

    fn error(&mut self, e: TypeError) {
        self.errors.push(e);
    }

    /// Comprueba el acceso (lectura o escritura) a una global `reactive`. Es
    /// estado de UI: solo se emite en el bundle del cliente. Se prohíbe desde
    /// `@server`/`@edge` —que compilaba a un `ReferenceError` en cada RPC— y
    /// también desde una función SIN anotación, porque el codegen la duplica en
    /// los dos bundles y el del servidor no tiene el estado. Simétrico a
    /// `E_STATE_OFF_SERVER`, que exige lo contrario para el store.
    fn check_reactive_access(&mut self, name: &str, span: Span) {
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

    /// Registra las variables de nivel superior (`let`/`reactive` de módulo) como
    /// globales, tipándolas por su anotación o por su inicializador. Corre tras
    /// `collect` (necesita fns/alias/store ya resueltos) y antes de los cuerpos.
    fn collect_globals(&mut self, module: &Module) {
        for item in &module.items {
            if let Item::Let(l) = item {
                // El inicializador se tipa en un scope vacío (una global solo puede
                // referir funciones, builtins y literales, no variables locales).
                self.scopes = vec![HashMap::new()];
                self.current_location = None;
                // Una `reactive` de módulo cuyo inicializador es una llamada es
                // un RECURSO: ya no dispara el RPC de forma que reviente el
                // arranque —empieza en `Cargando` y un fallo se vuelve `Fallo`—,
                // así que es el sitio natural para los datos de la app.
                let recurso_global = if l.reactive {
                    self.tipo_de_recurso(&l.value)
                } else {
                    None
                };
                if recurso_global.is_none() {
                    self.init_context = Some("el inicializador de una variable de módulo");
                }
                let ty = match &l.ty {
                    Some(t) => {
                        self.validate_type_exists(t);
                        // Se tipa igual el valor, para reportar sus errores.
                        self.check_expr(&l.value);
                        self.ty_from_syntax(t)
                    }
                    None => match &recurso_global {
                        Some(t) => {
                            self.check_expr(&l.value);
                            t.clone()
                        }
                        None => self.check_expr(&l.value),
                    },
                };
                self.init_context = None;
                // Ni como un almacén: ambos son nombres de primer nivel del
                // mismo módulo y el generado los declararía dos veces.
                if self.stores.contains_key(&l.name) {
                    self.error(TypeError::new(
                        "E_DUPLICATE_ITEM",
                        format!("'{}' ya está declarada como almacén", l.name),
                        l.span,
                    ));
                }
                // Una global no puede llamarse como un builtin: el bundle
                // generado importa los builtins del runtime, así que declararla
                // produce un `const` que redeclara el import (SyntaxError, el
                // archivo entero no carga). Misma regla que para las funciones.
                if builtins::lookup(&l.name).is_some() || es_interno_del_runtime(&l.name) {
                    self.error(TypeError::new(
                        "E_REDEFINE_BUILTIN",
                        format!("no se puede redefinir el builtin '{}'", l.name),
                        l.span,
                    ));
                }
                // Tampoco puede chocar con una función del módulo: ambas se
                // emiten como declaraciones de primer nivel en el mismo archivo.
                if self.fns.contains_key(&l.name) {
                    self.error(TypeError::new(
                        "E_DUPLICATE_ITEM",
                        format!("'{}' ya está declarada como función", l.name),
                        l.span,
                    ));
                }
                self.globals.insert(l.name.clone(), (ty, l.mutable));
                if l.reactive {
                    self.reactive_globals.insert(l.name.clone());
                }
            }
        }
        self.scopes = Vec::new();
    }

    // ===================== FASE A: recolección global =====================

    fn collect(&mut self, module: &Module) {
        // Spans de la primera declaración de cada nombre, para notas de duplicado.
        let mut fn_spans: HashMap<String, Span> = HashMap::new();
        let mut type_spans: HashMap<String, Span> = HashMap::new();
        // Tipo sintáctico del `store T;` (se resuelve tras registrar los alias).
        let mut store_decls: Vec<(String, Span, &Type)> = Vec::new();

        for item in &module.items {
            match item {
                Item::Fn(f) => {
                    // No se puede redefinir un builtin (print/concat/render/db/
                    // NotFound): además de confundir, generaría TS inválido por
                    // colisión con el import del runtime.
                    if builtins::lookup(&f.name).is_some() || es_interno_del_runtime(&f.name) {
                        self.error(TypeError::new(
                            "E_REDEFINE_BUILTIN",
                            format!("no se puede redefinir el builtin '{}'", f.name),
                            f.span,
                        ));
                    }
                    if let Some(prev) = fn_spans.get(&f.name) {
                        self.error(
                            TypeError::new(
                                "E_DUPLICATE_ITEM",
                                format!("la función '{}' ya está declarada", f.name),
                                f.span,
                            )
                            .with_note(format!(
                                "primera declaración en el byte {}",
                                prev.start
                            )),
                        );
                    } else {
                        fn_spans.insert(f.name.clone(), f.span);
                    }
                }
                Item::Type(t) => {
                    if let Some(prev) = type_spans.get(&t.name) {
                        self.error(
                            TypeError::new(
                                "E_DUPLICATE_ITEM",
                                format!("el tipo '{}' ya está declarado", t.name),
                                t.span,
                            )
                            .with_note(format!(
                                "primera declaración en el byte {}",
                                prev.start
                            )),
                        );
                    } else {
                        type_spans.insert(t.name.clone(), t.span);
                        self.aliases.insert(t.name.clone(), t.aliased.clone());
                    }
                }
                Item::Let(_) => {}
                Item::Store { name, name_span, ty, span } => {
                    if store_decls.iter().any(|(n, _, _)| n == name) {
                        self.error(TypeError::new(
                            "E_DUPLICATE_STORE",
                            format!("el almacén '{name}' ya fue declarado"),
                            *span,
                        ));
                    } else {
                        store_decls.push((name.clone(), *name_span, ty));
                    }
                }
            }
        }

        // Detección de alias cíclicos (`type A = B; type B = A`) ANTES de
        // registrar firmas: el registro llama a ty_from_syntax, que sin la marca
        // de ciclo recurriría infinitamente (stack overflow).
        self.detect_cyclic_types(&type_spans);

        // Resuelve los tipos de los almacenes ahora que los alias ya están
        // registrados.
        for (nombre, span, ty) in store_decls {
            self.validate_type_exists(ty);
            let elem = self.ty_from_syntax(ty);
            if builtins::lookup(&nombre).is_some() || es_interno_del_runtime(&nombre) {
                self.error(TypeError::new(
                    "E_REDEFINE_BUILTIN",
                    format!("no se puede llamar '{nombre}' a un almacén: es un builtin"),
                    span,
                ));
            }
            // Dos almacenes que solo difieren en mayúsculas acabarían en la
            // MISMA tabla (el nombre se pasa a minúsculas) y en el mismo archivo
            // (los sistemas de archivos de macOS y Windows no distinguen caja),
            // mezclando datos de tipos distintos sin un solo aviso.
            let bajo = nombre.to_lowercase();
            if let Some(previo) = self
                .stores
                .keys()
                .find(|k| k.to_lowercase() == bajo)
                .cloned()
            {
                self.error(TypeError::new(
                    "E_DUPLICATE_STORE",
                    format!(
                        "'{nombre}' y '{previo}' se guardarían en el mismo sitio: los nombres \
                         de almacén no pueden diferir sólo en mayúsculas"
                    ),
                    span,
                ));
            }
            // Los campos `__id` y `__doc` los usa la capa de persistencia: un
            // registro que los declare produce una tabla con la columna repetida
            // y todo `guardar` revienta en runtime.
            if let Ty::Record(campos) = &elem {
                for (campo, _) in campos {
                    if campo == "__id" || campo == "__doc" {
                        self.error(TypeError::new(
                            "E_CAMPO_RESERVADO",
                            format!(
                                "'{campo}' es un nombre de columna reservado por la \
                                 persistencia; renombra el campo del almacén '{nombre}'"
                            ),
                            span,
                        ));
                    }
                }
            }
            if self.fns.contains_key(&nombre) {
                self.error(TypeError::new(
                    "E_DUPLICATE_ITEM",
                    format!("'{nombre}' ya está declarada como función"),
                    span,
                ));
            }
            self.stores.insert(nombre, elem);
        }

        // Registra las firmas (solo la primera de cada nombre).
        for item in &module.items {
            if let Item::Fn(f) = item {
                if self.fns.contains_key(&f.name) {
                    continue;
                }
                let params: Vec<Ty> = f
                    .params
                    .iter()
                    .map(|p| self.ty_from_syntax(&p.ty))
                    .collect();
                let ret = match &f.return_type {
                    Some(t) => self.ty_from_syntax(t),
                    None => Ty::Unit,
                };
                let ret_nombres = match &f.return_type {
                    Some(Type::Name { name, .. }) => vec![name.clone()],
                    Some(Type::Union { variants, .. }) => variants
                        .iter()
                        .filter_map(|v| match v {
                            Type::Name { name, .. } => Some(name.clone()),
                            _ => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                };
                self.fns.insert(
                    f.name.clone(),
                    FnSig {
                        params,
                        ret,
                        location: f.location,
                        ret_nombres,
                    },
                );
            }
        }
    }

    fn detect_cyclic_types(&mut self, type_spans: &HashMap<String, Span>) {
        let names: Vec<String> = self.aliases.keys().cloned().collect();
        for name in &names {
            let mut visiting = std::collections::HashSet::new();
            if self.alias_cycles(name, &mut visiting) {
                // Marca el alias como cíclico para cortar la recursión en Fase B.
                self.cyclic.insert(name.clone());
                if let Some(span) = type_spans.get(name) {
                    self.error(TypeError::new(
                        "E_CYCLIC_TYPE",
                        format!("el alias de tipo '{name}' es cíclico"),
                        *span,
                    ));
                }
            }
        }
    }

    /// ¿Resolver `name` entra en un ciclo de alias? Sólo seguimos referencias
    /// directas a otros alias (`Type::Name`) y dentro de uniones.
    fn alias_cycles(&self, name: &str, visiting: &mut std::collections::HashSet<String>) -> bool {
        if !visiting.insert(name.to_string()) {
            return true;
        }
        let result = if let Some(t) = self.aliases.get(name) {
            self.type_refs_cycle(t, visiting)
        } else {
            false
        };
        visiting.remove(name);
        result
    }

    fn type_refs_cycle(&self, t: &Type, visiting: &mut std::collections::HashSet<String>) -> bool {
        match t {
            Type::Name { name, .. } => {
                if self.aliases.contains_key(name) {
                    self.alias_cycles(name, visiting)
                } else {
                    false
                }
            }
            Type::Union { variants, .. } => {
                variants.iter().any(|v| self.type_refs_cycle(v, visiting))
            }
            // Los registros cortan el ciclo (son estructurales, no transparentes).
            Type::Record { .. } => false,
        }
    }

    // ===================== FASE B: chequeo de cuerpos =====================

    fn check_bodies(&mut self, module: &Module) {
        for item in &module.items {
            match item {
                Item::Fn(f) => self.check_fn(f),
                // Un `let` a nivel módulo se chequea como una declaración laxa.
                Item::Let(l) => {
                    self.scopes.push(HashMap::new());
                    self.current_location = None;
                    self.current_return = Ty::Unit;
                    let value_ty = self.check_expr(&l.value);
                    if let Some(decl) = &l.ty {
                        let declared = self.ty_from_syntax(decl);
                        // Un literal del fuente es de confianza también aquí:
                        // sin esto valía dentro de una función pero no a nivel
                        // de módulo, para la misma línea escrita igual.
                        let value_ty = if matches!(declared, Ty::Html)
                            && matches!(l.value, Expr::Str { .. })
                        {
                            Ty::Html
                        } else {
                            value_ty.clone()
                        };
                        if !self.is_subtype(&value_ty, &declared) {
                            self.error(TypeError::new(
                                "E_LET_TYPE_MISMATCH",
                                format!(
                                    "el valor es '{}' pero se declaró '{}'",
                                    value_ty.display(),
                                    declared.display()
                                ),
                                l.value.span(),
                            ));
                        }
                    }
                    self.scopes.pop();
                }
                Item::Type(_) | Item::Store { .. } => {}
            }
        }
    }

    fn check_fn(&mut self, f: &FnDecl) {
        self.scopes = vec![HashMap::new()];
        self.current_location = f.location;
        self.current_return = match &f.return_type {
            Some(t) => self.ty_from_syntax(t),
            None => Ty::Unit,
        };

        // Parámetros en el scope raíz; detecta parámetros duplicados.
        let mut seen: HashMap<String, ()> = HashMap::new();
        for p in &f.params {
            // Valida que el tipo del parámetro exista.
            self.validate_type_exists(&p.ty);
            if seen.insert(p.name.clone(), ()).is_some() {
                self.error(TypeError::new(
                    "E_DUPLICATE_PARAM",
                    format!("el parámetro '{}' está repetido", p.name),
                    p.span,
                ));
            }
            let pty = self.ty_from_syntax(&p.ty);
            // Los parámetros son inmutables.
            self.scopes
                .last_mut()
                .unwrap()
                .insert(p.name.clone(), (pty, false));
        }

        // Valida el tipo de retorno declarado.
        if let Some(rt) = &f.return_type {
            self.validate_type_exists(rt);
        }

        // Un parámetro `Html` de una función remota lo rellena quien mande el
        // JSON: la confianza no cruza la red. Se comprueba en la declaración y
        // no solo en la llamada, porque el atacante no usa el cliente generado.
        if matches!(f.location, Some(Location::Server) | Some(Location::Edge)) {
            for p in &f.params {
                if matches!(self.ty_from_syntax(&p.ty), Ty::Html) {
                    self.error(TypeError::new(
                        "E_BOUNDARY_NOT_SERIALIZABLE",
                        format!(
                            "el parámetro '{}' es 'Html', pero la confianza no cruza la red: \
                             recíbelo como String y escápalo donde se use",
                            p.name
                        ),
                        p.span,
                    ));
                }
            }
        }

        // `marea build-app` monta en el DOM la función @client llamada `vista`
        // (`__mount(vista)` en el codegen), así que su retorno es un sumidero de
        // innerHTML igual que `render` y tiene que ser marcado seguro. Sin esta
        // regla la garantía de `Html` no cubría la ruta por defecto de build-app,
        // que es justo la que usa la demo desplegada.
        if f.name == "vista"
            && f.location == Some(Location::Client)
            && f.params.is_empty()
            && !matches!(self.current_return, Ty::Html | Ty::Unit | Ty::Unknown)
        {
            self.error(TypeError::new(
                "E_VISTA_NO_HTML",
                format!(
                    "'vista' se monta en el DOM, así que debe devolver 'Html', no '{}'; \
                     escapa los datos con escapar(...) y declara -> Html",
                    self.current_return.display()
                ),
                f.span,
            ));
        }

        // El cuerpo comparte el scope de los parámetros en vez de empujar uno
        // nuevo: así un `let` de primer nivel que redeclare un parámetro se
        // detecta. En JS `function f(x) { let x = 2; }` es un SyntaxError (y en
        // WASM da dos locales con el mismo nombre), así que dejarlo pasar
        // producía código que no carga. El shadowing en bloques ANIDADOS sigue
        // siendo legal, igual que en JS.
        self.check_block_in_current_scope(&f.body);

        // Funciones con retorno no-Unit deben terminar en todo camino.
        if !matches!(self.current_return, Ty::Unit | Ty::Unknown) && !block_terminates(&f.body) {
            self.error(TypeError::new(
                "E_MISSING_RETURN",
                format!(
                    "la función '{}' debe devolver '{}' en todos los caminos",
                    f.name,
                    self.current_return.display()
                ),
                f.span,
            ));
        }
    }

    fn check_block(&mut self, block: &Block) {
        self.scopes.push(HashMap::new());
        self.check_block_in_current_scope(block);
        self.scopes.pop();
    }

    /// Chequea las sentencias sin abrir un scope propio (el llamador ya abrió
    /// el suyo). Lo usa el cuerpo de una función para compartir el scope con
    /// sus parámetros.
    fn check_block_in_current_scope(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(l) => {
                // `reactive x = f(...)` es un RECURSO: la llamada es asíncrona,
                // así que el valor empieza en `Cargando`, pasa al resultado
                // cuando llega y a `Fallo` si la llamada revienta. Es la
                // composición de las dos fronteras —la de red y la del tiempo—
                // y el tipo lo dice: `Cargando | T | Fallo`, que obliga a un
                // match con los tres casos.
                let prev_init = self.init_context;
                let recurso = if l.reactive {
                    self.tipo_de_recurso(&l.value)
                } else {
                    None
                };
                if l.reactive && recurso.is_none() {
                    self.init_context = Some("el inicializador de una variable 'reactive'");
                }
                let value_ty = match &recurso {
                    Some(t) => {
                        // Se tipan los argumentos igual, para no perder errores.
                        self.check_expr(&l.value);
                        t.clone()
                    }
                    None => self.check_expr(&l.value),
                };
                self.init_context = prev_init;
                // Tipo destino, si se anotó.
                let bind_ty = if let Some(decl) = &l.ty {
                    self.validate_type_exists(decl);
                    let declared = self.ty_from_syntax(decl);
                    // Un literal del fuente vale como Html (confianza directa).
                    let value_ty = if matches!(declared, Ty::Html)
                        && matches!(l.value, Expr::Str { .. })
                    {
                        Ty::Html
                    } else {
                        value_ty.clone()
                    };
                    // `reactive` es laxo con la inferencia, pero no puede serlo
                    // con `Html`: saltarse el subtipado ahí permitía
                    // `reactive h: Html = s;` con un String cualquiera.
                    let laxo = l.reactive && !matches!(declared, Ty::Html);
                    if !laxo && !self.is_subtype(&value_ty, &declared) {
                        self.error(TypeError::new(
                            "E_LET_TYPE_MISMATCH",
                            format!(
                                "el valor es '{}' pero se declaró '{}'",
                                value_ty.display(),
                                declared.display()
                            ),
                            l.value.span(),
                        ));
                    }
                    declared
                } else {
                    value_ty
                };

                // Re-declaración en el MISMO scope = error; shadowing interno OK.
                if self.scopes.last().unwrap().contains_key(&l.name) {
                    self.error(TypeError::new(
                        "E_DUPLICATE_BINDING",
                        format!("la variable '{}' ya fue declarada en este ámbito", l.name),
                        l.span,
                    ));
                }
                // `mut` o `reactive` => reasignable.
                self.scopes
                    .last_mut()
                    .unwrap()
                    // Solo `mut` hace reasignable, igual que en las globales. Una
                    // `reactive` derivada es de solo lectura: el navegador lanza
                    // al asignarle y Node lo ignoraba en silencio, así que
                    // dejarlo pasar daba dos comportamientos para el mismo .mar.
                    .insert(l.name.clone(), (bind_ty, l.mutable));
            }
            Stmt::Return { value, span } => {
                let expected = self.current_return.clone();
                // Un literal del fuente es de confianza: vale como Html.
                let ret_ty = match value {
                    Some(e) if matches!(expected, Ty::Html) => self.check_expr_html(e),
                    Some(e) => self.check_expr(e),
                    None => Ty::Unit,
                };
                if !self.is_subtype(&ret_ty, &expected) {
                    let s = value.as_ref().map(|e| e.span()).unwrap_or(*span);
                    self.error(TypeError::new(
                        "E_RETURN_TYPE_MISMATCH",
                        format!(
                            "se devuelve '{}' pero la función declara '{}'",
                            ret_ty.display(),
                            expected.display()
                        ),
                        s,
                    ));
                }
            }
            Stmt::Assign { name, name_span, value, .. } => {
                let value_ty = self.check_expr(value);
                let mut target = None;
                for scope in self.scopes.iter().rev() {
                    if let Some(t) = scope.get(name) {
                        target = Some(t.clone());
                        break;
                    }
                }
                // Si no es una variable local, puede ser una global de módulo.
                if target.is_none() {
                    target = self.globals.get(name).cloned();
                    // Escribir una reactiva tiene la misma restricción de
                    // ubicación que leerla: antes solo se comprobaba la lectura,
                    // así que un @server podía asignarle y romper en runtime.
                    if target.is_some() {
                        self.check_reactive_access(name, *name_span);
                    }
                }
                match target {
                    None => self.error(TypeError::new(
                        "E_UNRESOLVED_NAME",
                        format!("'{name}' no está definido"),
                        *name_span,
                    )),
                    Some((var_ty, mutable)) => {
                        if !mutable {
                            self.error(TypeError::new(
                                "E_ASSIGN_IMMUTABLE",
                                format!(
                                    "no se puede reasignar '{name}': es inmutable (usa 'let mut' o 'reactive')"
                                ),
                                *name_span,
                            ));
                        }
                        if !self.is_subtype(&value_ty, &var_ty) {
                            self.error(TypeError::new(
                                "E_ASSIGN_TYPE_MISMATCH",
                                format!(
                                    "se asigna '{}' a una variable de tipo '{}'",
                                    value_ty.display(),
                                    var_ty.display()
                                ),
                                value.span(),
                            ));
                        }
                    }
                }
            }
            Stmt::Effect { body, .. } => {
                self.check_block(body);
            }
            Stmt::Expr(e) => {
                self.check_expr(e);
            }
        }
    }

    // ----------------------- chequeo de expresiones -----------------------

    fn check_expr(&mut self, expr: &Expr) -> Ty {
        match expr {
            Expr::Int { .. } => Ty::Int,
            Expr::Float { .. } => Ty::Float,
            Expr::Str { .. } => Ty::String,
            Expr::Bool { .. } => Ty::Bool,
            Expr::Ident { name, span } => self.resolve_ident(name, *span),
            Expr::Unary { op, expr: inner, span } => self.check_unary(*op, inner, *span),
            Expr::Binary { op, left, right, span } => self.check_binary(*op, left, right, *span),
            Expr::Call { callee, args, span } => self.check_call(callee, args, *span),
            Expr::Member { object, field, span } => self.check_member(object, field, *span),
            Expr::If { cond, then_branch, else_branch, .. } => {
                self.check_if(cond, then_branch, else_branch.as_deref())
            }
            Expr::Match { scrutinee, arms, span } => self.check_match(scrutinee, arms, *span),
            Expr::Record { type_name, type_name_span, fields, span } => {
                self.check_record(type_name.as_deref(), *type_name_span, fields, *span)
            }
            Expr::List { elements, span } => self.check_list(elements, *span),
            Expr::Index { object, index, .. } => self.check_index(object, index),
            Expr::Template { parts, .. } => {
                // Una plantilla SIEMPRE es `Html`: los huecos `{x}` se escapan
                // al emitir, y `{!x}` solo admite algo que ya sea Html. Por eso
                // olvidarse del escapado deja de ser posible: no hay forma de
                // meter texto sin escapar por ninguna de las dos puertas.
                for parte in parts {
                    if let marea_syntax::ast::TemplatePart::Interp { expr, raw } = parte {
                        let t = self.check_expr(expr);
                        if *raw && !self.is_subtype(&t, &Ty::Html) {
                            self.error(TypeError::new(
                                "E_INTERP_CRUDA_NO_HTML",
                                format!(
                                    "'{{!...}}' inserta marcado sin escapar, así que sólo admite \
                                     'Html', no '{}'; usa '{{...}}' para que se escape",
                                    t.display()
                                ),
                                expr.span(),
                            ));
                        }
                    }
                }
                Ty::Html
            }
        }
    }

    /// Como `check_expr`, pero tratando los literales de cadena del propio
    /// fuente como `Html`. Un literal lo escribió el programador, no un usuario:
    /// es de confianza por construcción. Gracias a esto el idioma habitual
    /// —`concat("<li>", escapar(x))`— sigue tipando sin conversiones, mientras
    /// que `concat(p.texto, ...)` (dato del store) NO produce Html.
    fn check_expr_html(&mut self, e: &Expr) -> Ty {
        let t = self.check_expr(e);
        if matches!(e, Expr::Str { .. }) && matches!(t, Ty::String) {
            return Ty::Html;
        }
        t
    }

    /// Si `e` es una llamada a una función declarada cuyo retorno es nombrable,
    /// devuelve el tipo del recurso: `Cargando | <retorno> | Fallo`. Devuelve
    /// `None` cuando no se puede construir —una llamada a un builtin, o un
    /// retorno que es un registro escrito en línea y por tanto sin nombre—, y
    /// entonces vuelve a aplicarse la prohibición de cruzar en un inicializador.
    fn tipo_de_recurso(&self, e: &Expr) -> Option<Ty> {
        let Expr::Call { callee, .. } = e else {
            return None;
        };
        let Expr::Ident { name, .. } = callee.as_ref() else {
            return None;
        };
        if es_builtin_sincrono(name) {
            return None;
        }
        let sig = self.fns.get(name)?;
        if sig.ret_nombres.is_empty() {
            return None;
        }
        let mut vs = vec!["Loading".to_string()];
        vs.extend(sig.ret_nombres.iter().cloned());
        vs.push("Failed".to_string());
        Some(Ty::Union(vs))
    }

    fn resolve_ident(&mut self, name: &str, span: Span) -> Ty {
        // Variable en algún scope (del más interno al más externo).
        for scope in self.scopes.iter().rev() {
            if let Some((ty, _)) = scope.get(name) {
                return ty.clone();
            }
        }
        // Nombre de un almacén declarado con `store nombre: T;`. Es un asa del
        // SERVIDOR: solo se declara en ese bundle, así que nombrarla desde otro
        // sitio compilaba a un `ReferenceError`. E_STATE_OFF_SERVER solo cubría
        // las LLAMADAS a los builtins (`todos(cosas)`), no la referencia suelta
        // (`let x = cosas;`), que tipaba limpio y reventaba al cargar.
        if let Some(elem) = self.stores.get(name).cloned() {
            if !matches!(
                self.current_location,
                Some(Location::Server) | Some(Location::Edge)
            ) {
                let donde = match self.current_location {
                    Some(Location::Client) => "una función @client",
                    _ => "una función sin anotación (se emite también en el cliente)",
                };
                self.error(TypeError::new(
                    "E_STATE_OFF_SERVER",
                    format!(
                        "'{name}' es un almacén del servidor y no existe en el cliente; \
                         no puede usarse desde {donde}: envuélvelo en una @server"
                    ),
                    span,
                ));
            }
            return Ty::Store(name.to_string(), Box::new(elem));
        }
        // Variable global (estado reactivo de módulo).
        if let Some(ty) = self.globals.get(name).map(|(t, _)| t.clone()) {
            // Una global `reactive` es estado de UI: solo existe en el bundle
            // del cliente. Leerla desde @server compilaba a un `ReferenceError`
            // en cada RPC, así que se rechaza aquí con un mensaje accionable.
            self.check_reactive_access(name, span);
            return ty;
        }
        // Función declarada.
        if let Some(sig) = self.fns.get(name) {
            return Ty::Fn {
                params: sig.params.clone(),
                ret: Box::new(sig.ret.clone()),
                location: sig.location,
            };
        }
        // Builtin.
        if let Some(ty) = builtins::lookup(name) {
            return ty;
        }
        // Variante nominal usada como valor ("errores como valores"): por
        // convención, un identificador con inicial Mayúscula que no es variable,
        // función ni builtin es una etiqueta de variante (p.ej. `NotFound`). Su
        // tipo es Named(name), que es subtipo de cualquier unión que la contenga.
        if name.chars().next().is_some_and(|c| c.is_uppercase()) {
            return Ty::Named(name.to_string());
        }
        self.error(TypeError::new(
            "E_UNRESOLVED_NAME",
            format!("'{name}' no está definido"),
            span,
        ));
        Ty::Unknown
    }

    fn check_unary(&mut self, op: UnaryOp, inner: &Expr, span: Span) -> Ty {
        let t = self.check_expr(inner);
        match op {
            UnaryOp::Neg => {
                if matches!(t, Ty::Int | Ty::Float | Ty::Unknown) {
                    t
                } else {
                    self.error(TypeError::new(
                        "E_ARITH_TYPE",
                        format!("no se puede negar un valor '{}'", t.display()),
                        span,
                    ));
                    Ty::Unknown
                }
            }
            UnaryOp::Not => {
                if !matches!(t, Ty::Bool | Ty::Unknown) {
                    self.error(TypeError::new(
                        "E_COND_NOT_BOOL",
                        format!("'!' espera un Bool, no '{}'", t.display()),
                        span,
                    ));
                }
                Ty::Bool
            }
        }
    }

    fn check_binary(&mut self, op: BinOp, left: &Expr, right: &Expr, span: Span) -> Ty {
        let lt = self.check_expr(left);
        let rt = self.check_expr(right);
        match op {
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => {
                if matches!(lt, Ty::Unknown) || matches!(rt, Ty::Unknown) {
                    return if matches!(lt, Ty::Unknown) { rt } else { lt };
                }
                let both_int = lt == Ty::Int && rt == Ty::Int;
                let both_float = lt == Ty::Float && rt == Ty::Float;
                if both_int {
                    Ty::Int
                } else if both_float {
                    Ty::Float
                } else {
                    self.error(TypeError::new(
                        "E_ARITH_TYPE",
                        format!(
                            "los operandos aritméticos deben ser ambos Int o ambos Float, no '{}' y '{}'",
                            lt.display(),
                            rt.display()
                        ),
                        span,
                    ));
                    Ty::Unknown
                }
            }
            BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                if matches!(lt, Ty::Unknown) || matches!(rt, Ty::Unknown) {
                    return Ty::Bool;
                }
                let ok = (lt == Ty::Int && rt == Ty::Int) || (lt == Ty::Float && rt == Ty::Float);
                if !ok {
                    self.error(TypeError::new(
                        "E_ARITH_TYPE",
                        format!(
                            "la comparación de orden requiere numéricos del mismo tipo, no '{}' y '{}'",
                            lt.display(),
                            rt.display()
                        ),
                        span,
                    ));
                }
                Ty::Bool
            }
            BinOp::Eq | BinOp::Ne => {
                if matches!(lt, Ty::Unknown) || matches!(rt, Ty::Unknown) {
                    return Ty::Bool;
                }
                // Igualdad entre escalares del mismo tipo, admitiendo que uno
                // sea subtipo del otro: `Html` es texto, así que compararlo con
                // un `String` es legítimo (y comparar dos literales del fuente,
                // que tipan como Html, con una variable String, es lo normal).
                let compatibles =
                    lt.is_scalar() && rt.is_scalar() && (self.is_subtype(&lt, &rt) || self.is_subtype(&rt, &lt));
                if !compatibles {
                    self.error(TypeError::new(
                        "E_ARITH_TYPE",
                        format!(
                            "la igualdad requiere el mismo tipo escalar, no '{}' y '{}'",
                            lt.display(),
                            rt.display()
                        ),
                        span,
                    ));
                }
                Ty::Bool
            }
            BinOp::And | BinOp::Or => {
                if !matches!(lt, Ty::Bool | Ty::Unknown) || !matches!(rt, Ty::Bool | Ty::Unknown) {
                    self.error(TypeError::new(
                        "E_COND_NOT_BOOL",
                        format!(
                            "los operadores lógicos esperan Bool, no '{}' y '{}'",
                            lt.display(),
                            rt.display()
                        ),
                        span,
                    ));
                }
                Ty::Bool
            }
        }
    }

    /// Chequea `guardar(x)` y `todos()` contra el store tipado (`store T;`):
    /// sólo en @server/@edge, requieren la declaración, y `guardar` exige que su
    /// argumento sea del tipo del store; `todos()` devuelve `List<T>`.
    fn check_state_builtin(&mut self, name: &str, args: &[Expr], span: Span) -> Ty {
        if !matches!(
            self.current_location,
            Some(Location::Server) | Some(Location::Edge)
        ) {
            self.error(TypeError::new(
                "E_STATE_OFF_SERVER",
                format!(
                    "'{name}' (estado del servidor) sólo puede usarse en una función @server; \
                     envuélvelo en una y llámala por RPC"
                ),
                span,
            ));
        }
        let arg_tys: Vec<Ty> = args.iter().map(|a| self.check_expr(a)).collect();

        // El PRIMER argumento es el almacén: `todos(productos)`. De ahí sale el
        // tipo de los elementos, así que un módulo puede tener varios almacenes
        // sin ambigüedad.
        let elem = match arg_tys.first() {
            Some(Ty::Store(_, e)) => (**e).clone(),
            Some(Ty::Unknown) | None => Ty::Unknown,
            Some(otro) => {
                self.error(TypeError::new(
                    "E_NO_STORE",
                    format!(
                        "el primer argumento de '{name}' debe ser un almacén declarado con \
                         'store nombre: T;', no '{}'",
                        otro.display()
                    ),
                    args.first().map(|a| a.span()).unwrap_or(span),
                ));
                Ty::Unknown
            }
        };

        // Firma de cada builtin de estado, contando el almacén: (aridad,
        // posiciones de índice Int, posiciones de valor, ¿devuelve List<T>?).
        let (expected, idx_args, elem_args, returns_list): (usize, &[usize], &[usize], bool) =
            match name {
                "all" => (1, &[], &[], true),
                "save" => (2, &[], &[1], false),
                "remove" => (2, &[1], &[], false),
                "update" => (3, &[1], &[2], false),
                _ => (0, &[], &[], false),
            };

        let ret = if returns_list {
            Ty::List(Box::new(elem.clone()))
        } else {
            Ty::Unit
        };

        if !self.arity(name, &arg_tys, expected, span) {
            return ret;
        }
        // Los índices deben ser Int.
        for &i in idx_args {
            if !matches!(arg_tys[i], Ty::Int | Ty::Unknown) {
                self.error(TypeError::new(
                    "E_ARG_TYPE",
                    format!("el índice debe ser Int, no '{}'", arg_tys[i].display()),
                    args[i].span(),
                ));
            }
        }
        // Los valores deben ser del tipo del store.
        for &i in elem_args {
            if !self.is_subtype(&arg_tys[i], &elem) {
                self.error(TypeError::new(
                    "E_ARG_TYPE",
                    format!(
                        "el valor es '{}' pero el store es de '{}'",
                        arg_tys[i].display(),
                        elem.display()
                    ),
                    args[i].span(),
                ));
            }
        }
        ret
    }

    /// Chequea la aridad exacta de un builtin; devuelve `true` si coincide.
    fn arity(&mut self, name: &str, args: &[Ty], expected: usize, span: Span) -> bool {
        if args.len() != expected {
            self.error(TypeError::new(
                "E_ARITY",
                format!("'{name}' espera {expected} argumento(s), se recibieron {}", args.len()),
                span,
            ));
            false
        } else {
            true
        }
    }

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Ty {
        // Dentro de un inicializador que se evalúa de forma síncrona (el memo de
        // una `reactive`, o una global de módulo) no puede haber ninguna llamada
        // que el codegen emita con `await`: eso es todo salvo un puñado de
        // builtins síncronos. El resultado era `__memo(() => (await f()))`, un
        // await en una arrow no-async, o sea un SyntaxError que impide cargar el
        // módulo entero. No basta con mirar los cruces de red: cualquier función
        // del usuario se compila a `async`.
        if let Some(ctx) = self.init_context {
            let nombre = callee_name(callee);
            if !es_builtin_sincrono(&nombre) {
                self.error(TypeError::new(
                    "E_BOUNDARY_IN_INIT",
                    format!(
                        "'{nombre}' se compila a una llamada asíncrona y no puede usarse en \
                         {ctx}; llámala dentro de una función y asigna el resultado"
                    ),
                    span,
                ));
            }
        }

        // Los builtins de ESTADO (`guardar`/`todos`) se chequean aparte: contra
        // el tipo del store y sólo dentro de @server/@edge.
        if let Expr::Ident { name, .. } = callee {
            if matches!(name.as_str(), "save" | "all" | "update" | "remove") {
                return self.check_state_builtin(name, args, span);
            }
            // La red saliente vive en el servidor, igual que el estado: desde el
            // navegador la llamada la haría el cliente (otro origen, otras
            // credenciales, y CORS decidiendo por ti), que no es lo que el
            // programa dice. Además la lista blanca de destinos es del servidor.
            if matches!(name.as_str(), "fetch" | "post") {
                if !matches!(
                    self.current_location,
                    Some(Location::Server) | Some(Location::Edge)
                ) {
                    self.error(TypeError::new(
                        "E_RED_OFF_SERVER",
                        format!(
                            "'{name}' sale a la red y sólo puede usarse en una función \
                             @server; envuélvelo en una y llámala por RPC"
                        ),
                        span,
                    ));
                }
            }
            if name == "text" {
                // aTexto sólo tiene sentido sobre escalares; un Record/List daría
                // '[object Object]'/'1,2,3' (basura mostrada al usuario).
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.check_expr(a)).collect();
                if self.arity("text", &arg_tys, 1, span) {
                    let t = &arg_tys[0];
                    if !t.is_scalar() && !matches!(t, Ty::Unknown) {
                        self.error(TypeError::new(
                            "E_ARG_TYPE",
                            format!(
                                "aTexto espera un valor escalar (Int/Float/Bool/String), no '{}'",
                                t.display()
                            ),
                            args[0].span(),
                        ));
                    }
                }
                // Un Int/Float/Bool no puede contener marcado, así que su
                // texto es seguro para el DOM: se tipa como Html y el idioma
                // `concat(html, aTexto(n))` sigue funcionando sin conversiones.
                // Un String sí puede traer marcado: sigue siendo String.
                return match arg_tys.first() {
                    Some(Ty::Int) | Some(Ty::Float) | Some(Ty::Bool) => Ty::Html,
                    _ => Ty::String,
                };
            }
            // `concat` sirve para texto Y para listas: son la misma idea sobre
            // dos estructuras. Sobre listas conserva el tipo del elemento —el
            // verificador no tiene genéricos, así que la firma se calcula aquí
            // desde los argumentos— y unir listas incompatibles es un error.
            if name == "concat" && args.len() == 2 && matches!(
                self.check_expr(&args[0]), Ty::List(_)
            ) {
                let ts: Vec<Ty> = args.iter().map(|a| self.check_expr(a)).collect();
                let mut elems = Vec::new();
                for (i, t) in ts.iter().enumerate() {
                    match t {
                        Ty::List(e) => elems.push((**e).clone()),
                        Ty::Unknown => elems.push(Ty::Unknown),
                        otro => {
                            self.error(TypeError::new(
                                "E_ARG_TYPE",
                                format!("concat espera listas o texto, no '{}'", otro.display()),
                                args[i].span(),
                            ));
                            elems.push(Ty::Unknown);
                        }
                    }
                }
                // El resultado toma el elemento más concreto de los dos; si son
                // incompatibles es un error (una lista es homogénea).
                let (a, b) = (&elems[0], &elems[1]);
                if matches!(a, Ty::Unknown) {
                    return Ty::List(Box::new(b.clone()));
                }
                if matches!(b, Ty::Unknown) || a == b {
                    return Ty::List(Box::new(a.clone()));
                }
                self.error(TypeError::new(
                    "E_LIST_HETEROGENEOUS",
                    format!(
                        "no se pueden concatenar 'List<{}>' y 'List<{}>'",
                        a.display(),
                        b.display()
                    ),
                    span,
                ));
                return Ty::List(Box::new(Ty::Unknown));
            }
            // `agregar(xs, x)`: el elemento debe encajar con el de la lista.
            if name == "append" {
                let ts: Vec<Ty> = args.iter().map(|a| self.check_expr(a)).collect();
                if !self.arity("append", &ts, 2, span) {
                    return Ty::List(Box::new(Ty::Unknown));
                }
                let elem = match &ts[0] {
                    Ty::List(e) => (**e).clone(),
                    Ty::Unknown => Ty::Unknown,
                    otro => {
                        self.error(TypeError::new(
                            "E_ARG_TYPE",
                            format!("append espera una lista, no '{}'", otro.display()),
                            args[0].span(),
                        ));
                        Ty::Unknown
                    }
                };
                if matches!(elem, Ty::Unknown) {
                    return Ty::List(Box::new(ts[1].clone()));
                }
                if !self.is_subtype(&ts[1], &elem) {
                    self.error(TypeError::new(
                        "E_LIST_HETEROGENEOUS",
                        format!(
                            "se añade '{}' a una 'List<{}>'",
                            ts[1].display(),
                            elem.display()
                        ),
                        args[1].span(),
                    ));
                }
                return Ty::List(Box::new(elem));
            }
            // `concat` propaga la seguridad: si los dos lados son marcado
            // seguro, el resultado lo es. Con un solo lado inseguro cae a String
            // y entonces no puede llegar a `render`, que es el objetivo.
            if name == "concat" {
                let arg_tys: Vec<Ty> = args.iter().map(|a| self.check_expr_html(a)).collect();
                if self.arity("concat", &arg_tys, 2, span) {
                    for (i, t) in arg_tys.iter().enumerate() {
                        if !matches!(t, Ty::String | Ty::Html | Ty::Unknown) {
                            self.error(TypeError::new(
                                "E_ARG_TYPE",
                                format!("concat espera String, no '{}'", t.display()),
                                args[i].span(),
                            ));
                        }
                    }
                    if arg_tys.iter().all(|t| matches!(t, Ty::Html)) {
                        return Ty::Html;
                    }
                }
                return Ty::String;
            }
        }

        // Llamadas sobre miembros de un objeto abierto (`db.users.find`) son
        // Unknown: chequeamos los argumentos pero no la firma.
        let callee_ty = self.check_expr(callee);

        let arg_tys: Vec<Ty> = args.iter().map(|a| self.check_expr(a)).collect();

        match &callee_ty {
            Ty::Unknown => Ty::Unknown,
            Ty::Fn { params, ret, location } => {
                // Clasifica el cruce de frontera y valida serializabilidad.
                self.classify_boundary(callee, *location, params, ret, span);

                if params.len() != arg_tys.len() {
                    self.error(TypeError::new(
                        "E_ARITY",
                        format!(
                            "se esperaban {} argumentos, se recibieron {}",
                            params.len(),
                            arg_tys.len()
                        ),
                        span,
                    ));
                } else {
                    for (i, (pty, aty)) in params.iter().zip(arg_tys.iter()).enumerate() {
                        // Un literal de cadena del propio fuente es de confianza:
                        // vale donde se espera marcado seguro (`render("...")`).
                        let literal_confiable = matches!(pty, Ty::Html)
                            && matches!(aty, Ty::String)
                            && matches!(args[i], Expr::Str { .. });
                        if literal_confiable {
                            continue;
                        }
                        if !self.is_subtype(aty, pty) {
                            self.error(TypeError::new(
                                "E_ARG_TYPE",
                                format!(
                                    "el argumento {} es '{}' pero se esperaba '{}'",
                                    i + 1,
                                    aty.display(),
                                    pty.display()
                                ),
                                args[i].span(),
                            ));
                        }
                    }
                }
                (**ret).clone()
            }
            _ => {
                self.error(TypeError::new(
                    "E_NOT_CALLABLE",
                    format!("'{}' no es una función", callee_ty.display()),
                    callee.span(),
                ));
                Ty::Unknown
            }
        }
    }

    /// Clasifica una llamada respecto a la frontera de red y valida reglas.
    fn classify_boundary(
        &mut self,
        callee: &Expr,
        callee_loc: Option<Location>,
        params: &[Ty],
        ret: &Ty,
        span: Span,
    ) {
        let from = self.current_location;
        let to = callee_loc;
        let callee_name = callee_name(callee);

        // Sin ubicación destino o misma ubicación: llamada local, sin frontera.
        if to.is_none() || to == from {
            return;
        }

        // @server llamando @client: prohibido.
        // Ni @server ni @edge pueden empujar ejecución al navegador (@client).
        if matches!(from, Some(Location::Server) | Some(Location::Edge))
            && to == Some(Location::Client)
        {
            let lado = if from == Some(Location::Edge) { "@edge" } else { "@server" };
            self.error(TypeError::new(
                "E_CALL_CLIENT_FROM_SERVER",
                format!("una función {lado} no puede llamar a '{callee_name}' (@client)"),
                span,
            ));
            return;
        }

        // @client/None/@edge → @server/@edge: cruce válido. Registra y exige
        // que argumentos y retorno sean serializables.
        let is_valid_target = matches!(to, Some(Location::Server) | Some(Location::Edge));
        let is_valid_source = matches!(
            from,
            None | Some(Location::Client) | Some(Location::Edge)
        );
        if is_valid_target && is_valid_source {
            // Un cruce de red es asíncrono. Dentro de un inicializador que se
            // evalúa de forma síncrona no hay dónde esperarlo: el memo de una
            // `reactive` se compilaba a `__memo(() => (await f()))` —un await en
            // una arrow no-async, es decir un SyntaxError que impide cargar el
            // módulo— y una global de módulo disparaba el RPC al importar,
            // antes de que el servidor existiera. Mejor un error claro aquí.
            if let Some(ctx) = self.init_context {
                self.error(TypeError::new(
                    "E_BOUNDARY_IN_INIT",
                    format!(
                        "'{callee_name}' cruza la frontera de red y no puede llamarse en \
                         {ctx}; llámala dentro de una función @client y asigna el resultado"
                    ),
                    span,
                ));
            }
            self.crossings.push(BoundaryCrossing {
                callee: callee_name.clone(),
                from,
                to,
                span,
            });
            for (i, pty) in params.iter().enumerate() {
                // `Html` codifica confianza, y la confianza no se serializa: al
                // otro lado del cable la reconstruye quien envíe el JSON. Un
                // parámetro remoto debe recibir `String` y escaparlo.
                if matches!(pty, Ty::Html) {
                    self.error(TypeError::new(
                        "E_BOUNDARY_NOT_SERIALIZABLE",
                        format!(
                            "el parámetro {} de '{callee_name}' es 'Html'; la confianza no \
                             cruza la red: recíbelo como String y escápalo en el servidor",
                            i + 1
                        ),
                        span,
                    ));
                    continue;
                }
                if !is_serializable(pty) {
                    self.error(TypeError::new(
                        "E_BOUNDARY_NOT_SERIALIZABLE",
                        format!(
                            "el parámetro {} de '{callee_name}' es '{}' y no es serializable a través de la frontera",
                            i + 1,
                            pty.display()
                        ),
                        span,
                    ));
                }
            }
            if !is_serializable(ret) {
                self.error(TypeError::new(
                    "E_BOUNDARY_NOT_SERIALIZABLE",
                    format!(
                        "el retorno '{}' de '{callee_name}' no es serializable a través de la frontera",
                        ret.display()
                    ),
                    span,
                ));
            }
        }
    }

    fn check_member(&mut self, object: &Expr, field: &str, span: Span) -> Ty {
        let obj_ty = self.check_expr(object);
        match &obj_ty {
            // Objeto/tipo abierto: cualquier campo es Unknown, sin error.
            Ty::Unknown => Ty::Unknown,
            Ty::Record(fields) => {
                if let Some((_, fty)) = fields.iter().find(|(n, _)| n == field) {
                    fty.clone()
                } else {
                    self.error(TypeError::new(
                        "E_NO_FIELD",
                        format!("el registro no tiene el campo '{field}'"),
                        span,
                    ));
                    Ty::Unknown
                }
            }
            // Un nombre de tipo (alias): resuélvelo a su registro y busca el campo.
            Ty::Named(name) => match self.resolve_named_to_record(name) {
                // Registro abierto (`Record`): cualquier campo es Unknown.
                Some(None) => Ty::Unknown,
                Some(Some(fields)) => {
                    if let Some((_, fty)) = fields.iter().find(|(n, _)| n == field) {
                        fty.clone()
                    } else {
                        self.error(TypeError::new(
                            "E_NO_FIELD",
                            format!("el tipo '{name}' no tiene el campo '{field}'"),
                            span,
                        ));
                        Ty::Unknown
                    }
                }
                // Variante nominal sin declaración de registro: campo abierto.
                None => Ty::Unknown,
            },
            // Acceso a campo sobre una unión opaca: prohibido sin match.
            Ty::Union(_) => {
                self.error(TypeError::new(
                    "E_FIELD_ON_UNION",
                    format!(
                        "no se puede acceder al campo '{field}' de un valor de tipo unión '{}'; usa 'match' para distinguir la variante",
                        obj_ty.display()
                    ),
                    span,
                ));
                Ty::Unknown
            }
            // Escalares y demás: no tienen campos.
            _ => {
                self.error(TypeError::new(
                    "E_NO_FIELD",
                    format!(
                        "un valor de tipo '{}' no tiene el campo '{field}'",
                        obj_ty.display()
                    ),
                    span,
                ));
                Ty::Unknown
            }
        }
    }

    fn check_if(
        &mut self,
        cond: &Expr,
        then_branch: &Block,
        else_branch: Option<&ElseBranch>,
    ) -> Ty {
        let cond_ty = self.check_expr(cond);
        if !matches!(cond_ty, Ty::Bool | Ty::Unknown) {
            self.error(TypeError::new(
                "E_COND_NOT_BOOL",
                format!("la condición del 'if' debe ser Bool, no '{}'", cond_ty.display()),
                cond.span(),
            ));
        }
        self.check_block(then_branch);
        if let Some(eb) = else_branch {
            match eb {
                ElseBranch::Block(b) => self.check_block(b),
                ElseBranch::If(e) => {
                    self.check_expr(e);
                }
            }
        }
        // El `if` como expresión no se usa por valor en Marea v1.5.
        Ty::Unit
    }

    fn check_match(&mut self, scrutinee: &Expr, arms: &[marea_syntax::ast::MatchArm], span: Span) -> Ty {
        let scrut_ty = self.check_expr(scrutinee);
        // Nombre de la variable escrutada, para el narrowing nominal.
        let scrut_name = if let Expr::Ident { name, .. } = scrutinee {
            Some(name.clone())
        } else {
            None
        };

        let variants: Vec<String> = match &scrut_ty {
            Ty::Union(vs) => vs.clone(),
            _ => Vec::new(),
        };

        let mut covered: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut has_catch_all = false;
        // Tipos de los cuerpos de cada rama, para inferir el tipo del 'match'
        // cuando se usa en posición de expresión (`let x = match ...`).
        let mut arm_types: Vec<Ty> = Vec::new();

        for arm in arms {
            // Tras una rama atrapa-todo el resto nunca se ejecuta, y el codegen
            // las descarta al emitir (encadenarlas daría `else if` después de un
            // `else`). Avisar es mejor que borrar código en silencio.
            if has_catch_all {
                self.error(TypeError::new(
                    "E_UNREACHABLE_ARM",
                    "esta rama nunca se ejecuta: una rama anterior ya cubre todos los casos",
                    arm.pattern.span(),
                ));
            }
            match &arm.pattern {
                Pattern::Binding { name, span: pspan } => {
                    let first = name.chars().next();
                    let is_variant = first.map(|c| c.is_uppercase()).unwrap_or(false);
                    if is_variant {
                        // Variante nominal: debe pertenecer a la unión (si la hay).
                        if !variants.is_empty() && !variants.contains(name) {
                            self.error(TypeError::new(
                                "E_UNKNOWN_VARIANT",
                                format!(
                                    "la variante '{name}' no pertenece a la unión '{}'",
                                    scrut_ty.display()
                                ),
                                *pspan,
                            ));
                        }
                        // Una variante que resuelve a un REGISTRO no se puede
                    // discriminar en runtime: los registros no llevan etiqueta,
                    // así que la rama quedaría muerta en silencio (el `match`
                    // no ejecutaría ninguna). Mejor decirlo al compilar.
                    if let Some(Some(_)) = self.resolve_named_to_record(name) {
                        self.error(TypeError::new(
                            "E_VARIANTE_SIN_ETIQUETA",
                            format!(
                                "'{name}' es un registro y no lleva etiqueta en runtime, así que \
                                 esta rama nunca se ejecutaría; usa un comodín (`_` o un nombre) \
                                 para el caso del registro"
                            ),
                            *pspan,
                        ));
                    }
                    covered.insert(name.clone());
                        // Narrowing: dentro de la rama, la variable escrutada es
                        // esta variante (estrechada a Named).
                        self.scopes.push(HashMap::new());
                        if let Some(sn) = &scrut_name {
                            let narrowed = self.narrow_variant(name);
                            self.scopes.last_mut().unwrap().insert(sn.clone(), (narrowed, false));
                        }
                        arm_types.push(self.check_expr_html(&arm.body));
                        self.scopes.pop();
                    } else {
                        // minúscula = binding catch-all. Captura el residual
                        // de la unión (las variantes aún no cubiertas).
                        has_catch_all = true;
                        let residual = self.residual_narrow(&variants, &covered, &scrut_ty);
                        self.scopes.push(HashMap::new());
                        self.scopes.last_mut().unwrap().insert(name.clone(), (residual, false));
                        arm_types.push(self.check_expr_html(&arm.body));
                        self.scopes.pop();
                    }
                }
                Pattern::Wildcard { .. } => {
                    // El comodín estrecha la variable escrutada al residual.
                    has_catch_all = true;
                    let residual = self.residual_narrow(&variants, &covered, &scrut_ty);
                    self.scopes.push(HashMap::new());
                    if let Some(sn) = &scrut_name {
                        self.scopes.last_mut().unwrap().insert(sn.clone(), (residual, false));
                    }
                    arm_types.push(self.check_expr_html(&arm.body));
                    self.scopes.pop();
                }
                Pattern::Int { .. } | Pattern::Bool { .. } | Pattern::Str { .. } => {
                    // Patrón literal: chequea la rama; no aporta a la exhaustividad nominal.
                    arm_types.push(self.check_expr_html(&arm.body));
                }
            }
        }

        // Exhaustividad sobre uniones nominales.
        if !variants.is_empty() && !has_catch_all {
            let missing: Vec<String> = variants
                .iter()
                .filter(|v| !covered.contains(*v))
                .cloned()
                .collect();
            if !missing.is_empty() {
                self.error(TypeError::new(
                    "E_NON_EXHAUSTIVE_MATCH",
                    format!(
                        "el 'match' no cubre todas las variantes; faltan: {}",
                        missing.join(", ")
                    ),
                    span,
                ));
            }
        }

        // Tipo del 'match': el común de las ramas (ignorando Unknown). Si todas
        // coinciden, ese tipo; si difieren, Unknown (no forzamos un error aquí);
        // sin ramas, Unit.
        // Unificación: si los tipos difieren pero uno es supertipo del otro, ese
        // es el del match. Antes cualquier diferencia daba Unknown, y entonces
        // un match cuyas ramas mezclaran `Html` y `String` —lo normal al pintar—
        // no se podía devolver desde una función tipada.
        arm_types
            .into_iter()
            .reduce(|acc, t| {
                if matches!(acc, Ty::Unknown) {
                    t
                } else if matches!(t, Ty::Unknown) || acc == t {
                    acc
                } else if self.is_subtype(&t, &acc) {
                    acc
                } else if self.is_subtype(&acc, &t) {
                    t
                } else {
                    Ty::Unknown
                }
            })
            .unwrap_or(Ty::Unit)
    }

    /// Estrecha el escrutinio en una rama catch-all al residual de la unión:
    /// las variantes que aún no se cubrieron. Si queda exactamente una, se
    /// estrecha a ella (permitiendo acceso a sus campos); si quedan varias,
    /// sigue siendo una unión opaca; si no era una unión, conserva su tipo.
    fn residual_narrow(
        &self,
        variants: &[String],
        covered: &std::collections::HashSet<String>,
        scrut_ty: &Ty,
    ) -> Ty {
        if variants.is_empty() {
            return scrut_ty.clone();
        }
        let residual: Vec<String> =
            variants.iter().filter(|v| !covered.contains(*v)).cloned().collect();
        match residual.len() {
            0 => scrut_ty.clone(),
            1 => self.narrow_variant(&residual[0]),
            _ => Ty::Union(residual),
        }
    }

    /// Estrecha una variante nominal a un tipo concreto para el narrowing.
    /// Si la variante es un alias a un registro, devuelve el registro; si no,
    /// `Named` (que es opaco salvo si resuelve a Record vía acceso a campo).
    fn narrow_variant(&self, name: &str) -> Ty {
        if let Some(alias) = self.aliases.get(name).cloned() {
            self.ty_from_syntax(&alias)
        } else if builtins::type_lookup(name).is_some() {
            builtins::type_lookup(name).unwrap()
        } else {
            // Variante nominal sin declaración (NotFound, Error): registro abierto
            // para no romper accesos a campo dentro de la rama.
            Ty::Named(name.to_string())
        }
    }

    fn check_record(
        &mut self,
        type_name: Option<&str>,
        type_name_span: Option<Span>,
        fields: &[marea_syntax::ast::FieldInit],
        span: Span,
    ) -> Ty {
        let name = match type_name {
            Some(n) => n,
            None => {
                self.error(TypeError::new(
                    "E_UNKNOWN_TYPE",
                    "el literal de registro debe nombrar un tipo".to_string(),
                    span,
                ));
                return Ty::Unknown;
            }
        };
        let name_span = type_name_span.unwrap_or(span);

        // Resuelve el tipo nombrado a un registro estructural.
        let resolved = self.resolve_named_to_record(name);
        let record_fields = match resolved {
            Some(fs) => fs,
            None => {
                self.error(TypeError::new(
                    "E_UNKNOWN_TYPE",
                    format!("'{name}' no es un tipo registro conocido"),
                    name_span,
                ));
                // Aun así chequea las expresiones de los campos.
                for fi in fields {
                    self.check_expr(&fi.value);
                }
                return Ty::Unknown;
            }
        };

        // `Record` abierto: cualquier campo vale.
        if record_fields.is_none() {
            for fi in fields {
                self.check_expr(&fi.value);
            }
            return Ty::Unknown;
        }
        let record_fields = record_fields.unwrap();

        let mut seen: HashMap<String, Span> = HashMap::new();
        let mut provided: HashMap<String, Ty> = HashMap::new();
        for fi in fields {
            let vty = self.check_expr(&fi.value);
            if let Some(_prev) = seen.get(&fi.name) {
                self.error(TypeError::new(
                    "E_DUPLICATE_BINDING",
                    format!("el campo '{}' está repetido en el literal de registro", fi.name),
                    fi.span,
                ));
                continue;
            }
            seen.insert(fi.name.clone(), fi.span);

            match record_fields.iter().find(|(n, _)| n == &fi.name) {
                Some((_, expected)) => {
                    // Un literal del fuente vale como Html (confianza directa).
                    let vty = if matches!(expected, Ty::Html)
                        && matches!(fi.value, Expr::Str { .. })
                    {
                        Ty::Html
                    } else {
                        vty.clone()
                    };
                    if !self.is_subtype(&vty, expected) {
                        self.error(TypeError::new(
                            "E_ARG_TYPE",
                            format!(
                                "el campo '{}' es '{}' pero se esperaba '{}'",
                                fi.name,
                                vty.display(),
                                expected.display()
                            ),
                            fi.span,
                        ));
                    }
                }
                None => {
                    self.error(TypeError::new(
                        "E_NO_FIELD",
                        format!("el tipo '{name}' no tiene el campo '{}'", fi.name),
                        fi.span,
                    ));
                }
            }
            provided.insert(fi.name.clone(), vty);
        }

        // Campos faltantes.
        for (fname, _) in &record_fields {
            if !provided.contains_key(fname) {
                self.error(TypeError::new(
                    "E_ARG_TYPE",
                    format!("falta el campo '{fname}' en el literal de tipo '{name}'"),
                    span,
                ));
            }
        }

        Ty::Named(name.to_string())
    }

    /// Resuelve un nombre a sus campos de registro.
    /// - `Some(Some(fields))`: registro estructural con esos campos.
    /// - `Some(None)`: tipo abierto (`Record` builtin) — cualquier campo vale.
    /// - `None`: no es un tipo registro.
    fn resolve_named_to_record(&self, name: &str) -> Option<Option<Vec<(String, Ty)>>> {
        if name == "Record" {
            return Some(None);
        }
        // Un alias cíclico no es resoluble: cortamos antes de recurrir.
        if self.cyclic.contains(name) {
            return None;
        }
        let alias = self.aliases.get(name)?;
        match alias {
            Type::Record { fields, .. } => Some(Some(
                fields
                    .iter()
                    .map(|f| (f.name.clone(), self.ty_from_syntax(&f.ty)))
                    .collect(),
            )),
            // `type User = Record;` → registro abierto.
            Type::Name { name: inner, .. } if inner == "Record" => Some(None),
            // Alias a otro alias.
            Type::Name { name: inner, .. } => self.resolve_named_to_record(inner),
            _ => None,
        }
    }

    fn check_list(&mut self, elements: &[Expr], span: Span) -> Ty {
        let tys: Vec<Ty> = elements.iter().map(|e| self.check_expr(e)).collect();
        // Elemento = tipo del primero si todos coinciden (ignorando Unknown).
        // Tipos concretos incompatibles en la misma lista = error (si se dejara
        // pasar como List<Unknown>, ese Unknown envenenaría usos posteriores como
        // `len(xs[i])`, que en WASM hace un i32.load ciego de memoria ajena).
        let elem = match tys.first() {
            None => Ty::Unknown,
            Some(first) => {
                if tys
                    .iter()
                    .all(|t| matches!(t, Ty::Unknown) || t == first)
                {
                    first.clone()
                } else {
                    self.error(TypeError::new(
                        "E_LIST_HETEROGENEOUS",
                        "los elementos de una lista deben ser todos del mismo tipo",
                        span,
                    ));
                    Ty::Unknown
                }
            }
        };
        Ty::List(Box::new(elem))
    }

    /// `xs[i]`: el objeto debe ser una lista y el índice un Int; el resultado es
    /// el tipo del elemento.
    fn check_index(&mut self, object: &Expr, index: &Expr) -> Ty {
        let obj_ty = self.check_expr(object);
        let idx_ty = self.check_expr(index);
        if !matches!(idx_ty, Ty::Int | Ty::Unknown) {
            self.error(TypeError::new(
                "E_INDEX_NOT_INT",
                format!("el índice debe ser Int, no '{}'", idx_ty.display()),
                index.span(),
            ));
        }
        match obj_ty {
            Ty::List(elem) => *elem,
            Ty::Unknown => Ty::Unknown,
            other => {
                self.error(TypeError::new(
                    "E_INDEX_NOT_LIST",
                    format!("no se puede indexar un valor de tipo '{}'", other.display()),
                    object.span(),
                ));
                Ty::Unknown
            }
        }
    }

    // ----------------------- utilidades de tipos -----------------------

    /// Traduce un `Type` sintáctico al `Ty` interno (sin validar existencia).
    fn ty_from_syntax(&self, t: &Type) -> Ty {
        self.ty_from_syntax_guarded(t, &mut std::collections::HashSet::new())
    }

    /// Igual que `ty_from_syntax`, pero con un conjunto de alias en expansión para
    /// cortar los tipos registro estructuralmente recursivos (p.ej.
    /// `type Nodo = { siguiente: Nodo }`, una lista enlazada perfectamente válida).
    /// Sin esta guarda, expandir el campo `siguiente` vuelve a `Nodo` y recurre
    /// hasta desbordar la pila. Nótese que estos tipos NO son un error —los ciclos
    /// de alias *transparentes* (`type A = A`) sí, y esos ya se marcan en `cyclic`.
    fn ty_from_syntax_guarded(
        &self,
        t: &Type,
        expanding: &mut std::collections::HashSet<String>,
    ) -> Ty {
        match t {
            Type::Name { name, args, .. } if name == "List" => {
                // `List` o `List<T>`: el elemento es el argumento, o desconocido.
                let elem = match args.first() {
                    Some(a) => self.ty_from_syntax_guarded(a, expanding),
                    None => Ty::Unknown,
                };
                Ty::List(Box::new(elem))
            }
            Type::Name { name, .. } => {
                if let Some(prim) = builtins::type_lookup(name) {
                    return prim;
                }
                // Un alias cíclico no resuelve: lo tratamos como Unknown para no
                // recurrir infinitamente (el E_CYCLIC_TYPE ya se reportó).
                if self.cyclic.contains(name) {
                    return Ty::Unknown;
                }
                // Alias a registro → registro; alias a otra cosa → su Ty;
                // de lo contrario, Named opaco.
                if let Some(alias) = self.aliases.get(name) {
                    // Si ya estábamos expandiendo este alias, la referencia es
                    // recursiva: la dejamos opaca (`Named`) en vez de recurrir.
                    // El acceso posterior (`n.siguiente`) la re-resuelve un nivel
                    // por vez vía `resolve_named_to_record`.
                    if !expanding.insert(name.clone()) {
                        return Ty::Named(name.clone());
                    }
                    let ty = self.ty_from_syntax_guarded(alias, expanding);
                    expanding.remove(name);
                    return ty;
                }
                Ty::Named(name.clone())
            }
            Type::Union { variants, .. } => {
                // Cada variante nominal aporta su nombre.
                let mut names = Vec::new();
                for v in variants {
                    match v {
                        Type::Name { name, .. } => names.push(name.clone()),
                        _ => names.push("?".to_string()),
                    }
                }
                Ty::Union(names)
            }
            Type::Record { fields, .. } => Ty::Record(
                fields
                    .iter()
                    .map(|f| (f.name.clone(), self.ty_from_syntax_guarded(&f.ty, expanding)))
                    .collect(),
            ),
        }
    }

    /// Valida que un `Type` referencie sólo primitivos, alias o builtins.
    fn validate_type_exists(&mut self, t: &Type) {
        match t {
            Type::Name { name, args, span } => {
                let known = builtins::type_lookup(name).is_some()
                    || self.aliases.contains_key(name)
                    || name == "Record"
                    || name == "List";
                if !known {
                    self.error(TypeError::new(
                        "E_UNKNOWN_TYPE",
                        format!("el tipo '{name}' no está declarado"),
                        *span,
                    ));
                }
                for a in args {
                    self.validate_type_exists(a);
                }
            }
            Type::Union { variants, .. } => {
                for v in variants {
                    // En una unión, los nombres pueden ser etiquetas nominales
                    // (NotFound, Error) sin declaración: sólo validamos los que
                    // claramente son tipos estructurales o args.
                    if let Type::Record { .. } = v {
                        self.validate_type_exists(v);
                    }
                }
            }
            Type::Record { fields, .. } => {
                for f in fields {
                    self.validate_type_exists(&f.ty);
                }
            }
        }
    }

    /// Relación de subtipado. `Unknown` es subtipo y supertipo de todo (silencia).
    fn is_subtype(&self, sub: &Ty, sup: &Ty) -> bool {
        self.is_subtype_rec(sub, sup, &mut std::collections::HashSet::new())
    }

    /// Clave del par que se está desplegando, para la regla de Amber. Se usa el
    /// par COMPLETO (sub, sup) y no solo el nombre del alias: con un conjunto de
    /// nombres, reencontrar `A` aceptaba contra cualquier cosa, así que el mismo
    /// par se rechazaba a profundidad 3 y se aceptaba a profundidad 4.
    fn par_clave(sub: &Ty, sup: &Ty) -> (String, String) {
        (sub.display(), sup.display())
    }

    /// Igual que `is_subtype`, pero con un conjunto de nombres de alias ya
    /// desplegados para dar subtipado *coinductivo* sobre tipos registro
    /// mutuamente recursivos (`type A = { x: B }; type B = { y: A }`). Sin la
    /// guarda, `is_subtype` alterna desplegando `A`→`B`→`A`… hasta desbordar la
    /// pila. Al reencontrar un nombre en despliegue lo aceptamos: es la regla
    /// estándar de subtipado equirecursivo (asumir la meta y verla cumplida).
    fn is_subtype_rec(
        &self,
        sub: &Ty,
        sup: &Ty,
        unfolding: &mut std::collections::HashSet<(String, String)>,
    ) -> bool {
        // `Html` es la excepción al comodín: `Unknown` absorbe operaciones para
        // no encadenar errores, pero si además se colara en `Html` entonces un
        // `Record`, un campo de tipo abierto o un `match` con ramas de tipos
        // distintos lavarían cualquier dato hasta el DOM. Un tipo que codifica
        // confianza no puede aceptar "no sé".
        if matches!(sup, Ty::Html) {
            return matches!(sub, Ty::Html);
        }
        if matches!(sub, Ty::Unknown) || matches!(sup, Ty::Unknown) {
            return true;
        }
        match (sub, sup) {
            (a, b) if a == b => true,
            // Un marcado seguro vale donde se espera texto; lo contrario no,
            // que es justamente lo que impide que un dato sin escapar llegue al
            // DOM. La conversión String -> Html es explícita: `escapar`/`html`.
            (Ty::Html, Ty::String) => true,
            // Una variante nominal individual es subtipo de la unión que la contiene.
            (Ty::Named(n), Ty::Union(vs)) => vs.contains(n),
            // Una unión es subtipo de otra si todas sus variantes están contenidas.
            (Ty::Union(a), Ty::Union(b)) => a.iter().all(|v| b.contains(v)),
            // Listas: covariantes en el elemento. `List<?>` (lista vacía o de
            // elemento desconocido) es subtipo de cualquier `List<T>` porque el
            // elemento Unknown es subtipo de todo.
            (Ty::List(ea), Ty::List(eb)) => self.is_subtype_rec(ea, eb, unfolding),
            // Registros estructurales: ancho + profundidad.
            (Ty::Record(sa), Ty::Record(sb)) => sb.iter().all(|(n, tb)| {
                sa.iter()
                    .find(|(na, _)| na == n)
                    .map(|(_, ta)| self.is_subtype_rec(ta, tb, unfolding))
                    .unwrap_or(false)
            }),
            // Un nombre de tipo que resuelve a un registro estructural es
            // intercambiable con su forma estructural: así un literal de registro
            // (tipado nominalmente como su alias) es subtipo del mismo `type` que
            // se declaró como `{ ... }` (p.ej. `fn origen() -> Punto` que devuelve
            // `Punto { x, y }`), y viceversa.
            (Ty::Named(n), Ty::Record(_)) => {
                // Par ya en despliegue → aceptar (regla de Amber).
                let clave = Self::par_clave(sub, sup);
                if !unfolding.insert(clave.clone()) {
                    return true;
                }
                let ok = match self.resolve_named_to_record(n) {
                    Some(Some(fields)) => {
                        self.is_subtype_rec(&Ty::Record(fields), sup, unfolding)
                    }
                    // `Record` abierto: acepta cualquier registro estructural.
                    Some(None) => true,
                    None => false,
                };
                unfolding.remove(&clave);
                ok
            }
            (Ty::Record(_), Ty::Named(n)) => {
                let clave = Self::par_clave(sub, sup);
                if !unfolding.insert(clave.clone()) {
                    return true;
                }
                let ok = match self.resolve_named_to_record(n) {
                    Some(Some(fields)) => {
                        self.is_subtype_rec(sub, &Ty::Record(fields), unfolding)
                    }
                    Some(None) => true,
                    None => false,
                };
                unfolding.remove(&clave);
                ok
            }
            // Un registro estructural es subtipo de una unión si coincide con
            // alguna de sus variantes: `{nombre: String}` vale donde se espera
            // `User | NotFound` si `User` es ese registro. Sin esta regla no se
            // podía devolver el valor de un store tipado desde una función cuyo
            // retorno es una unión, que es el patrón central del lenguaje.
            (Ty::Record(_), Ty::Union(vs)) => vs.iter().any(|v| {
                let clave = (sub.display(), v.clone());
                if unfolding.contains(&clave) {
                    return true;
                }
                match self.resolve_named_to_record(v) {
                    Some(Some(fields)) => {
                        let mut u = unfolding.clone();
                        u.insert(clave);
                        self.is_subtype_rec(sub, &Ty::Record(fields), &mut u)
                    }
                    Some(None) => true,
                    None => false,
                }
            }),
            _ => false,
        }
    }
}

/// Identificadores que el bundle importa del runtime. No son builtins del
/// lenguaje (no se pueden llamar desde un `.mar`), pero sí ocupan el espacio de
/// nombres del archivo generado: declarar uno produce un `const`/`function` que
/// redeclara el import y el archivo entero deja de cargar.
/// Builtins que el codegen emite SIN `await` (ver `is_sync_builtin` en
/// `marea-codegen`). Las dos listas deben coincidir: si aquí sobra un nombre se
/// generará un `await` en un contexto síncrono, y si falta se rechazará código
/// válido.
fn es_builtin_sincrono(name: &str) -> bool {
    matches!(
        name,
        "print" | "concat" | "render" | "len" | "text" | "escape" | "html"
            | "concat" | "append" | "len" | "contains" | "lower"
            | "jsonText" | "jsonInt" | "jsonFloat" | "jsonLen"
    )
}

fn es_interno_del_runtime(name: &str) -> bool {
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

/// Nombre textual de un callee (`getUser`, `todos`).
fn callee_name(callee: &Expr) -> String {
    match callee {
        Expr::Ident { name, .. } => name.clone(),
        Expr::Member { object, field, .. } => format!("{}.{}", callee_name(object), field),
        _ => "<expr>".to_string(),
    }
}

/// ¿Es un tipo serializable a través de la frontera de red?
fn is_serializable(ty: &Ty) -> bool {
    match ty {
        Ty::Int | Ty::Float | Ty::Bool | Ty::String | Ty::Html | Ty::Unit | Ty::Unknown => true,
        // Un almacén es un asa del servidor: no tiene representación en el cable.
        Ty::Store(_, _) => false,
        // Las uniones de etiquetas/escalares son serializables (etiqueta + datos).
        Ty::Union(_) => true,
        Ty::Named(_) => true,
        Ty::Record(fields) => fields.iter().all(|(_, t)| is_serializable(t)),
        // Una lista es serializable si su elemento lo es.
        Ty::List(elem) => is_serializable(elem),
        // Una función no cruza la frontera.
        Ty::Fn { .. } => false,
    }
}

/// ¿El bloque termina garantizado (return en todo camino)?
fn block_terminates(block: &Block) -> bool {
    block.stmts.iter().any(stmt_terminates)
}

fn stmt_terminates(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::Expr(e) => expr_terminates(e),
        Stmt::Let(_) | Stmt::Assign { .. } | Stmt::Effect { .. } => false,
    }
}

fn expr_terminates(expr: &Expr) -> bool {
    match expr {
        Expr::If { then_branch, else_branch, .. } => {
            let then_ok = block_terminates(then_branch);
            let else_ok = match else_branch {
                Some(eb) => match eb.as_ref() {
                    ElseBranch::Block(b) => block_terminates(b),
                    ElseBranch::If(e) => expr_terminates(e),
                },
                None => false,
            };
            then_ok && else_ok
        }
        Expr::Match { arms, .. } => {
            !arms.is_empty() && arms.iter().all(|a| expr_terminates(&a.body))
        }
        _ => false,
    }
}
