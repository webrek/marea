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
//     export function html(s/*ts: string*/)/*ts: string*/ { return s; }
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
export function __marea_is(value/*ts: unknown*/, tag/*ts: string*/)/*ts: boolean*/ {
  return (
    value !== null &&
    typeof value === "object" &&
    (value/*ts as Record<string, unknown>*/).$tag === tag
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

/*ts
interface Reaction {
  invalidate(): void;
  // Los conjuntos de suscriptores en los que esta reacción está metida. Sin
  // esto no se puede cancelar: la reacción no sabría de dónde quitarse.
  fuentes: Set<Set<Reaction>>;
}

interface EffectReaction extends Reaction {
  execute(): void;
}

export interface Cell<T> {
  get(): T;
  set(v: T): void;
}
*/

let __currentSub/*ts: Reaction | null*/ = null;
const __pending/*ts: Set<EffectReaction>*/ = new Set();
let __flushing = false;

function __flush()/*ts: void*/ {
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
      const r = __pending.values().next().value/*ts as EffectReaction*/;
      __pending.delete(r);
      r.execute();
    }
  } finally {
    __flushing = false;
  }
}

export function __signal/*ts<T>*/(initial/*ts: T*/)/*ts: Cell<T>*/ {
  let value = initial;
  const subs/*ts: Set<Reaction>*/ = new Set();
  return {
    get()/*ts: T*/ {
      if (__currentSub) {
        subs.add(__currentSub);
        // La relación se guarda en los DOS sentidos: la fuente sabe quién la
        // lee, y el lector sabe de qué fuentes cuelga. Lo segundo es lo que
        // permite desmontarse después.
        __currentSub.fuentes.add(subs);
      }
      return value;
    },
    set(v/*ts: T*/)/*ts: void*/ {
      if (v === value) return;
      value = v;
      for (const r of [...subs]) r.invalidate();
      __flush();
    },
  };
}

/// Registra un efecto y devuelve cómo DETENERLO.
///
/// Cancelar hacía falta en cuanto Marea dejó de ser siempre la app entera: un
/// componente que el anfitrión (React, por ejemplo) desmonta y vuelve a montar
/// dejaba efectos vivos pintando sobre un elemento que ya no está en el
/// documento, y cada re-montaje añadía otro.
export function __effect(fn/*ts: () => void | Promise<void>*/)/*ts: () => void*/ {
  const reaction/*ts: EffectReaction*/ = {
    fuentes: new Set(),
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
  return () => {
    // Salirse de cada fuente que lo tenía apuntado, y de la cola por si estaba
    // pendiente de ejecutarse cuando lo detuvieron.
    for (const subs of reaction.fuentes) subs.delete(reaction);
    reaction.fuentes.clear();
    __pending.delete(reaction);
  };
}

// Un RECURSO: la composición de las dos fronteras. Arranca en `Cargando`, lanza
// la llamada asíncrona y se pone al resultado cuando llega, o a `Fallo` si
// revienta. Como es un signal, cualquier vista que lo lea se re-pinta sola en
// cada transición: no hay que orquestar nada a mano.
export function __resource(f/*ts: () => Promise<unknown>*/)/*ts: Cell<unknown>*/ {
  const s = __signal/*ts<unknown>*/({ $tag: "Cargando" });
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

export function __memo/*ts<T>*/(fn/*ts: () => T*/)/*ts: Cell<T>*/ {
  const subs/*ts: Set<Reaction>*/ = new Set();
  let value/*ts: T*/;
  let dirty = true;
  const reaction/*ts: Reaction*/ = {
    fuentes: new Set(),
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
    get()/*ts: T*/ {
      if (dirty) recompute();
      if (__currentSub) {
        subs.add(__currentSub);
        __currentSub.fuentes.add(subs);
      }
      return value;
    },
    // Un memo es DERIVADO: asignarle no significa nada, y el verificador ya
    // rechaza `x = ...` sobre una `reactive` sin `mut` (E_ASSIGN_IMMUTABLE). Si
    // aun así se llega aquí —con `--no-check`, o desde JS a mano— se avisa en
    // vez de tragárselo: la versión muda de Node perdía la escritura en silencio
    // mientras la del navegador ya avisaba, y no puede haber dos respuestas.
    set(_v/*ts: T*/)/*ts: void*/ {
      throw new Error("una derivada 'reactive' es de solo lectura");
    },
  };
}

// --- builtins del lenguaje ---

export function print(x/*ts: unknown*/)/*ts: void*/ {
  console.log(x);
}

// `concat` y `len` sirven para texto Y para listas: son la misma idea sobre dos
// estructuras, y tener `unir`/`largo` aparte solo duplicaba la superficie.
export function concat(a/*ts: unknown*/, b/*ts: unknown*/)/*ts: unknown*/ {
  if (Array.isArray(a) && Array.isArray(b)) return a.concat(b);
  return String(a) + String(b);
}

export function len(x/*ts: unknown*/)/*ts: number*/ {
  if (Array.isArray(x)) return x.length;
  // Se cuentan puntos de código, no unidades UTF-16: "🌊" mide 1, no 2.
  return Array.from(String(x)).length;
}

export function text(x/*ts: unknown*/)/*ts: string*/ {
  // Una variante se imprime por su nombre; si no, saldría "[object Object]".
  if (x !== null && typeof x === "object") {
    const t = (x/*ts as Record<string, unknown>*/).$tag;
    if (typeof t === "string") return t;
  }
  return String(x);
}

// 'render' pinta en el #app cuando hay DOM (el navegador) y registra en consola
// cuando no lo hay (Node). Una sola implementación: la rama que sobra en cada
// blanco no se toma, y así el mismo programa no tiene dos comportamientos según
// qué archivo se copió.
export function render(x/*ts: unknown*/)/*ts: void*/ {
  const app = typeof document !== "undefined" && document.getElementById("app");
  if (app) {
    const marcado = String(x);
    // Podar ANTES de escribir: el marcado que entra es el que dice qué
    // manejadores siguen vivos (ver `__podar`).
    __podar(marcado, __sinIsla);
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

/*ts
interface __Manejador {
  fn: () => unknown;
}
*/

/// Los cierres vivos, por ID. Cada `on` mete uno; `__podar` saca los que ya no
/// nombra el marcado que está en pantalla.
const __manejadores/*ts: Map<string, __Manejador>*/ = new Map();
/// Los tipos de evento que ya tienen su oyente en el documento. Uno por tipo,
/// puesto la primera vez que hace falta y nunca retirado.
const __enganchados/*ts: Set<string>*/ = new Set();
let __idManejador = 0;

// Estos dos no burbujean, así que un manejador suyo solo puede ser el del
// elemento que recibió el evento: subir buscando uno haría saltar el del
// contenedor cuando el puntero pasa de un hijo a otro.
const __SIN_BURBUJEO/*ts: Set<string>*/ = new Set(["blur", "pointerleave"]);

// La isla que está pintando ahora mismo, si hay alguna. Mismo mecanismo que
// `__currentSub` en la reactividad: durante la evaluación de una vista, todo lo
// que se registre queda atribuido a quien la está pintando.
let __islaActual/*ts: Set<string> | null*/ = null;

// Los manejadores que se registran sin isla en curso: los de `render`.
const __sinIsla/*ts: Set<string>*/ = new Set();

/// Ejecuta `f` atribuyendo a `isla` los manejadores que registre.
export function __pintandoEn/*ts<T>*/(isla/*ts: Set<string>*/, f/*ts: () => T*/)/*ts: T*/ {
  const prev = __islaActual;
  __islaActual = isla;
  try {
    return f();
  } finally {
    __islaActual = prev;
  }
}

export function on(evento/*ts: string*/, manejador/*ts: () => unknown*/)/*ts: string*/ {
  const id = "h" + ++__idManejador;
  __manejadores.set(id, { fn: manejador });
  // Sin isla en curso, el manejador es del ámbito por defecto: el de `render`,
  // que pinta la app entera en `#app`. Así cada ámbito poda lo suyo y ninguno
  // se lleva por delante lo del otro.
  (__islaActual ?? __sinIsla).add(id);
  __engancharRaiz(evento);
  // Un atributo por tipo de evento: dos `on` distintos en la misma etiqueta
  // serían dos atributos con el mismo nombre y el analizador de HTML se quedaría
  // con el primero, callándose el segundo.
  return `data-marea-on-${evento}="${id}"`;
}

function __engancharRaiz(evento/*ts: string*/)/*ts: void*/ {
  if (typeof document === "undefined" || __enganchados.has(evento)) return;
  __enganchados.add(evento);
  document.addEventListener(evento, (ev) => __despachar(evento, ev), true);
}

/// Busca el manejador desde el objetivo hacia arriba y ejecuta el PRIMERO que
/// encuentra. Uno solo: que un clic dispare además los manejadores de todos los
/// contenedores de encima es la clase de sorpresa que se paga depurando.
function __despachar(evento/*ts: string*/, ev/*ts: Event*/)/*ts: void*/ {
  const atributo = "data-marea-on-" + evento;
  let nodo = (ev.target/*ts as Element | null*/);
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
    nodo = (nodo.parentNode/*ts as Element | null*/);
  }
}

/// Tira los cierres que el marcado entrante ya no nombra.
///
/// La vida de un manejador es la del elemento que lo lleva, y el elemento vive
/// mientras esté en el marcado que se escribe en el `#app`: ese texto es la
/// lista de los vivos, así que no hace falta llevar la cuenta de re-pintados ni
/// adivinar cuándo caduca uno. Sin esto la tabla crecería un cierre por botón y
/// por re-pintado —un contador pulsado mil veces dejaría mil cierres muertos.
/// Saca de la tabla los manejadores que el marcado nuevo ya no nombra.
///
/// `mios` acota la poda a UNA isla. Cuando Marea era siempre la app entera esto
/// no hacía falta y se podaba contra la tabla completa; con varias islas en la
/// misma página, esa versión borraba los manejadores de las demás —y en
/// silencio, porque los oyentes cuelgan del documento: el clic llegaba y no
/// encontraba a nadie, así que el botón dejaba de responder sin ningún error—.
export function __podar(html/*ts: string*/, mios/*ts: Set<string>*/)/*ts: void*/ {
  const vivos/*ts: Set<string>*/ = new Set();
  for (const m of html.matchAll(/data-marea-on-[a-z]+="([^"]*)"/g)) vivos.add(m[1]);
  for (const id of [...mios]) {
    if (vivos.has(id)) continue;
    __manejadores.delete(id);
    mios.delete(id);
  }
}

/// Suelta todos los manejadores de una isla. Se usa al desmontarla.
export function __soltar(mios/*ts: Set<string>*/)/*ts: void*/ {
  for (const id of mios) __manejadores.delete(id);
  mios.clear();
}

// División entera. En JS `7/0` es Infinity y `0/0` es NaN: valores que no son
// enteros y que se colarían dentro de un Int mintiendo sobre su tipo. El backend
// WASM trapea, así que aquí también se corta —el mismo programa no puede dar
// Infinity en un blanco y morir en el otro—. Trunca HACIA CERO, igual que
// `i32.div_s`: `-7/2` es -3, no -4.
export function __div(a/*ts: number*/, b/*ts: number*/)/*ts: number*/ {
  if (b === 0) {
    throw new __BoundaryError("división entre cero");
  }
  return Math.trunc(a / b);
}

export function __rem(a/*ts: number*/, b/*ts: number*/)/*ts: number*/ {
  if (b === 0) {
    throw new __BoundaryError("módulo entre cero");
  }
  return a % b;
}

// Escapa un texto para incrustarlo en HTML. El lenguaje construye marcado
// concatenando cadenas y 'render' lo inyecta por innerHTML, así que sin esto un
// dato persistido vía RPC se ejecuta como marcado en todos los clientes. El `&`
// va PRIMERO: después, el `&` de `&lt;` se volvería a escapar.
export function escape(x/*ts: unknown*/)/*ts: string*/ {
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
export function html(s/*ts: string*/)/*ts: string*/ {
  return s;
}

// --- listas: construir en runtime (el lenguaje no tiene bucles ni cierres, así
// que sin esto una función no puede devolver un subconjunto filtrado) ---

export function append(xs/*ts: unknown[]*/, x/*ts: unknown*/)/*ts: unknown[]*/ {
  return xs.concat([x]);
}

// Indexado de lista con verificación de rango: un índice fuera de rango lanza
// un error claro en vez de devolver 'undefined' (que reventaría más tarde).
export function __index/*ts<T>*/(xs/*ts: T[]*/, i/*ts: number*/)/*ts: T*/ {
  if (i < 0 || i >= xs.length) {
    // Si el índice vino de la red es una petición mal formada (400), no un
    // fallo del servidor: si no, el cliente induce 500 y ruido de log a placer.
    throw new __BoundaryError(`índice fuera de rango: ${i} (longitud ${xs.length})`);
  }
  return xs[i];
}

// --- texto ---

export function contains(s/*ts: string*/, sub/*ts: string*/)/*ts: boolean*/ {
  return String(s).includes(String(sub));
}

// Convertir texto a número PUEDE fallar, así que el tipo lo dice
// (`Int | NoEsNumero`) y el `match` obliga a decidir el valor por defecto en el
// fuente. Es lo que hacía falta para leer una query string —`entero(consulta(
// "pagina"))`—, pero el hueco que tapa es más viejo: hasta ahora el lenguaje no
// tenía NINGUNA forma de convertir texto a número.
//
// Estricto como la conversión de un segmento de ruta, y por la misma razón:
// `Number(" 7 ")` es 7 y `Number("")` es 0, de modo que delegar en `Number`
// convertiría en enteros dos cadenas que nadie escribió como números.
export function entero(s/*ts: string*/)/*ts: number | { $tag: string }*/ {
  const t = String(s);
  if (!/^-?\d+$/.test(t)) return { $tag: "NoEsNumero" };
  const n = Number(t);
  if (!Number.isSafeInteger(n)) return { $tag: "NoEsNumero" };
  return n;
}

export function lower(s/*ts: string*/)/*ts: string*/ {
  return String(s).toLowerCase();
}
