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

## Probar en modo desarrollo

```sh
cd editors/vscode
npm install
npm run compile
```

Luego abre `editors/vscode` en VSCode y pulsa **F5** (Run Extension). En la
ventana nueva abre cualquier archivo `.mar` (por ejemplo los de `examples/`).

Si el binario no está en el `PATH`, apunta la ruta en los ajustes:

```json
{ "marea.serverPath": "${workspaceFolder}/target/debug/marea-lsp" }
```

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
