// Runtime de Marea (generado automáticamente — no editar a mano).
//
// Contiene el transporte RPC que materializa la "frontera de red":
//   - lado servidor: un registro de handlers + un servidor HTTP mínimo.
//   - lado cliente:  __rpc(), que serializa la llamada y la manda por fetch.
// Más los builtins del lenguaje.

import http from "node:http";
import fs from "node:fs";

// Lee un entero positivo del entorno. Un valor ausente o mal formado cae al
// valor por defecto en vez de convertirse en NaN: `size > NaN` es siempre false,
// así que un `MAREA_MAX_BODY=ilimitado` desactivaría el tope del cuerpo, y un
// timeout NaN desactivaría las defensas anti-slowloris que decimos aplicar.
function __envInt(name: string, def: number): number {
  const raw = process.env[name];
  if (raw === undefined || raw === "") return def;
  const n = Number(raw);
  if (!Number.isFinite(n) || n <= 0) {
    console.warn(`[marea] ${name}='${raw}' no es un entero positivo; se usa ${def}`);
    return def;
  }
  return Math.floor(n);
}

// El puerto: PORT (lo fija el entorno de hosting, p.ej. Cloud Run) tiene
// prioridad, luego MAREA_PORT, y 8787 por defecto en local.
// PORT (hosting) tiene prioridad; si no está, MAREA_PORT; si no, 8787. Se elige
// la variable ANTES de convertir, para no avisar de una que no se va a usar.
const MAREA_PORT = __envInt(
  process.env.PORT !== undefined && process.env.PORT !== "" ? "PORT" : "MAREA_PORT",
  8787,
);
// Escuchamos solo en loopback por defecto: la frontera de red es para el cliente
// local de la app, no para exponer en la LAN. Se puede ampliar con MAREA_HOST.
const MAREA_HOST = process.env.MAREA_HOST ?? "127.0.0.1";
const MAREA_URL = `http://127.0.0.1:${MAREA_PORT}/__marea`;
// El puerto ya resuelto y validado, para que quien informe al usuario no tenga
// que recalcularlo (y no pueda equivocarse si el valor del entorno es basura).
export function puerto(): number {
  return MAREA_PORT;
}
// Tope del cuerpo RPC (configurable) para no acumular memoria sin límite.
const MAREA_MAX_BODY = __envInt("MAREA_MAX_BODY", 1_048_576); // 1 MiB

type Handler = (args: unknown[]) => unknown | Promise<unknown>;
// Tabla sin prototipo: así `fn` no puede resolver a métodos heredados de
// Object.prototype (constructor/toString/…) y solo alcanza handlers reales.
const __handlers: Record<string, Handler> = Object.create(null);

export function __register(name: string, fn: Handler): void {
  __handlers[name] = fn;
}

/// Error de validación del límite: lo provoca una petición mal formada, no un
/// fallo del servidor, así que se responde 400 y no 500.
export class __ErrorDeLimite extends Error {}

export function __malFormado(detalle: string): never {
  throw new __ErrorDeLimite(detalle);
}

let __server: http.Server | null = null;

// Sirve archivos estáticos de la app web (index.html, client.js) desde la raíz
// dada por MAREA_WEB_ROOT (leída en cada petición, así el entry puede fijarla
// tras los imports). Solo se activa si esa variable está puesta (el modo demo no
// la usa). Evita el path traversal: solo nombres simples bajo la raíz.
const __MIME: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json; charset=utf-8",
};
function __serveStatic(req: http.IncomingMessage, res: http.ServerResponse): boolean {
  const root = process.env.MAREA_WEB_ROOT ?? null;
  if (!root || req.method !== "GET") return false;
  let path = (req.url ?? "/").split("?")[0];
  if (path === "/") path = "/index.html";
  // Rechaza traversal: solo segmentos simples (sin '..', sin separadores extra).
  if (path.includes("..") || /[^A-Za-z0-9._/-]/.test(path)) {
    res.statusCode = 400;
    res.end();
    return true;
  }
  // Lista blanca de extensiones servibles. Sin ella, la raíz estática (que es
  // el propio directorio de salida) expone `server.ts` —la lista completa de
  // handlers, anulando la anti-enumeración del endpoint— y el `.log` del store
  // de archivo, es decir todos los datos persistidos.
  const dot = path.lastIndexOf(".");
  const ext = dot === -1 ? "" : path.slice(dot);
  const mime = __MIME[ext];
  if (mime === undefined) {
    res.statusCode = 404;
    res.end();
    return true;
  }
  const file = root + path;
  let data: Buffer;
  try {
    data = fs.readFileSync(file);
  } catch {
    res.statusCode = 404;
    res.end();
    return true;
  }
  res.setHeader("content-type", mime);
  res.end(data);
  return true;
}

export function startServer(): Promise<void> {
  return new Promise((resolve) => {
    __server = http.createServer((req, res) => {
      if (__serveStatic(req, res)) return;
      if (req.method !== "POST" || req.url !== "/__marea") {
        res.statusCode = 404;
        res.end();
        return;
      }
      // Exigir JSON no es cosmético: sin esta comprobación un formulario
      // cross-origin con enctype="text/plain" califica como "simple request",
      // se salta el preflight de CORS y ejecuta el handler. La respuesta le
      // queda opaca al atacante, pero el efecto lateral (guardar/borrar) ya
      // ocurrió. Ambos clientes generados mandan este content-type.
      const __ct = (req.headers["content-type"] ?? "").split(";")[0].trim();
      if (__ct !== "application/json") {
        res.statusCode = 415;
        res.end(JSON.stringify({ error: "se requiere content-type: application/json" }));
        return;
      }
      // Un navegador siempre manda Origin en una petición cross-origin. Si
      // viene y no está permitido, se rechaza: cierra el CSRF que quedara y el
      // DNS rebinding. MAREA_ALLOWED_ORIGINS lo amplía (lista separada por
      // comas); sin él solo se acepta el mismo host que sirve la app.
      const __origin = req.headers["origin"];
      if (typeof __origin === "string" && __origin !== "") {
        const permitidos = (process.env.MAREA_ALLOWED_ORIGINS ?? "")
          .split(",")
          .map((o) => o.trim())
          .filter((o) => o !== "");
        const host = req.headers["host"] ?? "";
        const mismoHost =
          __origin === `http://${host}` || __origin === `https://${host}`;
        if (!mismoHost && !permitidos.includes(__origin)) {
          res.statusCode = 403;
          res.end(JSON.stringify({ error: "origen no permitido" }));
          return;
        }
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
          if (e instanceof __ErrorDeLimite) {
            // Petición mal formada: es culpa del cliente, no del servidor. No se
            // hace eco del detalle para no dar un oráculo de las firmas.
            res.statusCode = 400;
            res.end(JSON.stringify({ error: "petición mal formada" }));
            return;
          }
          // El detalle se queda en el servidor; al cliente solo error genérico.
          console.error("[marea] error en handler:", e);
          res.statusCode = 500;
          res.end(JSON.stringify({ error: "error interno" }));
        }
      });
    });
    // Cortar conexiones lentas/colgadas (defensa contra slowloris y fugas).
    __server.requestTimeout = __envInt("MAREA_REQUEST_TIMEOUT", 15_000);
    __server.headersTimeout = __envInt("MAREA_HEADERS_TIMEOUT", 10_000);
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
// Escapa un texto para incrustarlo en HTML. El lenguaje construye marcado
// concatenando cadenas y 'render' lo inyecta por innerHTML, así que sin esto un
// dato persistido vía RPC se ejecuta como marcado en todos los clientes.
// Marca una cadena como marcado ya seguro. En runtime es la identidad:
// la garantía es estática (el tipo Html), esto solo la hace explícita
// en el fuente para que se vea en una revisión.
export function html(s: string): string {
  return s;
}
export function escapar(x: unknown): string {
  return String(x)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
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
// Modelo: el arreglo en memoria es la copia de trabajo con índices posicionales
// estables; un arreglo paralelo (__ids) mapea cada posición a un id persistente.
// Cada mutación toca SOLO su fila en el backend (insert/update/remove por id) —
// O(1) por operación, sin reescribir el store completo en cada cambio.

interface __Schema {
  table: string;
  columns: { name: string; kind: "text" | "int" | "real" | "bool" | "json" }[];
}
const __STORE_SCHEMA: __Schema | null = { table: "post", columns: [{ name: "autor", kind: "text" }, { name: "texto", kind: "text" }, { name: "likes", kind: "int" }] };

// Una fila persistida: id estable + el valor del .mar. El id desacopla la
// identidad en almacenamiento del índice posicional que ve el lenguaje.
interface __Row {
  id: number;
  item: unknown;
}
interface __Backend {
  load(): Promise<__Row[]>;
  insert(id: number, item: unknown): Promise<void>;
  update(id: number, item: unknown): Promise<void>;
  remove(id: number): Promise<void>;
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
function __colList(q: string): string[] {
  return (__STORE_SCHEMA as __Schema).columns.map((c) => __quoteId(c.name, q));
}
function __table(q: string): string {
  return __quoteId((__STORE_SCHEMA as __Schema).table, q);
}
// Columna interna de id (clave primaria). Convención '__id', como '__doc': el
// lexer permite el nombre, pero un campo de usuario así llamado es improbable.
function __idCol(q: string): string {
  return __quoteId("__id", q);
}
function __createSql(q: string): string {
  const s = __STORE_SCHEMA as __Schema;
  const defs = [
    `${__idCol(q)} INTEGER PRIMARY KEY`,
    ...s.columns.map((c) => `${__quoteId(c.name, q)} ${__sqlType(c.kind)}`),
  ].join(", ");
  return `CREATE TABLE IF NOT EXISTS ${__table(q)} (${defs})`;
}

// Constructores de SQL incremental, parametrizados por dialecto (`q` = comilla de
// identificador, `p(i)` = i-ésimo placeholder: '?' en sqlite/mysql, '$i' en pg).
function __selectSql(q: string): string {
  return `SELECT ${__idCol(q)}, ${__cols(q)} FROM ${__table(q)} ORDER BY ${__idCol(q)}`;
}
function __insertSql(q: string, p: (i: number) => string): string {
  const cols = [__idCol(q), ...__colList(q)];
  const vals = cols.map((_, i) => p(i + 1)).join(", ");
  return `INSERT INTO ${__table(q)} (${cols.join(", ")}) VALUES (${vals})`;
}
function __updateSql(q: string, p: (i: number) => string): string {
  const cols = __colList(q);
  const sets = cols.map((c, i) => `${c} = ${p(i + 1)}`).join(", ");
  return `UPDATE ${__table(q)} SET ${sets} WHERE ${__idCol(q)} = ${p(cols.length + 1)}`;
}
function __deleteSql(q: string, p: (i: number) => string): string {
  return `DELETE FROM ${__table(q)} WHERE ${__idCol(q)} = ${p(1)}`;
}
// Fila SQL -> __Row. __fromRow ignora la columna __id (solo lee las del esquema).
function __rowFrom(row: any): __Row {
  return { id: Number(row.__id), item: __fromRow(row) };
}

// --- backend: archivo (por defecto, cero dependencias) ---
//
// Formato: LOG append-only JSONL, una línea por mutación {op,id,item}. Escribir
// es O(1) (un append pequeño y atómico) en vez de reserializar el store entero
// en cada cambio; al cargar se reproduce el log. Se compacta al cargar si creció
// mucho respecto a las filas vivas, así no crece sin límite entre reinicios.
function __fileBackend(): __Backend {
  const file = process.env.MAREA_STORE ?? "marea-store.Post-autor-texto-likes.log";
  // Append síncrono de UNA línea: O(1) y atómico para escrituras pequeñas; no
  // congela el event-loop como sí lo haría volcar todo el store.
  function append(op: object): void {
    fs.appendFileSync(file, JSON.stringify(op) + "\n");
  }
  // Reescribe el log compactado (solo inserts de las filas vivas), atómicamente.
  function compact(rows: __Row[]): void {
    const tmp = `${file}.${process.pid}.tmp`;
    const body = rows.map((r) => JSON.stringify({ op: "i", id: r.id, item: r.item })).join("\n");
    const fd = fs.openSync(tmp, "w");
    try {
      fs.writeFileSync(fd, body.length ? body + "\n" : "");
      fs.fsyncSync(fd);
    } finally {
      fs.closeSync(fd);
    }
    fs.renameSync(tmp, file);
  }
  return {
    async load() {
      let data: string;
      try {
        data = fs.readFileSync(file, "utf8");
      } catch {
        return [];
      }
      // Map preserva el orden de inserción: un 'u' sobre una clave existente
      // conserva su posición; 'i' agrega al final; 'd' la quita.
      const live = new Map<number, unknown>();
      let ops = 0;
      for (const ln of data.split("\n")) {
        if (!ln) continue;
        let op: any;
        try {
          op = JSON.parse(ln);
        } catch {
          // Línea corrupta (p.ej. el último append truncado por un crash): se
          // ignora y el resto del log sigue siendo válido.
          continue;
        }
        ops++;
        if (op.op === "d") live.delete(op.id);
        else live.set(op.id, op.item); // 'i' o 'u': última escritura gana
      }
      const rows: __Row[] = [...live.entries()].map(([id, item]) => ({ id, item }));
      if (ops > rows.length * 2 + 64) compact(rows);
      return rows;
    },
    async insert(id, item) {
      append({ op: "i", id, item });
    },
    async update(id, item) {
      append({ op: "u", id, item });
    },
    async remove(id) {
      append({ op: "d", id });
    },
  };
}

// --- backend: SQLite (módulo integrado node:sqlite, cero dependencias) ---
function __sqliteBackend(): __Backend {
  const path = process.env.MAREA_DB_URL ?? "marea.sqlite";
  const q = '"';
  const p = () => "?";
  let db: any = null;
  async function open() {
    if (db) return db;
    const { DatabaseSync } = await import("node:sqlite");
    db = new DatabaseSync(path);
    db.exec(__createSql(q));
    return db;
  }
  return {
    async load() {
      const d = await open();
      return d.prepare(__selectSql(q)).all().map(__rowFrom);
    },
    async insert(id, item) {
      (await open()).prepare(__insertSql(q, p)).run(id, ...(__toRow(item) as any[]));
    },
    async update(id, item) {
      (await open()).prepare(__updateSql(q, p)).run(...(__toRow(item) as any[]), id);
    },
    async remove(id) {
      (await open()).prepare(__deleteSql(q, p)).run(id);
    },
  };
}

// --- backend: PostgreSQL (driver 'pg', import perezoso; requiere MAREA_DB_URL) ---
function __postgresBackend(): __Backend {
  const q = '"';
  const p = (i: number) => `$${i}`;
  let pool: any = null;
  async function open() {
    if (pool) return pool;
    const pg = await import("pg");
    pool = new pg.Pool({ connectionString: process.env.MAREA_DB_URL });
    await pool.query(__createSql(q));
    return pool;
  }
  return {
    async load() {
      const r = await (await open()).query(__selectSql(q));
      return r.rows.map(__rowFrom);
    },
    async insert(id, item) {
      await (await open()).query(__insertSql(q, p), [id, ...(__toRow(item) as any[])]);
    },
    async update(id, item) {
      await (await open()).query(__updateSql(q, p), [...(__toRow(item) as any[]), id]);
    },
    async remove(id) {
      await (await open()).query(__deleteSql(q, p), [id]);
    },
  };
}

// --- backend: MySQL (driver 'mysql2/promise', import perezoso; MAREA_DB_URL) ---
function __mysqlBackend(): __Backend {
  const q = "`";
  const p = () => "?";
  let pool: any = null;
  async function open() {
    if (pool) return pool;
    const mysql = await import("mysql2/promise");
    pool = mysql.createPool(process.env.MAREA_DB_URL ?? "");
    await pool.query(__createSql(q));
    return pool;
  }
  return {
    async load() {
      const [rows] = await (await open()).query(__selectSql(q));
      return (rows as any[]).map(__rowFrom);
    },
    async insert(id, item) {
      await (await open()).query(__insertSql(q, p), [id, ...(__toRow(item) as any[])]);
    },
    async update(id, item) {
      await (await open()).query(__updateSql(q, p), [...(__toRow(item) as any[]), id]);
    },
    async remove(id) {
      await (await open()).query(__deleteSql(q, p), [id]);
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
  // Empaqueta el item como documento con _id = id estable. Para el caso escalar
  // (__doc) el valor va en un campo; para registros se esparce el objeto.
  function __doc(id: number, item: unknown): any {
    return __isDoc() ? { _id: id, __doc: item } : { _id: id, ...(item as object) };
  }
  function __itemOf(d: any): unknown {
    if (__isDoc()) return d.__doc;
    const { _id, ...rest } = d;
    return rest;
  }
  return {
    async load() {
      const c = await open();
      const docs = (await c.find({}).sort({ _id: 1 }).toArray()) as any[];
      return docs.map((d) => ({ id: Number(d._id), item: __itemOf(d) }));
    },
    async insert(id, item) {
      await (await open()).insertOne(__doc(id, item));
    },
    async update(id, item) {
      await (await open()).replaceOne({ _id: id }, __doc(id, item));
    },
    async remove(id) {
      await (await open()).deleteOne({ _id: id });
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
let __store: unknown[] | null = null; // valores, por posición (lo que ve el .mar)
let __ids: number[] = []; // id persistente paralelo a cada posición de __store
let __nextId = 1; // contador de ids nuevos

// La carga se memoiza como PROMESA, no como resultado. El servidor atiende
// peticiones concurrentemente: si solo se comprobara `__store === null`, dos RPC
// simultáneos entrarían ambos en la rama durante el `await load()` (que cede),
// crearían dos backends, y el segundo pisaría el `__store` del primero
// reiniciando `__nextId` → ids duplicados (violación de PK en SQL/Mongo) y, en
// el backend de archivo, pérdida silenciosa de escrituras por "última gana".
let __loading: Promise<unknown[]> | null = null;

async function __ensureStore(): Promise<unknown[]> {
  if (__store !== null) return __store;
  if (__loading === null) {
    __loading = (async () => {
      if (__backend === null) __backend = __makeBackend();
      const rows = await __backend.load();
      __store = rows.map((r) => r.item);
      __ids = rows.map((r) => r.id);
      __nextId = rows.reduce((m, r) => Math.max(m, r.id), 0) + 1;
      return __store;
    })().catch((e) => {
      // Un fallo de carga no debe dejar el store envenenado para siempre:
      // se limpia la promesa para que el siguiente intento reintente.
      __loading = null;
      throw e;
    });
  }
  return __loading;
}

export async function guardar(x: unknown): Promise<void> {
  const s = await __ensureStore();
  const id = __nextId++;
  s.push(x);
  __ids.push(id);
  await (__backend as __Backend).insert(id, x);
}
export async function todos(): Promise<unknown[]> {
  return (await __ensureStore()).slice();
}
// Reemplaza el elemento en el índice 'i' (CRUD: update) — solo toca esa fila.
export async function actualizar(i: number, x: unknown): Promise<void> {
  const s = await __ensureStore();
  if (i >= 0 && i < s.length) {
    s[i] = x;
    await (__backend as __Backend).update(__ids[i], x);
  }
}
// Elimina el elemento en el índice 'i' (CRUD: delete) — solo borra esa fila.
export async function borrar(i: number): Promise<void> {
  const s = await __ensureStore();
  if (i >= 0 && i < s.length) {
    const id = __ids[i];
    s.splice(i, 1);
    __ids.splice(i, 1);
    await (__backend as __Backend).remove(id);
  }
}
