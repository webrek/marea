// Generado por Marea — lado servidor.
import { __register, __rpc, print, concat, render, len, aTexto, __index, guardar, todos, actualizar, borrar, __marea_is, __signal, __memo, __effect } from "./runtime.ts";

// @server fn publicar(autor: String, texto: String)
async function publicar(autor: string, texto: string) {
  (await guardar({ autor: autor, texto: texto, likes: 0 }));
}

__register("publicar", (args) => { if (args.length !== 2) throw new Error("aridad inválida"); return publicar(args[0], args[1]); });

// @server fn like(i: Int)
async function like(i: number) {
  const p = __index((await todos()), i);
  (await actualizar(i, { autor: p.autor, texto: p.texto, likes: (p.likes + 1) }));
}

__register("like", (args) => { if (args.length !== 1) throw new Error("aridad inválida"); return like(args[0]); });

// @server fn feed() -> List<Post>
async function feed() {
  return (await todos());
}

__register("feed", (args) => { if (args.length !== 0) throw new Error("aridad inválida"); return feed(); });

