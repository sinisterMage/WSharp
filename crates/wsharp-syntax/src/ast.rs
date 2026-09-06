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
    /// Whether another module may name this. Everything is private to the
    /// module it is declared in unless it says otherwise, which is what makes
    /// a module's surface something it states rather than something it leaks.
    pub is_public: bool,
    pub span: Span,
}

/// The parts of a function that a `fn` literal shares with a `fn` declaration.
#[derive(Debug, Clone)]
pub struct Func {
    /// Type parameters, written `fn f[T, U](..)` on a declaration and
    /// `fn [T, U](..)` on a literal. They live here rather than on [`FnDecl`]
    /// because both forms generalise: a `const` bound to a `fn` literal is a
    /// definition, and names type parameters exactly as a declaration does.
    pub generics: Vec<Ident>,
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
    /// See [`FnDecl::is_public`].
    pub is_public: bool,
    /// Type parameters written as `struct[T] { .. }`. A generic struct is
    /// outside the dispatch lattice -- see `parent`.
    pub generics: Vec<Ident>,
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
    /// See [`FnDecl::is_public`].
    pub is_public: bool,
    pub ty: Option<TypeExpr>,
    pub value: Expr,
    pub span: Span,
}

/// A type as written in the source. Resolution to a real type happens in sema.
#[derive(Debug, Clone)]
pub enum TypeExpr {
    /// `i64`, `bool`, `Point`, ...
    Named(Ident),
    /// `[]T`
    Array { elem: Box<TypeExpr>, span: Span },
    /// A type named through a module, at type arguments, or both:
    /// `http.Status4xx`, `Box[i64]`, `coll.Map[str, i64]`. A bare name with
    /// neither is [`TypeExpr::Named`].
    Path {
        segments: Vec<Ident>,
        args: Vec<TypeExpr>,
        span: Span,
    },
    /// `?T`
    Optional { inner: Box<TypeExpr>, span: Span },
    /// `!T`
    ErrUnion {
        inner: Box<TypeExpr>,
        /// `!{NotFound, IoFailed}str` names the errors it can carry, and is
        /// then checked. `None` -- a bare `!str` -- leaves the set to
        /// inference, which is what makes writing one a choice rather than an
        /// obligation.
        errors: Option<Vec<Ident>>,
        span: Span,
    },
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
            TypeExpr::Path { span, .. }
            | TypeExpr::Array { span, .. }
            | TypeExpr::Optional { span, .. }
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
    For(ForStmt),
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
            Stmt::For(s) => s.span,
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
/// `for (xs) |x| { }`, or `for (xs) |x, i| { }` to bind the index too.
#[derive(Debug, Clone)]
pub struct ForStmt {
    pub iter: Expr,
    pub value: Ident,
    /// The index, when the capture names two things.
    pub index: Option<Ident>,
    pub body: Block,
    pub span: Span,
}

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
    Int(i128, Span),
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
    /// `[]i64{ 1, 2, 3 }` -- the element type is written, so an empty literal
    /// still has one.
    ArrayLit {
        elem: TypeExpr,
        elems: Vec<Expr>,
        span: Span,
    },
    /// `a[i]`
    Index {
        obj: Box<Expr>,
        index: Box<Expr>,
        span: Span,
    },
    StructLit {
        /// The type's name, possibly reached through a module:
        /// `Point{ .. }` or `util.Point{ .. }`.
        path: Vec<Ident>,
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
    /// `@import("std/http")` -- names a module. Only legal as the value of a
    /// top-level `const`, which is what binds the name it is reached by.
    /// `{ stmt; stmt; value }` on the right of a `catch` or an `orelse`.
    ///
    /// The last thing in it may be an expression with no `;`, which is the
    /// value the whole block takes. A block with no such expression has to
    /// leave some other way -- `return`, `break`, `continue` -- because the
    /// operator it belongs to still has to produce something; a bare
    /// `f() catch return 0` is the one-statement spelling of that.
    Block {
        stmts: Vec<Stmt>,
        value: Option<Box<Expr>>,
        span: Span,
    },
    /// `@spawn(m, args..)` -- start a worker running module `m`'s service,
    /// with `m.init(args..)` as its first act.
    Spawn {
        module: Box<Expr>,
        args: Vec<Expr>,
        span: Span,
    },
    /// `@join(w)` -- wait for a worker to finish and shut it down.
    Join {
        worker: Box<Expr>,
        span: Span,
    },
    Import {
        path: Box<str>,
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
            | Expr::ArrayLit { span, .. }
            | Expr::Index { span, .. }
            | Expr::Block { span, .. }
            | Expr::Spawn { span, .. }
            | Expr::Join { span, .. }
            | Expr::Import { span, .. }
            | Expr::StructLit { span, .. }
            | Expr::Try { span, .. }
            | Expr::Catch { span, .. }
            | Expr::Orelse { span, .. }
            | Expr::Unwrap { span, .. } => *span,
            Expr::Fn(f) => f.span,
            Expr::If(e) => e.span,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Neg,
    Not,
    /// `~x`: every bit flipped. Spelled `~` rather than reusing `!`, which is
    /// boolean negation and stays that way -- `!x` on a number is a type
    /// error, not a bit pattern.
    BitNot,
}

impl UnOp {
    pub fn text(self) -> &'static str {
        match self {
            UnOp::Neg => "-",
            UnOp::Not => "!",
            UnOp::BitNot => "~",
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
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
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
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
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

    /// The bit operators. Like arithmetic they take two operands of the same
    /// type and produce it, but they demand an *integer* one: there is no
    /// meaning to give `1.5 & 2.0` that anybody would want.
    pub fn is_bitwise(self) -> bool {
        matches!(
            self,
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr
        )
    }
}
