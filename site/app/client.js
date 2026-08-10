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
  return String(x);
}
export function __index(xs, i) {
  if (i < 0 || i >= xs.length) {
    throw new Error(`índice fuera de rango: ${i} (longitud ${xs.length})`);
  }
  return xs[i];
}
export function __marea_is(value, tag) {
  if (value === tag) return true;
  if (value && typeof value === "object") {
    return value.tag === tag || value.kind === tag || value.type === tag;
  }
  return false;
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
  return concat(concat(concat("<li class=\"post\"><span class=\"autor\">@", p.autor), concat("</span> ", p.texto)), concat(concat(concat(" <button class=\"like\" onclick=\"marea.darLike(", aTexto(i)), concat(")\">♥ ", aTexto(p.likes))), "</button></li>"));
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
