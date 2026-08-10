// Generado por Marea — arranca el servidor web y lo deja vivo.
import { startServer, puerto } from "./runtime.ts";
import "./server.ts";
// Sirve los estáticos (index.html, client.js) desde esta carpeta.
process.env.MAREA_WEB_ROOT ??= import.meta.dirname;
await startServer();
// El puerto se lee del runtime, que es quien lo resolvió y validó:
// recalcularlo aquí haría que el mensaje mintiera si el valor es basura.
console.log(`[marea] app web en http://127.0.0.1:${puerto()}`);
