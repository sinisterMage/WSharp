//! Structural equality on a struct, generated once per type.
//!
//! `==` on a struct compares its fields, and a struct field recurses -- so the
//! comparison is a *function* rather than an inline sequence, because a type
//! that reaches itself would otherwise inline for ever. Two per type:
//!
//! * **`eq$n`** is what a comparison calls. It answers the questions that are
//!   about the pair rather than about the fields -- the same object, a null, two
//!   different concrete types -- and then dispatches on the runtime type id, so
//!   that two `Sub`s compared at `Base` compare `Sub`'s fields and not just the
//!   part `Base` declares.
//! * **`eqx$n`** is the comparison of one exact type's fields, reached only when
//!   both values are known to be that type.
//!
//! Emitted here rather than turned into a library call by inference, for the
//! reason `==` on a `str` already is: nothing in inference synthesises a call,
//! and this is the only place that would. Emitted as *generated code* rather
//! than written in Rust for the reason `array.concat` had to be: it reads
//! references out of objects, and generated code goes through the load barrier
//! and the stack maps by construction.
//!
//! **A value that reaches itself recurses for ever.** `a == a` stops at the
//! identity test, but `a.next = a; b.next = b; a == b` does not, and nothing
//! here detects it. That is what derived structural equality does in every
//! language that has it, and it is said out loud rather than papered over.

use std::collections::HashMap;

use cranelift_codegen::ir::{AbiParam, Signature, types};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module};

use wsharp_sema::hir;
use wsharp_sema::ty::{StructId, TyCon, Type, TypeStore};

use crate::lower::{self, Decls};
use crate::repr::PTR;
use crate::{CodegenError, HarvestedCode, err, harvest_stack_maps};

/// The pair of functions one struct type is compared by.
#[derive(Clone, Copy)]
pub struct EqFns {
    /// The entry point a `==` calls: identity, nulls, type ids, then dispatch.
    pub dispatch: FuncId,
    /// One exact type's fields.
    pub exact: FuncId,
}

/// Every struct type this program compares with `==`, and everything a
/// comparison of one will reach.
///
/// Keyed by [`TypeStore::show`], which is what already distinguishes one
/// instantiation of a generic struct from another in `instance_type_ids`.
#[derive(Default)]
pub struct Equality {
    /// In discovery order, which is also the order they are declared and
    /// defined in. The key is the type's rendering.
    pub types: Vec<Type>,
    pub index: HashMap<String, usize>,
    /// Parallel to `types`, and empty until [`declare`] has run.
    pub fns: Vec<EqFns>,
}

impl Equality {
    fn add(&mut self, store: &mut TypeStore, ty: &Type) -> bool {
        let key = store.show(ty);
        if self.index.contains_key(&key) {
            return false;
        }
        self.index.insert(key, self.types.len());
        self.types.push(ty.clone());
        true
    }

    pub fn get(&self, store: &mut TypeStore, ty: &Type) -> Option<usize> {
        self.index.get(&store.show(ty)).copied()
    }
}

/// Walk the program for `==` on a struct, and close the answer under the field
/// types and the subtypes a comparison will reach.
///
/// The closure over *subtypes* is what the dispatch in `eq$n` needs: a `==`
/// written at `Base` may be handed two `Sub`s, and then it is `Sub`'s exact
/// comparison that runs. Inference has already checked that every one of those
/// fields is comparable, so nothing here has to report anything.
pub fn plan(program: &hir::Program, store: &mut TypeStore) -> Equality {
    let mut plan = Equality::default();
    let mut roots: Vec<Type> = Vec::new();
    for func in &program.funcs {
        collect_block(&func.body, store, &mut roots);
    }
    for ty in roots {
        close(program, store, &mut plan, &ty);
    }
    plan
}

/// Add `ty` and everything reachable from it: its subtypes, and their fields.
fn close(program: &hir::Program, store: &mut TypeStore, plan: &mut Equality, ty: &Type) {
    let Type::Con(TyCon::Struct(id), args) = store.resolve(ty) else {
        return;
    };
    if !plan.add(store, ty) {
        return;
    }
    // The whole cone, because the dispatch picks by runtime type. A generic
    // struct stands outside the lattice, so its instantiation's cone is itself.
    let cone: Vec<StructId> = if args.is_empty() {
        (0..program.structs.len() as StructId)
            .filter(|&other| is_within(program, other, id))
            .collect()
    } else {
        vec![id]
    };
    for member in cone {
        let member_ty = if member == id {
            ty.clone()
        } else {
            Type::Con(TyCon::Struct(member), Vec::new())
        };
        if member != id {
            plan.add(store, &member_ty);
        }
        let fields = crate::instance_field_types(store, program.strukt(member), &member_ty);
        for field in fields {
            close_field(program, store, plan, &field);
        }
    }
}

fn close_field(program: &hir::Program, store: &mut TypeStore, plan: &mut Equality, ty: &Type) {
    match store.resolve(ty) {
        Type::Con(TyCon::Optional, inner) => close_field(program, store, plan, &inner[0]),
        Type::Con(TyCon::Struct(_), _) => close(program, store, plan, ty),
        _ => {}
    }
}

/// Whether `sub` is `sup` or below it, by the declared parent chain.
///
/// The parent links rather than the type-id range, because both answers are the
/// same here and only one of them needs a lookup table.
fn is_within(program: &hir::Program, sub: StructId, sup: StructId) -> bool {
    let mut at = Some(sub);
    while let Some(id) = at {
        if id == sup {
            return true;
        }
        at = program.strukt(id).parent;
    }
    false
}

// ---------------------------------------------------------------------------
// Finding the comparisons
// ---------------------------------------------------------------------------

fn collect_block(block: &hir::Block, store: &mut TypeStore, out: &mut Vec<Type>) {
    for stmt in &block.stmts {
        collect_stmt(stmt, store, out);
    }
}

fn collect_stmt(stmt: &hir::Stmt, store: &mut TypeStore, out: &mut Vec<Type>) {
    match stmt {
        hir::Stmt::Let { init, .. } => collect_expr(init, store, out),
        hir::Stmt::Assign { place, value } => {
            collect_place(place, store, out);
            collect_expr(value, store, out);
        }
        hir::Stmt::Expr(e) => collect_expr(e, store, out),
        hir::Stmt::Return(e) => {
            if let Some(e) = e {
                collect_expr(e, store, out);
            }
        }
        hir::Stmt::If {
            cond, then, els, ..
        } => {
            collect_expr(cond, store, out);
            collect_block(then, store, out);
            if let Some(els) = els {
                collect_block(els, store, out);
            }
        }
        hir::Stmt::While {
            cond, cont, body, ..
        } => {
            collect_expr(cond, store, out);
            if let Some(cont) = cont {
                collect_stmt(cont, store, out);
            }
            collect_block(body, store, out);
        }
        hir::Stmt::Block(b) => collect_block(b, store, out),
        hir::Stmt::Break | hir::Stmt::Continue => {}
    }
}

fn collect_place(place: &hir::Place, store: &mut TypeStore, out: &mut Vec<Type>) {
    match place {
        hir::Place::Local(_) => {}
        hir::Place::Field { obj, .. } => collect_expr(obj, store, out),
        hir::Place::Index { arr, index } => {
            collect_expr(arr, store, out);
            collect_expr(index, store, out);
        }
    }
}

fn collect_expr(expr: &hir::Expr, store: &mut TypeStore, out: &mut Vec<Type>) {
    use hir::ExprKind as E;
    match &expr.kind {
        E::Binary { op, lhs, rhs } => {
            if matches!(
                op,
                wsharp_syntax::ast::BinOp::Eq | wsharp_syntax::ast::BinOp::Ne
            ) && matches!(store.resolve(&lhs.ty), Type::Con(TyCon::Struct(_), _))
            {
                out.push(lhs.ty.clone());
            }
            collect_expr(lhs, store, out);
            collect_expr(rhs, store, out);
        }
        E::Int(_)
        | E::Float(_)
        | E::Bool(_)
        | E::Str(_)
        | E::ArrayConst(_)
        | E::Null
        | E::Local(_)
        | E::Err(_)
        | E::Singleton(_) => {}
        E::Convert(e)
        | E::Unary { expr: e, .. }
        | E::ArrayLen { arr: e }
        | E::Field { obj: e, .. }
        | E::Some(e)
        | E::Ok(e)
        | E::Raise(e)
        | E::Join(e)
        | E::Try(e)
        | E::Unwrap(e) => collect_expr(e, store, out),
        E::Logical { lhs, rhs, .. }
        | E::Index {
            arr: lhs,
            index: rhs,
        }
        | E::Orelse {
            expr: lhs,
            alt: rhs,
        }
        | E::Catch {
            expr: lhs,
            alt: rhs,
            ..
        } => {
            collect_expr(lhs, store, out);
            collect_expr(rhs, store, out);
        }
        E::Call { callee, args } => {
            if let hir::Callee::Indirect(e) = callee {
                collect_expr(e, store, out);
            }
            if let hir::Callee::Rpc { worker, .. } = callee {
                collect_expr(worker, store, out);
            }
            for a in args {
                collect_expr(a, store, out);
            }
        }
        E::ArrayNew { elems } => {
            for e in elems {
                collect_expr(e, store, out);
            }
        }
        E::StructNew { fields, .. } => {
            for e in fields {
                collect_expr(e, store, out);
            }
        }
        E::If {
            cond, then, els, ..
        } => {
            collect_expr(cond, store, out);
            collect_expr(then, store, out);
            collect_expr(els, store, out);
        }
        E::Closure { captures, .. } => {
            for e in captures {
                collect_expr(e, store, out);
            }
        }
        E::Block { stmts, value } => {
            for s in stmts {
                collect_stmt(s, store, out);
            }
            if let Some(v) = value {
                collect_expr(v, store, out);
            }
        }
        E::Spawn { args, .. } => {
            for e in args {
                collect_expr(e, store, out);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Declaring and defining
// ---------------------------------------------------------------------------

/// `fn(a, b) -> bool`. No environment pointer: these are the code generator's
/// own functions rather than W#'s, and nothing calls one through a closure.
pub fn signature(call_conv: cranelift_codegen::isa::CallConv) -> Signature {
    let mut sig = Signature::new(call_conv);
    sig.params.push(AbiParam::new(PTR));
    sig.params.push(AbiParam::new(PTR));
    sig.returns.push(AbiParam::new(types::I8));
    sig
}

pub fn declare<M: Module>(
    module: &mut M,
    plan: &mut Equality,
    call_conv: cranelift_codegen::isa::CallConv,
) -> Result<(), CodegenError> {
    let sig = signature(call_conv);
    let mut out = Vec::with_capacity(plan.types.len());
    for n in 0..plan.types.len() {
        let dispatch = module
            .declare_function(&format!("wsharp$eq{n}"), Linkage::Local, &sig)
            .map_err(|e| err("could not declare a struct comparison", e))?;
        let exact = module
            .declare_function(&format!("wsharp$eqx{n}"), Linkage::Local, &sig)
            .map_err(|e| err("could not declare a struct comparison", e))?;
        out.push(EqFns { dispatch, exact });
    }
    plan.fns = out;
    Ok(())
}

/// Emit both functions for every type in the plan.
///
/// After the program's own functions, so that everything they call is declared
/// -- and they call each other, which is why declaring and defining are two
/// passes rather than one.
#[allow(clippy::too_many_arguments)]
pub fn define<M: Module>(
    module: &mut M,
    program: &hir::Program,
    store: &mut TypeStore,
    decls: &Decls,
    ctx: &mut cranelift_codegen::Context,
    fb_ctx: &mut FunctionBuilderContext,
    harvested: &mut Vec<(FuncId, HarvestedCode)>,
    clif: &mut Option<&mut String>,
) -> Result<(), CodegenError> {
    let call_conv = module.target_config().default_call_conv;
    for n in 0..decls.equality.types.len() {
        let ty = decls.equality.types[n].clone();
        let shown = store.show(&ty);
        for exact in [false, true] {
            let fns = decls.equality.fns[n];
            let clif_id = if exact { fns.exact } else { fns.dispatch };
            ctx.func.signature = signature(call_conv);
            ctx.func.name = cranelift_codegen::ir::UserFuncName::user(0, clif_id.as_u32());
            {
                let builder = FunctionBuilder::new(&mut ctx.func, fb_ctx);
                lower::equality(builder, module, program, store, decls, &ty, exact);
            }
            if let Some(clif) = clif.as_deref_mut() {
                let what = if exact { "exact" } else { "dispatch" };
                clif.push_str(&format!("; {shown} == {shown} ({what})\n{}\n", ctx.func));
            }
            module
                .define_function(clif_id, ctx)
                .map_err(|e| err(&format!("could not compile `{shown}` equality"), e))?;
            harvested.push((clif_id, harvest_stack_maps(ctx)));
            module.clear_context(ctx);
        }
    }
    Ok(())
}
