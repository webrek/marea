#!/usr/bin/env bash
# Pasa por `tsc` el TypeScript que Marea escribe a mano y el que genera, y por
# `tsc --checkJs` el JavaScript que sirve al navegador.
#
# POR QUÉ EXISTE. `runtime.ts` son ~1200 líneas de TypeScript que se copian tal
# cual a cada proyecto generado, y nunca habían pasado por el compilador de
# TypeScript: lo escribíamos como TS y lo tratábamos como texto. Los tests lo
# ejecutan con `node`, que no mira tipos, así que dos fallos de portada vivieron
# en él sin que nada los viera —el builtin `fetch` tapando al del entorno, y
# `.append()` sobre un `Set`—. Los dos los encontró un consumidor externo que sí
# tenía `tsc` encima; los dos los señala esto, con línea exacta.
#
# NO reemplaza a los tests: `tsc` comprueba tipos, no comportamiento. La suite
# ejecuta las demos y compara su salida; esto mira lo que la ejecución no puede.
#
# Vive fuera de `cargo test` a propósito: `cargo test` sigue necesitando sólo
# Rust y node, sin `npm install`. Esto corre en CI, y a mano cuando quieras.
set -euo pipefail

aqui="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
raiz="$(cd "$aqui/../.." && pwd)"
src="$raiz/crates/marea-codegen/src"
tsc="$aqui/node_modules/.bin/tsc"

if [ ! -x "$tsc" ]; then
  echo "instalando el comprobador (una vez)…"
  (cd "$aqui" && npm install --silent --no-audit --no-fund)
fi

# `--typeRoots` explícito: tsc se invoca desde la raíz del repo, así que por sí
# solo no encontraría los @types que viven aquí abajo.
comun=(--noEmit --target ES2022 --module ESNext --moduleResolution bundler
       --lib ES2022,DOM --typeRoots "$aqui/node_modules/@types")

# `--allowJs` para que resuelva el `import` de `nucleo.js`, que es el núcleo
# compartido por los dos runtimes. Sin `--checkJs` no se comprueba ahí dentro:
# de eso se encarga el paso siguiente (como JavaScript) y el de la salida (como
# TypeScript, con las anotaciones ya destapadas).
echo "==> runtime.ts (estricto)"
"$tsc" "${comun[@]}" --strict --allowJs --types node "$aqui/ambient.d.ts" "$src/runtime.ts"

# `browser.js` y `nucleo.js` son JavaScript sin anotar, así que `strict` daría 36
# avisos de `implicitly any` que son ruido, no defectos. Sin él, `checkJs` sigue
# viendo lo que importa: llamar a un método que el objeto no tiene. Con el bug de
# `.append` reintroducido, esto lo caza y no dice nada más.
#
# Que `nucleo.js` pase por aquí es la mitad que importa: es el texto que se sirve
# al navegador tal cual, y lo comprueba COMO JAVASCRIPT, que es como se ejecuta.
# La otra mitad —que sus anotaciones `/*ts ... */` digan la verdad— la comprueba
# el paso de la salida, donde ese mismo texto llega ya destapado a `runtime.ts` y
# pasa por `--strict` entero.
echo "==> browser.js + nucleo.js (checkJs, sin exigir anotaciones)"
"$tsc" "${comun[@]}" --allowJs --checkJs --noImplicitAny false \
  "$src/browser.js" "$src/nucleo.js"

# Y la SALIDA, que es lo que acaba en el proyecto de otro. Se comprueban dos
# ejemplos que entre los dos cubren los dos caminos del codegen: con `store` (el
# runtime entero) y sin él (el recortado, que es el que se puede empaquetar).
# Aquí el `runtime.ts` generado ya lleva el núcleo compartido DENTRO, con sus
# anotaciones destapadas, y `tsc` lo comprueba como parte del programa: es donde
# se cae si una de esas anotaciones miente.
echo "==> la salida generada"
marea="$raiz/target/release/marea"
if [ ! -x "$marea" ]; then
  echo "   falta $marea — compila con: cargo build --release" >&2
  exit 1
fi
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
for ejemplo in sesion.mar listas.mar; do
  salida="$tmp/${ejemplo%.mar}"
  "$marea" build "$raiz/examples/$ejemplo" "$salida" >/dev/null
  # `allowImportingTsExtensions` porque el codegen importa "./runtime.ts" con
  # extensión: node lo exige para ejecutar los .ts sin paso de build.
  "$tsc" "${comun[@]}" --strict --types node --allowImportingTsExtensions \
    "$aqui/ambient.d.ts" "$salida/client.ts" "$salida/server.ts"
  echo "   $ejemplo ✓"
done

echo "todo el TypeScript comprueba limpio."
