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
//!   - Registros (structs) sobre **memoria lineal**: un registro es un puntero
//!     `i32` a `N` campos contiguos de 4 bytes; el campo `i` (en orden de
//!     declaración) vive en `offset 4*i`. Un campo `String` es a su vez un
//!     puntero `i32`, así que también cabe en 4 bytes. La construcción usa el
//!     allocador bump; el acceso `x.campo` es un `i32.load offset=4*i`.
//!
//! Aún no soportado (error claro, no WAT roto): flotantes, `match`, tipos
//! unión, `reactive`, listas.

use marea_syntax::ast::*;
use std::collections::{HashMap, HashSet};

/// Disposición en memoria de un registro declarado.
///
/// Cada campo ocupa 4 bytes; el `offset` del campo `i` (orden de declaración)
/// es `4*i`; el `size` total es `4 * fields.len()`.
#[derive(Clone)]
struct StructLayout {
    /// (nombre del campo, nombre del tipo del campo) en orden de declaración.
    fields: Vec<(String, String)>,
}

impl StructLayout {
    fn size(&self) -> i32 {
        4 * self.fields.len() as i32
    }

    /// Índice de declaración del campo, o `None` si no existe.
    fn index_of(&self, field: &str) -> Option<usize> {
        self.fields.iter().position(|(n, _)| n == field)
    }

    /// Nombre del tipo de un campo.
    fn type_of(&self, field: &str) -> Option<&str> {
        self.fields
            .iter()
            .find(|(n, _)| n == field)
            .map(|(_, t)| t.as_str())
    }
}

/// Contexto que viaja por el codegen para resolver offsets y tipos estáticos.
struct Ctx<'a> {
    strings: &'a Strings,
    /// nombre de tipo registro -> su disposición en memoria.
    layouts: &'a HashMap<String, StructLayout>,
    /// nombre de variable -> nombre de su tipo (para resolver `x.campo`).
    vars: HashMap<String, String>,
    /// contador de locales temporales para construir registros.
    rec_counter: usize,
}

/// Transpila un módulo a un `(module ...)` de WAT, o un error legible.
pub fn emit_wat(module: &Module) -> Result<String, String> {
    let strings = build_strings(module);
    let layouts = build_layouts(module)?;
    // El runtime de memoria (con `$__alloc`) hace falta si hay cadenas, registros
    // o construcción de listas (todas reservan en el heap).
    let needs_mem = !strings.data.is_empty()
        || uses_string_types(module)
        || !layouts.is_empty()
        || constructs_list(module);

    let mut funcs = Vec::new();
    for item in &module.items {
        if let Item::Fn(f) = item {
            let wat = emit_func(f, &strings, &layouts)
                .map_err(|e| format!("en la función '{}': {}", f.name, e))?;
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

/// Construye las disposiciones de los registros declarados como `type T = {...}`.
///
/// Sólo recorre `Item::Type` cuyo `aliased` es `Type::Record`. Cada campo se
/// reduce a (nombre, nombre del tipo); un campo registro o String encaja en 4
/// bytes (es un puntero). Rechaza campos de tipo flotante o estructuras sin
/// soporte con mensaje claro.
fn build_layouts(module: &Module) -> Result<HashMap<String, StructLayout>, String> {
    let mut layouts = HashMap::new();
    for item in &module.items {
        if let Item::Type(decl) = item {
            if let Type::Record { fields, .. } = &decl.aliased {
                let mut cols = Vec::with_capacity(fields.len());
                for fd in fields {
                    let ty_name = field_type_name(&fd.ty).map_err(|e| {
                        format!("en el tipo '{}', campo '{}': {}", decl.name, fd.name, e)
                    })?;
                    cols.push((fd.name.clone(), ty_name));
                }
                layouts.insert(
                    decl.name.clone(),
                    StructLayout { fields: cols },
                );
            }
        }
    }
    Ok(layouts)
}

/// Nombre del tipo de un campo de registro, validando que sea representable en
/// 4 bytes. `Float` se rechaza; uniones y registros inline también.
fn field_type_name(t: &Type) -> Result<String, String> {
    match t {
        Type::Name { name, .. } if name == "Float" => {
            Err("el campo es 'Float' y el backend WASM aún no soporta flotantes".to_string())
        }
        Type::Name { name, .. } => Ok(name.clone()),
        Type::Union { .. } => {
            Err("el backend WASM aún no soporta campos de tipo unión".to_string())
        }
        Type::Record { .. } => {
            Err("el backend WASM aún no soporta registros inline como campo".to_string())
        }
    }
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

    // Alinea el inicio del heap a múltiplo de 4: los campos de los registros se
    // leen con `i32.load`, que exige direcciones alineadas; si el heap arranca
    // tras literales String de longitud arbitraria, quedaría desalineado.
    let heap_start = (offset + 3) & !3;

    Strings {
        map,
        data,
        heap_start,
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

/// ¿El módulo TOCA el heap en algún punto? — construir un registro/lista, o
/// ACCEDER a memoria (indexar `xs[i]` / leer un campo `r.c`), o recibir/devolver
/// un valor de tipo puntero (String/List/registro). Cualquiera exige emitir la
/// memoria lineal con el allocador.
fn constructs_list(module: &Module) -> bool {
    fn in_block(b: &Block) -> bool {
        b.stmts.iter().any(|s| match s {
            Stmt::Let(l) => in_expr(&l.value),
            Stmt::Assign { value, .. } => in_expr(value),
            Stmt::Effect { body, .. } => in_block(body),
            Stmt::Return { value: Some(v), .. } => in_expr(v),
            Stmt::Return { .. } => false,
            Stmt::Expr(e) => in_expr(e),
        })
    }
    fn in_expr(e: &Expr) -> bool {
        match e {
            Expr::List { .. } => true,
            // Acceder a memoria también la requiere (i32.load), aunque no se
            // construya nada en esta función (p.ej. indexar un parámetro lista).
            Expr::Index { .. } | Expr::Member { .. } => true,
            Expr::Unary { expr, .. } => in_expr(expr),
            Expr::Binary { left, right, .. } => in_expr(left) || in_expr(right),
            Expr::Call { args, .. } => args.iter().any(in_expr),
            Expr::Record { fields, .. } => fields.iter().any(|f| in_expr(&f.value)),
            Expr::If {
                cond,
                then_branch,
                else_branch,
                ..
            } => {
                in_expr(cond)
                    || in_block(then_branch)
                    || match else_branch {
                        Some(eb) => match eb.as_ref() {
                            ElseBranch::Block(b) => in_block(b),
                            ElseBranch::If(e) => in_expr(e),
                        },
                        None => false,
                    }
            }
            Expr::Match { scrutinee, arms, .. } => {
                in_expr(scrutinee) || arms.iter().any(|a| in_expr(&a.body))
            }
            _ => false,
        }
    }
    module
        .items
        .iter()
        .any(|item| matches!(item, Item::Fn(f) if in_block(&f.body)))
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
         {INDEX}\n\
         {STREQ}\n\
         {CONCAT}\n",
        heap = strings.heap_start,
    )
}

/// Allocador bump: avanza el puntero `$__heap` y devuelve el bloque anterior.
/// Redondea el tamaño solicitado a múltiplo de 4 ((n+3) & ~3) para que TODA
/// asignación quede alineada: así un registro reservado tras una cadena de
/// longitud arbitraria sigue leyéndose con `i32.load` alineado.
const ALLOC: &str = "  (func $__alloc (param $n i32) (result i32)
    (local $p i32)
    (local.set $p (global.get $__heap))
    (global.set $__heap
      (i32.add (global.get $__heap)
        (i32.and (i32.add (local.get $n) (i32.const 3)) (i32.const -4))))
    (local.get $p)
  )";

/// `xs[i]` con comprobación de rango. Sin ella, el índice se sumaba al puntero
/// sin más: un índice negativo leía hacia atrás en el heap —`xs[-1]` devolvía la
/// longitud, `xs[-2]` el último elemento de la estructura anterior— y uno
/// grande, ceros o basura. Es divulgación de memoria, y además difería del
/// backend de TypeScript, que lanza. Aquí se trapea, que es lo que WebAssembly
/// puede hacer.
const INDEX: &str = "  (func $__index (param $xs i32) (param $i i32) (result i32)
    (if (i32.or
          (i32.lt_s (local.get $i) (i32.const 0))
          (i32.ge_s (local.get $i) (i32.load (local.get $xs))))
      (then unreachable))
    (i32.load (i32.add (local.get $xs)
      (i32.mul (i32.add (local.get $i) (i32.const 1)) (i32.const 4))))
  )";

/// Igualdad de CADENAS por contenido. `i32.eq` comparaba los punteros, así que
/// `concat("a","") == "a"` era falso en WASM y verdadero en TypeScript: el mismo
/// programa daba dos respuestas. Sólo funcionaba por accidente entre literales,
/// porque el recolector los deduplica —lo que hacía la divergencia más difícil
/// de ver, no menos real—.
const STREQ: &str = "  (func $__streq (param $a i32) (param $b i32) (result i32)
    (local $n i32) (local $i i32)
    (if (i32.eq (local.get $a) (local.get $b)) (then (return (i32.const 1))))
    (local.set $n (i32.load (local.get $a)))
    (if (i32.ne (local.get $n) (i32.load (local.get $b))) (then (return (i32.const 0))))
    (local.set $i (i32.const 0))
    (block $fin
      (loop $sig
        (br_if $fin (i32.ge_u (local.get $i) (local.get $n)))
        (if (i32.ne
              (i32.load8_u (i32.add (i32.add (local.get $a) (i32.const 4)) (local.get $i)))
              (i32.load8_u (i32.add (i32.add (local.get $b) (i32.const 4)) (local.get $i))))
          (then (return (i32.const 0))))
        (local.set $i (i32.add (local.get $i) (i32.const 1)))
        (br $sig)))
    (i32.const 1)
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

/// Nombres de export reservados por el runtime/prelude WASM; una función no
/// puede usarlos o produciría un export duplicado y WAT inválido.
const RESERVED_EXPORTS: &[&str] = &["memory", "concat"];

fn emit_func(
    f: &FnDecl,
    strings: &Strings,
    layouts: &HashMap<String, StructLayout>,
) -> Result<String, String> {
    if RESERVED_EXPORTS.contains(&f.name.as_str()) {
        return Err(format!(
            "el nombre '{}' está reservado por el runtime WASM; renómbrala",
            f.name
        ));
    }
    let params = f
        .params
        .iter()
        .map(|p| {
            check_value_type(&p.ty, layouts)?;
            Ok(format!(" (param ${} i32)", p.name))
        })
        .collect::<Result<Vec<_>, String>>()?
        .join("");

    let result = match &f.return_type {
        Some(t) => {
            check_value_type(t, layouts)?;
            " (result i32)".to_string()
        }
        None => String::new(),
    };

    // Los temporales de registro se nombran de forma determinista por un
    // contador; collect_locals debe usar el MISMO orden de recorrido que la
    // emisión para que `$__recN` coincida con su declaración.
    let mut locals = Vec::new();
    let mut rec_counter = 0usize;
    collect_locals(&f.body, &mut locals, &mut rec_counter)?;
    let locals_decl: String = locals
        .iter()
        .map(|n| format!("\n    (local ${n} i32)"))
        .collect();

    // Semilla del contexto: las variables conocidas arrancan con los params de
    // tipo registro (Type::Name cuyo nombre está en layouts).
    let mut vars = HashMap::new();
    for p in &f.params {
        if let Type::Name { name, .. } = &p.ty {
            if layouts.contains_key(name) {
                vars.insert(p.name.clone(), name.clone());
            }
        }
    }
    let mut ctx = Ctx {
        strings,
        layouts,
        vars,
        rec_counter: 0,
    };

    let body = emit_block(&f.body, 2, &mut ctx)?;

    // Si la función declara un resultado pero el último statement no es un
    // `return` plano (p.ej. termina en un `if/else` con return en ambas ramas),
    // el validador WASM no deduce que la caída es inalcanzable y exigiría un i32.
    // Un `(unreachable)` final lo marca como inalcanzable y produce WAT válido.
    let needs_unreachable = f.return_type.is_some()
        && !matches!(f.body.stmts.last(), Some(Stmt::Return { .. }));
    let tail = if needs_unreachable {
        "\n    (unreachable)"
    } else {
        ""
    };

    Ok(format!(
        "  (func ${name} (export \"{name}\"){params}{result}{locals_decl}\n{body}{tail}\n  )",
        name = f.name,
    ))
}

/// Recoge los nombres de los locales a declarar al inicio de la función:
///   - variables `let` (incluidas las de ramas `if`),
///   - temporales `$__recN` para construir literales de registro.
///
/// El recorrido debe ser idéntico al de la emisión para que la numeración de
/// `$__recN` cuadre con su uso.
fn collect_locals(
    block: &Block,
    out: &mut Vec<String>,
    rec_counter: &mut usize,
) -> Result<(), String> {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let(l) => {
                if l.reactive {
                    return Err(
                        "'reactive' no aplica en WASM (es del modelo de cliente)".to_string()
                    );
                }
                if let Some(t) = &l.ty {
                    // No validamos aquí structs como tipo de `let` para no
                    // rechazar `let p: Punto = ...`; check_value_type lo acepta.
                    check_let_type(t)?;
                }
                collect_locals_expr(&l.value, out, rec_counter)?;
                out.push(l.name.clone());
            }
            Stmt::Assign { value, .. } => collect_locals_expr(value, out, rec_counter)?,
            Stmt::Effect { body, .. } => collect_locals(body, out, rec_counter)?,
            Stmt::Return { value: Some(v), .. } => collect_locals_expr(v, out, rec_counter)?,
            Stmt::Return { .. } => {}
            Stmt::Expr(e) => collect_locals_expr(e, out, rec_counter)?,
        }
    }
    Ok(())
}

/// Valida tipos anotados en `let`: Int/Bool/String pasan; los demás se difieren
/// a check_value_type en su contexto (que conoce los layouts).
fn check_let_type(t: &Type) -> Result<(), String> {
    match t {
        Type::Name { .. } => Ok(()),
        Type::Union { .. } => Err("el backend WASM aún no soporta tipos unión".to_string()),
        Type::Record { .. } => {
            Err("el backend WASM aún no soporta registros inline".to_string())
        }
    }
}

fn collect_locals_expr(
    e: &Expr,
    out: &mut Vec<String>,
    rec_counter: &mut usize,
) -> Result<(), String> {
    match e {
        Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        } => {
            // La condición se emite ANTES de las ramas (emit_stmt); si construye
            // un registro/lista, su temporal $__recN debe contarse aquí primero.
            collect_locals_expr(cond, out, rec_counter)?;
            collect_locals(then_branch, out, rec_counter)?;
            if let Some(eb) = else_branch {
                match eb.as_ref() {
                    ElseBranch::Block(b) => collect_locals(b, out, rec_counter)?,
                    ElseBranch::If(inner) => collect_locals_expr(inner, out, rec_counter)?,
                }
            }
        }
        Expr::Record { fields, .. } => {
            // Primero recursa en los valores de los campos (pueden anidar más
            // registros), luego reserva el temporal de ESTE registro. El orden
            // debe replicar el de emit_expr.
            for fi in fields {
                collect_locals_expr(&fi.value, out, rec_counter)?;
            }
            let temp = format!("__rec{}", *rec_counter);
            *rec_counter += 1;
            out.push(temp);
        }
        Expr::Unary { expr, .. } => collect_locals_expr(expr, out, rec_counter)?,
        Expr::Binary { left, right, .. } => {
            collect_locals_expr(left, out, rec_counter)?;
            collect_locals_expr(right, out, rec_counter)?;
        }
        Expr::Call { args, .. } => {
            for a in args {
                collect_locals_expr(a, out, rec_counter)?;
            }
        }
        Expr::Member { object, .. } => collect_locals_expr(object, out, rec_counter)?,
        Expr::List { elements, .. } => {
            // Primero recursa en los elementos, luego reserva el temporal de
            // ESTA lista — replicando el orden de emit_list.
            for el in elements {
                collect_locals_expr(el, out, rec_counter)?;
            }
            let temp = format!("__rec{}", *rec_counter);
            *rec_counter += 1;
            out.push(temp);
        }
        Expr::Index { object, index, .. } => {
            collect_locals_expr(object, out, rec_counter)?;
            collect_locals_expr(index, out, rec_counter)?;
        }
        _ => {}
    }
    Ok(())
}

fn emit_block(block: &Block, indent: usize, ctx: &mut Ctx) -> Result<String, String> {
    block
        .stmts
        .iter()
        .map(|s| emit_stmt(s, indent, ctx))
        .collect::<Result<Vec<_>, String>>()
        .map(|parts| parts.join("\n"))
}

fn emit_stmt(stmt: &Stmt, indent: usize, ctx: &mut Ctx) -> Result<String, String> {
    let p = pad(indent);
    match stmt {
        Stmt::Let(l) => {
            // Registra el tipo estático de la variable para resolver `x.campo`:
            // por anotación `let x: T = ..` o por el literal `Expr::Record`.
            if let Some(name) = let_type_name(l, ctx) {
                ctx.vars.insert(l.name.clone(), name);
            }
            let value = emit_expr(&l.value, ctx)?;
            Ok(format!("{p}(local.set ${} {})", l.name, value))
        }
        // Asignación a un local existente. (La reactividad es del cliente; en
        // WASM puro un `assign` es simplemente reescribir el local.)
        Stmt::Assign { name, value, .. } => {
            let v = emit_expr(value, ctx)?;
            Ok(format!("{p}(local.set ${name} {v})"))
        }
        Stmt::Effect { .. } => {
            Err("WASM no soporta 'effect' (la reactividad vive en el cliente/TS)".to_string())
        }
        Stmt::Return { value, .. } => match value {
            Some(v) => Ok(format!("{p}(return {})", emit_expr(v, ctx)?)),
            None => Ok(format!("{p}(return)")),
        },
        Stmt::Expr(Expr::If {
            cond,
            then_branch,
            else_branch,
            ..
        }) => {
            let cond = emit_expr(cond, ctx)?;
            let then = emit_block(then_branch, indent + 2, ctx)?;
            let mut s = format!("{p}(if {cond}\n{p}  (then\n{then}\n{p}  )");
            if let Some(eb) = else_branch {
                let inner = match eb.as_ref() {
                    ElseBranch::Block(b) => emit_block(b, indent + 2, ctx)?,
                    ElseBranch::If(if_expr) => {
                        emit_stmt(&Stmt::Expr(if_expr.clone()), indent + 2, ctx)?
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

/// Tipo estático de la variable de un `let`, si es un registro conocido:
/// por anotación `T` o por el `type_name` de un literal de registro.
fn let_type_name(l: &LetStmt, ctx: &Ctx) -> Option<String> {
    if let Some(Type::Name { name, .. }) = &l.ty {
        if ctx.layouts.contains_key(name) {
            return Some(name.clone());
        }
    }
    if let Expr::Record {
        type_name: Some(name),
        ..
    } = &l.value
    {
        if ctx.layouts.contains_key(name) {
            return Some(name.clone());
        }
    }
    None
}

/// Emite una expresión en forma plegada (s-expr) de WAT, dejando un `i32` en
/// la pila (un valor entero, o un puntero a memoria si es String o registro).
fn emit_expr(e: &Expr, ctx: &mut Ctx) -> Result<String, String> {
    match e {
        Expr::Int { value, .. } => {
            // El backend WASM usa i32; un literal fuera de rango produciría WAT
            // que wat2wasm rechaza. Mejor un error claro.
            if *value < i64::from(i32::MIN) || *value > i64::from(i32::MAX) {
                return Err(format!(
                    "el entero {value} no cabe en i32 (rango del backend WASM por ahora)"
                ));
            }
            Ok(format!("(i32.const {value})"))
        }
        Expr::Bool { value, .. } => Ok(format!("(i32.const {})", if *value { 1 } else { 0 })),
        // Un literal de cadena es el puntero a su registro estático en memoria.
        Expr::Str { value, .. } => Ok(format!("(i32.const {})", ctx.strings.offset_of(value))),
        Expr::Ident { name, .. } => Ok(format!("(local.get ${name})")),
        Expr::Unary { op, expr, .. } => {
            let inner = emit_expr(expr, ctx)?;
            Ok(match op {
                UnaryOp::Neg => format!("(i32.sub (i32.const 0) {inner})"),
                UnaryOp::Not => format!("(i32.eqz {inner})"),
            })
        }
        Expr::Binary {
            op, left, right, ..
        } => {
            let l = emit_expr(left, ctx)?;
            let r = emit_expr(right, ctx)?;
            // `&&` y `||` CORTOCIRCUITAN, como en el backend de TypeScript.
            // Emitirlos como `i32.and`/`i32.or` evaluaba siempre los dos lados:
            // el modismo defensivo `i < len(xs) && xs[i] > 0` ejecutaba el
            // indexado igualmente, y con un operando que trapea (una división
            // por cero, por ejemplo) el programa moría donde en TS no.
            // La igualdad de CADENAS compara contenido, no punteros.
            if matches!(op, BinOp::Eq | BinOp::Ne)
                && (es_cadena(left, ctx) || es_cadena(right, ctx))
            {
                let cmp = format!("(call $__streq {l} {r})");
                return Ok(match op {
                    BinOp::Eq => cmp,
                    _ => format!("(i32.eqz {cmp})"),
                });
            }
            match op {
                BinOp::And => Ok(format!("(if (result i32) {l} (then {r}) (else (i32.const 0)))")),
                BinOp::Or => Ok(format!("(if (result i32) {l} (then (i32.const 1)) (else {r}))")),
                _ => Ok(format!("({} {l} {r})", wasm_binop(*op))),
            }
        }
        Expr::Call { callee, args, .. } => {
            let name = match callee.as_ref() {
                Expr::Ident { name, .. } => name,
                _ => return Err("WASM sólo soporta llamadas a funciones por nombre".to_string()),
            };
            let parts = args
                .iter()
                .map(|a| emit_expr(a, ctx))
                .collect::<Result<Vec<_>, String>>()?;
            // 'len(xs)' es un builtin: la longitud vive en la palabra 0 de la
            // lista, así que es un i32.load del puntero (no una llamada).
            if name == "len" && parts.len() == 1 {
                return Ok(format!("(i32.load {})", parts[0]));
            }
            // Builtins del runtime de TypeScript: no existen en WASM y
            // producirían WAT que ni siquiera ensambla ("undefined function
            // variable"). La lista debe cubrir TODOS los de
            // `marea_types::builtins::VALUE_NAMES` que WASM no implementa; antes
            // faltaban print/render/escapar y el error salía en wat2wasm, no en
            // el compilador, contra lo que promete el README.
            if matches!(
                name.as_str(),
                "guardar"
                    | "todos"
                    | "actualizar"
                    | "borrar"
                    | "aTexto"
                    | "print"
                    | "render"
                    | "escapar"
                    | "html"
                    | "unir"
                    | "agregar"
                    | "largo"
                    | "contiene"
                    | "minusculas"
                    | "pedir"
                    | "pedirPost"
                    | "jsonTexto"
                    | "jsonNumero"
                    | "jsonDecimal"
                    | "jsonLargo"
            ) {
                return Err(format!(
                    "'{name}' es un builtin del runtime de TypeScript; no existe en el backend WASM"
                ));
            }
            if parts.is_empty() {
                Ok(format!("(call ${name})"))
            } else {
                Ok(format!("(call ${name} {})", parts.join(" ")))
            }
        }
        Expr::Member { object, field, .. } => emit_member(object, field, ctx),
        Expr::Record {
            type_name,
            fields,
            ..
        } => emit_record(type_name.as_deref(), fields, ctx),
        Expr::List { elements, .. } => emit_list(elements, ctx),
        Expr::Index { object, index, .. } => {
            let obj = emit_expr(object, ctx)?;
            let idx = emit_expr(index, ctx)?;
            // dirección del elemento = ptr + (idx + 1) * 4  (la longitud ocupa
            // la palabra 0; los elementos empiezan en la palabra 1).
            Ok(format!(
                "(call $__index {obj} {idx})"
            ))
        }
        Expr::Float { .. } => Err("WASM aún no soporta flotantes (sólo i32 por ahora)".to_string()),
        Expr::If { .. } => {
            Err("WASM aún no soporta 'if' en posición de expresión".to_string())
        }
        Expr::Match { .. } => Err("WASM aún no soporta 'match'".to_string()),
    }
}

/// Construye una lista en memoria lineal: reserva `4*(N+1)` bytes, guarda la
/// longitud en la palabra 0 y cada elemento en la palabra `i+1`, y deja el
/// puntero. Mismo patrón temporal-luego-puntero que `emit_record`.
fn emit_list(elements: &[Expr], ctx: &mut Ctx) -> Result<String, String> {
    // Emite los valores ANTES de reservar el temporal (orden = collect_locals_expr).
    let emitted = elements
        .iter()
        .map(|el| emit_expr(el, ctx))
        .collect::<Result<Vec<_>, String>>()?;

    let n = elements.len();
    let temp = format!("__rec{}", ctx.rec_counter);
    ctx.rec_counter += 1;
    let size = 4 * (n as i32 + 1);

    let mut out = String::new();
    out.push_str(&format!(
        "(local.set ${temp} (call $__alloc (i32.const {size})))"
    ));
    out.push_str(&format!(
        " (i32.store offset=0 (local.get ${temp}) (i32.const {n}))"
    ));
    for (i, value) in emitted.iter().enumerate() {
        let offset = 4 * (i + 1);
        out.push_str(&format!(
            " (i32.store offset={offset} (local.get ${temp}) {value})"
        ));
    }
    out.push_str(&format!(" (local.get ${temp})"));
    Ok(out)
}

/// Construye un registro en memoria lineal: reserva `4*N` bytes con `$__alloc`,
/// guarda cada campo en su offset (orden de DECLARACIÓN) y deja el puntero.
fn emit_record(
    type_name: Option<&str>,
    fields: &[FieldInit],
    ctx: &mut Ctx,
) -> Result<String, String> {
    let name = type_name
        .ok_or("WASM no puede inferir el tipo del registro: anótalo como 'T { ... }'")?;
    let layout = ctx
        .layouts
        .get(name)
        .ok_or_else(|| format!("registro de tipo desconocido '{name}'"))?
        .clone();

    // Valida campos: ni repetidos, ni inexistentes, ni faltantes.
    let mut vistos: HashSet<&str> = HashSet::new();
    for fi in fields {
        if layout.index_of(&fi.name).is_none() {
            return Err(format!(
                "el tipo '{name}' no tiene un campo '{}'",
                fi.name
            ));
        }
        if !vistos.insert(fi.name.as_str()) {
            return Err(format!(
                "campo '{}' repetido en el literal de '{name}'",
                fi.name
            ));
        }
    }
    for (decl_name, _) in &layout.fields {
        if !vistos.contains(decl_name.as_str()) {
            return Err(format!(
                "falta el campo '{decl_name}' al construir '{name}'"
            ));
        }
    }

    // Emite los valores de los campos ANTES de reservar el temporal de este
    // registro, replicando el orden de recorrido de collect_locals_expr.
    let mut emitted: HashMap<&str, String> = HashMap::new();
    for fi in fields {
        let v = emit_expr(&fi.value, ctx)?;
        emitted.insert(fi.name.as_str(), v);
    }

    let temp = format!("__rec{}", ctx.rec_counter);
    ctx.rec_counter += 1;

    let mut out = String::new();
    // Reserva el bloque y guarda el puntero en el temporal.
    out.push_str(&format!(
        "(local.set ${temp} (call $__alloc (i32.const {})))",
        layout.size()
    ));
    // Guarda cada campo en su offset según el ORDEN DE DECLARACIÓN, sin importar
    // el orden en que aparezcan en el literal.
    for (i, (decl_name, _)) in layout.fields.iter().enumerate() {
        let value = emitted
            .get(decl_name.as_str())
            .expect("campo validado como presente");
        let offset = 4 * i;
        out.push_str(&format!(
            " (i32.store offset={offset} (local.get ${temp}) {value})"
        ));
    }
    // Deja el puntero al registro en la pila.
    out.push_str(&format!(" (local.get ${temp})"));
    Ok(out)
}

/// Resuelve `objeto.campo` como un `i32.load offset=4*i`.
///
/// Restricción documentada: sólo funciona cuando el objeto es un identificador
/// con tipo registro conocido, o un literal de registro directo. No resuelve
/// cadenas profundas (`a.b.c`) ni resultados de llamada (`getUser(id).age`).
fn emit_member(object: &Expr, field: &str, ctx: &mut Ctx) -> Result<String, String> {
    let (type_name, ptr) = match object {
        Expr::Ident { name, .. } => {
            let ty = ctx.vars.get(name).cloned().ok_or_else(|| {
                format!(
                    "no se conoce el tipo de '{name}' para acceder a '.{field}' \
                     (anótalo o constrúyelo como registro)"
                )
            })?;
            (ty, format!("(local.get ${name})"))
        }
        Expr::Record {
            type_name: Some(tn),
            ..
        } => {
            let ty = tn.clone();
            let ptr = emit_expr(object, ctx)?;
            (ty, ptr)
        }
        _ => {
            return Err(
                "WASM sólo resuelve acceso a miembro sobre una variable de tipo registro \
                 conocido o un literal de registro directo (no 'a.b.c' ni 'f(x).campo')"
                    .to_string(),
            )
        }
    };

    let layout = ctx
        .layouts
        .get(&type_name)
        .ok_or_else(|| format!("tipo de registro desconocido '{type_name}'"))?;
    let idx = layout
        .index_of(field)
        .ok_or_else(|| format!("el tipo '{type_name}' no tiene un campo '{field}'"))?;
    // Confirma que el tipo del campo está soportado (Float se habría filtrado
    // ya en build_layouts, pero dejamos la consulta documentada).
    let _ = layout.type_of(field);

    let offset = 4 * idx;
    Ok(format!("(i32.load offset={offset} {ptr})"))
}

// --- recolección de literales de cadena ---

fn collect_strings_block(block: &Block, out: &mut Vec<String>, seen: &mut HashSet<String>) {
    for stmt in &block.stmts {
        match stmt {
            Stmt::Let(l) => collect_strings_expr(&l.value, out, seen),
            Stmt::Assign { value, .. } => collect_strings_expr(value, out, seen),
            Stmt::Effect { body, .. } => collect_strings_block(body, out, seen),
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
        Expr::Index { object, index, .. } => {
            collect_strings_expr(object, out, seen);
            collect_strings_expr(index, out, seen);
        }
        Expr::Int { .. } | Expr::Float { .. } | Expr::Bool { .. } | Expr::Ident { .. } => {}
    }
}

// --- utilidades ---

/// ¿Se puede demostrar que esta expresión es una cadena? Conservador a
/// propósito: con que UN lado lo sea, la comparación pasa a ser por contenido,
/// que es lo correcto; si no se puede probar, se compara como i32 (que es lo que
/// hacía antes) y no se rompe nada.
fn es_cadena(e: &Expr, ctx: &Ctx) -> bool {
    match e {
        Expr::Str { .. } => true,
        Expr::Ident { name, .. } => ctx.vars.get(name).map(|t| t == "String" || t == "Html") == Some(true),
        Expr::Call { callee, .. } => matches!(
            callee.as_ref(),
            Expr::Ident { name, .. } if name == "concat"
        ),
        _ => false,
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
        // `And`/`Or` no llegan aquí: se emiten con cortocircuito en emit_expr.
        BinOp::And => "i32.and",
        BinOp::Or => "i32.or",
    }
}

/// Valida que un tipo de valor (param/return) sea representable como `i32`:
/// Int/Bool/String o un nombre de registro declarado (su puntero es `i32`).
fn check_value_type(t: &Type, layouts: &HashMap<String, StructLayout>) -> Result<(), String> {
    match t {
        // `Html` es una cadena en runtime (un puntero i32 idéntico a String):
        // el backend lo trata igual, si no una función pura que construya
        // marcado dejaría de compilar a WASM solo por declarar su tipo.
        Type::Name { name, .. }
            if name == "Int" || name == "Bool" || name == "String" || name == "Html" =>
        {
            Ok(())
        }
        // Una lista es un puntero i32 a [longitud][elementos...].
        Type::Name { name, .. } if name == "List" => Ok(()),
        Type::Name { name, .. } if layouts.contains_key(name) => Ok(()),
        Type::Name { name, .. } => Err(format!(
            "el backend WASM soporta Int/Bool/String/Html, List y registros declarados por ahora, no '{name}'"
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
