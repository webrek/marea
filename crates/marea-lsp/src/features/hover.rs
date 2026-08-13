//! Hover: muestra información SINTÁCTICA del nodo bajo el cursor, en markdown.
//!
//! No infiere tipos: solo reconstruye, a partir del AST, lo que ya está escrito
//! en la fuente. Lo que sabe enseñar:
//!   - una función (su nombre, su firma o un uso de ella) → la firma
//!     reconstruida, con su ubicación y su POLÍTICA (`@server(u: Usuario)`), que
//!     es la mitad de lo que hay que saber antes de llamarla;
//!   - un cierre → `fn(a: Int) -> Bool`;
//!   - un nombre local —parámetro, parámetro de cierre, `let`, variable de un
//!     `for`, la identidad de la política— → su tipo tal como se escribió;
//!   - un nombre IMPORTADO → su declaración, buscada en el módulo que lo declara,
//!     y de qué archivo viene;
//!   - un tipo, una `type`, un `store` y el propio `import`.

use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position};
use marea_syntax::ast::{Expr, FnDecl, Import, ImportName, Item, Location, Param, Type, TypeDecl};

use crate::analysis::{
    find_import_at, find_node_at, resolver_ligadura, ClaseLigadura, Ligadura, Node,
};
use crate::documents::Document;
use crate::programa::{Archivo, Salida};

/// Resuelve `textDocument/hover` en la posición dada.
///
/// Devuelve `None` si el nodo bajo el cursor no tiene una representación
/// sintáctica útil (p. ej. un literal numérico).
pub fn hover(salida: &Salida, doc: &Document, position: Position) -> Option<Hover> {
    let offset = doc.line_index.position_to_offset(position, &doc.text);
    let markdown = hover_markdown(salida, salida.entrada(), offset)?;
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    })
}

/// Construye el cuerpo markdown del hover en `offset`, o `None` si no aplica.
fn hover_markdown(salida: &Salida, entrada: &Archivo, offset: usize) -> Option<String> {
    // Los `import` viven fuera de `items`, así que se miran antes que el AST.
    if let Some((imp, nombre)) = find_import_at(&entrada.modulo, offset) {
        return hover_import(salida, entrada, imp, nombre);
    }

    match find_node_at(&entrada.modulo, offset)? {
        // Uso de un nombre: local, del módulo, o traído por un `import`.
        Node::Expr(Expr::Ident { name, .. }) => resolver_nombre(salida, entrada, offset, name),
        Node::Type(ty) => match ty {
            // Un alias del programa muestra su definición; si no lo es, el tipo
            // formateado tal cual (un builtin, un genérico).
            Type::Name { name, .. } => resolver_nombre(salida, entrada, offset, name)
                .or_else(|| Some(code_block(&render_type(ty)))),
            // Uniones y registros escritos en línea se formatean directos.
            _ => Some(code_block(&render_type(ty))),
        },
        // Sobre el nombre de un parámetro (de función o de cierre) → `n: T`.
        Node::Param(p) => Some(code_block(&format!("{}: {}", p.name, render_type(&p.ty)))),
        // Un cierre: su firma, que es lo único que tiene.
        Node::Expr(Expr::Fn {
            params,
            return_type,
            ..
        }) => Some(code_block(&closure_signature(params, return_type.as_ref()))),
        Node::Item(item) => Some(code_block(&item_signature(item))),
        _ => None,
    }
}

/// Qué es `name` visto desde `offset`, buscándolo en el mismo orden en que lo
/// resuelve el verificador: primero los ámbitos léxicos, luego lo que declara el
/// módulo, y sólo entonces lo que trajo un `import`.
fn resolver_nombre(
    salida: &Salida,
    entrada: &Archivo,
    offset: usize,
    name: &str,
) -> Option<String> {
    if let Some(l) = resolver_ligadura(&entrada.modulo, offset, name) {
        return Some(code_block(&render_ligadura(&l)));
    }
    if let Some(item) = entrada.modulo.items.iter().find(|i| i.name() == name) {
        return Some(code_block(&item_signature(item)));
    }
    let (destino, item) = salida.declaracion_importada(entrada, name)?;
    Some(con_origen(&item_signature(item), destino))
}

/// El hover de un `import`: sobre uno de sus nombres, la declaración que trae;
/// sobre el resto de la línea, qué declara el módulo del otro lado.
fn hover_import(
    salida: &Salida,
    entrada: &Archivo,
    imp: &Import,
    nombre: Option<&ImportName>,
) -> Option<String> {
    if let Some(n) = nombre {
        let (destino, item) = salida.declaracion_importada(entrada, &n.name)?;
        return Some(con_origen(&item_signature(item), destino));
    }
    let destino = salida.destino(entrada, &imp.path)?;
    let nombres: Vec<&str> = destino.modulo.items.iter().map(|i| i.name()).collect();
    let lista = nombres.join("`, `");
    let de = nombre_corto(destino);
    Some(format!("módulo `{de}`\n\ndeclara: `{lista}`"))
}

/// Una declaración con la coletilla de qué archivo la trae.
fn con_origen(codigo: &str, destino: &Archivo) -> String {
    let de = nombre_corto(destino);
    format!("{}\n\nimportado de `{de}`", code_block(codigo))
}

/// El nombre del archivo, sin su directorio: en el editor lo que ubica es
/// `usuarios.mar`, no la ruta entera.
fn nombre_corto(archivo: &Archivo) -> String {
    archivo
        .ruta
        .as_ref()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "este documento".to_string())
}

/// Reconstruye lo que se escribió para declarar un item.
fn item_signature(item: &Item) -> String {
    match item {
        Item::Fn(f) => fn_signature(f),
        Item::Type(t) => type_decl(t),
        Item::Let(l) => {
            let mut s = String::from(if l.reactive { "reactive " } else { "let " });
            if l.mutable {
                s.push_str("mut ");
            }
            s.push_str(&l.name);
            if let Some(ty) = &l.ty {
                s.push_str(": ");
                s.push_str(&render_type(ty));
            }
            s
        }
        Item::Store { name, ty, .. } => format!("store {}: {}", name, render_type(ty)),
    }
}

/// Reconstruye la firma de una función, con su anotación completa. La política
/// forma parte de la firma tanto como los parámetros: dice quién puede cruzar la
/// frontera hasta ella, y sin verla no se sabe si una llamada va a compilar.
fn fn_signature(f: &FnDecl) -> String {
    let mut sig = String::new();
    if f.es_session {
        sig.push_str("@session\n");
    }
    if let Some(loc) = f.location {
        sig.push('@');
        sig.push_str(location_name(loc));
        if let Some(politica) = &f.politica {
            sig.push('(');
            // `@server(u: Usuario)` liga la identidad a `u`; `@server(Usuario)`
            // la exige sin nombrarla.
            if let Some(bind) = &f.identidad_bind {
                sig.push_str(bind);
                sig.push_str(": ");
            }
            sig.push_str(&render_type(politica));
            sig.push(')');
        }
        sig.push('\n');
    }
    sig.push_str("fn ");
    sig.push_str(&f.name);
    sig.push('(');
    sig.push_str(&render_params(&f.params));
    sig.push(')');
    if let Some(rt) = &f.return_type {
        sig.push_str(" -> ");
        sig.push_str(&render_type(rt));
    }
    sig
}

/// La firma de un cierre: una declaración a la que sólo le falta el nombre.
fn closure_signature(params: &[Param], return_type: Option<&Type>) -> String {
    let mut sig = format!("fn({})", render_params(params));
    if let Some(rt) = return_type {
        sig.push_str(" -> ");
        sig.push_str(&render_type(rt));
    }
    sig
}

/// Cómo se enseña un nombre local. Que se distinga la clase importa: un
/// parámetro de cierre y una variable de `for` se leen igual y no son lo mismo.
fn render_ligadura(l: &Ligadura<'_>) -> String {
    let tipo = match l.ty {
        Some(t) => format!(": {}", render_type(t)),
        None => String::new(),
    };
    match l.clase {
        ClaseLigadura::Param => format!("param {}{}", l.nombre, tipo),
        ClaseLigadura::ParamCierre => format!("param de cierre {}{}", l.nombre, tipo),
        // No es un parámetro: la inyecta el runtime al resolver el token.
        ClaseLigadura::Identidad => format!("identidad {}{}", l.nombre, tipo),
        ClaseLigadura::Let => format!("let {}{}", l.nombre, tipo),
        ClaseLigadura::Reactive => format!("reactive {}{}", l.nombre, tipo),
        ClaseLigadura::For => format!("for {}{}", l.nombre, tipo),
        ClaseLigadura::Indice => format!("índice {}: Int", l.nombre),
        ClaseLigadura::Patron => format!("match {}{}", l.nombre, tipo),
    }
}

/// Renderiza la lista de parámetros como `n1: T1, n2: T2`.
fn render_params(params: &[Param]) -> String {
    params
        .iter()
        .map(|p| format!("{}: {}", p.name, render_type(&p.ty)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Reconstruye una declaración de tipo: `type N = <tipo>`.
fn type_decl(t: &TypeDecl) -> String {
    format!("type {} = {}", t.name, render_type(&t.aliased))
}

/// Nombre textual de una ubicación.
fn location_name(loc: Location) -> &'static str {
    match loc {
        Location::Server => "server",
        Location::Client => "client",
        Location::Edge => "edge",
    }
}

/// Formatea un `Type` del AST como texto fuente aproximado.
///
/// - `Name { args }` → `N` o `N<A, B>`;
/// - `Union { variants }` → `A | B | C`;
/// - `Record { fields }` → `{ n: T, ... }`.
fn render_type(ty: &Type) -> String {
    match ty {
        Type::Name { name, args, .. } => {
            if args.is_empty() {
                name.clone()
            } else {
                let inner = args.iter().map(render_type).collect::<Vec<_>>().join(", ");
                format!("{name}<{inner}>")
            }
        }
        Type::Union { variants, .. } => variants
            .iter()
            .map(render_type)
            .collect::<Vec<_>>()
            .join(" | "),
        Type::Record { fields, .. } => {
            let inner = fields
                .iter()
                .map(|f| format!("{}: {}", f.name, render_type(&f.ty)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{ {inner} }}")
        }
    }
}

/// Envuelve un fragmento de código en un bloque markdown con lenguaje `marea`.
fn code_block(code: &str) -> String {
    format!("```marea\n{code}\n```")
}
