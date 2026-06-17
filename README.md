# Marea 🌊

Un lenguaje de programación enfocado a la web. Su tesis: volver **primitivas del
lenguaje** las dos fronteras que hoy se cruzan a mano y con pegamento.

1. **La frontera de red** (cliente ↔ servidor). Una función lleva una anotación
   de ubicación (`@server`, `@client`, `@edge`) y el compilador genera el cruce:
   RPC, serialización, validación del límite y tipos de ambos lados. No hay
   "capa de API" que escribir.
2. **La frontera del tiempo** (reactividad). Una variable `reactive` propaga sus
   cambios; el grafo de dependencias lo conoce el compilador, no una librería.

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
- **CLI** `marea` con `tokens`, `parse` y `build`.

Lo que **todavía no** existe: chequeo de tipos y resolución de nombres robustos,
el modelo reactivo en runtime, y el backend WASM (siguiente fase).

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

## Estructura

```
crates/
  marea-syntax/   # lexer, AST, parser, errores  (la biblioteca)
  marea-codegen/  # transpilador a TypeScript + runtime RPC
  marea-cli/      # binario `marea`
examples/         # programas .mar de muestra
docs/GRAMMAR.md   # la gramática de v0
```

## Uso

```sh
# Ver los tokens de un archivo
cargo run --bin marea -- tokens examples/user.mar

# Ver el AST
cargo run --bin marea -- parse examples/user.mar

# Transpilar a TypeScript y correr la demo de la frontera de red
cargo run --bin marea -- build examples/saludo.mar /tmp/demo
node /tmp/demo/demo.ts

# Pruebas y linter
cargo test
cargo clippy --all-targets
```

## Hoja de ruta

- [x] **v0** — Lexer + AST + Parser + CLI
- [x] **v2** — Transpilación a TypeScript con cruce de frontera (`@server`/`@client`)
- [ ] **WASM** — Backend a WebAssembly (cortar la dependencia de JS) ← siguiente
- [ ] **v1.5** — Resolución de nombres + chequeo de tipos (con tipos de ubicación)
- [ ] **v3** — Modelo reactivo en runtime
- [ ] **LSP** — Servidor de lenguaje para el editor

## Licencia

MIT
