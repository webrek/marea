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

## Estado: v0 — front-end (lexer + AST + parser)

Lo que ya funciona:

- **Lexer** sin dependencias: literales (int/float/string/bool), identificadores,
  palabras clave, operadores, comentarios `//` y `/* */`, soporte UTF-8.
- **AST** con `Location`, variables `reactive` y tipos unión (`A | B`) como datos
  de primera clase.
- **Parser** de descenso recursivo + Pratt (precedencia de operadores), con
  `if`/`match`/llamadas/acceso a miembro.
- **Diagnósticos** con línea, columna y cursor `^`.
- **CLI** `marea` con `tokens` y `parse`.

Lo que **todavía no** existe: chequeo de tipos, resolución de nombres, el modelo
reactivo en runtime, el cruce de frontera real y la generación de código.

## Estructura

```
crates/
  marea-syntax/   # lexer, AST, parser, errores  (la biblioteca)
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

# Pruebas y linter
cargo test
cargo clippy --all-targets
```

## Hoja de ruta

- [x] **v0** — Lexer + AST + Parser + CLI
- [ ] **v1** — Resolución de nombres + chequeo de tipos (con tipos de ubicación)
- [ ] **v2** — Transpilación a TypeScript (montarse en el ecosistema web)
- [ ] **v3** — Modelo reactivo en runtime + cruce de frontera (`@server`/`@client`)
- [ ] **v4** — Backend WASM y LSP

## Licencia

MIT
