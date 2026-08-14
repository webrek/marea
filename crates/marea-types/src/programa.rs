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
    /// Qué almacenes de los exportados son PRESTADOS, y de qué tabla. Viaja con
    /// el almacén porque la regla viaja con él: importar `productos` no lo
    /// convierte en tuyo, y `save(productos, p)` en el módulo de al lado sigue
    /// siendo escribir en la tabla de otro.
    prestados: HashMap<String, String>,
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
    comprobar_tablas_del_programa(program, &mut errores);
    comprobar_nombres_unicos(program, &mut errores);

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
                prestados: ch.stores_prestados,
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
    // El almacén viaja con su marca de prestado: es parte de lo que promete.
    let mut stores: Vec<(String, Ty, Option<String>, Span)> = Vec::new();
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
                        stores.push((
                            n.name.clone(),
                            t.clone(),
                            dep.prestados.get(&n.name).cloned(),
                            n.span,
                        ));
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
    for (nombre, t, prestado, span) in stores {
        if ch.stores.contains_key(&nombre) {
            errores.push(colision(m.id, &nombre, span));
            continue;
        }
        if let Some(tabla) = prestado {
            ch.stores_prestados.insert(nombre.clone(), tabla);
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

/// Dos almacenes no pueden apuntar a la misma TABLA, aunque se llamen distinto
/// y vivan en archivos distintos.
///
/// `comprobar_almacenes_del_programa` mira los nombres, que es lo que decide la
/// tabla de un almacén PROPIO. Con `from "products"` el nombre deja de decirlo:
/// dos almacenes con nombres perfectamente distintos —y hasta con tipos
/// distintos— pueden estar mirando la misma tabla ajena. Serían dos vistas del
/// mismo sitio, y el día que una de las dos se quede atrás respecto al esquema
/// real nadie lo vería, porque nada compara los dos tipos entre sí.
///
/// Dentro de un módulo ya lo cubre `E_TABLA_EXTERNA_DUPLICADA` en la Fase A;
/// esto lo extiende al programa, que es justo donde deja de ser evidente: los
/// dos `store` están en archivos que nadie abre a la vez.
fn comprobar_tablas_del_programa(program: &Program, errores: &mut Vec<ProgramTypeError>) {
    // tabla normalizada -> (almacén, módulo, ¿prestado?).
    let mut visto: HashMap<String, (String, String, bool)> = HashMap::new();
    for m in &program.modulos {
        for item in &m.modulo.items {
            let Item::Store {
                name,
                name_span,
                tabla_externa,
                tabla_span,
                ..
            } = item
            else {
                continue;
            };
            // La tabla de un almacén propio se deriva del nombre en minúsculas;
            // la de uno prestado la escribió el programador.
            let destino = match tabla_externa {
                Some(t) => t.trim().to_lowercase(),
                None => name.to_lowercase(),
            };
            if destino.is_empty() {
                continue;
            }
            match visto.get(&destino) {
                // Dos almacenes PROPIOS que chocan es el mismo nombre, y eso ya
                // lo dice `comprobar_almacenes_del_programa` con un mensaje que
                // habla de nombres; aquí sólo hablamos de tablas prestadas.
                Some((previo, donde, previo_prestado))
                    if *previo_prestado || tabla_externa.is_some() =>
                {
                    errores.push(ProgramTypeError {
                        modulo: m.id,
                        error: TypeError::new(
                            "E_TABLA_EXTERNA_DUPLICADA",
                            format!(
                                "'{name}' apunta a la tabla '{destino}', que ya mapea '{previo}' \
                                 en '{donde}': serían dos vistas del mismo sitio con dos tipos, y \
                                 la deriva entre ellas no la vería nadie. Declara el almacén en \
                                 un solo módulo e impórtalo"
                            ),
                            tabla_span.unwrap_or(*name_span),
                        ),
                    });
                }
                _ => {
                    visto.insert(
                        destino,
                        (name.clone(), m.nombre.clone(), tabla_externa.is_some()),
                    );
                }
            }
        }
    }
}

/// Dos módulos no pueden DECLARAR el mismo nombre de nivel superior.
///
/// No contradice el aislamiento —que va de qué VE cada módulo—, sino que se
/// deriva de cómo se emite: la salida es un bundle plano, así que dos `fn fmt`
/// en archivos distintos acabarían siendo la misma declaración de JavaScript y
/// una se comería a la otra en silencio. Aquí se dice antes, y con los dos
/// archivos delante.
///
/// Importar un nombre no es declararlo, así que reutilizarlo no cuenta. La
/// alternativa —renombrar al emitir (`usuarios__fmt`)— quita la restricción,
/// pero ensucia la salida generada, que es un archivo que la gente commitea y
/// compara; queda para cuando esto estorbe de verdad.
fn comprobar_nombres_unicos(program: &Program, errores: &mut Vec<ProgramTypeError>) {
    let mut visto: HashMap<String, String> = HashMap::new();
    for m in &program.modulos {
        for item in &m.modulo.items {
            let (nombre, span) = match item {
                Item::Fn(f) => (f.name.clone(), f.span),
                Item::Type(t) => (t.name.clone(), t.span),
                Item::Let(l) => (l.name.clone(), l.span),
                // Los almacenes ya los cubre comprobar_almacenes_del_programa,
                // con un mensaje que explica lo de la tabla compartida.
                Item::Store { .. } => continue,
            };
            match visto.get(&nombre) {
                Some(donde) => errores.push(ProgramTypeError {
                    modulo: m.id,
                    error: TypeError::new(
                        "E_NOMBRE_DUPLICADO_EN_PROGRAMA",
                        format!(
                            "'{nombre}' ya se declara en '{donde}'. Los módulos se emiten en un \
                             solo bundle, así que los nombres de nivel superior son de todo el \
                             programa: renombra uno de los dos"
                        ),
                        span,
                    ),
                }),
                None => {
                    visto.insert(nombre, m.nombre.clone());
                }
            }
        }
    }
}
