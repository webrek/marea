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

// @marea:nucleo-inicio — el codegen sustituye este bloque por el contenido de
// `nucleo.js`, quitándole las anotaciones `/*ts ... */`: lo que llega al
// navegador es JavaScript plano, sin un `import` más que resolver ni servir.
// El mismo texto, con esas anotaciones destapadas, es el que se inserta en
// runtime.ts. Mientras se lee este archivo SIN generar, el import de abajo da lo
// mismo que la inserción, así que `tsc --checkJs` lo comprueba igual.
export * from "./nucleo.js";
import { __effect } from "./nucleo.js";
// @marea:nucleo-fin

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
