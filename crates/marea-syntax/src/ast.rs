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
}

impl Type {
    pub fn span(&self) -> Span {
        match self {
            Type::Name { span, .. } | Type::Union { span, .. } => *span,
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
    Let(LetStmt),
    Return { value: Option<Expr>, span: Span },
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

#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    Int { value: i64, span: Span },
    Float { value: f64, span: Span },
    Str { value: String, span: Span },
    Bool { value: bool, span: Span },
    Ident { name: String, span: Span },
    Unary { op: UnaryOp, expr: Box<Expr>, span: Span },
    Binary { op: BinOp, left: Box<Expr>, right: Box<Expr>, span: Span },
    Call { callee: Box<Expr>, args: Vec<Expr>, span: Span },
    Member { object: Box<Expr>, field: String, span: Span },
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
            | Expr::Match { span, .. } => *span,
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
