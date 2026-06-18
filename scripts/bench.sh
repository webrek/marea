#!/usr/bin/env bash
# Benchmark de tiempos de Marea — reproducible.
#
# Mide dos cosas DISTINTAS y no las confunde:
#   1) Tiempo de COMPILACIÓN del compilador `marea` (esto SÍ es de Marea).
#   2) Tiempo de EJECUCIÓN de un kernel CPU (fib recursivo) en cada blanco.
#      Ojo: Marea es un transpilador, así que su velocidad de ejecución es la de
#      su BLANCO (motor WASM / V8). El kernel mide el blanco + la calidad del
#      codegen, NO un runtime propio de Marea.
#
# Mismo algoritmo en todos (fib que llama a un add explícito), para que la
# comparación sea manzana-con-manzana. fib(N) con N por defecto 32.
#
# Uso:  bash scripts/bench.sh [N]
set -euo pipefail

N="${1:-32}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUSTBIN="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin"
export PATH="$RUSTBIN:$PATH"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

echo "== Marea · benchmark de tiempos =="
echo "fib(N=$N) · mismo algoritmo (fib -> add) en todos los blancos"
echo "máquina: $(uname -sm) · node $(node --version) · rustc $(rustc --version | awk '{print $2}') · python $(python3 --version | awk '{print $2}')"
echo

# --- 1) compilación: construir el binario release una vez, luego cronometrarlo ---
echo "-- construyendo el compilador (release, una vez) --"
cargo build --release -p marea-cli >/dev/null 2>&1
MAREA="$ROOT/target/release/marea"

timed() { # imprime ms de un comando, promediando R corridas
  local r="$1"; shift
  local best=999999
  for _ in $(seq "$r"); do
    local t0 t1
    t0=$(python3 -c 'import time;print(time.perf_counter_ns())')
    "$@" >/dev/null 2>&1
    t1=$(python3 -c 'import time;print(time.perf_counter_ns())')
    local ms=$(( (t1 - t0) / 1000000 ))
    (( ms < best )) && best=$ms
  done
  echo "$best"
}

C_TS=$(timed 5 "$MAREA" build "$ROOT/examples/math.mar" "$WORK/ts")
C_WASM=$(timed 5 "$MAREA" build-wasm "$ROOT/examples/math.mar" "$WORK/math.wat")
echo "compilar math.mar -> TypeScript : ${C_TS} ms (mejor de 5)"
echo "compilar math.mar -> WAT        : ${C_WASM} ms (mejor de 5)"
wat2wasm "$WORK/math.wat" -o "$WORK/math.wasm"
echo

# --- 2) ejecución: kernel fib(N) en cada blanco, ms del cómputo puro ---
echo "-- ejecución: fib($N), ms del cómputo (sin arranque de proceso) --"

# N se pasa como argv a cada kernel: así es un valor de RUNTIME y ningún
# compilador puede pre-calcular la recursión en tiempo de compilación.

# Marea -> WASM (motor WebAssembly de Node)
cat > "$WORK/run_wasm.mjs" <<'EOF'
import { readFileSync } from "node:fs";
const N = Number(process.argv[2]);
const bytes = readFileSync(process.argv[3]);
const { instance } = await WebAssembly.instantiate(bytes, {});
const fib = instance.exports.fib;
fib(N); // warm-up
let best = Infinity;
for (let i = 0; i < 20; i++) { const t = performance.now(); fib(N); best = Math.min(best, performance.now() - t); }
console.log(best.toFixed(2));
EOF
MAREA_WASM=$(node "$WORK/run_wasm.mjs" "$N" "$WORK/math.wasm")

# Marea -> TypeScript (V8) — fib es async y await-eado por el codegen
cat > "$WORK/run_ts.mjs" <<EOF
const N = Number(process.argv[2]);
const { fib } = await import("$WORK/ts/client.ts");
await fib(N); // warm-up
let best = Infinity;
for (let i = 0; i < 5; i++) { const t = performance.now(); await fib(N); best = Math.min(best, performance.now() - t); }
console.log(best.toFixed(2));
EOF
MAREA_TS=$(node "$WORK/run_ts.mjs" "$N")

# JavaScript a mano (V8) — baseline mismo algoritmo, síncrono
cat > "$WORK/run_js.mjs" <<'EOF'
const N = Number(process.argv[2]);
const add = (a, b) => a + b;
const fib = (n) => (n < 2 ? n : add(fib(n - 1), fib(n - 2)));
fib(N);
let best = Infinity;
for (let i = 0; i < 20; i++) { const t = performance.now(); fib(N); best = Math.min(best, performance.now() - t); }
console.log(best.toFixed(2));
EOF
HAND_JS=$(node "$WORK/run_js.mjs" "$N")

# Rust nativo (-O3) — N viene de argv (runtime), no se puede const-foldear
cat > "$WORK/bench.rs" <<'EOF'
use std::hint::black_box;
fn add(a: i64, b: i64) -> i64 { a + b }
fn fib(n: i64) -> i64 { if n < 2 { n } else { add(fib(n - 1), fib(n - 2)) } }
fn main() {
    let n: i64 = std::env::args().nth(1).unwrap().parse().unwrap();
    fib(black_box(n));
    let mut best = u128::MAX;
    for _ in 0..20 {
        let t = std::time::Instant::now();
        black_box(fib(black_box(n)));
        best = best.min(t.elapsed().as_nanos());
    }
    println!("{:.2}", best as f64 / 1_000_000.0);
}
EOF
rustc -C opt-level=3 "$WORK/bench.rs" -o "$WORK/bench_rs" 2>/dev/null
RUST_NATIVE=$("$WORK/bench_rs" "$N")

# Python (CPython, interpretado) — N de argv
cat > "$WORK/bench.py" <<'EOF'
import sys, time
N = int(sys.argv[1])
def add(a, b): return a + b
def fib(n): return n if n < 2 else add(fib(n - 1), fib(n - 2))
fib(N)
best = float("inf")
for _ in range(3):
    t = time.perf_counter(); fib(N); best = min(best, (time.perf_counter() - t) * 1000)
print(f"{best:.2f}")
EOF
PYTHON=$(python3 "$WORK/bench.py" "$N")

printf "%-26s %10s ms\n" "Rust nativo (-O3)"        "$RUST_NATIVE"
printf "%-26s %10s ms\n" "Marea -> WASM (Node WASM)" "$MAREA_WASM"
printf "%-26s %10s ms\n" "JavaScript a mano (V8)"    "$HAND_JS"
printf "%-26s %10s ms\n" "Marea -> TS, async (V8)"   "$MAREA_TS"
printf "%-26s %10s ms\n" "Python (CPython)"          "$PYTHON"
echo
echo "Nota: el kernel mide el BLANCO (WASM/V8/LLVM/CPython) + el codegen, no un"
echo "runtime propio de Marea. La señal interesante es Marea->WASM ≈ nativo y el"
echo "costo del 'async' uniforme en Marea->TS frente al JS a mano."
