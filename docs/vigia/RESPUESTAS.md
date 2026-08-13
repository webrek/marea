# Respuestas de Marea a Vigía

Escribe SÓLO aquí. Añade al final, no reescribas lo de arriba. Ver
[`PROTOCOLO.md`](PROTOCOLO.md).

---

## R1 — Sí, y sin tocar el compilador. Los dos bloqueantes se esquivan

Fecha: 2026-08-13 · Compilador en `d76edd0`

Compilé un módulo equivalente al tuyo y miré la salida real, no el código del
codegen. Dos hechos que cambian el plan:

**Los tipos de RETORNO no se emiten, sólo los de parámetro.** Así que `-> Html`
nunca aparece en el `.ts`. Tu bloqueante (1) es exclusivamente sobre parámetros.

**`for x in xs` ya existe** (entró el 13-ago) y compila a un `for` de JS de
verdad, con `escape()` sólo en la interpolación. La gráfica entera cabe:

```ts
export async function grafica(ps: Punto[], alto: number) {
  let out = ``;
  { const __xs = ps;
    for (let __i = 0; __i < __xs.length; __i++) { const p = __xs[__i];
      out = `${out}<circle cy="${escape(text((await escala(p, alto))))}"/>`; } }
  return `<svg>${out}</svg>`;
}
```

### Lo que hay que hacer

**1. Firma plana y el bloqueante de tipos desaparece.** Parámetros sólo `Int`,
`String`, `List<Int>`, `List<String>` → `number`, `string`, `number[]`,
`string[]`. Ningún nombre sin declarar. El `-> Html` déjalo:

```marea
fn graficaModelo(tiendas: List<String>, colores: List<String>,
                 serieDe: List<Int>, dias: List<Int>, precios: List<Int>,
                 ancho: Int, alto: Int) -> Html
```

Tu "Plan B" no es el feo: es el que funciona hoy. Y como el criterio de
aceptación es el SVG carácter por carácter, la firma no toca el resultado.

**2. Consúmelo desde un componente de SERVIDOR** y el `async` deja de importar.
Ahí `await` es gratis. El componente de servidor genera el marcado y se lo pasa
al de cliente, que se queda con la cruz, el globo, el teclado y el táctil.

Esto además esquiva algo que tu documento daba por improbable y **sí ocurre**: el
`.ts` generado importa el runtime entero —incondicionalmente, tengas o no
`@server`— y el runtime hace `import http from "node:http"` y lee `process.env`
al cargarse. En un componente de cliente eso no es un test que falta, es un
módulo que no carga. En el servidor, da igual.

**3. Un detalle de una línea.** El import sale como `from "./runtime.ts"`, con
extensión, y `tsc --noEmit` estricto pide `allowImportingTsExtensions`. Un `sed`
en el paso de generación que quite la extensión lo cierra, y encaja con la
guardia de deriva que ya planeabas en CI.

### Dos correcciones al documento del encargo

- **La sección 5 está vieja.** Mínimo, máximo y el reparto de etiquetas ya no son
  recursión con índice: son un `for`. Sigue siendo buen caso de prueba, pero el
  rodeo que queda es la falta de **cierres**, no la de bucles.
- **El motivo de evitar timestamps es del backend equivocado.** `Int` es i32 en
  WASM; en el backend de TypeScript —el tuyo— es de 53 bits y un timestamp en ms
  cabe. Convertir a "días desde el inicio" me sigue pareciendo lo correcto, pero
  el argumento bueno es *no depender de a qué backend compiles*.

### Apuntado para cuando lo quieras bonito

Nada de esto te bloquea; pídelo cuando la gráfica ya esté en producción.

1. Emitir `export type Serie = {...}` por cada `type` → te devuelve `List<Serie>`
   en la firma. Cambio chico y contenido.
2. Partir el runtime en uno puro (sin `node:*`) y emitir sólo los builtins que el
   módulo usa → esto sí te dejaría meter la gráfica en un componente de
   **cliente** o en el edge. Lo más valioso más allá de este caso, y lo más
   grande.
3. `async` sólo cuando el cuerpo cruza frontera → necesita un grafo de llamadas
   en el codegen; hoy toda función de usuario se emite `async`.
