// Generado por Marea — cliente de navegador (no editar).
// Runtime de Marea para el NAVEGADOR (generado automáticamente — no editar).
//
// Es el gemelo de runtime.ts pero sin nada de Node (ni http ni fs): solo lo que
// corre en el navegador — el cliente RPC (fetch al mismo origen), el núcleo
// reactivo (signals de grano fino) y los builtins. Más `__mount`, que ata una
// vista reactiva al DOM: cuando cambia un signal que la vista leyó, el #app se
// vuelve a pintar solo. Esa es la frontera del TIEMPO tocando el DOM.
//
// Lo que aquí es propio del navegador son DOS funciones: `__rpc` (fetch al mismo
// origen, sin URL absoluta ni lista blanca porque no hay SSRF que valga desde el
// navegador) y `__mount`. Todo lo demás lo comparte con runtime.ts, y lo comparte
// de verdad: viene de `nucleo.js`, un solo texto para los dos.

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
//     export function html(s) { return s; }
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
export function __marea_is(value, tag) {
  return (
    value !== null &&
    typeof value === "object" &&
    (value).$tag === tag
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
        throw new Error(
          "ciclo reactivo detectado: un effect reescribe una reactiva que lee"
        );
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
export function __resource(f) {
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
    get() {
      if (dirty) recompute();
      if (__currentSub) subs.add(__currentSub);
      return value;
    },
    // Un memo es DERIVADO: asignarle no significa nada, y el verificador ya
    // rechaza `x = ...` sobre una `reactive` sin `mut` (E_ASSIGN_IMMUTABLE). Si
    // aun así se llega aquí —con `--no-check`, o desde JS a mano— se avisa en
    // vez de tragárselo: la versión muda de Node perdía la escritura en silencio
    // mientras la del navegador ya avisaba, y no puede haber dos respuestas.
    set(_v) {
      throw new Error("una derivada 'reactive' es de solo lectura");
    },
  };
}

// --- builtins del lenguaje ---

export function print(x) {
  console.log(x);
}

// `concat` y `len` sirven para texto Y para listas: son la misma idea sobre dos
// estructuras, y tener `unir`/`largo` aparte solo duplicaba la superficie.
export function concat(a, b) {
  if (Array.isArray(a) && Array.isArray(b)) return a.concat(b);
  return String(a) + String(b);
}

export function len(x) {
  if (Array.isArray(x)) return x.length;
  // Se cuentan puntos de código, no unidades UTF-16: "🌊" mide 1, no 2.
  return Array.from(String(x)).length;
}

export function text(x) {
  // Una variante se imprime por su nombre; si no, saldría "[object Object]".
  if (x !== null && typeof x === "object") {
    const t = (x).$tag;
    if (typeof t === "string") return t;
  }
  return String(x);
}

// 'render' pinta en el #app cuando hay DOM (el navegador) y registra en consola
// cuando no lo hay (Node). Una sola implementación: la rama que sobra en cada
// blanco no se toma, y así el mismo programa no tiene dos comportamientos según
// qué archivo se copió.
export function render(x) {
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



/// Los cierres vivos, por ID. Cada `on` mete uno; `__podar` saca los que ya no
/// nombra el marcado que está en pantalla.
const __manejadores = new Map();
/// Los tipos de evento que ya tienen su oyente en el documento. Uno por tipo,
/// puesto la primera vez que hace falta y nunca retirado.
const __enganchados = new Set();
let __idManejador = 0;

// Estos dos no burbujean, así que un manejador suyo solo puede ser el del
// elemento que recibió el evento: subir buscando uno haría saltar el del
// contenedor cuando el puntero pasa de un hijo a otro.
const __SIN_BURBUJEO = new Set(["blur", "pointerleave"]);

export function on(evento, manejador) {
  const id = "h" + ++__idManejador;
  __manejadores.set(id, { fn: manejador });
  __engancharRaiz(evento);
  // Un atributo por tipo de evento: dos `on` distintos en la misma etiqueta
  // serían dos atributos con el mismo nombre y el analizador de HTML se quedaría
  // con el primero, callándose el segundo.
  return `data-marea-on-${evento}="${id}"`;
}

function __engancharRaiz(evento) {
  if (typeof document === "undefined" || __enganchados.has(evento)) return;
  __enganchados.add(evento);
  document.addEventListener(evento, (ev) => __despachar(evento, ev), true);
}

/// Busca el manejador desde el objetivo hacia arriba y ejecuta el PRIMERO que
/// encuentra. Uno solo: que un clic dispare además los manejadores de todos los
/// contenedores de encima es la clase de sorpresa que se paga depurando.
function __despachar(evento, ev) {
  const atributo = "data-marea-on-" + evento;
  let nodo = (ev.target);
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
    nodo = (nodo.parentNode);
  }
}

/// Tira los cierres que el marcado entrante ya no nombra.
///
/// La vida de un manejador es la del elemento que lo lleva, y el elemento vive
/// mientras esté en el marcado que se escribe en el `#app`: ese texto es la
/// lista de los vivos, así que no hace falta llevar la cuenta de re-pintados ni
/// adivinar cuándo caduca uno. Sin esto la tabla crecería un cierre por botón y
/// por re-pintado —un contador pulsado mil veces dejaría mil cierres muertos.
export function __podar(html) {
  const vivos = new Set();
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
export function __div(a, b) {
  if (b === 0) {
    throw new __BoundaryError("división entre cero");
  }
  return Math.trunc(a / b);
}

export function __rem(a, b) {
  if (b === 0) {
    throw new __BoundaryError("módulo entre cero");
  }
  return a % b;
}

// Escapa un texto para incrustarlo en HTML. El lenguaje construye marcado
// concatenando cadenas y 'render' lo inyecta por innerHTML, así que sin esto un
// dato persistido vía RPC se ejecuta como marcado en todos los clientes. El `&`
// va PRIMERO: después, el `&` de `&lt;` se volvería a escapar.
export function escape(x) {
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

export function append(xs, x) {
  return xs.concat([x]);
}

// Indexado de lista con verificación de rango: un índice fuera de rango lanza
// un error claro en vez de devolver 'undefined' (que reventaría más tarde).
export function __index(xs, i) {
  if (i < 0 || i >= xs.length) {
    // Si el índice vino de la red es una petición mal formada (400), no un
    // fallo del servidor: si no, el cliente induce 500 y ruido de log a placer.
    throw new __BoundaryError(`índice fuera de rango: ${i} (longitud ${xs.length})`);
  }
  return xs[i];
}

// --- texto ---

export function contains(s, sub) {
  return String(s).includes(String(sub));
}

export function lower(s) {
  return String(s).toLowerCase();
}

// --- el puente reactividad ↔ DOM ---
// Envuelve la vista en un effect: cada vez que cambia un signal que la vista
// leyó, se vuelve a pintar el #app. La vista lee sus reactivas en su parte
// SÍNCRONA (antes de cualquier await), así que la suscripción queda registrada.
//
// El re-pintado tira los nodos viejos, y con ellos los manejadores que llevaban
// puestos: `__podar` los saca de la tabla mirando qué IDs nombra el marcado
// nuevo. Los OYENTES no se tocan —cuelgan del documento, no de los nodos—, que
// es lo que hace que re-pintar no deje ninguno huérfano.
export function __mount(vista) {
  const app = typeof document !== "undefined" && document.getElementById("app");
  if (!app) return;
  __effect(async () => {
    const marcado = String(await vista());
    __podar(marcado);
    app.innerHTML = marcado;
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
  return concat(concat(concat("<li class=\"post\"><span class=\"autor\">@", escape(p.autor)), concat("</span> ", escape(p.texto))), concat(concat(concat(" <button class=\"like\" onclick=\"marea.darLike(", text(i)), concat(")\">♥ ", text(p.likes))), "</button></li>"));
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
