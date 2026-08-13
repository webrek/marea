//! Verificar un PROGRAMA: varios módulos unidos por `import`.
//!
//! `check` mira un módulo suelto y le basta un espacio de nombres global. Con
//! imports eso ya no vale: cada módulo tiene el suyo, y de los demás ve
//! exactamente lo que importó. Ese "ni más" es la mitad del valor de tener
//! módulos, así que es lo que hay que hacer cumplir aquí.
//!
//! El esquema se apoya en el orden topológico que devuelve `resolve_program`:
//! cuando le toca a un módulo, sus dependencias ya están recolectadas, así que
//! sus firmas se pueden **inyectar** antes de mirar los cuerpos.
//!
//! Dos cosas son del PROGRAMA y no de un módulo, y se resuelven aparte:
//!   - La `@session`: hay como mucho una, y un handler puede exigir identidad en
//!     un módulo distinto de aquel donde se resuelve.
//!   - Los nombres de almacén: dos módulos con `store posts: T;` escribirían en
//!     la misma tabla sin enterarse el uno del otro.

use crate::{BoundaryCrossing, Checker, FnSig, Sesion, Ty, TypeError};
use marea_syntax::ast::{Item, Module, Type};
use marea_syntax::program::{ModuloResuelto, Program};
use marea_syntax::span::Span;
use std::collections::HashMap;

/// Un error de tipos junto al módulo donde salió. Con varios archivos un error
/// suelto no se puede ni imprimir: los spans son desplazamientos dentro de UNA
/// fuente.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramTypeError {
    /// `id` del módulo en `Program::modulos`.
    pub modulo: usize,
    pub error: TypeError,
}

/// Lo que un módulo ya recolectado puede prestar a quien lo importe.
#[derive(Default)]
struct Exportaciones {
    fns: HashMap<String, FnSig>,
    aliases: HashMap<String, Type>,
    stores: HashMap<String, Ty>,
    globals: HashMap<String, (Ty, bool)>,
}

/// Chequea un programa entero, en orden topológico.
pub fn check_program(program: &Program) -> Vec<ProgramTypeError> {
    check_program_with_boundaries(program).0
}

/// Variante que además devuelve los cruces de frontera de todo el programa.
pub fn check_program_with_boundaries(
    program: &Program,
) -> (Vec<ProgramTypeError>, Vec<BoundaryCrossing>) {
    let mut errores: Vec<ProgramTypeError> = Vec::new();
    let mut crossings: Vec<BoundaryCrossing> = Vec::new();
    let mut exportado: HashMap<usize, Exportaciones> = HashMap::new();

    let sesion = resolver_sesion(program, &mut errores);
    comprobar_almacenes_del_programa(program, &mut errores);

    for m in &program.modulos {
        let deps = deps_por_ruta(program, m);
        let mut ch = Checker::new();
        ch.session = sesion.clone();

        // Fase A local: alias y almacenes propios.
        ch.collect(&m.modulo);
        // Los alias importados entran ANTES de calcular firmas: `fn f(u: Usuario)`
        // con `Usuario` importado no se puede tipar sin ellos.
        inyectar(&mut ch, m, &deps, &exportado, &mut errores, Fase::Tipos);
        ch.collect_globals(&m.modulo);
        // Funciones, almacenes y globales después: `collect_globals` sólo
        // registra lo local, y así un choque se ve como lo que es.
        inyectar(&mut ch, m, &deps, &exportado, &mut errores, Fase::Valores);

        ch.check_politicas(&m.modulo);
        ch.check_bodies(&m.modulo);

        for e in ch.errors.drain(..) {
            errores.push(ProgramTypeError {
                modulo: m.id,
                error: e,
            });
        }
        crossings.append(&mut ch.crossings);

        // Lo RECOLECTADO, no sólo lo declarado: así un nombre importado y vuelto
        // a exportar sigue resolviendo igual aguas abajo.
        exportado.insert(
            m.id,
            Exportaciones {
                fns: ch.fns,
                aliases: ch.aliases,
                stores: ch.stores,
                globals: ch.globals,
            },
        );
    }
    (errores, crossings)
}

/// Ruta escrita en el `import` -> id del módulo. `ModuloResuelto::deps` viene
/// sin repetir, así que no se puede emparejar por posición con los imports; se
/// resuelve igual que el resolvedor, contra el directorio del que importa.
fn deps_por_ruta(program: &Program, m: &ModuloResuelto) -> HashMap<String, usize> {
    let base = m.ruta.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let mut mapa = HashMap::new();
    for imp in &m.modulo.imports {
        let destino = base.join(&imp.path);
        let Ok(canon) = destino.canonicalize() else {
            continue;
        };
        if let Some(dep) = program.modulos.iter().find(|x| x.ruta == canon) {
            mapa.insert(imp.path.clone(), dep.id);
        }
    }
    mapa
}

#[derive(Clone, Copy, PartialEq)]
enum Fase {
    Tipos,
    Valores,
}

/// Mete en el verificador los nombres que este módulo importó. Un nombre que
/// además se declara aquí es un choque: los dos acabarían siendo la misma
/// declaración de primer nivel en el bundle.
fn inyectar(
    ch: &mut Checker,
    m: &ModuloResuelto,
    deps: &HashMap<String, usize>,
    exportado: &HashMap<usize, Exportaciones>,
    errores: &mut Vec<ProgramTypeError>,
    fase: Fase,
) {
    // Se acumula antes de insertar para no tener `ch` prestado dos veces.
    let mut tipos: Vec<(String, Type, Span)> = Vec::new();
    let mut fns: Vec<(String, FnSig, Span)> = Vec::new();
    let mut stores: Vec<(String, Ty, Span)> = Vec::new();
    let mut globals: Vec<(String, (Ty, bool), Span)> = Vec::new();

    for imp in &m.modulo.imports {
        let Some(dep) = deps.get(&imp.path).and_then(|d| exportado.get(d)) else {
            continue;
        };
        for n in &imp.names {
            match fase {
                Fase::Tipos => {
                    if let Some(t) = dep.aliases.get(&n.name) {
                        tipos.push((n.name.clone(), t.clone(), n.span));
                    }
                }
                Fase::Valores => {
                    if let Some(sig) = dep.fns.get(&n.name) {
                        fns.push((n.name.clone(), sig.clone(), n.span));
                    }
                    if let Some(t) = dep.stores.get(&n.name) {
                        stores.push((n.name.clone(), t.clone(), n.span));
                    }
                    if let Some(g) = dep.globals.get(&n.name) {
                        globals.push((n.name.clone(), g.clone(), n.span));
                    }
                }
            }
        }
    }

    for (nombre, t, span) in tipos {
        if ch.aliases.contains_key(&nombre) {
            errores.push(colision(m.id, &nombre, span));
            continue;
        }
        ch.aliases.insert(nombre, t);
    }
    for (nombre, sig, span) in fns {
        if ch.fns.contains_key(&nombre) {
            errores.push(colision(m.id, &nombre, span));
            continue;
        }
        ch.fns.insert(nombre, sig);
    }
    for (nombre, t, span) in stores {
        if ch.stores.contains_key(&nombre) {
            errores.push(colision(m.id, &nombre, span));
            continue;
        }
        ch.stores.insert(nombre, t);
    }
    for (nombre, g, span) in globals {
        if ch.globals.contains_key(&nombre) {
            errores.push(colision(m.id, &nombre, span));
            continue;
        }
        ch.globals.insert(nombre, g);
    }
}

fn colision(modulo: usize, nombre: &str, span: Span) -> ProgramTypeError {
    ProgramTypeError {
        modulo,
        error: TypeError::new(
            "E_IMPORT_COLISIONA",
            format!(
                "'{nombre}' se importa y además se declara en este módulo; en el bundle serían \
                 la misma declaración"
            ),
            span,
        ),
    }
}

/// La `@session` del programa. Se busca en todos los módulos porque la política
/// de un handler puede vivir lejos de quien resuelve la identidad.
fn resolver_sesion(program: &Program, errores: &mut Vec<ProgramTypeError>) -> Option<Sesion> {
    let mut encontrada: Option<(String, Sesion)> = None;
    for m in &program.modulos {
        let mut ch = Checker::new();
        // La firma de la @session se valida contra los alias de SU módulo.
        ch.collect(&m.modulo);
        ch.errors.clear();
        ch.collect_session(&m.modulo);
        for e in ch.errors.drain(..) {
            errores.push(ProgramTypeError {
                modulo: m.id,
                error: e,
            });
        }
        let Some(s) = ch.session.take() else { continue };
        match &encontrada {
            None => encontrada = Some((m.nombre.clone(), s)),
            Some((donde, previa)) => {
                let span = span_de_la_session(&m.modulo);
                errores.push(ProgramTypeError {
                    modulo: m.id,
                    error: TypeError::new(
                        "E_SESSION_DUPLICADA",
                        format!(
                            "el programa ya resuelve la identidad en '{donde}' (con '{}'); se \
                             resuelve de una sola manera, aunque sea en otro archivo",
                            previa.fn_name
                        ),
                        span,
                    ),
                });
            }
        }
    }
    encontrada.map(|(_, s)| s)
}

fn span_de_la_session(modulo: &Module) -> Span {
    modulo
        .items
        .iter()
        .find_map(|it| match it {
            Item::Fn(f) if f.es_session => Some(f.span),
            _ => None,
        })
        .unwrap_or(Span { start: 0, end: 1 })
}

/// Dos módulos no pueden declarar el mismo nombre de almacén: cada `store` va a
/// su propia tabla, derivada del nombre, así que serían el mismo sitio con dos
/// esquemas. Dentro de un módulo ya lo cubre `E_DUPLICATE_STORE`; esto lo
/// extiende al programa, que es donde deja de ser evidente.
fn comprobar_almacenes_del_programa(program: &Program, errores: &mut Vec<ProgramTypeError>) {
    let mut visto: HashMap<String, String> = HashMap::new();
    for m in &program.modulos {
        for item in &m.modulo.items {
            let Item::Store {
                name, name_span, ..
            } = item
            else {
                continue;
            };
            match visto.get(name) {
                Some(donde) => errores.push(ProgramTypeError {
                    modulo: m.id,
                    error: TypeError::new(
                        "E_DUPLICATE_STORE",
                        format!(
                            "'{name}' ya es un almacén declarado en '{donde}': los dos irían a la \
                             misma tabla. Decláralo en un solo módulo e impórtalo"
                        ),
                        *name_span,
                    ),
                }),
                None => {
                    visto.insert(name.clone(), m.nombre.clone());
                }
            }
        }
    }
}
