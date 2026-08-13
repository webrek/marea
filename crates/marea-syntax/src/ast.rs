//! Árbol de sintaxis abstracta (AST) de Marea.
//!
//! Tres ideas de Marea ya viven aquí, como datos de primera clase:
//!   - `Location`: dónde corre una función (servidor / cliente / borde).
//!   - `LetStmt.reactive`: una variable cuyo valor se recomputa solo.
//!   - `Type::Union`: tipos unión (`User | NotFound`) para errores como valores.

use crate::span::Span;

#[derive(Debug, Clone, PartialEq)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Item {
    Fn(FnDecl),
    Type(TypeDecl),
    Let(LetStmt),
    /// Declara el tipo de elemento del store del servidor: `store Post;`.
    /// Tipa los builtins de estado `guardar(T)` y `todos() -> List<T>`.
    /// `store nombre: Tipo;` — declara un almacén con nombre. El nombre es un
    /// valor de primera clase que se pasa a `guardar`/`todos`/`actualizar`/
    /// `borrar`, de modo que un módulo puede tener varios almacenes.
    Store {
        name: String,
        name_span: Span,
        ty: Type,
        span: Span,
    },
}

/// Dónde se ejecuta una función. El compilador genera el cruce de frontera
/// (RPC, serialización, validación) a partir de esto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Location {
    Server,
    Client,
    Edge,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FnDecl {
    pub location: Option<Location>,
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<Type>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    /// Span del nombre del parámetro (para hover/go-to-def en el LSP).
    pub name_span: Span,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypeDecl {
    pub name: String,
    pub aliased: Type,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Type {
    /// `User`, `Int`, `List<User>`, ...
    Name {
        name: String,
        args: Vec<Type>,
        span: Span,
    },
    /// `User | NotFound | Error`
    Union { variants: Vec<Type>, span: Span },
    /// `{ name: String, age: Int }` — registro estructural.
    Record { fields: Vec<FieldDef>, span: Span },
}

/// Un campo en la declaración de un registro: `name: String`.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDef {
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

impl Type {
    pub fn span(&self) -> Span {
        match self {
            Type::Name { span, .. } | Type::Union { span, .. } | Type::Record { span, .. } => *span,
        }
    }

    /// Si es un registro estructural, devuelve sus campos.
    pub fn as_record(&self) -> Option<&[FieldDef]> {
        match self {
            Type::Record { fields, .. } => Some(fields),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `for x in xs { ... }` — recorre una lista. Con índice: `for x, i in xs`.
    /// Siempre sobre una lista, así que siempre termina: no hay `while` y por
    /// tanto no hay bucles infinitos que escribir por accidente.
    For {
        var: String,
        var_span: Span,
        index: Option<String>,
        index_span: Option<Span>,
        iter: Expr,
        body: Block,
        span: Span,
    },
    Let(LetStmt),
    Return {
        value: Option<Expr>,
        span: Span,
    },
    /// Asignación a una variable existente: `n = n + 1;`.
    Assign {
        name: String,
        name_span: Span,
        value: Expr,
        span: Span,
    },
    /// Efecto reactivo: el bloque se re-ejecuta cuando cambian las variables
    /// reactivas que lee. `effect { print(total); }`.
    Effect {
        body: Block,
        span: Span,
    },
    Expr(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LetStmt {
    /// `true` si se declaró con `reactive` en vez de `let`.
    pub reactive: bool,
    pub mutable: bool,
    pub name: String,
    pub ty: Option<Type>,
    pub value: Expr,
    pub span: Span,
}

/// Una pieza de plantilla ya parseada.
#[derive(Debug, Clone, PartialEq)]
pub enum TemplatePart {
    Lit(String),
    /// `{expr}` se escapa; `{!expr}` se inserta tal cual y el verificador exige
    /// que sea `Html`, de modo que la forma cruda no puede colar texto sin
    /// escapar.
    Interp {
        expr: Box<Expr>,
        raw: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int {
        value: i64,
        span: Span,
    },
    Float {
        value: f64,
        span: Span,
    },
    Str {
        value: String,
        span: Span,
    },
    Bool {
        value: bool,
        span: Span,
    },
    Ident {
        name: String,
        span: Span,
    },
    Unary {
        op: UnaryOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        left: Box<Expr>,
        right: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Member {
        object: Box<Expr>,
        field: String,
        span: Span,
    },
    If {
        cond: Box<Expr>,
        then_branch: Block,
        else_branch: Option<Box<ElseBranch>>,
        span: Span,
    },
    Match {
        scrutinee: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// Literal de registro: `User { name: "x", age: 1 }`.
    /// `type_name` es el nombre que precede a `{` (el parser siempre pone Some;
    /// el Option deja espacio para inferencia de tipo destino más adelante).
    Record {
        type_name: Option<String>,
        type_name_span: Option<Span>,
        fields: Vec<FieldInit>,
        span: Span,
    },
    /// Literal de lista: `[1, 2, 3]`.
    List {
        elements: Vec<Expr>,
        span: Span,
    },
    /// Indexado de lista: `xs[i]`.
    /// Plantilla de texto: `` `hola {nombre}` ``. Produce `Html`: los huecos
    /// `{x}` se escapan solos y `{!x}` inserta marcado que ya es seguro, así que
    /// olvidarse del escapado deja de ser posible.
    Template {
        parts: Vec<TemplatePart>,
        span: Span,
    },
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
}

/// Inicialización de un campo en un literal de registro: `name: "x"`.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldInit {
    pub name: String,
    pub value: Expr,
    pub span: Span,
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int { span, .. }
            | Expr::Float { span, .. }
            | Expr::Str { span, .. }
            | Expr::Bool { span, .. }
            | Expr::Ident { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Call { span, .. }
            | Expr::Member { span, .. }
            | Expr::If { span, .. }
            | Expr::Match { span, .. }
            | Expr::Record { span, .. }
            | Expr::List { span, .. }
            | Expr::Template { span, .. } => *span,
            Expr::Index { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ElseBranch {
    Block(Block),
    If(Expr),
}

#[derive(Debug, Clone, PartialEq)]
pub struct MatchArm {
    pub pattern: Pattern,
    pub body: Expr,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pattern {
    Wildcard { span: Span },
    Binding { name: String, span: Span },
    Int { value: i64, span: Span },
    Bool { value: bool, span: Span },
    Str { value: String, span: Span },
}

impl Pattern {
    pub fn span(&self) -> Span {
        match self {
            Pattern::Wildcard { span }
            | Pattern::Binding { span, .. }
            | Pattern::Int { span, .. }
            | Pattern::Bool { span, .. }
            | Pattern::Str { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    Neg,
    Not,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
}
