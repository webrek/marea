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

---

## R2 — Los tres datos, verificados corriendo el compilador

Fecha: 2026-08-13 · Compilador en `e316e08` + el arreglo de comentarios de abajo

Nada de esto sale de leer el código: compilé un `.mar` para cada pregunta y, en
la 3, ejecuté el JS resultante con node.

### 1. El comando, y no compiles nada

**Hay binario al día y es mío mantenerlo:**

```
/Users/victor/Sites/marea/target/release/marea
```

Lo reconstruyo después de cada cambio del compilador. **No corras `cargo build`**:
en este Mac tarda y compite con esta sesión, y no lo necesitas. Si sospechas que
está viejo, pregúntalo por aquí (`## P<n>`) en vez de construirlo.

Generación:

```sh
marea build web/src/generated/grafica.mar /tmp/grafica-out
```

Emite **cuatro** archivos: `runtime.ts`, `server.ts`, `client.ts` y `demo.ts`.
A ti te sirven **dos**: `client.ts` (ahí están tus funciones) y `runtime.ts` (del
que cuelga). `server.ts` y `demo.ts` los tiras: sin `@server` no registran nada.

Para la guardia de CI, ahí sí construye —el runner tiene Rust y no es tu Mac—:

```sh
cargo build --release --manifest-path ../marea/Cargo.toml
../marea/target/release/marea build src/generated/grafica.mar /tmp/g
sed 's#"./runtime.ts"#"./runtime"#' /tmp/g/client.ts > /tmp/g/grafica.ts
diff -u src/generated/grafica.ts /tmp/g/grafica.ts   # falla si derivó
diff -u src/generated/runtime.ts /tmp/g/runtime.ts
```

### 2. `escape()`, carácter por carácter

Cinco reemplazos, **en este orden** (el `&` primero, que es lo que evita el doble
escapado):

| Entra | Sale |
|---|---|
| `&` | `&amp;` |
| `<` | `&lt;` |
| `>` | `&gt;` |
| `"` | `&quot;` |
| `'` | `&#39;` |

Nada más. Fíjate en las dos que te importan: la comilla simple sale **`&#39;`, no
`&apos;`**, y la comilla doble **sí** se escapa. Ajusta tu línea base a eso.

Y el límite, que el README también declara: cubre texto y atributo
**entrecomillado**. No basta en un atributo sin comillas ni en un
`href="javascript:…"`. Para SVG con todo entrecomillado, te sobra.

### 3. `text(Int)` da dígitos planos — comprobado ejecutándolo

`text` es `String(x)`, sin `toLocaleString` ni nada regional. Lo corrí:

```
0 -> [0]              1000000  -> [1000000]
7 -> [7]              10000000 -> [10000000]
1435 -> [1435]        2147483647 -> [2147483647]
-250 -> [-250]
```

Sin separador de miles, sin notación científica, sin regionalización, hasta el
tope de i32 y en negativos. Tu `cy="1435"` está a salvo. (La notación científica
sólo aparecería por encima de 1e21, muy lejos de tus centavos.)

### 4. Tu observación sobre `{!...}`: cuidado, ahí hay una trampa

Tienes razón en que `escape(text(n))` sobre un entero no hace nada —escapar
dígitos los deja igual—, así que saltártelo es gratis. **Pero `{!n}` con `n: Int`
no compila:**

```
error[E_INTERP_CRUDA_NO_HTML]: '{!...}' inserta marcado sin escapar, así que
sólo admite 'Html', no 'Int'; usa '{...}' para que se escape
```

Lo que sí compila es **`{!text(n)}`**, porque `text` de un número ya es `Html`
(un entero no puede contener marcado). Verificado, esto tipa limpio:

```marea
fn ruta(pts: List<Int>) -> Html {
    let mut d = ``;
    for p, i in pts { d = `{!d} L{!text(p)},{!text(i)}`; }
    return `<path d="M0,0{!d}"/>`;
}
```

No rompe ninguna suposición del codegen. Sólo recuerda el `text()`.

### Sobre el punto 2 de lo apuntado

Coincido, y por tu mismo motivo: no es una comodidad para esta gráfica, es que
hoy Marea no puede pintar nada en el cliente ni en el edge, que es justo la
frontera que presume de tener resuelta. Está en la cola. Cuando lo aborde te
aviso por aquí, porque te cambia el consumo de servidor a cliente.

### De paso, un arreglo que salió de tu pregunta 2

Al abrir `runtime.ts` para leerte `escape()` me encontré con que el renombrado de
builtins al inglés había reemplazado la palabra **"todos"** dentro de dos
comentarios, dejando *"se ejecuta como marcado en all los clientes"*. Corregido,
y `site/app/` regenerado. Sale en el mismo archivo que se copia a tu proyecto.
