// Runtime de Marea (generado automáticamente — no editar a mano).
//
// Contiene el transporte RPC que materializa la "frontera de red":
//   - lado servidor: un registro de handlers + un servidor HTTP mínimo.
//   - lado cliente:  __rpc(), que serializa la llamada y la manda por fetch.
// Más los builtins del lenguaje.

// el módulo no cruza la frontera de red ni declara almacenes. Aquí dentro está
// todo lo que ata este runtime a Node: `node:http`, `node:fs` y `process.env`.
// Un módulo que sólo calcula y devuelve marcado no necesita nada de esto, y
// arrastrarlo le impide vivir en un componente de cliente o en el edge.
import http from "node:http";
import fs from "node:fs";

// El `fetch` del entorno, capturado antes de que lo tape el builtin homónimo de
// Marea (`export function fetch`, más abajo). Una declaración de módulo gana al
// global en TODO el archivo, así que sin esto `__rpc` acababa llamando al
// builtin del lenguaje —que pasa por la lista blanca anti-SSRF y rechaza
// loopback, es decir el propio servidor— y `__http` se llamaba a sí mismo.
const __fetchDelEntorno: typeof globalThis.fetch = globalThis.fetch;

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

// El segundo parámetro es la identidad ya resuelta: la pasa el runtime, nunca
// el cliente. Los handlers sin política la ignoran.
type Handler = (args: unknown[], identidad?: unknown) => unknown | Promise<unknown>;
// Tabla sin prototipo: así `fn` no puede resolver a métodos heredados de
// Object.prototype (constructor/toString/…) y solo alcanza handlers reales.
const __handlers: Record<string, Handler> = Object.create(null);

// --------------------------------------------------------------------------
// POLÍTICA: quién puede cruzar la frontera
//
// El compilador ya obliga a que cada handler diga a quién deja llegar
// (`@server(Usuario)` o `@server(Public)`). Eso, por sí solo, es una garantía de
// COMPILACIÓN: el endpoint seguiría atendiendo a cualquiera. Aplicarla es cosa
// de aquí — sacar el token de la petición, resolverlo con la `@session` del
// programa y no ejecutar el cuerpo si no hay identidad.
//
// QUÉ CUENTA COMO IDENTIDAD. La `@session` devuelve una unión (`Usuario |
// NoAutorizado`) y el codegen representa una variante nominal como
// `{ $tag: "NoAutorizado" }`; el lexer no admite '$' en un identificador, así
// que ningún registro del usuario puede fabricar ese campo. La identidad, en
// cambio, es siempre un tipo DECLARADO —el verificador exige que la política
// nombre un alias del programa—, es decir un registro o un escalar. De ahí la
// regla, que es la que se documenta y la única que aplica el runtime:
//
//     autorizado  <=>  el valor existe y NO es una variante etiquetada
//
// Se elige así, y no por lista de fallos conocidos, porque falla CERRADO: una
// variante nueva (`Caducado`, `Bloqueado`) deniega sin tocar el runtime, y un
// resolutor que se sale por un camino sin `return` devuelve `undefined`, que
// tampoco autoriza.
type Resolutor = (token: string) => unknown | Promise<unknown>;
const __politicas: Record<string, Resolutor> = Object.create(null);

export function __register(name: string, fn: Handler, resolutor?: Resolutor): void {
  __handlers[name] = fn;
  if (resolutor !== undefined) __politicas[name] = resolutor;
}

export function __esIdentidad(v: unknown): boolean {
  if (v === null || v === undefined) return false;
  if (typeof v === "object" && typeof (v as Record<string, unknown>).$tag === "string") {
    return false;
  }
  return true;
}

// El token viaja en `authorization: Bearer <t>` y, si no está ahí, en la cookie
// `marea_session` —que es la vía del navegador: la adjunta sola en una petición
// al mismo origen, sin que el stub generado tenga que cambiar—. Que la cookie
// sea automática es justo lo que abre el CSRF, así que esta ruta se apoya en las
// defensas que ya tiene el endpoint (content-type JSON obligatorio y Origin
// comprobado); marca la cookie SameSite cuando la emitas.
function __tokenDe(req: http.IncomingMessage): string {
  const auth = req.headers["authorization"];
  if (typeof auth === "string") {
    const m = /^bearer[ \t]+(.+)$/i.exec(auth.trim());
    if (m) return m[1].trim();
  }
  const cookie = req.headers["cookie"];
  if (typeof cookie === "string") {
    for (const trozo of cookie.split(";")) {
      const i = trozo.indexOf("=");
      if (i === -1) continue;
      if (trozo.slice(0, i).trim() !== "marea_session") continue;
      let v = trozo.slice(i + 1).trim();
      // RFC 6265 permite el valor entre comillas.
      if (v.length >= 2 && v.startsWith('"') && v.endsWith('"')) v = v.slice(1, -1);
      try {
        return decodeURIComponent(v);
      } catch {
        return v;
      }
    }
  }
  // Sin token la `@session` recibe "" y decide ella: el runtime no adivina qué
  // es una credencial válida, que es precisamente lo que el programa declara.
  return "";
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
      // queda opaca al atacante, pero el efecto lateral (save/remove) ya
      // ocurrió. Ambos clientes generados mandan este content-type.
      const __ct = (req.headers["content-type"] ?? "").split(";")[0].trim().toLowerCase();
      if (__ct !== "application/json") {
        res.statusCode = 415;
        res.end(JSON.stringify({ error: "se requiere content-type: application/json" }));
        return;
      }
      // Defensa en profundidad CONTRA NAVEGADORES, no autenticación: cierra el
      // CSRF clásico (evil.com no puede falsear Origin). NO cierra el DNS
      // rebinding por sí sola —ahí el atacante controla Host y Origin y los hace
      // coincidir—, por eso se valida además Host contra MAREA_ALLOWED_HOSTS
      // cuando está puesto. Sin autenticación de verdad, esto es un cinturón,
      // no un candado.
      const __origin = req.headers["origin"];
      if (typeof __origin === "string" && __origin !== "") {
        const permitidos = (process.env.MAREA_ALLOWED_ORIGINS ?? "")
          .split(",")
          .map((o) => o.trim())
          .filter((o) => o !== "");
        const host = req.headers["host"] ?? "";
        // El Host lo elige el cliente, así que compararlo consigo mismo no
        // impide el rebinding. Con MAREA_ALLOWED_HOSTS se fija cuál es válido.
        const hostsOk = (process.env.MAREA_ALLOWED_HOSTS ?? "")
          .split(",")
          .map((h) => h.trim())
          .filter((h) => h !== "");
        if (hostsOk.length > 0 && !hostsOk.includes(host)) {
          res.statusCode = 403;
          res.end(JSON.stringify({ error: "host no permitido" }));
          return;
        }
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
        // La política se aplica ANTES del cuerpo: si el token no resuelve a una
        // identidad, el handler no llega a ejecutarse (ni sus efectos).
        let __identidad: unknown = undefined;
        const resolutor = __politicas[fn];
        if (resolutor !== undefined) {
          let quien: unknown;
          try {
            quien = await resolutor(__tokenDe(req));
          } catch (e) {
            // Que el resolutor reviente no autoriza a nadie; el detalle se queda
            // en el log del servidor.
            console.error("[marea] la @session falló:", e);
            quien = undefined;
          }
          if (!__esIdentidad(quien)) {
            // Misma respuesta para "sin token", "token inválido" y "la @session
            // falló": distinguirlas sería un oráculo con el que ir probando.
            res.statusCode = 401;
            res.setHeader("www-authenticate", "Bearer");
            res.end(JSON.stringify({ error: "no autorizado" }));
            return;
          }
          __identidad = quien;
        }
        try {
          const result = await handler(args as unknown[], __identidad);
          res.setHeader("content-type", "application/json");
          res.end(JSON.stringify({ ok: result }));
        } catch (e) {
          if (e instanceof __BoundaryError) {
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
  const res = await __fetchDelEntorno(MAREA_URL, {
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


// Núcleo compartido de los DOS runtimes de Marea (generado — no editar a mano).
//
// QUÉ ES. La única implementación de lo que Node y el navegador necesitan
// igual: el núcleo reactivo (signal / memo / effect / resource) y los builtins
// puros del lenguaje. Antes vivían por duplicado en `runtime.ts` y en
// `browser.js` y había que sincronizarlas a mano; el bug de `.append()` sobre un
// `Set` —un Set de JS tiene `.add`— estaba en los SEIS sitios, tres por runtime,
// y rompía el núcleo reactivo entero en los dos blancos a la vez.
//
// CÓMO LLEGA A CADA UNO. El codegen sustituye por este archivo el bloque
// `@marea:nucleo-inicio/fin` de `runtime.ts` y el de `browser.js`. Se INSERTA,
// no se importa: la salida tiene un juego de archivos fijo, y el navegador
// recibe `client.js` tal cual, sin un `import` más que resolver ni servir.
//
// POR QUÉ LOS TIPOS VAN EN COMENTARIO. Este texto acaba en los dos sitios: en
// `client.js`, que el navegador ejecuta como JavaScript plano, y dentro de
// `runtime.ts`, que pasa por `tsc --strict`. Escribirlo en TypeScript rompe lo
// primero. Escribirlo con tipos en JSDoc no arregla lo segundo: `tsc` IGNORA los
// tipos de JSDoc dentro de un `.ts` —solo los lee en un `.js`—, así que al
// quedar aquí dentro cada parámetro sería un `any` implícito y `--strict`
// cortaría. De modo que el tipo viaja dentro de un comentario de bloque marcado
// con `ts`. La línea de ejemplo que sigue está escrita con la marca puesta, así
// que en cada salida se lee ya convertida — que es la demostración más corta que
// hay de lo que hace la regla:
//
//     export function html(s: string): string { return s; }
//
//   - hacia `runtime.ts` se quitan las marcas del comentario y queda TypeScript
//     de verdad, que `--strict` comprueba entero, anotación por anotación;
//   - hacia `client.js` se quita el comentario completo y queda JavaScript.
//
// Es UNA regla textual, y lo que va dentro del comentario es TypeScript literal:
// no hay traducción por medio que pueda mentir. Y lo que este archivo ES —el
// JavaScript que corre en el navegador— se comprueba aparte con `tsc --checkJs`,
// que es justo quien cazó el `.append` sobre el `Set`.

/// Error de validación del límite: lo provoca una petición mal formada, no un
/// fallo del servidor, así que el servidor responde 400 y no 500. Vive aquí
/// porque los builtins que cortan (división entre cero, índice fuera de rango)
/// lo lanzan en los dos runtimes.
export class __BoundaryError extends Error {}

// Comparación de variantes para 'match' (best-effort hasta tener uniones reales).
export function __marea_is(value: unknown, tag: string): boolean {
  return (
    value !== null &&
    typeof value === "object" &&
    (value as Record<string, unknown>).$tag === tag
  );
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

export interface Cell<T> {
  get(): T;
  set(v: T): void;
}


let __currentSub: Reaction | null = null;
const __pending: Set<EffectReaction> = new Set();
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

export function __signal<T>(initial: T): Cell<T> {
  let value = initial;
  const subs: Set<Reaction> = new Set();
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

// Un RECURSO: la composición de las dos fronteras. Arranca en `Cargando`, lanza
// la llamada asíncrona y se pone al resultado cuando llega, o a `Fallo` si
// revienta. Como es un signal, cualquier vista que lo lea se re-pinta sola en
// cada transición: no hay que orquestar nada a mano.
export function __resource(f: () => Promise<unknown>): Cell<unknown> {
  const s = __signal<unknown>({ $tag: "Cargando" });
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

export function __memo<T>(fn: () => T): Cell<T> {
  const subs: Set<Reaction> = new Set();
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
    // Un memo es DERIVADO: asignarle no significa nada, y el verificador ya
    // rechaza `x = ...` sobre una `reactive` sin `mut` (E_ASSIGN_IMMUTABLE). Si
    // aun así se llega aquí —con `--no-check`, o desde JS a mano— se avisa en
    // vez de tragárselo: la versión muda de Node perdía la escritura en silencio
    // mientras la del navegador ya avisaba, y no puede haber dos respuestas.
    set(_v: T): void {
      throw new Error("una derivada 'reactive' es de solo lectura");
    },
  };
}

// --- builtins del lenguaje ---

export function print(x: unknown): void {
  console.log(x);
}

// `concat` y `len` sirven para texto Y para listas: son la misma idea sobre dos
// estructuras, y tener `unir`/`largo` aparte solo duplicaba la superficie.
export function concat(a: unknown, b: unknown): unknown {
  if (Array.isArray(a) && Array.isArray(b)) return a.concat(b);
  return String(a) + String(b);
}

export function len(x: unknown): number {
  if (Array.isArray(x)) return x.length;
  // Se cuentan puntos de código, no unidades UTF-16: "🌊" mide 1, no 2.
  return Array.from(String(x)).length;
}

export function text(x: unknown): string {
  // Una variante se imprime por su nombre; si no, saldría "[object Object]".
  if (x !== null && typeof x === "object") {
    const t = (x as Record<string, unknown>).$tag;
    if (typeof t === "string") return t;
  }
  return String(x);
}

// 'render' pinta en el #app cuando hay DOM (el navegador) y registra en consola
// cuando no lo hay (Node). Una sola implementación: la rama que sobra en cada
// blanco no se toma, y así el mismo programa no tiene dos comportamientos según
// qué archivo se copió.
export function render(x: unknown): void {
  const app = typeof document !== "undefined" && document.getElementById("app");
  if (app) {
    const marcado = String(x);
    // Podar ANTES de escribir: el marcado que entra es el que dice qué
    // manejadores siguen vivos (ver `__podar`).
    __podar(marcado);
    app.innerHTML = marcado;
  } else console.log("[render]", x);
}

// --- modelo de eventos: la frontera del TIEMPO en dirección al programa ---
//
// QUÉ RESUELVE. El reactivo pinta de estado a DOM; esto es la vuelta. `on` es un
// builtin del lenguaje: recibe el nombre del evento y un CIERRE, y devuelve el
// ATRIBUTO con el que ese elemento queda atado al manejador. Como devuelve
// marcado (`Html`), encaja en un hueco crudo de plantilla sin sintaxis nueva:
//
//     `<button {!on("click", fn() { cuenta = cuenta + 1; })}>suma</button>`
//
// POR QUÉ POR DELEGACIÓN. El cierre NO se puede escribir en el atributo: es
// código con entorno capturado, y un atributo solo guarda texto —de ahí venía el
// apaño de `onclick="marea.f(3)"`, que obligaba a exponer funciones globales y
// no lo miraba nadie—. Así que el atributo lleva un ID, el cierre vive en esta
// tabla, y hay UN oyente por tipo de evento colgado del documento que busca el
// ID y lo invoca. Enganchar el oyente a cada elemento sería el fallo clásico:
// re-pintar tira los nodos y sus oyentes, y quien los puso no se entera —o los
// vuelve a poner encima de los viejos—. Colgados del documento, que ningún
// re-pintado toca, no hay oyente huérfano ni duplicado que perseguir.
//
// EN FASE DE CAPTURA. Es la única que alcanza a los eventos que NO burbujean
// (`blur`, `pointerleave`): del documento hacia el objetivo pasa siempre, y
// hacia arriba solo pasan los que burbujean. Con una sola regla para los once.


interface __Manejador {
  fn: () => unknown;
}


/// Los cierres vivos, por ID. Cada `on` mete uno; `__podar` saca los que ya no
/// nombra el marcado que está en pantalla.
const __manejadores: Map<string, __Manejador> = new Map();
/// Los tipos de evento que ya tienen su oyente en el documento. Uno por tipo,
/// puesto la primera vez que hace falta y nunca retirado.
const __enganchados: Set<string> = new Set();
let __idManejador = 0;

// Estos dos no burbujean, así que un manejador suyo solo puede ser el del
// elemento que recibió el evento: subir buscando uno haría saltar el del
// contenedor cuando el puntero pasa de un hijo a otro.
const __SIN_BURBUJEO: Set<string> = new Set(["blur", "pointerleave"]);

export function on(evento: string, manejador: () => unknown): string {
  const id = "h" + ++__idManejador;
  __manejadores.set(id, { fn: manejador });
  __engancharRaiz(evento);
  // Un atributo por tipo de evento: dos `on` distintos en la misma etiqueta
  // serían dos atributos con el mismo nombre y el analizador de HTML se quedaría
  // con el primero, callándose el segundo.
  return `data-marea-on-${evento}="${id}"`;
}

function __engancharRaiz(evento: string): void {
  if (typeof document === "undefined" || __enganchados.has(evento)) return;
  __enganchados.add(evento);
  document.addEventListener(evento, (ev) => __despachar(evento, ev), true);
}

/// Busca el manejador desde el objetivo hacia arriba y ejecuta el PRIMERO que
/// encuentra. Uno solo: que un clic dispare además los manejadores de todos los
/// contenedores de encima es la clase de sorpresa que se paga depurando.
function __despachar(evento: string, ev: Event): void {
  const atributo = "data-marea-on-" + evento;
  let nodo = (ev.target as Element | null);
  while (nodo) {
    const id = typeof nodo.getAttribute === "function" ? nodo.getAttribute(atributo) : null;
    if (id !== null) {
      const entrada = __manejadores.get(id);
      if (entrada) {
        // Un `submit` navega por defecto, y navegar tira el estado de la app
        // entera: un manejador de Marea existe precisamente para atenderlo aquí.
        if (evento === "submit") ev.preventDefault();
        void entrada.fn();
      }
      return;
    }
    if (__SIN_BURBUJEO.has(evento)) return;
    nodo = (nodo.parentNode as Element | null);
  }
}

/// Tira los cierres que el marcado entrante ya no nombra.
///
/// La vida de un manejador es la del elemento que lo lleva, y el elemento vive
/// mientras esté en el marcado que se escribe en el `#app`: ese texto es la
/// lista de los vivos, así que no hace falta llevar la cuenta de re-pintados ni
/// adivinar cuándo caduca uno. Sin esto la tabla crecería un cierre por botón y
/// por re-pintado —un contador pulsado mil veces dejaría mil cierres muertos.
export function __podar(html: string): void {
  const vivos: Set<string> = new Set();
  for (const m of html.matchAll(/data-marea-on-[a-z]+="([^"]*)"/g)) vivos.add(m[1]);
  for (const id of [...__manejadores.keys()]) {
    if (!vivos.has(id)) __manejadores.delete(id);
  }
}

// División entera. En JS `7/0` es Infinity y `0/0` es NaN: valores que no son
// enteros y que se colarían dentro de un Int mintiendo sobre su tipo. El backend
// WASM trapea, así que aquí también se corta —el mismo programa no puede dar
// Infinity en un blanco y morir en el otro—. Trunca HACIA CERO, igual que
// `i32.div_s`: `-7/2` es -3, no -4.
export function __div(a: number, b: number): number {
  if (b === 0) {
    throw new __BoundaryError("división entre cero");
  }
  return Math.trunc(a / b);
}

export function __rem(a: number, b: number): number {
  if (b === 0) {
    throw new __BoundaryError("módulo entre cero");
  }
  return a % b;
}

// Escapa un texto para incrustarlo en HTML. El lenguaje construye marcado
// concatenando cadenas y 'render' lo inyecta por innerHTML, así que sin esto un
// dato persistido vía RPC se ejecuta como marcado en todos los clientes. El `&`
// va PRIMERO: después, el `&` de `&lt;` se volvería a escapar.
export function escape(x: unknown): string {
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
export function html(s: string): string {
  return s;
}

// --- listas: construir en runtime (el lenguaje no tiene bucles ni cierres, así
// que sin esto una función no puede devolver un subconjunto filtrado) ---

export function append(xs: unknown[], x: unknown): unknown[] {
  return xs.concat([x]);
}

// Indexado de lista con verificación de rango: un índice fuera de rango lanza
// un error claro en vez de devolver 'undefined' (que reventaría más tarde).
export function __index<T>(xs: T[], i: number): T {
  if (i < 0 || i >= xs.length) {
    // Si el índice vino de la red es una petición mal formada (400), no un
    // fallo del servidor: si no, el cliente induce 500 y ruido de log a placer.
    throw new __BoundaryError(`índice fuera de rango: ${i} (longitud ${xs.length})`);
  }
  return xs[i];
}

// --- texto ---

export function contains(s: string, sub: string): boolean {
  return String(s).includes(String(sub));
}

export function lower(s: string): string {
  return String(s).toLowerCase();
}

export function __badRequest(detalle: string): never {
  throw new __BoundaryError(detalle);
}

// módulo no declara ningún `store`. Aquí viven los backends de persistencia,
// y tres de ellos hacen `import("pg"|"mysql2"|"mongodb")`: paquetes de npm
// que un consumidor sin base de datos no instala. Un empaquetador (Next,
// Vite) resuelve ese especificador aunque el código sea inalcanzable, así
// que dejarlos ahí rompe el build de quien sólo quería generar HTML.
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
  // `store x: T from "tabla"`: la tabla NO es de Marea. Con esto el esquema deja
  // de ser una orden ("crea esta tabla") y pasa a ser una expectativa ("esta
  // tabla ya existe y debe tener estas columnas"). El almacén es de sólo
  // lectura: no se emite CREATE TABLE, no hay columnas '__id'/'__doc' —una tabla
  // ajena no las tiene— y escribir en ella es un error.
  prestado?: boolean;
}

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

// el módulo no cruza la frontera de red ni declara almacenes. Aquí dentro está
// todo lo que ata este runtime a Node: `node:http`, `node:fs` y `process.env`.
// Un módulo que sólo calcula y devuelve marcado no necesita nada de esto, y
// arrastrarlo le impide vivir en un componente de cliente o en el edge.
// --------------------------------------------------------------------------
// RED SALIENTE
//
// Dar acceso a la red desde el servidor abre SSRF: si una @server recibe la URL
// del cliente, el atacante consigue que el servidor pida por él —y el servidor
// suele estar dentro de la red, con acceso a metadatos del proveedor y a
// servicios internos que él no alcanza—. Por eso:
//   - solo http/https,
//   - se bloquean loopback, enlace local, metadatos de nube y rangos privados,
//   - MAREA_HTTP_HOSTS restringe a una lista blanca (recomendado en producción),
//   - hay tope de tiempo y de tamaño de respuesta.
// --------------------------------------------------------------------------

const __RANGOS_PRIVADOS = [
  /^127\./, /^10\./, /^192\.168\./, /^169\.254\./, /^0\./,
  /^172\.(1[6-9]|2\d|3[01])\./,
];

function __hostBloqueado(host: string): boolean {
  const h = host.toLowerCase().replace(/^\[|\]$/g, "");
  if (h === "localhost" || h.endsWith(".localhost") || h.endsWith(".internal")) return true;
  if (h === "::1" || h === "0.0.0.0") return true;
  // IPv6 local (fc00::/7 y fe80::/10) y IPv4 mapeada.
  if (/^f[cd]/.test(h) || /^fe[89ab]/.test(h)) return true;
  return __RANGOS_PRIVADOS.some((r) => r.test(h));
}

function __urlPermitida(u: string): URL {
  let url: URL;
  try {
    url = new URL(u);
  } catch {
    __badRequest("url inválida");
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") {
    __badRequest("esquema no permitido");
  }
  const lista = (process.env.MAREA_HTTP_HOSTS ?? "")
    .split(",")
    .map((h) => h.trim().toLowerCase())
    .filter((h) => h !== "");
  if (lista.length > 0) {
    if (!lista.includes(url.hostname.toLowerCase())) {
      __badRequest("host no permitido");
    }
    return url;
  }
  // Sin lista blanca se acepta cualquier host PÚBLICO. Los privados se bloquean
  // siempre: son justo el objetivo del SSRF.
  if (__hostBloqueado(url.hostname)) {
    __badRequest("host no permitido");
  }
  return url;
}

async function __http(u: string, metodo: string, cuerpo: string | null): Promise<string> {
  const url = __urlPermitida(u);
  const tope = __envInt("MAREA_HTTP_MAX", 1_048_576);
  const control = new AbortController();
  const reloj = setTimeout(() => control.abort(), __envInt("MAREA_HTTP_TIMEOUT", 10_000));
  try {
    const res = await __fetchDelEntorno(url, {
      method: metodo,
      headers: cuerpo === null ? {} : { "content-type": "application/json" },
      body: cuerpo === null ? undefined : cuerpo,
      redirect: "error", // un redirect puede saltarse la lista blanca
      signal: control.signal,
    });
    const largoCabecera = Number(res.headers.get("content-length") ?? "0");
    if (largoCabecera > tope) {
      throw new Error("respuesta demasiado grande");
    }
    const texto = await res.text();
    if (texto.length > tope) {
      throw new Error("respuesta demasiado grande");
    }
    return texto;
  } finally {
    clearTimeout(reloj);
  }
}

export function fetch(url: string): Promise<string> {
  return __http(url, "GET", null);
}
export function post(url: string, cuerpo: string): Promise<string> {
  return __http(url, "POST", cuerpo);
}


// --- lectura de JSON por ruta ---
// El lenguaje no tiene valores dinámicos, así que una respuesta se lee por
// ruta con puntos: "current.temperature_2m", "results.0.nombre".
function __jsonEn(texto: string, ruta: string): unknown {
  let v: unknown;
  try {
    v = JSON.parse(texto);
  } catch {
    return undefined;
  }
  if (ruta === "") return v;
  for (const parte of ruta.split(".")) {
    if (v === null || v === undefined) return undefined;
    if (Array.isArray(v)) {
      const i = Number(parte);
      if (!Number.isInteger(i)) return undefined;
      v = v[i];
    } else if (typeof v === "object") {
      // Sin prototipo por medio: __proto__ y constructor no son rutas válidas.
      v = Object.prototype.hasOwnProperty.call(v, parte)
        ? (v as Record<string, unknown>)[parte]
        : undefined;
    } else {
      return undefined;
    }
  }
  return v;
}

export function jsonText(texto: string, ruta: string): string {
  const v = __jsonEn(texto, ruta);
  if (v === undefined || v === null) return "";
  if (typeof v === "object") return JSON.stringify(v);
  return String(v);
}
export function jsonInt(texto: string, ruta: string): number {
  const v = __jsonEn(texto, ruta);
  const n = Number(v);
  return Number.isFinite(n) ? Math.trunc(n) : 0;
}
export function jsonFloat(texto: string, ruta: string): number {
  const v = __jsonEn(texto, ruta);
  const n = Number(v);
  return Number.isFinite(n) ? n : 0;
}
export function jsonLen(texto: string, ruta: string): number {
  const v = __jsonEn(texto, ruta);
  if (Array.isArray(v)) return v.length;
  if (v !== null && typeof v === "object") return Object.keys(v).length;
  return 0;
}

// módulo no declara ningún `store`. Aquí viven los backends de persistencia,
// y tres de ellos hacen `import("pg"|"mysql2"|"mongodb")`: paquetes de npm
// que un consumidor sin base de datos no instala. Un empaquetador (Next,
// Vite) resuelve ese especificador aunque el código sea inalcanzable, así
// que dejarlos ahí rompe el build de quien sólo quería generar HTML.
// --------------------------------------------------------------------------
// ALMACENES CON NOMBRE
//
// `store productos: Producto;` declara un almacén; el nombre se pasa como primer
// argumento a los builtins de estado (`all(productos)`), así que un módulo
// puede tener varios. Cada almacén tiene su propio backend, su propia tabla o
// archivo y su propio contador de ids: la factoría cierra sobre el esquema en
// vez de leer una constante global, que era lo que ataba el runtime a uno solo.
// --------------------------------------------------------------------------

export interface __Store {
  nombre: string;
  save(x: unknown): Promise<void>;
  // `any[]` y no `unknown[]`: el VERIFICADOR de Marea ya garantiza el tipo de
  // los elementos —un almacén se declara `store posts: Post;` y `all(posts)` se
  // tipa `List<Post>`—, así que la garantía existe, sólo que la da él y no
  // TypeScript. Con `unknown[]` habría que castear en cada uso, y anotar el
  // retorno de una función que lee el almacén dejaba de compilar. Es el mismo
  // criterio que traduce el `Unknown` de Marea a `any` y no a `unknown`.
  all(): Promise<any[]>;
  update(i: number, x: unknown): Promise<void>;
  remove(i: number): Promise<void>;
}

const __stores: Record<string, __Store> = Object.create(null);

// Los tres backends de npm se cargan con `await import(...)` la primera vez que
// se consulta, así que un paquete ausente no se nota al arrancar: se nota en la
// primera petición, y al cliente le llega "error interno" —el endpoint no filtra
// detalles del servidor, y hace bien—. Diagnosticarlo cuesta entonces veinte
// minutos de mirar al sitio equivocado: el proxy, el puerto, la contraseña.
//
// Así que se comprueba al ARRANCAR, junto al resto de guardas del almacén, y se
// dice qué falta y cómo instalarlo.
const __PAQUETE_DE: Record<string, string> = {
  postgres: "pg",
  mysql: "mysql2",
  mongodb: "mongodb",
};

async function __exigirDriver(): Promise<void> {
  const cual = process.env.MAREA_DB ?? "file";
  const paquete = __PAQUETE_DE[cual];
  if (paquete === undefined) return; // 'file' y 'sqlite' no piden nada de npm
  try {
    await import(cual === "mysql" ? "mysql2/promise" : paquete);
  } catch {
    throw new Error(
      `[marea] MAREA_DB=${cual} necesita el paquete '${paquete}', que no está instalado donde corre este programa. Instálalo con: npm i ${paquete}`,
    );
  }
}

export function __store(__nombre: string, E: __Schema): __Store {
  // Idempotente: dos declaraciones del mismo nombre comparten instancia (el
  // bundle de servidor se importa una vez, pero la demo importa dos módulos).
  const previo = __stores[__nombre];
  if (previo !== undefined) return previo;

  // El driver de npm se comprueba aquí, que es donde `__store` corre: al cargar
  // el bundle de servidor, no en la primera petición. La comprobación es
  // asíncrona (un `import`), así que no puede cortar la construcción; lo que
  // hace es avisar por stderr en cuanto se sabe, y guardar el fallo para que
  // cualquier operación posterior lo repita en vez de dejar un "error interno".
  let __faltaDriver: string | null = null;
  const __driverListo = __exigirDriver().catch((e: unknown) => {
    __faltaDriver = e instanceof Error ? e.message : String(e);
    console.error(__faltaDriver);
  });
  async function __driverOk(): Promise<void> {
    await __driverListo;
    if (__faltaDriver !== null) throw new Error(__faltaDriver);
  }

  // Cuando el store no guarda registros (p.ej. `store Int;` o `store String;`) el
  // esquema tiene una sola columna '__doc' de tipo json: ahí cabe el valor entero.
  function __isDoc(): boolean {
    const cols = E.columns;
    return cols.length === 1 && cols[0].name === "__doc";
  }

  // --- ALMACÉN PRESTADO ---
  //
  // Lo que se comprueba AL ARRANCAR, no en la primera petición: son dos
  // condiciones que no dependen de los datos, y descubrirlas a mitad de un RPC
  // convierte un error de configuración en un 500 sin explicación.
  const __prestado = E.prestado === true;
  if (__prestado) {
    // Sin campos no hay columnas que leer. Un almacén propio escalar cabe en una
    // columna '__doc' porque Marea la crea; en una tabla ajena esa columna no
    // existe, y adivinar cuál de las suyas es "el valor" sería inventar.
    if (__isDoc()) {
      throw new Error(
        `[marea] el almacén prestado '${__nombre}' no guarda un registro: una tabla ajena se lee por columnas y un tipo sin campos no dice cuáles. Declara un tipo registro.`,
      );
    }
    // El backend de archivo es un log JSONL que escribe Marea: no hay ninguna
    // tabla ajena que leer ahí, así que 'file' no es un backend degradado para
    // este caso, es el caso imposible.
    const cual = process.env.MAREA_DB ?? "file";
    if (cual !== "sqlite" && cual !== "postgres" && cual !== "mysql" && cual !== "mongodb") {
      throw new Error(
        `[marea] el almacén prestado '${__nombre}' quiere leer la tabla '${E.table}', pero MAREA_DB=${cual}: el backend de archivo es un log que escribe Marea, no una base con tablas de otro. Usa MAREA_DB=sqlite|postgres|mysql|mongodb (con MAREA_DB_URL).`,
      );
    }
  }

  // La contrapartida honesta de haber perdido la garantía de esquema: prestar
  // una tabla hace IMPOSIBLE impedir que su dueño le quite una columna, pero no
  // impide detectarlo en la primera lectura y decir CUÁL falta. Sin esto, el
  // campo llegaría `undefined` al programa y el error saldría tres capas más
  // abajo, hablando de otra cosa.
  function __exigirColumnas(existentes: string[]): void {
    if (existentes.length === 0) {
      throw new Error(
        `[marea] el almacén prestado '${__nombre}' no encuentra la tabla '${E.table}' (o no tiene columnas). Marea no la crea: es de otro.`,
      );
    }
    const hay = new Set(existentes);
    const faltan = E.columns.map((c) => c.name).filter((n) => !hay.has(n));
    if (faltan.length === 0) return;
    const cuales =
      faltan.length === 1
        ? `la columna '${faltan[0]}'`
        : `las columnas ${faltan.map((n) => `'${n}'`).join(", ")}`;
    throw new Error(
      `[marea] el almacén prestado '${__nombre}' lee la tabla '${E.table}', que no tiene ${cuales}. La tabla tiene: ${existentes.join(", ")}.`,
    );
  }

  // --- conversión registro <-> fila (para los backends SQL) ---
  function __toRow(rec: any): unknown[] {
    if (__isDoc()) return [JSON.stringify(rec ?? null)];
    return E.columns.map((c) => {
      const v = rec?.[c.name];
      if (c.kind === "bool") return v ? 1 : 0;
      if (c.kind === "json") return JSON.stringify(v ?? null);
      return v ?? null;
    });
  }
  // Parseo de columnas JSON tolerante: una celda corrupta cae a null en vez de
  // tumbar la carga completa del store (aislamos la fila mala).
  function __parseCell(v: any): unknown {
    // Un driver puede devolver la celda YA parseada (jsonb en postgres, JSON en
    // mysql, un arreglo nativo en Mongo). Volver a parsear lo que ya es un valor
    // lanzaría, y el catch lo dejaría en null: dato perdido en silencio.
    if (v !== null && typeof v === "object") return v;
    try {
      return JSON.parse(v ?? "null");
    } catch {
      return null;
    }
  }
  function __fromRow(row: any): unknown {
    if (__isDoc()) return __parseCell(row?.__doc);
    const rec: any = {};
    for (const c of E.columns) {
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
    return E.columns.map((c) => __quoteId(c.name, q)).join(", ");
  }
  function __colList(q: string): string[] {
    return E.columns.map((c) => __quoteId(c.name, q));
  }
  function __table(q: string): string {
    return __quoteId(E.table, q);
  }
  // Columna interna de id (clave primaria). Convención '__id', como '__doc': el
  // lexer permite el nombre, pero un campo de usuario así llamado es improbable.
  function __idCol(q: string): string {
    return __quoteId("__id", q);
  }
  function __createSql(q: string): string {
    const s = E;
    const defs = [
      `${__idCol(q)} INTEGER PRIMARY KEY`,
      ...s.columns.map((c) => `${__quoteId(c.name, q)} ${__sqlType(c.kind)}`),
    ].join(", ");
    return `CREATE TABLE IF NOT EXISTS ${__table(q)} (${defs})`;
  }

  // Constructores de SQL incremental, parametrizados por dialecto (`q` = comilla de
  // identificador, `p(i)` = i-ésimo placeholder: '?' en sqlite/mysql, '$i' en pg).
  function __selectSql(q: string): string {
    // Prestado: la tabla ajena no tiene '__id', así que ni se pide ni se ordena
    // por él. El orden es el que dé la tabla; ordenar por una columna del
    // usuario sería elegir por él un criterio que no ha declarado.
    if (__prestado) return `SELECT ${__cols(q)} FROM ${__table(q)}`;
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
  function __rowFrom(row: any, i: number): __Row {
    // Prestado: no hay '__id' que leer. El id es posicional y no se usa jamás
    // para escribir (un prestado no escribe): sólo numera la copia en memoria.
    if (__prestado) return { id: i + 1, item: __fromRow(row) };
    return { id: Number(row.__id), item: __fromRow(row) };
  }

  // --- backend: archivo (por defecto, cero dependencias) ---
  //
  // Formato: LOG append-only JSONL, una línea por mutación {op,id,item}. Escribir
  // es O(1) (un append pequeño y atómico) en vez de reserializar el store entero
  // en cada cambio; al cargar se reproduce el log. Se compacta al cargar si creció
  // mucho respecto a las filas vivas, así no crece sin límite entre reinicios.
  function __fileBackend(): __Backend {
    const file = `${process.env.MAREA_STORE_DIR ?? "."}/marea-store.${__nombre}.log`;
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
      if (__prestado) {
        // Ni CREATE TABLE ni CREATE TABLE IF NOT EXISTS: la tabla es de otro y
        // Marea no manda en su esquema. Lo único que hace es comprobar que
        // tiene lo que el tipo dice.
        __exigirColumnas(
          (db.prepare(`PRAGMA table_info(${__table(q)})`).all() as any[]).map((c) =>
            String(c.name),
          ),
        );
      } else {
        db.exec(__createSql(q));
      }
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
      if (__prestado) {
        const r = await pool.query(
          "SELECT column_name FROM information_schema.columns WHERE table_name = $1",
          [E.table],
        );
        __exigirColumnas((r.rows as any[]).map((c) => String(c.column_name)));
      } else {
        await pool.query(__createSql(q));
      }
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
      if (__prestado) {
        const [cols] = await pool.query(
          "SELECT column_name FROM information_schema.columns WHERE table_schema = DATABASE() AND table_name = ?",
          [E.table],
        );
        __exigirColumnas(
          (cols as any[]).map((c) => String(c.column_name ?? c.COLUMN_NAME)),
        );
      } else {
        await pool.query(__createSql(q));
      }
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
      coll = client.db().collection(E.table);
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
    // Prestado: se proyectan EXACTAMENTE los campos del tipo. Devolver el resto
    // del documento metería en el valor campos que el programa no declaró (y un
    // `_id` que ni siquiera es un número cuando lo escribe otro).
    function __itemPrestado(d: any): unknown {
      const rec: any = {};
      for (const c of E.columns) rec[c.name] = d?.[c.name];
      return rec;
    }
    return {
      async load() {
        const c = await open();
        const docs = (await c.find({}).sort({ _id: 1 }).toArray()) as any[];
        if (__prestado) {
          // Mongo no declara esquema: lo que hay es lo que traen los documentos,
          // así que la comprobación se hace contra el primero. Una colección
          // vacía no tiene nada que leer ni nada que contradecir.
          if (docs.length > 0) __exigirColumnas(Object.keys(docs[0]));
          return docs.map((d, i) => ({ id: i + 1, item: __itemPrestado(d) }));
        }
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

    async function _guardar(x: unknown): Promise<void> {
    const s = await __ensureStore();
    const id = __nextId++;
    s.push(x);
    __ids.push(id);
    await (__backend as __Backend).insert(id, x);
  }
    async function _todos(): Promise<unknown[]> {
    return (await __ensureStore()).slice();
  }
  // Reemplaza el elemento en el índice 'i' (CRUD: update) — solo toca esa fila.
    async function _actualizar(i: number, x: unknown): Promise<void> {
    const s = await __ensureStore();
    if (i >= 0 && i < s.length) {
      s[i] = x;
      await (__backend as __Backend).update(__ids[i], x);
    }
  }
  // Elimina el elemento en el índice 'i' (CRUD: delete) — solo borra esa fila.
    async function _borrar(i: number): Promise<void> {
    const s = await __ensureStore();
    if (i >= 0 && i < s.length) {
      const id = __ids[i];
      s.splice(i, 1);
      __ids.splice(i, 1);
      await (__backend as __Backend).remove(id);
    }
  }


  // Un prestado no escribe. El verificador ya rechaza `save`/`update`/`remove`
  // sobre él al compilar, y el codegen ni siquiera importa esos builtins cuando
  // todos los almacenes del módulo son prestados. Esto es la última línea: si
  // alguien llega hasta aquí igual, se entera de por qué en vez de escribir en
  // la tabla de otro.
  function __soloLectura(op: string): never {
    throw new Error(
      `[marea] '${op}' sobre el almacén prestado '${__nombre}': la tabla '${E.table}' la mantiene otro programa y Marea sólo la lee.`,
    );
  }

  // Toda operación espera antes a saber si el driver está: así el fallo llega
  // con su nombre y su comando de instalación en vez de como "error interno".
  const conDriver =
    <A extends unknown[], R>(f: (...a: A) => Promise<R>) =>
    async (...a: A): Promise<R> => {
      await __driverOk();
      return f(...a);
    };

  const a: __Store = __prestado
    ? {
        nombre: __nombre,
        save: () => __soloLectura("save"),
        all: conDriver(_todos),
        update: () => __soloLectura("update"),
        remove: () => __soloLectura("remove"),
      }
    : {
        nombre: __nombre,
        save: conDriver(_guardar),
        all: conDriver(_todos),
        update: conDriver(_actualizar),
        remove: conDriver(_borrar),
      };
  __stores[__nombre] = a;
  return a;
}

// Los builtins del lenguaje toman el almacén como primer argumento.
export function save(a: __Store, x: unknown): Promise<void> {
  return a.save(x);
}
// Mismo criterio que en `__Store.all`: el tipo de los elementos lo garantiza el
// verificador de Marea, no TypeScript.
export function all(a: __Store): Promise<any[]> {
  return a.all();
}
export function update(a: __Store, i: number, x: unknown): Promise<void> {
  return a.update(i, x);
}
export function remove(a: __Store, i: number): Promise<void> {
  return a.remove(i);
}
