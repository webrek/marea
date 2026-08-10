# Marea para VSCode

Soporte del lenguaje **Marea** en Visual Studio Code: diagnósticos en vivo
(errores de parser y de tipos), símbolos del documento, autocompletado, ir a la
definición y hover. Toda la inteligencia vive en el servidor `marea-lsp` (Rust);
esta extensión es el cliente LSP + el resaltado de sintaxis.

## Requisitos

El binario del servidor de lenguaje:

```sh
# desde la raíz del repo
cargo build -p marea-lsp        # genera target/debug/marea-lsp
```

### Que la extensión encuentre el binario

Por defecto el ajuste `marea.serverPath` vale `"marea-lsp"` **a secas**, así que
la extensión lo busca en el `PATH`. `cargo build` **no** lo deja ahí: solo escribe
`target/debug/marea-lsp`. Elige una de estas tres:

1. **Instalarlo en el `PATH`** (lo copia a `~/.cargo/bin`, que el instalador de
   Rust ya suele añadir al `PATH`); desde la raíz del repo:

   ```sh
   cargo install --path crates/marea-lsp
   ```

2. **Un enlace** al binario de desarrollo, para no reinstalar en cada cambio
   (desde la raíz del repo, quizá con `sudo`):

   ```sh
   ln -sf "$PWD/target/debug/marea-lsp" /usr/local/bin/marea-lsp
   ```

   O, solo para la sesión actual, `export PATH="$PWD/target/debug:$PATH"` y
   arrancar VSCode desde **esa misma** terminal con `code .` (si lo abres desde
   Finder/el Dock no hereda ese `PATH`).

3. **Apuntar la ruta a mano** en los ajustes, sin tocar el `PATH`:

   ```json
   { "marea.serverPath": "${workspaceFolder}/target/debug/marea-lsp" }
   ```

Si la extensión no logra arrancarlo, lo dice con un aviso que incluye la ruta que
intentó.

## Probar en modo desarrollo

```sh
cd editors/vscode
npm install
npm run compile
```

Luego abre `editors/vscode` en VSCode y pulsa **F5** (Run Extension). En la
ventana nueva abre cualquier archivo `.mar` (por ejemplo los de `examples/`).

## Empaquetar (.vsix)

```sh
npx @vscode/vsce package
```

## Qué provee

| Capacidad | Detalle |
|-----------|---------|
| Diagnósticos | errores de sintaxis y de tipos, en vivo al escribir |
| Símbolos | esquema de funciones y tipos del documento |
| Autocompletado | palabras clave, builtins, tipos y símbolos del módulo |
| Ir a definición | de un uso a su declaración (mismo archivo) |
| Hover | firma reconstruida del AST |
| Resaltado | gramática TextMate para `.mar` |
