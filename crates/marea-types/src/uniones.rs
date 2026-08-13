//! La regla protagonista: un valor `A | B` es opaco y sólo se consume con
//! un `match` exhaustivo, que estrecha (narrowing) cada rama a su
//! variante. El catch-all se estrecha al residual de la unión.

use super::*;

impl Checker {
    pub(crate) fn check_match(
        &mut self,
        scrutinee: &Expr,
        arms: &[marea_syntax::ast::MatchArm],
        span: Span,
    ) -> Ty {
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
                            self.scopes
                                .last_mut()
                                .unwrap()
                                .insert(sn.clone(), (narrowed, false));
                        }
                        arm_types.push(self.check_expr_html(&arm.body));
                        self.scopes.pop();
                    } else {
                        // minúscula = binding catch-all. Captura el residual
                        // de la unión (las variantes aún no cubiertas).
                        has_catch_all = true;
                        let residual = self.residual_narrow(&variants, &covered, &scrut_ty);
                        self.scopes.push(HashMap::new());
                        self.scopes
                            .last_mut()
                            .unwrap()
                            .insert(name.clone(), (residual, false));
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
                        self.scopes
                            .last_mut()
                            .unwrap()
                            .insert(sn.clone(), (residual, false));
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
                // `is_subtype` es reflexiva, así que esto ya cubre `acc == t`.
                } else if matches!(t, Ty::Unknown) || self.is_subtype(&t, &acc) {
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
    pub(crate) fn residual_narrow(
        &self,
        variants: &[String],
        covered: &std::collections::HashSet<String>,
        scrut_ty: &Ty,
    ) -> Ty {
        if variants.is_empty() {
            return scrut_ty.clone();
        }
        let residual: Vec<String> = variants
            .iter()
            .filter(|v| !covered.contains(*v))
            .cloned()
            .collect();
        match residual.len() {
            0 => scrut_ty.clone(),
            1 => self.narrow_variant(&residual[0]),
            _ => Ty::Union(residual),
        }
    }

    /// Estrecha una variante nominal a un tipo concreto para el narrowing.
    /// Si la variante es un alias a un registro, devuelve el registro; si no,
    /// `Named` (que es opaco salvo si resuelve a Record vía acceso a campo).
    pub(crate) fn narrow_variant(&self, name: &str) -> Ty {
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
}
