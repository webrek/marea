// Runtime de Marea (generado automáticamente — no editar a mano).
//
// Contiene el transporte RPC que materializa la "frontera de red":
//   - lado servidor: un registro de handlers + un servidor HTTP mínimo.
//   - lado cliente:  __rpc(), que serializa la llamada y la manda por fetch.
// Más los builtins del lenguaje.

import http from "node:http";

const MAREA_PORT = 8787;
const MAREA_URL = `http://127.0.0.1:${MAREA_PORT}/__marea`;

type Handler = (args: unknown[]) => unknown | Promise<unknown>;
const __handlers: Record<string, Handler> = {};

export function __register(name: string, fn: Handler): void {
  __handlers[name] = fn;
}

let __server: http.Server | null = null;

export function startServer(): Promise<void> {
  return new Promise((resolve) => {
    __server = http.createServer((req, res) => {
      if (req.method !== "POST" || req.url !== "/__marea") {
        res.statusCode = 404;
        res.end();
        return;
      }
      let body = "";
      req.on("data", (chunk) => (body += chunk));
      req.on("end", async () => {
        try {
          const { fn, args } = JSON.parse(body);
          const handler = __handlers[fn];
          if (!handler) {
            res.statusCode = 400;
            res.end(JSON.stringify({ error: `función desconocida: ${fn}` }));
            return;
          }
          const result = await handler(args);
          res.setHeader("content-type", "application/json");
          res.end(JSON.stringify({ ok: result }));
        } catch (e) {
          res.statusCode = 500;
          res.end(JSON.stringify({ error: String(e) }));
        }
      });
    });
    __server.listen(MAREA_PORT, () => {
      console.log(`[marea] servidor escuchando en ${MAREA_URL}`);
      resolve();
    });
  });
}

export function stopServer(): Promise<void> {
  return new Promise((resolve) => {
    if (!__server) {
      resolve();
      return;
    }
    __server.close(() => resolve());
  });
}

// La llamada del cliente que CRUZA la frontera de red.
export async function __rpc(fn: string, args: unknown[]): Promise<unknown> {
  const res = await fetch(MAREA_URL, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ fn, args }),
  });
  const data = await res.json();
  if (data && typeof data === "object" && "error" in data) {
    throw new Error(String((data as Record<string, unknown>).error));
  }
  return (data as Record<string, unknown>).ok;
}

// Comparación de variantes para 'match' (best-effort hasta tener uniones reales).
export function __marea_is(value: unknown, tag: string): boolean {
  if (value === tag) return true;
  if (value && typeof value === "object") {
    const v = value as Record<string, unknown>;
    return v.tag === tag || v.kind === tag || v.type === tag;
  }
  return false;
}

// --- núcleo reactivo (signals de grano fino, sin glitches) ---
//
// 'reactive mut' es un signal (fuente); 'reactive' es un memo (derivado perezoso);
// 'effect { ... }' se re-ejecuta cuando cambia algo que leyó. El rastreo de
// dependencias es automático: leer un signal/memo dentro de una reacción la
// suscribe. Al asignar un signal: se invalidan (marcan sucias) las reacciones
// dependientes y LUEGO se drenan los effects pendientes una sola vez, leyendo
// valores frescos — así no hay glitches (estados intermedios incoherentes). Un
// ciclo reactivo (un effect que reescribe lo que lee) se detecta y aborta con
// un error claro en vez de colgarse.

interface Reaction {
  invalidate(): void;
}

interface EffectReaction extends Reaction {
  execute(): void;
}

let __currentSub: Reaction | null = null;
const __pending = new Set<EffectReaction>();
let __flushing = false;

function __flush(): void {
  if (__flushing) return;
  __flushing = true;
  let guard = 0;
  try {
    while (__pending.size > 0) {
      if (++guard > 1000) {
        __pending.clear();
        throw new Error(
          "ciclo reactivo detectado: un effect reescribe una reactiva que lee"
        );
      }
      const r = __pending.values().next().value as EffectReaction;
      __pending.delete(r);
      r.execute();
    }
  } finally {
    __flushing = false;
  }
}

export interface Cell<T> {
  get(): T;
  set(v: T): void;
}

export function __signal<T>(initial: T): Cell<T> {
  let value = initial;
  const subs = new Set<Reaction>();
  return {
    get(): T {
      if (__currentSub) subs.add(__currentSub);
      return value;
    },
    set(v: T): void {
      if (v === value) return;
      value = v;
      for (const r of [...subs]) r.invalidate();
      __flush();
    },
  };
}

export function __effect(fn: () => void | Promise<void>): void {
  const reaction: EffectReaction = {
    execute() {
      const prev = __currentSub;
      __currentSub = reaction;
      // La suscripción ocurre en la porción síncrona del cuerpo (por eso los
      // builtins NO se awaitan: un await partiría el rastreo).
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

export function __memo<T>(fn: () => T): Cell<T> {
  const subs = new Set<Reaction>();
  let value: T;
  let dirty = true;
  const reaction: Reaction = {
    invalidate() {
      if (!dirty) {
        dirty = true;
        for (const r of [...subs]) r.invalidate();
      }
    },
  };
  const recompute = () => {
    const prev = __currentSub;
    __currentSub = reaction;
    try {
      value = fn();
      dirty = false;
    } finally {
      __currentSub = prev;
    }
  };
  return {
    get(): T {
      if (dirty) recompute();
      if (__currentSub) subs.add(__currentSub);
      return value;
    },
    // Un memo es derivado: no se asigna directamente.
    set(_v: T): void {},
  };
}

// --- builtins del lenguaje ---
export function print(x: unknown): void {
  console.log(x);
}
export function concat(a: string, b: string): string {
  return a + b;
}
export function render(x: unknown): void {
  console.log("[render]", x);
}
