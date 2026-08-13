//! Relación de subtipado y traducción de tipos sintácticos a `Ty`. El
//! subtipado es coinductivo sobre registros mutuamente recursivos
//! (regla de Amber), que sin guarda desbordaba la pila.

use super::*;

impl Checker {
    /// Resuelve un nombre a sus campos de registro.
    /// - `Some(Some(fields))`: registro estructural con esos campos.
    /// - `Some(None)`: tipo abierto (`Record` builtin) — cualquier campo vale.
    /// - `None`: no es un tipo registro.
    pub(crate) fn resolve_named_to_record(&self, name: &str) -> Option<Option<Vec<(String, Ty)>>> {
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

    /// Traduce un `Type` sintáctico al `Ty` interno (sin validar existencia).
    pub(crate) fn ty_from_syntax(&self, t: &Type) -> Ty {
        self.ty_from_syntax_guarded(t, &mut std::collections::HashSet::new())
    }

    /// Igual que `ty_from_syntax`, pero con un conjunto de alias en expansión para
    /// cortar los tipos registro estructuralmente recursivos (p.ej.
    /// `type Nodo = { siguiente: Nodo }`, una lista enlazada perfectamente válida).
    /// Sin esta guarda, expandir el campo `siguiente` vuelve a `Nodo` y recurre
    /// hasta desbordar la pila. Nótese que estos tipos NO son un error —los ciclos
    /// de alias *transparentes* (`type A = A`) sí, y esos ya se marcan en `cyclic`.
    pub(crate) fn ty_from_syntax_guarded(
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
                    .map(|f| {
                        (
                            f.name.clone(),
                            self.ty_from_syntax_guarded(&f.ty, expanding),
                        )
                    })
                    .collect(),
            ),
        }
    }

    /// Valida que un `Type` referencie sólo primitivos, alias o builtins.
    pub(crate) fn validate_type_exists(&mut self, t: &Type) {
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
    pub(crate) fn is_subtype(&self, sub: &Ty, sup: &Ty) -> bool {
        self.is_subtype_rec(sub, sup, &mut std::collections::HashSet::new())
    }

    /// Clave del par que se está desplegando, para la regla de Amber. Se usa el
    /// par COMPLETO (sub, sup) y no solo el nombre del alias: con un conjunto de
    /// nombres, reencontrar `A` aceptaba contra cualquier cosa, así que el mismo
    /// par se rechazaba a profundidad 3 y se aceptaba a profundidad 4.
    pub(crate) fn par_clave(sub: &Ty, sup: &Ty) -> (String, String) {
        (sub.display(), sup.display())
    }

    /// Igual que `is_subtype`, pero con un conjunto de nombres de alias ya
    /// desplegados para dar subtipado *coinductivo* sobre tipos registro
    /// mutuamente recursivos (`type A = { x: B }; type B = { y: A }`). Sin la
    /// guarda, `is_subtype` alterna desplegando `A`→`B`→`A`… hasta desbordar la
    /// pila. Al reencontrar un nombre en despliegue lo aceptamos: es la regla
    /// estándar de subtipado equirecursivo (asumir la meta y verla cumplida).
    pub(crate) fn is_subtype_rec(
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
                    Some(Some(fields)) => self.is_subtype_rec(&Ty::Record(fields), sup, unfolding),
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
                    Some(Some(fields)) => self.is_subtype_rec(sub, &Ty::Record(fields), unfolding),
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
