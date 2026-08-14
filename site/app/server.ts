// Generado por Marea — lado servidor.
import { __badRequest, print, concat, render, len, text, escape, html, on, __div, __rem, append, contains, lower, jsonText, jsonInt, jsonFloat, jsonLen, __index, __marea_is, __signal, __memo, __resource, __effect, parseInt, __register, __rpc, fetch, post, __store, save, all, update, remove } from "./runtime.ts";

export type Post = { autor: string; texto: string; likes: number };

const publicaciones = __store("publicaciones", { table: "publicaciones", columns: [{ name: "autor", kind: "text" }, { name: "texto", kind: "text" }, { name: "likes", kind: "int" }] });

// fn fila(p: Post, i: Int) -> Html
async function fila(p: Post, i: number): Promise<string> {
  return concat(concat(concat("<li class=\"post\"><span class=\"autor\">@", escape(p.autor)), concat("</span> ", escape(p.texto))), concat(concat(concat(" <button class=\"like\" onclick=\"marea.darLike(", text(i)), concat(")\">♥ ", text(p.likes))), "</button></li>"));
}

// fn filas(ps: List<Post>, i: Int) -> Html
async function filas(ps: Post[], i: number): Promise<string> {
  if ((i < len(ps))) {
    return concat((await fila(__index(ps, i), i)), (await filas(ps, (i + 1))));
  }
  return "";
}

// @server fn publicar(autor: String, texto: String)
async function publicar(autor: string, texto: string): Promise<void> {
  (await save(publicaciones, { autor: autor, texto: texto, likes: 0 }));
}

__register("publicar", (__args) => { if (__args.length !== 2) __badRequest("aridad"); if (!(typeof __args[0] === "string")) __badRequest("argumento 1"); if (!(typeof __args[1] === "string")) __badRequest("argumento 2"); return publicar(__args[0], __args[1]); });

// @server fn like(i: Int)
async function like(i: number): Promise<void> {
  const p = __index((await all(publicaciones)), i);
  (await update(publicaciones, i, { autor: p.autor, texto: p.texto, likes: (p.likes + 1) }));
}

__register("like", (__args) => { if (__args.length !== 1) __badRequest("aridad"); if (!(Number.isSafeInteger(__args[0]))) __badRequest("argumento 1"); return like(__args[0]); });

// @server fn feed() -> List<Post>
async function feed(): Promise<Post[]> {
  return (await all(publicaciones));
}

__register("feed", (__args) => { if (__args.length !== 0) __badRequest("aridad"); return feed(); });

