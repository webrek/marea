//! Lo que el GENERADOR hace con la política de un handler.
//!
//! La fase 1 hizo de `@server(Usuario)` una garantía de compilación; el endpoint
//! seguía atendiendo a cualquiera. Estos tests fijan la forma emitida que cierra
//! esa brecha: el handler se registra junto al resolutor de identidad, el runtime
//! la resuelve y la pasa como PRIMER argumento, y la aridad que se valida sigue
//! siendo la del cliente —la identidad no viaja en la llamada—.

use marea_codegen::{emit, emit_app};
use marea_syntax::parse;

fn build(src: &str) -> marea_codegen::Project {
    emit(&parse(src).unwrap())
}

/// Un programa con identidad: la `@session`, un handler que la exige y otro
/// declarado público.
const CON_SESSION: &str = r#"
type Usuario = { nombre: String };
type Post = { autor: String };
store posts: Post;

@session
fn quien(token: String) -> Usuario | NoAutorizado {
    if contains(token, "secreto") { return Usuario { nombre: "ada" }; }
    return NoAutorizado;
}

@server(u: Usuario)
fn publicar(texto: String) { save(posts, Post { autor: u.nombre }); }

@server(Public)
fn feed() -> List<Post> { return all(posts); }

@client
fn vista() -> Html { return `<p>x</p>`; }
"#;

/// La línea de `__register` de un handler, para no afirmar sobre la de otro.
fn registro(server: &str, nombre: &str) -> String {
    let prefijo = format!("__register(\"{nombre}\"");
    server
        .lines()
        .find(|l| l.starts_with(&prefijo))
        .unwrap_or_else(|| panic!("no se registró '{nombre}' en:\n{server}"))
        .to_string()
}

fn contiene(texto: &str, aguja: &str) {
    assert!(texto.contains(aguja), "falta «{aguja}» en:\n{texto}");
}

fn no_contiene(texto: &str, aguja: &str) {
    assert!(!texto.contains(aguja), "sobra «{aguja}» en:\n{texto}");
}

#[test]
fn el_handler_con_politica_se_registra_con_su_resolutor() {
    let p = build(CON_SESSION);
    let r = registro(&p.server, "publicar");
    // El tercer argumento es la @session: es lo que hace que el runtime pida el
    // token antes de ejecutar nada.
    assert!(r.ends_with("}, quien);"), "{r}");
    // La identidad llega por el segundo parámetro del envoltorio (la pone el
    // runtime) y se reenvía como PRIMER argumento de la función del usuario.
    contiene(&r, "(__args, __identidad) =>");
    // La identidad va DELANTE de lo que manda el cliente, y con su tipo
    // afirmado: el transporte la tipa `unknown` y aquí sí se sabe qué es.
    contiene(&r, "return publicar(__identidad as Usuario, __args[0]);");
}

#[test]
fn la_aridad_validada_es_la_del_cliente() {
    // La identidad no viaja en la llamada: contarla haría fallar a todo cliente
    // honesto y, encima, invitaría a mandarla desde fuera.
    let p = build(CON_SESSION);
    let r = registro(&p.server, "publicar");
    contiene(&r, "if (__args.length !== 1)");
    // Y el guarda del argumento 1 sigue mirando __args[0], no __args[1].
    contiene(&r, "typeof __args[0] === \"string\"");
    contiene(&r, "__badRequest(\"argumento 1\")");
}

#[test]
fn la_identidad_es_el_primer_parametro_de_la_funcion() {
    let p = build(CON_SESSION);
    let s = &p.server;
    contiene(s, "async function publicar(u: Usuario, texto: string)");
    // La firma emitida dice a quién deja entrar, igual que dice dónde corre.
    contiene(s, "// @server(u: Usuario) fn publicar(texto: String)");
}

#[test]
fn la_politica_sin_nombre_recibe_igual_la_identidad() {
    // `@server(Usuario)` exige identidad sin usarla: el runtime la pasa igual,
    // sólo que el cuerpo no la mira. Así el envoltorio tiene una sola forma.
    let p = build(
        "type Usuario = { nombre: String };
@session fn quien(t: String) -> Usuario | NoAutorizado { return NoAutorizado; }
@server(Usuario) fn borrar(i: Int) { print(i); }",
    );
    let s = &p.server;
    contiene(s, "async function borrar(__identidad: Usuario, i: number)");
    let r = registro(s, "borrar");
    contiene(&r, "return borrar(__identidad as Usuario, __args[0]);");
}

#[test]
fn public_se_registra_como_siempre() {
    // La otra mitad de la regla: lo declarado público no paga nada, ni siquiera
    // una comprobación de más. Es lo que mantiene el cambio compatible.
    let p = build(CON_SESSION);
    let r = registro(&p.server, "feed");
    contiene(&r, "(__args) =>");
    assert!(r.ends_with("return feed(); });"), "{r}");
}

#[test]
fn sin_politica_el_registro_no_cambia() {
    let p = build("@server fn saludar(n: String) -> String { return n; }");
    let r = registro(&p.server, "saludar");
    contiene(&r, "(__args) =>");
    no_contiene(&r, "__identidad");
}

#[test]
fn el_stub_del_cliente_no_cambia() {
    // El cliente llama igual que antes: la identidad no es un argumento suyo, y
    // por eso no puede fabricarla.
    let p = build(CON_SESSION);
    let c = &p.client;
    contiene(c, "async function publicar(texto: string)");
    contiene(c, "__rpc(\"publicar\", [texto])");
    no_contiene(c, "__identidad");
}

#[test]
fn la_session_vive_solo_en_el_servidor() {
    // Su cuerpo decide qué token vale: no tiene por qué viajar al navegador. Ni
    // el cliente puede llamarla (el verificador lo prohíbe) ni se registra como
    // handler (sería un oráculo: la identidad de cualquier token).
    let p = build(CON_SESSION);
    contiene(&p.server, "async function quien(token: string)");
    no_contiene(&p.server, "__register(\"quien\"");
    no_contiene(&p.client, "quien");

    let app = emit_app(&parse(CON_SESSION).unwrap());
    no_contiene(&app.client_js, "quien");
}

#[test]
fn sin_session_la_politica_falla_cerrada() {
    // El verificador ya rechaza una política sin @session, así que esto no
    // debería llegar a generarse; si aun así ocurre, el registro deniega en vez
    // de abrirse. Un error del compilador no puede acabar en endpoint público.
    let p = build(
        "type Usuario = { nombre: String };
@server(Usuario) fn borrar(i: Int) { print(i); }",
    );
    let r = registro(&p.server, "borrar");
    let cerrado = "}, () => ({ $tag: \"NoAutorizado\" }));";
    assert!(r.ends_with(cerrado), "{r}");
}

// ===================== El runtime que se puede empaquetar =====================

/// Sin `store`, el runtime emitido NO puede traer los backends de persistencia.
///
/// No es tamaño: tres de ellos hacen `import("pg" | "mysql2" | "mongodb")`, que
/// son paquetes de npm que un consumidor sin base de datos no instala, y un
/// empaquetador (Next, Vite) RESUELVE el especificador aunque el código sea
/// inalcanzable. Con ellos dentro, el `.ts` generado no se puede empaquetar:
/// "Module not found: Can't resolve 'mongodb'". Lo reportó Vigía al meter su
/// gráfica en un componente de servidor de Next, que es donde `node:http` sí
/// vale y estos tres no.
#[test]
fn sin_store_el_runtime_no_arrastra_drivers_de_npm() {
    let p = marea_codegen::emit(
        &marea_syntax::parse("fn grafica(xs: List<Int>) -> Html { return `<svg/>`; }").unwrap(),
    );
    for driver in [
        "\"pg\"",
        "\"mysql2/promise\"",
        "\"mongodb\"",
        "\"node:sqlite\"",
    ] {
        assert!(
            !p.runtime.contains(driver),
            "el runtime sin store no debe mencionar {driver}"
        );
    }
    // Y la lista de importación tampoco puede pedir lo que ya no se exporta.
    for n in ["__store", "save", "all", "update", "remove"] {
        let pedido = format!(" {n},");
        assert!(
            !p.client.lines().next().unwrap_or("").contains(&pedido)
                && !p.client.contains(&format!("{{ {n},")),
            "el import no debe pedir '{n}' sin store"
        );
    }
}

/// Y con `store`, sigue completo: el recorte no puede llevarse la persistencia
/// de quien sí la usa.
#[test]
fn con_store_el_runtime_conserva_los_backends() {
    let p = marea_codegen::emit(
        &marea_syntax::parse(
            "type P = { x: Int };\nstore cosas: P;\n\
             @server fn g(p: P) { save(cosas, p); }",
        )
        .unwrap(),
    );
    for driver in [
        "\"pg\"",
        "\"mysql2/promise\"",
        "\"mongodb\"",
        "\"node:sqlite\"",
    ] {
        assert!(p.runtime.contains(driver), "falta {driver}");
    }
    assert!(p.client.contains("__store"), "{}", p.client);
}

// ============== El runtime que puede vivir fuera de Node ==============

/// Un módulo que no cruza la frontera de red ni declara almacenes no necesita
/// nada de Node, y arrastrarlo le impide vivir en un componente de cliente o en
/// el edge. Es la frontera que el lenguaje presume de tener resuelta, así que no
/// puede ser el propio runtime quien la cierre.
#[test]
fn un_modulo_puro_no_arrastra_node() {
    let p = marea_codegen::emit(
        &marea_syntax::parse(
            "fn ficha(t: String) -> Html { return `<li>{t}</li>`; }\n\
             fn main() { print(ficha(\"x\")); }",
        )
        .unwrap(),
    );
    for atadura in ["node:http", "node:fs", "process.env", "http.createServer"] {
        assert!(
            !p.runtime.contains(atadura),
            "el runtime puro no debe mencionar {atadura}"
        );
    }
    // Y el import tampoco puede pedir lo que el runtime recortado ya no exporta.
    for n in ["__register", "__rpc", "fetch", "post", "__store"] {
        assert!(
            !p.client.contains(&format!(" {n},")) && !p.client.contains(&format!("{{ {n},")),
            "el import de un módulo puro no debe pedir '{n}'"
        );
    }
    // La demo no puede arrancar un servidor que ya no existe.
    assert!(!p.demo.contains("startServer"), "{}", p.demo);
    // Pero el núcleo reactivo SÍ tiene que seguir ahí: es lo que hará falta
    // para un manejador de evento, y es puro.
    assert!(
        p.runtime.contains("export function __signal"),
        "{}",
        p.runtime
    );
    assert!(
        p.runtime.contains("export function __effect"),
        "{}",
        p.runtime
    );
    // Los marcadores hablan con el codegen, no con quien lee la salida.
    assert!(
        !p.runtime.contains("@marea:"),
        "quedaron marcadores en la salida"
    );
}

/// Y en cuanto el módulo cruza la frontera, el runtime vuelve entero: el
/// recorte no puede llevarse el transporte de quien sí lo usa.
#[test]
fn un_modulo_con_server_conserva_el_transporte() {
    let p = marea_codegen::emit(
        &marea_syntax::parse(
            "@server fn dime() -> String { return \"hola\"; }\n\
             @client fn main() { print(dime()); }",
        )
        .unwrap(),
    );
    for necesario in [
        "node:http",
        "export function startServer",
        "export async function __rpc",
    ] {
        assert!(p.runtime.contains(necesario), "falta {necesario}");
    }
    assert!(p.client.contains("__rpc"), "{}", p.client);
}
