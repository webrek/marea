// Runtime de Marea (generado automáticamente — no editar a mano).
//
// Contiene el transporte RPC que materializa la "frontera de red":
//   - lado servidor: un registro de handlers + un servidor HTTP mínimo.
//   - lado cliente:  __rpc(), que serializa la llamada y la manda por fetch.
// Más los builtins del lenguaje.

import http from "node:http";
import fs from "node:fs";

const MAREA_PORT = 8787;
// Escuchamos solo en loopback por defecto: la frontera de red es para el cliente
// local de la app, no para exponer en la LAN. Se puede ampliar con MAREA_HOST.
const MAREA_HOST = process.env.MAREA_HOST ?? "127.0.0.1";
const MAREA_URL = `http://127.0.0.1:${MAREA_PORT}/__marea`;
// Tope del cuerpo RPC (configurable) para no acumular memoria sin límite.
const MAREA_MAX_BODY = Number(process.env.MAREA_MAX_BODY ?? 1_048_576); // 1 MiB

type Handler = (args: unknown[]) => unknown | Promise<unknown>;
// Tabla sin prototipo: así `fn` no puede resolver a métodos heredados de
// Object.prototype (constructor/toString/…) y solo alcanza handlers reales.
const __handlers: Record<string, Handler> = Object.create(null);

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
      const chunks: Buffer[] = [];
      let size = 0;
      let aborted = false;
      req.on("data", (chunk: Buffer) => {
        if (aborted) return;
        size += chunk.length;
        if (size > MAREA_MAX_BODY) {
          aborted = true;
          res.statusCode = 413;
          res.end(JSON.stringify({ error: "payload demasiado grande" }));
          req.destroy();
          return;
        }
        chunks.push(chunk);
      });
      req.on("end", async () => {
        if (aborted) return;
        let fn: unknown, args: unknown;
        try {
          ({ fn, args } = JSON.parse(Buffer.concat(chunks).toString("utf8")));
        } catch {
          res.statusCode = 400;
          res.end(JSON.stringify({ error: "JSON inválido" }));
          return;
        }
        // 'fn' debe ser string y 'args' un arreglo: rechazo barato antes de tocar
        // un handler (evita confusión de tipo aguas abajo).
        if (typeof fn !== "string" || !Array.isArray(args)) {
          res.statusCode = 400;
          res.end(JSON.stringify({ error: "petición mal formada" }));
          return;
        }
        const handler = __handlers[fn];
        if (!handler) {
          // No hacemos eco de 'fn' (evita oráculo de enumeración de handlers).
          res.statusCode = 400;
          res.end(JSON.stringify({ error: "petición mal formada" }));
          return;
        }
        try {
          const result = await handler(args as unknown[]);
          res.setHeader("content-type", "application/json");
          res.end(JSON.stringify({ ok: result }));
        } catch (e) {
          // El detalle se queda en el servidor; al cliente solo error genérico.
          console.error("[marea] error en handler:", e);
          res.statusCode = 500;
          res.end(JSON.stringify({ error: "error interno" }));
        }
      });
    });
    // Cortar conexiones lentas/colgadas (defensa contra slowloris y fugas).
    __server.requestTimeout = Number(process.env.MAREA_REQUEST_TIMEOUT ?? 15_000);
    __server.headersTimeout = Number(process.env.MAREA_HEADERS_TIMEOUT ?? 10_000);
    __server.listen(MAREA_PORT, MAREA_HOST, () => {
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
// Parseo de columnas JSON tolerante: una celda corrupta cae a null en vez de
// tumbar la carga completa del store (aislamos la fila mala).
function __parseCell(v: any): unknown {
  try {
    return JSON.parse(v ?? "null");
  } catch {
    return null;
  }
}
function __fromRow(row: any): unknown {
  if (__isDoc()) return __parseCell(row?.__doc);
  const rec: any = {};
  for (const c of (__STORE_SCHEMA as __Schema).columns) {
    const v = row?.[c.name];
    rec[c.name] = c.kind === "bool" ? v != 0 : c.kind === "json" ? __parseCell(v) : v;
  }
  return rec;
}
function __sqlType(kind: string): string {
  if (kind === "int" || kind === "bool") return "INTEGER";
  if (kind === "real") return "REAL";
  return "TEXT";
}
// Comilla un identificador SQL según el dialecto (`"` en sqlite/postgres, `` ` ``
// en mysql), duplicando el delimitador interno. Así un campo llamado como una
// palabra reservada (from, order, select…) no rompe el SQL generado. Los
// identificadores ya vienen sin metacaracteres (el lexer solo deja [A-Za-z0-9_]).
function __quoteId(name: string, q: string): string {
  return q + name.split(q).join(q + q) + q;
}
function __cols(q: string): string {
  return (__STORE_SCHEMA as __Schema).columns.map((c) => __quoteId(c.name, q)).join(", ");
}
function __table(q: string): string {
  return __quoteId((__STORE_SCHEMA as __Schema).table, q);
}
function __createSql(q: string): string {
  const s = __STORE_SCHEMA as __Schema;
  const defs = s.columns.map((c) => `${__quoteId(c.name, q)} ${__sqlType(c.kind)}`).join(", ");
  return `CREATE TABLE IF NOT EXISTS ${__table(q)} (${defs})`;
}

// Aparta un store corrupto a un archivo '.corrupt' y devuelve un store vacío,
// para no perder los datos originales ni dejar caer la app entera.
function __quarantine(file: string, motivo: string): unknown[] {
  try {
    fs.renameSync(file, `${file}.corrupt`);
  } catch {
    /* si no se puede renombrar, seguimos igual con store vacío */
  }
  console.error(`[marea] store corrupto (${motivo}) en ${file}; apartado a ${file}.corrupt`);
  return [];
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
        // Degradar en vez de morir: apartamos el archivo corrupto y seguimos en
        // limpio, dejando el original para inspección.
        return __quarantine(file, "JSON inválido");
      }
      if (!Array.isArray(parsed)) {
        return __quarantine(file, "no es un arreglo");
      }
      return parsed;
    },
    async saveAll(items) {
      // Escritura atómica: volcamos a un temporal y renombramos (rename es
      // atómico en el mismo FS), para que un crash a media escritura no deje el
      // store corrupto a medias.
      const tmp = `${file}.${process.pid}.tmp`;
      const fd = fs.openSync(tmp, "w");
      try {
        fs.writeFileSync(fd, JSON.stringify(items));
        fs.fsyncSync(fd);
      } finally {
        fs.closeSync(fd);
      }
      fs.renameSync(tmp, file);
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
    db.exec(__createSql('"'));
    return db;
  }
  return {
    async load() {
      const d = await open();
      return d.prepare(`SELECT ${__cols('"')} FROM ${__table('"')}`).all().map(__fromRow);
    },
    async saveAll(items) {
      const d = await open();
      const t = __table('"');
      const ph = (__STORE_SCHEMA as __Schema).columns.map(() => "?").join(", ");
      const ins = d.prepare(`INSERT INTO ${t} (${__cols('"')}) VALUES (${ph})`);
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
    await pool.query(__createSql('"'));
    return pool;
  }
  return {
    async load() {
      const p = await open();
      const r = await p.query(`SELECT ${__cols('"')} FROM ${__table('"')}`);
      return r.rows.map(__fromRow);
    },
    async saveAll(items) {
      const p = await open();
      const s = __STORE_SCHEMA as __Schema;
      const c = await p.connect();
      try {
        await c.query("BEGIN");
        await c.query(`DELETE FROM ${__table('"')}`);
        for (const it of items) {
          const ph = s.columns.map((_, i) => `$${i + 1}`).join(", ");
          await c.query(`INSERT INTO ${__table('"')} (${__cols('"')}) VALUES (${ph})`, __toRow(it) as any[]);
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
    await pool.query(__createSql("`"));
    return pool;
  }
  return {
    async load() {
      const p = await open();
      const [rows] = await p.query(`SELECT ${__cols("`")} FROM ${__table("`")}`);
      return (rows as any[]).map(__fromRow);
    },
    async saveAll(items) {
      const p = await open();
      const s = __STORE_SCHEMA as __Schema;
      const ph = s.columns.map(() => "?").join(", ");
      const c = await p.getConnection();
      try {
        await c.beginTransaction();
        await c.query(`DELETE FROM ${__table("`")}`);
        for (const it of items) {
          await c.query(`INSERT INTO ${__table("`")} (${__cols("`")}) VALUES (${ph})`, __toRow(it) as any[]);
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
