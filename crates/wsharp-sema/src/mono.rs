//! Monomorphisation: turn a program with generic functions into one where
//! every function has a concrete type.
//!
//! Unboxed values mean a `fn(T) T` cannot be compiled once and shared -- `T`
//! could be an `i64` in a register or a pointer to a heap object. So inference
//! records, at every call site, the type arguments that use instantiates the
//! callee at, and this pass walks the call graph from `main` emitting one
//! specialised copy per distinct instantiation.
//!
//! Two useful side effects: functions never reached from `main` are dropped,
//! and after this pass no type in the program contains a variable -- which is
//! precisely the invariant the code generator needs.

use std::collections::{HashMap, HashSet};

use wsharp_syntax::{Diagnostic, Span};

use crate::hir;
use crate::ty::{Scheme, TyCon, Type, TypeStore, TypeVarId};

/// A mapping from a function's quantified variables to concrete types.
type Subst = HashMap<TypeVarId, Type>;

/// Identifies one specialisation: the original function plus the types it was
/// instantiated at.
type Key = (hir::FuncId, Vec<(TypeVarId, String)>);

pub struct MonoResult {
    pub program: hir::Program,
    pub diags: Vec<Diagnostic>,
}

/// Specialise `program` starting from its entry point.
///
/// Returns the program unchanged if it has no entry point -- there is nothing
/// to start a reachability walk from, and `wsharp check` does not need this
/// pass at all.
pub fn monomorphize(program: &hir::Program, store: &mut TypeStore) -> MonoResult {
    let Some(entry) = program.entry else {
        return MonoResult {
            program: clone_program(program),
            diags: Vec::new(),
        };
    };

    let mut mono = Mono {
        src: program,
        store,
        out: Vec::new(),
        cache: HashMap::new(),
        pending: Vec::new(),
        diags: Vec::new(),
        current: None,
        services: program.services.clone(),
        spawned: HashSet::new(),
    };

    let new_entry = mono.specialize(entry, Subst::new());
    while let Some((src_id, new_id, subst)) = mono.pending.pop() {
        mono.build(src_id, new_id, &subst);
    }

    let funcs = mono
        .out
        .into_iter()
        .map(|f| f.expect("every queued specialisation was built"))
        .collect();

    MonoResult {
        program: hir::Program {
            structs: program.structs.clone(),
            funcs,
            strings: program.strings.clone(),
            arrays: program.arrays.clone(),
            errors: program.errors.clone(),
            entry: Some(new_entry),
            services: mono.services,
        },
        diags: mono.diags,
    }
}

fn clone_program(p: &hir::Program) -> hir::Program {
    hir::Program {
        structs: p.structs.clone(),
        funcs: p.funcs.clone(),
        strings: p.strings.clone(),
        arrays: p.arrays.clone(),
        errors: p.errors.clone(),
        entry: p.entry,
        services: p.services.clone(),
    }
}

struct Mono<'a> {
    src: &'a hir::Program,
    store: &'a mut TypeStore,
    out: Vec<Option<hir::FuncDef>>,
    cache: HashMap<Key, hir::FuncId>,
    pending: Vec<(hir::FuncId, hir::FuncId, Subst)>,
    diags: Vec<Diagnostic>,
    /// The function being specialised: its name, where to point a diagnostic,
    /// and whether one has already been reported for it.
    current: Option<(String, Span, bool)>,
    /// The service table, with each entry's functions renumbered as they are
    /// specialised. A service's methods are reachable only through this table,
    /// so `@spawn` is the edge that keeps them alive.
    services: Vec<hir::ServiceDef>,
    spawned: HashSet<hir::ServiceId>,
}

impl Mono<'_> {
    /// Get the id of `src_id` specialised at `subst`, queueing the work if this
    /// combination has not been seen.
    fn specialize(&mut self, src_id: hir::FuncId, subst: Subst) -> hir::FuncId {
        let key = (src_id, self.key_of(&subst));
        if let Some(&id) = self.cache.get(&key) {
            return id;
        }
        let new_id = self.out.len() as hir::FuncId;
        self.out.push(None);
        self.cache.insert(key, new_id);
        self.pending.push((src_id, new_id, subst));
        new_id
    }

    fn key_of(&mut self, subst: &Subst) -> Vec<(TypeVarId, String)> {
        let mut pairs: Vec<(TypeVarId, String)> = subst
            .iter()
            .map(|(var, ty)| {
                let ty = ty.clone();
                (*var, self.store.show(&ty))
            })
            .collect();
        pairs.sort();
        pairs
    }

    fn build(&mut self, src_id: hir::FuncId, new_id: hir::FuncId, subst: &Subst) {
        let mut def = self.src.funcs[src_id as usize].clone();
        self.current = Some((def.name.clone(), def.span, false));

        def.ret = self.apply(&def.ret, subst);
        for i in 0..def.locals.len() {
            // Through `apply` rather than open-coded, so that a local's type
            // gets the same treatment a return type does -- including having
            // its error sets closed, which the `e` a `catch |e|` binds needs.
            let ty = def.locals[i].ty.clone();
            def.locals[i].ty = self.apply(&ty, subst);
        }

        let mut body = std::mem::take(&mut def.body);
        self.rewrite_block(&mut body, subst);
        def.body = body;

        // Everything is concrete now, so the scheme is no longer generic.
        let params: Vec<Type> = def
            .params
            .iter()
            .map(|p| def.locals[*p as usize].ty.clone())
            .collect();
        def.scheme = Scheme::mono(Type::func(params, def.ret.clone()));

        self.current = None;
        self.out[new_id as usize] = Some(def);
    }

    /// After substitution nothing should still be a type variable -- code
    /// generation cannot pick a machine representation for one. If something
    /// is, the program used a generic function at a type nothing pinned down.
    ///
    /// Every type in the function passes through here, including the types of
    /// intermediate expressions, so the invariant holds for the whole body and
    /// not just its signature.
    fn note_if_unresolved(&mut self, ty: &Type) {
        let already_reported = matches!(self.current, Some((_, _, true)) | None);
        if already_reported || !self.store.has_unbound(ty) {
            return;
        }
        let shown = self.store.show(ty);
        let (name, span, reported) = self.current.as_mut().expect("inside a function");
        *reported = true;
        let name = name.clone();
        let span = *span;

        let mut diag = Diagnostic::error(
            span,
            format!("cannot tell what type `{name}` is being used at"),
        );
        diag.primary.message = format!("this involves the unresolved type `{shown}`");
        diag.help = Some(
            "add a type annotation at the call site so the generic function can be specialised"
                .into(),
        );
        self.diags.push(diag);
    }

    fn apply(&mut self, ty: &Type, subst: &Subst) -> Type {
        let resolved = self.store.resolve_deep(ty);
        let out = if subst.is_empty() {
            resolved
        } else {
            self.store.subst_vars(&resolved, subst)
        };
        let out = self.close_error_sets(&out);
        self.note_if_unresolved(&out);
        out
    }

    /// Give every error set nothing decided its honest answer: the empty set.
    ///
    /// A set left open by the solver is one no `error.X` and no `try` ever
    /// reached, in this function or at this call site. That is a function that
    /// raises nothing, not a function whose errors are unknown -- so it is
    /// closed here rather than reported as an unresolved type, which is what
    /// `note_if_unresolved` would otherwise do to every infallible `!T`.
    fn close_error_sets(&mut self, ty: &Type) -> Type {
        match self.store.resolve(ty) {
            // Standing alone -- as the type argument of a call to a function
            // whose set nothing pinned -- as well as in place.
            Type::Var(v) if self.store.is_err_set_var(v) => self.store.err_set(Vec::new()),
            Type::Var(_) => ty.clone(),
            Type::Con(con, args) => {
                let args = args.iter().map(|a| self.close_error_sets(a)).collect();
                Type::Con(con, args)
            }
        }
    }

    /// Emit a service's `init` and every one of its methods, once.
    ///
    /// None of them is generic -- a worker calls a method through one machine
    /// implementation -- so the substitution is empty and the only work is
    /// renumbering the table to point at the copies.
    fn specialize_service(&mut self, service: hir::ServiceId) {
        if !self.spawned.insert(service) {
            return;
        }
        let def = self.services[service as usize].clone();
        let init = self.specialize(def.init, Subst::new());
        let methods = def
            .methods
            .iter()
            .map(|m| hir::ServiceMethod {
                name: m.name.clone(),
                func: self.specialize(m.func, Subst::new()),
            })
            .collect();
        self.services[service as usize] = hir::ServiceDef {
            name: def.name,
            init,
            methods,
        };
    }

    /// The substitution to specialise `callee` under, given the type arguments
    /// recorded at the call site and the caller's own substitution.
    fn callee_subst(&mut self, callee: hir::FuncId, targs: &[Type], caller: &Subst) -> Subst {
        let vars = self.src.funcs[callee as usize].scheme.vars.clone();
        // A `fn` literal's body refers to the enclosing function's variables as
        // well as to any of its own, so a closure composes the two rather than
        // replacing one with the other: the caller's substitution first, then
        // whatever this use instantiates the literal's own quantified variables
        // at. A monomorphic literal has none, and inherits the caller's
        // substitution exactly as it always did.
        //
        // Composing is also what keeps the cache key right. It is the whole
        // map, so one generic literal used at two types inside one enclosing
        // instantiation gets two keys, and one used at one type inside two
        // enclosing instantiations still gets two.
        let mut subst = if self.src.funcs[callee as usize].is_closure {
            caller.clone()
        } else {
            Subst::new()
        };
        for (var, ty) in vars.iter().zip(targs) {
            let concrete = self.apply(ty, caller);
            subst.insert(*var, concrete);
        }
        subst
    }

    // ---- traversal ------------------------------------------------------

    fn rewrite_block(&mut self, block: &mut hir::Block, subst: &Subst) {
        for stmt in &mut block.stmts {
            self.rewrite_stmt(stmt, subst);
        }
    }

    fn rewrite_stmt(&mut self, stmt: &mut hir::Stmt, subst: &Subst) {
        match stmt {
            hir::Stmt::Let { init, .. } => self.rewrite_expr(init, subst),
            hir::Stmt::Assign { place, value } => {
                match place {
                    hir::Place::Field { obj, .. } => self.rewrite_expr(obj, subst),
                    hir::Place::Index { arr, index } => {
                        self.rewrite_expr(arr, subst);
                        self.rewrite_expr(index, subst);
                    }
                    hir::Place::Local(_) => {}
                }
                self.rewrite_expr(value, subst);
            }
            hir::Stmt::Expr(e) => self.rewrite_expr(e, subst),
            hir::Stmt::Return(Some(e)) => self.rewrite_expr(e, subst),
            hir::Stmt::Return(None) | hir::Stmt::Break | hir::Stmt::Continue => {}
            hir::Stmt::If {
                cond, then, els, ..
            } => {
                self.rewrite_expr(cond, subst);
                self.rewrite_block(then, subst);
                if let Some(els) = els {
                    self.rewrite_block(els, subst);
                }
            }
            hir::Stmt::While {
                cond, cont, body, ..
            } => {
                self.rewrite_expr(cond, subst);
                self.rewrite_block(body, subst);
                if let Some(cont) = cont {
                    self.rewrite_stmt(cont, subst);
                }
            }
            hir::Stmt::Block(b) => self.rewrite_block(b, subst),
        }
    }

    fn rewrite_expr(&mut self, expr: &mut hir::Expr, subst: &Subst) {
        expr.ty = self.apply(&expr.ty, subst);
        // An integer literal used at an `f64` *is* a float literal. It takes
        // the type its context asks for, exactly as `0xff` is a `u8` here and a
        // `u32` there, and this is where that type is finally settled: a body
        // annotated `Number` does not know which member it will be compiled at
        // until its instantiation is substituted in, a line above. Inference
        // has already refused a value an `f64` cannot hold exactly.
        if let hir::ExprKind::Int(v) = expr.kind
            && matches!(self.store.resolve(&expr.ty), Type::Con(TyCon::F64, _))
        {
            expr.kind = hir::ExprKind::Float(v as f64);
        }
        match &mut expr.kind {
            // Spawning is what makes a service's functions reachable at all --
            // nothing calls them, the runtime does.
            hir::ExprKind::Spawn { service, args } => {
                self.specialize_service(*service);
                for arg in args {
                    self.rewrite_expr(arg, subst);
                }
            }
            hir::ExprKind::Join(worker) => self.rewrite_expr(worker, subst),
            hir::ExprKind::Block { stmts, value } => {
                for stmt in stmts {
                    self.rewrite_stmt(stmt, subst);
                }
                if let Some(v) = value {
                    self.rewrite_expr(v, subst);
                }
            }
            hir::ExprKind::Call { callee, args } => {
                match callee {
                    // The callee is chosen at run time by the worker's own
                    // dispatch table, which `specialize_service` rewrote.
                    hir::Callee::Rpc { worker, .. } => self.rewrite_expr(worker, subst),
                    hir::Callee::Static { func, targs } => {
                        let inner = self.callee_subst(*func, targs, subst);
                        *func = self.specialize(*func, inner);
                        targs.clear();
                    }
                    hir::Callee::Dynamic { cases } => {
                        // Every case must be specialised: a table entry is the
                        // only thing keeping an overload reachable, and an
                        // unreached function is dropped.
                        for case in cases {
                            let inner = self.callee_subst(case.func, &case.targs, subst);
                            case.func = self.specialize(case.func, inner);
                            case.targs.clear();
                        }
                    }
                    hir::Callee::Indirect(inner) => self.rewrite_expr(inner, subst),
                    hir::Callee::Builtin(_) => {}
                }
                for arg in args {
                    self.rewrite_expr(arg, subst);
                }
            }
            hir::ExprKind::Closure {
                func,
                targs,
                captures,
            } => {
                let inner = self.callee_subst(*func, targs, subst);
                *func = self.specialize(*func, inner);
                targs.clear();
                for c in captures {
                    self.rewrite_expr(c, subst);
                }
            }
            hir::ExprKind::Field { obj, .. } => self.rewrite_expr(obj, subst),
            hir::ExprKind::Convert(expr)
            | hir::ExprKind::Unary { expr, .. }
            | hir::ExprKind::Some(expr)
            | hir::ExprKind::Ok(expr)
            | hir::ExprKind::Raise(expr)
            | hir::ExprKind::Try(expr)
            | hir::ExprKind::Unwrap(expr) => self.rewrite_expr(expr, subst),
            hir::ExprKind::Binary { lhs, rhs, .. } | hir::ExprKind::Logical { lhs, rhs, .. } => {
                self.rewrite_expr(lhs, subst);
                self.rewrite_expr(rhs, subst);
            }
            hir::ExprKind::Orelse { expr, alt } | hir::ExprKind::Catch { expr, alt, .. } => {
                self.rewrite_expr(expr, subst);
                self.rewrite_expr(alt, subst);
            }
            hir::ExprKind::StructNew { fields, .. } => {
                for f in fields {
                    self.rewrite_expr(f, subst);
                }
            }
            hir::ExprKind::ArrayNew { elems } => {
                for e in elems {
                    self.rewrite_expr(e, subst);
                }
            }
            hir::ExprKind::Index { arr, index } => {
                self.rewrite_expr(arr, subst);
                self.rewrite_expr(index, subst);
            }
            hir::ExprKind::ArrayLen { arr } => self.rewrite_expr(arr, subst),
            hir::ExprKind::If {
                cond, then, els, ..
            } => {
                self.rewrite_expr(cond, subst);
                self.rewrite_expr(then, subst);
                self.rewrite_expr(els, subst);
            }
            hir::ExprKind::Int(_)
            | hir::ExprKind::Float(_)
            | hir::ExprKind::Bool(_)
            | hir::ExprKind::Str(_)
            | hir::ExprKind::ArrayConst(_)
            | hir::ExprKind::Null
            | hir::ExprKind::Local(_)
            | hir::ExprKind::Singleton(_)
            | hir::ExprKind::Err(_) => {}
        }
    }
}
