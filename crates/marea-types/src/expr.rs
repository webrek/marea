//! Tipado de expresiones: identificadores, operadores, llamadas, acceso a
//! miembro, literales de registro y de lista, e indexado.

use super::*;

impl Checker {
    pub(crate) fn check_expr(&mut self, expr: &Expr) -> Ty {
        match expr {
            Expr::Int { .. } => Ty::Int,
            Expr::Float { .. } => Ty::Float,
            Expr::Str { .. } => Ty::String,
            Expr::Bool { .. } => Ty::Bool,
            Expr::Ident { name, span } => self.resolve_ident(name, *span),
            Expr::Unary {
                op,
                expr: inner,
                span,
            } => self.check_unary(*op, inner, *span),
            Expr::Binary {
                op,
                left,
                right,
                span,
            } => self.check_binary(*op, left, right, *span),
            Expr::Call { callee, args, span } => self.check_call(callee, args, *span),
            Expr::Member {
                object,
                field,
                span,
            } => self.check_member(object, field, *span),
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => self.check_if(cond, then_branch, else_branch.as_deref()),
            Expr::Match {
                scrutinee,
                arms,
                span,
            } => self.check_match(scrutinee, arms, *span),
            Expr::Record {
                type_name,
                type_name_span,
                fields,
                span,
            } => self.check_record(type_name.as_deref(), *type_name_span, fields, *span),
            Expr::List { elements, span } => self.check_list(elements, *span),
            Expr::Index { object, index, .. } => self.check_index(object, index),
            Expr::Fn {
                params,
                return_type,
                body,
                span,
            } => self.check_fn_expr(params, return_type.as_ref(), body, *span),
            Expr::Template { parts, .. } => {
                // Una plantilla SIEMPRE es `Html`: los huecos `{x}` se escapan
                // al emitir, y `{!x}` solo admite algo que ya sea Html. Por eso
                // olvidarse del escapado deja de ser posible: no hay forma de
                // meter texto sin escapar por ninguna de las dos puertas.
                for parte in parts {
                    if let marea_syntax::ast::TemplatePart::Interp { expr, raw } = parte {
                        let t = self.check_expr(expr);
                        // `on` produce un ATRIBUTO: escapado deja de serlo y el
                        // elemento queda con un `data-marea-on-...` que ningún
                        // manejador reclama, es decir un botón mudo y ni un
                        // aviso. Es el hueco crudo o nada.
                        if !*raw && crate::eventos::es_llamada_a_on(expr) {
                            self.error(TypeError::new(
                                "E_ON_ESCAPADO",
                                format!(
                                    "'{}' escribe un atributo, así que va en un hueco CRUDO: \
                                     escribe '{{!{}(...)}}'. Escapado, el navegador lo lee como \
                                     texto y el manejador nunca se engancha",
                                    crate::eventos::ON,
                                    crate::eventos::ON
                                ),
                                expr.span(),
                            ));
                        }
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
    pub(crate) fn check_expr_html(&mut self, e: &Expr) -> Ty {
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
    pub(crate) fn tipo_de_recurso(&self, e: &Expr) -> Option<Ty> {
        let Expr::Call { callee, .. } = e else {
            return None;
        };
        let Expr::Ident { name, .. } = callee.as_ref() else {
            return None;
        };
        if es_sincrono(name) {
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

    pub(crate) fn resolve_ident(&mut self, name: &str, span: Span) -> Ty {
        // Variable en algún scope (del más interno al más externo). Si se
        // resolvió por fuera del cierre en curso, el uso es una captura.
        if let Some((idx, ty, mutable)) = self.buscar_en_scopes(name) {
            self.check_captura(name, idx, mutable, span);
            return ty;
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

    pub(crate) fn check_unary(&mut self, op: UnaryOp, inner: &Expr, span: Span) -> Ty {
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

    pub(crate) fn check_binary(&mut self, op: BinOp, left: &Expr, right: &Expr, span: Span) -> Ty {
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
                let compatibles = lt.is_scalar()
                    && rt.is_scalar()
                    && (self.is_subtype(&lt, &rt) || self.is_subtype(&rt, &lt));
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

    /// Chequea la aridad exacta de un builtin; devuelve `true` si coincide.
    pub(crate) fn arity(&mut self, name: &str, args: &[Ty], expected: usize, span: Span) -> bool {
        if args.len() != expected {
            self.error(TypeError::new(
                "E_ARITY",
                format!(
                    "'{name}' espera {expected} argumento(s), se recibieron {}",
                    args.len()
                ),
                span,
            ));
            false
        } else {
            true
        }
    }

    pub(crate) fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Ty {
        // La `@session` la invoca el runtime al recibir la petición, con el
        // token que venga en ella. Llamarla desde el programa sería fabricarse
        // una identidad pasando el token que uno quiera: justo lo que la
        // anotación existe para impedir.
        if let Some(s) = &self.session {
            let nombre = callee_name(callee);
            if nombre == s.fn_name {
                let fn_name = s.fn_name.clone();
                self.error(TypeError::new(
                    "E_SESSION_NO_INVOCABLE",
                    format!(
                        "'{fn_name}' es la @session del programa: la llama el runtime con el \
                         token de la petición. Llamarla desde aquí sería elegir tu propia \
                         identidad; recibe la que te pasa '@server(...)'"
                    ),
                    span,
                ));
            }
        }
        // Dentro de un inicializador que se evalúa de forma síncrona (el memo de
        // una `reactive`, o una global de módulo) no puede haber ninguna llamada
        // que el codegen emita con `await`: eso es todo salvo un puñado de
        // builtins síncronos. El resultado era `__memo(() => (await f()))`, un
        // await en una arrow no-async, o sea un SyntaxError que impide cargar el
        // módulo entero. No basta con mirar los cruces de red: cualquier función
        // del usuario se compila a `async`.
        if let Some(ctx) = self.init_context {
            let nombre = callee_name(callee);
            if !crate::eventos::es_sincrono_con_eventos(&nombre) {
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

        // Los builtins de ESTADO (`save`/`all`/`update`/`remove`) se chequean
        // aparte: contra el tipo del almacén y sólo dentro de @server/@edge.
        if let Expr::Ident { name, .. } = callee {
            if matches!(name.as_str(), "save" | "all" | "update" | "remove") {
                return self.check_state_builtin(name, args, span);
            }
            // El modelo de eventos se chequea aparte: el evento tiene que ser
            // uno de los conocidos y el manejador tener forma de manejador,
            // cosas que la firma del builtin sola no dice.
            if name == crate::eventos::ON {
                return self.check_on_builtin(args, span);
            }
            // La red saliente vive en el servidor, igual que el estado: desde el
            // navegador la llamada la haría el cliente (otro origen, otras
            // credenciales, y CORS decidiendo por ti), que no es lo que el
            // programa dice. Además la lista blanca de destinos es del servidor.
            if matches!(name.as_str(), "fetch" | "post")
                && !matches!(
                    self.current_location,
                    Some(Location::Server) | Some(Location::Edge)
                )
            {
                self.error(TypeError::new(
                    "E_RED_OFF_SERVER",
                    format!(
                        "'{name}' sale a la red y sólo puede usarse en una función \
                         @server; envuélvelo en una y llámala por RPC"
                    ),
                    span,
                ));
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
            if name == "concat"
                && args.len() == 2
                && matches!(self.check_expr(&args[0]), Ty::List(_))
            {
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
            Ty::Fn {
                params,
                ret,
                location,
            } => {
                // Clasifica el cruce de frontera y valida serializabilidad.
                let cruza_la_red = self.classify_boundary(callee, *location, params, ret, span);

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
                        // Lo que cruza la red viaja como JSON. `classify_boundary`
                        // ya miró la FIRMA, pero un parámetro `Unknown` acepta
                        // cualquier cosa, así que aquí se mira lo que de verdad
                        // se manda. El caso que importa es un cierre: es código,
                        // no un dato, y no hay JSON que lo represente —por eso un
                        // `sort(xs, criterio)` se queda de este lado del cable.
                        if cruza_la_red && !crate::frontera::is_serializable(aty) {
                            self.error(TypeError::new(
                                "E_BOUNDARY_NOT_SERIALIZABLE",
                                format!(
                                    "el argumento {} es '{}' y no cruza la frontera de red",
                                    i + 1,
                                    aty.display()
                                ),
                                args[i].span(),
                            ));
                            continue;
                        }
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

    pub(crate) fn check_member(&mut self, object: &Expr, field: &str, span: Span) -> Ty {
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

    pub(crate) fn check_if(
        &mut self,
        cond: &Expr,
        then_branch: &Block,
        else_branch: Option<&ElseBranch>,
    ) -> Ty {
        let cond_ty = self.check_expr(cond);
        if !matches!(cond_ty, Ty::Bool | Ty::Unknown) {
            self.error(TypeError::new(
                "E_COND_NOT_BOOL",
                format!(
                    "la condición del 'if' debe ser Bool, no '{}'",
                    cond_ty.display()
                ),
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

    pub(crate) fn check_record(
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
                    format!(
                        "el campo '{}' está repetido en el literal de registro",
                        fi.name
                    ),
                    fi.span,
                ));
                continue;
            }
            seen.insert(fi.name.clone(), fi.span);

            match record_fields.iter().find(|(n, _)| n == &fi.name) {
                Some((_, expected)) => {
                    // Un literal del fuente vale como Html (confianza directa).
                    let vty =
                        if matches!(expected, Ty::Html) && matches!(fi.value, Expr::Str { .. }) {
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

    pub(crate) fn check_list(&mut self, elements: &[Expr], span: Span) -> Ty {
        let tys: Vec<Ty> = elements.iter().map(|e| self.check_expr(e)).collect();
        // Elemento = tipo del primero si todos coinciden (ignorando Unknown).
        // Tipos concretos incompatibles en la misma lista = error (si se dejara
        // pasar como List<Unknown>, ese Unknown envenenaría usos posteriores como
        // `len(xs[i])`, que en WASM hace un i32.load ciego de memoria ajena).
        let elem = match tys.first() {
            None => Ty::Unknown,
            Some(first) => {
                if tys.iter().all(|t| matches!(t, Ty::Unknown) || t == first) {
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
    pub(crate) fn check_index(&mut self, object: &Expr, index: &Expr) -> Ty {
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

    // --------------------------- cierres ---------------------------

    /// Tipa una función anónima: `fn(a: Int, b: Int) -> Bool { ... }`.
    ///
    /// El cuerpo se chequea en un scope PROPIO apilado sobre los de fuera —eso
    /// es la captura: lo de fuera se sigue viendo— y con la misma ubicación que
    /// el `fn` que lo crea. Un cierre escrito en una @client es @client, así que
    /// las reglas de estado, de red y de frontera le caen encima solas, sin una
    /// anotación nueva que escribir ni una excepción nueva que recordar.
    pub(crate) fn check_fn_expr(
        &mut self,
        params: &[marea_syntax::ast::Param],
        return_type: Option<&Type>,
        body: &Block,
        span: Span,
    ) -> Ty {
        // El índice de este scope es la frontera del cierre: lo que se resuelva
        // por debajo viene de fuera y por tanto se captura.
        self.scopes.push(HashMap::new());
        self.closure_bases.push(self.scopes.len() - 1);

        let mut param_tys: Vec<Ty> = Vec::with_capacity(params.len());
        let mut vistos: HashMap<String, ()> = HashMap::new();
        for p in params {
            self.validate_type_exists(&p.ty);
            if vistos.insert(p.name.clone(), ()).is_some() {
                self.error(TypeError::new(
                    "E_DUPLICATE_PARAM",
                    format!("el parámetro '{}' está repetido", p.name),
                    p.span,
                ));
            }
            let pty = self.ty_from_syntax(&p.ty);
            param_tys.push(pty.clone());
            // Inmutables, igual que los de una función con nombre.
            self.scopes
                .last_mut()
                .unwrap()
                .insert(p.name.clone(), (pty, false));
        }

        let declarado = return_type.map(|t| {
            self.validate_type_exists(t);
            self.ty_from_syntax(t)
        });

        // Un cierre puede aparecer dentro de otro y dentro de cualquier función,
        // así que el estado del chequeo se salva y se restaura entero.
        let prev_return = std::mem::replace(
            &mut self.current_return,
            declarado.clone().unwrap_or(Ty::Unit),
        );
        // Sin `-> T` escrito no hay contra qué comparar los `return`: son ELLOS
        // los que dicen el tipo, así que se acumulan en vez de chequearse.
        let acumulador = if declarado.is_none() {
            Some(Vec::new())
        } else {
            None
        };
        let prev_infer = std::mem::replace(&mut self.inferring_return, acumulador);

        self.check_block_in_current_scope(body);

        let recogidos = std::mem::replace(&mut self.inferring_return, prev_infer);
        self.current_return = prev_return;
        self.closure_bases.pop();
        self.scopes.pop();

        let ret = match declarado {
            Some(t) => t,
            None => self.inferir_retorno(&recogidos.unwrap_or_default(), span),
        };

        // Igual que una función con nombre: si promete un valor, tiene que
        // darlo por todos los caminos.
        if !matches!(ret, Ty::Unit | Ty::Unknown) && !crate::stmt::block_terminates(body) {
            self.error(TypeError::new(
                "E_MISSING_RETURN",
                format!(
                    "este cierre debe devolver '{}' en todos los caminos",
                    ret.display()
                ),
                span,
            ));
        }

        Ty::Fn {
            params: param_tys,
            ret: Box::new(ret),
            location: self.current_location,
        }
    }

    /// Deduce el retorno de un cierre sin `-> T` a partir de los `return` que
    /// tiene. Sin ninguno es `Unit`. Si no coinciden, no hay nada que deducir:
    /// se pide escribirlo, que es más corto que la regla que haría falta para
    /// adivinarlo bien.
    fn inferir_retorno(&mut self, recogidos: &[Ty], span: Span) -> Ty {
        let mut inferido: Option<Ty> = None;
        for t in recogidos {
            // Un `Unknown` viene de un error ya reportado: no aporta ni estorba.
            if matches!(t, Ty::Unknown) {
                continue;
            }
            match inferido.as_ref() {
                None => {}
                Some(previo) if previo == t => continue,
                Some(previo) => {
                    let (a, b) = (previo.display(), t.display());
                    self.error(TypeError::new(
                        "E_CIERRE_RETORNO_AMBIGUO",
                        format!(
                            "no se puede deducir qué devuelve este cierre: unos 'return' dan \
                             '{a}' y otros '{b}'; escríbelo tú con '-> Tipo' tras los paréntesis"
                        ),
                        span,
                    ));
                    return Ty::Unknown;
                }
            }
            inferido = Some(t.clone());
        }
        match inferido {
            Some(t) => t,
            // Ni un `return` con valor deducible. Si los había pero todos eran
            // Unknown, el error que los produjo ya está reportado y no hay que
            // encadenar otro: `Unknown` lo absorbe.
            None if recogidos.is_empty() => Ty::Unit,
            None => Ty::Unknown,
        }
    }

    // ----------------------- utilidades de tipos -----------------------
}
