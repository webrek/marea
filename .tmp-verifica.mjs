// Temporal: replica en JS la composición que hace el codegen en Rust, para
// comprobar que el núcleo compartido es JavaScript válido y se comporta igual.
import fs from "node:fs";

const src = "crates/marea-codegen/src";
const nucleo = fs.readFileSync(`${src}/nucleo.js`, "utf8");

function recorrer(js, conTipo) {
  let out = "";
  let resto = js;
  let i;
  while ((i = resto.indexOf("/*ts")) !== -1) {
    out += resto.slice(0, i);
    const tras = resto.slice(i + 4);
    const j = tras.indexOf("*/");
    if (j === -1) throw new Error("marca /*ts sin cerrar");
    out += conTipo(tras.slice(0, j));
    resto = tras.slice(j + 2);
  }
  return out + resto;
}
const destapado = recorrer(nucleo, (t) => t);
const sinTipos = recorrer(nucleo, () => "");

function conNucleo(plantilla, nuc) {
  const out = [];
  let dentro = false;
  let puesto = false;
  for (const linea of plantilla.split("\n")) {
    if (dentro) {
      if (linea.startsWith("// @marea:nucleo-fin")) dentro = false;
      continue;
    }
    if (linea.startsWith("// @marea:nucleo-inicio")) {
      dentro = true;
      puesto = true;
      out.push(nuc.replace(/\n+$/, ""));
      continue;
    }
    out.push(linea);
  }
  if (!puesto) throw new Error("no se encontró el marcador del núcleo");
  return out.join("\n");
}

const runtimeTs = conNucleo(fs.readFileSync(`${src}/runtime.ts`, "utf8"), destapado);
const browserJs = conNucleo(fs.readFileSync(`${src}/browser.js`, "utf8"), sinTipos);

fs.mkdirSync(".tmp-verifica", { recursive: true });
fs.writeFileSync(".tmp-verifica/runtime.ts", runtimeTs);
fs.writeFileSync(".tmp-verifica/browser.mjs", browserJs);
fs.writeFileSync(".tmp-verifica/nucleo.mjs", sinTipos);

console.log("runtime.ts compuesto:", runtimeTs.split("\n").length, "líneas");
console.log("browser.js compuesto:", browserJs.split("\n").length, "líneas");
console.log("marcas /*ts en runtime.ts:", runtimeTs.includes("/*ts"));
console.log("tipos en browser.js:", [": unknown", ": string", "/*ts"].filter((a) => browserJs.includes(a)));

for (const archivo of ["./.tmp-verifica/browser.mjs", "./.tmp-verifica/runtime.ts"]) {
  const m = await import(archivo);
  const nombre = archivo.includes("browser") ? "navegador" : "node";
  console.log(`--- ${nombre} ---`);
  console.log("text:", [0, 7, 1435, -250, -0].map((n) => m.text(n)).join("|"));
  console.log("div:", [[7, 2], [-7, 2], [7, -2], [-1, 2]].map(([a, b]) => m.__div(a, b)).join("|"));
  console.log("escape:", m.escape(`&<>"'`));
  console.log("len:", m.len("🌊ab"), m.len([1, 2, 3]));
  console.log("concat:", m.concat("a", "b"), JSON.stringify(m.concat([1], [2])));
  const n = m.__signal(1);
  const doble = m.__memo(() => n.get() * 2);
  const visto = [];
  m.__effect(() => visto.push(doble.get()));
  n.set(5);
  n.set(5);
  n.set(7);
  console.log("reactivo:", visto.join("|"));
  try { doble.set(0); console.log("memo.set: NO avisó"); } catch { console.log("memo.set: avisó"); }
  try { m.__index([1, 2], 5); console.log("index: no cortó"); } catch (e) { console.log("index cortó:", e.constructor.name); }
  m.render("<b>hola</b>");
}
