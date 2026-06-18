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

// --- estado del servidor con BACKEND DE PERSISTENCIA CONECTABLE ---
//
// El store guarda registros del tipo de 'store T;'. El BACKEND se elige con la
// variable MAREA_DB: file (por defecto) | sqlite | postgres | mysql | mongodb.
// El esquema (tabla + columnas) lo inyecta el codegen desde 'store T;'. Todos
// los backends comparten la misma interfaz, así que el código .mar no cambia al
// pasar de un archivo JSON a una base de datos real.
//
// Modelo: el arreglo en memoria es la copia de trabajo (índices estables); cada
// mutación lo persiste completo (saveAll). Simple y correcto; una versión de
// producción usaría operaciones incrementales por id.

interface __Schema {
  table: string;
  columns: { name: string; kind: "text" | "int" | "real" | "bool" | "json" }[];
}
const __STORE_SCHEMA: __Schema | null = __MAREA_STORE_SCHEMA__;

interface __Backend {
  load(): Promise<unknown[]>;
  saveAll(items: unknown[]): Promise<void>;
}

// Cuando el store no guarda registros (p.ej. `store Int;` o `store String;`) el
// esquema tiene una sola columna '__doc' de tipo json: ahí cabe el valor entero.
function __isDoc(): boolean {
  const cols = (__STORE_SCHEMA as __Schema).columns;
  return cols.length === 1 && cols[0].name === "__doc";
}

// --- conversión registro <-> fila (para los backends SQL) ---
function __toRow(rec: any): unknown[] {
  if (__isDoc()) return [JSON.stringify(rec ?? null)];
  return (__STORE_SCHEMA as __Schema).columns.map((c) => {
    const v = rec?.[c.name];
    if (c.kind === "bool") return v ? 1 : 0;
    if (c.kind === "json") return JSON.stringify(v ?? null);
    return v ?? null;
  });
}
function __fromRow(row: any): unknown {
  if (__isDoc()) return JSON.parse(row?.__doc ?? "null");
  const rec: any = {};
  for (const c of (__STORE_SCHEMA as __Schema).columns) {
    const v = row?.[c.name];
    rec[c.name] = c.kind === "bool" ? v != 0 : c.kind === "json" ? JSON.parse(v ?? "null") : v;
  }
  return rec;
}
function __sqlType(kind: string): string {
  if (kind === "int" || kind === "bool") return "INTEGER";
  if (kind === "real") return "REAL";
  return "TEXT";
}
function __cols(): string {
  return (__STORE_SCHEMA as __Schema).columns.map((c) => c.name).join(", ");
}
function __createSql(): string {
  const s = __STORE_SCHEMA as __Schema;
  const defs = s.columns.map((c) => `${c.name} ${__sqlType(c.kind)}`).join(", ");
  return `CREATE TABLE IF NOT EXISTS ${s.table} (${defs})`;
}

// --- backend: archivo JSON (por defecto, cero dependencias) ---
function __fileBackend(): __Backend {
  const file = process.env.MAREA_STORE ?? "__MAREA_STORE_DEFAULT__";
  return {
    async load() {
      let data: string;
      try {
        data = fs.readFileSync(file, "utf8");
      } catch {
        return [];
      }
      let parsed: unknown;
      try {
        parsed = JSON.parse(data);
      } catch {
        throw new Error(`[marea] store corrupto (JSON inválido) en ${file}`);
      }
      if (!Array.isArray(parsed)) {
        throw new Error(`[marea] ${file} existe pero no es un arreglo`);
      }
      return parsed;
    },
    async saveAll(items) {
      fs.writeFileSync(file, JSON.stringify(items));
    },
  };
}

// --- backend: SQLite (módulo integrado node:sqlite, cero dependencias) ---
function __sqliteBackend(): __Backend {
  const path = process.env.MAREA_DB_URL ?? "marea.sqlite";
  let db: any = null;
  async function open() {
    if (db) return db;
    const { DatabaseSync } = await import("node:sqlite");
    db = new DatabaseSync(path);
    db.exec(__createSql());
    return db;
  }
  return {
    async load() {
      const d = await open();
      return d.prepare(`SELECT ${__cols()} FROM ${(__STORE_SCHEMA as __Schema).table}`).all().map(__fromRow);
    },
    async saveAll(items) {
      const d = await open();
      const t = (__STORE_SCHEMA as __Schema).table;
      const ph = (__STORE_SCHEMA as __Schema).columns.map(() => "?").join(", ");
      const ins = d.prepare(`INSERT INTO ${t} (${__cols()}) VALUES (${ph})`);
      d.exec("BEGIN");
      d.exec(`DELETE FROM ${t}`);
      for (const it of items) ins.run(...(__toRow(it) as any[]));
      d.exec("COMMIT");
    },
  };
}

// --- backend: PostgreSQL (driver 'pg', import perezoso; requiere MAREA_DB_URL) ---
function __postgresBackend(): __Backend {
  let pool: any = null;
  async function open() {
    if (pool) return pool;
    const pg = await import("pg");
    pool = new pg.Pool({ connectionString: process.env.MAREA_DB_URL });
    await pool.query(__createSql());
    return pool;
  }
  return {
    async load() {
      const p = await open();
      const r = await p.query(`SELECT ${__cols()} FROM ${(__STORE_SCHEMA as __Schema).table}`);
      return r.rows.map(__fromRow);
    },
    async saveAll(items) {
      const p = await open();
      const s = __STORE_SCHEMA as __Schema;
      const c = await p.connect();
      try {
        await c.query("BEGIN");
        await c.query(`DELETE FROM ${s.table}`);
        for (const it of items) {
          const ph = s.columns.map((_, i) => `$${i + 1}`).join(", ");
          await c.query(`INSERT INTO ${s.table} (${__cols()}) VALUES (${ph})`, __toRow(it) as any[]);
        }
        await c.query("COMMIT");
      } catch (e) {
        await c.query("ROLLBACK");
        throw e;
      } finally {
        c.release();
      }
    },
  };
}

// --- backend: MySQL (driver 'mysql2/promise', import perezoso; MAREA_DB_URL) ---
function __mysqlBackend(): __Backend {
  let pool: any = null;
  async function open() {
    if (pool) return pool;
    const mysql = await import("mysql2/promise");
    pool = mysql.createPool(process.env.MAREA_DB_URL ?? "");
    await pool.query(__createSql());
    return pool;
  }
  return {
    async load() {
      const p = await open();
      const [rows] = await p.query(`SELECT ${__cols()} FROM ${(__STORE_SCHEMA as __Schema).table}`);
      return (rows as any[]).map(__fromRow);
    },
    async saveAll(items) {
      const p = await open();
      const s = __STORE_SCHEMA as __Schema;
      const ph = s.columns.map(() => "?").join(", ");
      const c = await p.getConnection();
      try {
        await c.beginTransaction();
        await c.query(`DELETE FROM ${s.table}`);
        for (const it of items) {
          await c.query(`INSERT INTO ${s.table} (${__cols()}) VALUES (${ph})`, __toRow(it) as any[]);
        }
        await c.commit();
      } catch (e) {
        await c.rollback();
        throw e;
      } finally {
        c.release();
      }
    },
  };
}

// --- backend: MongoDB (driver 'mongodb', import perezoso; MAREA_DB_URL) ---
function __mongoBackend(): __Backend {
  let coll: any = null;
  async function open() {
    if (coll) return coll;
    const { MongoClient } = await import("mongodb");
    const client = new MongoClient(process.env.MAREA_DB_URL ?? "");
    await client.connect();
    coll = client.db().collection((__STORE_SCHEMA as __Schema).table);
    return coll;
  }
  return {
    async load() {
      const c = await open();
      // Documentos directos; quitamos el _id que Mongo agrega.
      const docs = (await c.find({}, { projection: { _id: 0 } }).toArray()) as any[];
      return __isDoc() ? docs.map((d) => d.__doc) : (docs as unknown[]);
    },
    async saveAll(items) {
      const c = await open();
      await c.deleteMany({});
      if (items.length === 0) return;
      const docs = __isDoc()
        ? items.map((x) => ({ __doc: x }))
        : items.map((x) => ({ ...(x as object) }));
      await c.insertMany(docs);
    },
  };
}

function __makeBackend(): __Backend {
  const which = process.env.MAREA_DB ?? "file";
  switch (which) {
    case "sqlite":
      return __sqliteBackend();
    case "postgres":
      return __postgresBackend();
    case "mysql":
      return __mysqlBackend();
    case "mongodb":
      return __mongoBackend();
    default:
      return __fileBackend();
  }
}

let __backend: __Backend | null = null;
let __store: unknown[] | null = null;

async function __ensureStore(): Promise<unknown[]> {
  if (__store === null) {
    if (__backend === null) __backend = __makeBackend();
    __store = await __backend.load();
  }
  return __store;
}
async function __persist(): Promise<void> {
  if (__backend) await __backend.saveAll(__store ?? []);
}

export async function guardar(x: unknown): Promise<void> {
  (await __ensureStore()).push(x);
  await __persist();
}
export async function todos(): Promise<unknown[]> {
  return (await __ensureStore()).slice();
}
// Reemplaza el elemento en el índice 'i' (CRUD: update).
export async function actualizar(i: number, x: unknown): Promise<void> {
  const s = await __ensureStore();
  if (i >= 0 && i < s.length) {
    s[i] = x;
    await __persist();
  }
}
// Elimina el elemento en el índice 'i' (CRUD: delete).
export async function borrar(i: number): Promise<void> {
  const s = await __ensureStore();
  if (i >= 0 && i < s.length) {
    s.splice(i, 1);
    await __persist();
  }
}
