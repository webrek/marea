//! La frontera de red y el estado del servidor: clasificación de cruces
//! `@client`→`@server`, qué tipos son serializables y los builtins de
//! estado, que sólo existen del lado servidor.

use super::*;

impl Checker {
    /// Chequea `save(a, x)` y `all(a)` contra el almacén tipado
    /// (`store a: T;`): sólo en @server/@edge, exigen que el almacén esté
    /// declarado, y `save` que su argumento sea del tipo de ESE almacén —guardar
    /// un Producto en `ordenes` no compila—; `all(a)` devuelve `List<T>`.
    pub(crate) fn check_state_builtin(&mut self, name: &str, args: &[Expr], span: Span) -> Ty {
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

    /// Clasifica una llamada respecto a la frontera de red y valida reglas.
    pub(crate) fn classify_boundary(
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
            let lado = if from == Some(Location::Edge) {
                "@edge"
            } else {
                "@server"
            };
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
        let is_valid_source = matches!(from, None | Some(Location::Client) | Some(Location::Edge));
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
}

/// ¿Es un tipo serializable a través de la frontera de red?
pub(crate) fn is_serializable(ty: &Ty) -> bool {
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

// --- Política: quién puede cruzar la frontera ---------------------------------
//
// La ubicación ya es parte del tipo de una función; el permiso también debería
// serlo. Si la tesis es que la frontera de red la maneja el compilador, decidir
// quién la cruza no puede quedar fuera: un middleware sería contradecirse.
//
// La forma es la misma que ya se usó dos veces en este lenguaje. `Html` volvió
// el escapado no-opcional; `$tag` volvió infalsificable la variante. Aquí: que
// algo sea público hay que escribirlo.

impl Checker {
    /// Fase A: localiza la función `@session` y deduce el tipo de identidad que
    /// resuelve. Su presencia es lo que activa la exigencia de política.
    pub(crate) fn collect_session(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Fn(f) = item else { continue };
            if !f.es_session {
                continue;
            }
            if let Some(previa) = &self.session {
                self.error(TypeError::new(
                    "E_SESSION_DUPLICADA",
                    format!(
                        "ya hay una función @session ('{}'); un programa resuelve la \
                         identidad de una sola manera",
                        previa.fn_name
                    ),
                    f.span,
                ));
                continue;
            }
            // Firma: recibe el token y devuelve una unión con la identidad y al
            // menos un fallo. La unión obliga a que quien la use cubra el caso
            // de "no autorizado": es la regla del match aplicada a la identidad.
            let params_ok = f.params.len() == 1
                && matches!(
                    self.ty_from_syntax(&f.params[0].ty),
                    Ty::String | Ty::Unknown
                );
            let variantes = match &f.return_type {
                Some(Type::Union { variants, .. }) => variants
                    .iter()
                    .filter_map(|v| match v {
                        Type::Name { name, .. } => Some(name.clone()),
                        _ => None,
                    })
                    .collect::<Vec<_>>(),
                _ => Vec::new(),
            };
            // La identidad es la primera variante que resuelve a un tipo
            // declarado; el resto son los fallos (`NoAutorizado`, …).
            let identidad = variantes.iter().find(|v| self.aliases.contains_key(*v));
            match (params_ok, identidad) {
                (true, Some(id)) if variantes.len() >= 2 => {
                    self.session = Some(Sesion {
                        fn_name: f.name.clone(),
                        identidad: id.clone(),
                    });
                }
                _ => {
                    self.error(TypeError::new(
                        "E_SESSION_FIRMA",
                        format!(
                            "una función @session recibe el token y devuelve la identidad o \
                             un fallo: 'fn {}(token: String) -> TuTipo | NoAutorizado'",
                            f.name
                        ),
                        f.span,
                    ));
                }
            }
        }
    }

    /// Fase A: comprueba la política de cada handler. Sin `@session` declarada
    /// no se exige nada —no se le puede pedir identidad a un programa que no ha
    /// dicho qué es una identidad—, pero en cuanto existe, `@server` a secas
    /// deja de compilar.
    pub(crate) fn check_politicas(&mut self, module: &Module) {
        for item in &module.items {
            let Item::Fn(f) = item else { continue };
            if f.es_session {
                continue;
            }
            let en_servidor = matches!(f.location, Some(Location::Server) | Some(Location::Edge));
            match &f.politica {
                Some(politica) => self.check_politica_declarada(f, politica, en_servidor),
                None if en_servidor && self.session.is_some() => {
                    let quien = self.session.as_ref().map(|s| s.identidad.clone());
                    self.error(TypeError::new(
                        "E_SERVER_SIN_POLITICA",
                        format!(
                            "'{}' cruza la frontera y no dice quién puede llegar hasta ella; \
                             escribe '@server({})' si exige identidad o '@server(Public)' si \
                             de verdad es pública",
                            f.name,
                            quien.unwrap_or_else(|| "Identidad".to_string())
                        ),
                        f.span,
                    ));
                }
                None => {}
            }
        }
    }

    fn check_politica_declarada(&mut self, f: &FnDecl, politica: &Type, en_servidor: bool) {
        if !en_servidor {
            self.error(TypeError::new(
                "E_POLITICA_OFF_SERVER",
                format!(
                    "'{}' no cruza la frontera de red, así que no tiene a quién exigirle \
                     identidad; la política solo aplica a @server y @edge",
                    f.name
                ),
                politica.span(),
            ));
            return;
        }
        let Type::Name { name, args, .. } = politica else {
            self.error(TypeError::new(
                "E_POLITICA_DESCONOCIDA",
                "la política es el nombre de un tipo: '@server(Public)' o el tipo que \
                 resuelve tu función @session"
                    .to_string(),
                politica.span(),
            ));
            return;
        };
        if !args.is_empty() {
            self.error(TypeError::new(
                "E_POLITICA_DESCONOCIDA",
                format!("la política '{name}' no lleva parámetros de tipo"),
                politica.span(),
            ));
            return;
        }
        // `Public` es la decisión explícita de no exigir identidad. Vale siempre,
        // también sin @session: es lo que hace que la regla sea una decisión y no
        // un impuesto.
        if name == "Public" {
            return;
        }
        match &self.session {
            None => self.error(TypeError::new(
                "E_POLITICA_SIN_SESSION",
                format!(
                    "'{}' exige una identidad de tipo '{name}', pero el programa no declara \
                     ninguna función @session que la resuelva",
                    f.name
                ),
                politica.span(),
            )),
            Some(s) if s.identidad != *name => {
                let esperado = s.identidad.clone();
                self.error(TypeError::new(
                    "E_POLITICA_NO_COINCIDE",
                    format!(
                        "la política de '{}' es '{name}', pero la @session del programa \
                         resuelve '{esperado}'",
                        f.name
                    ),
                    politica.span(),
                ));
            }
            Some(_) => {}
        }
    }
}
