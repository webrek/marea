// Runtime de Marea (generado automáticamente — no editar a mano).
//
// Contiene el transporte RPC que materializa la "frontera de red":
//   - lado servidor: un registro de handlers + un servidor HTTP mínimo.
//   - lado cliente:  __rpc(), que serializa la llamada y la manda por fetch.
// Más los builtins del lenguaje.

import http from "node:http";
import fs from "node:fs";

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
export function len(xs: unknown[]): number {
  return xs.length;
}
export function aTexto(x: unknown): string {
  return String(x);
}
// Indexado de lista con verificación de rango: un índice fuera de rango lanza
// un error claro en vez de devolver 'undefined' (que reventaría más tarde).
export function __index(xs: unknown[], i: number): unknown {
  if (i < 0 || i >= xs.length) {
    throw new Error(`índice fuera de rango: ${i} (longitud ${xs.length})`);
  }
  return xs[i];
}

// --- estado del servidor: un store PERSISTENTE A DISCO. Se carga del archivo
// la PRIMERA vez que se usa (carga perezosa: un programa sin store nunca toca el
// disco) y se reescribe en cada mutación. El nombre por defecto incluye la firma
// del esquema de 'store T;' (lo sustituye el codegen), así dos apps con esquemas
// distintos NO comparten archivo ni se contaminan. Override con MAREA_STORE.
const __STORE_FILE = process.env.MAREA_STORE ?? "__MAREA_STORE_DEFAULT__";

let __store: unknown[] | null = null;

function __loadStore(): unknown[] {
  let data: string;
  try {
    data = fs.readFileSync(__STORE_FILE, "utf8");
  } catch {
    return []; // sin archivo aún: store vacío.
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(data);
  } catch {
    throw new Error(`[marea] store corrupto (JSON inválido) en ${__STORE_FILE}`);
  }
  if (!Array.isArray(parsed)) {
    // No sobrescribir silenciosamente un archivo que no es un store.
    throw new Error(`[marea] ${__STORE_FILE} existe pero no es un arreglo`);
  }
  return parsed;
}

function __ensureStore(): unknown[] {
  if (__store === null) __store = __loadStore();
  return __store;
}

function __persist(): void {
  try {
    fs.writeFileSync(__STORE_FILE, JSON.stringify(__store));
  } catch (e) {
    console.error("[marea] no se pudo persistir el store:", e);
  }
}

export function guardar(x: unknown): void {
  __ensureStore().push(x);
  __persist();
}
export function todos(): unknown[] {
  return __ensureStore().slice();
}
// Reemplaza el elemento en el índice 'i' (CRUD: update).
export function actualizar(i: number, x: unknown): void {
  const s = __ensureStore();
  if (i >= 0 && i < s.length) {
    s[i] = x;
    __persist();
  }
}
// Elimina el elemento en el índice 'i' (CRUD: delete).
export function borrar(i: number): void {
  const s = __ensureStore();
  if (i >= 0 && i < s.length) {
    s.splice(i, 1);
    __persist();
  }
}
