// Los tres drivers de base de datos son PEERS OPCIONALES: el runtime los carga
// con `await import(...)` sólo si eliges ese backend, y un programa sin `store`
// ni siquiera los lleva (el codegen recorta esa sección). Instalarlos aquí para
// comprobar tipos sería instalarlos para nada; se declaran como opacos.
//
// Ojo: que a `tsc` le faltaran estos tres módulos era, literalmente, el mismo
// aviso que rompió el build de Next de un consumidor. Ahí lo señalaba de verdad;
// aquí es esperado. Si aparece un CUARTO nombre en esta lista, esa es la señal
// de que se está enviando una dependencia nueva a todo el mundo.
declare module "pg";
declare module "mysql2/promise";
declare module "mongodb";
