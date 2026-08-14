//! `@page("/modelo/:id")`: las reglas de una página.
//!
//! Una página no es "una función más con una anotación": es lo que atiende una
//! URL, y eso obliga a tres cosas que ninguna otra función tiene que cumplir.
//!
//!   1. **La ruta y la firma tienen que decir lo mismo.** Los segmentos
//!      `:nombre` se atan a los parámetros POR NOMBRE, así que un `:id` sin
//!      parámetro `id` es una página que el despachador no sabe invocar, y un
//!      parámetro que no está en la ruta es un argumento que nadie puede pasar.
//!      Las dos mitades del error son silenciosas en tiempo de ejecución —sale
//!      un `undefined` donde iba el identificador— y por eso se dicen aquí.
//!   2. **Sólo `Int` y `String` viajan en una URL.** Un `Float` o un registro en
//!      un segmento no significan nada: no hay forma de escribirlos que el
//!      lenguaje sepa leer de vuelta.
//!   3. **Se devuelve `Page` o `Response`, y nada más.** `Html` es el CUERPO
//!      de una página, no la página: no lleva título, ni canónica, ni tipo de
//!      contenido, que es exactamente lo que un buscador lee.
//!
//! El 404 no aparece por ninguna parte, y es deliberado: es la variante de fallo
//! del retorno (`Page | NotFound`), igual que en cualquier otra función
//! del lenguaje. No hay un registro de páginas de error que mantener.
//!
//! Dónde CORRE una página no se decide aquí: `ubicacion_efectiva` la trata como
//! `@server` y las reglas de ubicación que ya existen caen solas.

use super::*;

/// Un trozo de ruta entre barras.
#[derive(Debug, PartialEq)]
pub(crate) enum Segmento<'a> {
    /// Texto literal: `modelo` en `/modelo/:id`.
    Fijo(&'a str),
    /// Hueco atado a un parámetro por su nombre: `id` en `/modelo/:id`.
    Param(&'a str),
}

/// Parte una ruta en segmentos. Los vacíos se descartan, así que `/a/` y `/a`
/// dan lo mismo: la barra final no cambia qué URL se atiende, y dos páginas que
/// sólo difieran en ella serían la misma con dos escrituras.
pub(crate) fn segmentos(ruta: &str) -> Vec<Segmento<'_>> {
    ruta.split('/')
        .filter(|s| !s.is_empty())
        .map(|s| match s.strip_prefix(':') {
            Some(nombre) => Segmento::Param(nombre),
            None => Segmento::Fijo(s),
        })
        .collect()
}

/// La forma de lo que una ruta ATIENDE, que es lo que puede chocar con otra.
///
/// El nombre del parámetro no entra: `/modelo/:id` y `/modelo/:slug` responden
/// exactamente a las mismas URLs, así que declararlas las dos no es tener dos
/// rutas, es tener una escrita dos veces y un despachador sin criterio para
/// elegir. Comparar los literales tal cual dejaría pasar justo ese caso.
pub(crate) fn canonica(ruta: &str) -> String {
    let partes: Vec<&str> = segmentos(ruta)
        .into_iter()
        .map(|s| match s {
            Segmento::Fijo(f) => f,
            Segmento::Param(_) => ":",
        })
        .collect();
    format!("/{}", partes.join("/"))
}

impl Checker {
    /// Comprueba las páginas del módulo. Corre tras la recolección (necesita los
    /// alias para resolver el tipo de retorno) y antes de los cuerpos.
    pub(crate) fn check_paginas(&mut self, module: &Module) {
        // Canónica -> la ruta tal como se escribió la primera vez.
        let mut vistas: HashMap<String, String> = HashMap::new();
        for item in &module.items {
            let Item::Fn(f) = item else { continue };
            let Some(ruta) = &f.ruta else { continue };
            let span = f.ruta_span.unwrap_or(f.span);
            self.check_ruta(f, ruta, span);
            self.check_retorno_de_pagina(f);

            match vistas.get(&canonica(ruta)) {
                Some(previa) => {
                    let previa = previa.clone();
                    self.error(ruta_duplicada(ruta, &previa, "este módulo", span));
                }
                None => {
                    vistas.insert(canonica(ruta), ruta.clone());
                }
            }
        }
    }

    /// La ruta contra la firma: barra inicial, segmentos bien formados y una
    /// correspondencia EXACTA entre los `:nombre` y los parámetros.
    fn check_ruta(&mut self, f: &FnDecl, ruta: &str, span: Span) {
        // Una ruta es absoluta: se compara contra el camino de una URL, que
        // siempre empieza por '/'. Sin la barra no hay contra qué compararla.
        if !ruta.starts_with('/') {
            self.error(TypeError::new(
                "E_RUTA_SIN_BARRA",
                format!(
                    "la ruta de '{}' es \"{ruta}\" y una ruta empieza por '/': se compara \
                     contra el camino de una URL, que siempre lo lleva. Escribe \"/{ruta}\"",
                    f.name
                ),
                span,
            ));
        }

        let mut atados: Vec<&str> = Vec::new();
        for seg in segmentos(ruta) {
            let nombre = match seg {
                Segmento::Param(n) => n,
                Segmento::Fijo(_) => continue,
            };
            if nombre.is_empty() {
                self.error(TypeError::new(
                    "E_RUTA_PARAM_SIN_NOMBRE",
                    format!(
                        "hay un ':' suelto en la ruta de '{}': un hueco se ata a un parámetro \
                         por su nombre, así que hay que escribirlo ( \"/modelo/:id\" )",
                        f.name
                    ),
                    span,
                ));
                continue;
            }
            if atados.contains(&nombre) {
                self.error(TypeError::new(
                    "E_RUTA_PARAM_REPETIDO",
                    format!(
                        "':{nombre}' aparece dos veces en la ruta de '{}': los dos huecos irían \
                         al mismo parámetro y sólo uno podría ganar. Dales nombres distintos",
                        f.name
                    ),
                    span,
                ));
                continue;
            }
            atados.push(nombre);

            match f.params.iter().find(|p| p.name == nombre) {
                None => {
                    let nombres: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
                    let tiene = if nombres.is_empty() {
                        "no recibe ninguno".to_string()
                    } else {
                        format!("recibe: {}", nombres.join(", "))
                    };
                    self.error(TypeError::new(
                        "E_RUTA_SEGMENTO_SIN_PARAM",
                        format!(
                            "la ruta declara ':{nombre}' pero '{}' {tiene}: el valor del hueco \
                             no tendría dónde entrar. Añade '{nombre}' a la firma, o quítalo \
                             de la ruta",
                            f.name
                        ),
                        span,
                    ));
                }
                Some(p) => {
                    let ty = self.ty_from_syntax(&p.ty);
                    // Lo que viaja en una URL es texto. `Int` se lee de vuelta
                    // sin ambigüedad y `String` es lo que ya era; cualquier otra
                    // cosa —un Float, un registro— no tiene una escritura que el
                    // lenguaje sepa deshacer, así que no significa nada ahí.
                    if !matches!(ty, Ty::Int | Ty::String | Ty::Unknown) {
                        self.error(TypeError::new(
                            "E_RUTA_PARAM_TIPO",
                            format!(
                                "el parámetro '{}' es '{}' y va en un segmento de la ruta: en una \
                                 URL sólo caben 'Int' y 'String', que son los que se leen de vuelta \
                                 sin inventarse nada",
                                p.name,
                                ty.display()
                            ),
                            p.span,
                        ));
                    }
                }
            }
        }

        // La otra mitad de la correspondencia. A una página la invoca una URL y
        // nadie más: un parámetro que la ruta no menciona no se puede rellenar.
        for p in &f.params {
            if atados.contains(&p.name.as_str()) {
                continue;
            }
            let nombre = &p.name;
            self.error(TypeError::new(
                "E_PARAM_SIN_SEGMENTO",
                format!(
                    "'{}' recibe '{nombre}' pero la ruta \"{ruta}\" no lo menciona: a una página \
                     la invoca una URL, así que lo que no está en la ruta no se lo pasa nadie. \
                     Añade ':{nombre}' a la ruta, o léelo con query(\"{nombre}\")",
                    f.name
                ),
                p.span,
            ));
        }
    }

    /// Dos retornos válidos y ninguno más: `Page` —con sus variantes de fallo,
    /// si las hay— o `Response`.
    fn check_retorno_de_pagina(&mut self, f: &FnDecl) {
        let span = f.return_type.as_ref().map(|t| t.span()).unwrap_or(f.span);
        let ret = match &f.return_type {
            Some(t) => self.ty_from_syntax(t),
            None => Ty::Unit,
        };
        // Se mira el tipo RESUELTO y no lo escrito, para que un alias
        // (`type Resultado = Page | NotFound`) valga igual.
        let variantes: Vec<String> = match &ret {
            Ty::Named(n) => vec![n.clone()],
            Ty::Response => vec!["Response".to_string()],
            Ty::Union(vs) => vs.clone(),
            _ => Vec::new(),
        };
        let sirve_pagina = variantes.iter().any(|v| v == "Page");
        let sirve_respuesta = variantes.iter().any(|v| v == "Response");

        // Uno de los dos y sólo uno: el resto de variantes de la unión son
        // fallos (`NotFound`), que es como el lenguaje dice "puede no estar".
        if sirve_pagina != sirve_respuesta {
            return;
        }
        if sirve_pagina && sirve_respuesta {
            self.error(TypeError::new(
                "E_PAGINA_RETORNO",
                format!(
                    "'{}' declara que devuelve 'Page' y 'Response' a la vez: una ruta sirve \
                     un documento o sirve otra cosa, y el tipo de contenido se decide al \
                     declararlo, no en cada rama",
                    f.name
                ),
                span,
            ));
            return;
        }

        // Ninguno. `Html` se explica aparte porque es el error que se comete: se
        // devuelve el cuerpo creyendo que es la página.
        let mensaje = if matches!(ret, Ty::Html) {
            format!(
                "'{}' devuelve 'Html', que es el CUERPO de una página y no la página: no lleva \
                 título, ni canónica, ni metadatos, que es justo lo que lee un buscador. \
                 Envuélvelo: 'Page {{ titulo: ..., canonica: ..., cuerpo: <tu Html> }}'",
                f.name
            )
        } else {
            format!(
                "'{}' es una página y devuelve '{}': una página devuelve 'Page' (con sus \
                 variantes de fallo si las tiene, como 'Page | NotFound') o 'Response' \
                 para lo que no es HTML, como un sitemap o un robots.txt",
                f.name,
                ret.display()
            )
        };
        self.error(TypeError::new("E_PAGINA_RETORNO", mensaje, span));
    }
}

/// El error de dos páginas en la misma ruta. Vive suelto porque lo levantan dos
/// sitios —el módulo y el programa— y el mensaje tiene que ser el mismo.
pub(crate) fn ruta_duplicada(ruta: &str, previa: &str, donde: &str, span: Span) -> TypeError {
    let misma_letra = ruta == previa;
    let matiz = if misma_letra {
        String::new()
    } else {
        format!(
            " (\"{previa}\" y \"{ruta}\" atienden las mismas URLs: lo que cambia es el nombre \
             del hueco, no lo que responden)"
        )
    };
    TypeError::new(
        "E_RUTA_DUPLICADA",
        format!(
            "\"{ruta}\" ya la sirve otra página en {donde}{matiz}: el despachador no tendría con \
             qué elegir entre las dos"
        ),
        span,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn una_ruta_se_parte_en_fijos_y_huecos() {
        assert_eq!(segmentos("/"), vec![]);
        assert_eq!(
            segmentos("/modelo/:id"),
            vec![Segmento::Fijo("modelo"), Segmento::Param("id")]
        );
        // Un ':' suelto es un hueco sin nombre, no un segmento fijo: así el
        // verificador puede decir qué le falta en vez de tratarlo como literal.
        assert_eq!(
            segmentos("/a/:"),
            vec![Segmento::Fijo("a"), Segmento::Param("")]
        );
    }

    /// Lo que decide si dos rutas chocan es a qué URLs responden, no cómo se
    /// escribieron: ni el nombre del hueco ni la barra final cambian ninguna.
    #[test]
    fn la_canonica_ignora_el_nombre_del_hueco_y_la_barra_final() {
        assert_eq!(canonica("/modelo/:id"), canonica("/modelo/:slug"));
        assert_eq!(canonica("/precios/"), canonica("/precios"));
        assert_eq!(canonica("/"), "/");
        assert_ne!(canonica("/modelo/:id"), canonica("/modelo/nuevo"));
    }
}
