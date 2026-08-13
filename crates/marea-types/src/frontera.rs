//! La frontera de red y el estado del servidor: clasificación de cruces
//! `@client`→`@server`, qué tipos son serializables y los builtins de
//! estado, que sólo existen del lado servidor.

use super::*;

impl Checker {
    /// Chequea `guardar(x)` y `todos()` contra el store tipado (`store T;`):
    /// sólo en @server/@edge, requieren la declaración, y `guardar` exige que su
    /// argumento sea del tipo del store; `todos()` devuelve `List<T>`.
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
