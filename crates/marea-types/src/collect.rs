//! Fase A: recolección global de firmas y alias antes de chequear cuerpos.
//! Registrarlo todo primero es lo que permite recursión mutua y orden
//! libre de declaraciones; aquí vive también la detección de ciclos de
//! alias, que sin guarda desbordaba la pila.

use super::*;

/// Un `store` visto en Fase A, a la espera de que los alias estén registrados
/// para poder resolver su tipo.
struct StoreDecl<'a> {
    nombre: String,
    /// El del NOMBRE: es lo que se subraya en los errores del almacén.
    span: Span,
    ty: &'a Type,
    /// `Some(tabla)` si se escribió `from "tabla"`: el almacén es prestado.
    tabla: Option<&'a str>,
    tabla_span: Option<Span>,
}

impl Checker {
    /// Registra las variables de nivel superior (`let`/`reactive` de módulo) como
    /// globales, tipándolas por su anotación o por su inicializador. Corre tras
    /// `collect` (necesita fns/alias/store ya resueltos) y antes de los cuerpos.
    pub(crate) fn collect_globals(&mut self, module: &Module) {
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

    pub(crate) fn collect(&mut self, module: &Module) {
        // Spans de la primera declaración de cada nombre, para notas de duplicado.
        let mut fn_spans: HashMap<String, Span> = HashMap::new();
        let mut type_spans: HashMap<String, Span> = HashMap::new();
        // Los `store` vistos (se resuelven tras registrar los alias).
        let mut store_decls: Vec<StoreDecl> = Vec::new();

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
                            .with_note(format!("primera declaración en el byte {}", prev.start)),
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
                            .with_note(format!("primera declaración en el byte {}", prev.start)),
                        );
                    } else {
                        type_spans.insert(t.name.clone(), t.span);
                        self.aliases.insert(t.name.clone(), t.aliased.clone());
                    }
                }
                Item::Let(_) => {}
                Item::Store {
                    name,
                    name_span,
                    ty,
                    tabla_externa,
                    tabla_span,
                    span,
                } => {
                    if store_decls.iter().any(|d| &d.nombre == name) {
                        self.error(TypeError::new(
                            "E_DUPLICATE_STORE",
                            format!("el almacén '{name}' ya fue declarado"),
                            *span,
                        ));
                    } else {
                        store_decls.push(StoreDecl {
                            nombre: name.clone(),
                            span: *name_span,
                            ty,
                            tabla: tabla_externa.as_deref(),
                            tabla_span: *tabla_span,
                        });
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
        //
        // `tablas` lleva a qué tabla va a parar cada almacén —la prestada, o la
        // que Marea deriva del nombre para uno propio— para que dos no acaben en
        // el mismo sitio. Guarda además si era prestado: dos propios que chocan
        // ya lo dicen mejor los duplicados de nombre.
        let mut tablas: HashMap<String, (String, bool)> = HashMap::new();
        for StoreDecl {
            nombre,
            span,
            ty,
            tabla,
            tabla_span,
        } in store_decls
        {
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
            // y todo `save` revienta en runtime.
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
            // `store p: T from "tabla";` — el almacén es PRESTADO: la tabla ya
            // existe y la escribe otro. Marea deja de mandar en su esquema, así
            // que se le exige más al fuente que a un almacén propio.
            if let Some(tabla) = tabla {
                let sp = tabla_span.unwrap_or(span);
                let limpia = tabla.trim();
                if limpia.is_empty() {
                    self.error(TypeError::new(
                        "E_TABLA_EXTERNA_VACIA",
                        format!(
                            "'{nombre}' toma prestada una tabla sin nombre; escribe el nombre \
                             exacto de la tabla ajena, como en 'store {nombre}: T from \
                             \"products\";'"
                        ),
                        sp,
                    ));
                }
                // Una tabla ajena tiene COLUMNAS, y el tipo es lo único que dice
                // a qué campo va cada una. Un almacén propio sí admite un escalar
                // —Marea manda en el esquema y lo guarda en una columna suya—;
                // prestado no hay dónde meterlo ni de dónde sacarlo.
                if !matches!(elem, Ty::Record(_) | Ty::Named(_) | Ty::Unknown) {
                    self.error(TypeError::new(
                        "E_STORE_PRESTADO_NO_REGISTRO",
                        format!(
                            "'{nombre}' toma prestada la tabla '{limpia}' pero su tipo es '{}', \
                             que no tiene campos: una tabla ajena tiene columnas y hace falta \
                             decir a qué campo va cada una",
                            elem.display()
                        ),
                        span,
                    ));
                }
                self.stores_prestados
                    .insert(nombre.clone(), limpia.to_string());
            }
            // El nombre de la tabla se compara en minúsculas porque en minúsculas
            // se deriva la de un almacén propio, y porque la mayoría de motores
            // no distingue caja en los identificadores sin comillas.
            let destino = match tabla {
                Some(t) => t.trim().to_lowercase(),
                None => nombre.to_lowercase(),
            };
            if !destino.is_empty() {
                match tablas.get(&destino) {
                    // Dos vistas del mismo sitio con dos tipos: la deriva entre
                    // ellos no se vería, porque nadie los compara nunca.
                    Some((previo, previo_prestado)) if *previo_prestado || tabla.is_some() => {
                        self.error(TypeError::new(
                            "E_TABLA_EXTERNA_DUPLICADA",
                            format!(
                                "'{nombre}' y '{previo}' apuntan a la misma tabla '{destino}': \
                                 serían dos vistas del mismo sitio con dos tipos, y la deriva \
                                 entre ellas no la vería nadie. Deja un solo almacén por tabla"
                            ),
                            tabla_span.unwrap_or(span),
                        ));
                    }
                    _ => {
                        tablas.insert(destino, (nombre.clone(), tabla.is_some()));
                    }
                }
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

    pub(crate) fn detect_cyclic_types(&mut self, type_spans: &HashMap<String, Span>) {
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
    pub(crate) fn alias_cycles(
        &self,
        name: &str,
        visiting: &mut std::collections::HashSet<String>,
    ) -> bool {
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

    pub(crate) fn type_refs_cycle(
        &self,
        t: &Type,
        visiting: &mut std::collections::HashSet<String>,
    ) -> bool {
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
}
