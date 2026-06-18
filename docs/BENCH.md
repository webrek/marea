# Marea — tiempos (compilación y ejecución)

Reproducible con **`bash scripts/bench.sh [N]`** (N = parámetro de `fib`, por
defecto 32). Los números de abajo son de una corrida real; los tuyos variarán
con la máquina.

> Máquina de referencia: **Darwin arm64 (Apple Silicon)** · Node v26 · rustc
> 1.96 · Python 3.14.

## Lo que mide y lo que NO

Marea es un **transpilador**: no tiene runtime propio de ejecución. Por eso el
benchmark separa dos cosas que suelen confundirse:

1. **Compilación** (`.mar` → TypeScript / WAT). *Esto sí es de Marea.*
2. **Ejecución de un kernel CPU** (`fib` recursivo). Aquí se mide el **blanco**
   (el motor WebAssembly, V8, LLVM o CPython) **más la calidad del codegen** — no
   una VM de Marea. La señal útil no es "¿Marea es rápido?" sino "¿el codegen de
   Marea desperdicia el blanco o lo aprovecha?".

Mismo algoritmo en los cinco blancos para que sea manzana-con-manzana: un `fib`
recursivo que llama a un `add` explícito (igual que `examples/math.mar`), con `N`
pasado como argumento de **runtime** para que ningún compilador pueda
pre-calcular la recursión en tiempo de compilación.

## Compilación

| Etapa | Tiempo (mejor de 5) |
|---|---:|
| `marea build math.mar` → TypeScript (lex + parse + check + codegen) | **~15 ms** |
| `marea build-wasm math.mar` → WAT | **~15 ms** |

Todo el front-end + un backend en ~15 ms para un archivo pequeño. (El binario
`marea` se construye una vez con `cargo build --release`; eso no se cuenta aquí —
es el compilador, no la compilación de tu programa.)

## Ejecución — `fib(32)`, ms del cómputo puro

(sin contar el arranque del proceso; cada blanco se mide en caliente, mejor de
varias corridas)

| Blanco | Tiempo | vs Marea→WASM |
|---|---:|---:|
| Rust nativo (`-O3`) | **3.27 ms** | 0.9× |
| **Marea → WASM** (motor WASM de Node) | **3.56 ms** | **1.0×** |
| JavaScript a mano (V8) | 10.36 ms | 2.9× |
| **Marea → TypeScript** (async, V8) | **258.33 ms** | **72×** |
| Python (CPython) | 186.99 ms | 53× |

## Qué revela esto (honestamente)

**1. Marea → WASM va a ras de nativo.** 3.56 ms contra 3.27 ms de Rust con `-O3`:
~9% de diferencia. Para enteros y recursión, el codegen a WebAssembly de Marea no
deja rendimiento sobre la mesa — el `i32` nativo y las llamadas directas hacen lo
suyo. Y es ~3× más rápido que el mismo algoritmo en JavaScript a mano sobre V8.

**2. Marea → TypeScript paga un impuesto de `async` brutal.** 258 ms: ~25× más
lento que el JS a mano e **incluso más lento que CPython**. ¿Por qué? El codegen
declara **toda** función `async` y `await`-ea **cada** llamada
(`return (await add((await fib(n - 1)), (await fib(n - 2))))`), porque así
materializa de forma uniforme la frontera `@server` (que sí es asíncrona, cruza
la red). Para una función de cómputo puro como `fib`, eso significa **asignar una
promesa por cada llamada recursiva** — millones de promesas.

Esto **no es inherente a Marea**, es una decisión de codegen mejorable: una
función que **no** es alcanzable desde una frontera `@server`/RPC no necesita ser
`async`. Emitir esas como síncronas acercaría Marea→TS al JS a mano (~10 ms). Es
el tipo de hallazgo que un benchmark honesto saca a la luz, y queda como
**optimización pendiente** (analizar el grafo de llamadas y marcar async solo lo
que lo necesita).

## Conclusión

Para CPU-bound, **usa el backend WASM**: es la historia de rendimiento de Marea y
empata con nativo. El backend TypeScript es para la frontera de red + reactividad
(donde el `async` es correcto y el costo no importa), no para kernels numéricos —
y su sobrecosto en cómputo puro es un objetivo claro de optimización, no una
limitación de fondo.

Para la comparación de **ergonomía** (la misma app en 5 stacks), ver
[COMPARACION.md](COMPARACION.md).
