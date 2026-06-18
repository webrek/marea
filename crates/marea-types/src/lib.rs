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

mod builtins;
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
}

/// Punto de entrada: chequea un módulo y acumula TODOS los errores.
/// Un `Vec` vacío significa que el módulo tipa.
pub fn check(module: &Module) -> Vec<TypeError> {
    let mut checker = Checker::new();
    checker.collect(module);
    checker.check_bodies(module);
    checker.errors
}

/// Variante de [`check`] que además devuelve los cruces de frontera detectados.
pub fn check_with_boundaries(module: &Module) -> (Vec<TypeError>, Vec<BoundaryCrossing>) {
    let mut checker = Checker::new();
    checker.collect(module);
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
    /// Pila de scopes léxicos de variables (Fase B). El bool es la mutabilidad
    /// (`true` si se declaró `mut`/`reactive`); se usa para rechazar reasignar
    /// un binding inmutable.
    scopes: Vec<HashMap<String, (Ty, bool)>>,
    /// Ubicación de la función que se está chequeando (Fase B).
    current_location: Option<Location>,
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
            scopes: Vec::new(),
            current_location: None,
            current_return: Ty::Unit,
            errors: Vec::new(),
            crossings: Vec::new(),
        }
    }

    fn error(&mut self, e: TypeError) {
        self.errors.push(e);
    }

    // ===================== FASE A: recolección global =====================

    fn collect(&mut self, module: &Module) {
        // Spans de la primera declaración de cada nombre, para notas de duplicado.
        let mut fn_spans: HashMap<String, Span> = HashMap::new();
        let mut type_spans: HashMap<String, Span> = HashMap::new();

        for item in &module.items {
            match item {
                Item::Fn(f) => {
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
            }
        }

        // Detección de alias cíclicos (`type A = B; type B = A`) ANTES de
        // registrar firmas: el registro llama a ty_from_syntax, que sin la marca
        // de ciclo recurriría infinitamente (stack overflow).
        self.detect_cyclic_types(&type_spans);

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
                self.fns.insert(
                    f.name.clone(),
                    FnSig {
                        params,
                        ret,
                        location: f.location,
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
                Item::Type(_) => {}
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

        self.check_block(&f.body);

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
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        self.scopes.pop();
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let(l) => {
                let value_ty = self.check_expr(&l.value);
                // Tipo destino, si se anotó.
                let bind_ty = if let Some(decl) = &l.ty {
                    self.validate_type_exists(decl);
                    let declared = self.ty_from_syntax(decl);
                    // `reactive` es laxo: no exige el subtipado estricto.
                    if !l.reactive && !self.is_subtype(&value_ty, &declared) {
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
                    .insert(l.name.clone(), (bind_ty, l.mutable || l.reactive));
            }
            Stmt::Return { value, span } => {
                let ret_ty = match value {
                    Some(e) => self.check_expr(e),
                    None => Ty::Unit,
                };
                let expected = self.current_return.clone();
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
            Expr::List { elements, .. } => self.check_list(elements),
            Expr::Index { object, index, .. } => self.check_index(object, index),
        }
    }

    fn resolve_ident(&mut self, name: &str, span: Span) -> Ty {
        // Variable en algún scope (del más interno al más externo).
        for scope in self.scopes.iter().rev() {
            if let Some((ty, _)) = scope.get(name) {
                return ty.clone();
            }
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
                // Igualdad sólo entre el mismo escalar.
                if !(lt.is_scalar() && lt == rt) {
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

    fn check_call(&mut self, callee: &Expr, args: &[Expr], span: Span) -> Ty {
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
            self.crossings.push(BoundaryCrossing {
                callee: callee_name.clone(),
                from,
                to,
                span,
            });
            for (i, pty) in params.iter().enumerate() {
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
                        covered.insert(name.clone());
                        // Narrowing: dentro de la rama, la variable escrutada es
                        // esta variante (estrechada a Named).
                        self.scopes.push(HashMap::new());
                        if let Some(sn) = &scrut_name {
                            let narrowed = self.narrow_variant(name);
                            self.scopes.last_mut().unwrap().insert(sn.clone(), (narrowed, false));
                        }
                        arm_types.push(self.check_expr(&arm.body));
                        self.scopes.pop();
                    } else {
                        // minúscula = binding catch-all. Captura el residual
                        // de la unión (las variantes aún no cubiertas).
                        has_catch_all = true;
                        let residual = self.residual_narrow(&variants, &covered, &scrut_ty);
                        self.scopes.push(HashMap::new());
                        self.scopes.last_mut().unwrap().insert(name.clone(), (residual, false));
                        arm_types.push(self.check_expr(&arm.body));
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
                    arm_types.push(self.check_expr(&arm.body));
                    self.scopes.pop();
                }
                Pattern::Int { .. } | Pattern::Bool { .. } | Pattern::Str { .. } => {
                    // Patrón literal: chequea la rama; no aporta a la exhaustividad nominal.
                    arm_types.push(self.check_expr(&arm.body));
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
        arm_types
            .into_iter()
            .reduce(|acc, t| {
                if matches!(acc, Ty::Unknown) {
                    t
                } else if matches!(t, Ty::Unknown) || acc == t {
                    acc
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

    fn check_list(&mut self, elements: &[Expr]) -> Ty {
        let tys: Vec<Ty> = elements.iter().map(|e| self.check_expr(e)).collect();
        // Elemento = tipo del primero si todos coinciden (ignorando Unknown);
        // lista vacía o heterogénea → elemento desconocido.
        let elem = match tys.first() {
            None => Ty::Unknown,
            Some(first) => {
                if tys
                    .iter()
                    .all(|t| matches!(t, Ty::Unknown) || t == first)
                {
                    first.clone()
                } else {
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
        match t {
            Type::Name { name, args, .. } if name == "List" => {
                // `List` o `List<T>`: el elemento es el argumento, o desconocido.
                let elem = match args.first() {
                    Some(a) => self.ty_from_syntax(a),
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
                    return self.ty_from_syntax(alias);
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
                    .map(|f| (f.name.clone(), self.ty_from_syntax(&f.ty)))
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
        if matches!(sub, Ty::Unknown) || matches!(sup, Ty::Unknown) {
            return true;
        }
        match (sub, sup) {
            (a, b) if a == b => true,
            // Una variante nominal individual es subtipo de la unión que la contiene.
            (Ty::Named(n), Ty::Union(vs)) => vs.contains(n),
            // Una unión es subtipo de otra si todas sus variantes están contenidas.
            (Ty::Union(a), Ty::Union(b)) => a.iter().all(|v| b.contains(v)),
            // Listas: covariantes en el elemento. `List<?>` (lista vacía o de
            // elemento desconocido) es subtipo de cualquier `List<T>` porque el
            // elemento Unknown es subtipo de todo.
            (Ty::List(ea), Ty::List(eb)) => self.is_subtype(ea, eb),
            // Registros estructurales: ancho + profundidad.
            (Ty::Record(sa), Ty::Record(sb)) => sb.iter().all(|(n, tb)| {
                sa.iter()
                    .find(|(na, _)| na == n)
                    .map(|(_, ta)| self.is_subtype(ta, tb))
                    .unwrap_or(false)
            }),
            // Un nombre de tipo que resuelve a un registro estructural es
            // intercambiable con su forma estructural: así un literal de registro
            // (tipado nominalmente como su alias) es subtipo del mismo `type` que
            // se declaró como `{ ... }` (p.ej. `fn origen() -> Punto` que devuelve
            // `Punto { x, y }`), y viceversa.
            (Ty::Named(n), Ty::Record(_)) => match self.resolve_named_to_record(n) {
                Some(Some(fields)) => self.is_subtype(&Ty::Record(fields), sup),
                // `Record` abierto: acepta cualquier registro estructural.
                Some(None) => true,
                None => false,
            },
            (Ty::Record(_), Ty::Named(n)) => match self.resolve_named_to_record(n) {
                Some(Some(fields)) => self.is_subtype(sub, &Ty::Record(fields)),
                Some(None) => true,
                None => false,
            },
            _ => false,
        }
    }
}

/// Nombre textual de un callee (`getUser`, `db.users.find`).
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
        Ty::Int | Ty::Float | Ty::Bool | Ty::String | Ty::Unit | Ty::Unknown => true,
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
