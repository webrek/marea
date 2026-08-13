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

---

## R3 — Los tres comportamientos quedan atados con tests, no con una promesa

Fecha: 2026-08-13 · `crates/marea-codegen/tests/contrato_runtime.rs`

### Sobre el criterio de aceptación: tienes razón y el cambio es correcto

Cambiar "carácter por carácter" por "etiquetas idénticas como texto + geometría
dentro de 0.1 unidades" no es aflojar el listón, es medir lo que de verdad
distingue las dos implementaciones. Perseguir el cero habría sido perseguir la
diferencia entre truncar enteros y redondear flotantes, que no es un defecto:
es el método. Cambiar el criterio al descubrir que era inalcanzable **por
construcción** —y dejarlo escrito para quien lea sólo el documento viejo— es la
decisión correcta.

Y que los ejes te salgan idénticos a un `Intl.NumberFormat("es-MX")` con `/` y
`%` escritos a mano coincido en que es el mejor resultado del día.

### Lo que pediste: aviso antes de tocar los tres comportamientos

Un aviso depende de que alguien se acuerde, así que en vez de prometértelo lo
he **fijado con tests**. Si alguno de los tres cambia, el CI se pone rojo y el
aviso sale solo:

| Atado | Qué fija |
|---|---|
| `text_de_un_entero_son_digitos_planos` | dígitos planos de `-2147483648` a `2147483647` |
| `la_division_entera_trunca_hacia_cero` | `-7/2 = -3`, **no** `-4`; y el resto conserva el signo del dividendo |
| `escape_hace_cinco_reemplazos_con_el_ampersand_primero` | los cinco, el orden, y que la comilla simple es `&#39;` y no `&apos;` |

Cambiarlas sigue siendo legítimo: hay que editar ese archivo a propósito y
decírtelo. Lo que ya no se puede es cambiarlas **sin enterarse**. El comentario
de cabecera dice por qué existe y que es tu gráfica la que cuelga de ahí.

**Dos cosas que salieron al atarlo y te afectan:**

1. **La división trunca hacia cero, no hacia abajo.** `-7/2` es `-3`. Si tu
   escala mete negativos —un precio por debajo del mínimo del eje, por ejemplo—,
   ahí hay medio píxel de diferencia con un `Math.floor`. Es además lo que hace
   `i32.div_s` en el backend WASM, así que la elección es deliberada: el mismo
   programa no puede dibujar distinto según a qué blanco compiles.
2. **`__div(-1, 2)` da `-0`.** `text(-0)` imprime `"0"`, así que a tu SVG no le
   llega nunca un `-0`. Está fijado en el test, no vaya a ser.

### Sobre el ordenar sin cierres ni `sort`

Sí, cuenta como señal, y la apunto. Pero fíjate en cuál es: no te falta `sort`
—un builtin lo taparía—, te faltan **cierres**, porque sin ellos no puedes pasar
el criterio de orden. Es el mismo hueco que hace que no haya `map` ni `filter`.
Tu conteo de rangos con dos bucles anidados es exactamente el rodeo que documenta
`tienda.mar`, ahora con `for` en vez de recursión.

No lo voy a resolver para desbloquearte, porque dices que no te bloquea, y meter
cierres es una decisión de diseño del lenguaje, no un parche para una gráfica.
Pero es el segundo caso real que apunta ahí, y eso pesa.

### Estado de lo tuyo, desde aquí

Nada pendiente por mi lado. Cuando ataques las etiquetas pegadas a la línea y el
estado vacío, si algo se tuerce, `## P4`.

---

## R4 — Arreglado en el compilador. Tira el parche

Fecha: 2026-08-13 · `56c1c5b`..

Tenías razón y **corriges algo que dije mal en R1**: dije que en el servidor el
runtime "da igual" porque `node:http` es de Node. Cierto para `node:http`. Falso
para `pg`, `mysql2` y `mongodb`, que son paquetes de npm que nadie instaló. Un
empaquetador resuelve el especificador antes de saber si el código se ejecuta, y
eso no lo arregla ningún `serverExternalPackages`. Tu tabla de intentos ahorró
el camino: gracias por incluirla.

### Lo que hace ahora el compilador

Tu versión pequeña, implementada: **el runtime sólo trae la capa de persistencia
si el módulo declara algún `store`.** Tu `.mar` no declara ninguno, así que las
tres líneas ya no existen. Comprobado con un módulo como el tuyo:

```
runtime.ts:  1183 líneas  ->  722
drivers de npm mencionados:  0
import { __register, __badRequest, __rpc, print, concat, ... }   // sin __store/save/all
```

La lista de importación también se ajusta: pedir `__store` de un runtime que ya
no lo exporta habría cambiado un error del empaquetador por otro.

Lo hice con marcadores explícitos (`@marea:store-inicio` / `@marea:store-fin`) y
no con rangos de líneas, porque la sección no es contigua —el bloque de red
saliente y el de JSON están intercalados— y un recorte por números se habría
roto al siguiente cambio del runtime.

Hay dos tests que lo fijan, en los dos sentidos: sin `store` no puede aparecer
ninguno de los cuatro drivers; con `store`, tienen que estar los cuatro. El
comentario del primero cuenta tu caso, para que nadie lo "simplifique" dentro de
seis meses sin saber qué rompe.

### El segundo parche también, y era de una línea

`__index` devolvía `unknown`. Ahora es genérico:

```ts
export function __index<T>(xs: T[], i: number): T
```

Tal cual lo propusiste. Quita el `@ts-nocheck` del cliente generado.

### Qué significa para ti

**El post-proceso vuelve a ser el `sed` de la extensión.** Ya no hay que
arrancarle líneas a un archivo generado, así que la guardia de deriva de CI
vuelve a poder comparar contra lo commiteado sin excepciones — que era tu
objeción de fondo, y la correcta: un parche manual dentro de la guardia es
exactamente lo que se rompe en silencio dentro de seis meses.

Regenera y quita el `throw`.

### Lo que esto NO es todavía

Es la mitad pequeña del punto 2. El runtime recortado **sigue importando
`node:http` y `node:fs`** y leyendo `process.env` al cargarse, así que sigue
siendo de servidor: en un componente de cliente o en el edge todavía no entra.
Para eso hace falta el corte entero (un núcleo puro + un runtime de servidor,
emitiendo sólo los builtins que el módulo usa), que sigue en la cola.

Tu caso lo movió de "cuando esté bonito" a "esto separa de producción", y tenías
razón en el diagnóstico. Pero lo que te desatasca hoy es esto.

### Al encenderlo salieron dos tests que mentían

Dos pruebas del store construían un módulo que usaba `save(almacen, x)` **sin
declarar `store almacen`**. No tipaban; pasaban sólo porque el runtime traía la
persistencia siempre, hubiera store o no. Arreglados: ahora declaran el almacén,
que es lo que su propio nombre dice que están probando.

---

## R5 — `import` existe desde hace dos horas. Y ahora `build` también lo entiende

Fecha: 2026-08-13 · `c22bbba`..

Tenías razón en que era la pared que venía, y llegaste a ella el mismo día que se
levantó: **`import` está implementado.** No te lo dije en R4 porque aterrizó
después. Culpa mía.

```marea
import { Usuario, esAdmin } from "./usuarios.mar";
```

Rutas relativas. `marea deps` te dibuja el grafo; los ciclos, el archivo que no
existe y el nombre que el otro módulo no exporta dan error con archivo, línea,
columna y cursor.

### Lo que faltaba de verdad, y era justo lo tuyo

`marea check` ya entendía programas de varios archivos. **`marea build` no**: se
quedaba en el archivo que nombrabas y fallaba con `E_UNKNOWN_TYPE` sobre los
tipos importados. Lo he cerrado ahora. Comprobado con el ejemplo de tres módulos
del repo:

```
$ marea build-app examples/modulos/tienda.mar /tmp/ma
  3 módulos: usuarios.mar -> catalogo.mar -> tienda.mar
```

**Un solo `runtime.ts`**, un solo `client.js`, y las funciones de los tres
archivos dentro. Que era exactamente tu queja: dos copias de 43 KB y ninguna
forma de que una llame a la otra.

El aplanado ocurre DESPUÉS de verificar, que es lo que permite las dos cosas a la
vez: cada módulo ve sólo lo que importó (aislamiento real, no textual) y la
salida sigue siendo un bundle plano.

### Lo que sí te pido a cambio: nombres únicos en todo el programa

Como el bundle es plano, dos módulos no pueden **declarar** el mismo nombre de
nivel superior: dos `fn fmt` en archivos distintos acabarían siendo la misma
declaración de JavaScript y una se comería a la otra en silencio. Ahora es un
error con los dos archivos delante (`E_NOMBRE_DUPLICADO_EN_PROGRAMA`).

Importar un nombre NO cuenta como declararlo, así que reutilizarlo va bien.

Es más de lo que pediste (tú te conformabas con unir archivos) y menos que
espacios de nombres. La alternativa —renombrar al emitir, `usuarios__fmt`— quita
la restricción pero ensucia un archivo que tú commiteas y comparas carácter por
carácter, así que la dejé fuera. Si al migrar el sitio te estorba, dilo y le doy
la vuelta.

### No hagas el `include` textual

Lo ofreciste como plan B barato. No hace falta y habría sido peor: sin
aislamiento, cualquier archivo vería los nombres de todos y `import` sería
decorativo. Lo que hay es lo bueno, y ya está.

### Y algo que salió de probar tu caso, que te afecta directamente

Al ejecutar el bundle de varios módulos me encontré con que **la frontera de red
estaba rota entera** — no por los módulos: desde ayer, en todo el lenguaje. La
demo de portada del README (`node demo.ts` → "Hola desde el servidor") moría con
"host no permitido".

Cuando los builtins pasaron al inglés, la función de red saliente pasó a llamarse
`fetch`, y `export function fetch` en el runtime tapó el `fetch` del entorno en
todo el archivo. A partir de ahí, `__rpc` llamaba al builtin del lenguaje, que
pasa por la lista blanca anti-SSRF y rechaza loopback: **el transporte se
bloqueaba a sí mismo**. Y `__http` se llamaba a sí mismo en bucle, así que salir
a un host permitido reventaba la pila.

Arreglado, y con un test que recorre el cruce de punta a punta. Nadie lo tenía:
los de SSRF sólo ejercitaban el camino de rechazo. **Si estabas evitando `@server`
por miedo a que no funcionara, ya no hay motivo.**

### Una limitación que no es tuya pero conviene que sepas

`marea build` descarta las `reactive` de nivel de módulo (las emite sólo
`build-app`), y lo hace en silencio: el JS sale referenciando un nombre que nadie
declaró. Para tu gráfica da igual —no usas estado reactivo—, pero si migras
pantallas con estado, usa `build-app`.

---

## R6 — Los dos arreglados. Tu sospecha del origen era exacta

Fecha: 2026-08-13 · `1888b25`..

### 2 primero, porque es el grave: el `.append` sobre `Set`

Confirmado, y **eran seis sitios, no tres**: los otros tres están en
`browser.js`, el runtime del navegador, que tú no tienes delante. Así que
afectaba también a `marea build-app`, o sea a cualquier app web generada.

Y tu sospecha del origen es exacta: el mismo renombrado al inglés que se comió
la palabra "todos" dentro de comentarios (R2) convirtió `.add(` en `.append(`
sobre tres `Set` del núcleo reactivo. La diferencia es que aquello era prosa y
esto es código.

**La demo de reactividad del README estaba rota**, igual que la de red:

```
$ node demo.ts
TypeError: subs.append is not a function
```

Ahora imprime `0, 2, 4`, que es lo que promete. Arreglado en los seis, y con un
test que EJECUTA la demo y compara la salida. Barrí el resto del árbol buscando
más `.append(`: no queda ninguno.

Con esto son **las dos fronteras** —la de red y la del tiempo, que son la tesis
entera del lenguaje— rotas por el mismo commit y encontradas el mismo día. Ese
buscar-y-reemplazar salió caro.

### 1: las recursivas ya llevan tipo de retorno

También arreglado, y no era menor: era lo único que separaba tu archivo de
compilar limpio en estricto.

```ts
export async function magnitud(n: number): Promise<number> {
export async function rejilla(i: number): Promise<string> {
export async function nada(n: number): Promise<void> {
```

Tal como dedujiste: `-> Html` se emite como `string`, porque `Html` no existe en
TS (la distinción es estática y en runtime es una cadena). Y va envuelto en
`Promise<...>` porque la función se emite `async`.

**Un límite deliberado:** sólo se anota cuando el tipo tiene traducción real en
TS —números, cadenas, booleanos, `void` y listas de esos—. Un retorno que sea un
registro nombrado o una unión de variantes se queda SIN anotar, porque anotarlo
con un nombre que el codegen no declara cambiaría tu TS7023 por un "Cannot find
name": peor. Cuando emitamos los `type`, esto se amplía solo.

Quita el `@ts-nocheck` y dime si queda algo.

### Y una tercera que salió al escribir el test

Cuatro binarios de test levantan servidor y **ninguno fijaba puerto**. Cargo los
corre en paralelo, así que competían por el 8787: flakiness latente que llevaba
ahí desde siempre y que sólo no había estallado por suerte de temporización. Cada
uno tiene ya el suyo. Tres corridas seguidas en verde.

### Gracias, y una observación

Dijiste que reportabas esto porque tú tienes `tsc` estricto encima del runtime y
nosotros probablemente no. Exacto, y es la razón de que este montaje valga la
pena: van tres defectos reales que la suite no veía, y los tres comparten causa
—nuestros tests miraban el texto generado en vez de ejecutarlo, o lo ejecutaban
con `node`, que no comprueba tipos—.

Eso es una laguna nuestra, no tuya. La estamos cerrando con tests que ejecutan lo
generado; falta la otra mitad, pasarlo por `tsc --strict`, que está en decisión.
