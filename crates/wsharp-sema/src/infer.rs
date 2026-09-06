//! Hindley-Milner type inference (Algorithm W) producing typed HIR.
//!
//! The shape of a run:
//!
//! 1. **Collect** structs and top-level declarations, so every function has an
//!    id before any body is looked at.
//! 2. **Group** functions into strongly connected components of the call graph.
//!    Each group is inferred with its members' types held *monomorphic* -- that
//!    is what makes `fib` calling itself, and two mutually recursive functions,
//!    typecheck -- and generalised only once the whole group is done.
//! 3. **Infer** each group, collecting deferred constraints (is this numeric?
//!    does this type have that field?) that cannot be decided the moment they
//!    are met.
//! 4. **Solve** those constraints, then fill in the struct field indices that
//!    were left unresolved, then generalise.
//!
//! Annotations are optional everywhere. When present they seed the type
//! directly; when absent a fresh variable is used and inference fills it in.

use std::collections::{BTreeSet, HashMap, HashSet};

use wsharp_runtime::header::{HEADER_SIZE, TYPE_ID_FIRST_USER, align_up};
use wsharp_syntax::Diagnostic;
use wsharp_syntax::ast::{self, BinOp, UnOp};
use wsharp_syntax::diag::Label;
use wsharp_syntax::span::{Ident, Span};

use crate::hir::{self, UNRESOLVED};
use crate::ty::{
    AbstractId, Scheme, StructId, TyCon, Type, TypeStore, TypeVarId, UnifyError, abstract_members,
    abstract_name, lookup_abstract,
};

pub struct Analysis {
    pub program: hir::Program,
    pub store: TypeStore,
    pub diags: Vec<Diagnostic>,
    /// `(name, rendered type)` for every top-level function, for `--emit=types`.
    pub signatures: Vec<(String, String)>,
}

impl Analysis {
    pub fn has_errors(&self) -> bool {
        !self.diags.is_empty()
    }
}

/// One file of a program, or one of the standard library's modules.
pub struct SourceModule<'a> {
    /// The module's identity, and the prefix its top-level names are stored
    /// under. `"main"` for the root, a standard-library path such as
    /// `"std/http"`, or the file's own location.
    pub path: String,
    pub ast: &'a ast::Module,
    /// For each `@import` specifier this file writes, the module it names.
    ///
    /// Resolved by the driver rather than here: two files may reach the same
    /// module by different relative paths, and which file a path lands on is a
    /// question about the file system.
    pub imports: HashMap<String, String>,
}

/// Analyse a single-file program. The file is the module named `main`.
pub fn analyze(module: &ast::Module) -> Analysis {
    analyze_program(&[SourceModule {
        path: "main".into(),
        ast: module,
        imports: HashMap::new(),
    }])
}

/// Analyse a whole program: the root file, and everything it imports.
///
/// The root module must come first; it is the one `main` is looked for in.
pub fn analyze_program(modules: &[SourceModule]) -> Analysis {
    let mut inf = Inferencer::new(modules);
    inf.collect_structs();
    inf.collect_globals();
    inf.infer_all();
    inf.check_overloads();
    inf.check_function_consts();
    inf.check_entry();
    // Last, because inference is what materialises the status types a program
    // actually mentions, and numbering must see the final lattice.
    inf.number_structs();
    inf.finish()
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// What a top-level name refers to.
#[derive(Debug, Clone)]
enum GlobalRef {
    /// A module bound by `@import`. Never a value; naming one bare is an
    /// error that says so.
    Module,
    /// One or more functions. More than one is an overload set, resolved by
    /// multiple dispatch at each call site; a set of one is the ordinary case
    /// and takes exactly the path it always did.
    Func(Vec<hir::FuncId>),
    /// A top-level `const` bound to a literal. Session 1 has no global storage,
    /// so these are substituted at each use site.
    Const(usize),
    Builtin(hir::BuiltinId),
    /// The sole instance of a struct with no fields, named by the type itself:
    /// `NotFound404` is a value as well as a type. Emitted once into the data
    /// section, so mentioning it costs nothing.
    Singleton(StructId),
    /// A `const` bound to a function name *and* annotated with a signature,
    /// which is what picks one member out of an overload set. Indexes
    /// [`Inferencer::fn_consts`]; see [`FuncConst`] for why the member cannot
    /// be chosen here.
    FuncValue(usize),
}

/// A `const` bound to one member of an overload set by its type annotation.
///
/// The annotation cannot pick the member where the `const` is written: the
/// members' signatures are not known until their bodies have been inferred. So
/// the whole set is kept, and the choice is made at the first use -- or, for a
/// `const` nothing uses, by [`Inferencer::check_function_consts`], so that an
/// annotation matching nothing is still reported.
struct FuncConst {
    ids: Vec<hir::FuncId>,
    /// The annotation, resolved. Also the type of every use, so the annotation
    /// is enforced and not merely consulted.
    ty: Type,
    span: Span,
    /// The name being bound, for diagnostics.
    name: String,
    /// The member the annotation selected, once that has been worked out.
    chosen: Option<hir::FuncId>,
    /// Set when selection has failed, so it is reported once and later uses
    /// fall back to behaving like an unannotated alias.
    failed: bool,
}

/// A top-level `const` bound to a literal value.
struct ConstDef {
    scheme: Scheme,
    kind: hir::ExprKind,
}

/// One overload being considered at a call site.
struct Candidate {
    id: hir::FuncId,
    /// The parameter types this use instantiates the overload at. These are
    /// what the arguments unify with, and what records the type arguments
    /// monomorphisation needs.
    params: Vec<Type>,
    /// The parameter types *as written*, with abstract constructors intact.
    /// Identical to `params` unless a parameter is annotated with an abstract
    /// type, which becomes a variable in `params` -- and a variable orders
    /// against nothing, so specificity has to read the annotation instead.
    /// Overloaded functions must annotate every parameter, so these are always
    /// concrete, which is what makes specificity decidable here rather than
    /// after constraint solving.
    decl_params: Vec<Type>,
    ret: Type,
    targs: Vec<Type>,
    /// Per argument, the runtime test this case needs, or `None` if the static
    /// type already guarantees a match. See `Inferencer::overlap`.
    tests: Vec<Option<StructId>>,
}

/// A constraint that could not be decided where it was met, because the type
/// involved was still a variable. Solved at the end of the binding group.
enum Constraint {
    /// Must end up `i64` or `f64`; defaults to `i64` if still unconstrained.
    Numeric {
        ty: Type,
        span: Span,
        op: &'static str,
        /// `f64` is not enough: `%` has no float form, because Cranelift has
        /// no float remainder and the language does not define one.
        integers_only: bool,
    },
    /// Must be a type `==` can compare.
    Equatable { ty: Type, span: Span },
    /// Must be one of the concrete types an abstract type lists.
    ///
    /// Written by a parameter annotated with an abstract type, which is a
    /// *constrained generic* parameter rather than a type of its own, and by
    /// every use of such a function, on the type that use instantiates it at.
    /// Both are needed: the declaration is where the constraint is stated and
    /// exactly where it cannot be checked.
    Member {
        ty: Type,
        id: AbstractId,
        span: Span,
    },
    /// `obj` must be an array, whose element type is `elem`.
    ///
    /// A constraint rather than a decision on the spot for the same reason
    /// [`Constraint::HasField`] is: `a[0]` in a function with no annotation
    /// meets the question before anything can answer it.
    Indexable { obj: Type, elem: Type, span: Span },
    /// `set` must contain these errors, and if it is still a variable then it
    /// is *made* to: an inferred error set is the union of what its function
    /// can actually raise, and this is where each contribution is recorded.
    ErrorSetHas {
        set: Type,
        errors: Vec<hir::ErrorId>,
        /// Whether a still-open set should be *made* to contain these, or only
        /// checked against them. Raising an error widens the set it is raised
        /// into; naming one in a comparison must not, or `e == error.X` would
        /// quietly claim that `X` can happen.
        widens: bool,
        span: Span,
    },
    /// `sub` must be a subset of `sup`.
    ///
    /// What `try` needs: propagating a callee's errors is only sound if this
    /// function's set already covers them. Not unification, because the two
    /// sets are genuinely allowed to differ.
    ErrorSubset { sub: Type, sup: Type, span: Span },
    /// `obj` must be a struct with field `field`, whose type is `result`.
    HasField {
        obj: Type,
        field: Box<str>,
        result: Type,
        span: Span,
    },
}

impl Constraint {
    /// Every type this constraint still has an opinion about.
    ///
    /// Used to decide what a definition may generalise over: a variable a
    /// constraint owns is one the solver is about to pin, and quantifying it
    /// would give the same body two different types depending on whether it
    /// was written as a `fn` declaration or as a `fn` literal.
    fn types(&self) -> [&Type; 2] {
        match self {
            Constraint::Numeric { ty, .. }
            | Constraint::Equatable { ty, .. }
            | Constraint::Member { ty, .. } => [ty, ty],
            Constraint::ErrorSetHas { set, .. } => [set, set],
            Constraint::ErrorSubset { sub, sup, .. } => [sub, sup],
            Constraint::Indexable { obj, elem, .. } => [obj, elem],
            Constraint::HasField { obj, result, .. } => [obj, result],
        }
    }
}

// Subtyping deliberately has no `Constraint` of its own, despite being the
// obvious candidate for one. A constraint is for a question that cannot be
// answered where it is met -- but `try_unify` binds whichever side is still a
// variable, so by the time anything asks "is this a subtype?" both sides are
// already concrete and the lattice answers immediately. See `coerce`.

/// What a name bound inside a function body means.
///
/// Two of these are not values. An overload set bound by `const g = f;` is not
/// one because there is no single code pointer for a set; a generic `fn`
/// literal is not one because a closure value is a single code pointer and two
/// instantiations need two. Both are names that resolve to functions rather
/// than locals holding something.
#[derive(Debug, Clone)]
enum Binding {
    Local(hir::LocalId),
    Overloads(Vec<hir::FuncId>),
    /// A `const` bound to a generic `fn` literal. Each use materialises a
    /// closure at the type that use needs; `captures` names the hidden locals
    /// the definition snapshotted its captured values into, in the order the
    /// closure's own capture locals expect them.
    Definition {
        func: hir::FuncId,
        captures: Vec<String>,
    },
}

/// The names a `for` over a non-array subject calls, resolved in the module
/// that declares the subject's type. Two rather than one because an iterator
/// has state the subject does not: `iter` makes it, `next` advances it.
const PROTOCOL_NAMES: &[&str] = &["iter", "next"];

/// One function being inferred. A stack of these models nesting: a `fn` literal
/// pushes a frame, and a name resolved past a frame boundary becomes a capture.
struct Frame {
    locals: Vec<hir::LocalDef>,
    params: Vec<hir::LocalId>,
    /// Locals in *this* frame that are loaded from the closure environment.
    captures: Vec<hir::LocalId>,
    /// For each entry of `captures`, the local in the *enclosing* frame it is
    /// copied from.
    capture_sources: Vec<hir::LocalId>,
    captured_names: HashMap<String, hir::LocalId>,
    scopes: Vec<Vec<(String, Binding)>>,
    ret: Type,
    /// The local this function's own name is bound to inside its body, for a
    /// `fn` literal that may recurse. See [`hir::FuncDef::self_local`].
    self_local: Option<hir::LocalId>,
    /// Whether the body actually named itself. A literal that did not is left
    /// exactly as it was before recursion was allowed, down to the roots the
    /// collector sees.
    self_used: bool,
}

impl Frame {
    fn find(&self, name: &str) -> Option<Binding> {
        for scope in self.scopes.iter().rev() {
            if let Some((_, binding)) = scope.iter().rev().find(|(n, _)| n == name) {
                return Some(binding.clone());
            }
        }
        None
    }

    fn add_local(&mut self, name: &str, ty: Type, mutable: bool, span: Span) -> hir::LocalId {
        let id = self.locals.len() as hir::LocalId;
        self.locals.push(hir::LocalDef {
            name: name.to_string(),
            ty,
            mutable,
            span,
        });
        id
    }

    fn bind(&mut self, name: &str, binding: Binding) {
        self.scopes
            .last_mut()
            .expect("a scope is open")
            .push((name.to_string(), binding));
    }

    fn bind_local(&mut self, name: &str, id: hir::LocalId) {
        self.bind(name, Binding::Local(id));
    }
}

struct Inferencer<'a> {
    modules: &'a [SourceModule<'a>],
    /// The module whose items are being collected or inferred. Every
    /// unqualified name is looked up in this one first.
    current: usize,
    /// Per module, the local name each `@import` bound and the module path it
    /// names.
    imports: Vec<HashMap<String, String>>,
    /// Every module path that exists, so a path can be told from a member.
    module_paths: HashSet<String>,
    /// Where each struct was declared: which module, and which item in it.
    /// Two modules may each declare a `Point`, so a name is no longer enough.
    struct_decl_of: HashMap<StructId, (usize, usize)>,
    /// The module each function was declared in, so inference can put its
    /// module back in scope when it reaches the body.
    fn_modules: Vec<usize>,
    store: TypeStore,
    diags: Vec<Diagnostic>,

    structs: Vec<hir::StructDef>,
    struct_ids: HashMap<String, StructId>,
    /// Where each struct's name was written, so a redeclaration can point
    /// back at the first one. Builtin status types have no entry.
    struct_spans: HashMap<String, Span>,

    /// Per generic struct, the variable each of its type parameters stands
    /// for. Empty for every other struct.
    struct_type_params: HashMap<StructId, HashMap<String, Type>>,

    globals: HashMap<String, GlobalRef>,
    /// Where each global was first declared, for the same reason. Builtins
    /// and lazily materialised status types have no entry.
    global_spans: HashMap<String, Span>,
    consts: Vec<ConstDef>,
    /// `const`s bound to a function name and pinned by an annotation.
    fn_consts: Vec<FuncConst>,

    /// Per `FuncId`: the source function, absent for nothing in session 1 but
    /// kept as an option so generated functions can be added later.
    fn_asts: Vec<Option<&'a ast::Func>>,
    fn_names: Vec<String>,
    fn_spans: Vec<Span>,
    fn_is_closure: Vec<bool>,
    /// The monomorphic type of each function while its group is being inferred.
    fn_types: Vec<Type>,
    /// Per function, its parameter types *as written*. See
    /// [`Candidate::decl_params`] for why an inferred type will not do.
    fn_decl_params: Vec<Vec<Type>>,
    /// Per function, the parameters annotated with an abstract type: the
    /// variable standing in for each, and which abstract type constrains it.
    fn_member_vars: Vec<Vec<(Type, AbstractId, Span)>>,
    /// Per function, the type parameters it declares as `fn f[T](..)`.
    fn_generic_names: Vec<Vec<Ident>>,
    /// Per function, the variable each declared type parameter stands for.
    /// Filled in by [`Inferencer::signature_type`], because the variables must
    /// be created inside the binding group's level to generalise with it.
    fn_type_params: Vec<HashMap<String, Type>>,
    /// Filled in when the function's group is generalised.
    schemes: Vec<Option<Scheme>>,
    funcs: Vec<Option<hir::FuncDef>>,

    strings: Vec<String>,
    string_ids: HashMap<String, hir::StrId>,

    frames: Vec<Frame>,
    /// Qualified keys another module may *not* name. Private is the default, so
    /// this records the exceptions rather than the rule -- and a name a module
    /// declares is always visible to itself, which is why one flat set is
    /// enough: a qualified lookup only ever crosses a module boundary, since a
    /// module that imported itself would be a cycle.
    private: HashSet<String>,
    /// Spans a privacy error has already been reported at. One qualified name
    /// is resolved more than once -- a call asks whether its callee is an
    /// overload set, a builtin, and then a value -- and the reader wants to be
    /// told once.
    reported_private: HashSet<Span>,
    /// The name the *next* frame pushed should bind to the function it is the
    /// body of, so that a `fn` literal bound to a `const` can recurse. Consumed
    /// by [`Inferencer::infer_function`]; set only by
    /// [`Inferencer::infer_fn_literal_named`].
    pending_self: Option<(String, Type)>,
    constraints: Vec<Constraint>,
    loop_depth: usize,
    /// The type parameters in scope, innermost last. A `fn` literal inside a
    /// generic function can still name that function's type parameters, so
    /// this is a stack rather than one entry; a generic struct pushes one
    /// while its fields are being laid out.
    type_scopes: Vec<HashMap<String, Type>>,
}

impl<'a> Inferencer<'a> {
    fn new(modules: &'a [SourceModule<'a>]) -> Inferencer<'a> {
        Inferencer {
            modules,
            current: 0,
            imports: modules.iter().map(|_| HashMap::new()).collect(),
            // The standard library's modules exist whether or not the driver
            // handed one over: `std/http` has no source at all, because its
            // types are materialised from a table on first mention.
            module_paths: modules
                .iter()
                .map(|m| m.path.clone())
                .chain(wsharp_runtime::builtins::std_module_paths())
                .collect(),
            struct_decl_of: HashMap::new(),
            fn_modules: Vec::new(),
            store: TypeStore::new(),
            diags: Vec::new(),
            structs: Vec::new(),
            struct_ids: HashMap::new(),
            struct_spans: HashMap::new(),
            struct_type_params: HashMap::new(),
            globals: HashMap::new(),
            global_spans: HashMap::new(),
            consts: Vec::new(),
            fn_consts: Vec::new(),
            fn_asts: Vec::new(),
            fn_names: Vec::new(),
            fn_spans: Vec::new(),
            fn_is_closure: Vec::new(),
            fn_types: Vec::new(),
            fn_decl_params: Vec::new(),
            fn_member_vars: Vec::new(),
            fn_generic_names: Vec::new(),
            fn_type_params: Vec::new(),
            schemes: Vec::new(),
            funcs: Vec::new(),
            strings: Vec::new(),
            string_ids: HashMap::new(),
            frames: Vec::new(),
            private: HashSet::new(),
            reported_private: HashSet::new(),
            pending_self: None,
            constraints: Vec::new(),
            loop_depth: 0,
            type_scopes: Vec::new(),
        }
    }

    // -----------------------------------------------------------------------
    // Names, and the module they belong to
    // -----------------------------------------------------------------------

    /// The key a name declared in module `m` is stored under.
    ///
    /// One flat map with qualified keys, rather than a map per module: name
    /// resolution is one lookup either way, and every pass downstream keeps
    /// working on a single table. What a module *cannot* see is then simply
    /// what it does not have the key for.
    fn key_in(&self, m: usize, name: &str) -> String {
        format!("{}.{}", self.modules[m].path, name)
    }

    fn key(&self, name: &str) -> String {
        self.key_in(self.current, name)
    }

    /// Resolve an unqualified name: this module's own declarations, then the
    /// prelude, which is stored unqualified because every module has it.
    fn global(&self, name: &str) -> Option<&GlobalRef> {
        self.globals
            .get(&self.key(name))
            .or_else(|| self.globals.get(name))
    }

    fn global_mut(&mut self, name: &str) -> Option<&mut GlobalRef> {
        let key = self.key(name);
        if self.globals.contains_key(&key) {
            return self.globals.get_mut(&key);
        }
        self.globals.get_mut(name)
    }

    fn has_global(&self, name: &str) -> bool {
        self.global(name).is_some()
    }

    /// Declare a name in the module being collected.
    fn declare_global(&mut self, name: &str, global: GlobalRef, span: Span) {
        let key = self.key(name);
        self.globals.insert(key.clone(), global);
        self.global_spans.insert(key, span);
    }

    /// Where an unqualified name was first declared, for a redeclaration's
    /// second label.
    fn global_span(&self, name: &str) -> Option<Span> {
        self.global_spans
            .get(&self.key(name))
            .or_else(|| self.global_spans.get(name))
            .copied()
    }

    /// A struct named without qualification, from this module.
    fn struct_named(&self, name: &str) -> Option<StructId> {
        self.struct_ids.get(&self.key(name)).copied()
    }

    fn error(&mut self, span: Span, message: impl Into<String>) -> &mut Diagnostic {
        self.diags.push(Diagnostic::error(span, message));
        self.diags.last_mut().expect("just pushed")
    }

    fn finish(mut self) -> Analysis {
        let mut signatures = Vec::new();
        for id in 0..self.fn_names.len() {
            // The root module only: `--emit=types` is for reading back what
            // was inferred about the file in front of you, and the standard
            // library's own signatures would bury it.
            if self.fn_is_closure[id] || self.fn_modules[id] != 0 {
                continue;
            }
            let name = self.fn_names[id].clone();
            let scheme = self.schemes[id].clone();
            let rendered = match scheme {
                Some(scheme) => {
                    let scheme = self.display_scheme(id as hir::FuncId, scheme);
                    self.store.show_scheme(&scheme)
                }
                None => {
                    let ty = self.fn_types[id].clone();
                    self.store.show(&ty)
                }
            };
            signatures.push((name, rendered));
        }

        // An overloaded `main` has no single entry point; `check_entry` has
        // already reported it, so leave `entry` unset rather than picking one.
        let mut entry = self
            .globals
            .get(&self.key_in(0, "main"))
            .and_then(|g| match g {
                GlobalRef::Func(ids) if ids.len() == 1 => Some(ids[0]),
                _ => None,
            });

        // `funcs` is indexed by `FuncId`, so entries must never be dropped --
        // that would silently renumber every function. If any body failed to
        // produce a definition the whole table is discarded instead; errors
        // were already reported and no later pass runs.
        let complete = self.funcs.iter().all(|f| f.is_some());
        let funcs: Vec<hir::FuncDef> = if complete {
            self.funcs
                .into_iter()
                .map(|f| f.expect("just checked"))
                .collect()
        } else {
            debug_assert!(
                !self.diags.is_empty(),
                "a function was dropped without a diagnostic"
            );
            entry = None;
            Vec::new()
        };

        Analysis {
            program: hir::Program {
                structs: self.structs,
                funcs,
                strings: self.strings,
                errors: self.store.error_names().to_vec(),
                entry,
            },
            store: self.store,
            diags: self.diags,
            signatures,
        }
    }

    /// A function's scheme as it should be *shown*: with the variables that
    /// stand in for abstract annotations put back as the annotations they came
    /// from, and no longer quantified.
    ///
    /// `fn(Number) str` is what the program says, and `fn(T) str` -- which is
    /// what the scheme holds -- tells the reader nothing about which types `T`
    /// may be. A variable is substituted everywhere it occurs, including in the
    /// return type: `fn bigger(a: Number, b: Number)` really does return one of
    /// the numbers it was handed.
    fn display_scheme(&mut self, id: hir::FuncId, scheme: Scheme) -> Scheme {
        let members = self.fn_member_vars[id as usize].clone();
        if members.is_empty() {
            return scheme;
        }
        let mut map = HashMap::new();
        for (var, abstract_id, _) in members {
            if let Type::Var(v) = self.store.resolve(&var) {
                map.insert(v, Type::abstrakt(abstract_id));
            }
        }
        let ty = self.store.subst_vars(&scheme.ty, &map);
        let vars = scheme
            .vars
            .into_iter()
            .filter(|v| !map.contains_key(v))
            .collect();
        Scheme { vars, ty }
    }

    // -----------------------------------------------------------------------
    // Collection
    // -----------------------------------------------------------------------

    /// Declare every struct name, resolve the lattice, number the types, then
    /// resolve field types.
    ///
    /// Names come first so that a struct may refer to itself or to a struct
    /// declared later -- instances are heap pointers, so recursive shapes are
    /// fine. Fields come last, and in *lattice* order rather than source order,
    /// because a subtype's layout starts as a copy of its supertype's.
    fn collect_structs(&mut self) {
        for m in 0..self.modules.len() {
            self.current = m;
            self.collect_module_structs(m);
        }
        self.current = 0;

        self.resolve_struct_parents();
        self.layout_struct_fields();
    }

    fn collect_module_structs(&mut self, m: usize) {
        for (index, item) in self.modules[m].ast.items.iter().enumerate() {
            let ast::Item::Struct(decl) = item else {
                continue;
            };
            let key = self.key(decl.name.as_str());
            if let Some(&first) = self.struct_spans.get(&key) {
                self.error(
                    decl.name.span,
                    format!("`{}` is declared more than once", decl.name),
                )
                .secondary
                .push(Label {
                    span: first,
                    message: "first declared here".into(),
                });
                continue;
            }
            let id = self.store.declare_struct(decl.name.as_str());
            self.struct_ids.insert(key.clone(), id);
            self.struct_spans.insert(key, decl.name.span);
            self.struct_decl_of.insert(id, (m, index));
            self.check_generic_names(&decl.generics);
            if !decl.generics.is_empty() && decl.fields.is_empty() {
                self.error(
                    decl.name.span,
                    "a generic struct must have at least one field",
                )
                .help = Some(
                    "with no field to mention them, nothing could ever say what its type \
                     parameters are"
                        .into(),
                );
            }
            // One variable per parameter, and they are its identity: a field's
            // type mentions them, and an instantiation substitutes for them.
            let params: Vec<Type> = decl.generics.iter().map(|_| self.store.fresh()).collect();
            let bindings: HashMap<String, Type> = decl
                .generics
                .iter()
                .map(|g| g.to_string())
                .zip(params.iter().cloned())
                .collect();
            self.struct_type_params.insert(id, bindings);
            self.structs.push(hir::StructDef {
                name: decl.name.to_string(),
                params: params
                    .iter()
                    .map(|p| match p {
                        Type::Var(v) => *v,
                        _ => unreachable!("just made fresh"),
                    })
                    .collect(),
                parent: None,
                fields: Vec::new(),
                // Both are filled in by `number_structs` below.
                type_id: TYPE_ID_FIRST_USER,
                subtree_len: 1,
                size: HEADER_SIZE,
                field_end: HEADER_SIZE,
                inherited: 0,
                span: decl.span,
            });
        }
    }

    /// Lay fields out parents-first, so a subtype can start from a copy of its
    /// supertype's layout. Declaration order will not do: a subtype may be
    /// written above its supertype, and in another module entirely.
    fn layout_struct_fields(&mut self) {
        for id in self.layout_order() {
            let Some((m, decl)) = self.struct_decl_ast(id) else {
                continue;
            };
            // Field types are resolved in the module that wrote them.
            self.current = m;

            // A subtype's fields are its supertype's followed by its own, so
            // the two layouts agree on every inherited field. `order` is a
            // preorder walk, so the parent is already laid out.
            let (mut fields, mut offset) = match self.structs[id as usize].parent {
                Some(parent) => {
                    let parent = &self.structs[parent as usize];
                    (parent.fields.clone(), parent.field_end)
                }
                None => (Vec::new(), HEADER_SIZE),
            };
            let inherited = fields.len() as u32;
            let generic = self
                .struct_type_params
                .get(&id)
                .filter(|b| !b.is_empty())
                .cloned();
            let is_generic = generic.is_some();
            if let Some(bindings) = generic {
                self.type_scopes.push(bindings);
            }

            for field in &decl.fields {
                if let Some(index) = fields.iter().position(|f| f.name == field.name.as_str()) {
                    let message = if (index as u32) < inherited {
                        let parent = self.structs[id as usize]
                            .parent
                            .map(|p| self.structs[p as usize].name.clone())
                            .unwrap_or_default();
                        format!(
                            "field `{}` is already inherited from `{parent}`",
                            field.name
                        )
                    } else {
                        format!("field `{}` is declared more than once", field.name)
                    };
                    let first = fields[index].span;
                    self.error(field.name.span, message).secondary.push(Label {
                        span: first,
                        message: "first declared here".into(),
                    });
                    continue;
                }
                let ty = self.resolve_type_expr(&field.ty);
                // A generic struct's offsets depend on what it is used at, so
                // they are left unresolved and recomputed per instantiation.
                // `UNRESOLVED` rather than zero: a zero offset would quietly
                // write over the header instead of failing where it is wrong.
                let width = if is_generic {
                    0
                } else {
                    crate::layout::size_of(&mut self.store, &ty)
                };
                fields.push(hir::FieldDef {
                    name: field.name.to_string(),
                    ty,
                    offset: if is_generic { UNRESOLVED } else { offset },
                    span: field.span,
                });
                offset += width;
            }
            if is_generic {
                self.type_scopes.pop();
            }
            let strukt = &mut self.structs[id as usize];
            strukt.fields = fields;
            strukt.inherited = inherited;
            strukt.field_end = if is_generic { UNRESOLVED } else { offset };
            strukt.size = if is_generic { 0 } else { align_up(offset) };
        }
        self.current = 0;
    }

    /// The declaration a `StructId` came from, and the module it is in.
    ///
    /// By recorded position rather than by name: two modules may each declare
    /// a `Point`, and a lazily materialised status type has no declaration at
    /// all.
    fn struct_decl_ast(&self, id: StructId) -> Option<(usize, &'a ast::StructDecl)> {
        let &(m, index) = self.struct_decl_of.get(&id)?;
        match &self.modules[m].ast.items[index] {
            ast::Item::Struct(decl) => Some((m, decl)),
            _ => None,
        }
    }

    /// Resolve each `struct : Parent` link, rejecting unknown parents and
    /// cycles. A cycle is broken as well as reported, because everything
    /// downstream walks the parent chain and would not terminate on one.
    fn resolve_struct_parents(&mut self) {
        for m in 0..self.modules.len() {
            self.current = m;
            self.resolve_module_struct_parents(m);
        }
        self.current = 0;
        self.break_parent_cycles();
    }

    fn resolve_module_struct_parents(&mut self, m: usize) {
        for item in &self.modules[m].ast.items {
            let ast::Item::Struct(decl) = item else {
                continue;
            };
            let (Some(parent_name), Some(id)) =
                (decl.parent.as_ref(), self.struct_named(decl.name.as_str()))
            else {
                continue;
            };
            let Some(parent) = self.lookup_struct(parent_name.as_str()) else {
                if !self.reject_abstract(parent_name.as_str(), parent_name.span) {
                    self.error(
                        parent_name.span,
                        format!("unknown supertype `{parent_name}`"),
                    )
                    .help = Some(
                        "a supertype must be a struct declared in this file, or a status type"
                            .into(),
                    );
                }
                continue;
            };
            self.structs[id as usize].parent = Some(parent);
            self.store.set_struct_parent(id, parent);
        }
    }

    /// Break cycles before anything walks a parent chain.
    fn break_parent_cycles(&mut self) {
        for id in 0..self.structs.len() as StructId {
            let mut seen = vec![id];
            let mut cur = self.structs[id as usize].parent;
            while let Some(next) = cur {
                if seen.contains(&next) {
                    let name = &self.structs[id as usize].name;
                    let span = self.structs[id as usize].span;
                    let message = if next == id && seen.len() == 1 {
                        format!("`{name}` inherits from itself")
                    } else {
                        format!("`{name}` is part of a cycle of supertypes")
                    };
                    self.error(span, message).help =
                        Some("the supertype relation must form a tree".into());
                    let last = *seen.last().expect("seen starts non-empty");
                    self.structs[last as usize].parent = None;
                    self.store.clear_struct_parent(last);
                    break;
                }
                seen.push(next);
                cur = self.structs[next as usize].parent;
            }
        }
    }

    /// Resolve a struct name, materialising a builtin status type on first
    /// mention.
    ///
    /// The HTTP status lattice is a table rather than 27 declarations in every
    /// program, and a program pays only for the statuses it names: an unnamed
    /// one is never created, so it costs no type id, no registry entry and no
    /// singleton. Ancestors come along because the lattice needs them.
    fn lookup_struct(&mut self, name: &str) -> Option<StructId> {
        self.struct_named(name)
    }

    /// A struct named through a module: `http.NotFound404`.
    ///
    /// The HTTP status lattice is a table rather than 27 declarations in every
    /// program, and a program pays only for the statuses it names: an unnamed
    /// one is never created, so it costs no type id, no registry entry and no
    /// singleton. Ancestors come along because the lattice needs them.
    fn lookup_struct_in(&mut self, module: &str, name: &str) -> Option<StructId> {
        let key = format!("{module}.{name}");
        if let Some(&id) = self.struct_ids.get(&key) {
            return Some(id);
        }
        if module != HTTP_MODULE {
            return None;
        }
        let table = wsharp_runtime::builtins::status_types();
        let &(sname, parent) = table.iter().find(|(n, _)| *n == name)?;

        let id = self.store.declare_struct(sname);
        self.struct_ids.insert(key, id);
        self.structs.push(hir::StructDef {
            name: sname.to_string(),
            // A builtin status type is never generic.
            params: Vec::new(),
            parent: None,
            fields: Vec::new(),
            type_id: TYPE_ID_FIRST_USER,
            subtree_len: 1,
            size: HEADER_SIZE,
            field_end: HEADER_SIZE,
            inherited: 0,
            span: Span::EMPTY,
        });
        // A status type has no fields, so it is also a value.
        self.globals
            .insert(format!("{HTTP_MODULE}.{sname}"), GlobalRef::Singleton(id));

        if let Some(parent) = parent {
            // Recurses at most as deep as the lattice, and the table is
            // ordered parents-first, so this terminates.
            let parent = self
                .lookup_struct_in(HTTP_MODULE, parent)
                .expect("a status type's supertype is in the same table");
            self.structs[id as usize].parent = Some(parent);
            self.store.set_struct_parent(id, parent);
        }
        Some(id)
    }

    /// Structs in an order that puts every supertype before its subtypes,
    /// which is what field layout needs.
    fn layout_order(&self) -> Vec<StructId> {
        let mut order = Vec::with_capacity(self.structs.len());
        let mut done = vec![false; self.structs.len()];
        for id in 0..self.structs.len() as StructId {
            // Walk up to the root, then emit downwards.
            let mut chain = Vec::new();
            let mut cur = Some(id);
            while let Some(next) = cur {
                if done[next as usize] {
                    break;
                }
                chain.push(next);
                cur = self.structs[next as usize].parent;
            }
            for &node in chain.iter().rev() {
                if !done[node as usize] {
                    done[node as usize] = true;
                    order.push(node);
                }
            }
        }
        order
    }

    /// Assign runtime type ids in a preorder walk of the lattice, so that every
    /// type's subtypes form a contiguous id range.
    ///
    /// Runs after inference, once every type that will exist has been
    /// materialised. Nothing reads a `type_id` before then -- dispatch tables
    /// carry `StructId`s and code generation resolves them.
    fn number_structs(&mut self) {
        let count = self.structs.len();
        let mut children: Vec<Vec<StructId>> = vec![Vec::new(); count];
        let mut roots: Vec<StructId> = Vec::new();
        for id in 0..count as StructId {
            match self.structs[id as usize].parent {
                Some(parent) => children[parent as usize].push(id),
                None => roots.push(id),
            }
        }

        let mut next = TYPE_ID_FIRST_USER;
        // An explicit stack of (node, already_expanded) rather than recursion:
        // a deep lattice is user input, and this runs in the compiler.
        for root in roots {
            let mut stack = vec![(root, false)];
            while let Some((id, expanded)) = stack.pop() {
                if expanded {
                    // Second visit: every descendant has been numbered, so the
                    // subtree size is however far the counter has moved.
                    let start = self.structs[id as usize].type_id;
                    self.structs[id as usize].subtree_len = next - start;
                    continue;
                }
                self.structs[id as usize].type_id = next;
                next += 1;
                stack.push((id, true));
                for &child in children[id as usize].iter().rev() {
                    stack.push((child, false));
                }
            }
        }
    }

    fn collect_globals(&mut self) {
        // What each module keeps to itself, recorded before any name is
        // resolved through a module path. Structs and values share the key
        // shape, so one set covers `struct_ids` and `globals` both.
        for m in 0..self.modules.len() {
            for item in &self.modules[m].ast.items {
                let public = match item {
                    ast::Item::Fn(d) => d.is_public,
                    ast::Item::Struct(d) => d.is_public,
                    ast::Item::Const(d) => d.is_public,
                };
                if !public {
                    let key = self.key_in(m, item.name().as_str());
                    self.private.insert(key);
                }
            }
        }
        // First, so that their ids are the ones the library's own functions
        // return. A builtin cannot look an error up by name: it is compiled
        // long before the program that catches it is read.
        for name in wsharp_runtime::builtins::builtin_errors() {
            self.intern_error(name);
        }
        for (i, builtin) in wsharp_runtime::builtins().iter().enumerate() {
            // A prelude builtin is stored unqualified, which is exactly what
            // makes every module see it: `global` falls back to a bare key
            // when the current module has none.
            let key = if builtin.module == wsharp_runtime::builtins::PRELUDE {
                builtin.name.to_string()
            } else {
                format!("{}.{}", builtin.module, builtin.name)
            };
            self.globals
                .insert(key, GlobalRef::Builtin(i as hir::BuiltinId));
        }

        // A struct with no fields is also a value: its single instance, named
        // by the type. That is what makes `handle(req, http.NotFound404)` read
        // the way the lattice is written. Structs with fields are constructed
        // as usual and get no singleton.
        for id in 0..self.structs.len() as StructId {
            if !self.structs[id as usize].fields.is_empty() {
                continue;
            }
            let Some((m, _)) = self.struct_decl_ast(id) else {
                // A materialised status type; it got its singleton when it was
                // created, under the module it belongs to.
                continue;
            };
            let name = self.structs[id as usize].name.clone();
            let key = self.key_in(m, &name);
            if let Some(&span) = self.struct_spans.get(&key) {
                self.global_spans.insert(key.clone(), span);
            }
            self.globals.insert(key, GlobalRef::Singleton(id));
        }

        // Imports come first: a module's own declarations may mention one.
        for m in 0..self.modules.len() {
            self.current = m;
            self.collect_imports(m);
        }

        for m in 0..self.modules.len() {
            self.current = m;
            self.collect_module_globals(m);
        }
        self.current = 0;
    }

    /// Bind each `const name = @import("path");` in this module.
    fn collect_imports(&mut self, m: usize) {
        for item in &self.modules[m].ast.items {
            let ast::Item::Const(decl) = item else {
                continue;
            };
            let ast::Expr::Import { path, span } = &decl.value else {
                continue;
            };
            if let Some(annot) = &decl.ty {
                self.error(
                    annot.span(),
                    "an `@import` binding cannot have a type annotation",
                );
            }
            // The driver says what a specifier resolves to; a standard-library
            // path stands for itself, which is what lets a single-file program
            // be analysed with no driver at all.
            let target = match self.modules[m].imports.get(&**path) {
                // Resolved, but still has to name a module that exists:
                // `@import("std/nope")` resolves to itself and is nothing.
                Some(target) if self.module_paths.contains(target) => target.clone(),
                _ if self.module_paths.contains(&**path) => path.to_string(),
                _ => {
                    self.error(*span, format!("cannot find module `{path}`"))
                        .help = Some(
                        "a path is either a file next to this one, such as \
                         `@import(\"./util.ws\")`, or one of the standard library's, \
                         such as `@import(\"std/http\")`"
                            .into(),
                    );
                    continue;
                }
            };
            if self.imports[m]
                .insert(decl.name.to_string(), target)
                .is_some()
            {
                self.error(
                    decl.name.span,
                    format!("`{}` is declared more than once", decl.name),
                );
            }
            // Also a global, so that using the name on its own is met with
            // "is a module, not a value" rather than "cannot find".
            self.declare_global(decl.name.as_str(), GlobalRef::Module, decl.name.span);
        }
    }

    fn collect_module_globals(&mut self, m: usize) {
        // A `const` bound to a bare name may be naming a function declared
        // further down, so those wait for the whole file to be declared.
        let mut aliases: Vec<&'a ast::ConstDecl> = Vec::new();
        for item in &self.modules[m].ast.items {
            match item {
                ast::Item::Struct(_) => {}
                ast::Item::Fn(decl) => {
                    self.declare_function(&decl.name, &decl.func, decl.span, false);
                }
                ast::Item::Const(decl) => {
                    // Already bound by `collect_imports`.
                    if matches!(decl.value, ast::Expr::Import { .. }) {
                        continue;
                    }
                    // `const name = fn ...` is just another way to declare a
                    // function, and gets generalised like one.
                    if let ast::Expr::Fn(func) = &decl.value {
                        if let Some(annot) = &decl.ty {
                            self.error(
                                annot.span(),
                                "a `const` bound to a `fn` cannot have a type annotation",
                            );
                        }
                        self.declare_function(&decl.name, func, decl.span, false);
                    } else if matches!(decl.value, ast::Expr::Ident(_)) {
                        aliases.push(decl);
                    } else {
                        self.declare_const(decl);
                    }
                }
            }
        }

        for decl in aliases {
            self.declare_func_const(decl);
        }
    }

    /// A top-level `const` bound to a bare name.
    ///
    /// When the name means functions this binds a second name for them: with no
    /// annotation, an alias for the whole set, which dispatches at every call
    /// exactly as the original does; with one, the single member that signature
    /// names. Anything else is a computed global, which W# does not have, and
    /// falls back to the message that says so.
    fn declare_func_const(&mut self, decl: &'a ast::ConstDecl) {
        let ast::Expr::Ident(source) = &decl.value else {
            unreachable!("only `const x = name;` is queued")
        };
        let Some(GlobalRef::Func(ids)) = self.global(source.as_str()).cloned() else {
            self.declare_const(decl);
            return;
        };
        if self.has_global(decl.name.as_str()) {
            self.report_redeclaration(&decl.name);
            return;
        }

        let global = match &decl.ty {
            Some(annot) => {
                let ty = self.resolve_type_expr(annot);
                self.fn_consts.push(FuncConst {
                    ids,
                    ty,
                    span: decl.span,
                    name: source.to_string(),
                    chosen: None,
                    failed: false,
                });
                GlobalRef::FuncValue(self.fn_consts.len() - 1)
            }
            None => GlobalRef::Func(ids),
        };
        self.declare_global(decl.name.as_str(), global, decl.name.span);
    }

    /// Reject type parameter names that would mean something else.
    ///
    /// A type parameter introduces a name into the type namespace, so it can
    /// hide a primitive or a struct. Silently shadowing either turns a typo
    /// into a generic function that compiles and then fails to specialise
    /// somewhere else entirely, which is the wrong place to find out.
    fn check_generic_names(&mut self, generics: &[Ident]) {
        for (i, g) in generics.iter().enumerate() {
            let name = g.as_str();
            if matches!(name, "i64" | "f64" | "bool" | "void" | "str") {
                self.error(g.span, format!("`{name}` is a primitive type"))
                    .help = Some("a type parameter needs a name of its own, such as `T`".into());
            } else if self.struct_named(name).is_some() {
                self.error(g.span, format!("`{name}` is already a type"))
                    .help = Some("a type parameter needs a name of its own, such as `T`".into());
            } else if lookup_abstract(name).is_some() {
                self.error(g.span, format!("`{name}` is an abstract type"))
                    .help = Some("a type parameter needs a name of its own, such as `T`".into());
            } else if generics[..i].iter().any(|p| p.as_str() == name) {
                self.error(g.span, format!("type parameter `{name}` is declared twice"))
                    .help = Some("each type parameter needs a distinct name".into());
            }
        }
    }

    fn declare_function(
        &mut self,
        name: &Ident,
        func: &'a ast::Func,
        span: Span,
        is_closure: bool,
    ) -> hir::FuncId {
        let generics = &func.generics[..];
        let id = self.fn_names.len() as hir::FuncId;
        if !is_closure {
            // A second `fn` of the same name extends an overload set rather
            // than colliding with it. Colliding with anything *else* -- a
            // `const`, a builtin, a zero-field struct's singleton -- is still
            // an error, because dispatch only ranges over functions.
            match self.global(name.as_str()) {
                Some(GlobalRef::Func(_)) => {
                    let Some(GlobalRef::Func(ids)) = self.global_mut(name.as_str()) else {
                        unreachable!("just matched")
                    };
                    ids.push(id);
                }
                Some(_) => self.report_redeclaration(name),
                None => {
                    self.declare_global(name.as_str(), GlobalRef::Func(vec![id]), name.span);
                }
            }
        }
        self.fn_names.push(name.to_string());
        self.fn_modules.push(self.current);
        self.fn_asts.push(Some(func));
        self.fn_spans.push(span);
        self.fn_is_closure.push(is_closure);
        self.fn_types.push(Type::void());
        self.fn_decl_params.push(Vec::new());
        self.fn_member_vars.push(Vec::new());
        self.check_generic_names(generics);
        self.fn_generic_names.push(generics.to_vec());
        self.fn_type_params.push(HashMap::new());
        self.schemes.push(None);
        self.funcs.push(None);
        id
    }

    /// Top-level `const` is restricted to literal values. Anything else would
    /// need global storage and a startup initialiser, which session 1 does not
    /// have; the diagnostic says so.
    fn declare_const(&mut self, decl: &ast::ConstDecl) {
        let (ty, kind) =
            match &decl.value {
                ast::Expr::Int(v, _) => (Type::i64(), hir::ExprKind::Int(*v)),
                ast::Expr::Float(v, _) => (Type::f64(), hir::ExprKind::Float(*v)),
                ast::Expr::Bool(v, _) => (Type::bool(), hir::ExprKind::Bool(*v)),
                ast::Expr::Str(s, _) => {
                    let id = self.intern_string(s);
                    (Type::str(), hir::ExprKind::Str(id))
                }
                ast::Expr::Null(_) => {
                    let inner = self.store.fresh();
                    (Type::optional(inner), hir::ExprKind::Null)
                }
                other => {
                    self.error(other.span(), "a top-level `const` must be a literal or a `fn`")
                    .help = Some(
                    "computed globals need a startup initialiser, which W# does not have yet -- \
                     move the computation into a function"
                        .into(),
                );
                    return;
                }
            };

        if let Some(annot) = &decl.ty {
            let expected = self.resolve_type_expr(annot);
            self.expect(&ty, &expected, decl.value.span(), "this constant");
        }

        if self.has_global(decl.name.as_str()) {
            self.report_redeclaration(&decl.name);
        }
        // `null` is the only literal with a free variable, and generalising it
        // makes `const NOTHING = null;` usable at any optional type.
        let scheme = self.store.generalize(&ty);
        let global = GlobalRef::Const(self.consts.len());
        self.declare_global(decl.name.as_str(), global, decl.name.span);
        self.consts.push(ConstDef { scheme, kind });
    }

    /// `name` collides with a global that already exists. Builtins get their
    /// own message: "declared more than once" would send the reader looking
    /// for a first declaration that is not in the file.
    fn report_redeclaration(&mut self, name: &Ident) {
        let is_builtin = matches!(self.global(name.as_str()), Some(GlobalRef::Builtin(_)));
        let first = self.global_span(name.as_str());
        let (message, help) = if is_builtin {
            (
                format!("`{name}` is a builtin and cannot be redeclared"),
                "builtins cannot be overloaded yet; choose another name",
            )
        } else {
            (
                format!("`{name}` is declared more than once"),
                "only functions may share a name, as an overload set",
            )
        };
        let diag = self.error(name.span, message);
        diag.help = Some(help.into());
        if let Some(first) = first {
            diag.secondary.push(Label {
                span: first,
                message: "first declared here".into(),
            });
        }
    }

    fn resolve_type_expr(&mut self, t: &ast::TypeExpr) -> Type {
        match t {
            ast::TypeExpr::Named(id) => {
                // A type parameter in scope wins over everything: it was
                // checked at its declaration for clashing with a real type.
                if let Some(ty) = self.lookup_type_param(id.as_str()) {
                    return ty;
                }
                match id.as_str() {
                    "i64" => Type::i64(),
                    "f64" => Type::f64(),
                    "bool" => Type::bool(),
                    "void" => Type::void(),
                    "str" => Type::str(),
                    name => match self.lookup_struct(name) {
                        Some(sid) if self.structs[sid as usize].params.is_empty() => {
                            Type::strukt(sid)
                        }
                        Some(sid) => {
                            let arity = self.structs[sid as usize].params.len();
                            self.report_arity(id, arity, 0, id.span);
                            self.store.fresh()
                        }
                        None => {
                            if !self.reject_abstract(name, id.span) {
                                self.error(id.span, format!("unknown type `{name}`"));
                            }
                            // Recover with a fresh variable so one bad annotation
                            // does not cascade into every use of the function.
                            self.store.fresh()
                        }
                    },
                }
            }
            ast::TypeExpr::Path {
                segments,
                args,
                span,
            } => {
                let args: Vec<Type> = args.iter().map(|a| self.resolve_type_expr(a)).collect();
                let name = segments.last().expect("a path has a last segment");
                let found = match self.split_path(segments) {
                    Some(module) => {
                        let found = self.lookup_struct_in(&module, name.as_str());
                        if found.is_some() {
                            let key = format!("{module}.{name}");
                            self.check_visible(&key, name);
                        }
                        found
                    }
                    // No module prefix, so it is an ordinary name with type
                    // arguments: `Box[i64]`.
                    None if segments.len() == 1 => self.lookup_struct(name.as_str()),
                    None => {
                        self.report_unknown_module(segments);
                        return self.store.fresh();
                    }
                };
                match found {
                    Some(id) => {
                        let arity = self.structs[id as usize].params.len();
                        if arity != args.len() {
                            self.report_arity(name, arity, args.len(), *span);
                            return self.store.fresh();
                        }
                        Type::Con(TyCon::Struct(id), args)
                    }
                    None => {
                        self.error(name.span, format!("unknown type `{name}`"));
                        self.store.fresh()
                    }
                }
            }
            ast::TypeExpr::Array { elem, .. } => Type::array(self.resolve_type_expr(elem)),
            ast::TypeExpr::Optional { inner, .. } => Type::optional(self.resolve_type_expr(inner)),
            ast::TypeExpr::ErrUnion { inner, errors, .. } => {
                let inner = self.resolve_type_expr(inner);
                // A written set is fixed and checked; an unwritten one is a
                // variable the solver fills in with what the body raises.
                let set = match errors {
                    Some(names) => {
                        let ids: Vec<hir::ErrorId> =
                            names.iter().map(|n| self.intern_error(n.as_str())).collect();
                        self.store.err_set(ids)
                    }
                    None => self.store.fresh_err_set(),
                };
                Type::err_union(inner, set)
            }
            ast::TypeExpr::Fn { params, ret, .. } => {
                let params = params.iter().map(|p| self.resolve_type_expr(p)).collect();
                let ret = self.resolve_type_expr(ret);
                Type::func(params, ret)
            }
        }
    }

    /// The variable a type parameter stands for, searching innermost first.
    ///
    /// A `fn` literal nested inside a generic function can name that
    /// function's type parameters, which is why this walks the whole stack.
    fn lookup_type_param(&self, name: &str) -> Option<Type> {
        self.type_scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    /// A field's type as seen through a particular instantiation: `value` in
    /// `Box[i64]` is `i64`, not the variable the declaration wrote.
    ///
    /// The identity substitution for every non-generic struct, which is all of
    /// them until one is declared `struct[T]`.
    fn substitute_params(&mut self, id: StructId, args: &[Type], ty: &Type) -> Type {
        let params = self.structs[id as usize].params.clone();
        if params.is_empty() || params.len() != args.len() {
            return ty.clone();
        }
        let subst: HashMap<TypeVarId, Type> =
            params.into_iter().zip(args.iter().cloned()).collect();
        self.store.subst_vars(ty, &subst)
    }

    /// Read the leading segments of a path as a module, returning its path.
    ///
    /// `http.NotFound404` is the module `std/http` and the name `NotFound404`;
    /// `std.http.NotFound404` reaches the same place through an import of
    /// `std`, because a module path is a prefix and each further segment
    /// extends it. `None` means the first segment names no module at all,
    /// which for a single-segment path is the ordinary case.
    fn split_path(&self, segments: &[Ident]) -> Option<String> {
        let mut path = self.imports[self.current]
            .get(segments[0].as_str())?
            .clone();
        // Every segment but the last may extend the module path; the last one
        // is the member being named.
        for segment in &segments[1..segments.len() - 1] {
            path = format!("{path}/{segment}");
            if !self.module_paths.contains(&path) {
                return None;
            }
        }
        Some(path)
    }

    /// A path whose first segment is not an import, and which is therefore not
    /// a name this module has.
    fn report_unknown_module(&mut self, segments: &[Ident]) {
        let first = &segments[0];
        self.error(first.span, format!("cannot find module `{first}`"))
            .help = Some(format!(
            "bind it first, as in `const {first} = @import(\"std/{first}\");`"
        ));
    }

    /// A generic struct named at the wrong number of type arguments.
    fn report_arity(&mut self, name: &Ident, want: usize, got: usize, span: Span) {
        let plural = if want == 1 { "" } else { "s" };
        let example: Vec<String> = (0..want).map(generic_placeholder).collect();
        self.error(
            span,
            format!("`{name}` takes {want} type argument{plural}, but {got} were given"),
        )
        .help = Some(format!("write it as `{name}[{}]`", example.join(", ")));
    }

    /// Report `name` if it is an abstract type, and say so.
    ///
    /// Returns whether it was one, so a caller can skip its own "unknown type"
    /// message: the name does exist, it is just not a name for a type of a
    /// value. Every position other than a parameter's annotation ends up here
    /// -- a supertype, a return type, a local's or a field's annotation, a
    /// value -- because none of them can be given a machine representation.
    fn reject_abstract(&mut self, name: &str, span: Span) -> bool {
        if lookup_abstract(name).is_none() {
            return false;
        }
        self.error(
            span,
            format!(
                "`{name}` is an abstract type: it classifies values for dispatch \
                 and cannot itself be one"
            ),
        )
        .help = Some(format!(
            "write it as a whole parameter's type, as in `fn f(x: {name})`, and name a \
             concrete type such as `i64` everywhere else"
        ));
        true
    }

    // -----------------------------------------------------------------------
    // Binding groups
    // -----------------------------------------------------------------------

    fn infer_all(&mut self) {
        let count = self.fn_asts.len();
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); count];
        for (id, deps) in edges.iter_mut().enumerate() {
            let Some(func) = self.fn_asts[id] else {
                continue;
            };
            let mut names = HashSet::new();
            collect_deps_func(func, &mut names);
            let module = self.fn_modules[id];
            for name in names {
                // Every member of an overload set is a dependency: the call
                // could resolve to any of them, and they must all be
                // generalised together. A `const` bound to a set depends on the
                // same functions -- including one pinned by an annotation,
                // whose member is not known until they have been inferred.
                //
                // A dotted name is a call through a module, whose callee lives
                // in another one; resolving it here is what puts the two in
                // dependency order rather than in arbitrary order.
                let key = match name.split_once('.') {
                    Some((alias, member)) => match self.imports[module].get(alias) {
                        Some(path) => format!("{path}.{member}"),
                        None => continue,
                    },
                    None => self.key_in(module, &name),
                };
                // The `for` protocol resolves in the module that declares the
                // subject's type, which inference has not run yet to know. So
                // every `iter` and `next` this module can see is a dependency.
                //
                // Over-approximating is sound rather than merely convenient: a
                // dependency only matters when it is part of a cycle, and a
                // library's iterator never calls back into the program using
                // it -- so `std/list.next` is simply inferred and generalised
                // first. An iterable declared in the same file genuinely does
                // belong in the same binding group.
                if PROTOCOL_NAMES.contains(&name.as_str()) {
                    for path in self.imports[module].values() {
                        let key = format!("{path}.{name}");
                        if let Some(GlobalRef::Func(ids)) = self.globals.get(&key) {
                            deps.extend(ids.iter().map(|id| *id as usize));
                        }
                    }
                }
                let callees = match self.globals.get(&key) {
                    Some(GlobalRef::Func(ids)) => ids,
                    Some(GlobalRef::FuncValue(index)) => &self.fn_consts[*index].ids,
                    _ => continue,
                };
                deps.extend(callees.iter().map(|c| *c as usize));
            }
        }

        // Tarjan emits a component only after everything it depends on, so the
        // groups come out in dependency order and a callee is always
        // generalised before its callers are inferred.
        for group in tarjan_scc(count, &edges) {
            self.infer_group(&group);
        }
    }

    fn infer_group(&mut self, group: &[usize]) {
        self.store.enter_level();

        // Seed each member with a monomorphic type built from its annotations.
        for &id in group {
            let Some(func) = self.fn_asts[id] else {
                continue;
            };
            let ty = self.signature_type(id as hir::FuncId, func);
            self.fn_types[id] = ty;
        }

        // `fn` literals inside these bodies are declared as inference runs,
        // so anything from here on is part of this group too.
        let first_nested = self.funcs.len();
        for &id in group {
            self.infer_function(id as hir::FuncId);
        }
        let nested: Vec<usize> = (first_nested..self.funcs.len()).collect();

        self.store.exit_level();
        self.solve_constraints();
        self.fixup_fields();

        for &id in group {
            let ty = self.fn_types[id].clone();
            let scheme = self.store.generalize(&ty);
            if let Some(def) = &mut self.funcs[id] {
                def.scheme = scheme.clone();
            }
            self.schemes[id] = Some(scheme);
        }

        self.record_in_group_targs(group, &nested);
    }

    /// Give every call *within* this group the type arguments it makes.
    ///
    /// A call to a function whose group is still being inferred takes the
    /// monomorphic arm of [`Inferencer::func_type`] and so records no type
    /// arguments -- which is right while inferring, because the callee is not
    /// yet generalised, and wrong afterwards: monomorphisation zips the
    /// callee's quantified variables against an empty list, gets an empty
    /// substitution, and queues a second copy of the callee with nothing
    /// substituted into it. That copy still mentions type variables, and
    /// `cannot tell what type X is being used at` is what the reader sees.
    ///
    /// The arguments such a call makes are exactly the callee's own quantified
    /// variables. Hindley-Milner holds a binding group monomorphic, so a call
    /// inside it is always at the variables the callee was inferred with, and
    /// the caller's own substitution then carries them to concrete types. This
    /// is why polymorphic recursion stays out of reach and ordinary recursion
    /// works.
    fn record_in_group_targs(&mut self, group: &[usize], nested: &[usize]) {
        let members: HashSet<hir::FuncId> = group.iter().map(|id| *id as hir::FuncId).collect();
        let targs_for: HashMap<hir::FuncId, Vec<Type>> = members
            .iter()
            .filter_map(|id| {
                let scheme = self.schemes[*id as usize].as_ref()?;
                Some((*id, scheme.vars.iter().map(|v| Type::Var(*v)).collect()))
            })
            .collect();

        for id in group.iter().chain(nested) {
            let Some(def) = &mut self.funcs[*id] else {
                continue;
            };
            let mut body = std::mem::take(&mut def.body);
            patch_targs_block(&mut body, &targs_for);
            self.funcs[*id].as_mut().expect("just matched").body = body;
        }
    }

    /// The function's type as written: annotated parts fixed, the rest fresh.
    ///
    /// Records what the parameters were annotated with, which is not always
    /// what the type says: an abstract annotation becomes a variable here.
    fn signature_type(&mut self, id: hir::FuncId, func: &ast::Func) -> Type {
        // One fresh variable per declared type parameter, created here rather
        // than at declaration time because it must belong to this binding
        // group's level to generalise with the rest of the signature.
        let bindings: HashMap<String, Type> = self.fn_generic_names[id as usize]
            .clone()
            .iter()
            .map(|g| (g.to_string(), self.store.fresh()))
            .collect();
        self.fn_type_params[id as usize] = bindings.clone();
        self.type_scopes.push(bindings);
        let enclosing = self.current;
        self.current = self.fn_modules[id as usize];

        let mut decl = Vec::with_capacity(func.params.len());
        let mut params = Vec::with_capacity(func.params.len());
        for p in &func.params {
            let (ty, written) = match &p.ty {
                Some(t) => self.resolve_param_type_expr(id, t),
                None => {
                    let v = self.store.fresh();
                    (v.clone(), v)
                }
            };
            params.push(ty);
            decl.push(written);
        }
        self.fn_decl_params[id as usize] = decl;
        let ret = match &func.ret {
            Some(t) => self.resolve_type_expr(t),
            None => self.store.fresh(),
        };
        self.type_scopes.pop();
        self.current = enclosing;
        Type::func(params, ret)
    }

    /// Resolve a parameter's annotation, which is the one place an abstract
    /// type may be written.
    ///
    /// It does not name the parameter's type there -- it constrains it. `x` in
    /// `fn area(x: Number)` gets a fresh variable plus a [`Constraint::Member`],
    /// so the function generalises to `fn(T) ...` and is monomorphised per
    /// concrete argument like any other generic. That is what gives `x` a
    /// machine representation: there is none for "a number", but there is for
    /// each type the function is used at.
    ///
    /// Returns the type to check against and the type as written.
    fn resolve_param_type_expr(&mut self, id: hir::FuncId, t: &ast::TypeExpr) -> (Type, Type) {
        let ast::TypeExpr::Named(name) = t else {
            return self.resolve_value_type_expr(t);
        };
        if self.struct_named(name.as_str()).is_some() {
            return self.resolve_value_type_expr(t);
        }
        let Some(abstract_id) = lookup_abstract(name.as_str()) else {
            return self.resolve_value_type_expr(t);
        };
        let var = self.store.fresh();
        self.constraints.push(Constraint::Member {
            ty: var.clone(),
            id: abstract_id,
            span: name.span,
        });
        self.fn_member_vars[id as usize].push((var.clone(), abstract_id, name.span));
        (var, Type::abstrakt(abstract_id))
    }

    /// [`Inferencer::resolve_type_expr`], paired with itself: everywhere but a
    /// parameter, what is written is what is checked.
    fn resolve_value_type_expr(&mut self, t: &ast::TypeExpr) -> (Type, Type) {
        let ty = self.resolve_type_expr(t);
        (ty.clone(), ty)
    }

    /// Infer one function's body. Returns the locals *in the enclosing frame*
    /// that this function captures, which is what the enclosing function needs
    /// in order to build the closure. Always empty for a top-level function.
    fn infer_function(&mut self, id: hir::FuncId) -> Vec<hir::LocalId> {
        let Some(func) = self.fn_asts[id as usize] else {
            return Vec::new();
        };
        let fn_ty = self.fn_types[id as usize].clone();
        let (params, ret) = match fn_ty.as_fn() {
            Some((p, r)) => (p.to_vec(), r.clone()),
            None => return Vec::new(),
        };

        let mut frame = Frame {
            locals: Vec::new(),
            params: Vec::new(),
            captures: Vec::new(),
            capture_sources: Vec::new(),
            captured_names: HashMap::new(),
            scopes: vec![Vec::new()],
            ret: ret.clone(),
            self_local: None,
            self_used: false,
        };
        // Bound before the parameters, so a parameter of the same name shadows
        // it -- the ordinary rule, and the one a reader expects.
        if let Some((name, ty)) = self.pending_self.take() {
            let local = frame.add_local(&name, ty, false, self.fn_spans[id as usize]);
            frame.bind_local(&name, local);
            frame.self_local = Some(local);
        }
        for (param, ty) in func.params.iter().zip(&params) {
            let local = frame.add_local(param.name.as_str(), ty.clone(), false, param.span);
            frame.bind_local(param.name.as_str(), local);
            frame.params.push(local);
        }
        self.frames.push(frame);
        self.type_scopes
            .push(self.fn_type_params[id as usize].clone());
        // Unqualified names in the body mean what they mean where it was
        // written, which is not where its binding group happens to start.
        let enclosing = self.current;
        self.current = self.fn_modules[id as usize];

        let body = self.infer_block(&func.body);

        self.type_scopes.pop();
        self.current = enclosing;

        // A function that can fall off the end must return nothing. This also
        // guarantees code generation can terminate every block.
        let ret_is_void = matches!(self.store.resolve(&ret), Type::Con(TyCon::Void, _));
        if !ret_is_void && !block_terminates(&func.body) {
            let shown = self.store.show(&ret);
            let end = Span::new(
                func.body.span.end.saturating_sub(1) as usize,
                func.body.span.end as usize,
            );
            self.error(end, "not every path returns a value").help = Some(format!(
                "this function returns `{shown}`, so every path needs a `return`"
            ));
        }

        let frame = self.frames.pop().expect("frame was pushed");
        let capture_sources = frame.capture_sources;
        self.funcs[id as usize] = Some(hir::FuncDef {
            name: self.fn_names[id as usize].clone(),
            params: frame.params,
            captures: frame.captures,
            locals: frame.locals,
            ret,
            body,
            scheme: Scheme::mono(fn_ty),
            is_closure: self.fn_is_closure[id as usize],
            // Only when the body named itself: an unused self local would make
            // the code generator keep the environment as a root in every
            // literal, which is a change no existing program asked for.
            self_local: frame.self_used.then_some(frame.self_local).flatten(),
            span: self.fn_spans[id as usize],
        });
        capture_sources
    }

    fn frame(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .expect("inference is inside a function")
    }

    // -----------------------------------------------------------------------
    // Statements
    // -----------------------------------------------------------------------

    fn infer_block(&mut self, block: &'a ast::Block) -> hir::Block {
        self.frame().scopes.push(Vec::new());
        let stmts = block
            .stmts
            .iter()
            .filter_map(|s| self.infer_stmt(s))
            .collect();
        self.frame().scopes.pop();
        hir::Block { stmts }
    }

    fn infer_stmt(&mut self, stmt: &'a ast::Stmt) -> Option<hir::Stmt> {
        match stmt {
            ast::Stmt::Let(let_stmt) => {
                if let Some(stmt) = self.infer_let_of_overloads(let_stmt) {
                    return stmt;
                }
                if let Some(stmt) = self.infer_let_of_fn_literal(let_stmt) {
                    return stmt;
                }
                let annotated = let_stmt.ty.as_ref().map(|t| self.resolve_type_expr(t));
                // A struct literal is told what it is being checked against
                // before its fields are inferred. Its type arguments otherwise
                // come only from the field values, and `Pair[?i64, i64]` would
                // reject `.first = 5` -- the coercion into an optional that
                // every other annotated binding gets.
                let mut init = match &annotated {
                    Some(expected) => {
                        let expected = expected.clone();
                        self.infer_expecting(&let_stmt.init, &expected)
                    }
                    None => self.infer_expr(&let_stmt.init),
                };
                if let Some(expected) = &annotated {
                    init = self.coerce(init, expected, "this initialiser");
                }
                let ty = annotated.unwrap_or_else(|| init.ty.clone());
                let local = self.frame().add_local(
                    let_stmt.name.as_str(),
                    ty,
                    let_stmt.mutable,
                    let_stmt.name.span,
                );
                self.frame().bind_local(let_stmt.name.as_str(), local);
                Some(hir::Stmt::Let { local, init })
            }

            ast::Stmt::Assign(assign) => {
                let (place, target_ty) = self.infer_place(&assign.target)?;
                let value = if let Some(op) = assign.op {
                    // `x += e` is checked as `x = x + e`, so the operand rules
                    // (numeric, matching types) apply unchanged.
                    let lhs = place_as_expr(&place, &target_ty, assign.target.span());
                    let rhs = self.infer_expr(&assign.value);
                    self.infer_binary(op, lhs, rhs, assign.span)
                } else {
                    self.infer_expr(&assign.value)
                };
                let value = self.coerce(value, &target_ty, "this assignment");
                Some(hir::Stmt::Assign { place, value })
            }

            ast::Stmt::Expr(expr) => Some(hir::Stmt::Expr(self.infer_expr(expr))),

            ast::Stmt::Return { value, span } => {
                let ret = self.frames.last().expect("inside a function").ret.clone();
                match value {
                    Some(expr) => {
                        let value = self.infer_expr(expr);
                        let value = self.coerce(value, &ret, "this return value");
                        Some(hir::Stmt::Return(Some(value)))
                    }
                    None => {
                        self.expect(&Type::void(), &ret, *span, "this `return`");
                        Some(hir::Stmt::Return(None))
                    }
                }
            }

            ast::Stmt::If(if_stmt) => self.infer_if_stmt(if_stmt),

            ast::Stmt::While(while_stmt) => {
                let (cond, capture) =
                    self.infer_condition(&while_stmt.cond, while_stmt.capture.as_ref());
                self.frame().scopes.push(Vec::new());
                if let (Some(name), Some(local)) = (&while_stmt.capture, capture) {
                    self.frame().bind_local(name.as_str(), local);
                }
                self.loop_depth += 1;
                let body = self.infer_block(&while_stmt.body);
                let cont = while_stmt
                    .cont
                    .as_deref()
                    .and_then(|s| self.infer_stmt(s))
                    .map(Box::new);
                self.loop_depth -= 1;
                self.frame().scopes.pop();
                Some(hir::Stmt::While {
                    cond,
                    capture,
                    cont,
                    body,
                })
            }

            ast::Stmt::For(for_stmt) => self.infer_for_stmt(for_stmt),

            ast::Stmt::Block(block) => Some(hir::Stmt::Block(self.infer_block(block))),

            ast::Stmt::Break(span) => {
                if self.loop_depth == 0 {
                    self.error(*span, "`break` outside of a loop");
                }
                Some(hir::Stmt::Break)
            }
            ast::Stmt::Continue(span) => {
                if self.loop_depth == 0 {
                    self.error(*span, "`continue` outside of a loop");
                }
                Some(hir::Stmt::Continue)
            }
        }
    }

    /// `const g = f;` and `const g: fn(Base) i64 = f;`, where `f` names several
    /// functions.
    ///
    /// Without an annotation `g` is a second name for the whole set and emits
    /// no statement at all -- there is nothing to store, since which member a
    /// call means is decided per call. With one, the annotation picks a member
    /// and `g` is an ordinary function value.
    ///
    /// The outer `Option` says whether this took the statement over.
    fn infer_let_of_overloads(&mut self, let_stmt: &'a ast::LetStmt) -> Option<Option<hir::Stmt>> {
        let ast::Expr::Ident(source) = &let_stmt.init else {
            return None;
        };
        // `var g = f;` is not an alias: a set is not a value, so there would be
        // nothing to reassign. It falls through to the ordinary path, whose
        // error explains that.
        if let_stmt.mutable {
            return None;
        }
        let ids = self.overload_set(source.as_str())?;
        if ids.len() < 2 {
            // One function is a value already, and takes the ordinary path.
            return None;
        }
        let span = let_stmt.name.span;
        let Some(annot) = &let_stmt.ty else {
            self.frame()
                .bind(let_stmt.name.as_str(), Binding::Overloads(ids));
            return Some(None);
        };

        let want = self.resolve_type_expr(annot);
        let init = match self.select_overload(source.as_str(), &ids, &want, let_stmt.span) {
            Some(chosen) => {
                let (ty, targs) = self.func_type(chosen);
                // Selection proved these unify; doing it again is what binds
                // the type arguments this value is taken at.
                let _ = self.store.try_unify(&ty, &want);
                self.note_member_constraints(chosen, &targs, span);
                hir::ExprKind::Closure {
                    func: chosen,
                    targs,
                    captures: Vec::new(),
                }
            }
            // Reported; carry on with a well-formed local so that uses of `g`
            // do not each add "cannot find `g` in this scope".
            None => hir::ExprKind::Null,
        };
        let local = self
            .frame()
            .add_local(let_stmt.name.as_str(), want.clone(), false, span);
        self.frame().bind_local(let_stmt.name.as_str(), local);
        Some(Some(hir::Stmt::Let {
            local,
            init: hir::Expr {
                kind: init,
                ty: want,
                span,
            },
        }))
    }

    /// `const f = fn (x) { .. };`, which may be a definition rather than a
    /// value.
    ///
    /// A `fn` literal bound to a `const` is inferred at a level of its own and
    /// generalised there -- ordinary let-polymorphism. If that yields type
    /// variables, `f` cannot be a local: a closure value is one code pointer,
    /// and two instantiations need two. So the name binds a *definition*, as
    /// `const g = f;` does for an overload set, and each use materialises a
    /// closure at the type it needs.
    ///
    /// Two forms stay values, and both for the same reason -- they name one
    /// type. `var f = fn ...` is a storage location holding one function value;
    /// `const f: fn(i64) i64 = fn ...` says which one it is.
    ///
    /// The outer `Option` says whether this took the statement over.
    fn infer_let_of_fn_literal(&mut self, let_stmt: &'a ast::LetStmt) -> Option<Option<hir::Stmt>> {
        let ast::Expr::Fn(func) = &let_stmt.init else {
            return None;
        };
        if let_stmt.mutable || let_stmt.ty.is_some() {
            return None;
        }
        let span = let_stmt.init.span();

        // A level of its own: the literal's fresh variables sit one deeper than
        // the enclosing binding group, so generalising quantifies exactly them.
        // Anything that escapes into the enclosing frame -- a capture's type, a
        // type parameter of the function this sits in -- has had its level
        // lowered by `occurs_and_adjust` on the way out, and is left alone.
        self.store.enter_level();
        let (id, fn_ty, capture_sources) =
            self.infer_fn_literal_named(func, span, Some(let_stmt.name.as_str()));
        self.store.exit_level();

        let scheme = self.generalize_definition(&fn_ty);
        self.schemes[id as usize] = Some(scheme.clone());
        if let Some(def) = &mut self.funcs[id as usize] {
            def.scheme = scheme.clone();
        }

        if !scheme.is_generic() {
            // Nothing to quantify, so this is an ordinary function value and
            // takes the path every `fn` literal took before definitions
            // existed.
            let init = self.closure_value(id, fn_ty, &capture_sources, span);
            let local = self.frame().add_local(
                let_stmt.name.as_str(),
                init.ty.clone(),
                false,
                let_stmt.name.span,
            );
            self.frame().bind_local(let_stmt.name.as_str(), local);
            return Some(Some(hir::Stmt::Let { local, init }));
        }

        // Captured values are snapshotted here rather than read at each use, so
        // a captured `var` assigned afterwards cannot change what the closure
        // sees. Captures are by value, and this is where that value is.
        let mut stmts = Vec::new();
        let mut names = Vec::new();
        for (i, &source) in capture_sources.iter().enumerate() {
            let ty = self.frames.last().expect("in a function").locals[source as usize]
                .ty
                .clone();
            // Bracketed, as the `for` desugaring's locals are: no source name
            // can collide with one, so a definition cannot shadow anything.
            let name = format!("[{}#{i}]", let_stmt.name);
            let local = self
                .frame()
                .add_local(&name, ty.clone(), false, let_stmt.name.span);
            self.frame().bind_local(&name, local);
            stmts.push(hir::Stmt::Let {
                local,
                init: hir::Expr {
                    kind: hir::ExprKind::Local(source),
                    ty,
                    span,
                },
            });
            names.push(name);
        }

        self.frame().bind(
            let_stmt.name.as_str(),
            Binding::Definition {
                func: id,
                captures: names,
            },
        );
        if stmts.is_empty() {
            return Some(None);
        }
        Some(Some(hir::Stmt::Block(hir::Block { stmts })))
    }

    /// `for (xs) |x| { .. }` becomes the `while` it stands for.
    ///
    /// Desugaring here rather than adding a HIR statement keeps the loop's
    /// back-edge safepoint, its `break`/`continue` handling and its lowering
    /// exactly the ones `while` already has. A second loop form in the code
    /// generator would be a second place for the poll to be forgotten.
    ///
    /// The array goes into a hidden local so it is evaluated once --
    /// `for (build()) |x|` must not rebuild it on every test -- and so that it
    /// is an ordinary root for the collector, which a temporary would not be.
    fn infer_for_stmt(&mut self, for_stmt: &'a ast::ForStmt) -> Option<hir::Stmt> {
        let span = for_stmt.span;
        let iter = self.infer_expr(&for_stmt.iter);
        // An array is walked by index, which is a load and a compare and needs
        // no library at all. Anything else has to say how it is walked, and a
        // struct is the only other thing that can. A subject still a variable
        // takes the array path too, and so records `Indexable`: the protocol
        // has to be chosen here, and there is nothing yet to choose it from.
        if let Type::Con(TyCon::Struct(id), _) = self.store.resolve(&iter.ty) {
            return self.infer_for_protocol(for_stmt, iter, id);
        }
        let elem = self.element_of(&iter.ty, for_stmt.iter.span());
        let arr_ty = iter.ty.clone();

        // The scope holding the hidden locals, so the user's body cannot see
        // them and a nested `for` gets its own.
        self.frame().scopes.push(Vec::new());
        // Bracketed names cannot collide with anything the user can write.
        let arr_local = self
            .frame()
            .add_local("[array]", arr_ty.clone(), false, span);
        let index_local = self.frame().add_local("[index]", Type::i64(), true, span);

        let int = |v: i64| hir::Expr {
            kind: hir::ExprKind::Int(v),
            ty: Type::i64(),
            span,
        };
        let index_ref = || hir::Expr {
            kind: hir::ExprKind::Local(index_local),
            ty: Type::i64(),
            span,
        };
        let array_ref = || hir::Expr {
            kind: hir::ExprKind::Local(arr_local),
            ty: arr_ty.clone(),
            span,
        };

        // `[index] < len([array])`
        let cond = hir::Expr {
            kind: hir::ExprKind::Binary {
                op: BinOp::Lt,
                lhs: Box::new(index_ref()),
                rhs: Box::new(hir::Expr {
                    kind: hir::ExprKind::ArrayLen {
                        arr: Box::new(array_ref()),
                    },
                    ty: Type::i64(),
                    span,
                }),
            },
            ty: Type::bool(),
            span,
        };

        // `[index] += 1`, as the continue expression, so an explicit
        // `continue` still advances the loop.
        let step = hir::Stmt::Assign {
            place: hir::Place::Local(index_local),
            value: hir::Expr {
                kind: hir::ExprKind::Binary {
                    op: BinOp::Add,
                    lhs: Box::new(index_ref()),
                    rhs: Box::new(int(1)),
                },
                ty: Type::i64(),
                span,
            },
        };

        // The user's bindings, in a scope of their own so the body sees them.
        self.frame().scopes.push(Vec::new());
        let value_local = self.frame().add_local(
            for_stmt.value.as_str(),
            elem.clone(),
            false,
            for_stmt.value.span,
        );
        self.frame()
            .bind_local(for_stmt.value.as_str(), value_local);
        let mut prologue = vec![hir::Stmt::Let {
            local: value_local,
            init: hir::Expr {
                kind: hir::ExprKind::Index {
                    arr: Box::new(array_ref()),
                    index: Box::new(index_ref()),
                },
                ty: elem,
                span,
            },
        }];
        // The index is copied into a binding of its own rather than being the
        // counter: the counter has to be mutable, and the user's is not.
        if let Some(name) = &for_stmt.index {
            let local = self
                .frame()
                .add_local(name.as_str(), Type::i64(), false, name.span);
            self.frame().bind_local(name.as_str(), local);
            prologue.push(hir::Stmt::Let {
                local,
                init: index_ref(),
            });
        }

        self.loop_depth += 1;
        let mut body = self.infer_block(&for_stmt.body);
        self.loop_depth -= 1;
        self.frame().scopes.pop();

        prologue.append(&mut body.stmts);
        let body = hir::Block { stmts: prologue };

        self.frame().scopes.pop();

        Some(hir::Stmt::Block(hir::Block {
            stmts: vec![
                hir::Stmt::Let {
                    local: arr_local,
                    init: iter,
                },
                hir::Stmt::Let {
                    local: index_local,
                    init: int(0),
                },
                hir::Stmt::While {
                    cond,
                    capture: None,
                    cont: Some(Box::new(step)),
                    body,
                },
            ],
        }))
    }

    /// `for (xs) |x| { .. }` over something that is not an array.
    ///
    /// The subject's own module says how it is walked: `iter` turns it into an
    /// iterator and `next` produces `?T` until it produces null, which is
    /// exactly the shape `while (cond) |v|` already has. So this desugars to
    ///
    /// ```text
    /// { const [iter] = M.iter(xs); while (M.next([iter])) |x| { .. } }
    /// ```
    ///
    /// and inherits the back-edge safepoint, `break`, `continue` and the
    /// lowering from `while`, as the array form does.
    ///
    /// Resolved in the module that *declares the type* rather than in the one
    /// the loop is written in. A `for` over a `List` would otherwise need
    /// `list.next` in scope, which would make the protocol a thing the caller
    /// has to import rather than a thing the type has.
    fn infer_for_protocol(
        &mut self,
        for_stmt: &'a ast::ForStmt,
        subject: hir::Expr,
        strukt: StructId,
    ) -> Option<hir::Stmt> {
        let span = for_stmt.span;
        let at = for_stmt.iter.span();

        // Everything that can fail is resolved before a scope is opened, so a
        // failure leaves the frame exactly as it found it.
        let iter_ids = self.protocol_fn(strukt, "iter", at)?;
        let next_ids = self.protocol_fn(strukt, "next", at)?;

        let iter_name = Ident::new("iter", at);
        let iterator = self.dispatched_call(&iter_name, &iter_ids, vec![subject], at);
        let iter_ty = iterator.ty.clone();

        // The scope holding the hidden locals, so the user's body cannot see
        // them and a nested `for` gets its own.
        self.frame().scopes.push(Vec::new());
        let iter_local = self.frame().add_local("[iter]", iter_ty.clone(), false, span);
        let index_local = for_stmt
            .index
            .as_ref()
            .map(|_| self.frame().add_local("[index]", Type::i64(), true, span));

        let next_name = Ident::new("next", at);
        let cond = self.dispatched_call(
            &next_name,
            &next_ids,
            vec![hir::Expr {
                kind: hir::ExprKind::Local(iter_local),
                ty: iter_ty,
                span,
            }],
            at,
        );
        // `next` is what says when the walk is over, so it has to be able to
        // say so: an optional, whose null is the end.
        let elem = self.store.fresh();
        let want = Type::optional(elem.clone());
        let cond_ty = cond.ty.clone();
        let cond = if self.store.try_unify(&cond_ty, &want) {
            cond
        } else {
            let shown = self.store.show(&cond_ty);
            self.error(at, format!("`next` must return an optional, not `{shown}`"))
                .help = Some(
                "a `for` stops when `next` produces null, so it needs somewhere to put that"
                    .into(),
            );
            hir::Expr {
                kind: hir::ExprKind::Null,
                ty: want,
                span,
            }
        };

        // `[index] += 1`, as the continue expression, so an explicit
        // `continue` still advances the count. `next` advances the walk itself,
        // because the condition is what calls it.
        let cont = index_local.map(|index| {
            let index_ref = || hir::Expr {
                kind: hir::ExprKind::Local(index),
                ty: Type::i64(),
                span,
            };
            Box::new(hir::Stmt::Assign {
                place: hir::Place::Local(index),
                value: hir::Expr {
                    kind: hir::ExprKind::Binary {
                        op: BinOp::Add,
                        lhs: Box::new(index_ref()),
                        rhs: Box::new(hir::Expr {
                            kind: hir::ExprKind::Int(1),
                            ty: Type::i64(),
                            span,
                        }),
                    },
                    ty: Type::i64(),
                    span,
                },
            })
        });

        // The user's bindings, in a scope of their own so the body sees them.
        self.frame().scopes.push(Vec::new());
        let value_local =
            self.frame()
                .add_local(for_stmt.value.as_str(), elem, false, for_stmt.value.span);
        self.frame()
            .bind_local(for_stmt.value.as_str(), value_local);
        let mut prologue = Vec::new();
        if let (Some(name), Some(index)) = (&for_stmt.index, index_local) {
            let local = self
                .frame()
                .add_local(name.as_str(), Type::i64(), false, name.span);
            self.frame().bind_local(name.as_str(), local);
            prologue.push(hir::Stmt::Let {
                local,
                init: hir::Expr {
                    kind: hir::ExprKind::Local(index),
                    ty: Type::i64(),
                    span,
                },
            });
        }

        self.loop_depth += 1;
        let mut body = self.infer_block(&for_stmt.body);
        self.loop_depth -= 1;
        self.frame().scopes.pop();

        prologue.append(&mut body.stmts);
        let body = hir::Block { stmts: prologue };

        self.frame().scopes.pop();

        let mut stmts = vec![hir::Stmt::Let {
            local: iter_local,
            init: iterator,
        }];
        if let Some(index) = index_local {
            stmts.push(hir::Stmt::Let {
                local: index,
                init: hir::Expr {
                    kind: hir::ExprKind::Int(0),
                    ty: Type::i64(),
                    span,
                },
            });
        }
        stmts.push(hir::Stmt::While {
            cond,
            capture: Some(value_local),
            cont,
            body,
        });
        Some(hir::Stmt::Block(hir::Block { stmts }))
    }

    /// The overload set of `name` in the module that declares `strukt`.
    fn protocol_fn(
        &mut self,
        strukt: StructId,
        name: &str,
        span: Span,
    ) -> Option<Vec<hir::FuncId>> {
        let shown = self.structs[strukt as usize].name.clone();
        let module = self
            .struct_decl_of
            .get(&strukt)
            .map(|&(m, _)| self.modules[m].path.clone());
        let found = module
            .as_ref()
            .and_then(|path| self.globals.get(&format!("{path}.{name}")));
        if let Some(GlobalRef::Func(ids)) = found {
            let ids = ids.clone();
            if let Some(path) = &module {
                let key = format!("{path}.{name}");
                let at = Ident::new(name, span);
                self.check_visible(&key, &at);
            }
            return Some(ids);
        }
        self.error(span, format!("`{shown}` cannot be iterated")).help = Some(format!(
            "a `for` over a struct calls `iter` and `next` from the module that declares it; \
             `{shown}` has no `{name}`"
        ));
        None
    }

    /// `{ stmt; stmt; value }`, the block form of a `catch` or an `orelse`.
    ///
    /// A block that ends in an expression has that expression's type. A block
    /// with no trailing expression has to *leave* instead -- `return`, `break`
    /// or `continue` -- and then its type is a fresh variable: it produces
    /// nothing, so it fits wherever it is written, which is what makes
    /// `f() catch return false;` check in a function returning `bool` and in
    /// one returning `str` alike. That is the whole of "diverging" here; no
    /// `noreturn` type is needed for it.
    fn infer_value_block(
        &mut self,
        stmts: &'a [ast::Stmt],
        value: &'a Option<Box<ast::Expr>>,
        span: Span,
    ) -> hir::Expr {
        self.frame().scopes.push(Vec::new());
        let lowered: Vec<hir::Stmt> = stmts.iter().filter_map(|s| self.infer_stmt(s)).collect();
        let value = value.as_deref().map(|v| self.infer_expr(v));
        self.frame().scopes.pop();

        let ty = match &value {
            Some(v) => v.ty.clone(),
            None if stmts_terminate(stmts) => self.store.fresh(),
            None => {
                self.error(span, "this block never produces a value").help = Some(
                    "end it with an expression and no `;`, or leave with `return`, `break` \
                     or `continue`"
                        .into(),
                );
                self.store.fresh()
            }
        };
        hir::Expr {
            kind: hir::ExprKind::Block {
                stmts: lowered,
                value: value.map(Box::new),
            },
            ty,
            span,
        }
    }

    fn infer_if_stmt(&mut self, if_stmt: &'a ast::IfStmt) -> Option<hir::Stmt> {
        let (cond, capture) = self.infer_condition(&if_stmt.cond, if_stmt.capture.as_ref());
        self.frame().scopes.push(Vec::new());
        if let (Some(name), Some(local)) = (&if_stmt.capture, capture) {
            self.frame().bind_local(name.as_str(), local);
        }
        let then = self.infer_block(&if_stmt.then);
        self.frame().scopes.pop();

        let els = match if_stmt.else_.as_deref() {
            Some(ast::ElseBranch::Block(b)) => Some(self.infer_block(b)),
            // `else if` becomes a block holding the nested `if`.
            Some(ast::ElseBranch::If(inner)) => {
                let stmt = self.infer_if_stmt(inner)?;
                Some(hir::Block { stmts: vec![stmt] })
            }
            None => None,
        };
        Some(hir::Stmt::If {
            cond,
            capture,
            then,
            els,
        })
    }

    /// A condition is either a `bool`, or -- when the construct binds a payload
    /// with `|v|` -- an optional whose payload becomes the bound local.
    fn infer_condition(
        &mut self,
        cond: &'a ast::Expr,
        capture: Option<&Ident>,
    ) -> (hir::Expr, Option<hir::LocalId>) {
        let cond = self.infer_expr(cond);
        match capture {
            None => {
                let cond = self.coerce(cond, &Type::bool(), "this condition");
                (cond, None)
            }
            Some(name) => {
                let payload = self.store.fresh();
                let expected = Type::optional(payload.clone());
                let span = cond.span;
                let cond = self.coerce(cond, &expected, "this condition");
                let local = self.frame().add_local(name.as_str(), payload, false, span);
                (cond, Some(local))
            }
        }
    }

    /// Resolve an assignment target, returning the place and the type stored
    /// there.
    fn infer_place(&mut self, target: &'a ast::Expr) -> Option<(hir::Place, Type)> {
        match target {
            ast::Expr::Ident(name)
                if matches!(
                    self.lookup_binding(name.as_str()),
                    Some(Binding::Definition { .. })
                ) =>
            {
                self.error(
                    name.span,
                    format!("cannot assign to `{name}`, which is a generic `fn`"),
                )
                .help = Some(
                    "a generic `fn` is a definition rather than a value, so there is nothing \
                     to reassign -- bind it with `var` to get one function value instead"
                        .into(),
                );
                None
            }
            ast::Expr::Ident(name) => match self.lookup_local(name.as_str()) {
                Some(local) => {
                    let frame = self.frames.last().expect("in a function");
                    let def = &frame.locals[local as usize];
                    let ty = def.ty.clone();
                    if !def.mutable {
                        let is_capture = frame.captures.contains(&local);
                        let (msg, help) = if is_capture {
                            (
                                format!("cannot assign to `{name}`, which this closure captured"),
                                "closures capture by value, so captured bindings are read-only",
                            )
                        } else {
                            (
                                format!("cannot assign to `{name}`, which is `const`"),
                                "declare it with `var` to make it mutable",
                            )
                        };
                        self.error(name.span, msg).help = Some(help.into());
                    }
                    Some((hir::Place::Local(local), ty))
                }
                None => {
                    self.error(name.span, format!("cannot find `{name}` in this scope"));
                    None
                }
            },
            ast::Expr::Field { obj, name, .. } => {
                // A compound assignment reads the place and writes it back, so
                // the object expression is evaluated twice. Restricting the base
                // to a variable or field chain keeps that from being observable.
                if !is_place_base(obj) {
                    self.error(obj.span(), "cannot assign through this expression")
                        .help = Some(
                        "the left of a `.` in an assignment must be a variable or a field of one"
                            .into(),
                    );
                    return None;
                }
                let obj = self.infer_expr(obj);
                let (strukt, index, ty) = self.field_of(&obj.ty, name);
                Some((
                    hir::Place::Field {
                        obj,
                        strukt,
                        index,
                        name: name.as_str().into(),
                    },
                    ty,
                ))
            }
            ast::Expr::Index { obj, index, .. } => {
                // As for a field: a compound assignment evaluates the target
                // twice, so the base has to be something re-reading is free of.
                if !is_place_base(obj) {
                    self.error(obj.span(), "cannot assign through this expression")
                        .help = Some(
                        "the left of a `[` in an assignment must be a variable or a field of one"
                            .into(),
                    );
                    return None;
                }
                let (arr, index, elem) = self.infer_index(obj, index);
                Some((hir::Place::Index { arr, index }, elem))
            }
            other => {
                self.error(other.span(), "cannot assign to this expression");
                None
            }
        }
    }

    /// The shared half of `a[i]`, as an expression and as a place: infer both
    /// sides, require an `i64` index, and produce the element type.
    fn infer_index(
        &mut self,
        obj: &'a ast::Expr,
        index: &'a ast::Expr,
    ) -> (hir::Expr, hir::Expr, Type) {
        let arr = self.infer_expr(obj);
        let index = self.infer_expr(index);
        let index = self.coerce(index, &Type::i64(), "this index");
        let elem = self.element_of(&arr.ty, obj.span());
        (arr, index, elem)
    }

    /// The element type of `[]T`, deferred when the array's type is still a
    /// variable. See [`Constraint::Indexable`].
    fn element_of(&mut self, arr: &Type, span: Span) -> Type {
        match self.store.resolve(arr) {
            Type::Con(TyCon::Array, args) => args[0].clone(),
            Type::Var(_) => {
                let elem = self.store.fresh();
                self.constraints.push(Constraint::Indexable {
                    obj: arr.clone(),
                    elem: elem.clone(),
                    span,
                });
                elem
            }
            other => {
                let shown = self.store.show(&other);
                self.error(span, format!("`{shown}` cannot be indexed"))
                    .help = Some("only an array `[]T` can be indexed".into());
                self.store.fresh()
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    fn infer_expr(&mut self, expr: &'a ast::Expr) -> hir::Expr {
        let span = expr.span();
        match expr {
            ast::Expr::Block { stmts, value, .. } => self.infer_value_block(stmts, value, span),
            ast::Expr::Int(v, _) => self.lit(hir::ExprKind::Int(*v), Type::i64(), span),
            ast::Expr::Float(v, _) => self.lit(hir::ExprKind::Float(*v), Type::f64(), span),
            ast::Expr::Bool(v, _) => self.lit(hir::ExprKind::Bool(*v), Type::bool(), span),
            ast::Expr::Str(s, _) => {
                let id = self.intern_string(s);
                self.lit(hir::ExprKind::Str(id), Type::str(), span)
            }
            ast::Expr::Null(_) => {
                let inner = self.store.fresh();
                self.lit(hir::ExprKind::Null, Type::optional(inner), span)
            }

            ast::Expr::Ident(name) => self.infer_ident(name),

            ast::Expr::ErrorLit { name, .. } => {
                let id = self.intern_error(name.as_str());
                let payload = self.store.fresh();
                // A fresh set rather than the singleton `{name}` itself: this
                // literal is about to be unified with whatever it is returned
                // into, and the *union* of every such contribution is what
                // that function's set should be. The constraint is what
                // records the contribution; the solver adds them up.
                let set = self.store.fresh_err_set();
                self.constraints.push(Constraint::ErrorSetHas {
                    set: set.clone(),
                    errors: vec![id],
                    widens: true,
                    span,
                });
                self.lit(hir::ExprKind::Err(id), Type::err_union(payload, set), span)
            }

            ast::Expr::Unary {
                op, expr: inner, ..
            } => {
                let inner = self.infer_expr(inner);
                let ty = match op {
                    UnOp::Neg => {
                        self.constraints.push(Constraint::Numeric {
                            ty: inner.ty.clone(),
                            span,
                            op: "-",
                            integers_only: false,
                        });
                        inner.ty.clone()
                    }
                    UnOp::Not => {
                        self.expect(&inner.ty, &Type::bool(), inner.span, "the operand of `!`");
                        Type::bool()
                    }
                };
                hir::Expr {
                    kind: hir::ExprKind::Unary {
                        op: *op,
                        expr: Box::new(inner),
                    },
                    ty,
                    span,
                }
            }

            ast::Expr::Binary { op, lhs, rhs, .. } => {
                let lhs = self.infer_expr(lhs);
                let rhs = self.infer_expr(rhs);
                self.infer_binary(*op, lhs, rhs, span)
            }

            ast::Expr::Call { callee, args, .. } => self.infer_call(callee, args, span),

            ast::Expr::Field { obj, name, .. } => {
                // `http.NotFound404` is a name reached through a module, not a
                // field of a value called `http`. Modules are checked first,
                // and only when nothing local shadows the leading name, so
                // user code always wins.
                if let Some(path) = self.module_path_of(obj)
                    && let Some(value) = self.module_member(&path, name)
                {
                    return value;
                }
                let obj = self.infer_expr(obj);
                let (strukt, index, ty) = self.field_of(&obj.ty, name);
                hir::Expr {
                    kind: hir::ExprKind::Field {
                        obj: Box::new(obj),
                        strukt,
                        index,
                        name: name.as_str().into(),
                    },
                    ty,
                    span,
                }
            }

            // An `@import` anywhere but a top-level `const` is a mistake with
            // a specific fix, so say what it is rather than "not a value".
            ast::Expr::Import { span, .. } => {
                self.error(*span, "`@import` is only allowed at the top level")
                    .help = Some(
                    "bind it once beside the other declarations, as in \
                     `const http = @import(\"std/http\");`"
                        .into(),
                );
                let ty = self.store.fresh();
                hir::Expr {
                    kind: hir::ExprKind::Null,
                    ty,
                    span: *span,
                }
            }

            ast::Expr::ArrayLit { elem, elems, .. } => {
                let elem_ty = self.resolve_type_expr(elem);
                let elems = elems
                    .iter()
                    .map(|e| {
                        let e = self.infer_expecting(e, &elem_ty);
                        self.coerce(e, &elem_ty, "this element")
                    })
                    .collect();
                hir::Expr {
                    kind: hir::ExprKind::ArrayNew { elems },
                    ty: Type::array(elem_ty),
                    span,
                }
            }

            ast::Expr::Index { obj, index, .. } => {
                let (arr, index, elem) = self.infer_index(obj, index);
                hir::Expr {
                    kind: hir::ExprKind::Index {
                        arr: Box::new(arr),
                        index: Box::new(index),
                    },
                    ty: elem,
                    span,
                }
            }

            ast::Expr::StructLit { path, fields, .. } => {
                self.infer_struct_lit(path, fields, span, None)
            }

            ast::Expr::Fn(func) => self.infer_closure(func, span),

            ast::Expr::If(if_expr) => {
                let (cond, capture) = self.infer_condition(&if_expr.cond, if_expr.capture.as_ref());
                self.frame().scopes.push(Vec::new());
                if let (Some(name), Some(local)) = (&if_expr.capture, capture) {
                    self.frame().bind_local(name.as_str(), local);
                }
                let then = self.infer_expr(&if_expr.then);
                self.frame().scopes.pop();
                let els = self.infer_expr(&if_expr.else_);
                let els = self.coerce(els, &then.ty, "this `else` branch");
                let ty = then.ty.clone();
                hir::Expr {
                    kind: hir::ExprKind::If {
                        cond: Box::new(cond),
                        capture,
                        then: Box::new(then),
                        els: Box::new(els),
                    },
                    ty,
                    span,
                }
            }

            ast::Expr::Try { expr: inner, .. } => {
                let inner = self.infer_expr(inner);
                let payload = self.store.fresh();
                let raised = self.store.fresh_err_set();
                self.expect(
                    &inner.ty,
                    &Type::err_union(payload.clone(), raised.clone()),
                    inner.span,
                    "the operand of `try`",
                );
                // `try` propagates, so the enclosing function must be fallible.
                let ret = self.frames.last().expect("in a function").ret.clone();
                let out = self.store.fresh();
                let propagated = self.store.fresh_err_set();
                // Subset rather than unification: propagating a callee's
                // errors only needs this function's set to *cover* them, and
                // two functions that raise different things must still be able
                // to call each other.
                self.constraints.push(Constraint::ErrorSubset {
                    sub: raised,
                    sup: propagated.clone(),
                    span,
                });
                if self
                    .store
                    .unify(&ret, &Type::err_union(out, propagated))
                    .is_err()
                {
                    let shown = self.store.show(&ret);
                    self.error(span, "`try` in a function that cannot fail")
                        .help = Some(format!(
                        "this function returns `{shown}`; change it to `!{shown}` to propagate errors"
                    ));
                }
                hir::Expr {
                    kind: hir::ExprKind::Try(Box::new(inner)),
                    ty: payload,
                    span,
                }
            }

            ast::Expr::Catch {
                expr: inner,
                capture,
                alt,
                ..
            } => {
                let inner = self.infer_expr(inner);
                let payload = self.store.fresh();
                let caught = self.store.fresh_err_set();
                self.expect(
                    &inner.ty,
                    &Type::err_union(payload.clone(), caught.clone()),
                    inner.span,
                    "the operand of `catch`",
                );
                self.frame().scopes.push(Vec::new());
                let error_ty = Type::error(caught);
                let capture_local = capture.as_ref().map(|name| {
                    let local =
                        self.frame()
                            .add_local(name.as_str(), error_ty, false, name.span);
                    self.frame().bind_local(name.as_str(), local);
                    local
                });
                let alt = self.infer_expr(alt);
                self.frame().scopes.pop();
                let alt = self.coerce(alt, &payload, "this `catch` value");
                hir::Expr {
                    kind: hir::ExprKind::Catch {
                        expr: Box::new(inner),
                        capture: capture_local,
                        alt: Box::new(alt),
                    },
                    ty: payload,
                    span,
                }
            }

            ast::Expr::Orelse {
                expr: inner, alt, ..
            } => {
                let inner = self.infer_expr(inner);
                let payload = self.store.fresh();
                self.expect(
                    &inner.ty,
                    &Type::optional(payload.clone()),
                    inner.span,
                    "the operand of `orelse`",
                );
                let alt = self.infer_expr(alt);
                let alt = self.coerce(alt, &payload, "this `orelse` value");
                hir::Expr {
                    kind: hir::ExprKind::Orelse {
                        expr: Box::new(inner),
                        alt: Box::new(alt),
                    },
                    ty: payload,
                    span,
                }
            }

            ast::Expr::Unwrap { expr: inner, .. } => {
                let inner = self.infer_expr(inner);
                let payload = self.store.fresh();
                self.expect(
                    &inner.ty,
                    &Type::optional(payload.clone()),
                    inner.span,
                    "the operand of `.?`",
                );
                hir::Expr {
                    kind: hir::ExprKind::Unwrap(Box::new(inner)),
                    ty: payload,
                    span,
                }
            }
        }
    }

    fn lit(&mut self, kind: hir::ExprKind, ty: Type, span: Span) -> hir::Expr {
        hir::Expr { kind, ty, span }
    }

    /// The module a path expression names, if its leading segment is an
    /// import that nothing local shadows.
    fn module_path_of(&self, expr: &ast::Expr) -> Option<String> {
        let mut segments = Vec::new();
        let mut cur = expr;
        loop {
            match cur {
                ast::Expr::Field { obj, name, .. } => {
                    segments.push(name.clone());
                    cur = obj;
                }
                ast::Expr::Ident(name) => {
                    segments.push(name.clone());
                    break;
                }
                _ => return None,
            }
        }
        segments.reverse();
        // A local of the same name shadows the import, so that binding a
        // variable called `http` cannot be broken by an import elsewhere.
        if self.lookup_binding_exists(segments[0].as_str()) {
            return None;
        }
        let mut path = self.imports[self.current]
            .get(segments[0].as_str())?
            .clone();
        for segment in &segments[1..] {
            path = format!("{path}/{segment}");
            if !self.module_paths.contains(&path) {
                return None;
            }
        }
        Some(path)
    }

    /// Whether a name is bound locally, without threading a capture through
    /// the enclosing closures the way looking it up would.
    fn lookup_binding_exists(&self, name: &str) -> bool {
        self.frames.iter().any(|f| f.find(name).is_some())
    }

    /// What a module member names, without turning it into a value.
    ///
    /// A call needs this rather than [`Inferencer::module_member`]: a builtin
    /// is not a value, and an overload set has to be chosen from with the
    /// arguments in hand.
    fn module_member_ref(&mut self, obj: &ast::Expr, name: &Ident) -> Option<GlobalRef> {
        let path = self.module_path_of(obj)?;
        if path == HTTP_MODULE {
            self.lookup_struct_in(HTTP_MODULE, name.as_str());
        }
        let key = format!("{path}.{name}");
        let found = self.globals.get(&key).cloned();
        if found.is_some() {
            self.check_visible(&key, name);
        }
        found
    }

    /// Report a name this module is not allowed to see, once per span.
    ///
    /// Reported rather than hidden: a private name that resolved to nothing
    /// would come back as "cannot find", which sends the reader looking for a
    /// spelling mistake instead of at the declaration that is right there.
    fn check_visible(&mut self, key: &str, name: &Ident) {
        // A module always sees its own, whatever it said. Belt and braces: a
        // qualified name cannot reach the module it is written in today, since
        // a module that imported itself would be a cycle -- but the protocol
        // lookup below resolves in a *type's* module, which may well be this
        // one.
        let here = format!("{}.", self.modules[self.current].path);
        if key.starts_with(&here) {
            return;
        }
        if !self.private.contains(key) || !self.reported_private.insert(name.span) {
            return;
        }
        // No secondary label pointing at the declaration, tempting as it is: a
        // `Span` carries no file, and the renderer lays a diagnostic's labels
        // out in the file its primary span falls in. The declaration is in
        // another one by construction.
        self.error(
            name.span,
            format!("`{name}` is private to the module that declares it"),
        )
        .help = Some(format!(
            "everything is private unless it says otherwise; write `pub` in front of `{name}` \
             to let other modules name it"
        ));
    }

    /// A value named through a module: `http.NotFound404`, `math.abs`.
    fn module_member(&mut self, path: &str, name: &Ident) -> Option<hir::Expr> {
        // A status type is materialised on first mention, which is what keeps
        // a program from paying for the 26 it never names.
        if path == HTTP_MODULE {
            self.lookup_struct_in(HTTP_MODULE, name.as_str());
        }
        let key = format!("{path}.{name}");
        let global = self.globals.get(&key).cloned()?;
        self.check_visible(&key, name);
        Some(self.global_as_value(global, name))
    }

    fn infer_ident(&mut self, name: &Ident) -> hir::Expr {
        let span = name.span;
        if let Some(local) = self.lookup_local(name.as_str()) {
            let ty = self.frames.last().expect("in a function").locals[local as usize]
                .ty
                .clone();
            return hir::Expr {
                kind: hir::ExprKind::Local(local),
                ty,
                span,
            };
        }
        if let Some(Binding::Definition { func, captures }) = self.lookup_binding(name.as_str()) {
            return self.definition_use(func, &captures, span);
        }
        // Functions come first, so that a `const` alias for an overload set
        // reads exactly as the name it aliases does.
        if let Some(ids) = self.overload_set(name.as_str()) {
            return self.func_value(name, &ids);
        }
        match self.global(name.as_str()).cloned() {
            Some(global) => self.global_as_value(global, name),
            None => {
                // A status type mentioned for the first time is materialised
                // here, which is what makes `handle(req, NotFound404)` work
                // without the program having declared anything.
                if let Some(id) = self.lookup_struct(name.as_str())
                    && self.structs[id as usize].fields.is_empty()
                {
                    return hir::Expr {
                        kind: hir::ExprKind::Singleton(id),
                        ty: Type::strukt(id),
                        span,
                    };
                }
                if !self.reject_abstract(name.as_str(), span) {
                    self.error(span, format!("cannot find `{name}` in this scope"));
                }
                let ty = self.store.fresh();
                hir::Expr {
                    kind: hir::ExprKind::Null,
                    ty,
                    span,
                }
            }
        }
    }

    /// What a resolved global means as a value.
    fn global_as_value(&mut self, global: GlobalRef, name: &Ident) -> hir::Expr {
        let span = name.span;
        match global {
            GlobalRef::Func(ids) => self.func_value(name, &ids),
            GlobalRef::FuncValue(index) => {
                let ids = self.fn_consts[index].ids.clone();
                self.func_value(name, &ids)
            }
            GlobalRef::Module => {
                self.error(span, format!("`{name}` is a module, not a value"))
                    .help = Some("name something inside it, as in `http.NotFound404`".into());
                let ty = self.store.fresh();
                hir::Expr {
                    kind: hir::ExprKind::Null,
                    ty,
                    span,
                }
            }
            GlobalRef::Singleton(id) => hir::Expr {
                kind: hir::ExprKind::Singleton(id),
                ty: Type::strukt(id),
                span,
            },
            GlobalRef::Const(index) => {
                let scheme = self.consts[index].scheme.clone();
                let (ty, _) = self.store.instantiate(&scheme);
                hir::Expr {
                    kind: self.consts[index].kind.clone(),
                    ty,
                    span,
                }
            }
            GlobalRef::Builtin(id) => {
                let owned = name.to_string();
                self.error(span, format!("builtin `{owned}` cannot be used as a value"))
                    .help = Some("call it directly, e.g. `print(x)`".into());
                let ty = self.builtin_type(id);
                hir::Expr {
                    kind: hir::ExprKind::Null,
                    ty,
                    span,
                }
            }
        }
    }

    /// The type at which to use function `id` here, plus the type arguments
    /// this use instantiates it at. Within a binding group the function is
    /// still monomorphic, which is exactly what makes recursion check.
    fn func_type(&mut self, id: hir::FuncId) -> (Type, Vec<Type>) {
        match &self.schemes[id as usize] {
            Some(scheme) => {
                let scheme = scheme.clone();
                self.store.instantiate(&scheme)
            }
            None => (self.fn_types[id as usize].clone(), Vec::new()),
        }
    }

    /// Register, on the types *this use* instantiates `id` at, the abstract-type
    /// constraints its parameters were annotated with.
    ///
    /// The constraint travels with the instantiation because the declaration is
    /// exactly where it cannot be checked: the parameter is generic there, and
    /// deciding it then would mean picking one member and losing the genericity
    /// the annotation exists to create. Only a use that keeps its instantiation
    /// may call this -- a speculative one that is rolled back would leave a
    /// constraint on a variable that no longer means anything.
    fn note_member_constraints(&mut self, id: hir::FuncId, targs: &[Type], span: Span) {
        let Some(scheme) = self.schemes[id as usize].clone() else {
            // Still inside its own binding group, so this use shares the very
            // variable the signature recorded a constraint on.
            return;
        };
        for (var, abstract_id, _) in self.fn_member_vars[id as usize].clone() {
            let Type::Var(v) = self.store.resolve(&var) else {
                // The body pinned it to a concrete type, which the constraint
                // recorded at the declaration has already checked.
                continue;
            };
            if let Some(pos) = scheme.vars.iter().position(|q| *q == v)
                && let Some(targ) = targs.get(pos)
            {
                self.constraints.push(Constraint::Member {
                    ty: targ.clone(),
                    id: abstract_id,
                    span,
                });
            }
        }
    }

    /// Instantiate function `id` for a use written as `name`, holding the
    /// instantiation to whatever that name was pinned to.
    ///
    /// A `const` annotated with a signature does not merely *select* a member:
    /// the annotation is the type of every use of it, so a member that is still
    /// generic -- one with an abstract parameter -- is used at the types the
    /// annotation names and at no others.
    fn func_use(&mut self, name: &str, id: hir::FuncId, span: Span) -> (Type, Vec<Type>) {
        let (ty, targs) = self.func_type(id);
        // A binding in scope shadows the global, pin and all.
        let pinned = match self.lookup_binding(name) {
            Some(_) => None,
            None => match self.global(name) {
                Some(GlobalRef::FuncValue(index)) => Some(*index),
                _ => None,
            },
        };
        if let Some(index) = pinned {
            // `select_overload` already proved these unify; this use only needs
            // the type arguments that doing it again produces.
            let want = self.fn_consts[index].ty.clone();
            let _ = self.store.try_unify(&ty, &want);
        }
        self.note_member_constraints(id, &targs, span);
        (ty, targs)
    }

    /// The functions a name refers to: a `const` alias for an overload set in
    /// scope, or a top-level name. `None` if it means anything else.
    ///
    /// An annotated alias is narrowed to the one member its annotation selects,
    /// so everything downstream sees a set of one and takes the ordinary path.
    fn overload_set(&mut self, name: &str) -> Option<Vec<hir::FuncId>> {
        match self.lookup_binding(name) {
            // A local or a definition shadows the global of the same name,
            // value or not.
            Some(Binding::Local(_) | Binding::Definition { .. }) => return None,
            Some(Binding::Overloads(ids)) => return Some(ids),
            None => {}
        }
        match self.global(name) {
            Some(GlobalRef::Func(ids)) => Some(ids.clone()),
            Some(GlobalRef::FuncValue(index)) => Some(self.resolve_func_const(*index)),
            _ => None,
        }
    }

    /// The member a `const`'s annotation selected, worked out on first use and
    /// remembered. Falls back to the whole set once the choice has failed, so
    /// the failure is reported once and later uses behave like a plain alias.
    fn resolve_func_const(&mut self, index: usize) -> Vec<hir::FuncId> {
        if let Some(chosen) = self.fn_consts[index].chosen {
            return vec![chosen];
        }
        if self.fn_consts[index].failed {
            return self.fn_consts[index].ids.clone();
        }
        let c = &self.fn_consts[index];
        let (ids, want, span, name) = (c.ids.clone(), c.ty.clone(), c.span, c.name.clone());
        match self.select_overload(&name, &ids, &want, span) {
            Some(chosen) => {
                self.fn_consts[index].chosen = Some(chosen);
                vec![chosen]
            }
            None => {
                self.fn_consts[index].failed = true;
                ids
            }
        }
    }

    /// The member of an overload set whose signature is `want`.
    ///
    /// "Whose signature is", not "which accepts": a function value is one code
    /// pointer, so the annotation has to name a signature some member *has*.
    /// Selecting by subtyping would let `fn(Sub) i64` answer to `fn(Base) i64`,
    /// and a call through the value would then hand a `Base` to a body compiled
    /// to read `Sub`'s fields.
    fn select_overload(
        &mut self,
        name: &str,
        ids: &[hir::FuncId],
        want: &Type,
        span: Span,
    ) -> Option<hir::FuncId> {
        let mut matches = Vec::new();
        for &id in ids {
            // Speculative: a trial must not pin anything for the next one, and
            // the winner is instantiated again by whoever uses it.
            let snapshot = self.store.snapshot();
            let (ty, _) = self.func_type(id);
            if self.store.try_unify(&ty, want) {
                matches.push(id);
            }
            self.store.rollback_to(snapshot);
        }
        if let [only] = matches[..] {
            return Some(only);
        }
        let shown = self.store.show(want);
        let listed: Vec<String> = ids
            .iter()
            .map(|&id| {
                let ty = match &self.schemes[id as usize] {
                    Some(s) => s.ty.clone(),
                    None => self.fn_types[id as usize].clone(),
                };
                self.store.show(&ty)
            })
            .collect();
        let listed = listed.join("`, `");
        if matches.is_empty() {
            self.error(span, format!("no overload of `{name}` has type `{shown}`"))
                .help = Some(format!(
                "the annotation must be one member's signature exactly; `{name}` has \
                 `{listed}`"
            ));
        } else {
            let n = matches.len();
            self.error(
                span,
                format!("`{shown}` matches {n} overloads of `{name}`, so it selects none"),
            )
            .help = Some(format!(
                "annotate the overloads' return types so they can be told apart; `{name}` has \
                 `{listed}`"
            ));
        }
        None
    }

    /// A name that means functions, used as a value.
    fn func_value(&mut self, name: &Ident, ids: &[hir::FuncId]) -> hir::Expr {
        let span = name.span;
        if let [id] = ids[..] {
            // A named function used as a value becomes a closure with an empty
            // environment, so calls through it look like any other.
            let (ty, targs) = self.func_use(name.as_str(), id, span);
            return hir::Expr {
                kind: hir::ExprKind::Closure {
                    func: id,
                    targs,
                    captures: Vec::new(),
                },
                ty,
                span,
            };
        }
        // A closure value is one code pointer; an overload set is not.
        let n = ids.len();
        let message = if self.is_alias(name.as_str(), ids) {
            format!("`{name}` is an alias for an overload set, so it is not a single value")
        } else {
            format!("`{name}` names {n} functions, so it is not a single value")
        };
        // Show a signature the set really has, so the suggestion can be copied.
        let example = match &self.schemes[ids[0] as usize] {
            Some(s) => s.ty.clone(),
            None => self.fn_types[ids[0] as usize].clone(),
        };
        let example = self.store.show(&example);
        self.error(span, message).help = Some(format!(
            "a set can only be called, not passed around -- annotate the binding with one \
             member's signature, as in `const one: {example} = {name};`"
        ));
        let ty = self.store.fresh();
        hir::Expr {
            kind: hir::ExprKind::Null,
            ty,
            span,
        }
    }

    /// Whether `name` reaches these functions under a name of its own, i.e. is
    /// a `const` bound to the set rather than the name they were declared with.
    fn is_alias(&self, name: &str, ids: &[hir::FuncId]) -> bool {
        ids.first()
            .is_some_and(|&id| self.fn_names[id as usize] != name)
    }

    /// The type of a builtin *at this use*.
    ///
    /// A generic builtin is instantiated here rather than generalised into a
    /// scheme: it has one machine implementation, so there is nothing for
    /// monomorphisation to specialise, and the variables only have to be fresh
    /// per use for two calls to `len` to be at different element types.
    fn builtin_type(&mut self, id: hir::BuiltinId) -> Type {
        let builtins = wsharp_runtime::builtins();
        let b = &builtins[id as usize];
        let mut vars = HashMap::new();
        let params = b
            .params
            .iter()
            .map(|t| Type::from_builtin_with(*t, &mut self.store, &mut vars))
            .collect();
        let ret = Type::from_builtin_with(b.ret, &mut self.store, &mut vars);
        Type::func(params, ret)
    }

    fn infer_binary(&mut self, op: BinOp, lhs: hir::Expr, rhs: hir::Expr, span: Span) -> hir::Expr {
        if op.is_logical() {
            self.expect(&lhs.ty, &Type::bool(), lhs.span, "the left operand");
            self.expect(&rhs.ty, &Type::bool(), rhs.span, "the right operand");
            return hir::Expr {
                kind: hir::ExprKind::Logical {
                    op,
                    lhs: Box::new(lhs),
                    rhs: Box::new(rhs),
                },
                ty: Type::bool(),
                span,
            };
        }

        // `e == error.X`. The literal is an error *union* everywhere else, and
        // a bare tag here; only the comparison says which of the two it is, so
        // it is coerced rather than unified.
        let (lhs, rhs) = match (
            self.store.resolve(&lhs.ty),
            self.store.resolve(&rhs.ty),
            op.is_comparison(),
        ) {
            (Type::Con(TyCon::Error, _), _, true) => {
                let want = lhs.ty.clone();
                let rhs = self.coerce(rhs, &want, "the right operand");
                (lhs, rhs)
            }
            (_, Type::Con(TyCon::Error, _), true) => {
                let want = rhs.ty.clone();
                let lhs = self.coerce(lhs, &want, "the left operand");
                (lhs, rhs)
            }
            _ => (lhs, rhs),
        };
        self.expect(&rhs.ty, &lhs.ty, rhs.span, "the right operand");
        let ty = if op.is_comparison() {
            if op.is_ordering() {
                self.constraints.push(Constraint::Numeric {
                    ty: lhs.ty.clone(),
                    span,
                    op: op.text(),
                    integers_only: false,
                });
            } else {
                self.constraints.push(Constraint::Equatable {
                    ty: lhs.ty.clone(),
                    span,
                });
            }
            Type::bool()
        } else {
            // `%=` comes through here too, as `x = x % e`, so it is covered.
            self.constraints.push(Constraint::Numeric {
                ty: lhs.ty.clone(),
                span,
                op: op.text(),
                integers_only: op == BinOp::Rem,
            });
            lhs.ty.clone()
        };

        hir::Expr {
            kind: hir::ExprKind::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            },
            ty,
            span,
        }
    }

    fn infer_call(
        &mut self,
        callee: &'a ast::Expr,
        args: &'a [ast::Expr],
        span: Span,
    ) -> hir::Expr {
        // A `const` alias for a set resolves exactly as the name it aliases.
        let named = match callee {
            ast::Expr::Ident(name) => self.overload_set(name.as_str()).map(|ids| (name, ids)),
            // `str.concat(a, b)`: a call through a module, which resolves to
            // the same kinds of callee an unqualified one does.
            ast::Expr::Field { obj, name, .. } => match self.module_member_ref(obj, name) {
                Some(GlobalRef::Func(ids)) => Some((name, ids)),
                Some(GlobalRef::FuncValue(index)) => Some((name, self.resolve_func_const(index))),
                _ => None,
            },
            _ => None,
        };

        // An overload set needs its arguments inferred before the callee can be
        // chosen at all, so it takes a separate path.
        if let Some((name, ids)) = &named
            && ids.len() > 1
        {
            let (name, ids) = (*name, ids.clone());
            return self.infer_dispatched_call(name, &ids, args, span);
        }

        // A call to a name that resolves to a known function or builtin is
        // lowered as a direct call; anything else goes through a closure value.
        let (callee_ty, hir_callee) = match named {
            Some((name, ids)) => {
                let id = ids[0];
                let (ty, targs) = self.func_use(name.as_str(), id, span);
                (ty, hir::Callee::Static { func: id, targs })
            }
            None => match callee {
                // `lookup_binding` rather than `lookup_local`: a definition
                // is not a local, but it still shadows the global whose name
                // it shares. It falls through to the indirect arm below, which
                // materialises it at the type this call needs.
                ast::Expr::Ident(name) if self.lookup_binding(name.as_str()).is_none() => {
                    match self.global(name.as_str()).cloned() {
                        Some(GlobalRef::Builtin(id)) => {
                            (self.builtin_type(id), hir::Callee::Builtin(id))
                        }
                        _ => {
                            let e = self.infer_expr(callee);
                            (e.ty.clone(), hir::Callee::Indirect(Box::new(e)))
                        }
                    }
                }
                ast::Expr::Field { obj, name, .. }
                    if matches!(
                        self.module_member_ref(obj, name),
                        Some(GlobalRef::Builtin(_))
                    ) =>
                {
                    let Some(GlobalRef::Builtin(id)) = self.module_member_ref(obj, name) else {
                        unreachable!("just matched")
                    };
                    (self.builtin_type(id), hir::Callee::Builtin(id))
                }
                other => {
                    let e = self.infer_expr(other);
                    (e.ty.clone(), hir::Callee::Indirect(Box::new(e)))
                }
            },
        };

        let mut hir_args: Vec<hir::Expr> = args.iter().map(|a| self.infer_expr(a)).collect();

        // Unify against the callee's shape so parameter types flow into the
        // arguments, then coerce each argument (which lets `1` pass where `?i64`
        // is wanted, and so on).
        let ret = self.store.fresh();
        let arg_tys: Vec<Type> = hir_args.iter().map(|_| self.store.fresh()).collect();
        let expected = Type::func(arg_tys.clone(), ret.clone());
        if !self.store.try_unify(&callee_ty, &expected) {
            let resolved = self.store.resolve_deep(&callee_ty);
            let shown = self.store.show(&resolved);
            match resolved.as_fn() {
                Some((params, _)) if params.len() != args.len() => {
                    let n = params.len();
                    self.error(
                        span,
                        format!(
                            "this function takes {n} argument{} but {} {} given",
                            if n == 1 { "" } else { "s" },
                            args.len(),
                            if args.len() == 1 { "was" } else { "were" }
                        ),
                    );
                }
                _ => {
                    self.error(
                        span,
                        format!("`{shown}` is not callable with these arguments"),
                    );
                }
            }
        } else {
            for (arg, want) in hir_args.iter_mut().zip(&arg_tys) {
                let taken = std::mem::replace(
                    arg,
                    hir::Expr {
                        kind: hir::ExprKind::Null,
                        ty: Type::void(),
                        span,
                    },
                );
                *arg = self.coerce(taken, want, "this argument");
            }
        }

        hir::Expr {
            kind: hir::ExprKind::Call {
                callee: hir_callee,
                args: hir_args,
            },
            ty: ret,
            span,
        }
    }

    /// Resolve a call to an overload set: Julia's rule, decided at compile time.
    ///
    /// The arguments are inferred first, because which overload is meant is a
    /// question about their types. A candidate survives if its parameter cones
    /// can overlap the arguments' at every position -- statically applicable is
    /// not enough, since an argument typed `Status` may arrive as a
    /// `NotFound404` and select a more specific overload than any that fits the
    /// static type.
    fn infer_dispatched_call(
        &mut self,
        name: &Ident,
        ids: &[hir::FuncId],
        args: &'a [ast::Expr],
        span: Span,
    ) -> hir::Expr {
        let hir_args = args.iter().map(|a| self.infer_expr(a)).collect();
        self.dispatched_call(name, ids, hir_args, span)
    }

    /// The same, over arguments that have already been inferred.
    ///
    /// Split out for the `for` desugaring, which builds its call to `iter` and
    /// `next` out of hidden locals rather than out of source: an iterable's
    /// module may well have more than one of each, and choosing between them is
    /// the dispatcher's job rather than a second, worse one written here.
    fn dispatched_call(
        &mut self,
        name: &Ident,
        ids: &[hir::FuncId],
        mut hir_args: Vec<hir::Expr>,
        span: Span,
    ) -> hir::Expr {
        let arity = hir_args.len();
        let arg_tys: Vec<Type> = hir_args.iter().map(|a| a.ty.clone()).collect();

        let mut cands: Vec<Candidate> = Vec::new();
        let mut wrong_arity = 0usize;
        for &id in ids {
            let snapshot = self.store.snapshot();
            let (ty, targs) = self.func_type(id);
            let resolved = self.store.resolve_deep(&ty);
            let Some((params, ret)) = resolved.as_fn() else {
                self.store.rollback_to(snapshot);
                continue;
            };
            if params.len() != arity {
                wrong_arity += 1;
                self.store.rollback_to(snapshot);
                continue;
            }
            // Keep a candidate when every parameter cone overlaps its
            // argument's, in either direction. Disjoint cones can never both
            // hold of one value, so such a candidate is impossible here.
            let params: Vec<Type> = params.to_vec();
            let ret = ret.clone();
            // Fall back to the instantiated type wherever nothing was written,
            // so the two lists always line up with the arguments.
            let written = self.fn_decl_params[id as usize].clone();
            let decl_params: Vec<Type> = params
                .iter()
                .enumerate()
                .map(|(i, p)| written.get(i).cloned().unwrap_or_else(|| p.clone()))
                .collect();
            let mut tests = Vec::with_capacity(params.len());
            let mut possible = true;
            for ((param, decl), arg) in params.iter().zip(&decl_params).zip(&arg_tys) {
                match self.overlap(arg, param, decl) {
                    Some(test) => tests.push(test),
                    None => {
                        possible = false;
                        break;
                    }
                }
            }
            self.store.rollback_to(snapshot);
            if possible {
                cands.push(Candidate {
                    id,
                    params,
                    decl_params,
                    ret,
                    targs,
                    tests,
                });
            }
        }

        if cands.is_empty() {
            let shown: Vec<String> = arg_tys
                .iter()
                .map(|t| {
                    let t = t.clone();
                    self.store.show(&t)
                })
                .collect();
            let list = shown.join(", ");
            let n = ids.len();
            let message = if wrong_arity == n {
                format!("no overload of `{name}` takes {arity} arguments")
            } else {
                format!("no overload of `{name}` accepts ({list})")
            };
            self.error(span, message).help = Some(format!(
                "`{name}` has {n} overloads; one of them must accept every argument"
            ));
            let ty = self.store.fresh();
            return hir::Expr {
                kind: hir::ExprKind::Null,
                ty,
                span,
            };
        }

        // Most specific first, which makes the runtime table a first-match win.
        cands.sort_by(|a, b| self.compare_specificity(a, b));
        if let Some((a, b)) = self.first_ambiguous_pair(&cands) {
            let (x, y) = (self.show_params(&cands[a]), self.show_params(&cands[b]));
            let spans = [
                self.fn_spans[cands[a].id as usize],
                self.fn_spans[cands[b].id as usize],
            ];
            let diag = self.error(
                span,
                format!("this call to `{name}` is ambiguous: `{x}` and `{y}` are equally specific"),
            );
            diag.help = Some(
                "one overload must be at least as specific as the other in every argument -- \
                 add one that is"
                    .into(),
            );
            for span in spans {
                diag.secondary.push(Label {
                    span,
                    message: "this overload".into(),
                });
            }
        }

        // Every case must produce the same type, because the call site has one.
        let ret = cands[0].ret.clone();
        for cand in &cands[1..] {
            let other = cand.ret.clone();
            if !self.store.try_unify(&ret, &other) && !self.store.is_sub_ty(&other, &ret) {
                let (a, b) = (self.store.show(&ret), self.store.show(&other));
                self.error(
                    span,
                    format!(
                        "overloads of `{name}` disagree about the return type: `{a}` and `{b}`"
                    ),
                )
                .help =
                    Some("a dispatched call has one type, so every overload must share it".into());
                break;
            }
        }

        // Pin only the arguments inference has not settled -- a literal, say.
        // A concrete argument needs nothing done to it: `overlap` already
        // proved every surviving case accepts it, and widening to a supertype
        // is representationally free. Coercing it to some particular case's
        // parameter type would be wrong anyway, because when overloads cross
        // (`(Sub, Base)` against `(Base, Sub)`) no single case is widest.
        let joins: Vec<Option<Type>> = (0..arg_tys.len())
            .map(|i| self.join_at(&cands, i))
            .collect();
        for (i, arg) in hir_args.iter_mut().enumerate() {
            if !self.store.has_unbound(&arg.ty) {
                continue;
            }
            let Some(want) = joins[i].clone() else {
                continue;
            };
            let taken = std::mem::replace(
                arg,
                hir::Expr {
                    kind: hir::ExprKind::Null,
                    ty: Type::void(),
                    span,
                },
            );
            *arg = self.coerce(taken, &want, "this argument");
        }

        // Bind each surviving case's abstract parameters to the types it will
        // actually be called with. The trial unifications were undone so that
        // one candidate could not pin an argument for the next, but a case with
        // an abstract parameter is generic, and every case in the table must
        // reach monomorphisation with concrete type arguments. Only abstract
        // positions: anywhere else, re-unifying could pin an argument to one
        // arbitrary case's parameter type.
        for cand in &cands {
            let positions = cand.decl_params.iter().zip(&cand.params).zip(&arg_tys);
            for ((decl, param), arg) in positions {
                if matches!(self.store.resolve(decl), Type::Con(TyCon::Abstract(_), _)) {
                    let _ = self.store.try_unify(param, arg);
                }
            }
        }

        // If the most specific case needs no runtime test then it applies to
        // every value the arguments can take, so it always wins and the call is
        // static -- costing exactly what an ordinary call does. That covers the
        // usual case of arguments whose static types are already leaves, even
        // though less specific overloads also "apply".
        let kind = if cands[0].tests.iter().all(Option::is_none) {
            hir::ExprKind::Call {
                callee: hir::Callee::Static {
                    func: cands[0].id,
                    targs: cands[0].targs.clone(),
                },
                args: std::mem::take(&mut hir_args),
            }
        } else {
            let cases = cands
                .iter()
                .map(|c| hir::DispatchCase {
                    params: c.tests.clone(),
                    func: c.id,
                    targs: c.targs.clone(),
                })
                .collect();
            hir::ExprKind::Call {
                callee: hir::Callee::Dynamic { cases },
                args: std::mem::take(&mut hir_args),
            }
        };

        hir::Expr {
            kind,
            ty: ret,
            span,
        }
    }

    /// How a runtime value of static type `arg` can satisfy parameter type
    /// `param`, declared as `decl`.
    ///
    /// `None` means it never can. `Some(None)` means it always does, so the
    /// dispatcher need not test this position. `Some(Some(id))` means it does
    /// exactly when the runtime value is an instance of `id` or a subtype.
    fn overlap(&mut self, arg: &Type, param: &Type, decl: &Type) -> Option<Option<StructId>> {
        // An abstract parameter is a constrained generic one: the constraint is
        // on what was *declared*, while what a call site binds is the variable
        // standing in for it. Membership is decided here and now, because a
        // scalar's type is always statically known -- there is no header to
        // read and so no runtime test to emit, which is why a dispatch case
        // still only ever carries struct ids.
        if let Type::Con(TyCon::Abstract(_), _) = self.store.resolve(decl) {
            if !self.store.is_sub_ty(arg, decl) {
                return None;
            }
            // Records the type argument this case would be called at.
            let _ = self.store.try_unify(arg, param);
            return Some(None);
        }
        // Anything the ordinary rules already accept needs no test.
        if self.store.is_sub_ty(arg, param) || self.store.try_unify(arg, param) {
            return Some(None);
        }
        match (self.store.resolve(arg), self.store.resolve(param)) {
            (Type::Con(TyCon::Struct(a), _), Type::Con(TyCon::Struct(p), _))
                if self.store.is_subtype(p, a) =>
            {
                // The parameter is a proper subtype of the argument's static
                // type, so this case applies to part of the cone: test for it.
                Some(Some(p))
            }
            _ => None,
        }
    }

    /// True if `a` is at least as specific as `b` in every argument position.
    ///
    /// On the declared types, not the instantiated ones: an abstract annotation
    /// becomes a variable in the latter, and a variable is ordered against
    /// nothing, so `fn(i64)` would not come out narrower than `fn(Number)`.
    fn at_least_as_specific(&mut self, a: &Candidate, b: &Candidate) -> bool {
        a.decl_params.len() == b.decl_params.len()
            && a.decl_params
                .iter()
                .zip(&b.decl_params)
                .all(|(x, y)| self.store.is_sub_ty(x, y))
    }

    fn compare_specificity(&mut self, a: &Candidate, b: &Candidate) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        match (
            self.at_least_as_specific(a, b),
            self.at_least_as_specific(b, a),
        ) {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            // Equal or incomparable: keep declaration order so the table is
            // deterministic. An incomparable pair whose cones overlap is
            // reported separately.
            _ => a.id.cmp(&b.id),
        }
    }

    /// The first pair of candidates that could both match one runtime value yet
    /// neither is more specific -- Julia's ambiguity, which must be a compile
    /// error rather than a coin flip.
    fn first_ambiguous_pair(&mut self, cands: &[Candidate]) -> Option<(usize, usize)> {
        for i in 0..cands.len() {
            for j in i + 1..cands.len() {
                if self.at_least_as_specific(&cands[i], &cands[j])
                    || self.at_least_as_specific(&cands[j], &cands[i])
                {
                    continue;
                }
                // Incomparable, so ambiguous only if some concrete tuple
                // satisfies both. In a tree lattice two cones overlap exactly
                // when one contains the other, and the overlap is the narrower
                // of the two at every position.
                let Some(meet) = self.meet(&cands[i], &cands[j]) else {
                    continue;
                };
                // Julia's escape hatch: a third overload more specific than
                // both, and applicable to the whole overlap, settles it. This
                // is why adding `pick(Sub, Sub)` fixes `pick(Sub, Base)` versus
                // `pick(Base, Sub)`.
                if !self.is_resolved_by_a_third(cands, i, j, &meet) {
                    return Some((i, j));
                }
            }
        }
        None
    }

    /// The parameter type at position `i` that every candidate's is a subtype
    /// of, if there is one. That is the most permissive thing an unresolved
    /// argument can be pinned to without ruling out a case.
    fn join_at(&mut self, cands: &[Candidate], i: usize) -> Option<Type> {
        let mut widest = cands.first()?.params.get(i)?.clone();
        for c in &cands[1..] {
            let other = c.params.get(i)?.clone();
            if self.store.is_sub_ty(&widest, &other) {
                widest = other;
            }
        }
        for c in cands {
            if !self.store.is_sub_ty(&c.params[i], &widest) {
                return None;
            }
        }
        Some(widest)
    }

    /// The narrowest argument tuple both candidates accept, or `None` if no
    /// value satisfies both.
    fn meet(&mut self, a: &Candidate, b: &Candidate) -> Option<Vec<Type>> {
        let mut meet = Vec::with_capacity(a.decl_params.len());
        for (x, y) in a.decl_params.iter().zip(&b.decl_params) {
            if self.store.is_sub_ty(x, y) {
                meet.push(x.clone());
            } else if self.store.is_sub_ty(y, x) {
                meet.push(y.clone());
            } else {
                return None;
            }
        }
        Some(meet)
    }

    fn is_resolved_by_a_third(
        &mut self,
        cands: &[Candidate],
        i: usize,
        j: usize,
        meet: &[Type],
    ) -> bool {
        for (k, c) in cands.iter().enumerate() {
            if k == i || k == j {
                continue;
            }
            if !self.at_least_as_specific(c, &cands[i]) || !self.at_least_as_specific(c, &cands[j])
            {
                continue;
            }
            if meet
                .iter()
                .zip(&c.decl_params)
                .all(|(m, p)| self.store.is_sub_ty(m, p))
            {
                return true;
            }
        }
        false
    }

    fn show_params(&mut self, cand: &Candidate) -> String {
        let shown: Vec<String> = cand
            .decl_params
            .iter()
            .map(|t| self.store.show(t))
            .collect();
        format!("({})", shown.join(", "))
    }

    fn infer_struct_lit(
        &mut self,
        path: &[Ident],
        inits: &'a [ast::FieldInit],
        span: Span,
        expected: Option<Type>,
    ) -> hir::Expr {
        let name = path.last().expect("a path has a last segment");
        let found = match self.split_path(path) {
            Some(module) => self.lookup_struct_in(&module, name.as_str()),
            None if path.len() == 1 => self.lookup_struct(name.as_str()),
            None => {
                self.report_unknown_module(path);
                for init in inits {
                    self.infer_expr(&init.value);
                }
                let ty = self.store.fresh();
                return hir::Expr {
                    kind: hir::ExprKind::Null,
                    ty,
                    span,
                };
            }
        };
        let Some(id) = found else {
            // A name that exists but is not a type is a different mistake from
            // a name that does not exist, and "unknown" would send the reader
            // hunting for a typo. `find` rather than `lookup_local`: this is
            // only a question, and must not thread a capture through closures.
            let is_something_else = self.has_global(name.as_str())
                || self.frames.iter().any(|f| f.find(name.as_str()).is_some());
            if is_something_else {
                self.error(name.span, format!("`{name}` is not a struct"))
                    .help = Some(
                    "a struct literal names a type declared as `const Name = struct { ... }`"
                        .into(),
                );
            } else if !self.reject_abstract(name.as_str(), name.span) {
                self.error(name.span, format!("unknown struct `{name}`"));
            }
            for init in inits {
                self.infer_expr(&init.value);
            }
            let ty = self.store.fresh();
            return hir::Expr {
                kind: hir::ExprKind::Null,
                ty,
                span,
            };
        };

        let decl = self.structs[id as usize].clone();
        // `Box{ .value = 1 }` rather than `Box[i64]{ .. }`: the arguments come
        // from the field values, which is the only place they could come from
        // without `Box[i64]{` colliding with indexing `Box` by `i64`.
        let args: Vec<Type> = decl.params.iter().map(|_| self.store.fresh()).collect();
        let ty = Type::Con(TyCon::Struct(id), args.clone());
        // Bind the arguments from the context when there is one, so each
        // field is checked against the type it will actually be stored at.
        // A failure here is not reported: the caller compares the whole types
        // afterwards and says so once, rather than once per field.
        if let Some(expected) = expected {
            let _ = self.store.try_unify(&ty, &expected);
        }
        let mut values: Vec<Option<hir::Expr>> = vec![None; decl.fields.len()];
        for init in inits {
            let Some(index) = decl.field_index(init.name.as_str()) else {
                self.error(
                    init.name.span,
                    format!("`{}` has no field `{}`", decl.name, init.name),
                );
                self.infer_expr(&init.value);
                continue;
            };
            if values[index as usize].is_some() {
                self.error(
                    init.name.span,
                    format!("field `{}` is set twice", init.name),
                );
            }
            let want = decl.fields[index as usize].ty.clone();
            let want = self.substitute_params(id, &args, &want);
            let value = self.infer_expecting(&init.value, &want);
            let value = self.coerce(value, &want, "this field");
            values[index as usize] = Some(value);
        }

        let missing: Vec<&str> = decl
            .fields
            .iter()
            .zip(&values)
            .filter(|(_, v)| v.is_none())
            .map(|(f, _)| f.name.as_str())
            .collect();
        if !missing.is_empty() {
            let plural = if missing.len() == 1 {
                "field"
            } else {
                "fields"
            };
            let list = missing.join("`, `");
            self.error(
                span,
                format!("missing {plural} `{list}` in `{}`", decl.name),
            )
            .help = Some(format!("`{}` requires a value for every field", decl.name));
        }

        // Fill any gaps so the HIR stays well-formed after an error.
        let fields = values
            .into_iter()
            .zip(&decl.fields)
            .map(|(v, f)| {
                v.unwrap_or(hir::Expr {
                    kind: hir::ExprKind::Null,
                    ty: f.ty.clone(),
                    span,
                })
            })
            .collect();

        hir::Expr {
            kind: hir::ExprKind::StructNew { strukt: id, fields },
            ty,
            span,
        }
    }

    /// Infer an expression that is already known to be checked against
    /// `expected`.
    ///
    /// This matters only for a struct literal, whose type arguments otherwise
    /// come from its field values alone: `Pair[?i64, i64]` would then reject
    /// `.first = 5`, because the field is checked against a variable nothing
    /// has bound yet rather than against `?i64`. Telling the literal first is
    /// what gives its fields the coercion every other annotated position gets.
    fn infer_expecting(&mut self, expr: &'a ast::Expr, expected: &Type) -> hir::Expr {
        match expr {
            ast::Expr::StructLit { path, fields, span } => {
                self.infer_struct_lit(path, fields, *span, Some(expected.clone()))
            }
            other => self.infer_expr(other),
        }
    }

    /// A `fn` literal in expression position. Its body is inferred in a fresh
    /// frame; names it uses from enclosing frames become captures, copied in by
    /// value.
    ///
    /// A literal written where a value is wanted *is* a value -- one code
    /// pointer, and so one type -- so it stays monomorphic. Only a `const`
    /// bound to one is a definition, and only a definition can generalise; see
    /// [`Inferencer::infer_let_of_fn_literal`].
    fn infer_closure(&mut self, func: &'a ast::Func, span: Span) -> hir::Expr {
        let (id, fn_ty, capture_sources) = self.infer_fn_literal(func, span);
        self.schemes[id as usize] = Some(Scheme::mono(fn_ty.clone()));
        self.closure_value(id, fn_ty, &capture_sources, span)
    }

    /// Declare and infer a `fn` literal, returning its id, the type its
    /// signature and body worked out, and the locals *in the enclosing frame*
    /// it captures.
    fn infer_fn_literal(
        &mut self,
        func: &'a ast::Func,
        span: Span,
    ) -> (hir::FuncId, Type, Vec<hir::LocalId>) {
        self.infer_fn_literal_named(func, span, None)
    }

    /// The same, with the name the literal knows itself by inside its own body.
    ///
    /// A literal bound to a `const` may name itself. The name is bound to the
    /// literal's *own closure value*, which the environment pointer already
    /// holds: a literal is only ever entered through a closure, and a call
    /// passes that closure as the environment. So the recursive reference
    /// needs no allocation, and needs nothing new from monomorphisation
    /// either -- the closure at the outer use site already points at the
    /// specialisation this body is, which is why polymorphic recursion is out
    /// of reach here for exactly the reason Hindley-Milner puts it out of
    /// reach everywhere else.
    ///
    /// The signature is worked out *before* the body, and `schemes[id]` stays
    /// `None` throughout it, so a self-use takes the monomorphic arm of
    /// [`Inferencer::func_type`] exactly as a recursive `fn` declaration's
    /// does.
    fn infer_fn_literal_named(
        &mut self,
        func: &'a ast::Func,
        span: Span,
        self_name: Option<&str>,
    ) -> (hir::FuncId, Type, Vec<hir::LocalId>) {
        let name = Ident::new(format!("closure@{}", span.start), span);
        let id = self.declare_function(&name, func, span, true);
        let fn_ty = self.signature_type(id, func);
        self.fn_types[id as usize] = fn_ty.clone();
        if let Some(self_name) = self_name {
            self.pending_self = Some((self_name.to_string(), fn_ty.clone()));
        }
        let capture_sources = self.infer_function(id);
        (id, fn_ty, capture_sources)
    }

    /// The closure a `fn` literal evaluates to: its captures read straight out
    /// of the enclosing frame, in the order the closure's own capture locals
    /// expect them.
    fn closure_value(
        &mut self,
        id: hir::FuncId,
        fn_ty: Type,
        capture_sources: &[hir::LocalId],
        span: Span,
    ) -> hir::Expr {
        let enclosing = self
            .frames
            .last()
            .expect("a `fn` literal is inside a function");
        let captures = capture_sources
            .iter()
            .map(|&source| hir::Expr {
                kind: hir::ExprKind::Local(source),
                ty: enclosing.locals[source as usize].ty.clone(),
                span,
            })
            .collect();

        hir::Expr {
            kind: hir::ExprKind::Closure {
                func: id,
                targs: Vec::new(),
                captures,
            },
            ty: fn_ty,
            span,
        }
    }

    /// A generic `fn` literal's name, used. Materialise a closure at the type
    /// this use needs, over the values the definition snapshotted.
    fn definition_use(
        &mut self,
        func: hir::FuncId,
        capture_names: &[String],
        span: Span,
    ) -> hir::Expr {
        let (ty, targs) = self.func_type(func);
        self.note_member_constraints(func, &targs, span);

        let mut captures = Vec::with_capacity(capture_names.len());
        for name in capture_names {
            let local = self
                .lookup_local(name)
                .expect("a definition's captures are in scope wherever its name is");
            let ty = self.frames.last().expect("in a function").locals[local as usize]
                .ty
                .clone();
            captures.push(hir::Expr {
                kind: hir::ExprKind::Local(local),
                ty,
                span,
            });
        }

        hir::Expr {
            kind: hir::ExprKind::Closure {
                func,
                targs,
                captures,
            },
            ty,
            span,
        }
    }

    /// Generalise a definition's type, leaving alone anything a constraint
    /// still has an opinion about.
    ///
    /// `solve_constraints` runs once per binding group, after that group's
    /// level closes, which is why `fn add(a, b) { return a + b; }` is
    /// `fn(i64, i64) i64` rather than generic: `Numeric` defaults it before
    /// anything is quantified. A `fn` literal generalised at its own binding
    /// closes its level *first*, so quantifying a variable the solver is about
    /// to pin would give the same body two different types depending on which
    /// of the two ways it was written. Such a variable stays where it is and is
    /// defaulted with the rest of the group.
    fn generalize_definition(&mut self, ty: &Type) -> Scheme {
        let mut scheme = self.store.generalize(ty);
        if scheme.vars.is_empty() {
            return scheme;
        }
        let mut owned = Vec::new();
        for constraint in &self.constraints {
            for ty in constraint.types() {
                self.store.collect_vars(ty, &mut owned);
            }
        }
        scheme.vars.retain(|v| !owned.contains(v));
        scheme
    }

    // -----------------------------------------------------------------------
    // Names and captures
    // -----------------------------------------------------------------------

    fn lookup_local(&mut self, name: &str) -> Option<hir::LocalId> {
        match self.lookup_binding(name)? {
            Binding::Local(id) => Some(id),
            Binding::Overloads(_) | Binding::Definition { .. } => None,
        }
    }

    fn lookup_binding(&mut self, name: &str) -> Option<Binding> {
        let depth = self.frames.len().checked_sub(1)?;
        self.lookup_at(depth, name)
    }

    /// Look `name` up in frame `depth`, threading a capture through every
    /// intervening closure if it is found further out.
    fn lookup_at(&mut self, depth: usize, name: &str) -> Option<Binding> {
        if let Some(binding) = self.frames[depth].find(name) {
            // A name that turned out to be this frame's own is what decides
            // whether the function needs its environment kept as a value.
            // Noted here rather than in `find`, because two callers use that
            // only to ask whether a name is shadowed.
            if let Binding::Local(id) = &binding
                && self.frames[depth].self_local == Some(*id)
            {
                self.frames[depth].self_used = true;
            }
            return Some(binding);
        }
        if let Some(&id) = self.frames[depth].captured_names.get(name) {
            return Some(Binding::Local(id));
        }
        let outer = depth.checked_sub(1)?;
        // An overload set and a generic `fn` literal are both decided at
        // compile time, so a closure that names one reaches straight past the
        // frame boundary: there is no value to copy in, and capturing it would
        // invent a local with no type. A definition's captured values are
        // reached through its hidden locals, each of which threads a capture of
        // its own when this runs on it.
        let source = match self.lookup_at(outer, name)? {
            Binding::Local(id) => id,
            found @ (Binding::Overloads(_) | Binding::Definition { .. }) => return Some(found),
        };
        let ty = self.frames[outer].locals[source as usize].ty.clone();
        let span = self.frames[outer].locals[source as usize].span;

        let frame = &mut self.frames[depth];
        let id = frame.add_local(name, ty, false, span);
        frame.captures.push(id);
        frame.capture_sources.push(source);
        frame.captured_names.insert(name.to_string(), id);
        Some(Binding::Local(id))
    }

    // -----------------------------------------------------------------------
    // Coercion, unification helpers
    // -----------------------------------------------------------------------

    /// Unify, reporting a readable mismatch on failure.
    fn expect(&mut self, actual: &Type, expected: &Type, span: Span, what: &str) {
        if let Err(err) = self.store.unify(actual, expected) {
            self.report_unify_error(err, actual, expected, span, what);
        }
    }

    /// The diagnostic for a failed unification. A plain mismatch is the usual
    /// case; the occurs check failing means the two types can only agree if
    /// one contains itself, and "mismatch" would misdescribe that.
    fn report_unify_error(
        &mut self,
        err: UnifyError,
        actual: &Type,
        expected: &Type,
        span: Span,
        what: &str,
    ) {
        let a = self.store.show(actual);
        let e = self.store.show(expected);
        match err {
            UnifyError::Occurs => {
                self.error(span, "this expression's type would be infinite")
                    .help = Some(format!(
                    "a type cannot contain itself, but {what} has type `{a}` where `{e}` is expected"
                ));
            }
            UnifyError::Mismatch => {
                self.error(
                    span,
                    format!("type mismatch: {what} has type `{a}`, expected `{e}`"),
                );
            }
        }
    }

    /// Fit `expr` to `target`, wrapping it into an optional or an error union
    /// when that is what the target asks for. This is what lets `return n;`
    /// work in a function declared `!i64`, and `return 1;` in one declared
    /// `?i64`.
    fn coerce(&mut self, expr: hir::Expr, target: &Type, what: &str) -> hir::Expr {
        let Err(direct) = self.store.try_unify_checked(&expr.ty, target) else {
            return expr;
        };
        // Widening to a supertype. Free at run time: a subtype's layout starts
        // with a copy of its supertype's, and every struct is one pointer slot,
        // so there is nothing to emit.
        if self.store.is_sub_ty(&expr.ty, target) {
            return expr;
        }
        let resolved = self.store.resolve(target);
        // `error.X` written where an `error` value is wanted -- next to a
        // `catch |e|` binding, in practice. The literal is one tag either way,
        // so only its type changes; what it must *not* do is widen the set it
        // is being compared against, which is why the constraint only checks.
        if let Type::Con(TyCon::Error, args) = &resolved
            && let hir::ExprKind::Err(id) = &expr.kind
        {
            self.constraints.push(Constraint::ErrorSetHas {
                set: args[0].clone(),
                errors: vec![*id],
                widens: false,
                span: expr.span,
            });
            return hir::Expr {
                kind: expr.kind,
                ty: resolved,
                span: expr.span,
            };
        }
        // A narrower error set where a wider one is wanted. Free at run time --
        // the tag is the same number -- and exactly the widening `try` does
        // through `ErrorSubset`, arriving here instead when the value is
        // returned or assigned rather than propagated.
        if let (Type::Con(TyCon::ErrUnion, want), Type::Con(TyCon::ErrUnion, have)) =
            (&resolved, &self.store.resolve(&expr.ty))
            && let (Some(want_set), Some(have_set)) = (
                self.store.as_err_set(&want[1]),
                self.store.as_err_set(&have[1]),
            )
            && have_set.iter().all(|e| want_set.contains(e))
            && self.store.try_unify(&have[0].clone(), &want[0].clone())
        {
            return expr;
        }
        let wrap = match &resolved {
            Type::Con(TyCon::Optional, args) => Some((args[0].clone(), true)),
            Type::Con(TyCon::ErrUnion, args) => Some((args[0].clone(), false)),
            _ => None,
        };
        if let Some((inner, is_optional)) = wrap
            && self.store.try_unify(&expr.ty, &inner)
        {
            let span = expr.span;
            let kind = if is_optional {
                hir::ExprKind::Some(Box::new(expr))
            } else {
                hir::ExprKind::Ok(Box::new(expr))
            };
            return hir::Expr {
                kind,
                ty: resolved,
                span,
            };
        }
        self.report_unify_error(direct, &expr.ty, target, expr.span, what);
        expr
    }

    /// Resolve `obj.field`. When the object's type is not yet known the work is
    /// deferred; the returned indices are [`UNRESOLVED`] and get patched later.
    fn field_of(&mut self, obj: &Type, field: &Ident) -> (StructId, u32, Type) {
        match self.store.resolve(obj) {
            Type::Con(TyCon::Struct(id), args) => {
                let decl = &self.structs[id as usize];
                match decl.field_index(field.as_str()) {
                    Some(index) => {
                        let ty = decl.fields[index as usize].ty.clone();
                        let ty = self.substitute_params(id, &args, &ty);
                        (id, index, ty)
                    }
                    None => {
                        let name = decl.name.clone();
                        self.error(field.span, format!("`{name}` has no field `{field}`"));
                        (id, UNRESOLVED, self.store.fresh())
                    }
                }
            }
            _ => {
                let result = self.store.fresh();
                self.constraints.push(Constraint::HasField {
                    obj: obj.clone(),
                    field: field.as_str().into(),
                    result: result.clone(),
                    span: field.span,
                });
                (UNRESOLVED, UNRESOLVED, result)
            }
        }
    }

    fn intern_string(&mut self, s: &str) -> hir::StrId {
        if let Some(&id) = self.string_ids.get(s) {
            return id;
        }
        let id = self.strings.len() as hir::StrId;
        self.strings.push(s.to_string());
        self.string_ids.insert(s.to_string(), id);
        id
    }

    /// The id of an error name.
    ///
    /// The table lives on the [`TypeStore`] rather than here, because an error
    /// set is part of a type now and a type has to be printable. Two tables
    /// would be two numberings, and a builtin's set is interned from the
    /// runtime's row rather than from source.
    fn intern_error(&mut self, name: &str) -> hir::ErrorId {
        self.store.intern_error(name)
    }

    // -----------------------------------------------------------------------
    // Constraint solving and fix-ups
    // -----------------------------------------------------------------------

    /// Work out every error set the group left open, before anything is checked.
    ///
    /// An inferred set is the union of what its function raises directly and
    /// what it propagates, and propagation chains: `a` may `try` `b`, which
    /// may `try` `c`. So the contributions are gathered, the `try` edges are
    /// followed to a fixed point, and each still-open set is bound to the union
    /// that reaches it. A set nothing contributes to is left alone rather than
    /// bound to the empty set -- it may still be a *parameter's*, and pinning
    /// it here would decide for a caller what its argument may raise.
    fn solve_error_sets(&mut self, constraints: &[Constraint]) {
        let mut union: HashMap<TypeVarId, BTreeSet<hir::ErrorId>> = HashMap::new();
        let mut edges: Vec<(TypeVarId, TypeVarId)> = Vec::new();

        for constraint in constraints {
            match constraint {
                Constraint::ErrorSetHas {
                    set, errors, widens, ..
                } => {
                    if !widens {
                        continue;
                    }
                    if let Type::Var(v) = self.store.resolve(set) {
                        union.entry(v).or_default().extend(errors.iter().copied());
                    }
                }
                Constraint::ErrorSubset { sub, sup, .. } => {
                    let Type::Var(v) = self.store.resolve(sup) else {
                        continue;
                    };
                    match self.store.resolve(sub) {
                        Type::Var(u) => edges.push((u, v)),
                        _ => {
                            if let Some(members) = self.store.as_err_set(sub) {
                                union.entry(v).or_default().extend(members);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Terminates: every round either adds an element to some set or stops,
        // and the elements come from a finite table.
        loop {
            let mut changed = false;
            for &(from, to) in &edges {
                let Some(src) = union.get(&from).cloned() else {
                    continue;
                };
                let dst = union.entry(to).or_default();
                let before = dst.len();
                dst.extend(src);
                changed |= dst.len() != before;
            }
            if !changed {
                break;
            }
        }

        for (var, errors) in union {
            if !matches!(self.store.resolve(&Type::Var(var)), Type::Var(_)) {
                continue;
            }
            let set = self.store.err_set(errors);
            let _ = self.store.unify(&Type::Var(var), &set);
        }
    }

    /// `{NotFound, IoFailed}` for a diagnostic.
    fn show_err_set(&mut self, errors: &[hir::ErrorId]) -> String {
        let names: Vec<String> = errors
            .iter()
            .map(|&e| self.store.error_name(e).to_string())
            .collect();
        format!("{{{}}}", names.join(", "))
    }

    fn solve_constraints(&mut self) {
        let constraints = std::mem::take(&mut self.constraints);
        // Which variables an abstract type already holds to a set of concrete
        // types. `Numeric` and `Equatable` on one of those are answered by the
        // set, and -- more importantly -- must not default it to `i64`: the
        // whole point of the annotation is that the function stays generic over
        // the members, and `x + x` in the body must not decide for the caller.
        let mut constrained: HashMap<TypeVarId, AbstractId> = HashMap::new();
        for constraint in &constraints {
            if let Constraint::Member { ty, id, .. } = constraint
                && let Type::Var(v) = self.store.resolve(ty)
            {
                constrained.insert(v, *id);
            }
        }

        self.solve_error_sets(&constraints);

        for constraint in constraints {
            match constraint {
                Constraint::ErrorSetHas {
                    set, errors, span, ..
                } => {
                    // A set still open here is one nothing pinned; it is closed
                    // to the empty set at monomorphisation, along with every
                    // instantiation of it. Only a decided set can be wrong.
                    let Some(members) = self.store.as_err_set(&set) else {
                        continue;
                    };
                    for error in errors {
                        if members.contains(&error) {
                            continue;
                        }
                        let name = self.store.error_name(error).to_string();
                        let shown = self.show_err_set(&members);
                        self.error(span, format!("`error.{name}` is not one of `{shown}`"))
                            .help = Some(
                            "the error set here is written down, so it says exactly what may \
                             be raised -- add this name to it, or raise one it already has"
                                .into(),
                        );
                    }
                }
                Constraint::ErrorSubset { sub, sup, span } => {
                    let (Some(sub), Some(sup)) =
                        (self.store.as_err_set(&sub), self.store.as_err_set(&sup))
                    else {
                        continue;
                    };
                    let missing: Vec<hir::ErrorId> =
                        sub.iter().copied().filter(|e| !sup.contains(e)).collect();
                    if missing.is_empty() {
                        continue;
                    }
                    let names = self.show_err_set(&missing);
                    let shown = self.show_err_set(&sup);
                    self.error(
                        span,
                        format!("`try` propagates `{names}`, which this function cannot raise"),
                    )
                    .help = Some(format!(
                        "this function's error set is `{shown}`; widen it, or catch what it \
                         does not cover"
                    ));
                }
                Constraint::Numeric {
                    ty,
                    span,
                    op,
                    integers_only,
                } => {
                    let resolved = self.store.resolve(&ty);
                    if let Type::Var(v) = resolved
                        && let Some(&abstract_id) = constrained.get(&v)
                    {
                        self.check_members_support(abstract_id, span, op, integers_only);
                        continue;
                    }
                    match resolved {
                        // Unconstrained by anything else: default to i64.
                        Type::Var(_) => {
                            let _ = self.store.unify(&ty, &Type::i64());
                        }
                        Type::Con(TyCon::F64, _) if integers_only => {
                            self.error(
                                span,
                                format!("`{op}` needs an integer, but this is `f64`"),
                            )
                            .help = Some(format!(
                                "`{op}` works on `i64`; use `f64` subtraction and truncation \
                                 for a float remainder"
                            ));
                        }
                        t if t.is_numeric() => {}
                        t => {
                            let shown = self.store.show(&t);
                            self.error(
                                span,
                                format!("`{op}` needs a number, but this is `{shown}`"),
                            )
                            .help = Some("arithmetic works on `i64` and `f64`".into());
                        }
                    }
                }
                Constraint::Equatable { ty, span } => {
                    let resolved = self.store.resolve(&ty);
                    if let Type::Var(v) = resolved
                        && let Some(&abstract_id) = constrained.get(&v)
                    {
                        self.check_members_equatable(abstract_id, span);
                        continue;
                    }
                    match resolved {
                        Type::Var(_) => {
                            let _ = self.store.unify(&ty, &Type::i64());
                        }
                        // `str` compares by contents rather than by address,
                        // which takes a call; code generation emits it, so
                        // nothing here has to know that it is not one
                        // instruction like the others.
                        // An error is its tag, so comparing two is comparing
                        // two integers -- which is what makes the `e` a
                        // `catch |e|` binds worth having now that its set says
                        // what it can be.
                        Type::Con(
                            TyCon::I64 | TyCon::F64 | TyCon::Bool | TyCon::Str | TyCon::Error,
                            _,
                        ) => {}
                        t => {
                            let shown = self.store.show(&t);
                            self.error(
                                span,
                                format!("`{shown}` values cannot be compared with `==`"),
                            )
                            .help = Some(
                                "only `i64`, `f64`, `bool`, `str` and `error` can be compared \
                                 so far"
                                    .into(),
                            );
                        }
                    }
                }
                Constraint::Indexable { obj, elem, span } => match self.store.resolve(&obj) {
                    Type::Con(TyCon::Array, args) => {
                        let _ = self.store.unify(&elem, &args[0]);
                    }
                    // Still a variable, so nothing said what was indexed.
                    // Unlike `Numeric` there is no sensible default: an
                    // element type guessed here would be wrong everywhere.
                    Type::Var(_) => {
                        self.error(span, "cannot tell what is being indexed").help =
                            Some("annotate the value being indexed, e.g. `fn f(a: []i64)`".into());
                    }
                    other => {
                        let shown = self.store.show(&other);
                        self.error(span, format!("`{shown}` cannot be indexed"))
                            .help = Some("only an array `[]T` can be indexed".into());
                    }
                },

                Constraint::HasField {
                    obj,
                    field,
                    result,
                    span,
                } => match self.store.resolve(&obj) {
                    Type::Con(TyCon::Struct(id), _) => {
                        let decl = &self.structs[id as usize];
                        match decl.field_index(&field) {
                            Some(index) => {
                                let ty = decl.fields[index as usize].ty.clone();
                                let _ = self.store.unify(&result, &ty);
                            }
                            None => {
                                let name = decl.name.clone();
                                self.error(span, format!("`{name}` has no field `{field}`"));
                            }
                        }
                    }
                    Type::Var(_) => {
                        self.error(
                            span,
                            format!("cannot tell which type the field `{field}` belongs to"),
                        )
                        .help = Some(
                            "W# resolves fields by the object's declared type -- annotate the \
                                 parameter, e.g. `fn f(p: Point)`"
                                .into(),
                        );
                    }
                    other => {
                        let shown = self.store.show(&other);
                        self.error(span, format!("`{shown}` has no fields"));
                    }
                },

                Constraint::Member { ty, id, span } => match self.store.resolve(&ty) {
                    // Still a variable, which here means a parameter of the
                    // scheme about to be generalised -- exactly the constrained
                    // generic the annotation asks for. Defaulting it the way
                    // `Numeric` defaults to `i64` would destroy the genericity
                    // the annotation exists to create, so it is left alone; a
                    // variable nothing ever pins is monomorphisation's to
                    // report, at the use site that failed to pin it.
                    Type::Var(_) => {}
                    resolved if abstract_members(id).contains(&resolved) => {}
                    resolved => {
                        let name = abstract_name(id);
                        let shown = self.store.show(&resolved);
                        let members = self.list_members(id);
                        self.error(
                            span,
                            format!("`{name}` accepts {members}, but this is `{shown}`"),
                        )
                        .help = Some(format!(
                            "`{name}` is the set of those types; use one of them here, or a \
                             parameter with no annotation to accept any type at all"
                        ));
                    }
                },
            }
        }
    }

    /// The members of an abstract type, as a phrase: "`i64` and `f64`".
    fn list_members(&mut self, id: AbstractId) -> String {
        let shown: Vec<String> = abstract_members(id)
            .iter()
            .map(|m| self.store.show(m))
            .map(|m| format!("`{m}`"))
            .collect();
        match shown.split_last() {
            Some((last, [])) => last.clone(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => "nothing".into(),
        }
    }

    /// An arithmetic operator applied to a value an abstract type constrains.
    ///
    /// The question is decidable without knowing which member it will be: the
    /// operator has to work for *every* one of them, because the caller picks.
    fn check_members_support(
        &mut self,
        id: AbstractId,
        span: Span,
        op: &'static str,
        integers_only: bool,
    ) {
        let name = abstract_name(id);
        for member in abstract_members(id) {
            let bad = if integers_only {
                member != Type::i64()
            } else {
                !member.is_numeric()
            };
            if bad {
                let shown = self.store.show(&member);
                let article = if integers_only {
                    "an integer"
                } else {
                    "a number"
                };
                self.error(
                    span,
                    format!("`{op}` needs {article}, but `{name}` includes `{shown}`"),
                )
                .help = Some(format!(
                    "a parameter annotated `{name}` must work for every type `{name}` lists"
                ));
                return;
            }
        }
    }

    fn check_members_equatable(&mut self, id: AbstractId, span: Span) {
        let name = abstract_name(id);
        for member in abstract_members(id) {
            if !matches!(member, Type::Con(TyCon::I64 | TyCon::F64 | TyCon::Bool, _)) {
                let shown = self.store.show(&member);
                self.error(
                    span,
                    format!(
                        "`{shown}` values cannot be compared with `==`, and `{name}` includes one"
                    ),
                )
                .help = Some("only `i64`, `f64` and `bool` can be compared so far".into());
                return;
            }
        }
    }

    /// Patch the struct/field indices that inference had to leave open.
    fn fixup_fields(&mut self) {
        let structs = self.structs.clone();
        for func in self.funcs.iter_mut().flatten() {
            fixup_block(&mut func.body, &structs, &mut self.store);
        }
    }

    /// Rules that only apply once a name has more than one function behind it.
    /// Run after inference, because both need the resolved signatures.
    fn check_overloads(&mut self) {
        let mut sets: Vec<(String, Vec<hir::FuncId>)> = self
            .globals
            .iter()
            .filter_map(|(name, g)| match g {
                // An alias for a set is not a set of its own: its members were
                // already checked under the name they were declared with, and
                // checking them twice would say everything twice.
                GlobalRef::Func(ids) if ids.len() > 1 && !self.is_alias(bare_name(name), ids) => {
                    Some((bare_name(name).to_string(), ids.clone()))
                }
                _ => None,
            })
            .collect();
        // `globals` is a HashMap, so sort for a deterministic diagnostic order.
        sets.sort_by(|a, b| a.0.cmp(&b.0));

        for (name, ids) in sets {
            // Dispatch picks an overload *by* its parameter types, so leaving
            // one to inference would make the choice depend on the call sites
            // it is trying to resolve.
            for &id in &ids {
                let Some(func) = self.fn_asts[id as usize] else {
                    continue;
                };
                for param in &func.params {
                    if param.ty.is_none() {
                        let pname = param.name.clone();
                        self.error(
                            param.name.span,
                            format!("`{pname}` needs a type: `{name}` is an overload set"),
                        )
                        .help = Some(
                            "every parameter of an overloaded function must be annotated".into(),
                        );
                    }
                }
            }

            // Two overloads that accept exactly the same arguments are a
            // duplicate definition, not a choice. On the declared types, so
            // that `Number` reads as `Number` and is told apart from `i64`.
            let rendered: Vec<String> = ids
                .iter()
                .map(|&id| {
                    let params = self.fn_decl_params[id as usize].clone();
                    let shown: Vec<String> = params.iter().map(|p| self.store.show(p)).collect();
                    shown.join(", ")
                })
                .collect();
            for i in 0..ids.len() {
                for j in i + 1..ids.len() {
                    if rendered[i] == rendered[j] {
                        let span = self.fn_spans[ids[j] as usize];
                        let params = &rendered[j];
                        self.error(
                            span,
                            format!("`{name}({params})` is declared more than once"),
                        )
                        .help = Some("overloads must differ in at least one parameter type".into());
                    }
                }
            }
        }
    }

    /// Force the choice for every `const` pinned to a signature that nothing
    /// used, so an annotation naming no member is reported whether or not the
    /// program went on to call it.
    fn check_function_consts(&mut self) {
        for index in 0..self.fn_consts.len() {
            self.resolve_func_const(index);
        }
    }

    fn check_entry(&mut self) {
        let root_main = self.key_in(0, "main");
        let Some(GlobalRef::Func(ids)) = self.globals.get(&root_main).cloned() else {
            return;
        };
        if ids.len() > 1 {
            let span = self.fn_spans[ids[1] as usize];
            self.error(span, "`main` cannot be overloaded").help =
                Some("a program has exactly one entry point".into());
            return;
        }
        let id = ids[0];
        let ty = match &self.schemes[id as usize] {
            Some(s) => s.ty.clone(),
            None => self.fn_types[id as usize].clone(),
        };
        let ok = match ty.as_fn() {
            Some((params, ret)) => {
                params.is_empty()
                    && matches!(
                        self.store.resolve(ret),
                        Type::Con(TyCon::I64 | TyCon::Void, _)
                    )
            }
            None => false,
        };
        if !ok {
            let shown = self.store.show(&ty);
            let span = self.fn_spans[id as usize];
            let diag = self.error(
                span,
                "`main` must take no arguments and return `i64` or `void`",
            );
            diag.primary.message = format!("this one has type `{shown}`");
        }
    }
}

/// Rebuild the expression that reads a place, for lowering `x += e`.
fn place_as_expr(place: &hir::Place, ty: &Type, span: Span) -> hir::Expr {
    let kind = match place {
        hir::Place::Local(id) => hir::ExprKind::Local(*id),
        hir::Place::Field {
            obj,
            strukt,
            index,
            name,
        } => hir::ExprKind::Field {
            obj: Box::new(obj.clone()),
            strukt: *strukt,
            index: *index,
            name: name.clone(),
        },
        hir::Place::Index { arr, index } => hir::ExprKind::Index {
            arr: Box::new(arr.clone()),
            index: Box::new(index.clone()),
        },
    };
    hir::Expr {
        kind,
        ty: ty.clone(),
        span,
    }
}

/// Whether an expression names a storage location rather than computing a value.
fn is_place_base(expr: &ast::Expr) -> bool {
    match expr {
        ast::Expr::Ident(_) => true,
        ast::Expr::Field { obj, .. } => is_place_base(obj),
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Control flow analysis
// ---------------------------------------------------------------------------

/// Whether control can reach the end of this block, i.e. whether it can fall
/// through instead of returning or jumping. Used both to require a `return` on
/// every path and to guarantee code generation can terminate every basic block.
fn block_terminates(block: &ast::Block) -> bool {
    stmts_terminate(&block.stmts)
}

fn stmts_terminate(stmts: &[ast::Stmt]) -> bool {
    stmts.iter().any(stmt_terminates)
}

/// The standard library's module of HTTP status types, which are materialised
/// from a table on first mention rather than declared.
pub use wsharp_runtime::builtins::HTTP_MODULE;

/// A stand-in type name for the "takes N type arguments" help line.
fn generic_placeholder(i: usize) -> String {
    const LETTERS: [char; 4] = ['T', 'U', 'V', 'W'];
    match i {
        0..=3 => LETTERS[i].to_string(),
        _ => format!("T{}", i - 3),
    }
}

/// A global's key without the module it was qualified with.
///
/// Keys are `"<module path>.<name>"`, and a name may not contain a `.`, so the
/// last one separates them however many the path itself has.
fn bare_name(key: &str) -> &str {
    key.rsplit('.').next().unwrap_or(key)
}

fn stmt_terminates(stmt: &ast::Stmt) -> bool {
    match stmt {
        ast::Stmt::Return { .. } | ast::Stmt::Break(_) | ast::Stmt::Continue(_) => true,
        ast::Stmt::Block(b) => block_terminates(b),
        ast::Stmt::If(s) => if_terminates(s),
        // A `while` may run zero times, so it never guarantees termination.
        _ => false,
    }
}

/// An `if` only terminates when both sides do -- an `if` with no `else` can
/// always fall through.
fn if_terminates(s: &ast::IfStmt) -> bool {
    let then = block_terminates(&s.then);
    let els = match s.else_.as_deref() {
        Some(ast::ElseBranch::Block(b)) => block_terminates(b),
        Some(ast::ElseBranch::If(inner)) => if_terminates(inner),
        None => false,
    };
    then && els
}

// ---------------------------------------------------------------------------
// Dependency collection
// ---------------------------------------------------------------------------

fn collect_deps_func(func: &ast::Func, out: &mut HashSet<String>) {
    collect_deps_block(&func.body, out);
}

fn collect_deps_block(block: &ast::Block, out: &mut HashSet<String>) {
    for stmt in &block.stmts {
        collect_deps_stmt(stmt, out);
    }
}

fn collect_deps_stmt(stmt: &ast::Stmt, out: &mut HashSet<String>) {
    match stmt {
        ast::Stmt::Let(s) => collect_deps_expr(&s.init, out),
        ast::Stmt::Assign(s) => {
            collect_deps_expr(&s.target, out);
            collect_deps_expr(&s.value, out);
        }
        ast::Stmt::Expr(e) => collect_deps_expr(e, out),
        ast::Stmt::Return { value: Some(e), .. } => collect_deps_expr(e, out),
        ast::Stmt::Return { .. } | ast::Stmt::Break(_) | ast::Stmt::Continue(_) => {}
        ast::Stmt::Block(b) => collect_deps_block(b, out),
        ast::Stmt::If(s) => collect_deps_if(s, out),
        ast::Stmt::While(s) => {
            collect_deps_expr(&s.cond, out);
            collect_deps_block(&s.body, out);
            if let Some(cont) = &s.cont {
                collect_deps_stmt(cont, out);
            }
        }
        ast::Stmt::For(s) => {
            collect_deps_expr(&s.iter, out);
            collect_deps_block(&s.body, out);
            // A `for` over anything but an array calls `iter` and `next`, and
            // which module's is not knowable before inference. Naming them here
            // is what puts them in dependency order; see `PROTOCOL_NAMES`.
            for name in PROTOCOL_NAMES {
                out.insert((*name).to_string());
            }
        }
    }
}

fn collect_deps_if(s: &ast::IfStmt, out: &mut HashSet<String>) {
    collect_deps_expr(&s.cond, out);
    collect_deps_block(&s.then, out);
    match s.else_.as_deref() {
        Some(ast::ElseBranch::Block(b)) => collect_deps_block(b, out),
        Some(ast::ElseBranch::If(inner)) => collect_deps_if(inner, out),
        None => {}
    }
}

fn collect_deps_expr(expr: &ast::Expr, out: &mut HashSet<String>) {
    match expr {
        ast::Expr::Block { stmts, value, .. } => {
            for stmt in stmts {
                collect_deps_stmt(stmt, out);
            }
            if let Some(v) = value {
                collect_deps_expr(v, out);
            }
        }
        ast::Expr::Ident(name) => {
            out.insert(name.to_string());
        }
        // A module name is not a dependency: it is resolved before any
        // binding group is formed.
        ast::Expr::Import { .. } => {}
        ast::Expr::ArrayLit { elems, .. } => {
            for e in elems {
                collect_deps_expr(e, out);
            }
        }
        ast::Expr::Index { obj, index, .. } => {
            collect_deps_expr(obj, out);
            collect_deps_expr(index, out);
        }
        ast::Expr::Int(..)
        | ast::Expr::Float(..)
        | ast::Expr::Bool(..)
        | ast::Expr::Str(..)
        | ast::Expr::Null(..)
        | ast::Expr::ErrorLit { .. } => {}
        ast::Expr::Unary { expr, .. }
        | ast::Expr::Try { expr, .. }
        | ast::Expr::Unwrap { expr, .. } => collect_deps_expr(expr, out),
        ast::Expr::Field { obj, name, .. } => {
            // `str.concat(..)` depends on `concat` in whatever `str` names.
            // Recorded dotted and resolved by the caller, which knows the
            // module this was written in; a field of a value records the base
            // name only, and a name that turns out to mean nothing is simply
            // not an edge.
            if let ast::Expr::Ident(base) = &**obj {
                out.insert(format!("{base}.{name}"));
            }
            collect_deps_expr(obj, out);
        }
        ast::Expr::Binary { lhs, rhs, .. } => {
            collect_deps_expr(lhs, out);
            collect_deps_expr(rhs, out);
        }
        ast::Expr::Orelse { expr, alt, .. } | ast::Expr::Catch { expr, alt, .. } => {
            collect_deps_expr(expr, out);
            collect_deps_expr(alt, out);
        }
        ast::Expr::Call { callee, args, .. } => {
            collect_deps_expr(callee, out);
            for arg in args {
                collect_deps_expr(arg, out);
            }
        }
        ast::Expr::StructLit { fields, .. } => {
            for f in fields {
                collect_deps_expr(&f.value, out);
            }
        }
        ast::Expr::Fn(func) => collect_deps_func(func, out),
        ast::Expr::If(e) => {
            collect_deps_expr(&e.cond, out);
            collect_deps_expr(&e.then, out);
            collect_deps_expr(&e.else_, out);
        }
    }
}

// ---------------------------------------------------------------------------
// Strongly connected components (Tarjan)
// ---------------------------------------------------------------------------

/// Components in dependency order: a component is emitted only after every
/// component it depends on, which is exactly the order inference wants.
fn tarjan_scc(n: usize, edges: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct State<'e> {
        edges: &'e [Vec<usize>],
        index: Vec<Option<u32>>,
        low: Vec<u32>,
        on_stack: Vec<bool>,
        stack: Vec<usize>,
        next: u32,
        out: Vec<Vec<usize>>,
    }

    fn strong_connect(s: &mut State, v: usize) {
        s.index[v] = Some(s.next);
        s.low[v] = s.next;
        s.next += 1;
        s.stack.push(v);
        s.on_stack[v] = true;

        for &w in &s.edges[v] {
            match s.index[w] {
                None => {
                    strong_connect(s, w);
                    s.low[v] = s.low[v].min(s.low[w]);
                }
                Some(idx) if s.on_stack[w] => {
                    s.low[v] = s.low[v].min(idx);
                }
                Some(_) => {}
            }
        }

        if s.low[v] == s.index[v].expect("v was indexed") {
            let mut group = Vec::new();
            loop {
                let w = s.stack.pop().expect("stack holds v");
                s.on_stack[w] = false;
                group.push(w);
                if w == v {
                    break;
                }
            }
            s.out.push(group);
        }
    }

    let mut state = State {
        edges,
        index: vec![None; n],
        low: vec![0; n],
        on_stack: vec![false; n],
        stack: Vec::new(),
        next: 0,
        out: Vec::new(),
    };
    for v in 0..n {
        if state.index[v].is_none() {
            strong_connect(&mut state, v);
        }
    }
    state.out
}

// ---------------------------------------------------------------------------
// Field index fix-up
// ---------------------------------------------------------------------------

/// Fill in the type arguments of calls that were made before their callee was
/// generalised. See [`Inferencer::record_in_group_targs`].
///
/// Only an *empty* list is filled: a call recorded outside the group already
/// carries the arguments its instantiation made.
fn patch_targs_block(block: &mut hir::Block, targs_for: &HashMap<hir::FuncId, Vec<Type>>) {
    for stmt in &mut block.stmts {
        patch_targs_stmt(stmt, targs_for);
    }
}

fn patch_targs_stmt(stmt: &mut hir::Stmt, targs_for: &HashMap<hir::FuncId, Vec<Type>>) {
    match stmt {
        hir::Stmt::Let { init, .. } => patch_targs_expr(init, targs_for),
        hir::Stmt::Assign { place, value } => {
            match place {
                hir::Place::Field { obj, .. } => patch_targs_expr(obj, targs_for),
                hir::Place::Index { arr, index } => {
                    patch_targs_expr(arr, targs_for);
                    patch_targs_expr(index, targs_for);
                }
                hir::Place::Local(_) => {}
            }
            patch_targs_expr(value, targs_for);
        }
        hir::Stmt::Expr(e) | hir::Stmt::Return(Some(e)) => patch_targs_expr(e, targs_for),
        hir::Stmt::Return(None) | hir::Stmt::Break | hir::Stmt::Continue => {}
        hir::Stmt::If {
            cond, then, els, ..
        } => {
            patch_targs_expr(cond, targs_for);
            patch_targs_block(then, targs_for);
            if let Some(els) = els {
                patch_targs_block(els, targs_for);
            }
        }
        hir::Stmt::While {
            cond, cont, body, ..
        } => {
            patch_targs_expr(cond, targs_for);
            if let Some(cont) = cont {
                patch_targs_stmt(cont, targs_for);
            }
            patch_targs_block(body, targs_for);
        }
        hir::Stmt::Block(b) => patch_targs_block(b, targs_for),
    }
}

fn patch_targs_expr(expr: &mut hir::Expr, targs_for: &HashMap<hir::FuncId, Vec<Type>>) {
    let fill = |func: &hir::FuncId, targs: &mut Vec<Type>| {
        if targs.is_empty()
            && let Some(vars) = targs_for.get(func)
        {
            *targs = vars.clone();
        }
    };
    match &mut expr.kind {
        hir::ExprKind::Block { stmts, value } => {
            for stmt in stmts {
                patch_targs_stmt(stmt, targs_for);
            }
            if let Some(v) = value {
                patch_targs_expr(v, targs_for);
            }
        }
        hir::ExprKind::Call { callee, args } => {
            match callee {
                hir::Callee::Static { func, targs } => fill(func, targs),
                hir::Callee::Dynamic { cases } => {
                    for case in cases {
                        fill(&case.func, &mut case.targs);
                    }
                }
                hir::Callee::Indirect(e) => patch_targs_expr(e, targs_for),
                hir::Callee::Builtin(_) => {}
            }
            for arg in args {
                patch_targs_expr(arg, targs_for);
            }
        }
        hir::ExprKind::Closure {
            func,
            targs,
            captures,
        } => {
            fill(func, targs);
            for c in captures {
                patch_targs_expr(c, targs_for);
            }
        }
        hir::ExprKind::Unary { expr, .. }
        | hir::ExprKind::Some(expr)
        | hir::ExprKind::Ok(expr)
        | hir::ExprKind::Try(expr)
        | hir::ExprKind::Unwrap(expr)
        | hir::ExprKind::Field { obj: expr, .. } => patch_targs_expr(expr, targs_for),
        hir::ExprKind::Binary { lhs, rhs, .. } | hir::ExprKind::Logical { lhs, rhs, .. } => {
            patch_targs_expr(lhs, targs_for);
            patch_targs_expr(rhs, targs_for);
        }
        hir::ExprKind::Orelse { expr, alt } | hir::ExprKind::Catch { expr, alt, .. } => {
            patch_targs_expr(expr, targs_for);
            patch_targs_expr(alt, targs_for);
        }
        hir::ExprKind::StructNew { fields, .. } => {
            for f in fields {
                patch_targs_expr(f, targs_for);
            }
        }
        hir::ExprKind::ArrayNew { elems } => {
            for e in elems {
                patch_targs_expr(e, targs_for);
            }
        }
        hir::ExprKind::Index { arr, index } => {
            patch_targs_expr(arr, targs_for);
            patch_targs_expr(index, targs_for);
        }
        hir::ExprKind::ArrayLen { arr } => patch_targs_expr(arr, targs_for),
        hir::ExprKind::If {
            cond, then, els, ..
        } => {
            patch_targs_expr(cond, targs_for);
            patch_targs_expr(then, targs_for);
            patch_targs_expr(els, targs_for);
        }
        hir::ExprKind::Int(_)
        | hir::ExprKind::Float(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::Str(_)
        | hir::ExprKind::Null
        | hir::ExprKind::Local(_)
        | hir::ExprKind::Singleton(_)
        | hir::ExprKind::Err(_) => {}
    }
}

fn fixup_block(block: &mut hir::Block, structs: &[hir::StructDef], store: &mut TypeStore) {
    for stmt in &mut block.stmts {
        fixup_stmt(stmt, structs, store);
    }
}

fn fixup_stmt(stmt: &mut hir::Stmt, structs: &[hir::StructDef], store: &mut TypeStore) {
    match stmt {
        hir::Stmt::Let { init, .. } => fixup_expr(init, structs, store),
        hir::Stmt::Assign { place, value } => {
            match place {
                hir::Place::Field {
                    obj,
                    strukt,
                    index,
                    name,
                } => {
                    fixup_expr(obj, structs, store);
                    resolve_field(&obj.ty, name, structs, store, strukt, index);
                }
                hir::Place::Index { arr, index } => {
                    fixup_expr(arr, structs, store);
                    fixup_expr(index, structs, store);
                }
                hir::Place::Local(_) => {}
            }
            fixup_expr(value, structs, store);
        }
        hir::Stmt::Expr(e) => fixup_expr(e, structs, store),
        hir::Stmt::Return(Some(e)) => fixup_expr(e, structs, store),
        hir::Stmt::Return(None) | hir::Stmt::Break | hir::Stmt::Continue => {}
        hir::Stmt::If {
            cond, then, els, ..
        } => {
            fixup_expr(cond, structs, store);
            fixup_block(then, structs, store);
            if let Some(els) = els {
                fixup_block(els, structs, store);
            }
        }
        hir::Stmt::While {
            cond, cont, body, ..
        } => {
            fixup_expr(cond, structs, store);
            fixup_block(body, structs, store);
            if let Some(cont) = cont {
                fixup_stmt(cont, structs, store);
            }
        }
        hir::Stmt::Block(b) => fixup_block(b, structs, store),
    }
}

fn fixup_expr(expr: &mut hir::Expr, structs: &[hir::StructDef], store: &mut TypeStore) {
    expr.ty = store.resolve_deep(&expr.ty);
    match &mut expr.kind {
        hir::ExprKind::Block { stmts, value } => {
            for stmt in stmts {
                fixup_stmt(stmt, structs, store);
            }
            if let Some(v) = value {
                fixup_expr(v, structs, store);
            }
        }
        hir::ExprKind::Field {
            obj,
            strukt,
            index,
            name,
        } => {
            fixup_expr(obj, structs, store);
            resolve_field(&obj.ty, name, structs, store, strukt, index);
        }
        hir::ExprKind::Unary { expr, .. }
        | hir::ExprKind::Some(expr)
        | hir::ExprKind::Ok(expr)
        | hir::ExprKind::Try(expr)
        | hir::ExprKind::Unwrap(expr) => fixup_expr(expr, structs, store),
        hir::ExprKind::Binary { lhs, rhs, .. } | hir::ExprKind::Logical { lhs, rhs, .. } => {
            fixup_expr(lhs, structs, store);
            fixup_expr(rhs, structs, store);
        }
        hir::ExprKind::Orelse { expr, alt } | hir::ExprKind::Catch { expr, alt, .. } => {
            fixup_expr(expr, structs, store);
            fixup_expr(alt, structs, store);
        }
        hir::ExprKind::Call { callee, args } => {
            if let hir::Callee::Indirect(e) = callee {
                fixup_expr(e, structs, store);
            }
            if let hir::Callee::Static { targs, .. } = callee {
                for t in targs.iter_mut() {
                    *t = store.resolve_deep(t);
                }
            }
            if let hir::Callee::Dynamic { cases } = callee {
                for t in cases.iter_mut().flat_map(|c| c.targs.iter_mut()) {
                    *t = store.resolve_deep(t);
                }
            }
            for arg in args {
                fixup_expr(arg, structs, store);
            }
        }
        hir::ExprKind::StructNew { fields, .. } => {
            for f in fields {
                fixup_expr(f, structs, store);
            }
        }
        hir::ExprKind::ArrayNew { elems } => {
            for e in elems {
                fixup_expr(e, structs, store);
            }
        }
        hir::ExprKind::Index { arr, index } => {
            fixup_expr(arr, structs, store);
            fixup_expr(index, structs, store);
        }
        hir::ExprKind::ArrayLen { arr } => fixup_expr(arr, structs, store),
        hir::ExprKind::If {
            cond, then, els, ..
        } => {
            fixup_expr(cond, structs, store);
            fixup_expr(then, structs, store);
            fixup_expr(els, structs, store);
        }
        hir::ExprKind::Closure {
            targs, captures, ..
        } => {
            for t in targs.iter_mut() {
                *t = store.resolve_deep(t);
            }
            for c in captures {
                fixup_expr(c, structs, store);
            }
        }
        hir::ExprKind::Int(_)
        | hir::ExprKind::Float(_)
        | hir::ExprKind::Bool(_)
        | hir::ExprKind::Str(_)
        | hir::ExprKind::Null
        | hir::ExprKind::Local(_)
        | hir::ExprKind::Singleton(_)
        | hir::ExprKind::Err(_) => {}
    }
}

fn resolve_field(
    obj_ty: &Type,
    name: &str,
    structs: &[hir::StructDef],
    store: &mut TypeStore,
    strukt: &mut StructId,
    index: &mut u32,
) {
    if *index != UNRESOLVED {
        return;
    }
    if let Type::Con(TyCon::Struct(id), _) = store.resolve(obj_ty)
        && let Some(i) = structs[id as usize].field_index(name)
    {
        *strukt = id;
        *index = i;
    }
}
