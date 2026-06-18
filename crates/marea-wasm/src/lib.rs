//! `marea-wasm` — backend de Marea a WebAssembly (formato texto WAT).
//!
//! Toma el mismo AST que el backend de TypeScript y emite WAT. Es un segundo
//! destino enchufado al front-end: el lenguaje no cambia, sólo el codegen. Así
//! la lógica corre como WebAssembly en el navegador, recortando la dependencia
//! del JS-como-lenguaje a un pegamento mínimo.
//!
//! Primer slice (sin typechecker todavía): sólo enteros y booleanos (`i32`),
//! `let`, `return`, `if` como sentencia, aritmética/comparación/lógica y
//! llamadas a funciones. Lo no soportado (flotantes, cadenas, `match`, miembros,
//! `reactive`) produce un error claro en vez de WAT roto.

use marea_syntax::ast::*;

/// Transpila un módulo a un `(module ...)` de WAT, o un error legible.
pub fn emit_wat(module: &Module) -> Result<String, String> {
    let mut funcs = Vec::new();
    for item in &module.items {
        if let Item::Fn(f) = item {
            let wat = emit_func(f).map_err(|e| format!("en la función '{}': {}", f.name, e))?;
            funcs.push(wat);
        }
        // 'type' y 'let' de nivel superior se ignoran en este slice.
    }
    if funcs.is_empty() {
        return Err("no hay funciones que compilar a WASM".to_string());
    }
    Ok(format!("(module\n{}\n)\n", funcs.join("\n")))
}

fn emit_func(f: &FnDecl) -> Result<String, String> {
    let params = f
        .params
        .iter()
        .map(|p| {
            check_i32(&p.ty)?;
            Ok(format!(" (param ${} i32)", p.name))
        })
        .collect::<Result<Vec<_>, String>>()?
        .join("");

    let result = match &f.return_type {
        Some(t) => {
            check_i32(t)?;
            " (result i32)".to_string()
        }
        None => String::new(),
    };

    let mut locals = Vec::new();
    collect_locals(&f.body, &mut locals)?;
    let locals_decl: String = locals
        .iter()
        .map(|n| format!("\n    (local ${n} i32)"))
        .collect();

    let body = emit_block(&f.body, 2)?;

    Ok(format!(
        "  (func ${name} (export \"{name}\"){params}{result}{locals_decl}\n{body}\n  )",
        name = f.name,
    ))
}

/// Recoge los nombres de las variables `let` (incluidas las de ramas `if`) para
/// declararlas como locales al inicio de la función, como exige WAT.
fn collect_locals(block: &Block, out: &mut Vec<String>) -> Result<(), String> {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let(l) => {
                if l.reactive {
                    return Err(
                        "'reactive' no aplica en WASM (es del modelo de cliente)".to_string()
                    );
                }
                if let Some(t) = &l.ty {
                    check_i32(t)?;
                }
                out.push(l.name.clone());
            }
            Stmt::Expr(e) => collect_locals_expr(e, out)?,
            Stmt::Return { .. } => {}
        }
    }
    Ok(())
}

fn collect_locals_expr(e: &Expr, out: &mut Vec<String>) -> Result<(), String> {
    if let Expr::If {
        then_branch,
        else_branch,
        ..
    } = e
    {
        collect_locals(then_branch, out)?;
        if let Some(eb) = else_branch {
            match eb.as_ref() {
                ElseBranch::Block(b) => collect_locals(b, out)?,
                ElseBranch::If(inner) => collect_locals_expr(inner, out)?,
            }
        }
    }
    Ok(())
}

fn emit_block(block: &Block, indent: usize) -> Result<String, String> {
    block
        .stmts
        .iter()
        .map(|s| emit_stmt(s, indent))
        .collect::<Result<Vec<_>, String>>()
        .map(|parts| parts.join("\n"))
}

fn emit_stmt(stmt: &Stmt, indent: usize) -> Result<String, String> {
    let p = pad(indent);
    match stmt {
        Stmt::Let(l) => Ok(format!("{p}(local.set ${} {})", l.name, emit_expr(&l.value)?)),
        Stmt::Return { value, .. } => match value {
            Some(v) => Ok(format!("{p}(return {})", emit_expr(v)?)),
            None => Ok(format!("{p}(return)")),
        },
        Stmt::Expr(Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        }) => {
            let mut s = format!(
                "{p}(if {}\n{p}  (then\n{}\n{p}  )",
                emit_expr(cond)?,
                emit_block(then_branch, indent + 2)?
            );
            if let Some(eb) = else_branch {
                let inner = match eb.as_ref() {
                    ElseBranch::Block(b) => emit_block(b, indent + 2)?,
                    ElseBranch::If(if_expr) => {
                        emit_stmt(&Stmt::Expr(if_expr.clone()), indent + 2)?
                    }
                };
                s.push_str(&format!("\n{p}  (else\n{inner}\n{p}  )"));
            }
            s.push_str(&format!("\n{p})"));
            Ok(s)
        }
        Stmt::Expr(Expr::Match { .. }) => {
            Err("WASM aún no soporta 'match' (llega con las uniones reales)".to_string())
        }
        Stmt::Expr(_) => Err(
            "WASM puro no tiene efectos: una función debe usar 'let', 'return' e 'if', \
             no expresiones sueltas"
                .to_string(),
        ),
    }
}

/// Emite una expresión en forma plegada (s-expr) de WAT, dejando un `i32` en
/// la pila.
fn emit_expr(e: &Expr) -> Result<String, String> {
    match e {
        Expr::Int { value, .. } => Ok(format!("(i32.const {value})")),
        Expr::Bool { value, .. } => Ok(format!("(i32.const {})", if *value { 1 } else { 0 })),
        Expr::Ident { name, .. } => Ok(format!("(local.get ${name})")),
        Expr::Unary { op, expr, .. } => {
            let inner = emit_expr(expr)?;
            Ok(match op {
                UnaryOp::Neg => format!("(i32.sub (i32.const 0) {inner})"),
                UnaryOp::Not => format!("(i32.eqz {inner})"),
            })
        }
        Expr::Binary {
            op, left, right, ..
        } => Ok(format!(
            "({} {} {})",
            wasm_binop(*op),
            emit_expr(left)?,
            emit_expr(right)?
        )),
        Expr::Call { callee, args, .. } => {
            let name = match callee.as_ref() {
                Expr::Ident { name, .. } => name,
                _ => return Err("WASM sólo soporta llamadas a funciones por nombre".to_string()),
            };
            let parts = args
                .iter()
                .map(emit_expr)
                .collect::<Result<Vec<_>, String>>()?;
            if parts.is_empty() {
                Ok(format!("(call ${name})"))
            } else {
                Ok(format!("(call ${name} {})", parts.join(" ")))
            }
        }
        Expr::Float { .. } => Err("WASM aún no soporta flotantes (sólo i32 por ahora)".to_string()),
        Expr::Str { .. } => {
            Err("WASM aún no soporta cadenas (requieren memoria lineal)".to_string())
        }
        Expr::Member { .. } => Err("WASM aún no soporta acceso a miembros".to_string()),
        Expr::If { .. } => {
            Err("WASM aún no soporta 'if' en posición de expresión".to_string())
        }
        Expr::Match { .. } => Err("WASM aún no soporta 'match'".to_string()),
    }
}

fn wasm_binop(op: BinOp) -> &'static str {
    match op {
        BinOp::Add => "i32.add",
        BinOp::Sub => "i32.sub",
        BinOp::Mul => "i32.mul",
        BinOp::Div => "i32.div_s",
        BinOp::Rem => "i32.rem_s",
        BinOp::Eq => "i32.eq",
        BinOp::Ne => "i32.ne",
        BinOp::Lt => "i32.lt_s",
        BinOp::Gt => "i32.gt_s",
        BinOp::Le => "i32.le_s",
        BinOp::Ge => "i32.ge_s",
        // Sin cortocircuito; correcto sobre booleanos 0/1.
        BinOp::And => "i32.and",
        BinOp::Or => "i32.or",
    }
}

fn check_i32(t: &Type) -> Result<(), String> {
    match t {
        Type::Name { name, .. } if name == "Int" || name == "Bool" => Ok(()),
        Type::Name { name, .. } => Err(format!(
            "el backend WASM sólo soporta Int/Bool por ahora, no '{name}'"
        )),
        Type::Union { .. } => Err("el backend WASM aún no soporta tipos unión".to_string()),
    }
}

fn pad(n: usize) -> String {
    "  ".repeat(n)
}
