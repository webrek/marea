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
const BUILTINS: &str = "{ __register, __rpc, print, concat, render, __marea_is }";

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
    let body = emit_block_inner(&f.body, 1);
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

fn emit_block_inner(block: &Block, indent: usize) -> String {
    block
        .stmts
        .iter()
        .map(|s| emit_stmt(s, indent))
        .collect::<Vec<_>>()
        .join("\n")
}

fn emit_stmt(stmt: &Stmt, indent: usize) -> String {
    let p = pad(indent);
    match stmt {
        Stmt::Let(l) => {
            let kw = if l.mutable { "let" } else { "const" };
            let note = if l.reactive {
                "  /* reactive: modelo de reactividad en v3 */"
            } else {
                ""
            };
            format!("{p}{kw} {} = {};{note}", l.name, emit_expr(&l.value))
        }
        Stmt::Return { value, .. } => match value {
            Some(v) => format!("{p}return {};", emit_expr(v)),
            None => format!("{p}return;"),
        },
        Stmt::Expr(e) => match e {
            Expr::If { .. } | Expr::Match { .. } => emit_control(e, indent),
            _ => format!("{p}{};", emit_expr(e)),
        },
    }
}

fn emit_control(e: &Expr, indent: usize) -> String {
    let p = pad(indent);
    match e {
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            let mut s = format!("{p}if ({}) {{\n", emit_expr(cond));
            s.push_str(&emit_block_inner(then_branch, indent + 1));
            s.push_str(&format!("\n{p}}}"));
            if let Some(eb) = else_branch {
                match eb.as_ref() {
                    ElseBranch::Block(b) => {
                        s.push_str(" else {\n");
                        s.push_str(&emit_block_inner(b, indent + 1));
                        s.push_str(&format!("\n{p}}}"));
                    }
                    ElseBranch::If(inner) => {
                        s.push_str(" else ");
                        s.push_str(emit_control(inner, indent).trim_start());
                    }
                }
            }
            s
        }
        Expr::Match {
            scrutinee, arms, ..
        } => emit_match(scrutinee, arms, indent),
        _ => format!("{p}{};", emit_expr(e)),
    }
}

fn emit_match(scrut: &Expr, arms: &[MatchArm], indent: usize) -> String {
    let p = pad(indent);
    let pin = pad(indent + 1);
    let mut s = format!("{p}{{\n{pin}const __m = {};\n", emit_expr(scrut));
    let mut chained = false; // ¿ya emitimos algún if/else-if?
    for arm in arms {
        let body = match &arm.body {
            Expr::If { .. } | Expr::Match { .. } => emit_control(&arm.body, indent + 2),
            _ => format!("{}{};", pad(indent + 2), emit_expr(&arm.body)),
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

fn emit_expr(e: &Expr) -> String {
    match e {
        Expr::Int { value, .. } => value.to_string(),
        Expr::Float { value, .. } => value.to_string(),
        Expr::Str { value, .. } => js_string(value),
        Expr::Bool { value, .. } => value.to_string(),
        Expr::Ident { name, .. } => name.clone(),
        Expr::Unary { op, expr, .. } => {
            let inner = emit_expr(expr);
            match op {
                UnaryOp::Neg => format!("-({inner})"),
                UnaryOp::Not => format!("!({inner})"),
            }
        }
        Expr::Binary {
            op, left, right, ..
        } => format!("({} {} {})", emit_expr(left), map_binop(*op), emit_expr(right)),
        // Toda llamada se 'await': uniforme para builtins, locales y cruces de
        // frontera (los stubs RPC son async).
        Expr::Call { callee, args, .. } => {
            let a: Vec<String> = args.iter().map(emit_expr).collect();
            format!("(await {}({}))", emit_expr(callee), a.join(", "))
        }
        Expr::Member { object, field, .. } => format!("{}.{}", emit_expr(object), field),
        // if/match en posición de expresión: IIFE async (sin síntesis de valor).
        Expr::If { .. } | Expr::Match { .. } => {
            format!(
                "(await (async () => {{\n{}\n}})())",
                emit_control(e, 1)
            )
        }
        // Literal de registro -> objeto JS.
        Expr::Record { fields, .. } => {
            let parts: Vec<String> = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, emit_expr(&f.value)))
                .collect();
            format!("{{ {} }}", parts.join(", "))
        }
        // Literal de lista -> arreglo JS.
        Expr::List { elements, .. } => {
            let parts: Vec<String> = elements.iter().map(emit_expr).collect();
            format!("[{}]", parts.join(", "))
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
