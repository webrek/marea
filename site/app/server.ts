// Generado por Marea — lado servidor.
import { __register, __malFormado, __rpc, print, concat, render, len, aTexto, escapar, html, unir, agregar, largo, contiene, minusculas, __index, guardar, todos, actualizar, borrar, __marea_is, __signal, __memo, __effect } from "./runtime.ts";

// fn fila(p: Post, i: Int) -> Html
async function fila(p: Post, i: number) {
  return concat(concat(concat("<li class=\"post\"><span class=\"autor\">@", escapar(p.autor)), concat("</span> ", escapar(p.texto))), concat(concat(concat(" <button class=\"like\" onclick=\"marea.darLike(", aTexto(i)), concat(")\">♥ ", aTexto(p.likes))), "</button></li>"));
}

// fn filas(ps: List<Post>, i: Int) -> Html
async function filas(ps: Post[], i: number) {
  if ((i < len(ps))) {
    return concat((await fila(__index(ps, i), i)), (await filas(ps, (i + 1))));
  }
  return "";
}

// @server fn publicar(autor: String, texto: String)
async function publicar(autor: string, texto: string) {
  (await guardar({ autor: autor, texto: texto, likes: 0 }));
}

__register("publicar", (__args) => { if (__args.length !== 2) __malFormado("aridad"); if (!(typeof __args[0] === "string")) __malFormado("argumento 1"); if (!(typeof __args[1] === "string")) __malFormado("argumento 2"); return publicar(__args[0], __args[1]); });

// @server fn like(i: Int)
async function like(i: number) {
  const p = __index((await todos()), i);
  (await actualizar(i, { autor: p.autor, texto: p.texto, likes: (p.likes + 1) }));
}

__register("like", (__args) => { if (__args.length !== 1) __malFormado("aridad"); if (!(Number.isSafeInteger(__args[0]))) __malFormado("argumento 1"); return like(__args[0]); });

// @server fn feed() -> List<Post>
async function feed() {
  return (await todos());
}

__register("feed", (__args) => { if (__args.length !== 0) __malFormado("aridad"); return feed(); });

