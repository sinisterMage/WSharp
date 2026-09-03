//! The W# abstract syntax tree.
//!
//! Every node carries a span. Nothing here is resolved or typed -- names are
//! plain strings and annotations are unresolved `TypeExpr`s; `wsharp-sema` turns
//! this into typed HIR.

use crate::span::{Ident, Span};

#[derive(Debug, Clone)]
pub struct Module {
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub enum Item {
    Fn(FnDecl),
    Struct(StructDecl),
    Const(ConstDecl),
}

impl Item {
    pub fn name(&self) -> &Ident {
        match self {
            Item::Fn(d) => &d.name,
            Item::Struct(d) => &d.name,
            Item::Const(d) => &d.name,
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Item::Fn(d) => d.span,
            Item::Struct(d) => d.span,
            Item::Const(d) => d.span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: Ident,
    pub func: Func,
    pub span: Span,
}

/// The parts of a function that a `fn` literal shares with a `fn` declaration.
#[derive(Debug, Clone)]
pub struct Func {
    pub params: Vec<Param>,
    /// `None` when the return type is left to inference.
    pub ret: Option<TypeExpr>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: Ident,
    /// `None` when the parameter type is left to inference.
    pub ty: Option<TypeExpr>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: Ident,
    /// The supertype written as `struct : Parent { ... }`, if any. Unresolved
    /// here; sema turns it into a `StructId` and builds the dispatch lattice.
    pub parent: Option<Ident>,
    pub fields: Vec<FieldDecl>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldDecl {
    pub name: Ident,
    pub ty: TypeExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

/// A type as written in the source. Resolution to a real type happens in sema.
#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// `i64`, `bool`, `Point`, ...
    Named(Ident),
    /// `?T`
    Optional { inner: Box<TypeExpr>, span: Span },
    /// `!T`
    ErrUnion { inner: Box<TypeExpr>, span: Span },
    /// `fn(i64, i64) i64`
    Fn {
        params: Vec<TypeExpr>,
        ret: Box<TypeExpr>,
        span: Span,
    },
}

impl TypeExpr {
    pub fn span(&self) -> Span {
        match self {
            TypeExpr::Named(id) => id.span,
            TypeExpr::Optional { span, .. }
            | TypeExpr::ErrUnion { span, .. }
            | TypeExpr::Fn { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let(LetStmt),
    Assign(AssignStmt),
    Expr(Expr),
    Return { value: Option<Expr>, span: Span },
    If(IfStmt),
    While(WhileStmt),
    Block(Block),
    Break(Span),
    Continue(Span),
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let(s) => s.span,
            Stmt::Assign(s) => s.span,
            Stmt::Expr(e) => e.span(),
            Stmt::Return { span, .. } => *span,
            Stmt::If(s) => s.span,
            Stmt::While(s) => s.span,
            Stmt::Block(b) => b.span,
            Stmt::Break(span) | Stmt::Continue(span) => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct LetStmt {
    /// `var` is mutable, `const` is not.
    pub mutable: bool,
    pub name: Ident,
    pub ty: Option<TypeExpr>,
    pub init: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct AssignStmt {
    pub target: Expr,
    /// `Some(op)` for compound assignment (`+=` is `Some(Add)`).
    pub op: Option<BinOp>,
    pub value: Expr,
    pub span: Span,
}

/// `if (cond) |capture| { .. } else { .. }` in statement position.
#[derive(Debug, Clone)]
pub struct IfStmt {
    pub cond: Expr,
    /// The `|v|` payload binding used to unwrap an optional.
    pub capture: Option<Ident>,
    pub then: Block,
    pub else_: Option<Box<ElseBranch>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ElseBranch {
    Block(Block),
    If(IfStmt),
}

/// `while (cond) |capture| : (cont) { .. }`
#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub cond: Expr,
    pub capture: Option<Ident>,
    /// The `: (i += 1)` continue expression, run after each iteration and before
    /// re-testing the condition. `continue` jumps to it, not past it.
    pub cont: Option<Box<Stmt>>,
    pub body: Block,
    pub span: Span,
}

/// `if (cond) a else b` in expression position. Unlike the statement form the
/// `else` is mandatory, because the expression must always produce a value.
#[derive(Debug, Clone)]
pub struct IfExpr {
    pub cond: Expr,
    pub capture: Option<Ident>,
    pub then: Expr,
    pub else_: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FieldInit {
    pub name: Ident,
    pub value: Expr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Float(f64, Span),
    Bool(bool, Span),
    Str(Box<str>, Span),
    Null(Span),
    Ident(Ident),
    /// `error.Negative`
    ErrorLit {
        name: Ident,
        span: Span,
    },
    Unary {
        op: UnOp,
        expr: Box<Expr>,
        span: Span,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
    Call {
        callee: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    Field {
        obj: Box<Expr>,
        name: Ident,
        span: Span,
    },
    StructLit {
        name: Ident,
        fields: Vec<FieldInit>,
        span: Span,
    },
    /// A `fn` literal: `fn (a, b) { .. }`.
    Fn(Box<Func>),
    If(Box<IfExpr>),
    /// `try e` -- propagate the error out of the enclosing function.
    Try {
        expr: Box<Expr>,
        span: Span,
    },
    /// `e catch |err| alt`
    Catch {
        expr: Box<Expr>,
        capture: Option<Ident>,
        alt: Box<Expr>,
        span: Span,
    },
    /// `e orelse alt`
    Orelse {
        expr: Box<Expr>,
        alt: Box<Expr>,
        span: Span,
    },
    /// `e.?` -- unwrap an optional, trapping on null.
    Unwrap {
        expr: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s)
            | Expr::Float(_, s)
            | Expr::Bool(_, s)
            | Expr::Str(_, s)
            | Expr::Null(s) => *s,
            Expr::Ident(id) => id.span,
            Expr::ErrorLit { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Call { span, .. }
            | Expr::Field { span, .. }
            | Expr::StructLit { span, .. }
            | Expr::Try { span, .. }
            | Expr::Catch { span, .. }
            | Expr::Orelse { span, .. }
            | Expr::Unwrap { span, .. } => *span,
            Expr::Fn(f) => f.span,
            Expr::If(e) => e.span,
        }
    }

    /// Whether this expression is a syntactic value, in the sense of the value
    /// restriction: sema only generalises `const` bindings whose initialiser is
    /// one of these.
    pub fn is_syntactic_value(&self) -> bool {
        matches!(
            self,
            Expr::Int(..)
                | Expr::Float(..)
                | Expr::Bool(..)
                | Expr::Str(..)
                | Expr::Null(..)
                | Expr::Fn(..)
                | Expr::Ident(..)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
}

impl UnOp {
    pub fn text(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
        }
    }
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
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn text(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "and",
            BinOp::Or => "or",
        }
    }

    /// Arithmetic operators take two operands of the same numeric type and
    /// produce that type; comparisons produce `bool`; `and`/`or` are boolean.
    pub fn is_arithmetic(self) -> bool {
        matches!(
            self,
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem
        )
    }

    pub fn is_comparison(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    }

    /// `==` and `!=` work on any two values of the same type; the ordering
    /// comparisons additionally require a numeric type.
    pub fn is_ordering(self) -> bool {
        matches!(self, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge)
    }

    pub fn is_logical(self) -> bool {
        matches!(self, BinOp::And | BinOp::Or)
    }
}
