// Generado por Marea — arranca el servidor web y lo deja vivo.
import { startServer } from "./runtime.ts";
import "./server.ts";
// Sirve los estáticos (index.html, client.js) desde esta carpeta.
process.env.MAREA_WEB_ROOT ??= import.meta.dirname;
await startServer();
// Mismo orden de precedencia que runtime.ts: PORT (hosting) > MAREA_PORT > 8787.
const __port = Number(process.env.PORT ?? process.env.MAREA_PORT ?? 8787);
console.log(`[marea] app web en http://127.0.0.1:${__port}`);
