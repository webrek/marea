//! Recorrido de funciones, bloques y sentencias: la pila de scopes léxicos
//! y el análisis de terminación (`block_terminates`), que es lo que
//! decide si a una función con retorno declarado le falta el `return`.

use super::*;

impl Checker {
    pub(crate) fn check_bodies(&mut self, module: &Module) {
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

    pub(crate) fn check_fn(&mut self, f: &FnDecl) {
        self.scopes = vec![HashMap::new()];
        // Una página corre en el servidor aunque no lo escriba: `ubicacion_efectiva`.
        self.current_location = ubicacion_efectiva(f);
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

        // La identidad que exige la política entra en el mismo scope que los
        // parámetros, pero DESPUÉS de ellos: así un choque de nombres se detecta
        // aquí y no sombrea en silencio a un parámetro.
        self.bind_identidad(f);

        // Valida el tipo de retorno declarado.
        if let Some(rt) = &f.return_type {
            self.validate_type_exists(rt);
        }

        // Un parámetro `Html` de una función remota lo rellena quien mande el
        // JSON: la confianza no cruza la red. Se comprueba en la declaración y
        // no solo en la llamada, porque el atacante no usa el cliente generado.
        // En una página vale igual, y de sobra: sus parámetros salen de la URL.
        if matches!(
            self.current_location,
            Some(Location::Server) | Some(Location::Edge)
        ) {
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

    /// Liga el nombre de `@server(u: Usuario)` en el scope de la función.
    ///
    /// La identidad NO es un parámetro: no viaja en la llamada, la inyecta el
    /// runtime tras resolver el token. Pero dentro del cuerpo tiene que ser una
    /// variable como cualquier otra, o la política sería una etiqueta decorativa
    /// que no deja escribir `u.nombre`. Es inmutable: quién eres no se reasigna.
    ///
    /// `@server(Usuario)` sin nombre no liga nada —exige identidad sin usarla— y
    /// `@server(Public)` no tiene ninguna que ligar.
    fn bind_identidad(&mut self, f: &FnDecl) {
        let Some(politica) = &f.politica else { return };
        let span = politica.span();
        if matches!(politica, Type::Name { name, .. } if name == "Public") {
            if let Some(nombre) = &f.identidad_bind {
                self.error(TypeError::new(
                    "E_IDENTIDAD_EN_PUBLIC",
                    format!(
                        "'Public' es la decisión de no exigir identidad, así que no hay \
                         ninguna que ligar a '{nombre}'; escribe '@server(Public)' a secas"
                    ),
                    span,
                ));
            }
            return;
        }
        // El generador inyecta la identidad como PRIMER parámetro de la función
        // emitida: con el nombre que dé la política, o con uno reservado si no lo
        // da. Un parámetro que se llame igual produciría dos parámetros con el
        // mismo nombre, es decir un archivo que ni siquiera carga.
        let nombre = match &f.identidad_bind {
            Some(n) => n.clone(),
            None => "__identidad".to_string(),
        };
        if f.params.iter().any(|p| p.name == nombre) {
            self.error(TypeError::new(
                "E_IDENTIDAD_CHOCA_PARAM",
                format!(
                    "'{nombre}' ya es un parámetro de '{}': la identidad no viaja en la \
                     llamada —la resuelve el servidor desde el token—, así que no puede \
                     compartir nombre con algo que el cliente sí manda",
                    f.name
                ),
                span,
            ));
            return;
        }
        // `@server(Usuario)` exige identidad sin nombrarla: no hay nada que ligar.
        if f.identidad_bind.is_none() {
            return;
        }
        // El nombre también sombrearía a un builtin dentro del cuerpo:
        // `print(...)` dejaría de ser el builtin y el archivo generado moriría.
        if builtins::lookup(&nombre).is_some() || es_interno_del_runtime(&nombre) {
            self.error(TypeError::new(
                "E_REDEFINE_BUILTIN",
                format!("no se puede llamar '{nombre}' a la identidad: es un builtin"),
                span,
            ));
            return;
        }
        let ty = self.ty_from_syntax(politica);
        let scope = self.scopes.last_mut().unwrap();
        scope.insert(nombre, (ty, false));
    }

    pub(crate) fn check_block(&mut self, block: &Block) {
        self.scopes.push(HashMap::new());
        self.check_block_in_current_scope(block);
        self.scopes.pop();
    }

    /// Chequea las sentencias sin abrir un scope propio (el llamador ya abrió
    /// el suyo). Lo usa el cuerpo de una función para compartir el scope con
    /// sus parámetros.
    pub(crate) fn check_block_in_current_scope(&mut self, block: &Block) {
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
    }

    pub(crate) fn check_stmt(&mut self, stmt: &Stmt) {
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
                    let value_ty =
                        if matches!(declared, Ty::Html) && matches!(l.value, Expr::Str { .. }) {
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
                // Dentro de un cierre que no escribió `-> T` no hay nada contra
                // qué comparar: este `return` es parte de lo que DEFINE el tipo,
                // así que se apunta y ya lo juntará `inferir_retorno`.
                if let Some(recogidos) = &mut self.inferring_return {
                    recogidos.push(ret_ty);
                    return;
                }
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
            Stmt::Assign {
                name,
                name_span,
                value,
                ..
            } => {
                let value_ty = self.check_expr(value);
                let mut target: Option<(Ty, bool)> = None;
                if let Some((idx, ty, mutable)) = self.buscar_en_scopes(name) {
                    // Asignar a algo de fuera del cierre es la trampa de la
                    // captura por valor en su forma más nítida: escribiría en la
                    // copia. `check_captura` la explica.
                    self.check_captura(name, idx, mutable, *name_span);
                    target = Some((ty, mutable));
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
            Stmt::For {
                var,
                var_span,
                index,
                index_span,
                iter,
                body,
                ..
            } => {
                let it = self.check_expr(iter);
                let elem = match &it {
                    Ty::List(e) => (**e).clone(),
                    Ty::Unknown => Ty::Unknown,
                    otro => {
                        self.error(TypeError::new(
                            "E_FOR_NO_LISTA",
                            format!("'for' recorre una lista, no '{}'", otro.display()),
                            iter.span(),
                        ));
                        Ty::Unknown
                    }
                };
                // El elemento y el índice viven en un scope propio y son
                // INMUTABLES: reasignarlos no cambiaría la lista, así que
                // permitirlo solo crearía una expectativa falsa.
                self.scopes.push(HashMap::new());
                if builtins::lookup(var).is_some() {
                    self.error(TypeError::new(
                        "E_REDEFINE_BUILTIN",
                        format!("no se puede llamar '{var}' al elemento: es un builtin"),
                        *var_span,
                    ));
                }
                self.scopes
                    .last_mut()
                    .unwrap()
                    .insert(var.clone(), (elem, false));
                if let Some(i) = index {
                    if builtins::lookup(i).is_some() {
                        self.error(TypeError::new(
                            "E_REDEFINE_BUILTIN",
                            format!("no se puede llamar '{i}' al índice: es un builtin"),
                            index_span.unwrap_or(*var_span),
                        ));
                    }
                    self.scopes
                        .last_mut()
                        .unwrap()
                        .insert(i.clone(), (Ty::Int, false));
                }
                self.check_block_in_current_scope(body);
                self.scopes.pop();
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
}

/// ¿El bloque termina garantizado (return en todo camino)?
pub(crate) fn block_terminates(block: &Block) -> bool {
    block.stmts.iter().any(stmt_terminates)
}

pub(crate) fn stmt_terminates(stmt: &Stmt) -> bool {
    match stmt {
        Stmt::Return { .. } => true,
        Stmt::Expr(e) => expr_terminates(e),
        // Un `for` no garantiza retorno: la lista puede estar vacía y el
        // cuerpo no ejecutarse ni una vez.
        Stmt::Let(_) | Stmt::Assign { .. } | Stmt::Effect { .. } | Stmt::For { .. } => false,
    }
}

pub(crate) fn expr_terminates(expr: &Expr) -> bool {
    match expr {
        Expr::If {
            then_branch,
            else_branch,
            ..
        } => {
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
