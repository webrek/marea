//! `marea-codegen` — transpilador de Marea a TypeScript.
//!
//! La rebanada angosta ("bala trazadora") que demuestra la tesis: una función
//! `@server` se vuelve (a) un handler registrado en el servidor y (b) un *stub*
//! en el cliente que cruza la frontera por RPC. El `@client` que la llama no
//! nota la diferencia: la sigue invocando como si fuera local.
//!
//! No hace chequeo de tipos (eso es v1.5). El objetivo es ver un `.mar` corriendo
//! end-to-end y validar el diseño.

use marea_syntax::ast::*;
use std::collections::HashSet;

/// El runtime TypeScript embebido (transporte RPC + builtins).
pub const RUNTIME_TS: &str = include_str!("runtime.ts");

/// Los cuatro archivos TypeScript que produce el transpilador.
pub struct Project {
    pub runtime: String,
    pub server: String,
    pub client: String,
    pub demo: String,
}

/// Builtins provistos por el runtime; no se transpilan ni se registran.
const BUILTINS: &str =
    "{ __register, __rpc, print, concat, render, __marea_is, __signal, __memo, __effect }";

/// Una función con `@server` o `@edge` corre "remota": handler + stub RPC.
fn is_remote(f: &FnDecl) -> bool {
    matches!(f.location, Some(Location::Server) | Some(Location::Edge))
}

pub fn emit(module: &Module) -> Project {
    let mut remote = Vec::new();
    let mut local = Vec::new();
    for item in &module.items {
        if let Item::Fn(f) = item {
            if is_remote(f) {
                remote.push(f);
            } else {
                local.push(f);
            }
        }
        // Los `type` y `let` de nivel superior se ignoran en esta fase.
    }

    Project {
        runtime: RUNTIME_TS.to_string(),
        server: emit_server(&remote),
        client: emit_client(&remote, &local),
        demo: emit_demo(&local),
    }
}

fn emit_server(remote: &[&FnDecl]) -> String {
    let mut s = String::new();
    s.push_str("// Generado por Marea — lado servidor.\n");
    s.push_str(&format!("import {} from \"./runtime.ts\";\n\n", BUILTINS));
    for f in remote {
        s.push_str(&emit_fn_def(f, false));
        s.push('\n');
        let pass: Vec<String> = (0..f.params.len()).map(|i| format!("args[{i}]")).collect();
        s.push_str(&format!(
            "__register(\"{}\", (args) => {}({}));\n\n",
            f.name,
            f.name,
            pass.join(", ")
        ));
    }
    s
}

fn emit_client(remote: &[&FnDecl], local: &[&FnDecl]) -> String {
    let mut s = String::new();
    s.push_str("// Generado por Marea — lado cliente.\n");
    s.push_str(&format!("import {} from \"./runtime.ts\";\n\n", BUILTINS));
    for f in remote {
        s.push_str(&emit_stub(f));
        s.push('\n');
    }
    for f in local {
        s.push_str(&emit_fn_def(f, true));
        s.push('\n');
    }
    s
}

fn emit_demo(local: &[&FnDecl]) -> String {
    let has_main = local.iter().any(|f| f.name == "main");
    let mut s = String::new();
    s.push_str("// Generado por Marea — orquestador de la demo.\n");
    s.push_str("import { startServer, stopServer } from \"./runtime.ts\";\n");
    s.push_str("import \"./server.ts\";\n");
    if has_main {
        s.push_str("import { main } from \"./client.ts\";\n");
    }
    s.push_str("\nawait startServer();\ntry {\n");
    if has_main {
        s.push_str("  await main();\n");
    } else {
        s.push_str("  console.log(\"[marea] no hay función main() en @client\");\n");
    }
    s.push_str("} finally {\n  await stopServer();\n}\n");
    s
}

// --- funciones ---

fn emit_fn_def(f: &FnDecl, export: bool) -> String {
    let params = ts_params(f);
    let kw = if export {
        "export async function"
    } else {
        "async function"
    };
    // El conjunto de variables reactivas se construye incrementalmente por
    // bloque (respetando el alcance léxico), arrancando vacío.
    let body = emit_block_inner(&f.body, 1, &HashSet::new());
    format!(
        "{}\n{} {}({}) {{\n{}\n}}\n",
        signature_comment(f),
        kw,
        f.name,
        params,
        body
    )
}

fn emit_stub(f: &FnDecl) -> String {
    let params = ts_params(f);
    let arg_names: Vec<String> = f.params.iter().map(|p| p.name.clone()).collect();
    let ret = f
        .return_type
        .as_ref()
        .map(map_type)
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        "// stub generado para la función @server '{name}' (cruza la frontera por RPC)\n\
         async function {name}({params}): Promise<{ret}> {{\n  \
         return (await __rpc(\"{name}\", [{args}])) as {ret};\n}}\n",
        name = f.name,
        params = params,
        ret = ret,
        args = arg_names.join(", ")
    )
}

fn ts_params(f: &FnDecl) -> String {
    f.params
        .iter()
        .map(|p| format!("{}: {}", p.name, map_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

// --- sentencias ---

fn emit_block_inner(block: &Block, indent: usize, reactive: &HashSet<String>) -> String {
    // `current` arranca con las reactivas del alcance externo y se actualiza al
    // procesar cada sentencia: un `reactive` añade el nombre; un `let`/binding
    // no-reactivo del mismo nombre lo SOMBREA (lo quita) para el resto del
    // bloque. Así una lectura del nombre sombreado no emite `.get()` erróneo.
    let mut current = reactive.clone();
    let mut lines = Vec::with_capacity(block.stmts.len());
    for stmt in &block.stmts {
        // La sentencia se emite con el alcance ANTERIOR a su propio binding
        // (el RHS de `let n = ...` ve la `n` externa, no la que declara).
        lines.push(emit_stmt(stmt, indent, &current));
        if let Stmt::Let(l) = stmt {
            if l.reactive {
                current.insert(l.name.clone());
            } else {
                current.remove(&l.name);
            }
        }
    }
    lines.join("\n")
}

fn emit_stmt(stmt: &Stmt, indent: usize, reactive: &HashSet<String>) -> String {
    let p = pad(indent);
    match stmt {
        Stmt::Let(l) if l.reactive => {
            // Fuente reactiva (mutable) = signal; derivada (inmutable) = memo.
            let init = emit_expr(&l.value, reactive);
            if l.mutable {
                format!("{p}const {} = __signal({init});", l.name)
            } else {
                format!("{p}const {} = __memo(() => {init});", l.name)
            }
        }
        Stmt::Let(l) => {
            let kw = if l.mutable { "let" } else { "const" };
            format!("{p}{kw} {} = {};", l.name, emit_expr(&l.value, reactive))
        }
        // Asignar a una variable reactiva = invocar su setter; si no, asignación normal.
        Stmt::Assign { name, value, .. } => {
            let v = emit_expr(value, reactive);
            if reactive.contains(name) {
                format!("{p}{name}.set({v});")
            } else {
                format!("{p}{name} = {v};")
            }
        }
        // Efecto: se re-ejecuta cuando cambian las reactivas que lee.
        Stmt::Effect { body, .. } => {
            let inner = emit_block_inner(body, indent + 1, reactive);
            format!("{p}__effect(async () => {{\n{inner}\n{p}}});")
        }
        Stmt::Return { value, .. } => match value {
            Some(v) => format!("{p}return {};", emit_expr(v, reactive)),
            None => format!("{p}return;"),
        },
        Stmt::Expr(e) => match e {
            Expr::If { .. } | Expr::Match { .. } => emit_control(e, indent, reactive),
            _ => format!("{p}{};", emit_expr(e, reactive)),
        },
    }
}

fn emit_control(e: &Expr, indent: usize, reactive: &HashSet<String>) -> String {
    let p = pad(indent);
    match e {
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let mut s = format!("{p}if ({}) {{\n", emit_expr(cond, reactive));
            s.push_str(&emit_block_inner(then_branch, indent + 1, reactive));
            s.push_str(&format!("\n{p}}}"));
            if let Some(eb) = else_branch {
                match eb.as_ref() {
                    ElseBranch::Block(b) => {
                        s.push_str(" else {\n");
                        s.push_str(&emit_block_inner(b, indent + 1, reactive));
                        s.push_str(&format!("\n{p}}}"));
                    }
                    ElseBranch::If(inner) => {
                        s.push_str(" else ");
                        s.push_str(emit_control(inner, indent, reactive).trim_start());
                    }
                }
            }
            s
        }
        Expr::Match {
            scrutinee, arms, ..
        } => emit_match(scrutinee, arms, indent, reactive, false),
        _ => format!("{p}{};", emit_expr(e, reactive)),
    }
}

/// `returning`: si es `true`, el cuerpo de cada rama se emite como `return <v>;`
/// (match en posición de EXPRESIÓN, dentro de un IIFE que produce el valor); si
/// es `false`, como sentencia (`<v>;`).
fn emit_match(
    scrut: &Expr,
    arms: &[MatchArm],
    indent: usize,
    reactive: &HashSet<String>,
    returning: bool,
) -> String {
    let p = pad(indent);
    let pin = pad(indent + 1);
    let mut s = format!("{p}{{\n{pin}const __m = {};\n", emit_expr(scrut, reactive));
    let mut chained = false; // ¿ya emitimos algún if/else-if?
    for arm in arms {
        let body = match &arm.body {
            Expr::If { .. } | Expr::Match { .. } => emit_control(&arm.body, indent + 2, reactive),
            _ if returning => format!(
                "{}return {};",
                pad(indent + 2),
                emit_expr(&arm.body, reactive)
            ),
            _ => format!("{}{};", pad(indent + 2), emit_expr(&arm.body, reactive)),
        };
        match &arm.pattern {
            Pattern::Wildcard { .. } => {
                s.push_str(&format!("{pin}else {{\n{body}\n{pin}}}\n"));
            }
            Pattern::Binding { name, .. } => {
                if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                    // Variante (Mayúscula): comparación best-effort.
                    let kw = if chained { "else if" } else { "if" };
                    s.push_str(&format!(
                        "{pin}{kw} (__marea_is(__m, \"{name}\")) {{\n{body}\n{pin}}}\n"
                    ));
                    chained = true;
                } else {
                    // Enlace (minúscula): captura todo y nombra el valor.
                    s.push_str(&format!("{pin}else {{ const {name} = __m;\n{body}\n{pin}}}\n"));
                }
            }
            Pattern::Int { value, .. } => {
                let kw = if chained { "else if" } else { "if" };
                s.push_str(&format!("{pin}{kw} (__m === {value}) {{\n{body}\n{pin}}}\n"));
                chained = true;
            }
            Pattern::Bool { value, .. } => {
                let kw = if chained { "else if" } else { "if" };
                s.push_str(&format!("{pin}{kw} (__m === {value}) {{\n{body}\n{pin}}}\n"));
                chained = true;
            }
            Pattern::Str { value, .. } => {
                let kw = if chained { "else if" } else { "if" };
                s.push_str(&format!(
                    "{pin}{kw} (__m === {}) {{\n{body}\n{pin}}}\n",
                    js_string(value)
                ));
                chained = true;
            }
        }
    }
    s.push_str(&format!("{p}}}"));
    s
}

// --- expresiones ---

fn emit_expr(e: &Expr, reactive: &HashSet<String>) -> String {
    match e {
        Expr::Int { value, .. } => value.to_string(),
        Expr::Float { value, .. } => value.to_string(),
        Expr::Str { value, .. } => js_string(value),
        Expr::Bool { value, .. } => value.to_string(),
        Expr::Ident { name, .. } => {
            if reactive.contains(name) {
                // Leer una reactiva = su getter (rastrea dependencias).
                format!("{name}.get()")
            } else if name.chars().next().is_some_and(|c| c.is_uppercase()) {
                // Variante nominal usada como valor (errores como valores): se
                // representa por su etiqueta, que '__marea_is' reconoce en match.
                js_string(name)
            } else {
                name.clone()
            }
        }
        Expr::Unary { op, expr, .. } => {
            let inner = emit_expr(expr, reactive);
            match op {
                UnaryOp::Neg => format!("-({inner})"),
                UnaryOp::Not => format!("!({inner})"),
            }
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = emit_expr(left, reactive);
            let r = emit_expr(right, reactive);
            match op {
                // División entera: trunca hacia cero, igual que i32.div_s de WASM.
                // JS '/' daría flotante (7/2=3.5) y rompería el contrato Int.
                BinOp::Div => format!("Math.trunc({l} / {r})"),
                _ => format!("({} {} {})", l, map_binop(*op), r),
            }
        }
        // Las llamadas a funciones del usuario y stubs RPC son async (se
        // 'await'); los builtins síncronos (print/concat/render) NO se awaitan,
        // porque un 'await' espurio rompe el rastreo de dependencias reactivas
        // (el cuerpo del effect se suspendería y __currentSub se restauraría
        // antes de leer los signals).
        Expr::Call { callee, args, .. } => {
            let a: Vec<String> = args.iter().map(|x| emit_expr(x, reactive)).collect();
            let callee_ts = emit_expr(callee, reactive);
            let is_sync_builtin = matches!(
                callee.as_ref(),
                Expr::Ident { name, .. } if name == "print" || name == "concat" || name == "render"
            );
            if is_sync_builtin {
                format!("{}({})", callee_ts, a.join(", "))
            } else {
                format!("(await {}({}))", callee_ts, a.join(", "))
            }
        }
        Expr::Member { object, field, .. } => format!("{}.{}", emit_expr(object, reactive), field),
        // 'match' en posición de expresión: IIFE que RETORNA el valor de la rama.
        Expr::Match { scrutinee, arms, .. } => {
            format!(
                "(await (async () => {{\n{}\n}})())",
                emit_match(scrutinee, arms, 1, reactive, true)
            )
        }
        // 'if' en posición de expresión: IIFE (las ramas-bloque no tienen valor
        // de cola en la gramática; queda como sentencia, valor diferido).
        Expr::If { .. } => {
            format!(
                "(await (async () => {{\n{}\n}})())",
                emit_control(e, 1, reactive)
            )
        }
        // Literal de registro -> objeto JS.
        Expr::Record { fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, emit_expr(&f.value, reactive)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        // Literal de lista -> arreglo JS.
        Expr::List { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(|x| emit_expr(x, reactive)).collect();
            format!("[{}]", parts.join(", "))
        }
        // Indexado -> acceso por índice JS.
        Expr::Index { object, index, .. } => {
            format!("{}[{}]", emit_expr(object, reactive), emit_expr(index, reactive))
        }
    }
}

// --- utilidades ---

fn pad(n: usize) -> String {
    "  ".repeat(n)
}

fn map_binop(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "+",
        BinOp::Sub => "-",
        BinOp::Mul => "*",
        BinOp::Div => "/",
        BinOp::Rem => "%",
        BinOp::Eq => "===",
        BinOp::Ne => "!==",
        BinOp::Lt => "<",
        BinOp::Gt => ">",
        BinOp::Le => "<=",
        BinOp::Ge => ">=",
        BinOp::And => "&&",
        BinOp::Or => "||",
    }
}

fn map_type(t: &Type) -> String {
    match t {
        Type::Name { name, .. } => match name.as_str() {
            "Int" | "Float" => "number".to_string(),
            "String" => "string".to_string(),
            "Bool" => "boolean".to_string(),
            // Tipos aún sin definir: conservamos el nombre (Node lo ignora al
            // ejecutar; el chequeo real de tipos llega en v1.5).
            _ => name.clone(),
        },
        Type::Union { variants, .. } => variants
            .iter()
            .map(map_type)
            .collect::<Vec<_>>()
            .join(" | "),
        // Tipo registro -> type-literal TS preciso.
        Type::Record { fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, map_type(&f.ty)))
                .collect();
            format!("{{ {} }}", parts.join("; "))
        }
    }
}

fn signature_comment(f: &FnDecl) -> String {
    let loc = match f.location {
        Some(Location::Server) => "@server ",
        Some(Location::Client) => "@client ",
        Some(Location::Edge) => "@edge ",
        None => "",
    };
    let params = f
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, type_to_src(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    let ret = f
        .return_type
        .as_ref()
        .map(|t| format!(" -> {}", type_to_src(t)))
        .unwrap_or_default();
    format!("// {loc}fn {}({}){}", f.name, params, ret)
}

fn type_to_src(t: &Type) -> String {
    match t {
        Type::Name { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                format!(
                    "{}<{}>",
                    name,
                    args.iter().map(type_to_src).collect::<Vec<_>>().join(", ")
                )
            }
        }
        Type::Union { variants, .. } => variants
            .iter()
            .map(type_to_src)
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Record { fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, type_to_src(&f.ty)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
    }
}

/// Literal de cadena JS con escapes. Conserva UTF-8 (emojis, acentos) tal cual.
fn js_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}
