//! El PROGRAMA que hay detrás del documento abierto: el grafo de `import`
//! resuelto, verificado entero y con sus diagnósticos repartidos por archivo.
//!
//! Un programa de Marea dejó de ser un archivo. Mientras el LSP siguió llamando
//! a `check` sobre el documento suelto, cualquiera que escribiera `import` veía
//! el editor rojo: todos los nombres importados salían sin resolver. Aquí se
//! resuelve el grafo y se llama a `marea_types::check_program`, que verifica los
//! módulos en orden topológico inyectando en cada uno lo que importó, y devuelve
//! cada error con el módulo al que pertenece.
//!
//! ## Por qué no se usa `marea_syntax::resolve_program` tal cual
//!
//! Aquel resuelve **el disco**, y el editor tiene delante texto que aún no se ha
//! guardado: usarlo dejaría los diagnósticos un `Ctrl+S` por detrás de lo que se
//! está escribiendo, que es justo cuando hacen falta. Además aborta en el primer
//! problema de resolución y devuelve un `ProgramError` con el mensaje YA
//! renderizado (archivo, línea y cursor `^` en una cadena), sin span ni módulo:
//! sirve para imprimir en una terminal, no para subrayar en un editor.
//!
//! Así que el recorrido se rehace aquí con dos diferencias que son justo lo que
//! el editor necesita: lee de los buffers abiertos antes que del disco, y anota
//! cada fallo de resolución como un diagnóstico con su span en el archivo que
//! escribió el `import`, sin abandonar el resto del grafo. Las reglas —rutas
//! relativas, nada fuera del árbol, sin ciclos, sólo lo que el destino exporta—
//! son las mismas, porque el editor no puede decir que sí a algo que luego
//! `marea build` rechace.
//!
//! Lo que haría falta en `marea-syntax` para no duplicar nada: que
//! `resolve_program` aceptara una fuente de texto sustituible (los buffers) y
//! devolviera los errores como datos —clase, ruta y span— en vez de una cadena
//! ya pintada, acumulándolos en vez de parar en el primero.
//!
//! ## El coste
//!
//! Resolver el grafo en cada pulsación sería leer y parsear N archivos por
//! tecla. Hay dos cachés:
//!   - [`Cache::parseos`], por ruta: reusa el parseo mientras la **huella** del
//!     archivo no cambie (la versión del buffer si está abierto, la fecha y el
//!     tamaño si está en disco). Al teclear en un archivo de un programa de
//!     cinco, se reparsea uno.
//!   - [`Cache::resultados`], por documento: si NINGUNA huella cambió, el
//!     resultado entero vale y no se recorre nada. Es el caso de las peticiones
//!     de hover o completado, que llegan a ráfagas sin que el texto cambie.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use marea_syntax::ast::{Item, Module};
use marea_syntax::parse_recovering;
use marea_syntax::program::{ModuloResuelto, Program};
use marea_syntax::span::Span;
use marea_types::{check, check_program};

use crate::analysis::{diag_de_sintaxis, diag_de_tipos, NeutralDiag, Severity};

/// Profundidad máxima de la cadena de imports. La misma que la del resolvedor
/// del compilador: el recorrido es recursivo y un árbol patológico tiene que
/// salir por un error ordinario, no por un desbordamiento de pila.
const MAX_PROFUNDIDAD: usize = 64;

// ===================== Buffers abiertos =====================

/// Los buffers que el editor tiene abiertos, indexados por ruta CANÓNICA.
///
/// Es la razón de ser de este módulo: para el compilador un módulo es un archivo
/// del disco, y para el editor es lo que hay en pantalla.
#[derive(Debug, Default)]
pub struct Abiertos<'a> {
    por_ruta: HashMap<PathBuf, (&'a str, i32)>,
}

impl<'a> Abiertos<'a> {
    pub fn new() -> Self {
        Abiertos {
            por_ruta: HashMap::new(),
        }
    }

    /// Registra el buffer de `ruta` (canónica) con su texto y su versión.
    pub fn insertar(&mut self, ruta: PathBuf, texto: &'a str, version: i32) {
        self.por_ruta.insert(ruta, (texto, version));
    }

    fn get(&self, ruta: &Path) -> Option<(&'a str, i32)> {
        self.por_ruta.get(ruta).copied()
    }
}

// ===================== Resultado =====================

/// Un archivo del programa, ya parseado y con sus diagnósticos.
#[derive(Debug, Clone)]
pub struct Archivo {
    /// Ruta canónica, o `None` si el documento no vive en el disco (`untitled:`
    /// o un archivo que aún no existe).
    pub ruta: Option<PathBuf>,
    pub fuente: String,
    pub modulo: Module,
    pub diags: Vec<NeutralDiag>,
    /// Para cada `import` que resolvió: la ruta tal como se escribió y el índice
    /// de su archivo en [`Salida::archivos`]. Guardarlo evita volver a tocar el
    /// disco cada vez que el editor pregunta por un nombre importado.
    pub imports: Vec<(String, usize)>,
}

/// El resultado de analizar un documento: sus archivos en orden topológico
/// —las dependencias antes—, con el propio documento SIEMPRE el último, que es
/// donde lo deja el recorrido en profundidad porque nada del programa lo importa.
#[derive(Debug, Clone)]
pub struct Salida {
    pub archivos: Vec<Archivo>,
}

impl Salida {
    /// El archivo de entrada: el documento que el editor tiene abierto.
    pub fn entrada(&self) -> &Archivo {
        self.archivos
            .last()
            .expect("una Salida siempre trae al menos el documento de entrada")
    }

    /// El archivo al que apunta un `import` de `desde`.
    pub fn destino(&self, desde: &Archivo, path: &str) -> Option<&Archivo> {
        let (_, id) = desde.imports.iter().find(|(p, _)| p == path)?;
        self.archivos.get(*id)
    }

    /// Dónde se declara `nombre` si `desde` lo importa: el archivo destino y su
    /// item. Es lo que convierte un `import` en un salto real del editor.
    pub fn declaracion_importada(
        &self,
        desde: &Archivo,
        nombre: &str,
    ) -> Option<(&Archivo, &Item)> {
        for imp in &desde.modulo.imports {
            if !imp.names.iter().any(|n| n.name == nombre) {
                continue;
            }
            let Some(destino) = self.destino(desde, &imp.path) else {
                continue;
            };
            if let Some(item) = destino.modulo.items.iter().find(|i| i.name() == nombre) {
                return Some((destino, item));
            }
        }
        None
    }

    /// Todo lo que `desde` importa y de verdad existe, en el orden en que se
    /// escribieron los `import`. Lo consume el completado.
    pub fn importados<'s>(&'s self, desde: &'s Archivo) -> Vec<(&'s str, &'s Archivo, &'s Item)> {
        let mut out = Vec::new();
        for imp in &desde.modulo.imports {
            let Some(destino) = self.destino(desde, &imp.path) else {
                continue;
            };
            for n in &imp.names {
                if let Some(item) = destino.modulo.items.iter().find(|i| i.name() == n.name) {
                    out.push((n.name.as_str(), destino, item));
                }
            }
        }
        out
    }
}

// ===================== Caché =====================

/// Qué hace que un archivo esté "igual que la última vez".
#[derive(Debug, Clone, PartialEq, Eq)]
enum Huella {
    /// Un buffer abierto: basta su versión, que el editor sube en cada cambio.
    Buffer(i32),
    /// Un archivo del disco: fecha de modificación y tamaño.
    Disco(Option<SystemTime>, u64),
    /// Ni buffer ni archivo legible. Se registra igual: que aparezca es un
    /// cambio, y es exactamente lo que pasa al crear el módulo que faltaba.
    Ausente,
}

fn huella(ruta: &Path, abiertos: &Abiertos<'_>) -> Huella {
    if let Some((_, version)) = abiertos.get(ruta) {
        return Huella::Buffer(version);
    }
    match std::fs::metadata(ruta) {
        Ok(md) => Huella::Disco(md.modified().ok(), md.len()),
        Err(_) => Huella::Ausente,
    }
}

#[derive(Debug, Clone)]
struct Parseado {
    huella: Huella,
    fuente: String,
    modulo: Module,
    sintaxis: Vec<NeutralDiag>,
}

struct Resuelto {
    /// Versión del buffer de entrada cuando se calculó.
    version: i32,
    /// Cada archivo que se miró, incluidos los que no se pudieron leer.
    huellas: Vec<(PathBuf, Huella)>,
    salida: Salida,
}

/// Caché de parseos y de resultados. Vive en el bucle del servidor.
#[derive(Default)]
pub struct Cache {
    /// Parseos por ruta canónica.
    parseos: HashMap<PathBuf, Parseado>,
    /// Resultados por documento, con el URI del documento como clave.
    resultados: HashMap<String, Resuelto>,
}

impl Cache {
    pub fn new() -> Self {
        Cache::default()
    }

    /// Analiza el documento `clave` (su URI), que vive en `ruta` si vive en
    /// algún sitio, con `texto` y `version` como los tiene el editor.
    ///
    /// Devuelve el resultado cacheado si nada de lo que lo compone cambió.
    pub fn analizar(
        &mut self,
        clave: &str,
        ruta: Option<&Path>,
        texto: &str,
        version: i32,
        abiertos: &Abiertos<'_>,
    ) -> &Salida {
        if !self.vigente(clave, version, abiertos) {
            let resuelto = recomputar(&mut self.parseos, abiertos, ruta, texto, version);
            self.resultados.insert(clave.to_string(), resuelto);
        }
        &self.resultados[clave].salida
    }

    /// Olvida el resultado de un documento (al cerrarlo). Los parseos por ruta
    /// se conservan: el archivo sigue en el disco y otro documento del mismo
    /// programa puede seguir necesitándolo.
    pub fn olvidar(&mut self, clave: &str) {
        self.resultados.remove(clave);
    }

    fn vigente(&self, clave: &str, version: i32, abiertos: &Abiertos<'_>) -> bool {
        let Some(r) = self.resultados.get(clave) else {
            return false;
        };
        r.version == version && r.huellas.iter().all(|(p, h)| huella(p, abiertos) == *h)
    }
}

fn recomputar(
    parseos: &mut HashMap<PathBuf, Parseado>,
    abiertos: &Abiertos<'_>,
    ruta: Option<&Path>,
    texto: &str,
    version: i32,
) -> Resuelto {
    if let Some(entrada) = ruta {
        if let Some(r) = recomputar_programa(parseos, abiertos, entrada, version) {
            return r;
        }
    }
    recomputar_suelto(texto, version)
}

/// El documento no vive en el disco (o no se pudo leer): se analiza el buffer
/// tal cual, exactamente como antes de que existieran los módulos.
fn recomputar_suelto(texto: &str, version: i32) -> Resuelto {
    let analisis = crate::analysis::analyze(texto);
    let modulo = analisis.module.unwrap_or_else(|| Module {
        imports: Vec::new(),
        items: Vec::new(),
    });
    let archivo = Archivo {
        ruta: None,
        fuente: texto.to_string(),
        modulo,
        diags: analisis.diagnostics,
        imports: Vec::new(),
    };
    Resuelto {
        version,
        huellas: Vec::new(),
        salida: Salida {
            archivos: vec![archivo],
        },
    }
}

fn recomputar_programa(
    parseos: &mut HashMap<PathBuf, Parseado>,
    abiertos: &Abiertos<'_>,
    entrada: &Path,
    version: i32,
) -> Option<Resuelto> {
    // La raíz es el directorio del archivo abierto, como en el compilador: nada
    // del programa puede quedar fuera de ella.
    let raiz = entrada.parent().map(Path::to_path_buf)?;
    let mut paseo = Paseo {
        parseos,
        abiertos,
        raiz,
        por_ruta: HashMap::new(),
        pila: Vec::new(),
        archivos: Vec::new(),
        huellas: Vec::new(),
    };
    // Si ni el archivo de entrada se puede leer, no hay programa que enseñar:
    // que decida el buffer.
    visitar(entrada.to_path_buf(), 0, &mut paseo).ok()?;
    let Paseo {
        raiz,
        archivos,
        huellas,
        ..
    } = paseo;
    Some(Resuelto {
        version,
        huellas,
        salida: terminar(raiz, archivos),
    })
}

// ===================== El recorrido =====================

/// Un archivo mientras se recorre el grafo, antes de saber si hay que chequear
/// tipos: sus diagnósticos todavía van en dos montones porque el de resolución
/// es el que decide si el de tipos llega a existir.
struct EnCurso {
    ruta: PathBuf,
    fuente: String,
    modulo: Module,
    sintaxis: Vec<NeutralDiag>,
    resolucion: Vec<NeutralDiag>,
    /// Sin repetir y todos menores que el propio índice: eso es el orden
    /// topológico, y es lo que `check_program` espera.
    deps: Vec<usize>,
    imports: Vec<(String, usize)>,
}

struct Paseo<'p, 'o, 'b> {
    parseos: &'p mut HashMap<PathBuf, Parseado>,
    abiertos: &'o Abiertos<'b>,
    raiz: PathBuf,
    /// Ruta canónica → índice, sólo de módulos YA terminados. Que los pendientes
    /// no estén aquí es lo que distingue "ya visto" de "ciclo".
    por_ruta: HashMap<PathBuf, usize>,
    /// El camino de módulos abiertos, de la entrada hasta el actual.
    pila: Vec<PathBuf>,
    archivos: Vec<EnCurso>,
    huellas: Vec<(PathBuf, Huella)>,
}

/// Un fallo de resolución: código para el editor y mensaje para la persona. El
/// span lo pone quien lo recibe, porque el sitio donde duele es el `import` que
/// lo provocó, no el archivo que no se pudo leer.
type Fallo = (&'static str, String);

impl Paseo<'_, '_, '_> {
    /// Lee y parsea `ruta`, reusando el parseo cacheado si el archivo no cambió.
    /// Registra su huella pase lo que pase: hasta un archivo ausente forma parte
    /// de lo que hay que vigilar.
    fn leer(&mut self, ruta: &Path) -> Option<Parseado> {
        let h = huella(ruta, self.abiertos);
        self.huellas.push((ruta.to_path_buf(), h.clone()));
        if let Some(p) = self.parseos.get(ruta) {
            if p.huella == h {
                return Some(p.clone());
            }
        }
        let fuente = match self.abiertos.get(ruta) {
            Some((texto, _)) => texto.to_string(),
            None => std::fs::read_to_string(ruta).ok()?,
        };
        let (modulo, errores) = parse_recovering(&fuente);
        let parseado = Parseado {
            huella: h,
            fuente,
            modulo,
            sintaxis: errores.into_iter().map(diag_de_sintaxis).collect(),
        };
        self.parseos.insert(ruta.to_path_buf(), parseado.clone());
        Some(parseado)
    }

    /// Convierte el texto de un `import` en la ruta canónica del archivo que
    /// nombra, con las mismas dos reglas que el compilador: la ruta se escribe
    /// con `./` o `../`, y no puede salirse del directorio del archivo abierto.
    fn resolver_ruta(&mut self, desde: &Path, escrita: &str) -> Result<PathBuf, Fallo> {
        if !(escrita.starts_with("./") || escrita.starts_with("../")) {
            let pista = if Path::new(escrita).is_absolute() {
                "las rutas absolutas no son portables entre máquinas"
            } else {
                "la forma sin './' queda reservada para los paquetes, que aún no existen"
            };
            return Err((
                "E_MODULO_RUTA",
                format!("la ruta '{escrita}' debe empezar por './' o '../': {pista}"),
            ));
        }
        let base = desde.parent().unwrap_or(self.raiz.as_path());
        let candidata = base.join(escrita);
        // Canonicalizar confirma que existe y le da una identidad única,
        // resolviendo `..` y enlaces simbólicos de un golpe.
        let Ok(ruta) = candidata.canonicalize() else {
            // Que hoy no exista es parte del estado vigilado: en cuanto se cree,
            // la huella cambia y el resultado se recalcula solo.
            self.huellas.push((candidata, Huella::Ausente));
            return Err((
                "E_MODULO_NO_ENCONTRADO",
                format!("no se encontró el módulo '{escrita}'"),
            ));
        };
        if !ruta.starts_with(&self.raiz) {
            return Err((
                "E_MODULO_FUERA",
                format!(
                    "el módulo '{escrita}' queda fuera del programa: resuelve a '{}', \
                     que no está bajo '{}'",
                    ruta.display(),
                    self.raiz.display()
                ),
            ));
        }
        Ok(ruta)
    }
}

/// Lee, parsea y resuelve `ruta` y todo lo que importe. Devuelve su índice en
/// `archivos`. Los hijos se registran antes que el padre: de ahí sale el orden
/// topológico, sin ordenar nada después.
fn visitar(ruta: PathBuf, profundidad: usize, st: &mut Paseo<'_, '_, '_>) -> Result<usize, Fallo> {
    if let Some(&id) = st.por_ruta.get(&ruta) {
        return Ok(id); // ya resuelto por otro import: no se vuelve a parsear
    }
    if let Some(desde) = st.pila.iter().position(|p| *p == ruta) {
        let raiz = st.raiz.clone();
        let mut cadena: Vec<String> = st.pila[desde..].iter().map(|p| mostrar(&raiz, p)).collect();
        cadena.push(mostrar(&raiz, &ruta));
        return Err((
            "E_MODULO_CICLO",
            format!("ciclo de módulos: {}", cadena.join(" -> ")),
        ));
    }
    if profundidad > MAX_PROFUNDIDAD {
        return Err((
            "E_MODULO_PROFUNDIDAD",
            format!("la cadena de imports pasa de {MAX_PROFUNDIDAD} niveles"),
        ));
    }

    let Some(parseado) = st.leer(&ruta) else {
        return Err((
            "E_MODULO_NO_ENCONTRADO",
            format!("no se pudo leer '{}'", mostrar(&st.raiz, &ruta)),
        ));
    };

    st.pila.push(ruta.clone());
    let mut deps: Vec<usize> = Vec::new();
    let mut imports: Vec<(String, usize)> = Vec::new();
    let mut resolucion: Vec<NeutralDiag> = Vec::new();
    for imp in &parseado.modulo.imports {
        let destino = match st.resolver_ruta(&ruta, &imp.path) {
            Ok(d) => d,
            Err(fallo) => {
                resolucion.push(diag(fallo, imp.path_span));
                continue;
            }
        };
        let dep = match visitar(destino, profundidad + 1, st) {
            Ok(d) => d,
            Err(fallo) => {
                resolucion.push(diag(fallo, imp.path_span));
                continue;
            }
        };
        // El destino ya está resuelto, así que se le puede preguntar qué
        // exporta. Todo elemento de nivel superior exporta: en Marea no hay
        // `pub` porque la unidad de privacidad es el archivo.
        for n in &imp.names {
            if st.archivos[dep].modulo.items.iter().any(|i| i.name() == n.name) {
                continue;
            }
            let donde = mostrar(&st.raiz, &st.archivos[dep].ruta);
            let mensaje = format!("el módulo '{donde}' no exporta '{}'", n.name);
            resolucion.push(diag(("E_MODULO_NO_EXPORTA", mensaje), n.span));
        }
        if !deps.contains(&dep) {
            deps.push(dep);
        }
        imports.push((imp.path.clone(), dep));
    }
    st.pila.pop();

    let id = st.archivos.len();
    st.por_ruta.insert(ruta.clone(), id);
    st.archivos.push(EnCurso {
        ruta,
        fuente: parseado.fuente,
        modulo: parseado.modulo,
        sintaxis: parseado.sintaxis,
        resolucion,
        deps,
        imports,
    });
    Ok(id)
}

/// Verifica el grafo ya recorrido y reparte los errores por archivo.
fn terminar(raiz: PathBuf, archivos: Vec<EnCurso>) -> Salida {
    // Los tipos sólo se miran si la sintaxis y la resolución están limpias. Con
    // un archivo que no parsea o un import que no resuelve, el verificador vería
    // medio programa y llenaría el editor de nombres sin resolver: exactamente
    // el ruido que se viene a quitar.
    let bloqueado = archivos.iter().any(|a| !a.sintaxis.is_empty() || !a.resolucion.is_empty());

    let mut diags: Vec<Vec<NeutralDiag>> = archivos
        .iter()
        .map(|a| {
            let mut v = a.sintaxis.clone();
            v.extend(a.resolucion.iter().cloned());
            v
        })
        .collect();

    if !bloqueado {
        if archivos.len() == 1 && archivos[0].modulo.imports.is_empty() {
            // Un archivo sin imports es lo de siempre: el chequeo de UN módulo,
            // que es el que tiene el comportamiento conocido y las pruebas.
            diags[0].extend(check(&archivos[0].modulo).into_iter().map(diag_de_tipos));
        } else {
            let program = Program {
                raiz: raiz.clone(),
                modulos: archivos
                    .iter()
                    .enumerate()
                    .map(|(id, a)| ModuloResuelto {
                        id,
                        nombre: mostrar(&raiz, &a.ruta),
                        ruta: a.ruta.clone(),
                        fuente: a.fuente.clone(),
                        modulo: a.modulo.clone(),
                        deps: a.deps.clone(),
                    })
                    .collect(),
            };
            for e in check_program(&program) {
                if let Some(d) = diags.get_mut(e.modulo) {
                    d.push(diag_de_tipos(e.error));
                }
            }
        }
    }

    Salida {
        archivos: archivos
            .into_iter()
            .zip(diags)
            .map(|(a, propios)| Archivo {
                ruta: Some(a.ruta),
                fuente: a.fuente,
                modulo: a.modulo,
                diags: propios,
                imports: a.imports,
            })
            .collect(),
    }
}

fn diag((code, message): Fallo, span: Span) -> NeutralDiag {
    NeutralDiag {
        severity: Severity::Error,
        span,
        code: Some(code.to_string()),
        message,
        notes: Vec::new(),
    }
}

/// La ruta como se le enseña a una persona: relativa a la raíz del programa,
/// que es lo que esa persona escribió.
fn mostrar(raiz: &Path, ruta: &Path) -> String {
    ruta.strip_prefix(raiz)
        .unwrap_or(ruta)
        .display()
        .to_string()
}
