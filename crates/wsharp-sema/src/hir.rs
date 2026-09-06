//! Typed, name-resolved intermediate representation -- the type checker's
//! output and the code generator's input.
//!
//! Compared with the AST: names are resolved to indices, every expression
//! carries its type, struct fields are byte offsets, and `and`/`or` are split
//! out from the other binary operators because they compile to branches.

use wsharp_runtime::TypeId;
use wsharp_syntax::Span;
use wsharp_syntax::ast::{BinOp, UnOp};

pub use crate::ty::ServiceId;
use crate::ty::{Scheme, StructId, Type};

pub type FuncId = u32;
pub type LocalId = u32;
pub type StrId = u32;
/// Index into [`Program::arrays`]; a top-level `const` array of scalars.
pub type ArrayId = u32;
pub type BuiltinId = u32;
/// Index into [`Program::errors`]; the runtime representation of an error value.
pub type ErrorId = u32;

#[derive(Debug)]
pub struct Program {
    pub structs: Vec<StructDef>,
    pub funcs: Vec<FuncDef>,
    /// String literals, emitted as immortal static data by the code generator.
    pub strings: Vec<String>,
    /// Top-level `const` arrays, emitted the same way and for the same reason.
    pub arrays: Vec<ArrayConst>,
    /// Error names in declaration order; an error value is one of these indices.
    pub errors: Vec<String>,
    /// `main`, if the program has one.
    pub entry: Option<FuncId>,
    /// The services a program spawns, indexed by `ServiceId`. A service is an
    /// ordinary module: no new declaration form, because W# has no mutable
    /// globals and the state had to be explicit anyway.
    pub services: Vec<ServiceDef>,
}

impl Program {
    pub fn func(&self, id: FuncId) -> &FuncDef {
        &self.funcs[id as usize]
    }

    pub fn strukt(&self, id: StructId) -> &StructDef {
        &self.structs[id as usize]
    }
}

/// A top-level `const` bound to an array of scalar literals.
///
/// Emitted into the data section beside the string literals and immortal for
/// the same reason: the collector must neither move it nor free it. It holds
/// no references -- which is the whole of why it needs no roots and no startup
/// initialiser, and so why it is a literal rather than a computed global.
#[derive(Debug, Clone)]
pub struct ArrayConst {
    /// The array type. Its rendered form is the key the code generator looks
    /// the runtime type id up by, and its element decides the stride.
    pub ty: Type,
    /// One entry per element, already masked into its element's width: an
    /// integer as `IntTy::mask` leaves it, an `f64` as its bits, a `bool` as 0
    /// or 1. The code generator writes the low `stride` bytes of each.
    pub values: Vec<u64>,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    /// The variables this struct's type parameters stand for, empty unless it
    /// was declared `struct[T] { .. }`. A generic struct's field offsets and
    /// size depend on what it is instantiated at -- a `?T` field is two slots
    /// or three -- so they are left [`UNRESOLVED`] here and computed per
    /// instantiation by the code generator, where every type is concrete.
    pub params: Vec<crate::ty::TypeVarId>,
    /// The declared supertype, if any.
    pub parent: Option<StructId>,
    /// Every field, **inherited ones first**: a subtype's layout begins with a
    /// byte-identical copy of its supertype's, so a field read compiled against
    /// the supertype is correct on any instance of a subtype.
    pub fields: Vec<FieldDef>,
    /// Runtime type id, written into the header of every instance. Ids are
    /// assigned in a preorder walk of the lattice, so a type's subtypes occupy
    /// `type_id .. type_id + subtree_len` -- which turns the dispatcher's
    /// "is this a subtype of T?" into one subtract and one unsigned compare.
    pub type_id: TypeId,
    /// Number of types in this type's subtree, itself included.
    pub subtree_len: u32,
    /// Total instance size in bytes, header included.
    pub size: u32,
    /// Offset just past the last field, before rounding up to `size`. A
    /// subtype starts laying its own fields out here.
    pub field_end: u32,
    /// How many of `fields` came from the supertype.
    pub inherited: u32,
    pub span: Span,
}

impl StructDef {
    pub fn field_index(&self, name: &str) -> Option<u32> {
        self.fields
            .iter()
            .position(|f| f.name == name)
            .map(|i| i as u32)
    }
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: String,
    pub ty: Type,
    /// Byte offset from the start of the object, past the header.
    pub offset: u32,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    /// Locals holding the parameters, in order.
    pub params: Vec<LocalId>,
    /// Locals holding values loaded out of the closure environment on entry,
    /// in the order the enclosing `Closure` expression supplies them.
    pub captures: Vec<LocalId>,
    pub locals: Vec<LocalDef>,
    pub ret: Type,
    pub body: Block,
    /// The generalised type of this function. Generic functions are specialised
    /// by monomorphisation before code generation.
    pub scheme: Scheme,
    /// True for `fn` literals, which are reached only through a closure value.
    pub is_closure: bool,
    /// The local holding this function's own closure value, for a `fn` literal
    /// whose body names itself.
    ///
    /// A literal is only ever entered through a closure value, and a call
    /// passes that value as the environment pointer -- so the environment *is*
    /// a closure for this function at this instantiation, and the recursive
    /// reference costs a register rather than an allocation. `None` when the
    /// body never names itself, which is every function written before
    /// recursion was allowed here.
    pub self_local: Option<LocalId>,
    pub span: Span,
}

/// One module, used as a worker's service.
///
/// `init` makes the state; a method is any function in the module whose first
/// parameter is that state. Nothing here is generic: the runtime calls a
/// method through a fixed trampoline, which one machine implementation has to
/// satisfy.
#[derive(Debug, Clone)]
pub struct ServiceDef {
    pub name: String,
    pub init: FuncId,
    pub methods: Vec<ServiceMethod>,
}

#[derive(Debug, Clone)]
pub struct ServiceMethod {
    pub name: String,
    pub func: FuncId,
}

#[derive(Debug, Clone)]
pub struct LocalDef {
    pub name: String,
    pub ty: Type,
    pub mutable: bool,
    pub span: Span,
}

#[derive(Debug, Clone, Default)]
pub struct Block {
    pub stmts: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        local: LocalId,
        init: Expr,
    },
    Assign {
        place: Place,
        value: Expr,
    },
    Expr(Expr),
    Return(Option<Expr>),
    If {
        cond: Expr,
        capture: Option<LocalId>,
        then: Block,
        els: Option<Block>,
    },
    While {
        cond: Expr,
        capture: Option<LocalId>,
        cont: Option<Box<Stmt>>,
        body: Block,
    },
    Block(Block),
    Break,
    Continue,
}

/// Placeholder for a struct/field index that inference could not resolve on the
/// spot, because the object's type was still a variable. A fix-up pass fills
/// these in once the binding group's constraints have been solved.
pub const UNRESOLVED: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub enum Place {
    Local(LocalId),
    Field {
        obj: Expr,
        strukt: StructId,
        index: u32,
        name: Box<str>,
    },
    /// `a[i] = v`. Bounds-checked when it runs, like a read.
    Index {
        arr: Expr,
        index: Expr,
    },
}

#[derive(Debug, Clone)]
pub struct Expr {
    pub kind: ExprKind,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum ExprKind {
    Int(i128),
    /// `u32(x)`: a numeric conversion, written rather than inferred. The type
    /// converted *to* is the expression's own type; this is what it came from.
    Convert(Box<Expr>),
    Float(f64),
    Bool(bool),
    Str(StrId),
    /// The address of a top-level `const` array, which lives in the data
    /// section rather than the heap. No allocation, exactly as [`Self::Str`]
    /// and [`Self::Singleton`] are none.
    ArrayConst(ArrayId),
    /// The absent case of an optional.
    Null,
    Local(LocalId),
    Unary {
        op: UnOp,
        expr: Box<Expr>,
    },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    /// `and` / `or`, which must not evaluate their right operand eagerly.
    Logical {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
    },
    Call {
        callee: Callee,
        args: Vec<Expr>,
    },
    /// Allocate an array holding these elements. The element type is
    /// [`Expr::ty`]'s argument, which is what an empty literal still has.
    ArrayNew {
        elems: Vec<Expr>,
    },
    /// `a[i]`. Panics if `i` is not below the array's length.
    Index {
        arr: Box<Expr>,
        index: Box<Expr>,
    },
    /// How many elements an array holds -- one load of the header's `aux`
    /// word. A node rather than a builtin call because `for` desugars into a
    /// loop that needs it before any library exists.
    ArrayLen {
        arr: Box<Expr>,
    },
    /// Allocate a struct instance; `fields` are in declaration order.
    StructNew {
        strukt: StructId,
        fields: Vec<Expr>,
    },
    Field {
        obj: Box<Expr>,
        strukt: StructId,
        index: u32,
        name: Box<str>,
    },
    If {
        cond: Box<Expr>,
        capture: Option<LocalId>,
        then: Box<Expr>,
        els: Box<Expr>,
    },
    /// Build a closure object capturing the given values.
    Closure {
        func: FuncId,
        targs: Vec<Type>,
        captures: Vec<Expr>,
    },
    /// The sole instance of a struct with no fields. It lives in the module's
    /// data section rather than the heap, so this compiles to one address --
    /// no allocation, and nothing for the collector to trace.
    Singleton(StructId),

    // -- optionals and error unions --
    /// Wrap a present value into an optional.
    Some(Box<Expr>),
    /// Wrap a success value into an error union.
    Ok(Box<Expr>),
    /// An error value.
    Err(ErrorId),
    /// `e orelse alt`
    /// `{ stmt; stmt; value }` -- the block form of a `catch` or an `orelse`.
    ///
    /// `value` is what the block produces. `None` means it never produces
    /// anything: every path out of it returns, breaks or continues, and the
    /// type inference gave the whole expression is whatever its context wanted.
    Block {
        stmts: Vec<Stmt>,
        value: Option<Box<Expr>>,
    },
    /// `@spawn(m, args..)` -- start a worker, run `m.init(args..)` on it.
    Spawn {
        service: ServiceId,
        args: Vec<Expr>,
    },
    /// `@join(w)`.
    Join(Box<Expr>),
    Orelse {
        expr: Box<Expr>,
        alt: Box<Expr>,
    },
    /// `e catch |err| alt`
    Catch {
        expr: Box<Expr>,
        capture: Option<LocalId>,
        alt: Box<Expr>,
    },
    /// `try e` -- return early if `e` is an error.
    Try(Box<Expr>),
    /// `e.?` -- trap if `e` is null.
    Unwrap(Box<Expr>),
}

/// Where a call goes.
///
/// Inference resolves a call statically whenever the argument types pin a
/// single overload, which is the common case; `Dynamic` is only reached when an
/// argument's static type is a proper supertype of what can arrive at runtime.
#[derive(Debug, Clone)]
pub enum Callee {
    /// A known top-level function. `targs` are the type arguments this call
    /// instantiates it at, and are empty for a non-generic callee.
    Static {
        func: FuncId,
        targs: Vec<Type>,
    },
    Builtin(BuiltinId),
    /// A call through a closure value.
    Indirect(Box<Expr>),
    /// `w.f(..)` -- a call into another worker's service. The arguments are
    /// copied there and the result copied back, so it can fail: the type is
    /// `!R` whatever the method returns.
    Rpc {
        worker: Box<Expr>,
        service: ServiceId,
        /// Index into the service's `methods`.
        method: u32,
    },
    /// Multiple dispatch. `cases` are in specificity order, most specific
    /// first, so the first one whose runtime types match is the winner --
    /// which is exactly Julia's rule, decided at compile time and then simply
    /// read off at run time.
    ///
    /// The table is stored inline rather than in a side table on `Program` so
    /// that monomorphisation specialises it per caller instantiation for free.
    Dynamic {
        cases: Vec<DispatchCase>,
    },
}

/// One row of a dispatch table.
#[derive(Debug, Clone)]
pub struct DispatchCase {
    /// Per argument, the type it must be an instance of, or `None` if the
    /// static type already guarantees a match and no test is needed. Code
    /// generation turns each into the half-open type-id range that preorder
    /// numbering guarantees the type's subtree occupies.
    pub params: Vec<Option<StructId>>,
    pub func: FuncId,
    /// Type arguments, as for [`Callee::Static`]; cleared by monomorphisation.
    pub targs: Vec<Type>,
}
