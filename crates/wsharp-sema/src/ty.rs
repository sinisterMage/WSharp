//! Types, the union-find substitution, and unification.
//!
//! Type variables live in a single arena and are unified destructively. Each
//! unbound variable carries a *level* (Rémy's scheme): the depth of `let`
//! nesting at which it was created. Generalising at level `n` quantifies
//! exactly those variables whose level is greater than `n` -- that is, the ones
//! created inside the binding and not captured by anything outside it. It makes
//! generalisation a cheap traversal of the type instead of a scan of the whole
//! environment.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;

pub type TypeVarId = u32;
pub type StructId = u32;
/// Index into [`TypeStore`]'s table of interned error sets.
pub type ErrSetId = u32;
/// Index into [`TypeStore`]'s table of error names, and what a `!T`'s runtime
/// tag is (plus one, so that zero can mean success).
pub type ErrorId = u32;
/// Index into [`wsharp_runtime::builtins::abstract_types`].
pub type AbstractId = u32;

/// A type. Everything is either a variable or a constructor applied to
/// arguments -- there is no separate case for functions or optionals, so
/// arrays and user generics (a later session) need no change here.
#[derive(Clone, Debug, PartialEq)]
pub enum Type {
    Var(TypeVarId),
    Con(TyCon, Vec<Type>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TyCon {
    I64,
    F64,
    Bool,
    Void,
    Str,
    /// Arguments are the parameter types followed by the return type, so the
    /// arity is `args.len() - 1`.
    Fn,
    /// `?T`, one argument.
    Optional,
    /// `!T`: two arguments, the payload and the set of errors it may carry.
    ///
    /// The set is an ordinary type argument rather than part of the
    /// constructor, so that `unify` needs no special case: two error unions
    /// unify by unifying their sets, which merges two variables or binds one
    /// to a written set. Widening a *smaller* set into a larger one is
    /// `coerce`'s job, exactly as widening a subtype into its supertype is,
    /// and for the same reason -- by the time anything asks, unification has
    /// already bound whichever side was a variable.
    ErrUnion,
    /// The type of an error value, as bound by `catch |e|`. One argument: the
    /// set it can be drawn from.
    Error,
    /// A set of error names, interned. Appears only as the second argument of
    /// [`TyCon::ErrUnion`] and the argument of [`TyCon::Error`], so nothing
    /// that walks a value's shape ever meets one.
    ErrorSet(ErrSetId),
    /// `[]T`, one argument. A heap object whose header holds the element
    /// count; the elements follow it inline, as a string's bytes do.
    Array,
    Struct(StructId),
    /// A name for a set of concrete types, such as `Number`. It classifies
    /// values for dispatch and is never the type of one, so it appears only in
    /// a parameter's declared type -- inference replaces it with a variable in
    /// the type it actually checks against. See [`abstract_members`].
    Abstract(AbstractId),
}

impl Type {
    pub const I64: Type = Type::Con(TyCon::I64, Vec::new());

    pub fn prim(con: TyCon) -> Type {
        Type::Con(con, Vec::new())
    }

    pub fn i64() -> Type {
        Type::prim(TyCon::I64)
    }
    pub fn f64() -> Type {
        Type::prim(TyCon::F64)
    }
    pub fn bool() -> Type {
        Type::prim(TyCon::Bool)
    }
    pub fn void() -> Type {
        Type::prim(TyCon::Void)
    }
    pub fn str() -> Type {
        Type::prim(TyCon::Str)
    }
    /// The type `catch |e|` binds, over a set of errors.
    pub fn error(set: Type) -> Type {
        Type::Con(TyCon::Error, vec![set])
    }

    pub fn func(mut params: Vec<Type>, ret: Type) -> Type {
        params.push(ret);
        Type::Con(TyCon::Fn, params)
    }

    /// `[]T`.
    pub fn array(elem: Type) -> Type {
        Type::Con(TyCon::Array, vec![elem])
    }

    pub fn optional(inner: Type) -> Type {
        Type::Con(TyCon::Optional, vec![inner])
    }

    /// `!T` over a named set of errors.
    pub fn err_union(inner: Type, set: Type) -> Type {
        Type::Con(TyCon::ErrUnion, vec![inner, set])
    }

    pub fn strukt(id: StructId) -> Type {
        Type::Con(TyCon::Struct(id), Vec::new())
    }

    pub fn abstrakt(id: AbstractId) -> Type {
        Type::Con(TyCon::Abstract(id), Vec::new())
    }

    /// The type a builtin signature's type stands for. The runtime describes
    /// its own tables with a small enum of its own, so that it stays a leaf
    /// crate; this is the single place that translates it.
    /// A builtin's type, with `Var(n)` resolved through `vars`.
    ///
    /// The caller supplies the map so that one use of a generic builtin binds
    /// each variable once across its whole signature, and the next use gets
    /// fresh ones -- which is instantiation, done where the store is.
    pub fn from_builtin_with(
        t: wsharp_runtime::BuiltinTy,
        store: &mut TypeStore,
        vars: &mut std::collections::HashMap<u8, Type>,
    ) -> Type {
        use wsharp_runtime::BuiltinTy as B;
        match t {
            B::Array(inner) => Type::array(Type::from_builtin_with(*inner, store, vars)),
            B::Optional(inner) => Type::optional(Type::from_builtin_with(*inner, store, vars)),
            B::ErrUnion(inner, errors) => {
                let ids: Vec<ErrorId> = errors.iter().map(|e| store.intern_error(e)).collect();
                let set = store.err_set(ids);
                Type::err_union(Type::from_builtin_with(*inner, store, vars), set)
            }
            B::Var(n) => vars.entry(n).or_insert_with(|| store.fresh()).clone(),
            simple => Type::from_builtin(simple),
        }
    }

    /// The types a builtin signature can mention without a store: the scalars.
    ///
    /// # Panics
    /// On a constructor that needs one. [`Type::from_builtin_with`] is the
    /// general form.
    pub fn from_builtin(t: wsharp_runtime::BuiltinTy) -> Type {
        use wsharp_runtime::BuiltinTy as B;
        match t {
            B::Array(_) | B::Optional(_) | B::ErrUnion(..) | B::Var(_) => {
                unreachable!("`{t:?}` needs a type store; use `from_builtin_with`")
            }
            B::I64 => Type::i64(),
            B::F64 => Type::f64(),
            B::Bool => Type::bool(),
            B::Void => Type::void(),
            B::Str => Type::str(),
        }
    }

    /// For a function type, its parameter types and return type.
    pub fn as_fn(&self) -> Option<(&[Type], &Type)> {
        match self {
            Type::Con(TyCon::Fn, args) => args.split_last().map(|(ret, params)| (params, ret)),
            _ => None,
        }
    }

    pub fn is_numeric(&self) -> bool {
        matches!(self, Type::Con(TyCon::I64 | TyCon::F64, _))
    }
}

/// The abstract type of this name, if there is one.
///
/// Abstract types come from a table in the runtime rather than from the
/// program, so this is a lookup rather than a scope: a struct of the same name
/// shadows it, which is why every caller consults the struct table first.
pub fn lookup_abstract(name: &str) -> Option<AbstractId> {
    let table = wsharp_runtime::builtins::abstract_types();
    table.iter().position(|(n, _)| *n == name).map(|i| i as u32)
}

pub fn abstract_name(id: AbstractId) -> &'static str {
    wsharp_runtime::builtins::abstract_types()[id as usize].0
}

/// The concrete types an abstract type classifies, in table order. The first
/// is the one a program means when it says nothing more.
pub fn abstract_members(id: AbstractId) -> Vec<Type> {
    wsharp_runtime::builtins::abstract_types()[id as usize]
        .1
        .iter()
        .map(|t| Type::from_builtin(*t))
        .collect()
}

/// A generalised type: `vars` are universally quantified in `ty`.
#[derive(Clone, Debug, PartialEq)]
pub struct Scheme {
    pub vars: Vec<TypeVarId>,
    pub ty: Type,
}

impl Scheme {
    /// A type with nothing quantified.
    pub fn mono(ty: Type) -> Scheme {
        Scheme {
            vars: Vec::new(),
            ty,
        }
    }

    pub fn is_generic(&self) -> bool {
        !self.vars.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnifyError {
    /// The two types have different constructors.
    Mismatch,
    /// A variable would have to contain itself, e.g. `T = fn(T) T`.
    Occurs,
}

#[derive(Debug, Clone)]
enum Slot {
    Unbound { level: u32 },
    Bound(Type),
}

/// A point in the substitution's history that it can be rewound to.
#[derive(Debug, Clone, Copy)]
pub struct Snapshot(usize);

/// The substitution: an arena of type variables plus the current `let` level.
pub struct TypeStore {
    slots: Vec<Slot>,
    level: u32,
    /// Previous slot values, so a speculative unification can be undone. This
    /// is what makes [`TypeStore::try_unify`] possible: unification is
    /// destructive, and a failed attempt would otherwise leave variables
    /// half-bound.
    trail: Vec<(TypeVarId, Slot)>,
    /// Names for `TyCon::Struct` ids, so types can be printed.
    struct_names: Vec<String>,
    /// The declared supertype of each struct, parallel to `struct_names`. This
    /// is the dispatch lattice; `unify` deliberately does not consult it (see
    /// [`TypeStore::unify`]), only the constraint solver and overload selection
    /// do.
    struct_parents: Vec<Option<StructId>>,
    /// Error names, in declaration order. An error value is one of these
    /// indices, and its runtime tag is the index plus one.
    ///
    /// Kept here rather than beside the rest of inference because `show` needs
    /// them: a set is part of a type now, and a type has to be printable.
    error_names: Vec<String>,
    error_ids: HashMap<String, ErrorId>,
    /// Interned error sets, each sorted and deduplicated so that two spellings
    /// of the same set are the same id -- which is what lets `unify` compare
    /// two written sets with `==` and get the right answer.
    err_sets: Vec<Vec<ErrorId>>,
    err_set_ids: HashMap<Vec<ErrorId>, ErrSetId>,
    /// Variables standing for an error set rather than for a value's type.
    ///
    /// Worth knowing apart, because one nothing ever pins means "raises
    /// nothing" rather than "could not be worked out" -- so it is closed to
    /// the empty set instead of being reported. Instantiation carries the mark
    /// across, or a generic function's set would lose it at every call.
    err_set_vars: HashSet<TypeVarId>,
}

impl Default for TypeStore {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeStore {
    /// The id of an error name, interning it if this is the first mention.
    pub fn intern_error(&mut self, name: &str) -> ErrorId {
        if let Some(&id) = self.error_ids.get(name) {
            return id;
        }
        let id = self.error_names.len() as ErrorId;
        self.error_names.push(name.to_string());
        self.error_ids.insert(name.to_string(), id);
        id
    }

    /// Every error name, indexed by id -- what `hir::Program.errors` becomes.
    pub fn error_names(&self) -> &[String] {
        &self.error_names
    }

    pub fn error_name(&self, id: ErrorId) -> &str {
        self.error_names
            .get(id as usize)
            .map(String::as_str)
            .unwrap_or("?")
    }

    /// The type standing for a set of errors, interned so that two spellings
    /// of the same set are one id.
    pub fn err_set(&mut self, errors: impl IntoIterator<Item = ErrorId>) -> Type {
        let mut errors: Vec<ErrorId> = errors.into_iter().collect();
        errors.sort_unstable();
        errors.dedup();
        let id = match self.err_set_ids.get(&errors) {
            Some(&id) => id,
            None => {
                let id = self.err_sets.len() as ErrSetId;
                self.err_sets.push(errors.clone());
                self.err_set_ids.insert(errors, id);
                id
            }
        };
        Type::Con(TyCon::ErrorSet(id), Vec::new())
    }

    /// A fresh variable standing for an error set nothing has decided yet.
    pub fn fresh_err_set(&mut self) -> Type {
        let ty = self.fresh();
        if let Type::Var(v) = ty {
            self.err_set_vars.insert(v);
        }
        ty
    }

    pub fn is_err_set_var(&self, v: TypeVarId) -> bool {
        self.err_set_vars.contains(&v)
    }

    pub fn err_set_members(&self, id: ErrSetId) -> &[ErrorId] {
        self.err_sets.get(id as usize).map_or(&[], Vec::as_slice)
    }

    /// The members of a type that is a set, if it has been decided yet.
    pub fn as_err_set(&mut self, ty: &Type) -> Option<Vec<ErrorId>> {
        match self.resolve(ty) {
            Type::Con(TyCon::ErrorSet(id), _) => Some(self.err_set_members(id).to_vec()),
            _ => None,
        }
    }

    /// `{NotFound, IoFailed}`, or nothing at all for the empty set.
    fn set_text(&self, id: ErrSetId) -> String {
        let members = self.err_set_members(id);
        if members.is_empty() {
            return String::new();
        }
        let names: Vec<&str> = members.iter().map(|&e| self.error_name(e)).collect();
        format!("{{{}}}", names.join(", "))
    }

    pub fn new() -> TypeStore {
        TypeStore {
            slots: Vec::new(),
            level: 0,
            trail: Vec::new(),
            struct_names: Vec::new(),
            struct_parents: Vec::new(),
            error_names: Vec::new(),
            error_ids: HashMap::new(),
            err_sets: Vec::new(),
            err_set_ids: HashMap::new(),
            err_set_vars: HashSet::new(),
        }
    }

    fn set_slot(&mut self, var: TypeVarId, slot: Slot) {
        self.trail.push((var, self.slots[var as usize].clone()));
        self.slots[var as usize] = slot;
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot(self.trail.len())
    }

    /// Undo every substitution made since `snapshot`.
    pub fn rollback_to(&mut self, snapshot: Snapshot) {
        while self.trail.len() > snapshot.0 {
            let (var, old) = self.trail.pop().expect("trail is at least this long");
            self.slots[var as usize] = old;
        }
    }

    /// Unify, keeping the result on success and undoing it on failure.
    ///
    /// Used to test whether a value fits a type before falling back to a
    /// coercion, e.g. `return n;` in a function returning `!i64`.
    pub fn try_unify(&mut self, a: &Type, b: &Type) -> bool {
        self.try_unify_checked(a, b).is_ok()
    }

    /// [`TypeStore::try_unify`], but saying *why* it failed, so a caller that
    /// goes on to report the failure can tell an infinite type from a plain
    /// mismatch.
    pub fn try_unify_checked(&mut self, a: &Type, b: &Type) -> Result<(), UnifyError> {
        let snapshot = self.snapshot();
        let result = self.unify(a, b);
        if result.is_err() {
            self.rollback_to(snapshot);
        }
        result
    }

    pub fn declare_struct(&mut self, name: impl Into<String>) -> StructId {
        self.struct_names.push(name.into());
        self.struct_parents.push(None);
        (self.struct_names.len() - 1) as StructId
    }

    pub fn struct_name(&self, id: StructId) -> &str {
        self.struct_names
            .get(id as usize)
            .map_or("<struct>", |s| s.as_str())
    }

    pub fn struct_count(&self) -> usize {
        self.struct_names.len()
    }

    /// Record `parent` as `id`'s supertype. The caller is responsible for
    /// having rejected cycles first -- [`TypeStore::is_subtype`] walks the
    /// chain and would not terminate on one.
    pub fn set_struct_parent(&mut self, id: StructId, parent: StructId) {
        self.struct_parents[id as usize] = Some(parent);
    }

    /// Drop a supertype link. Used to break a reported cycle so that the
    /// chain walks below terminate.
    pub fn clear_struct_parent(&mut self, id: StructId) {
        self.struct_parents[id as usize] = None;
    }

    pub fn struct_parent(&self, id: StructId) -> Option<StructId> {
        self.struct_parents[id as usize]
    }

    /// True if `sub` is `sup` or inherits from it, transitively.
    pub fn is_subtype(&self, sub: StructId, sup: StructId) -> bool {
        let mut cur = Some(sub);
        while let Some(id) = cur {
            if id == sup {
                return true;
            }
            cur = self.struct_parents[id as usize];
        }
        false
    }

    /// Distance from `id` to a lattice root, used to order overloads by
    /// specificity: a deeper type is the more specific one.
    pub fn struct_depth(&self, id: StructId) -> u32 {
        let mut depth = 0;
        let mut cur = self.struct_parents[id as usize];
        while let Some(parent) = cur {
            depth += 1;
            cur = self.struct_parents[parent as usize];
        }
        depth
    }

    /// The subtype relation lifted to whole types. Structs have the declared
    /// lattice and abstract types have the table's; everything else is
    /// subtyping-by-equality, which keeps `?T`, `!T` and `fn` invariant and so
    /// avoids needing variance.
    ///
    /// This is the only place the lattice reaches past structs. An abstract
    /// type is above exactly the concrete types it lists, which is what lets
    /// specificity order `i64` under `Number` -- and it is a flat extension:
    /// the table has no nesting, so two abstract types are related only when
    /// they are the same one.
    pub fn is_sub_ty(&mut self, sub: &Type, sup: &Type) -> bool {
        match (self.resolve(sub), self.resolve(sup)) {
            // A generic struct stands outside the lattice, so its
            // instantiations are related only to themselves -- and only when
            // their arguments agree. `Box[i64]` is not a `Box[str]`.
            (Type::Con(TyCon::Struct(a), aa), Type::Con(TyCon::Struct(b), ba)) => {
                self.is_subtype(a, b) && self.args_agree(&aa, &ba)
            }
            (Type::Con(TyCon::Abstract(a), _), Type::Con(TyCon::Abstract(b), _)) => a == b,
            (a, Type::Con(TyCon::Abstract(id), _)) => abstract_members(id).contains(&a),
            (a, b) => a == b,
        }
    }

    /// Whether two argument lists are the same types.
    ///
    /// Deeply resolved, because either side may still hold a variable that has
    /// since been bound to the other's type.
    fn args_agree(&mut self, a: &[Type], b: &[Type]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(x, y)| {
                let x = self.resolve_deep(x);
                let y = self.resolve_deep(y);
                x == y
            })
    }

    pub fn fresh(&mut self) -> Type {
        self.slots.push(Slot::Unbound { level: self.level });
        Type::Var((self.slots.len() - 1) as TypeVarId)
    }

    pub fn enter_level(&mut self) {
        self.level += 1;
    }

    pub fn exit_level(&mut self) {
        self.level -= 1;
    }

    pub fn level(&self) -> u32 {
        self.level
    }

    fn level_of(&self, var: TypeVarId) -> u32 {
        match self.slots[var as usize] {
            Slot::Unbound { level } => level,
            Slot::Bound(_) => u32::MAX,
        }
    }

    /// Follow bound variables one layer, compressing the path as it goes. The
    /// result is either an unbound `Var` or a `Con`.
    pub fn resolve(&mut self, ty: &Type) -> Type {
        let Type::Var(v) = ty else { return ty.clone() };
        let Slot::Bound(inner) = self.slots[*v as usize].clone() else {
            return ty.clone();
        };
        let root = self.resolve(&inner);
        self.set_slot(*v, Slot::Bound(root.clone()));
        root
    }

    /// Substitute every bound variable throughout, leaving only unbound
    /// variables and constructors.
    pub fn resolve_deep(&mut self, ty: &Type) -> Type {
        match self.resolve(ty) {
            Type::Var(v) => Type::Var(v),
            Type::Con(con, args) => {
                let args = args.iter().map(|a| self.resolve_deep(a)).collect();
                Type::Con(con, args)
            }
        }
    }

    /// True if the type still contains an unbound variable after resolution.
    pub fn has_unbound(&mut self, ty: &Type) -> bool {
        match self.resolve(ty) {
            Type::Var(_) => true,
            Type::Con(_, args) => args.iter().any(|a| self.has_unbound(a)),
        }
    }

    pub fn unify(&mut self, a: &Type, b: &Type) -> Result<(), UnifyError> {
        let a = self.resolve(a);
        let b = self.resolve(b);
        match (&a, &b) {
            (Type::Var(x), Type::Var(y)) if x == y => Ok(()),
            (Type::Var(x), _) => self.bind(*x, &b),
            (_, Type::Var(y)) => self.bind(*y, &a),
            (Type::Con(c1, args1), Type::Con(c2, args2)) => {
                if c1 != c2 || args1.len() != args2.len() {
                    return Err(UnifyError::Mismatch);
                }
                for (x, y) in args1.iter().zip(args2) {
                    self.unify(x, y)?;
                }
                Ok(())
            }
        }
    }

    fn bind(&mut self, var: TypeVarId, ty: &Type) -> Result<(), UnifyError> {
        let level = self.level_of(var);
        self.occurs_and_adjust(var, level, ty)?;
        self.set_slot(var, Slot::Bound(ty.clone()));
        Ok(())
    }

    /// Reject `T = ... T ...`, and lower the level of every variable inside
    /// `ty` to at most `level`. The second half is what keeps generalisation
    /// honest: a variable that escapes into an outer binding must not later be
    /// quantified by an inner one.
    fn occurs_and_adjust(
        &mut self,
        var: TypeVarId,
        level: u32,
        ty: &Type,
    ) -> Result<(), UnifyError> {
        match self.resolve(ty) {
            Type::Var(v) => {
                if v == var {
                    return Err(UnifyError::Occurs);
                }
                if self.level_of(v) > level {
                    self.set_slot(v, Slot::Unbound { level });
                }
                Ok(())
            }
            Type::Con(_, args) => {
                for arg in &args {
                    self.occurs_and_adjust(var, level, arg)?;
                }
                Ok(())
            }
        }
    }

    /// Quantify every variable created inside the current level.
    pub fn generalize(&mut self, ty: &Type) -> Scheme {
        let mut vars = Vec::new();
        self.collect_generalizable(ty, &mut vars);
        Scheme {
            vars,
            ty: ty.clone(),
        }
    }

    /// Every variable still unbound anywhere in `ty`, at any level.
    ///
    /// [`TypeStore::generalize`] asks a narrower question -- which variables
    /// belong to the level being closed. This one is for deciding what a
    /// *constraint* still has an opinion about, where the level is irrelevant.
    pub fn collect_vars(&mut self, ty: &Type, out: &mut Vec<TypeVarId>) {
        match self.resolve(ty) {
            Type::Var(v) => {
                if !out.contains(&v) {
                    out.push(v);
                }
            }
            Type::Con(_, args) => {
                for arg in &args {
                    self.collect_vars(arg, out);
                }
            }
        }
    }

    fn collect_generalizable(&mut self, ty: &Type, out: &mut Vec<TypeVarId>) {
        match self.resolve(ty) {
            Type::Var(v) => {
                if self.level_of(v) > self.level && !out.contains(&v) {
                    out.push(v);
                }
            }
            Type::Con(_, args) => {
                for arg in &args {
                    self.collect_generalizable(arg, out);
                }
            }
        }
    }

    /// Replace a scheme's quantified variables with fresh ones. The fresh
    /// variables are returned in the scheme's order: they are the call site's
    /// type arguments, and monomorphisation reads them back once inference has
    /// resolved them to concrete types.
    pub fn instantiate(&mut self, scheme: &Scheme) -> (Type, Vec<Type>) {
        if scheme.vars.is_empty() {
            return (scheme.ty.clone(), Vec::new());
        }
        // A quantified error-set variable stays one through instantiation:
        // its meaning -- "nothing decided this, so it raises nothing" -- has to
        // survive to the call site that leaves it undecided.
        let fresh: Vec<Type> = scheme
            .vars
            .iter()
            .map(|&v| {
                if self.is_err_set_var(v) {
                    self.fresh_err_set()
                } else {
                    self.fresh()
                }
            })
            .collect();
        let map: HashMap<TypeVarId, Type> = scheme
            .vars
            .iter()
            .copied()
            .zip(fresh.iter().cloned())
            .collect();
        let ty = self.subst_vars(&scheme.ty, &map);
        (ty, fresh)
    }

    /// Replace the variables named in `map` throughout `ty`. Used both to
    /// instantiate a scheme and, by monomorphisation, to make a generic
    /// function's body concrete.
    pub fn subst_vars(&mut self, ty: &Type, map: &HashMap<TypeVarId, Type>) -> Type {
        match self.resolve(ty) {
            Type::Var(v) => map.get(&v).cloned().unwrap_or(Type::Var(v)),
            Type::Con(con, args) => {
                let args = args.iter().map(|a| self.subst_vars(a, map)).collect();
                Type::Con(con, args)
            }
        }
    }

    // ---- printing -------------------------------------------------------

    /// Render a type. Unbound variables print as `_`: they are holes inference
    /// has not filled, and numbering them (`?0`) both reads as optional syntax
    /// and suggests a distinction the reader cannot use.
    pub fn show(&mut self, ty: &Type) -> String {
        let mut names = HashMap::new();
        self.write_ty(ty, &mut names)
    }

    /// Render a scheme, naming quantified variables `T`, `U`, `V`, ...
    pub fn show_scheme(&mut self, scheme: &Scheme) -> String {
        let mut names = HashMap::new();
        for (i, var) in scheme.vars.iter().enumerate() {
            names.insert(*var, generic_name(i));
        }
        self.write_ty(&scheme.ty, &mut names)
    }

    fn write_ty(&mut self, ty: &Type, names: &mut HashMap<TypeVarId, String>) -> String {
        match self.resolve(ty) {
            Type::Var(v) => names.get(&v).cloned().unwrap_or_else(|| "_".into()),
            Type::Con(con, args) => match con {
                TyCon::I64 => "i64".into(),
                TyCon::F64 => "f64".into(),
                TyCon::Bool => "bool".into(),
                TyCon::Void => "void".into(),
                TyCon::Str => "str".into(),
                TyCon::Error => "error".into(),
                // Never printed on its own: a set is only ever an argument of
                // the two constructors above, which render it themselves.
                TyCon::ErrorSet(id) => self.set_text(id),
                TyCon::Struct(id) => {
                    let name = self.struct_name(id).to_string();
                    if args.is_empty() {
                        name
                    } else {
                        let args: Vec<String> =
                            args.iter().map(|a| self.write_ty(a, names)).collect();
                        format!("{name}[{}]", args.join(", "))
                    }
                }
                TyCon::Abstract(id) => abstract_name(id).to_string(),
                TyCon::Array => format!("[]{}", self.write_ty(&args[0], names)),
                TyCon::Optional => format!("?{}", self.write_ty(&args[0], names)),
                TyCon::ErrUnion => {
                    // An empty or still-unsolved set prints as bare `!T`, which
                    // is what every signature looked like before sets existed
                    // and is still the right thing for a function that cannot
                    // fail.
                    let set = match self.resolve(&args[1]) {
                        Type::Con(TyCon::ErrorSet(id), _) => self.set_text(id),
                        _ => String::new(),
                    };
                    format!("!{set}{}", self.write_ty(&args[0], names))
                }
                TyCon::Fn => {
                    let (ret, params) = args.split_last().expect("fn type has a return type");
                    let mut out = String::from("fn(");
                    for (i, p) in params.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        let _ = write!(out, "{}", self.write_ty(p, names));
                    }
                    let _ = write!(out, ") {}", self.write_ty(ret, names));
                    out
                }
            },
        }
    }
}

fn generic_name(i: usize) -> String {
    // T, U, V, W, then T1, U1, ...
    const LETTERS: [char; 4] = ['T', 'U', 'V', 'W'];
    let letter = LETTERS[i % LETTERS.len()];
    let round = i / LETTERS.len();
    if round == 0 {
        letter.to_string()
    } else {
        format!("{letter}{round}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unifies_a_variable_with_a_concrete_type() {
        let mut s = TypeStore::new();
        let v = s.fresh();
        s.unify(&v, &Type::i64()).unwrap();
        assert_eq!(s.resolve(&v), Type::i64());
        assert_eq!(s.show(&v), "i64");
    }

    #[test]
    fn unifies_structurally_through_functions() {
        let mut s = TypeStore::new();
        let a = s.fresh();
        let b = s.fresh();
        let f1 = Type::func(vec![a.clone()], Type::bool());
        let f2 = Type::func(vec![Type::i64()], b.clone());
        s.unify(&f1, &f2).unwrap();
        assert_eq!(s.show(&a), "i64");
        assert_eq!(s.show(&b), "bool");
    }

    #[test]
    fn mismatched_constructors_fail() {
        let mut s = TypeStore::new();
        assert_eq!(
            s.unify(&Type::i64(), &Type::bool()),
            Err(UnifyError::Mismatch)
        );
    }

    #[test]
    fn differing_arity_fails() {
        let mut s = TypeStore::new();
        let f1 = Type::func(vec![Type::i64()], Type::i64());
        let f2 = Type::func(vec![Type::i64(), Type::i64()], Type::i64());
        assert_eq!(s.unify(&f1, &f2), Err(UnifyError::Mismatch));
    }

    #[test]
    fn occurs_check_rejects_infinite_types() {
        let mut s = TypeStore::new();
        let v = s.fresh();
        let f = Type::func(vec![v.clone()], v.clone());
        assert_eq!(s.unify(&v, &f), Err(UnifyError::Occurs));
    }

    #[test]
    fn generalises_only_variables_from_the_inner_level() {
        let mut s = TypeStore::new();
        let outer = s.fresh(); // level 0
        s.enter_level();
        let inner = s.fresh(); // level 1
        s.exit_level();

        // At level 0, only `inner` is generalisable.
        let ty = Type::func(vec![outer.clone(), inner.clone()], Type::void());
        let scheme = s.generalize(&ty);
        assert_eq!(scheme.vars.len(), 1);
        assert_eq!(
            scheme.vars[0],
            match inner {
                Type::Var(v) => v,
                _ => unreachable!(),
            }
        );
    }

    #[test]
    fn unifying_lowers_the_level_so_escaping_vars_are_not_generalised() {
        let mut s = TypeStore::new();
        let outer = s.fresh(); // level 0
        s.enter_level();
        let inner = s.fresh(); // level 1
        // `inner` escapes into a level-0 variable, so it must drop to level 0.
        s.unify(&outer, &inner).unwrap();
        s.exit_level();

        let scheme = s.generalize(&inner);
        assert!(
            scheme.vars.is_empty(),
            "escaped variable must not be generalised"
        );
    }

    #[test]
    fn instantiation_produces_independent_copies() {
        let mut s = TypeStore::new();
        s.enter_level();
        let v = s.fresh();
        s.exit_level();
        // forall T. fn(T) T
        let scheme = s.generalize(&Type::func(vec![v.clone()], v.clone()));
        assert!(scheme.is_generic());
        assert_eq!(s.show_scheme(&scheme), "fn(T) T");

        let (t1, args1) = s.instantiate(&scheme);
        let (t2, _) = s.instantiate(&scheme);
        // Using the first copy at i64 must leave the second copy free.
        s.unify(&t1, &Type::func(vec![Type::i64()], Type::i64()))
            .unwrap();
        assert_eq!(s.show(&t1), "fn(i64) i64");
        assert!(s.has_unbound(&t2));
        // The recorded type argument is what monomorphisation reads back.
        assert_eq!(s.show(&args1[0]), "i64");
    }

    #[test]
    fn shows_optionals_error_unions_and_structs() {
        let mut s = TypeStore::new();
        let point = s.declare_struct("Point");
        assert_eq!(s.show(&Type::optional(Type::i64())), "?i64");
        let empty = s.err_set(Vec::new());
        assert_eq!(s.show(&Type::err_union(Type::strukt(point), empty)), "!Point");
        assert_eq!(
            s.show(&Type::func(vec![Type::str(), Type::bool()], Type::void())),
            "fn(str, bool) void"
        );
    }

    #[test]
    fn a_failed_trial_unification_leaves_no_trace() {
        let mut s = TypeStore::new();
        let a = s.fresh();
        // fn(A) bool  vs  fn(i64) i64 -- the parameter unifies A with i64, and
        // only then do the return types clash. Without rollback, A would be
        // left bound to i64 by the half-finished attempt.
        let f1 = Type::func(vec![a.clone()], Type::bool());
        let f2 = Type::func(vec![Type::i64()], Type::i64());
        assert!(!s.try_unify(&f1, &f2));
        assert!(
            s.has_unbound(&a),
            "a must still be free after a failed attempt"
        );

        // And a later real unification still works.
        s.unify(&a, &Type::str()).unwrap();
        assert_eq!(s.show(&a), "str");
    }

    #[test]
    fn a_successful_trial_unification_is_kept() {
        let mut s = TypeStore::new();
        let a = s.fresh();
        assert!(s.try_unify(&a, &Type::i64()));
        assert_eq!(s.show(&a), "i64");
    }

    #[test]
    fn rollback_restores_lowered_levels() {
        let mut s = TypeStore::new();
        let outer = s.fresh();
        s.enter_level();
        let inner = s.fresh();
        let snapshot = s.snapshot();
        s.unify(&outer, &inner).unwrap();
        s.rollback_to(snapshot);
        s.exit_level();
        // The level adjustment was undone, so `inner` is generalisable again.
        assert_eq!(s.generalize(&inner).vars.len(), 1);
    }

    #[test]
    fn an_abstract_type_is_above_the_types_it_lists() {
        let mut s = TypeStore::new();
        let number = lookup_abstract("Number").expect("`Number` is in the table");
        let num = Type::abstrakt(number);
        assert_eq!(s.show(&num), "Number");
        assert!(s.is_sub_ty(&Type::i64(), &num));
        assert!(s.is_sub_ty(&Type::f64(), &num));
        // Only downwards, and only for what the table lists.
        assert!(!s.is_sub_ty(&num, &Type::i64()));
        assert!(!s.is_sub_ty(&Type::str(), &num));
        // Flat: an abstract type is comparable only with itself.
        assert!(s.is_sub_ty(&num, &num));
        // And a variable is nothing's subtype until it is resolved.
        let v = s.fresh();
        assert!(!s.is_sub_ty(&v, &num));
    }

    #[test]
    fn generic_names_run_past_the_alphabet_slice() {
        assert_eq!(generic_name(0), "T");
        assert_eq!(generic_name(3), "W");
        assert_eq!(generic_name(4), "T1");
    }
}
