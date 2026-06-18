//! `marea-wasm` — backend de Marea a WebAssembly (formato texto WAT).
//!
//! Segundo destino enchufado al mismo AST: el lenguaje no cambia, sólo el
//! codegen. Así la lógica corre como WebAssembly en el navegador, recortando la
//! dependencia del JS-como-lenguaje a un pegamento mínimo.
//!
//! Soportado hoy:
//!   - Enteros y booleanos (`i32`): `let`, `return`, `if`, aritmética/lógica,
//!     comparación, llamadas y recursión.
//!   - Cadenas (`String`) sobre **memoria lineal**: un string es un puntero
//!     `i32` a un registro `[longitud:i32][bytes UTF-8...]`. Los literales van
//!     en el *data section*; `concat` es un builtin en WAT (allocador bump +
//!     `memory.copy`).
//!
//! Aún no soportado (error claro, no WAT roto): flotantes, `match`, acceso a
//! miembros, tipos unión, `reactive`.

use marea_syntax::ast::*;
use std::collections::{HashMap, HashSet};

/// Transpila un módulo a un `(module ...)` de WAT, o un error legible.
pub fn emit_wat(module: &Module) -> Result<String, String> {
    let strings = build_strings(module);
    let needs_mem = !strings.data.is_empty() || uses_string_types(module);

    let mut funcs = Vec::new();
    for item in &module.items {
        if let Item::Fn(f) = item {
            let wat = emit_func(f, &strings).map_err(|e| format!("en la función '{}': {}", f.name, e))?;
            funcs.push(wat);
        }
    }
    if funcs.is_empty() {
        return Err("no hay funciones que compilar a WASM".to_string());
    }

    let prelude = if needs_mem {
        emit_string_runtime(&strings)
    } else {
        String::new()
    };

    Ok(format!("(module\n{prelude}{}\n)\n", funcs.join("\n")))
}

// --- tabla de cadenas y runtime de memoria ---

/// Literales de cadena ubicados en memoria lineal con su offset asignado.
struct Strings {
    /// texto del literal -> offset de su registro en memoria.
    map: HashMap<String, i32>,
    /// (offset, bytes del registro = longitud LE + contenido UTF-8).
    data: Vec<(i32, Vec<u8>)>,
    /// dónde empieza el heap (justo después de los literales estáticos).
    heap_start: i32,
}

impl Strings {
    fn offset_of(&self, text: &str) -> i32 {
        self.map.get(text).copied().unwrap_or(0)
    }
}

fn build_strings(module: &Module) -> Strings {
    let mut texts = Vec::new();
    let mut seen = HashSet::new();
    for item in &module.items {
        if let Item::Fn(f) = item {
            collect_strings_block(&f.body, &mut texts, &mut seen);
        }
    }

    let mut map = HashMap::new();
    let mut data = Vec::new();
    let mut offset: i32 = 0;
    for text in texts {
        let bytes = text.as_bytes();
        let len = bytes.len() as i32;
        let mut record = Vec::with_capacity(4 + bytes.len());
        record.extend_from_slice(&len.to_le_bytes());
        record.extend_from_slice(bytes);
        let record_len = record.len() as i32;
        map.insert(text, offset);
        data.push((offset, record));
        offset += record_len;
    }

    Strings {
        map,
        data,
        heap_start: offset,
    }
}

fn uses_string_types(module: &Module) -> bool {
    module.items.iter().any(|item| {
        if let Item::Fn(f) = item {
            let in_params = f.params.iter().any(|p| is_string_type(&p.ty));
            let in_ret = f.return_type.as_ref().is_some_and(is_string_type);
            in_params || in_ret
        } else {
            false
        }
    })
}

fn is_string_type(t: &Type) -> bool {
    matches!(t, Type::Name { name, .. } if name == "String")
}

/// Emite memoria, literales, allocador bump y el builtin `concat`.
fn emit_string_runtime(strings: &Strings) -> String {
    let mut data = String::new();
    for (offset, bytes) in &strings.data {
        let escaped: String = bytes.iter().map(|b| format!("\\{b:02x}")).collect();
        data.push_str(&format!("  (data (i32.const {offset}) \"{escaped}\")\n"));
    }

    format!(
        "  (memory (export \"memory\") 1)\n\
           (global $__heap (mut i32) (i32.const {heap}))\n\
         {data}\
         {ALLOC}\n\
         {CONCAT}\n",
        heap = strings.heap_start,
    )
}

/// Allocador bump: avanza el puntero `$__heap` y devuelve el bloque anterior.
const ALLOC: &str = "  (func $__alloc (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $__heap))
    (global.set $__heap (i32.add (global.get $__heap) (local.get $n)))
    (local.get $p)
  )";

/// `concat(a, b)`: ubica un registro nuevo y copia ambos contenidos.
const CONCAT: &str = "  (func $concat (export \"concat\") (param $a i32) (param $b i32) (result i32)
    (local $la i32) (local $lb i32) (local $p i32)
    (local.set $la (i32.load (local.get $a)))
    (local.set $lb (i32.load (local.get $b)))
    (local.set $p (call $__alloc (i32.add (i32.add (local.get $la) (local.get $lb)) (i32.const 4))))
    (i32.store (local.get $p) (i32.add (local.get $la) (local.get $lb)))
    (memory.copy (i32.add (local.get $p) (i32.const 4)) (i32.add (local.get $a) (i32.const 4)) (local.get $la))
    (memory.copy (i32.add (i32.add (local.get $p) (i32.const 4)) (local.get $la)) (i32.add (local.get $b) (i32.const 4)) (local.get $lb))
    (local.get $p)
  )";

// --- funciones ---

fn emit_func(f: &FnDecl, strings: &Strings) -> Result<String, String> {
    let params = f
        .params
        .iter()
        .map(|p| {
            check_value_type(&p.ty)?;
            Ok(format!(" (param ${} i32)", p.name))
        })
        .collect::<Result<Vec<_>, String>>()?
        .join("");

    let result = match &f.return_type {
        Some(t) => {
            check_value_type(t)?;
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

    let body = emit_block(&f.body, 2, strings)?;

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
                    check_value_type(t)?;
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

fn emit_block(block: &Block, indent: usize, strings: &Strings) -> Result<String, String> {
    block
        .stmts
        .iter()
        .map(|s| emit_stmt(s, indent, strings))
        .collect::<Result<Vec<_>, String>>()
        .map(|parts| parts.join("\n"))
}

fn emit_stmt(stmt: &Stmt, indent: usize, strings: &Strings) -> Result<String, String> {
    let p = pad(indent);
    match stmt {
        Stmt::Let(l) => Ok(format!(
            "{p}(local.set ${} {})",
            l.name,
            emit_expr(&l.value, strings)?
        )),
        Stmt::Return { value, .. } => match value {
            Some(v) => Ok(format!("{p}(return {})", emit_expr(v, strings)?)),
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
                emit_expr(cond, strings)?,
                emit_block(then_branch, indent + 2, strings)?
            );
            if let Some(eb) = else_branch {
                let inner = match eb.as_ref() {
                    ElseBranch::Block(b) => emit_block(b, indent + 2, strings)?,
                    ElseBranch::If(if_expr) => {
                        emit_stmt(&Stmt::Expr(if_expr.clone()), indent + 2, strings)?
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
/// la pila (un valor entero, o un puntero a memoria si es String).
fn emit_expr(e: &Expr, strings: &Strings) -> Result<String, String> {
    match e {
        Expr::Int { value, .. } => Ok(format!("(i32.const {value})")),
        Expr::Bool { value, .. } => Ok(format!("(i32.const {})", if *value { 1 } else { 0 })),
        // Un literal de cadena es el puntero a su registro estático en memoria.
        Expr::Str { value, .. } => Ok(format!("(i32.const {})", strings.offset_of(value))),
        Expr::Ident { name, .. } => Ok(format!("(local.get ${name})")),
        Expr::Unary { op, expr, .. } => {
            let inner = emit_expr(expr, strings)?;
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
            emit_expr(left, strings)?,
            emit_expr(right, strings)?
        )),
        Expr::Call { callee, args, .. } => {
            let name = match callee.as_ref() {
                Expr::Ident { name, .. } => name,
                _ => return Err("WASM sólo soporta llamadas a funciones por nombre".to_string()),
            };
            let parts = args
                .iter()
                .map(|a| emit_expr(a, strings))
                .collect::<Result<Vec<_>, String>>()?;
            if parts.is_empty() {
                Ok(format!("(call ${name})"))
            } else {
                Ok(format!("(call ${name} {})", parts.join(" ")))
            }
        }
        Expr::Float { .. } => Err("WASM aún no soporta flotantes (sólo i32 por ahora)".to_string()),
        Expr::Member { .. } => Err("WASM aún no soporta acceso a miembros".to_string()),
        Expr::If { .. } => {
            Err("WASM aún no soporta 'if' en posición de expresión".to_string())
        }
        Expr::Match { .. } => Err("WASM aún no soporta 'match'".to_string()),
        Expr::Record { .. } => {
            Err("WASM: registros en construcción (Paso 1-WASM)".to_string())
        }
        Expr::List { .. } => {
            Err("WASM aún no soporta listas (requieren memoria lineal)".to_string())
        }
    }
}

// --- recolección de literales de cadena ---

fn collect_strings_block(block: &Block, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let(l) => collect_strings_expr(&l.value, out, seen),
            Stmt::Return { value: Some(v), .. } => collect_strings_expr(v, out, seen),
            Stmt::Return { .. } => {}
            Stmt::Expr(e) => collect_strings_expr(e, out, seen),
        }
    }
}

fn collect_strings_expr(e: &Expr, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    match e {
        Expr::Str { value, .. } => {
            if seen.insert(value.clone()) {
                out.push(value.clone());
            }
        }
        Expr::Unary { expr, .. } => collect_strings_expr(expr, out, seen),
        Expr::Binary { left, right, .. } => {
            collect_strings_expr(left, out, seen);
            collect_strings_expr(right, out, seen);
        }
        Expr::Call { callee, args, .. } => {
            collect_strings_expr(callee, out, seen);
            for a in args {
                collect_strings_expr(a, out, seen);
            }
        }
        Expr::Member { object, .. } => collect_strings_expr(object, out, seen),
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            collect_strings_expr(cond, out, seen);
            collect_strings_block(then_branch, out, seen);
            if let Some(eb) = else_branch {
                match eb.as_ref() {
                    ElseBranch::Block(b) => collect_strings_block(b, out, seen),
                    ElseBranch::If(inner) => collect_strings_expr(inner, out, seen),
                }
            }
        }
        Expr::Match { scrutinee, arms, .. } => {
            collect_strings_expr(scrutinee, out, seen);
            for arm in arms {
                collect_strings_expr(&arm.body, out, seen);
            }
        }
        // Recursar en registros y listas: sus literales String DEBEN quedar en
        // el data section o 'offset_of' devolvería basura.
        Expr::Record { fields, .. } => {
            for f in fields {
                collect_strings_expr(&f.value, out, seen);
            }
        }
        Expr::List { elements, .. } => {
            for el in elements {
                collect_strings_expr(el, out, seen);
            }
        }
        Expr::Int { .. } | Expr::Float { .. } | Expr::Bool { .. } | Expr::Ident { .. } => {}
    }
}

// --- utilidades ---

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

fn check_value_type(t: &Type) -> Result<(), String> {
    match t {
        Type::Name { name, .. } if name == "Int" || name == "Bool" || name == "String" => Ok(()),
        Type::Name { name, .. } => Err(format!(
            "el backend WASM soporta Int/Bool/String por ahora, no '{name}'"
        )),
        Type::Union { .. } => Err("el backend WASM aún no soporta tipos unión".to_string()),
        Type::Record { .. } => {
            Err("el backend WASM aún no soporta tipos registro inline".to_string())
        }
    }
}

fn pad(n: usize) -> String {
    "  ".repeat(n)
}
