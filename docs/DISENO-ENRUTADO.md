# Enrutado y metadatos: propuesta de diseño

**Estado: propuesta.** No hay una línea escrita. Esto existe para decidir la
forma antes de construir, porque construir sobre la forma equivocada aquí sale
caro: toca el papel de `Html` como sumidero, que es una de las garantías
centrales del lenguaje.

Medido contra un sitio real (nueve rutas, cinco páginas con metadatos calculados
desde los datos, `sitemap.xml` y `robots.txt`), no contra un ejemplo.

## El problema, que no es "una ruta más"

Dos hechos del caso real tiran en la misma dirección:

1. **`sitemap.xml` y `robots.txt` no son HTML.** Necesitan su propio
   `content-type`. Si una ruta sólo sabe devolver `Html`, esos dos quedan fuera
   —y con ellos el SEO, que es el negocio.
2. **Los metadatos se calculan desde los datos.** El título lleva el nombre del
   producto; la descripción cuenta en cuántas tiendas está. Salen de la MISMA
   consulta que el cuerpo, así que no vale un bloque estático por ruta.

Juntos dicen una cosa: **una página no es una función que devuelve `Html`.** Es
algo que devuelve una respuesta —con su tipo de contenido, sus metadatos y su
cuerpo—, calculada de una vez.

## La forma propuesta

### Una página es una función anotada que devuelve `Page`

```marea
type Producto = { nombre: String, tiendas: Int, desde: Int };

@page("/modelo/:id")
fn modelo(id: Int) -> Page | NotFound {
    let p = buscarModelo(id);
    if p.tiendas < 1 {
        return NotFound;
    }
    return Page {
        titulo: concat(p.nombre, " — ¿dónde está más barato? · Ahórrame"),
        descripcion: `Precio de {p.nombre} comparado en {text(p.tiendas)} tiendas`,
        canonica: concat("https://ahorrame.mx/modelo/", text(id)),
        imagen: p.foto,
        jsonld: [productoLd(p), migaLd(p)],
        cuerpo: fichaCompleta(p),
    };
}
```

`Page` es un tipo builtin (un registro con campos fijos). El compilador sabe
armar el documento: `<head>` con las etiquetas, un `<script
type="application/ld+json">` **por cada elemento** de `jsonld` —nunca un arreglo,
que es lo que revienta a las herramientas que lo leen— y el `cuerpo` dentro.

**El 404 sale gratis y no es un caso especial.** Es la variante de fallo del tipo
de retorno, exactamente como el `User | NotFound` de la portada del README. El
runtime traduce `NotFound` a un 404; el compilador ya obliga a que el tipo lo
diga. No hay una "página de error" que registrar aparte.

### Lo que no es HTML devuelve `Response`

```marea
@page("/robots.txt")
fn robots() -> Response {
    return plainText("User-agent: *\nSitemap: https://ahorrame.mx/sitemap.xml\n");
}

@page("/sitemap.xml")
fn sitemap() -> Response {
    return xmlDoc(`<urlset>{!urlsDeTodosLosModelos()}</urlset>`);
}
```

Dos builtins y ninguna sintaxis nueva:

- `plainText(String) -> Response` — sin escapado, porque no hay marcado que
  escapar.
- `xmlDoc(Html) -> Response` — **exige `Html`**, y ahí está el detalle
  bueno: XML escapa los mismos cinco caracteres que HTML, así que la garantía que
  ya existe vale tal cual. Un nombre de producto con un `&` no rompe el sitemap
  porque el tipo no deja construirlo sin escapar.

No se inventa un tipo `Xml`. Sería un `Html` con otro nombre.

### La query string: leer sí, construir no hace falta

```marea
@page("/buscar")
fn buscar() -> Page {
    let q = query("q");
    let pagina = match parseInt(query("pagina")) {
        NotANumber => 1,
        n => n,
    };
    ...
}
```

- `query(nombre: String) -> String` — cadena vacía si no está. Las query
  strings **son** cadenas; fingir otra cosa sería mentir.
- `parseInt(String) -> Int | NotANumber` — convertir puede fallar, así que el tipo
  lo dice y el `match` obliga a decidir el valor por defecto. Hoy el lenguaje no
  tiene forma de convertir texto a número: esto tapa un hueco que va más allá del
  enrutado.

**Construir** la query no necesita nada: se concatena en el `href` y navega el
navegador. Ya está probado en el filtro de precio y funciona.

### Dónde corre una página

`@page` implica servidor: se renderiza para que Google lo lea. Puede llamar a
`@server` libremente y no puede tocar estado reactivo del cliente —las mismas
reglas que ya existen—.

La interactividad va **encima**, con las islas que ya están: el cuerpo incluye
sus puntos de montaje y el cliente los monta. Es la arquitectura que el sitio ya
tiene con Next, pero nativa: **el servidor pinta la página, las islas la
animan.**

## Lo que esta propuesta NO resuelve, y hay que decidir aparte

- **`/api/alerts`.** No sale gratis con `@server`, contra lo que se suponía: una
  `@server` se expone en `/__marea`, no en la URL que elijas. Sale gratis **sólo
  si nada externo depende de esa URL** —si la llama tu propio frontend, se
  sustituye por una llamada RPC y la URL deja de importar—. Si hay clientes
  externos, hace falta poder fijar la ruta de un endpoint, y eso es otra decisión.
- **Redirecciones y códigos de estado** que no sean 200/404.
- **Cabeceras de caché**, que en un sitio que vive de Google no son un detalle.
- **Arranque en frío y servir en producción** (Cloud Run): sigue en el nivel 3 y
  sin tocar.

## Riesgo principal

`Page` con campos fijos acierta hoy y envejece: en cuanto haga falta una
etiqueta que no está (`og:type`, `twitter:card`, `hreflang`), o se amplía el
registro —y cada ampliación es un cambio del lenguaje— o se añade un campo de
escape que acepte `Html` crudo en el `<head>`, y entonces la garantía de que los
metadatos están bien formados se pierde.

No lo resuelvo aquí a propósito. Es la pregunta que hay que contestar antes de
implementar, y contestarla mal es lo que haría de esto una capa que estorba en
vez de una que ayuda.
