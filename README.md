# Marea 🌊

Un lenguaje de programación enfocado a la web. Su tesis: volver **primitivas del
lenguaje** las dos fronteras que hoy se cruzan a mano y con pegamento.

1. **La frontera de red** (cliente ↔ servidor). Una función lleva una anotación
   de ubicación (`@server`, `@client`, `@edge`) y el compilador genera el cruce:
   RPC, serialización, tipos de ambos lados y la **validación del límite** —los
   guardas de cada handler se derivan de la firma, así que un argumento con el
   tipo equivocado se rechaza con un 400 antes de tocar tu código. No hay "capa
   de API" que escribir.
2. **La frontera del tiempo** (reactividad). Una variable `reactive` propaga sus
   cambios; el grafo de dependencias lo conoce el compilador, no una librería.
   `reactive mut` es una fuente (signal), `reactive` una derivada (memo), y
   `effect { ... }` se re-ejecuta solo cuando cambia algo que leyó.

```marea
type UserId = Int;
type User = { nombre: String };

store usuarios: User;

@server
fn getUser(id: UserId) -> User | NotFound {
    let us = todos(usuarios);
    if id < len(us) {
        return us[id];
    }
    return NotFound;
}

@client
fn perfil(id: UserId) -> Html {
    reactive usuario = getUser(id);   // se llama como si fuera local
    return match usuario {            // el tipo obliga a cubrir los cuatro casos
        Cargando => "<p>Cargando…</p>",
        Fallo    => "<p>Error de red</p>",
        NotFound => "<p>No existe</p>",
        otro     => concat("<h1>", concat(escapar(otro.nombre), "</h1>")),
    };
}
```

**Las dos fronteras se componen.** `reactive usuario = getUser(id)` es un
**recurso**: cruzar la red es asíncrono, así que el valor arranca en `Cargando`,
pasa al resultado cuando llega y a `Fallo` si la llamada revienta. Y el tipo lo
dice —`Cargando | User | NotFound | Fallo`—, de modo que el compilador **no te
deja leer el dato sin haber cubierto los cuatro casos**: mientras no cubras
`Cargando` y `Fallo`, lo que queda es una unión opaca.

Como el recurso es un signal, la vista se re-pinta sola en cada transición: no
hay estado de carga que orquestar a mano. Un recurso también puede vivir a nivel
de módulo, que es el sitio natural para los datos de la app.

## Estado: front-end + transpilador a TypeScript

Lo que ya funciona:

- **Lexer** sin dependencias: literales (int/float/string/bool), identificadores,
  palabras clave, operadores, comentarios `//` y `/* */`, soporte UTF-8.
- **AST** con `Location`, variables `reactive` y tipos unión (`A | B`) como datos
  de primera clase.
- **Parser** de descenso recursivo + Pratt (precedencia de operadores), con
  `if`/`match`/llamadas/acceso a miembro.
- **Diagnósticos** con línea, columna y cursor `^`.
- **Transpilador a TypeScript** (`marea-codegen`) que materializa la **frontera
  de red**: una función `@server` se vuelve un handler registrado + un *stub* RPC
  en el cliente. El `@client` que la llama no nota la diferencia.
- **Backend WebAssembly** (`marea-wasm`) que compila a WAT → `.wasm`, ejecutable
  en el navegador/Node **sin pasar por JS**. Soporta enteros/booleanos (`i32`),
  `let`, `if`, aritmética/comparación/lógica, llamadas/recursión, **cadenas**
  sobre memoria lineal, **registros (structs)** (campos contiguos de 4 bytes,
  `Punto { x: 1, y: 2 }` construye con el allocador bump y `x.campo` es un
  `i32.load offset=4*i`), y **listas** (`[10, 20, 30]` = `[longitud][elementos]`
  en memoria; `xs[i]` es un `i32.load` calculado).
- **Verificador de tipos** (`marea-types`) expuesto como `marea check`:
  resolución de nombres + tipos ligeros + **tipos de ubicación** (cruce de
  frontera `@client`→`@server` válido; `@server`→`@client` prohibido) + la regla
  estrella: un retorno unión `User | NotFound` es **opaco** y obliga a un `match`
  exhaustivo (con *narrowing* en la rama). Acumula todos los errores.
- **Servidor de lenguaje** (`marea-lsp`): un LSP sobre stdio que da diagnósticos
  en vivo (errores de parser y de tipos), document symbols, completado, ir a la
  definición y hover. Reusa el mismo `parse` + `check` del compilador. Las deps
  externas quedan aisladas aquí; los crates de compilador siguen 100% std.
- **App web** (`marea build-web`): genera `index.html` + `glue.mjs` + `module.wat`
  para correr un módulo WASM de Marea en el navegador (el glue carga el `.wasm`,
  expone las funciones en `window.marea` decodificando cadenas, y renderiza
  `vista()`/`main()` en el DOM).
- **CLI** `marea` con `tokens`, `parse`, `check`, `build`, `build-wasm`,
  `build-web` y `build-app`. **Todos los `build*` verifican tipos antes de
  emitir** (la garantía del verificador dejó de ser opt-in); `--no-check` la
  omite si quieres compilar de todos modos.

### Hablar con servicios externos

Una app real consume APIs de terceros. `pedir(url)` hace un GET y devuelve el
cuerpo; `pedirPost(url, cuerpo)` manda JSON. Como el lenguaje no tiene valores
dinámicos, la respuesta se lee **por ruta**: `jsonTexto`, `jsonNumero`,
`jsonDecimal` y `jsonLargo`.

```marea
@server
fn consultar(lat: String, lon: String) -> Int {
    let cuerpo = pedir(concat("https://api.open-meteo.com/v1/forecast?latitude=", lat));
    return jsonNumero(cuerpo, "current.temperature_2m");
}
```

Salir a la red **solo se permite desde `@server`** (`E_RED_OFF_SERVER`): desde el
navegador la petición la haría el cliente —otro origen, otras credenciales, CORS
decidiendo por ti—, que no es lo que el programa dice. Leer JSON sí vale en
cualquier lado: es cómputo puro sobre un texto que ya se tiene.

**Defensas contra SSRF**, porque dar red al servidor es dársela a quien controle
la URL: sólo `http`/`https`; se bloquean loopback, enlace local, `169.254.169.254`
(metadatos de nube), rangos privados y hosts `.internal`; los redirects se
rechazan (podrían saltarse la lista blanca); y hay tope de tiempo
(`MAREA_HTTP_TIMEOUT`) y de tamaño (`MAREA_HTTP_MAX`).

> **Limitación honesta:** el bloqueo mira el nombre del host, no la IP a la que
> resuelve. Un dominio público cuyo registro A apunte a una dirección privada
> pasaría el filtro. Para producción, fija la lista blanca `MAREA_HTTP_HOSTS`:
> con ella sólo se contactan los destinos que enumeres.

### Listas y texto

Sin construir listas en tiempo de ejecución no se puede escribir una búsqueda:
el lenguaje no tiene bucles ni cierres, así que una función no podía devolver un
subconjunto filtrado —sólo pintarlo—. `unir(a, b)` concatena dos listas y
`agregar(xs, x)` añade un elemento; ambas conservan el tipo del elemento (el
verificador no tiene genéricos, así que su firma se calcula desde los
argumentos). Para el texto: `largo(s)`, `contiene(s, sub)` y `minusculas(s)`.

```marea
@server
fn buscar(q: String, i: Int) -> List<Producto> {
    let ps = todos();
    if i < len(ps) {
        let p = ps[i];
        let resto = buscar(q, i + 1);
        if contiene(minusculas(p.titulo), minusculas(q)) {
            return unir([p], resto);   // el filtrado como DATOS, no como HTML
        }
        return resto;
    }
    return [];
}
```

### El escapado no es opcional: el tipo `Html`

El sumidero del DOM (`render`) solo acepta `Html`, y a `Html` solo se llega por
tres caminos: `escapar(x)`, un literal del propio fuente (lo escribiste tú, es
de confianza por construcción) o `html(s)`, la confianza explícita que se ve en
una revisión de código. Un `String` que venga del store o de la red **no** es
`Html`, así que incrustarlo sin escapar no compila:

```marea
render(concat("<li>", p.texto))            // error: se esperaba 'Html'
render(concat("<li>", escapar(p.texto)))   // ✅
```

`Html` es subtipo de `String` (el marcado seguro vale donde va texto) pero no al
revés — ahí está la garantía. `aTexto` de un número o un booleano ya es `Html`,
porque no pueden contener marcado; el de un `String`, no. En tiempo de ejecución
`Html` es una cadena: la distinción es puramente estática y no cuesta nada.

`Unknown` (el comodín que absorbe errores) es el único tipo que **no** satisface
`Html`: si no, un `Record`, un campo de tipo abierto o un `match` con ramas de
tipos distintos lavarían cualquier dato hasta el DOM. Y `Html` no vale como
parámetro de una función `@server`: la confianza no cruza la red, porque al otro
lado del cable la reconstruye quien mande el JSON.

Los dos sumideros del DOM están cubiertos: `render`, y el retorno de `vista` —la
función que `marea build-app` monta en la página—, que debe declararse `-> Html`.

**Lo que `escapar` no cubre:** escapa `& < > " '`, que basta en contexto de
texto y de atributo entrecomillado. **No** basta dentro de un atributo sin
comillas ni en un `href="javascript:..."`. Si construyes esos contextos, el tipo
`Html` no te salva: revísalos a mano.

### Las uniones llevan etiqueta

Una variante nominal se representa con un campo reservado: `NotFound` viaja como
`{ $tag: "NotFound" }`. El lexer no admite `$` en un identificador, así que
**ningún registro del programa puede tener ese campo** y hacerse pasar por una
variante. Antes una variante era una cadena desnuda y el discriminante miraba
campos de datos corrientes (`tag`, `kind`, `type`): un registro con un campo
`tag` decidía qué rama del `match` corría —el dato controlaba el flujo—, y el
`String` `"NotFound"` era indistinguible de la variante.

Queda un límite explícito: una variante que resuelve a un **registro** no lleva
etiqueta, así que no puede nombrarse en una rama (`E_VARIANTE_SIN_ETIQUETA`); se
cubre con un comodín, que es como ya lo hacían los ejemplos.

### Los dos backends significan lo mismo (con una excepción)

Un lenguaje con dos blancos corre el riesgo de que el mismo programa signifique
dos cosas. Hay una **prueba diferencial** (`crates/marea-wasm/tests/diferencial.rs`)
que compila el mismo `.mar` a TypeScript y a WebAssembly, ejecuta ambos y compara
resultados. Cuando se escribió encontró siete divergencias reales, todas ya
cerradas: la igualdad de cadenas comparaba punteros en WASM, `&&`/`||` no
cortocircuitaban, el indexado no comprobaba rango —y un índice negativo leía
memoria ajena— y la división entre cero daba `Infinity` dentro de un `Int`.

> **La excepción, que sigue abierta:** `Int` es **i32** en WASM y un entero de 53
> bits en TypeScript, así que al desbordar los dos blancos discrepan
> (`2000000000 + 2000000000` da `4000000000` en TS y `-294967296` en WASM).
> Mientras el backend WASM sea para núcleos numéricos, mantén los enteros dentro
> del rango de 32 bits.

Lo que **todavía no** existe: en el **backend WASM**, flotantes, `match`, tipos
unión y registros inline (cada caso falla con un error claro, nunca con WAT
roto). En el **lenguaje**, cierres/lambdas, genéricos en funciones, `import` y el
operador de propagación de errores.

### La frontera de red, corriendo de verdad

```sh
marea build examples/saludo.mar /tmp/demo   # transpila a TypeScript
node /tmp/demo/demo.ts                       # arranca servidor + cliente
# → [marea] servidor escuchando en http://127.0.0.1:8787/__marea
# → Hola desde el servidor, Marea
```

Un solo `.mar` con `saludar` (`@server`) y `main` (`@client`). Al correr, `main()`
llama a `saludar()` como si fuera local; por debajo viaja un `fetch` real por
HTTP al servidor. **Cero capa de API escrita a mano.**

> **Requiere Node ≥ 22.18 (o ≥ 23.6).** La salida del transpilador son archivos
> `.ts` que se ejecutan **directamente** con `node archivo.ts`, apoyándose en el
> *type stripping* nativo de Node (sin `tsc`, sin `ts-node`, sin paso de build).
> En versiones anteriores hay que activarlo con `--experimental-strip-types`.

### La misma fuente, ahora como WebAssembly

```sh
marea build-wasm examples/math.mar /tmp/math.wat   # .mar -> WAT
wat2wasm /tmp/math.wat -o /tmp/math.wasm            # WAT -> .wasm (116 bytes)
node -e 'const b=require("fs").readFileSync("/tmp/math.wasm");
  WebAssembly.instantiate(b).then(({instance})=>
    console.log("fib(20)=", instance.exports.fib(20)))'
# → fib(20)= 6765
```

La lógica corre en el motor WebAssembly **sin una línea de JavaScript**. Es el
camino para que el JS quede reducido a un pegamento mínimo (DOM/red).

Las **cadenas** también corren en WASM, sobre memoria lineal:

```sh
marea build-wasm examples/texto.mar /tmp/texto.wat
wat2wasm /tmp/texto.wat -o /tmp/texto.wasm
# saludar() -> "Hola desde WebAssembly 🌊", concatenado dentro del módulo WASM
# (allocador bump + memory.copy), leído por el host desde la memoria exportada.
```

## Estructura

```
crates/
  marea-syntax/   # lexer, AST, parser, errores  (la biblioteca)
  marea-types/    # verificador de tipos (nombres, tipos, ubicación, unión)
  marea-codegen/  # transpilador a TypeScript + runtime RPC
  marea-wasm/     # backend a WebAssembly (WAT)
  marea-lsp/      # servidor de lenguaje (LSP) — deps externas aisladas aquí
  marea-cli/      # binario `marea`
examples/         # programas .mar de muestra (+ check_fail/ que deben fallar)
examples/tienda.mar  # el ejemplo grande: un marketplace tipo Mercado Libre
editors/vscode/   # extensión de VSCode: cliente LSP + resaltado TextMate
site/             # la app de demo desplegable (salida de `marea build-app` + Dockerfile)
docs/GRAMMAR.md   # la gramática de v0
docs/COMPARACION.md  # la misma app en 5 stacks (Marea vs React/tRPC, LiveView, Leptos, Livewire)
docs/BENCH.md        # tiempos: compilación + kernel CPU (Marea→WASM ≈ nativo)
scripts/bench.sh     # el benchmark de tiempos, reproducible
```

`site/` es la demo tal cual se despliega: `site/marea-demo.mar` es la fuente y
`site/app/` la salida de `marea build-app` más un `Dockerfile` (Node 26) para
subirla a un contenedor.

¿Por qué un lenguaje nuevo y no una librería? La respuesta en código está en
**[docs/COMPARACION.md](docs/COMPARACION.md)**: la misma app ("X-mini": timeline
con likes, servidor↔cliente, persistencia) escrita lado a lado en cinco stacks,
con el recuento honesto de archivos, líneas y **dónde vive cada frontera** (en el
lenguaje, en un framework, o en tu pegamento).

## Uso

Herramientas necesarias: **Rust** (para construir el compilador), **Node ≥ 22.18**
(o ≥ 23.6, para correr los `.ts` generados sin paso de build) y **`wat2wasm`** del
paquete [wabt](https://github.com/WebAssembly/wabt) para ensamblar el `.wat`.

```sh
# Ver los tokens de un archivo
cargo run --bin marea -- tokens examples/user.mar

# Ver el AST
cargo run --bin marea -- parse examples/user.mar

# Verificar los tipos (incluye la regla unión-opaca + match exhaustivo)
cargo run --bin marea -- check examples/user.mar
cargo run --bin marea -- check examples/check_fail/match_no_exhaustivo.mar  # falla a propósito

# Transpilar a TypeScript y correr la demo de la frontera de red
cargo run --bin marea -- build examples/saludo.mar /tmp/demo
node /tmp/demo/demo.ts

# Compilar a WebAssembly y ejecutar sin JS
cargo run --bin marea -- build-wasm examples/math.mar /tmp/math.wat
wat2wasm /tmp/math.wat -o /tmp/math.wasm

# Reactividad: un efecto que se re-ejecuta solo al cambiar una fuente
cargo run --bin marea -- build examples/contador.mar /tmp/c
node /tmp/c/demo.ts   # imprime 0, 2, 4 (el effect reacciona a cada n = n + 1)

# App web completa (RPC + reactivo + DOM) en una sola página
cargo run --bin marea -- build-app examples/web-likes.mar /tmp/app
node /tmp/app/serve.ts   # abre http://127.0.0.1:8787

# Los build* verifican tipos antes de emitir; para saltarse esa verificación:
cargo run --bin marea -- build examples/saludo.mar /tmp/demo --no-check

# Pruebas y linter
cargo test
cargo clippy --all-targets
```

## Persistencia: almacenes con nombre y backends intercambiables

El estado del servidor se declara con `store nombre: T;` y se opera con cuatro
builtins (CRUD) que reciben el almacén como primer argumento: `guardar(a, x)`,
`todos(a)`, `actualizar(a, i, x)` y `borrar(a, i)`. **Un módulo puede declarar
todos los almacenes que necesite** —cada uno con su tipo, su tabla y su
archivo—, que es lo que separa una app real de una demo. El código `.mar` no
sabe **dónde** vive ese estado: el backend se elige al correr, con variables de
entorno, sin tocar el lenguaje.

```mar
type Producto = { titulo: String, precio: Int };
type Orden = { comprador: String, total: Int };

store productos: Producto;
store ordenes: Orden;

@server fn publicar(t: String, p: Int) { guardar(productos, Producto { titulo: t, precio: p }); }
@server fn catalogo() -> List<Producto> { return todos(productos); }
@server fn ventas() -> List<Orden> { return todos(ordenes); }
```

Guardar en el almacén equivocado es un error de tipos: `guardar(ordenes, p)` con
un `Producto` no compila. `examples/tienda.mar` usa cuatro (`productos`,
`ordenes`, `preguntas`, `resenas`).

```sh
# Por defecto: archivo (log JSONL append-only, cero dependencias)
node /tmp/x/demo.ts

# SQLite (módulo integrado de Node, cero dependencias)
MAREA_DB=sqlite MAREA_DB_URL=marea.sqlite node /tmp/x/demo.ts

# PostgreSQL / MySQL / MongoDB (requieren su driver instalado)
MAREA_DB=postgres MAREA_DB_URL=postgres://user:pass@host/db node /tmp/x/demo.ts   # npm i pg
MAREA_DB=mysql    MAREA_DB_URL=mysql://user:pass@host/db    node /tmp/x/demo.ts   # npm i mysql2
MAREA_DB=mongodb  MAREA_DB_URL=mongodb://host/db            node /tmp/x/demo.ts   # npm i mongodb
```

**Lo que la validación del límite NO cubre**, dicho sin rodeos: no hay
autenticación de ninguna clase —todo `@server` es invocable por quien alcance el
puerto, y `MAREA_HOST=0.0.0.0` lo expone a la red—; las uniones solo se
comprueban como "no nulo", porque todavía no hay discriminante de runtime; un
tipo recursivo se valida a profundidad 1; y las comprobaciones de `Origin`/`Host`
son defensa en profundidad contra navegadores (`MAREA_ALLOWED_ORIGINS` y
`MAREA_ALLOWED_HOSTS` las ajustan), no un sustituto de autenticar.

Cada almacén va a su propia tabla (o a su propio archivo,
`marea-store.<nombre>.log` bajo `MAREA_STORE_DIR`, que por defecto es el
directorio actual). El codegen deriva el esquema (tabla + columnas tipadas) del
tipo de cada almacén: un
registro produce una columna por campo; un escalar/lista/unión se guarda como una
sola columna JSON `__doc`. Los drivers externos (`pg`, `mysql2`, `mongodb`) se
cargan con `import()` perezoso, así que un programa sin base de datos —o con
`file`/`sqlite`— no necesita instalarlos.

**Persistencia incremental por id:** el arreglo en memoria tiene índices
posicionales estables y un id persistente paralelo; cada mutación toca **una sola
fila** (`insert`/`update`/`remove` por id) — O(1), sin reescribir el store
completo. En SQL/Mongo es un statement puntual; el backend de archivo es un log
append-only (compactado al cargar).

**Endurecimiento del endpoint RPC** (`/__marea`): escucha solo en `127.0.0.1`
(ampliable con `MAREA_HOST`), rechaza cuerpos sobre `MAREA_MAX_BODY` (1 MiB) con
`413`, valida forma y aridad de los argumentos, no refleja errores internos al
cliente (solo `error interno` + log en servidor) y usa una tabla de handlers sin
prototipo. Los identificadores SQL se comillan por dialecto.


| `MAREA_DB` | Backend         | Dependencia        | `MAREA_DB_URL`              |
| ---------- | --------------- | ------------------ | -------------------------- |
| `file` (def) | Archivo JSON  | ninguna            | (usa `MAREA_STORE`)        |
| `sqlite`   | `node:sqlite`   | ninguna            | ruta del archivo `.sqlite` |
| `postgres` | `pg`            | `npm i pg`         | cadena de conexión         |
| `mysql`    | `mysql2/promise`| `npm i mysql2`     | cadena de conexión         |
| `mongodb`  | `mongodb`       | `npm i mongodb`    | cadena de conexión         |

## App web de verdad: las dos fronteras tocando el DOM

`marea build-app <archivo.mar> [dir]` genera una **app web completa** donde los
dos pilares de Marea trabajan juntos en una página: las `@server` se llaman por
RPC desde el navegador (frontera de **red**) y el estado `reactive` de módulo
re-pinta el DOM solo cuando cambia (frontera del **tiempo**). Sin React, sin
fetch a mano.

```marea
reactive mut posts = [];                  // estado de app (signal de módulo)

@server fn like(i: Int) { /* … persiste … */ }
@server fn feed() -> List<Post> { return todos(); }

@client fn vista() -> String { /* lee 'posts' y devuelve HTML */ }
@client fn darLike(i: Int) { like(i); posts = feed(); }  // RPC → reactivo → DOM
```

```sh
marea build-app examples/web-likes.mar /tmp/app
node /tmp/app/serve.ts        # abre http://127.0.0.1:8787
```

Genera `index.html` + `client.js` (navegador: cliente RPC + núcleo reactivo +
render al DOM, **sin Node ni tipos**) y `runtime.ts`/`server.ts`/`serve.ts`
(servidor Node: RPC + store + estáticos en el **mismo origen**, sin CORS). Una
`reactive mut` de nivel superior es el estado compartido entre la vista y los
manejadores; `vista()` se monta en un `effect` que re-pinta `#app` al cambiar.

## Hoja de ruta

- [x] **v0** — Lexer + AST + Parser + CLI
- [x] **v2** — Transpilación a TypeScript con cruce de frontera (`@server`/`@client`)
- [x] **WASM (numérico)** — Backend a WebAssembly: i32, `let`, `if`, llamadas
- [x] **WASM (cadenas)** — Strings sobre memoria lineal: literales + `concat`
- [x] **WASM (structs)** — Registros sobre memoria lineal: construir + leer campos
- [x] **v1.5** — Verificador de tipos (`marea check`): nombres, tipos, ubicación, unión+match
- [x] **WASM (listas)** — Listas sobre memoria + indexado `xs[i]`, tipadas `List<T>`
- [x] **v3 (reactividad)** — `reactive mut`/`reactive`/`effect` → signals/memo/effect en TS
- [x] **LSP** — Servidor de lenguaje: diagnósticos en vivo, symbols, completion, definition, hover
- [x] **Recuperación de errores** — el parser reporta múltiples diagnósticos (no fail-fast)
- [x] **glue DOM (web)** — `marea build-web` genera una app WASM para el navegador
- [x] **Persistencia** — `store nombre: T;` (varios por módulo) con backends intercambiables: archivo JSON,
      SQLite, PostgreSQL, MySQL y MongoDB (elegidos por `MAREA_DB`, sin cambiar el `.mar`)
- [x] **App web (RPC + reactivo + DOM)** — `marea build-app` genera una página real
      donde las `@server` se llaman por RPC y el estado `reactive` de módulo re-pinta el DOM

## Licencia

MIT
