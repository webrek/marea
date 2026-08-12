// Generado por Marea — cliente de navegador (no editar).
// Runtime de Marea para el NAVEGADOR (generado automáticamente — no editar).
//
// Es el gemelo de runtime.ts pero sin nada de Node (ni http ni fs): solo lo que
// corre en el navegador — el cliente RPC (fetch al mismo origen), el núcleo
// reactivo (signals de grano fino) y un puñado de builtins. Más `__mount`, que
// ata una vista reactiva al DOM: cuando cambia un signal que la vista leyó, el
// #app se vuelve a pintar solo. Esa es la frontera del TIEMPO tocando el DOM.

// --- cliente RPC: cruza la frontera de red al servidor del mismo origen ---
export async function __rpc(fn, args) {
  const res = await fetch("/__marea", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ fn, args }),
  });
  const data = await res.json();
  if (data && typeof data === "object" && "error" in data) {
    throw new Error(String(data.error));
  }
  return data.ok;
}

// --- núcleo reactivo (idéntico en semántica a runtime.ts, sin glitches) ---
let __currentSub = null;
const __pending = new Set();
let __flushing = false;

function __flush() {
  if (__flushing) return;
  __flushing = true;
  let guard = 0;
  try {
    while (__pending.size > 0) {
      if (++guard > 1000) {
        __pending.clear();
        throw new Error("ciclo reactivo detectado: un effect reescribe una reactiva que lee");
      }
      const r = __pending.values().next().value;
      __pending.delete(r);
      r.execute();
    }
  } finally {
    __flushing = false;
  }
}

export function __signal(initial) {
  let value = initial;
  const subs = new Set();
  return {
    get() {
      if (__currentSub) subs.add(__currentSub);
      return value;
    },
    set(v) {
      if (v === value) return;
      value = v;
      for (const r of [...subs]) r.invalidate();
      __flush();
    },
  };
}

export function __effect(fn) {
  const reaction = {
    execute() {
      const prev = __currentSub;
      __currentSub = reaction;
      try {
        void fn();
      } finally {
        __currentSub = prev;
      }
    },
    invalidate() {
      __pending.add(reaction);
    },
  };
  reaction.execute();
}

// Gemelo de runtime.ts: un recurso arranca en Cargando y se resuelve solo.
export function __recurso(f) {
  const s = __signal({ $tag: "Cargando" });
  Promise.resolve()
    .then(f)
    .then(
      (v) => s.set(v),
      (e) => {
        console.error("[marea] recurso falló:", e);
        s.set({ $tag: "Fallo" });
      },
    );
  return s;
}

export function __memo(fn) {
  const subs = new Set();
  let value;
  let dirty = true;
  const reaction = {
    invalidate() {
      if (!dirty) {
        dirty = true;
        for (const r of [...subs]) r.invalidate();
      }
    },
  };
  return {
    get() {
      if (__currentSub) subs.add(__currentSub);
      if (dirty) {
        const prev = __currentSub;
        __currentSub = reaction;
        try {
          value = fn();
          dirty = false;
        } finally {
          __currentSub = prev;
        }
      }
      return value;
    },
    set() {
      throw new Error("una derivada 'reactive' es de solo lectura");
    },
  };
}

// --- builtins del lenguaje (gemelos de runtime.ts) ---
export function print(x) {
  console.log(x);
}
export function concat(a, b) {
  return a + b;
}
export function len(xs) {
  return xs.length;
}
export function aTexto(x) {
  if (x !== null && typeof x === "object" && typeof x.$tag === "string") return x.$tag;
  return String(x);
}
// Gemelo de runtime.ts: escapa un texto para incrustarlo en HTML.
// Gemelos de runtime.ts: cortar la división entre cero en vez de devolver
// Infinity/NaN dentro de un Int.
export function __div(a, b) {
  if (b === 0) throw new Error("división entre cero");
  return Math.trunc(a / b);
}
export function __rem(a, b) {
  if (b === 0) throw new Error("módulo entre cero");
  return a % b;
}

export function escapar(x) {
  return String(x)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}
// Marca una cadena como marcado ya seguro. En runtime es la identidad:
// la garantía es estática (el tipo Html), esto solo la hace explícita
// en el fuente para que se vea en una revisión.
export function html(s) {
  return s;
}
// --- listas: construir en runtime (el lenguaje no tiene bucles ni cierres, así
// que sin esto una función no puede devolver un subconjunto filtrado) ---
export function unir(a, b) {
  return a.concat(b);
}
export function agregar(xs, x) {
  return xs.concat([x]);
}
// --- texto ---
export function largo(s) {
  return Array.from(String(s)).length;
}
export function contiene(s, sub) {
  return String(s).includes(String(sub));
}
export function minusculas(s) {
  return String(s).toLowerCase();
}
export function __index(xs, i) {
  if (i < 0 || i >= xs.length) {
    throw new Error(`índice fuera de rango: ${i} (longitud ${xs.length})`);
  }
  return xs[i];
}
export function __marea_is(value, tag) {
  return value !== null && typeof value === "object" && value.$tag === tag;
}
// 'render' en el navegador pinta HTML en #app (en Node solo registraba en consola).
export function render(x) {
  const app = typeof document !== "undefined" && document.getElementById("app");
  if (app) app.innerHTML = String(x);
  else console.log("[render]", x);
}

// --- el puente reactividad ↔ DOM ---
// Envuelve la vista en un effect: cada vez que cambia un signal que la vista
// leyó, se vuelve a pintar el #app. La vista lee sus reactivas en su parte
// SÍNCRONA (antes de cualquier await), así que la suscripción queda registrada.
export function __mount(vista) {
  const app = typeof document !== "undefined" && document.getElementById("app");
  if (!app) return;
  __effect(async () => {
    app.innerHTML = await vista();
  });
}

// --- programa ---
const posts = __signal([]);

async function publicar(autor, texto) {
  return await __rpc("publicar", [autor, texto]);
}
async function like(i) {
  return await __rpc("like", [i]);
}
async function feed() {
  return await __rpc("feed", []);
}
async function fila(p, i) {
  return concat(concat(concat("<li class=\"post\"><span class=\"autor\">@", escapar(p.autor)), concat("</span> ", escapar(p.texto))), concat(concat(concat(" <button class=\"like\" onclick=\"marea.darLike(", aTexto(i)), concat(")\">♥ ", aTexto(p.likes))), "</button></li>"));
}
async function filas(ps, i) {
  if ((i < len(ps))) {
    return concat((await fila(__index(ps, i), i)), (await filas(ps, (i + 1))));
  }
  return "";
}
async function vista() {
  const ps = posts.get();
  return concat("<ul class=\"feed\">", concat((await filas(ps, 0)), "</ul>"));
}
async function darLike(i) {
  (await like(i));
  posts.set((await feed()));
}
async function main() {
  const actual = (await feed());
  if ((len(actual) < 1)) {
    (await publicar("ada", "Primer post en Marea 🌊"));
    (await publicar("grace", "El estado es reactivo en el navegador"));
  }
  posts.set((await feed()));
}

// --- arranque ---
globalThis.marea = { fila, filas, vista, darLike, main };
await main();
__mount(vista);
