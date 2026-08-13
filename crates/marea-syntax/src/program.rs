//! Resolución del grafo de módulos: de un archivo de entrada al programa entero.
//!
//! Un programa de Marea deja de ser un archivo y pasa a ser un árbol de ellos.
//! [`resolve_program`] parte del archivo de entrada, sigue sus `import` en
//! profundidad y devuelve todos los módulos en orden topológico —dependencias
//! antes que dependientes—, que es el orden en el que cualquier fase posterior
//! querrá recorrerlos: al llegar a un módulo, todo lo que importa ya está visto.
//!
//! Lo que este archivo NO hace, a propósito: verificar tipos, resolver nombres
//! dentro de los cuerpos, ni generar código. Aquí sólo se decide qué archivos
//! forman el programa y en qué orden.
//!
//! Dos reglas acotan el alcance en v0:
//!   - Las rutas son relativas y se escriben con `./` o `../`. La forma desnuda
//!     (`"usuarios.mar"`) queda libre para un futuro registro de paquetes, que
//!     inventar ahora sería el error.
//!   - Ningún módulo puede quedar fuera del directorio del archivo de entrada.
//!     Como las rutas se canonicalizan, esto ataja tanto un `../..` de más como
//!     un enlace simbólico que apunte fuera del árbol.

use crate::ast::Module;
use crate::span::{line_col, Span};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Profundidad máxima de la cadena de imports. Igual que el `MAX_DEPTH` del
/// parser: la resolución es recursiva, y un árbol patológico no debe tumbar el
/// proceso, sino salir por un error ordinario.
const MAX_PROFUNDIDAD: usize = 64;

/// Un programa entero: todos sus módulos, en orden topológico.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    /// El directorio del archivo de entrada. Todo módulo del programa vive
    /// dentro de él.
    pub raiz: PathBuf,
    /// Dependencias antes que dependientes; el archivo de entrada es el último.
    pub modulos: Vec<ModuloResuelto>,
}

impl Program {
    /// El archivo de entrada. Es el último del orden topológico porque nada
    /// del programa lo importa.
    pub fn entrada(&self) -> &ModuloResuelto {
        self.modulos
            .last()
            .expect("un programa resuelto tiene al menos el módulo de entrada")
    }
}

/// Un módulo ya leído y parseado, con su sitio en el grafo.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuloResuelto {
    /// Índice de este módulo en `Program::modulos`, y por tanto su posición en
    /// el orden topológico.
    pub id: usize,
    /// Ruta canónica: es la identidad del módulo. Dos imports que llegan al
    /// mismo archivo por caminos distintos comparten esta ruta, y por eso el
    /// archivo se lee y parsea una sola vez.
    pub ruta: PathBuf,
    /// La ruta relativa a la raíz del programa, para mostrarla a una persona.
    pub nombre: String,
    /// El fuente tal cual se leyó, para poder renderizar errores posteriores
    /// sin volver a tocar el disco.
    pub fuente: String,
    pub modulo: Module,
    /// Los `id` de los módulos que este importa, sin repetir. Todos son menores
    /// que el propio `id`, que es justo lo que significa el orden topológico.
    pub deps: Vec<usize>,
}

/// Qué clase de problema encontró el resolvedor. El texto para la persona va en
/// `ProgramError::mensaje`; esto es para que el código pueda distinguirlos sin
/// leer cadenas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Clase {
    /// El archivo no existe, o no se pudo leer.
    NoSePudoLeer,
    /// El archivo existe pero no parsea.
    Sintaxis,
    /// Los imports se muerden la cola.
    Ciclo,
    /// Se importó un nombre que el módulo destino no declara.
    NoExportado,
    /// La ruta no empieza por `./` ni por `../`.
    RutaNoRelativa,
    /// La ruta resuelve fuera del directorio del archivo de entrada.
    FueraDelArbol,
    /// La cadena de imports pasa del máximo de profundidad.
    DemasiadoProfundo,
}

/// Un error de resolución, con el mensaje ya listo para imprimir.
///
/// El mensaje se compone aquí y no en quien lo muestra porque este es el único
/// punto donde el fuente del archivo culpable sigue a mano: cada error puede
/// señalar su línea y su columna aunque el archivo no sea el de entrada.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramError {
    pub clase: Clase,
    pub mensaje: String,
}

impl ProgramError {
    /// El mensaje completo, con el trozo de código señalado.
    pub fn render(&self) -> &str {
        &self.mensaje
    }
}

impl std::fmt::Display for ProgramError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.mensaje)
    }
}

impl std::error::Error for ProgramError {}

/// Resuelve el programa que cuelga de `entrada`.
///
/// Devuelve los módulos en orden topológico. Cada archivo aparece una sola vez
/// por mucho que lo importen varios, porque la identidad de un módulo es su
/// ruta canónica y no el texto que se escribió para llegar a él.
pub fn resolve_program(entrada: &Path) -> Result<Program, ProgramError> {
    let ruta = entrada.canonicalize().map_err(|e| ProgramError {
        clase: Clase::NoSePudoLeer,
        mensaje: format!("error: no se pudo abrir '{}': {}", entrada.display(), e),
    })?;
    // La raíz es el directorio de la entrada, ya canónico por venir de una ruta
    // canónica. `parent()` sólo falla en la raíz del sistema de archivos, que no
    // es un archivo `.mar`.
    let raiz = ruta
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| ProgramError {
            clase: Clase::NoSePudoLeer,
            mensaje: format!("error: '{}' no está en ningún directorio", ruta.display()),
        })?;

    let mut r = Resolvedor {
        raiz,
        por_ruta: HashMap::new(),
        modulos: Vec::new(),
        pila: Vec::new(),
    };
    r.visitar(ruta, 0, None)?;
    Ok(Program {
        raiz: r.raiz,
        modulos: r.modulos,
    })
}

/// El `import` que llevó hasta el módulo que se está resolviendo: sirve para
/// que el error señale la línea del archivo que importa, no la del importado.
#[derive(Clone, Copy)]
struct Origen<'a> {
    ruta: &'a Path,
    fuente: &'a str,
    span: Span,
}

impl<'a> Origen<'a> {
    /// El mismo origen apuntando a otro trozo del mismo archivo (p. ej. al
    /// nombre importado en vez de a la ruta).
    fn con_span(self, span: Span) -> Origen<'a> {
        Origen { span, ..self }
    }
}

struct Resolvedor {
    raiz: PathBuf,
    /// Ruta canónica -> id, sólo de módulos YA terminados. Que los pendientes no
    /// estén aquí es justo lo que permite distinguir "ya visto" de "ciclo".
    por_ruta: HashMap<PathBuf, usize>,
    modulos: Vec<ModuloResuelto>,
    /// El camino de módulos abiertos, de la entrada hasta el actual.
    pila: Vec<PathBuf>,
}

impl Resolvedor {
    /// Lee, parsea y resuelve el módulo de `ruta`, y los que este importe.
    /// Devuelve su `id`. Los hijos se registran antes que el padre: de ahí sale
    /// el orden topológico, sin ordenar nada después.
    fn visitar(
        &mut self,
        ruta: PathBuf,
        profundidad: usize,
        origen: Option<Origen<'_>>,
    ) -> Result<usize, ProgramError> {
        if let Some(&id) = self.por_ruta.get(&ruta) {
            return Ok(id); // ya resuelto por otro import: no se vuelve a parsear
        }
        if let Some(desde) = self.pila.iter().position(|p| *p == ruta) {
            return Err(self.error_ciclo(desde, &ruta, origen));
        }
        if profundidad > MAX_PROFUNDIDAD {
            return Err(self.error(
                Clase::DemasiadoProfundo,
                format!("la cadena de imports pasa de {MAX_PROFUNDIDAD} niveles"),
                origen,
            ));
        }

        let fuente = std::fs::read_to_string(&ruta).map_err(|e| {
            self.error(
                Clase::NoSePudoLeer,
                format!("no se pudo leer '{}': {}", self.mostrar(&ruta), e),
                origen,
            )
        })?;
        let modulo = crate::parse(&fuente).map_err(|e| ProgramError {
            clase: Clase::Sintaxis,
            mensaje: apuntar(&e.message, &self.mostrar(&ruta), &fuente, e.span),
        })?;

        self.pila.push(ruta.clone());
        let mut deps: Vec<usize> = Vec::new();
        for import in &modulo.imports {
            let aqui = Origen {
                ruta: &ruta,
                fuente: &fuente,
                span: import.path_span,
            };
            let ruta_destino = self.resolver_ruta(&ruta, &import.path, aqui)?;
            let dep = self.visitar(ruta_destino, profundidad + 1, Some(aqui))?;
            // El módulo destino ya está resuelto, así que se puede preguntar qué
            // exporta. Todo elemento de nivel superior exporta: en Marea no hay
            // `pub` porque la unidad de privacidad es el archivo.
            for nombre in &import.names {
                let destino = &self.modulos[dep];
                if !destino.modulo.items.iter().any(|i| i.name() == nombre.name) {
                    let mensaje = format!(
                        "el módulo '{}' no exporta '{}'",
                        destino.nombre, nombre.name
                    );
                    return Err(self.error(
                        Clase::NoExportado,
                        mensaje,
                        Some(aqui.con_span(nombre.span)),
                    ));
                }
            }
            if !deps.contains(&dep) {
                deps.push(dep);
            }
        }
        self.pila.pop();

        let id = self.modulos.len();
        let nombre = self.mostrar(&ruta);
        self.por_ruta.insert(ruta.clone(), id);
        self.modulos.push(ModuloResuelto {
            id,
            nombre,
            ruta,
            fuente,
            modulo,
            deps,
        });
        Ok(id)
    }

    /// Convierte el texto de un `import` en la ruta canónica del archivo que
    /// nombra, o explica por qué no puede.
    fn resolver_ruta(
        &self,
        desde: &Path,
        escrita: &str,
        origen: Origen<'_>,
    ) -> Result<PathBuf, ProgramError> {
        if !(escrita.starts_with("./") || escrita.starts_with("../")) {
            let pista = if Path::new(escrita).is_absolute() {
                "las rutas absolutas no son portables entre máquinas"
            } else {
                "la forma sin './' queda reservada para los paquetes, que aún no existen"
            };
            return Err(self.error(
                Clase::RutaNoRelativa,
                format!(
                    "la ruta '{escrita}' debe ser relativa y empezar por './' o '../': {pista}"
                ),
                Some(origen),
            ));
        }
        // `desde` es canónica, así que su directorio existe.
        let base = desde.parent().unwrap_or(self.raiz.as_path());
        let candidata = base.join(escrita);
        // Canonicalizar hace dos cosas de un golpe: confirma que el archivo
        // existe y deja una identidad única para él, resolviendo `..` y enlaces.
        let ruta = candidata.canonicalize().map_err(|e| {
            self.error(
                Clase::NoSePudoLeer,
                format!("no se encontró el módulo '{escrita}': {e}"),
                Some(origen),
            )
        })?;
        if !ruta.starts_with(&self.raiz) {
            return Err(self.error(
                Clase::FueraDelArbol,
                format!(
                    "el módulo '{escrita}' queda fuera del programa: resuelve a '{}', \
                     que no está bajo '{}'",
                    ruta.display(),
                    self.raiz.display()
                ),
                Some(origen),
            ));
        }
        Ok(ruta)
    }

    /// El ciclo, con la cadena entera desde la primera vez que apareció el
    /// módulo repetido hasta él mismo otra vez: `a.mar -> b.mar -> a.mar`.
    fn error_ciclo(
        &self,
        desde: usize,
        repetido: &Path,
        origen: Option<Origen<'_>>,
    ) -> ProgramError {
        let mut cadena: Vec<_> = self.pila[desde..].iter().map(|p| self.mostrar(p)).collect();
        cadena.push(self.mostrar(repetido));
        self.error(
            Clase::Ciclo,
            format!("ciclo de módulos: {}", cadena.join(" -> ")),
            origen,
        )
    }

    fn error(&self, clase: Clase, mensaje: String, origen: Option<Origen<'_>>) -> ProgramError {
        let mensaje = match origen {
            Some(o) => apuntar(&mensaje, &self.mostrar(o.ruta), o.fuente, o.span),
            None => format!("error: {mensaje}"),
        };
        ProgramError { clase, mensaje }
    }

    /// La ruta como se la enseñamos a una persona: relativa a la raíz del
    /// programa, que es lo que esa persona escribió.
    fn mostrar(&self, ruta: &Path) -> String {
        ruta.strip_prefix(&self.raiz)
            .unwrap_or(ruta)
            .display()
            .to_string()
    }
}

/// Renderiza un error señalando su sitio: archivo, línea, columna y un cursor
/// `^` bajo el código. Mismo formato que [`crate::SyntaxError::render`], más el
/// nombre del archivo, que con varios módulos deja de sobreentenderse.
fn apuntar(mensaje: &str, ruta: &str, fuente: &str, span: Span) -> String {
    let (linea, col) = line_col(fuente, span.start);
    let texto = fuente.lines().nth(linea - 1).unwrap_or("");
    let ancho = span.end.saturating_sub(span.start).max(1);
    let cursor = format!("{}{}", " ".repeat(col.saturating_sub(1)), "^".repeat(ancho));
    format!(
        "error: {}\n  --> {}, línea {}, columna {}\n{:>4} | {}\n{:>4} | {}",
        mensaje, ruta, linea, col, linea, texto, "", cursor
    )
}

/// Une los módulos de un programa en uno solo, en orden topológico.
///
/// Es lo que consume el codegen: la salida de Marea es un bundle plano —un
/// `client.ts` y un `server.ts`—, no un módulo ES por archivo, así que emitir
/// por separado daría una copia del runtime de 43 KB por módulo y ninguna forma
/// de que una función llame a otra.
///
/// Los `import` desaparecen aquí: ya cumplieron su papel en el verificador, que
/// es donde deciden qué ve cada módulo. Aplanar DESPUÉS de verificar es lo que
/// permite que el aislamiento sea real y el bundle siga siendo plano.
///
/// El orden importa y es el topológico: las dependencias van antes, así que una
/// constante de módulo que dependa de otra se inicializa en el orden correcto.
pub fn aplanar(program: &Program) -> Module {
    let mut items = Vec::new();
    for m in &program.modulos {
        items.extend(m.modulo.items.iter().cloned());
    }
    Module {
        items,
        imports: Vec::new(),
    }
}
