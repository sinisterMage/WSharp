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

use std::collections::{HashMap, HashSet};

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

pub fn analyze(module: &ast::Module) -> Analysis {
    let mut inf = Inferencer::new(module);
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
    /// `obj` must be a struct with field `field`, whose type is `result`.
    HasField {
        obj: Type,
        field: Box<str>,
        result: Type,
        span: Span,
    },
}

// Subtyping deliberately has no `Constraint` of its own, despite being the
// obvious candidate for one. A constraint is for a question that cannot be
// answered where it is met -- but `try_unify` binds whichever side is still a
// variable, so by the time anything asks "is this a subtype?" both sides are
// already concrete and the lattice answers immediately. See `coerce`.

/// What a name bound inside a function body means.
///
/// An overload set bound by `const g = f;` is not a value -- there is no single
/// code pointer for a set -- so it is a name that resolves to the same
/// functions `f` does rather than a local holding something.
#[derive(Debug, Clone)]
enum Binding {
    Local(hir::LocalId),
    Overloads(Vec<hir::FuncId>),
}

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
    module: &'a ast::Module,
    store: TypeStore,
    diags: Vec<Diagnostic>,

    structs: Vec<hir::StructDef>,
    struct_ids: HashMap<String, StructId>,
    /// Where each struct's name was written, so a redeclaration can point
    /// back at the first one. Builtin status types have no entry.
    struct_spans: HashMap<String, Span>,

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
    /// Filled in when the function's group is generalised.
    schemes: Vec<Option<Scheme>>,
    funcs: Vec<Option<hir::FuncDef>>,

    strings: Vec<String>,
    string_ids: HashMap<String, hir::StrId>,
    errors: Vec<String>,
    error_ids: HashMap<String, hir::ErrorId>,

    frames: Vec<Frame>,
    constraints: Vec<Constraint>,
    loop_depth: usize,
}

impl<'a> Inferencer<'a> {
    fn new(module: &'a ast::Module) -> Inferencer<'a> {
        Inferencer {
            module,
            store: TypeStore::new(),
            diags: Vec::new(),
            structs: Vec::new(),
            struct_ids: HashMap::new(),
            struct_spans: HashMap::new(),
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
            schemes: Vec::new(),
            funcs: Vec::new(),
            strings: Vec::new(),
            string_ids: HashMap::new(),
            errors: Vec::new(),
            error_ids: HashMap::new(),
            frames: Vec::new(),
            constraints: Vec::new(),
            loop_depth: 0,
        }
    }

    fn error(&mut self, span: Span, message: impl Into<String>) -> &mut Diagnostic {
        self.diags.push(Diagnostic::error(span, message));
        self.diags.last_mut().expect("just pushed")
    }

    fn finish(mut self) -> Analysis {
        let mut signatures = Vec::new();
        for id in 0..self.fn_names.len() {
            if self.fn_is_closure[id] {
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
        let mut entry = self.globals.get("main").and_then(|g| match g {
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
                errors: self.errors,
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
        for item in &self.module.items {
            let ast::Item::Struct(decl) = item else {
                continue;
            };
            if let Some(&first) = self.struct_spans.get(decl.name.as_str()) {
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
            self.struct_ids.insert(decl.name.to_string(), id);
            self.struct_spans
                .insert(decl.name.to_string(), decl.name.span);
            self.structs.push(hir::StructDef {
                name: decl.name.to_string(),
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

        self.resolve_struct_parents();

        // Lay fields out parents-first, so a subtype can start from a copy of
        // its supertype's layout. Declaration order will not do: a subtype may
        // be written above its supertype.
        for id in self.layout_order() {
            let Some(decl) = self.struct_decl_ast(id) else {
                continue;
            };

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
                let width = crate::layout::size_of(&mut self.store, &ty);
                fields.push(hir::FieldDef {
                    name: field.name.to_string(),
                    ty,
                    offset,
                    span: field.span,
                });
                offset += width;
            }
            let strukt = &mut self.structs[id as usize];
            strukt.fields = fields;
            strukt.inherited = inherited;
            strukt.field_end = offset;
            strukt.size = align_up(offset);
        }
    }

    /// The AST declaration a `StructId` came from.
    fn struct_decl_ast(&self, id: StructId) -> Option<&'a ast::StructDecl> {
        let name = self.structs[id as usize].name.as_str();
        self.module.items.iter().find_map(|item| match item {
            ast::Item::Struct(decl) if decl.name.as_str() == name => Some(decl),
            _ => None,
        })
    }

    /// Resolve each `struct : Parent` link, rejecting unknown parents and
    /// cycles. A cycle is broken as well as reported, because everything
    /// downstream walks the parent chain and would not terminate on one.
    fn resolve_struct_parents(&mut self) {
        for item in &self.module.items {
            let ast::Item::Struct(decl) = item else {
                continue;
            };
            let (Some(parent_name), Some(&id)) = (
                decl.parent.as_ref(),
                self.struct_ids.get(decl.name.as_str()),
            ) else {
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

        // Break cycles before anything walks a parent chain.
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
        if let Some(&id) = self.struct_ids.get(name) {
            return Some(id);
        }
        let table = wsharp_runtime::builtins::status_types();
        let &(sname, parent) = table.iter().find(|(n, _)| *n == name)?;

        let id = self.store.declare_struct(sname);
        self.struct_ids.insert(sname.to_string(), id);
        self.structs.push(hir::StructDef {
            name: sname.to_string(),
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
            .insert(sname.to_string(), GlobalRef::Singleton(id));

        if let Some(parent) = parent {
            // Recurses at most as deep as the lattice, and the table is
            // ordered parents-first, so this terminates.
            let parent = self
                .lookup_struct(parent)
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
        for (i, builtin) in wsharp_runtime::builtins().iter().enumerate() {
            self.globals.insert(
                builtin.name.to_string(),
                GlobalRef::Builtin(i as hir::BuiltinId),
            );
        }

        // A struct with no fields is also a value: its single instance, named
        // by the type. That is what makes `handle(req, NotFound404)` read the
        // way the lattice is written. Structs with fields are constructed as
        // usual and get no singleton.
        for id in 0..self.structs.len() as StructId {
            if self.structs[id as usize].fields.is_empty() {
                let name = self.structs[id as usize].name.clone();
                if let Some(&span) = self.struct_spans.get(&name) {
                    self.global_spans.insert(name.clone(), span);
                }
                self.globals.insert(name, GlobalRef::Singleton(id));
            }
        }

        // A `const` bound to a bare name may be naming a function declared
        // further down, so those wait for the whole file to be declared.
        let mut aliases: Vec<&'a ast::ConstDecl> = Vec::new();
        for item in &self.module.items {
            match item {
                ast::Item::Struct(_) => {}
                ast::Item::Fn(decl) => {
                    self.declare_function(&decl.name, &decl.func, decl.span, false);
                }
                ast::Item::Const(decl) => {
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
        let Some(GlobalRef::Func(ids)) = self.globals.get(source.as_str()).cloned() else {
            self.declare_const(decl);
            return;
        };
        if self.globals.contains_key(decl.name.as_str()) {
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
        self.globals.insert(decl.name.to_string(), global);
        self.global_spans
            .insert(decl.name.to_string(), decl.name.span);
    }

    fn declare_function(
        &mut self,
        name: &Ident,
        func: &'a ast::Func,
        span: Span,
        is_closure: bool,
    ) -> hir::FuncId {
        let id = self.fn_names.len() as hir::FuncId;
        if !is_closure {
            // A second `fn` of the same name extends an overload set rather
            // than colliding with it. Colliding with anything *else* -- a
            // `const`, a builtin, a zero-field struct's singleton -- is still
            // an error, because dispatch only ranges over functions.
            match self.globals.get(name.as_str()) {
                Some(GlobalRef::Func(_)) => {
                    let Some(GlobalRef::Func(ids)) = self.globals.get_mut(name.as_str()) else {
                        unreachable!("just matched")
                    };
                    ids.push(id);
                }
                Some(_) => self.report_redeclaration(name),
                None => {
                    self.globals
                        .insert(name.to_string(), GlobalRef::Func(vec![id]));
                    self.global_spans.insert(name.to_string(), name.span);
                }
            }
        }
        self.fn_names.push(name.to_string());
        self.fn_asts.push(Some(func));
        self.fn_spans.push(span);
        self.fn_is_closure.push(is_closure);
        self.fn_types.push(Type::void());
        self.fn_decl_params.push(Vec::new());
        self.fn_member_vars.push(Vec::new());
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

        if self.globals.contains_key(decl.name.as_str()) {
            self.report_redeclaration(&decl.name);
        }
        // `null` is the only literal with a free variable, and generalising it
        // makes `const NOTHING = null;` usable at any optional type.
        let scheme = self.store.generalize(&ty);
        self.globals
            .insert(decl.name.to_string(), GlobalRef::Const(self.consts.len()));
        self.global_spans
            .insert(decl.name.to_string(), decl.name.span);
        self.consts.push(ConstDef { scheme, kind });
    }

    /// `name` collides with a global that already exists. Builtins get their
    /// own message: "declared more than once" would send the reader looking
    /// for a first declaration that is not in the file.
    fn report_redeclaration(&mut self, name: &Ident) {
        let is_builtin = matches!(self.globals.get(name.as_str()), Some(GlobalRef::Builtin(_)));
        let first = self.global_spans.get(name.as_str()).copied();
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
            ast::TypeExpr::Named(id) => match id.as_str() {
                "i64" => Type::i64(),
                "f64" => Type::f64(),
                "bool" => Type::bool(),
                "void" => Type::void(),
                "str" => Type::str(),
                name => match self.lookup_struct(name) {
                    Some(sid) => Type::strukt(sid),
                    None => {
                        if !self.reject_abstract(name, id.span) {
                            self.error(id.span, format!("unknown type `{name}`"));
                        }
                        // Recover with a fresh variable so one bad annotation
                        // does not cascade into every use of the function.
                        self.store.fresh()
                    }
                },
            },
            ast::TypeExpr::Optional { inner, .. } => Type::optional(self.resolve_type_expr(inner)),
            ast::TypeExpr::ErrUnion { inner, .. } => Type::err_union(self.resolve_type_expr(inner)),
            ast::TypeExpr::Fn { params, ret, .. } => {
                let params = params.iter().map(|p| self.resolve_type_expr(p)).collect();
                let ret = self.resolve_type_expr(ret);
                Type::func(params, ret)
            }
        }
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
            for name in names {
                // Every member of an overload set is a dependency: the call
                // could resolve to any of them, and they must all be
                // generalised together. A `const` bound to a set depends on the
                // same functions -- including one pinned by an annotation,
                // whose member is not known until they have been inferred.
                let callees = match self.globals.get(&name) {
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

        for &id in group {
            self.infer_function(id as hir::FuncId);
        }

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
    }

    /// The function's type as written: annotated parts fixed, the rest fresh.
    ///
    /// Records what the parameters were annotated with, which is not always
    /// what the type says: an abstract annotation becomes a variable here.
    fn signature_type(&mut self, id: hir::FuncId, func: &ast::Func) -> Type {
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
        if self.struct_ids.contains_key(name.as_str()) {
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
        };
        for (param, ty) in func.params.iter().zip(&params) {
            let local = frame.add_local(param.name.as_str(), ty.clone(), false, param.span);
            frame.bind_local(param.name.as_str(), local);
            frame.params.push(local);
        }
        self.frames.push(frame);

        let body = self.infer_block(&func.body);

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
                let annotated = let_stmt.ty.as_ref().map(|t| self.resolve_type_expr(t));
                let mut init = self.infer_expr(&let_stmt.init);
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
            other => {
                self.error(other.span(), "cannot assign to this expression");
                None
            }
        }
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    fn infer_expr(&mut self, expr: &'a ast::Expr) -> hir::Expr {
        let span = expr.span();
        match expr {
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
                self.lit(hir::ExprKind::Err(id), Type::err_union(payload), span)
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

            ast::Expr::StructLit { name, fields, .. } => self.infer_struct_lit(name, fields, span),

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
                self.expect(
                    &inner.ty,
                    &Type::err_union(payload.clone()),
                    inner.span,
                    "the operand of `try`",
                );
                // `try` propagates, so the enclosing function must be fallible.
                let ret = self.frames.last().expect("in a function").ret.clone();
                let out = self.store.fresh();
                if self.store.unify(&ret, &Type::err_union(out)).is_err() {
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
                self.expect(
                    &inner.ty,
                    &Type::err_union(payload.clone()),
                    inner.span,
                    "the operand of `catch`",
                );
                self.frame().scopes.push(Vec::new());
                let capture_local = capture.as_ref().map(|name| {
                    let local =
                        self.frame()
                            .add_local(name.as_str(), Type::error(), false, name.span);
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
        // Functions come first, so that a `const` alias for an overload set
        // reads exactly as the name it aliases does.
        if let Some(ids) = self.overload_set(name.as_str()) {
            return self.func_value(name, &ids);
        }
        match self.globals.get(name.as_str()).cloned() {
            // Both are handled by `overload_set` above.
            Some(GlobalRef::Func(_) | GlobalRef::FuncValue(_)) => {
                unreachable!("a name meaning functions was resolved above")
            }
            Some(GlobalRef::Singleton(id)) => hir::Expr {
                kind: hir::ExprKind::Singleton(id),
                ty: Type::strukt(id),
                span,
            },
            Some(GlobalRef::Const(index)) => {
                let scheme = self.consts[index].scheme.clone();
                let (ty, _) = self.store.instantiate(&scheme);
                hir::Expr {
                    kind: self.consts[index].kind.clone(),
                    ty,
                    span,
                }
            }
            Some(GlobalRef::Builtin(id)) => {
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
            None => match self.globals.get(name) {
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
            // A local shadows the global of the same name, value or not.
            Some(Binding::Local(_)) => return None,
            Some(Binding::Overloads(ids)) => return Some(ids),
            None => {}
        }
        match self.globals.get(name) {
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

    fn builtin_type(&mut self, id: hir::BuiltinId) -> Type {
        let builtins = wsharp_runtime::builtins();
        let b = &builtins[id as usize];
        let params = b.params.iter().map(|t| Type::from_builtin(*t)).collect();
        Type::func(params, Type::from_builtin(b.ret))
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
                ast::Expr::Ident(name) if self.lookup_local(name.as_str()).is_none() => {
                    match self.globals.get(name.as_str()).cloned() {
                        Some(GlobalRef::Builtin(id)) => {
                            (self.builtin_type(id), hir::Callee::Builtin(id))
                        }
                        _ => {
                            let e = self.infer_expr(callee);
                            (e.ty.clone(), hir::Callee::Indirect(Box::new(e)))
                        }
                    }
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
        let mut hir_args: Vec<hir::Expr> = args.iter().map(|a| self.infer_expr(a)).collect();
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
            if params.len() != args.len() {
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
                format!("no overload of `{name}` takes {} arguments", args.len())
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
        name: &Ident,
        inits: &'a [ast::FieldInit],
        span: Span,
    ) -> hir::Expr {
        let Some(id) = self.lookup_struct(name.as_str()) else {
            // A name that exists but is not a type is a different mistake from
            // a name that does not exist, and "unknown" would send the reader
            // hunting for a typo. `find` rather than `lookup_local`: this is
            // only a question, and must not thread a capture through closures.
            let is_something_else = self.globals.contains_key(name.as_str())
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
            let value = self.infer_expr(&init.value);
            let want = decl.fields[index as usize].ty.clone();
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
            ty: Type::strukt(id),
            span,
        }
    }

    /// A `fn` literal. Its body is inferred in a fresh frame; names it uses
    /// from enclosing frames become captures, copied in by value.
    fn infer_closure(&mut self, func: &'a ast::Func, span: Span) -> hir::Expr {
        let name = Ident::new(format!("closure@{}", span.start), span);
        let id = self.declare_function(&name, func, span, true);
        let fn_ty = self.signature_type(id, func);
        self.fn_types[id as usize] = fn_ty.clone();

        let capture_sources = self.infer_function(id);
        // `fn` literals stay monomorphic: generalising them would mean
        // specialising closure bodies, which session 1 does not do.
        self.schemes[id as usize] = Some(Scheme::mono(fn_ty.clone()));

        // Captures are copied by value out of the enclosing frame, in the same
        // order the closure's own capture locals expect them.
        let enclosing = self
            .frames
            .last()
            .expect("a `fn` literal is inside a function");
        let captures = capture_sources
            .into_iter()
            .map(|source| hir::Expr {
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

    // -----------------------------------------------------------------------
    // Names and captures
    // -----------------------------------------------------------------------

    fn lookup_local(&mut self, name: &str) -> Option<hir::LocalId> {
        match self.lookup_binding(name)? {
            Binding::Local(id) => Some(id),
            Binding::Overloads(_) => None,
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
            return Some(binding);
        }
        if let Some(&id) = self.frames[depth].captured_names.get(name) {
            return Some(Binding::Local(id));
        }
        let outer = depth.checked_sub(1)?;
        // An overload set is decided at compile time, so a closure that names
        // one reaches straight past the frame boundary: there is no value to
        // copy in, and capturing it would invent a local with no type.
        let source = match self.lookup_at(outer, name)? {
            Binding::Local(id) => id,
            overloads @ Binding::Overloads(_) => return Some(overloads),
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
            Type::Con(TyCon::Struct(id), _) => {
                let decl = &self.structs[id as usize];
                match decl.field_index(field.as_str()) {
                    Some(index) => {
                        let ty = decl.fields[index as usize].ty.clone();
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

    fn intern_error(&mut self, name: &str) -> hir::ErrorId {
        if let Some(&id) = self.error_ids.get(name) {
            return id;
        }
        let id = self.errors.len() as hir::ErrorId;
        self.errors.push(name.to_string());
        self.error_ids.insert(name.to_string(), id);
        id
    }

    // -----------------------------------------------------------------------
    // Constraint solving and fix-ups
    // -----------------------------------------------------------------------

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

        for constraint in constraints {
            match constraint {
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
                        Type::Con(TyCon::I64 | TyCon::F64 | TyCon::Bool, _) => {}
                        t => {
                            let shown = self.store.show(&t);
                            self.error(
                                span,
                                format!("`{shown}` values cannot be compared with `==`"),
                            )
                            .help =
                                Some("only `i64`, `f64` and `bool` can be compared so far".into());
                        }
                    }
                }
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
                GlobalRef::Func(ids) if ids.len() > 1 && !self.is_alias(name, ids) => {
                    Some((name.clone(), ids.clone()))
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
        let Some(GlobalRef::Func(ids)) = self.globals.get("main").cloned() else {
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
    block.stmts.iter().any(stmt_terminates)
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
        ast::Expr::Ident(name) => {
            out.insert(name.to_string());
        }
        ast::Expr::Int(..)
        | ast::Expr::Float(..)
        | ast::Expr::Bool(..)
        | ast::Expr::Str(..)
        | ast::Expr::Null(..)
        | ast::Expr::ErrorLit { .. } => {}
        ast::Expr::Unary { expr, .. }
        | ast::Expr::Try { expr, .. }
        | ast::Expr::Unwrap { expr, .. }
        | ast::Expr::Field { obj: expr, .. } => collect_deps_expr(expr, out),
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

fn fixup_block(block: &mut hir::Block, structs: &[hir::StructDef], store: &mut TypeStore) {
    for stmt in &mut block.stmts {
        fixup_stmt(stmt, structs, store);
    }
}

fn fixup_stmt(stmt: &mut hir::Stmt, structs: &[hir::StructDef], store: &mut TypeStore) {
    match stmt {
        hir::Stmt::Let { init, .. } => fixup_expr(init, structs, store),
        hir::Stmt::Assign { place, value } => {
            if let hir::Place::Field {
                obj,
                strukt,
                index,
                name,
            } = place
            {
                fixup_expr(obj, structs, store);
                resolve_field(&obj.ty, name, structs, store, strukt, index);
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
