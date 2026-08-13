//! El modelo de eventos: `on(evento, manejador)`, la frontera del TIEMPO en
//! dirección al programa.
//!
//! El reactivo lleva el estado hasta el DOM; esto es la vuelta. `on` no es
//! sintaxis nueva: es una llamada que devuelve `Html` —el ATRIBUTO con el que un
//! elemento queda atado a un cierre— y por eso encaja en un hueco crudo de
//! plantilla como cualquier otro marcado:
//!
//! ```text
//! `<button {!on("click", fn() { cuenta = cuenta + 1; })}>suma</button>`
//! ```
//!
//! Lo que aquí se comprueba es lo que el apaño anterior —emitir
//! `onclick="marea.f(3)"` a mano y exponer funciones en `window`— no podía
//! comprobar nadie: que el evento exista, que el manejador tenga la forma de un
//! manejador y que no se escriba en un lado donde no hay DOM que enganchar.

use super::*;

/// Los eventos que Marea sabe enganchar. Es una lista CERRADA a propósito: un
/// nombre mal escrito no produce un error en ningún sitio —el navegador acepta
/// `addEventListener("clcik", ...)` sin rechistar— y lo único que se ve es un
/// botón que no hace nada, que es de lo más caro que hay de encontrar. Con la
/// lista cerrada, el error sale al compilar y con los nombres válidos delante.
///
/// Son los que un consumidor real ya usa. Añadir uno es añadirlo aquí: el
/// runtime no tiene lista propia —engancha el que le llegue—, así que esta es la
/// única puerta y no hay dos listas que puedan divergir.
pub const EVENTOS: &[&str] = &[
    "click",
    "submit",
    "change",
    "input",
    "keydown",
    "keyup",
    "blur",
    "pointermove",
    "pointerdown",
    "pointerup",
    "pointerleave",
];

/// El nombre del builtin, en un sitio: lo miran el tipado de llamadas, la
/// comprobación de los huecos de plantilla y la lista de síncronos.
pub const ON: &str = "on";

/// Los eventos válidos, listos para meter en un mensaje de error.
fn lista_de_eventos() -> String {
    EVENTOS.join(", ")
}

/// ¿Es `e` una llamada a `on(...)`?
pub(crate) fn es_llamada_a_on(e: &Expr) -> bool {
    match e {
        Expr::Call { callee, .. } => {
            matches!(callee.as_ref(), Expr::Ident { name, .. } if name == ON)
        }
        _ => false,
    }
}

/// `on` es SÍNCRONO: registra un cierre y devuelve texto, sin tocar ni la red ni
/// el disco. La lista canónica de builtins síncronos vive en
/// `marea_syntax::builtins::SINCRONOS`, que es de la otra mitad del reparto, así
/// que hasta que `on` entre ahí la excepción vive aquí —y otra igual en el
/// emisor—. Cuando se pueda tocar ese crate, esto se borra y `on` se añade a la
/// lista, que es donde le toca.
pub(crate) fn es_sincrono_con_eventos(name: &str) -> bool {
    es_sincrono(name) || name == ON
}

impl Checker {
    /// Chequea `on(evento, manejador)`.
    ///
    /// Tres reglas, y las tres existen porque lo que fallaría si no está callado:
    /// un evento inexistente es un botón mudo, un manejador en el servidor es un
    /// `ReferenceError` en cada petición (o peor: nada), y un manejador con
    /// argumentos es una llamada que el despachador nunca hará como se espera.
    pub(crate) fn check_on_builtin(&mut self, args: &[Expr], span: Span) -> Ty {
        // Un manejador no puede vivir en el servidor: el DOM al que se engancha
        // está en el navegador, y el bundle del servidor ni siquiera lleva el
        // despachador. Es la misma regla de ubicación que E_REACTIVE_OFF_CLIENT,
        // solo que aquí una función SIN anotación sí vale: se emite en los dos
        // bundles y en el del servidor `on` simplemente no llega a llamarse.
        if matches!(
            self.current_location,
            Some(Location::Server) | Some(Location::Edge)
        ) {
            let lado = if self.current_location == Some(Location::Edge) {
                "@edge"
            } else {
                "@server"
            };
            self.error(TypeError::new(
                "E_ON_OFF_CLIENT",
                format!(
                    "'{ON}' engancha un manejador al DOM, que solo existe en el navegador: una \
                     función {lado} no tiene dónde ponerlo. Devuelve el marcado a una @client y \
                     engánchalo allí; el manejador ya puede llamar a esta {lado} por RPC"
                ),
                span,
            ));
        }

        let arg_tys: Vec<Ty> = args.iter().map(|a| self.check_expr(a)).collect();
        if !self.arity(ON, &arg_tys, 2, span) {
            return Ty::Html;
        }

        // El evento se escribe literal: es lo que permite comprobarlo ahora y no
        // en el navegador de un usuario.
        match &args[0] {
            Expr::Str { value, span: s } if !EVENTOS.contains(&value.as_str()) => {
                let (v, s) = (value.clone(), *s);
                self.error(TypeError::new(
                    "E_EVENTO_DESCONOCIDO",
                    format!(
                        "'{v}' no es un evento que Marea sepa enganchar; los válidos son: {}",
                        lista_de_eventos()
                    ),
                    s,
                ));
            }
            Expr::Str { .. } => {}
            otro => {
                self.error(TypeError::new(
                    "E_EVENTO_NO_LITERAL",
                    format!(
                        "el evento de '{ON}' se escribe como literal (\"click\"): el compilador \
                         comprueba que exista, y de una variable no puede. Los válidos son: {}",
                        lista_de_eventos()
                    ),
                    otro.span(),
                ));
            }
        }

        // El manejador: un cierre sin argumentos que no devuelve nada. Sin
        // argumentos porque el evento como valor todavía no existe en el
        // lenguaje —cuando haga falta se añade—; sin retorno porque lo que hace
        // un manejador es un efecto, y devolver algo que nadie recoge solo
        // parecería que sirve para algo.
        match &arg_tys[1] {
            Ty::Unknown => {}
            Ty::Fn { params, ret, .. } => {
                if !params.is_empty() {
                    self.error(TypeError::new(
                        "E_MANEJADOR_CON_PARAMS",
                        format!(
                            "el manejador de '{ON}' no recibe argumentos: escríbelo \
                             'fn() {{ ... }}'. Lo que necesite lo captura del entorno"
                        ),
                        args[1].span(),
                    ));
                }
                if !matches!(**ret, Ty::Unit | Ty::Unknown) {
                    let r = ret.display();
                    self.error(TypeError::new(
                        "E_MANEJADOR_DEVUELVE",
                        format!(
                            "el manejador de '{ON}' devuelve '{r}' y nadie lo recoge: un \
                             manejador actúa —asigna a una 'reactive mut'—, no calcula"
                        ),
                        args[1].span(),
                    ));
                }
            }
            otro => {
                let t = otro.display();
                self.error(TypeError::new(
                    "E_MANEJADOR_NO_FN",
                    format!(
                        "el segundo argumento de '{ON}' es el manejador: un cierre \
                         'fn() {{ ... }}', no '{t}'"
                    ),
                    args[1].span(),
                ));
            }
        }

        // Devuelve marcado —un atributo— y por eso entra en `{!...}` sin que la
        // gramática tenga que aprender nada nuevo.
        Ty::Html
    }
}
