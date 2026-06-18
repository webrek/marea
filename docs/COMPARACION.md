# Marea frente a otros lenguajes — la misma app, lado a lado

Marea sostiene una tesis concreta: las dos fronteras que toda app web cruza a
mano —la de **red** (cliente↔servidor) y la del **tiempo** (cuándo se recalcula
algo)— deberían ser **primitivas del lenguaje**, no andamiaje que montas con
librerías y archivos de configuración.

Este documento pone esa tesis a prueba con un experimento simple: **la misma app,
escrita en cinco stacks**. Misma lógica, contada honestamente.

---

## Qué se compara (y qué NO)

**Sí se compara:** cuántas piezas móviles —archivos, librerías, líneas de
"pegamento"— necesita cada stack para expresar (a) una operación que vive en el
servidor y se invoca desde el cliente, y (b) una vista que se actualiza sola
cuando cambia el estado. Es una comparación de **diseño de lenguaje**.

**NO se compara madurez ni rendimiento.** Marea es un lenguaje **v0 de
investigación**; React/tRPC, Phoenix, Leptos y Livewire son ecosistemas de
**producción** con años de batalla, validación, migraciones, auth y comunidades
enormes. En todo lo que importa para enviar a producción hoy, los otros cuatro
ganan. Aquí solo miramos **dónde vive cada frontera**: en el lenguaje, en un
framework, o en tu propio pegamento.

**Honestidad sobre la columna de Marea:** el ejemplo real (`examples/x-likes.mar`)
corre las dos fronteras **de verdad** —el RPC cruza por HTTP, la reactividad son
signals reales en el runtime— pero las renderiza a **consola**, no a un DOM
reactivo en el navegador. Marea aún no tiene una demo web que cablee ambas
fronteras en una página (su backend WASM/DOM es incipiente). Los otros cuatro son
apps de navegador interactivas. Lo comparable es **cómo el lenguaje expresa la
lógica servidor↔cliente + reactividad**, no el pulido de la UI.

> Los fragmentos de los stacks no-Marea son implementaciones **mínimas
> idiomáticas** escritas a mano; no se ejecutaron en esta máquina. El conteo de
> líneas es de **código de app**, y excluye manifiestos (`package.json`,
> `Cargo.toml`, `mix.exs`, `composer.json`) y configuración de build. Marea no
> excluye ninguno porque **no tiene**: la app es un solo archivo `.mar`.

---

## La app: "X-mini" (un timeline con likes)

La misma especificación para todos:

- **Dato:** `Post { autor, texto, likes }`.
- **Servidor:** `publicar(autor, texto)`, `like(i)`, `feed()`. El estado
  **persiste** (sobrevive reinicios).
- **Cliente:** invoca esas operaciones y muestra el timeline; dar like sube el
  contador.

---

## 1. Marea — un archivo, dos fronteras como primitivas

```marea
type Post = { autor: String, texto: String, likes: Int };

store Post;                                   // ← persistencia: una línea

@server
fn publicar(autor: String, texto: String) {
    guardar(Post { autor: autor, texto: texto, likes: 0 });
}

@server
fn like(i: Int) {
    let p = todos()[i];
    actualizar(i, Post { autor: p.autor, texto: p.texto, likes: p.likes + 1 });
}

@server
fn feed() -> List<Post> { return todos(); }

@client
fn main() {
    publicar("ada", "Primer post en Marea");
    like(0);
    mostrarDesde(feed(), 0);                  // feed() cruza la red sola
}
```

- **Frontera de red:** `@server`/`@client`. El compilador genera el handler, el
  stub RPC y la serialización. El `@client` llama `feed()` como si fuera local;
  **no escribes router, ni cliente, ni transporte, ni esquema de API.**
- **Frontera del tiempo:** `reactive`/`effect` (ver `examples/contador.mar`). Un
  `reactive mut n = 0` es una fuente; `reactive doble = n * 2` se recomputa solo;
  `effect { ... }` se re-ejecuta cuando cambia lo que leyó. **Sin librería.**
- **Persistencia:** `store Post;`. El backend (archivo/SQLite/Postgres/MySQL/
  Mongo) se elige con una variable de entorno, sin tocar el `.mar`.

**1 archivo · 42 líneas de código (`examples/x-likes.mar` completo, incluye los
helpers `linea`/`mostrarDesde` que el fragmento de arriba abrevia) · 0
dependencias · 0 configuración.**

---

## 2. TypeScript + React + tRPC + Prisma

El baseline "full-stack type-safe" moderno. Tipado de punta a punta, pero las dos
fronteras son **librerías que tú cableas**.

```prisma
// prisma/schema.prisma
model Post {
  id     Int    @id @default(autoincrement())
  autor  String
  texto  String
  likes  Int    @default(0)
}
```

```ts
// src/server/router.ts
import { initTRPC } from "@trpc/server";
import { z } from "zod";
import { PrismaClient } from "@prisma/client";
const prisma = new PrismaClient();
const t = initTRPC.create();
export const appRouter = t.router({
  publicar: t.procedure
    .input(z.object({ autor: z.string(), texto: z.string() }))
    .mutation(({ input }) => prisma.post.create({ data: { ...input } })),
  like: t.procedure
    .input(z.object({ id: z.number() }))
    .mutation(({ input }) =>
      prisma.post.update({ where: { id: input.id }, data: { likes: { increment: 1 } } })),
  feed: t.procedure.query(() => prisma.post.findMany({ orderBy: { id: "asc" } })),
});
export type AppRouter = typeof appRouter;
```

```ts
// src/server/index.ts
import { createHTTPServer } from "@trpc/server/adapters/standalone";
import { appRouter } from "./router";
createHTTPServer({ router: appRouter }).listen(3000);
```

```ts
// src/client/trpc.ts
import { createTRPCReact } from "@trpc/react-query";
import type { AppRouter } from "../server/router";
export const trpc = createTRPCReact<AppRouter>();
```

```tsx
// src/client/App.tsx
import { trpc } from "./trpc";
export function App() {
  const feed = trpc.feed.useQuery();
  const util = trpc.useUtils();
  const like = trpc.like.useMutation({ onSuccess: () => util.feed.invalidate() });
  const pub = trpc.publicar.useMutation({ onSuccess: () => util.feed.invalidate() });
  return (
    <ul>
      {feed.data?.map((p) => (
        <li key={p.id}>
          {p.autor}: {p.texto} ({p.likes})
          <button onClick={() => like.mutate({ id: p.id })}>like</button>
        </li>
      ))}
    </ul>
  );
}
```

```tsx
// src/client/main.tsx  (+ QueryClientProvider + trpc.Provider + httpBatchLink wiring)
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { httpBatchLink } from "@trpc/client";
import { trpc } from "./trpc";
import { App } from "./App";
const qc = new QueryClient();
const tc = trpc.createClient({ links: [httpBatchLink({ url: "http://localhost:3000" })] });
createRoot(document.getElementById("root")!).render(
  <trpc.Provider client={tc} queryClient={qc}>
    <QueryClientProvider client={qc}><App /></QueryClientProvider>
  </trpc.Provider>
);
```

- **Red:** tRPC te da seguridad de tipos, pero defines el router en el servidor y
  además montas el adaptador HTTP, el cliente, el `httpBatchLink` y dos
  *providers*. La frontera es real y la cruzas a mano (con ayuda de tipos).
- **Tiempo:** React Query + `useState`. Tras un `like` invalidas el query y
  refetcheas para que la UI cambie. La reactividad es de la **librería**.

**~6 archivos · ~150–180 líneas · ~5 deps directas (+ árbol enorme) · build (Vite/Next) + Prisma migrate.**

---

## 3. Elixir + Phoenix LiveView

El framework que **colapsa el cliente y el servidor** mejor del lado mainstream:
el servidor renderiza, calcula el diff y lo empuja por WebSocket. Tú casi no
escribes JS.

```elixir
# priv/repo/migrations/001_create_posts.exs
defmodule App.Repo.Migrations.CreatePosts do
  use Ecto.Migration
  def change do
    create table(:posts) do
      add :autor, :string
      add :texto, :string
      add :likes, :integer, default: 0
    end
  end
end
```

```elixir
# lib/app/timeline.ex
defmodule App.Timeline do
  import Ecto.Query
  alias App.{Repo, Timeline.Post}
  def feed,            do: Repo.all(from p in Post, order_by: p.id)
  def publicar(a, t),  do: Repo.insert!(%Post{autor: a, texto: t, likes: 0})
  def like(id) do
    p = Repo.get!(Post, id)
    Repo.update!(Ecto.Changeset.change(p, likes: p.likes + 1))
  end
end
```

```elixir
# lib/app_web/live/timeline_live.ex
defmodule AppWeb.TimelineLive do
  use AppWeb, :live_view
  alias App.Timeline

  def mount(_, _, socket), do: {:ok, assign(socket, posts: Timeline.feed())}

  def handle_event("like", %{"id" => id}, socket) do
    Timeline.like(String.to_integer(id))
    {:noreply, assign(socket, posts: Timeline.feed())}
  end

  def render(assigns) do
    ~H"""
    <ul>
      <li :for={p <- @posts}>
        <%= p.autor %>: <%= p.texto %> (<%= p.likes %>)
        <button phx-click="like" phx-value-id={p.id}>like</button>
      </li>
    </ul>
    """
  end
end
```

- **Red:** la frontera la maneja **LiveView** (un socket persistente). Tú escribes
  `handle_event` y `assign`; el diff y el empuje al navegador son del framework.
  No hay archivo de cliente.
- **Tiempo:** `assign` cambia el estado y LiveView re-renderiza el diff mínimo.
  Reactividad **del framework**, server-centric.

**~3 archivos · ~90–110 líneas · framework (Phoenix) + Ecto/migraciones.**

> LiveView es el más cercano *en espíritu* a Marea (borra la costura
> cliente/servidor), pero lo hace **un framework**, no el lenguaje, y es
> server-céntrico: lógica de cliente real exige *hooks* de JS.

---

## 4. Rust + Leptos + Axum

Isomórfico y con macros: `#[server]` es lo más parecido a `@server` en el mundo
mainstream —pero con ceremonia de framework, hidratación y manifiesto.

```rust
// src/app.rs
use leptos::*;

#[server]
pub async fn publicar(autor: String, texto: String) -> Result<(), ServerFnError> {
    db().execute("INSERT INTO posts (autor, texto, likes) VALUES (?,?,0)",
                 (autor, texto)).await?; Ok(())
}
#[server]
pub async fn like(id: i64) -> Result<(), ServerFnError> {
    db().execute("UPDATE posts SET likes = likes + 1 WHERE id = ?", (id,)).await?; Ok(())
}
#[server]
pub async fn feed() -> Result<Vec<Post>, ServerFnError> { Ok(db_query_all().await?) }

#[component]
pub fn App() -> impl IntoView {
    let posts = create_resource(|| (), |_| async { feed().await.unwrap_or_default() });
    let dar_like = create_action(|id: &i64| { let id = *id; async move {
        let _ = like(id).await; }});
    view! {
        <Suspense>
            <ul>{move || posts.get().map(|ps| ps.into_iter().map(|p| view! {
                <li>{p.autor} ": " {p.texto} " (" {p.likes} ")"
                    <button on:click=move |_| dar_like.dispatch(p.id)>"like"</button>
                </li>
            }).collect_view())}</ul>
        </Suspense>
    }
}
```

```rust
// src/main.rs  (Axum + Leptos SSR: monta rutas, server fns, fallback, hidratación)
#[tokio::main]
async fn main() {
    let conf = get_configuration(None).await.unwrap();
    let routes = generate_route_list(App);
    let app = axum::Router::new()
        .leptos_routes(&conf.leptos_options, routes, App)
        .route("/api/*fn_name", post(leptos_axum::handle_server_fns))
        .fallback(file_and_error_handler);
    axum::serve(listener, app.into_make_service()).await.unwrap();
}
```

- **Red:** la macro `#[server]` genera el endpoint y el stub de cliente —cerca de
  Marea— pero **registras** las server fns en Axum, montas SSR y configuras
  hidratación. Es macro + framework, no primitiva del lenguaje.
- **Tiempo:** `create_resource`/signals de Leptos. Reactividad de **librería**
  (excelente, pero la importas y la orquestas).

**~3 archivos · ~140–170 líneas · framework (Leptos+Axum) + `Cargo.toml` + tooling WASM.**

---

## 5. PHP + Laravel + Livewire

```php
// app/Livewire/Timeline.php
class Timeline extends Component
{
    public function publicar($autor, $texto) {
        Post::create(['autor' => $autor, 'texto' => $texto, 'likes' => 0]);
    }
    public function like($id) {
        Post::where('id', $id)->increment('likes');
    }
    public function render() {
        return view('livewire.timeline', ['posts' => Post::orderBy('id')->get()]);
    }
}
```

```blade
{{-- resources/views/livewire/timeline.blade.php --}}
<ul>
  @foreach ($posts as $p)
    <li>{{ $p->autor }}: {{ $p->texto }} ({{ $p->likes }})
      <button wire:click="like({{ $p->id }})">like</button>
    </li>
  @endforeach
</ul>
```

```php
// database/migrations/..._create_posts.php  +  app/Models/Post.php (modelo de 3 líneas)
Schema::create('posts', function (Blueprint $t) {
    $t->id(); $t->string('autor'); $t->string('texto'); $t->integer('likes')->default(0);
});
```

- **Red:** Livewire hace AJAX por debajo; `wire:click="like(id)"` invoca el método
  del componente en el servidor. Frontera del **framework**.
- **Tiempo:** al cambiar una propiedad pública, Livewire re-renderiza el componente
  y parcha el DOM. Reactividad **del framework**.

**~3 archivos (+ blade) · ~100–120 líneas · framework (Laravel+Livewire) + migraciones.**

---

## El recuento

| Stack | Archivos | Líneas de app | Frontera de RED | Frontera del TIEMPO | Persistencia | Deps / build |
|---|---:|---:|---|---|---|---|
| **Marea** | **1** | **42** | `@server`/`@client` — **primitiva del lenguaje** | `reactive`/`effect` — **primitiva** | `store T;` (1 línea) | **0 / 0** |
| React+tRPC+Prisma | ~6 | ~150–180 | router + cliente + HTTP link (librería, tú lo cableas) | React Query + `useState` (librería) | Prisma schema + migrate | ~5 directas / Vite o Next |
| Phoenix LiveView | ~3 | ~90–110 | socket WS (framework) | `assign` + diff (framework) | Ecto + migración | framework / mix |
| Rust+Leptos+Axum | ~3 | ~140–170 | `#[server]` macro + registro en Axum | signals/`create_resource` (librería) | sqlx + esquema | framework / cargo+wasm |
| Laravel+Livewire | ~3 | ~100–120 | `wire:` AJAX (framework) | re-render por propiedad (framework) | Eloquent + migración | framework / composer+vite |

**La diferencia no es "menos líneas".** Es **dónde vive cada frontera**:

- En los otros cuatro, las fronteras viven en un **framework** o en **librerías
  que cableas** (router+cliente, providers, `#[server]`+SSR, sockets). Borran la
  costura a nivel de *framework*.
- En Marea viven en el **lenguaje**: `@server` y `reactive` son palabras clave que
  el **compilador** materializa. No hay framework que adoptar ni pegamento que
  mantener; un programa nuevo no arranca con `package.json`.

---

## Honestidad: dónde Marea aún NO compite

Esto sería deshonesto sin la otra cara:

- **Madurez:** Marea es v0. Los otros cuatro corren en producción a escala. Marea
  no tiene auth, validación robusta, ni un ORM real con migraciones versionadas.
- **UI de navegador:** los otros cuatro pintan un DOM interactivo. La demo de
  Marea renderiza a **consola**; aún no cablea RPC + reactividad + DOM en una
  página (su backend web es incipiente).
- **Ecosistema:** `npm`/`hex`/`crates`/`composer` tienen todo. Marea tiene su
  runtime y poco más.
- **Drivers de BD probados:** los backends Postgres/MySQL/Mongo de Marea existen
  pero solo SQLite y archivo están verificados end-to-end aquí.
- **El argumento es de diseño, no de "úsalo hoy".** Marea muestra que las dos
  fronteras *pueden* ser primitivas; los demás muestran qué se necesita para
  enviar a producción. Ambas cosas son ciertas a la vez.

---

## Conclusión

Phoenix, Livewire y Leptos ya demostraron que **la costura cliente/servidor se
puede borrar** —a nivel de framework. tRPC demostró que **se puede tipar de punta
a punta** —a nivel de librería. Marea pregunta lo siguiente: si esto es tan
fundamental para la web, **¿por qué no es parte del lenguaje?** El experimento de
arriba es la respuesta en código: cuando `@server` y `reactive` son palabras clave,
la app es un archivo sin configuración, y las dos fronteras dejan de ser tu
problema para volverse del compilador.

Queda el trabajo difícil —madurez, ecosistema, una UI de navegador de verdad—
pero la tesis se sostiene: **las fronteras de la web caben en el lenguaje.**
