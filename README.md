# Marea 🌊

Un lenguaje de programación enfocado a la web. Su tesis: volver **primitivas del
lenguaje** las dos fronteras que hoy se cruzan a mano y con pegamento.

1. **La frontera de red** (cliente ↔ servidor). Una función lleva una anotación
   de ubicación (`@server`, `@client`, `@edge`) y el compilador genera el cruce:
   RPC, serialización, validación del límite y tipos de ambos lados. No hay
   "capa de API" que escribir.
2. **La frontera del tiempo** (reactividad). Una variable `reactive` propaga sus
   cambios; el grafo de dependencias lo conoce el compilador, no una librería.
   `reactive mut` es una fuente (signal), `reactive` una derivada (memo), y
   `effect { ... }` se re-ejecuta solo cuando cambia algo que leyó.

```marea
type UserId = Int;

@server
fn getUser(id: UserId) -> User | NotFound {
    db.users.find(id)
}

@client
fn perfil(id: UserId) {
    reactive usuario = getUser(id);   // se llama como si fuera local
    match usuario {
        NotFound => render("no existe"),
        _        => render(usuario.nombre),
    }
}
```

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
- **CLI** `marea` con `tokens`, `parse`, `check`, `build`, `build-wasm` y `build-web`.

Lo que **todavía no** existe: chequeo de tipos y resolución de nombres robustos,
el modelo reactivo en runtime, y WASM para tipos no numéricos (cadenas/structs,
que requieren memoria lineal).

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
docs/GRAMMAR.md   # la gramática de v0
```

## Uso

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

# Pruebas y linter
cargo test
cargo clippy --all-targets
```

## Persistencia: un `store` con backends intercambiables

El estado del servidor se declara con `store T;` y se opera con cuatro builtins
(CRUD): `guardar(x)`, `todos()`, `actualizar(i, x)` y `borrar(i)`. El código
`.mar` no sabe **dónde** vive ese estado: el backend se elige al correr, con
variables de entorno, sin tocar el lenguaje.

```mar
type Post = { autor: String, texto: String, likes: Int };
store Post;

@server fn publicar(a: String, t: String) { guardar(Post { autor: a, texto: t, likes: 0 }); }
@server fn feed() -> List<Post> { return todos(); }
```

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

El codegen deriva el esquema (tabla + columnas tipadas) del tipo del `store`: un
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
- [x] **Persistencia** — `store T;` con backends intercambiables: archivo JSON,
      SQLite, PostgreSQL, MySQL y MongoDB (elegidos por `MAREA_DB`, sin cambiar el `.mar`)

## Licencia

MIT
