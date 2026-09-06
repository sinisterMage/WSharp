//! Cranelift JIT code generation for W#.
//!
//! Functions are declared in one pass and defined in a second, which is what
//! makes recursion and mutual recursion work: every callee already has an id by
//! the time any body is translated.
//!
//! This runs only on a monomorphised program, so every type it meets is
//! concrete.

pub mod lower;
pub mod repr;

use std::collections::{HashMap, HashSet};
use std::fmt;

use cranelift_codegen::ir;
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::settings::{self, Configurable};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{DataDescription, Linkage, Module};
use wsharp_runtime::TypeLayout;
use wsharp_runtime::header::{FLAG_IMMORTAL, TYPE_ID_FIRST_USER, TYPE_ID_STR, align_up, meta_word};
use wsharp_runtime::stackwalk::{FunctionCode, SafePoint};
use wsharp_sema::hir;
use wsharp_sema::layout;
use wsharp_sema::ty::{TyCon, Type, TypeStore};

pub use lower::Options;

#[derive(Debug)]
pub struct CodegenError(pub String);

impl fmt::Display for CodegenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodegenError {}

fn err(context: &str, e: impl fmt::Display) -> CodegenError {
    CodegenError(format!("{context}: {e}"))
}

/// A compiled program, holding the executable memory its entry point lives in.
pub struct Jit {
    /// Kept alive because it owns the mapped code. `JITModule` has no `Drop`
    /// impl, so the pages simply live until the process exits.
    _module: JITModule,
    entry: *const u8,
    entry_returns_value: bool,
    /// Cranelift IR for every function, when `Options::emit_clif` asked for it.
    pub clif: String,
}

impl Jit {
    /// Run `main` and return its result.
    ///
    /// Safe to call because the compiled code is trusted: it came from this
    /// compiler, and the type checker has already rejected anything that could
    /// misbehave.
    pub fn run(&self) -> i64 {
        // The leading argument is the environment pointer every W# function
        // takes; `main` is a top-level function, so it is null.
        let code = if self.entry_returns_value {
            let entry: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(self.entry) };
            entry(0)
        } else {
            let entry: extern "C" fn(i64) = unsafe { std::mem::transmute(self.entry) };
            entry(0);
            0
        };
        // Every worker still parked on its queue would keep the process alive,
        // and one in the middle of a trace would be left half way through it.
        wsharp_runtime::rpc::stop_all();
        // Sockets the program left open. The kernel would close them anyway;
        // doing it here releases a listener's port before the next process
        // wants it, which in a test suite is immediately.
        wsharp_runtime::net::close_all();
        // A trace may still be in flight; settle it so the report is stable.
        wsharp_runtime::gc::quiesce();
        wsharp_runtime::gc::report_if_asked();
        code
    }
}

/// Compile a monomorphised program, returning something that can be run.
pub fn compile(
    program: &hir::Program,
    store: &mut TypeStore,
    opts: &Options,
    emit_clif: bool,
) -> Result<Jit, CodegenError> {
    let entry = program
        .entry
        .ok_or_else(|| CodegenError("this program has no `main` function to run".to_string()))?;

    wsharp_runtime::gc::set_stress(opts.gc_stress);

    let mut flags = settings::builder();
    // The JIT resolves calls through absolute addresses in the same process.
    flags
        .set("use_colocated_libcalls", "false")
        .map_err(|e| err("flag", e))?;
    flags.set("is_pic", "false").map_err(|e| err("flag", e))?;
    // The collector finds its roots by following the frame-pointer chain from
    // inside the runtime, so every generated frame must have one.
    flags
        .set("preserve_frame_pointers", "true")
        .map_err(|e| err("flag", e))?;
    flags
        .set("opt_level", "speed")
        .map_err(|e| err("flag", e))?;

    let isa_builder = cranelift_native::builder()
        .map_err(|e| CodegenError(format!("unsupported host architecture: {e}")))?;
    let isa = isa_builder
        .finish(settings::Flags::new(flags))
        .map_err(|e| err("could not configure the target", e))?;

    let mut jit_builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
    for (name, ptr) in wsharp_runtime::runtime_symbols() {
        jit_builder.symbol(name, ptr);
    }
    let builtins = wsharp_runtime::builtins();
    for builtin in &builtins {
        jit_builder.symbol(builtin.symbol(), builtin.ptr);
    }

    let mut module = JITModule::new(jit_builder);
    let call_conv = module.target_config().default_call_conv;

    register_layouts(program, store);
    let closure_type_ids = register_closure_layouts(program, store);
    // Arrays and generic instantiations continue past the closures, which
    // continue past the structs.
    let array_base = TYPE_ID_FIRST_USER + program.structs.len() as u32 + program.funcs.len() as u32;
    let instance_type_ids = register_instance_layouts(program, store, array_base);
    // Freeze the registry now that every type is in it: from here the collector
    // reads layouts with no lock, which is what makes tracing affordable.
    wsharp_runtime::publish();

    let decls = declare_all(
        &mut module,
        program,
        store,
        &builtins,
        call_conv,
        closure_type_ids,
        instance_type_ids,
    )?;

    let mut clif = String::new();
    let mut ctx = module.make_context();
    let mut fb_ctx = FunctionBuilderContext::new();
    let mut harvested: Vec<(cranelift_module::FuncId, HarvestedCode)> =
        Vec::with_capacity(program.funcs.len());

    for (id, func) in program.funcs.iter().enumerate() {
        let clif_id = decls.funcs[id];
        ctx.func.signature = lower::signature_of(store, func, call_conv);
        ctx.func.name = ir::UserFuncName::user(0, clif_id.as_u32());

        {
            let builder = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
            lower::translate(builder, &mut module, program, store, &decls, func);
        }

        if emit_clif {
            clif.push_str(&format!("; {} (#{id})\n{}\n", func.name, ctx.func));
        }

        module
            .define_function(clif_id, &mut ctx)
            .map_err(|e| err(&format!("could not compile `{}`", func.name), e))?;
        // Harvest the stack maps before `clear_context` discards them. Absolute
        // addresses are not known until the module is finalized, so keep the
        // Cranelift function id and resolve it below.
        harvested.push((clif_id, harvest_stack_maps(&ctx)));
        module.clear_context(&mut ctx);
    }

    // One trampoline per service function, emitted after the functions they
    // call so that every `FuncId` they need is declared.
    let services = define_trampolines(
        &mut module,
        program,
        store,
        &decls,
        call_conv,
        &mut ctx,
        &mut fb_ctx,
        &mut harvested,
    )?;

    module
        .finalize_definitions()
        .map_err(|e| err("could not finalize the module", e))?;

    register_stack_maps(&module, harvested);
    register_services(&module, program, services);

    let entry_func = program.func(entry);
    let entry_returns_value = !lower::returns_nothing(store, &entry_func.ret);
    let entry_ptr = module.get_finalized_function(decls.funcs[entry as usize]);

    Ok(Jit {
        _module: module,
        entry: entry_ptr,
        entry_returns_value,
        clif,
    })
}

/// A trampoline, pending the address it will finally live at.
struct Trampoline {
    clif_id: cranelift_module::FuncId,
    /// Which service, and which of its methods -- `None` for its `init`.
    service: usize,
    method: Option<usize>,
    name: String,
    args: Vec<wsharp_runtime::rpc::SlotKind>,
    ret: Vec<wsharp_runtime::rpc::SlotKind>,
}

/// Emit the unpacking stub each service function is called through.
///
/// A worker is handed its arguments as bytes and has to turn them back into a
/// call. Doing that in Rust would mean moving references into a call with none
/// of the write barrier, the load barrier or the stack maps -- the mistake
/// `array.concat` taught -- so it is done in generated code, where all three
/// apply by construction.
#[allow(clippy::too_many_arguments)]
fn define_trampolines(
    module: &mut JITModule,
    program: &hir::Program,
    store: &mut TypeStore,
    decls: &lower::Decls,
    call_conv: cranelift_codegen::isa::CallConv,
    ctx: &mut cranelift_codegen::Context,
    fb_ctx: &mut FunctionBuilderContext,
    harvested: &mut Vec<(cranelift_module::FuncId, HarvestedCode)>,
) -> Result<Vec<Trampoline>, CodegenError> {
    let mut out = Vec::new();
    for (sid, service) in program.services.iter().enumerate() {
        let mut wanted: Vec<(Option<usize>, String, hir::FuncId, bool)> =
            vec![(None, format!("service{sid}$init"), service.init, false)];
        for (m, method) in service.methods.iter().enumerate() {
            wanted.push((Some(m), format!("service{sid}$m{m}"), method.func, true));
        }

        for (method, name, target_id, takes_state) in wanted {
            let target = program.func(target_id);
            let sig = lower::trampoline_signature(call_conv, takes_state);
            let clif_id = module
                .declare_function(&name, Linkage::Local, &sig)
                .map_err(|e| err(&format!("could not declare `{name}`"), e))?;

            ctx.func.signature = sig;
            ctx.func.name = ir::UserFuncName::user(0, clif_id.as_u32());
            {
                let builder = FunctionBuilder::new(&mut ctx.func, fb_ctx);
                lower::trampoline(
                    builder,
                    module,
                    store,
                    decls,
                    target,
                    target_id,
                    takes_state,
                );
            }
            module
                .define_function(clif_id, ctx)
                .map_err(|e| err(&format!("could not compile `{name}`"), e))?;
            harvested.push((clif_id, harvest_stack_maps(ctx)));
            module.clear_context(ctx);

            // The state is the worker's own and never crosses, so a method's
            // wire arguments start after it.
            let skip = usize::from(takes_state);
            let mut args = Vec::new();
            for param in target.params.iter().skip(skip) {
                let ty = target.locals[*param as usize].ty.clone();
                args.extend(lower::slot_kinds(store, &ty));
            }
            let ret = lower::slot_kinds(store, &target.ret.clone());
            out.push(Trampoline {
                clif_id,
                service: sid,
                method,
                name: match method {
                    None => "init".to_string(),
                    Some(m) => service.methods[m].name.clone(),
                },
                args,
                ret,
            });
        }
    }
    Ok(out)
}

/// Hand the runtime the finished addresses, so a worker can call into a
/// service it was only ever given the number of.
fn register_services(module: &JITModule, program: &hir::Program, found: Vec<Trampoline>) {
    use wsharp_runtime::rpc::{MethodCode, ServiceCode};
    let mut services: Vec<Vec<Trampoline>> = program.services.iter().map(|_| Vec::new()).collect();
    for t in found {
        services[t.service].push(t);
    }
    let mut out: Vec<&'static ServiceCode> = Vec::new();
    for (sid, mut entries) in services.into_iter().enumerate() {
        entries.sort_by_key(|t| t.method.map_or(0, |m| m + 1));
        let init = entries.remove(0);
        let methods: Vec<MethodCode> = entries
            .into_iter()
            .map(|t| MethodCode {
                name: Box::leak(t.name.into_boxed_str()),
                call: module.get_finalized_function(t.clif_id),
                args: Box::leak(t.args.into_boxed_slice()),
                ret: Box::leak(t.ret.into_boxed_slice()),
            })
            .collect();
        out.push(Box::leak(Box::new(ServiceCode {
            name: Box::leak(program.services[sid].name.clone().into_boxed_str()),
            init: module.get_finalized_function(init.clif_id),
            init_args: Box::leak(init.args.into_boxed_slice()),
            methods: Box::leak(methods.into_boxed_slice()),
        })));
    }
    wsharp_runtime::rpc::register_services(out);
}

/// A function's code length and its safepoints, pending a base address.
struct HarvestedCode {
    len: usize,
    safepoints: Vec<SafePoint>,
}

/// Read Cranelift's stack maps for the function just compiled into `ctx`.
///
/// Valid only between `define_function` and `clear_context`. Each entry is
/// keyed by the **return address** of a call, and the `u32` alongside it is the
/// frame's `sp_to_fp` distance -- so at that safepoint the stack pointer is
/// `fp - frame_size` and each root sits at `sp + offset`.
fn harvest_stack_maps(ctx: &cranelift_codegen::Context) -> HarvestedCode {
    let compiled = ctx.compiled_code().expect("the function was just compiled");
    let safepoints = compiled
        .buffer
        .user_stack_maps()
        .iter()
        .map(|(return_offset, frame_size, map)| SafePoint {
            return_offset: *return_offset,
            frame_size: *frame_size,
            roots: map
                .entries()
                .filter(|(ty, _)| *ty == repr::PTR)
                .map(|(_, offset)| offset)
                .collect(),
        })
        .collect();
    HarvestedCode {
        len: compiled.code_info().total_size as usize,
        safepoints,
    }
}

/// Hand the finalized code ranges and their stack maps to the runtime, which is
/// what lets the collector turn a return address into a set of live roots.
fn register_stack_maps(
    module: &JITModule,
    harvested: Vec<(cranelift_module::FuncId, HarvestedCode)>,
) {
    let funcs = harvested
        .into_iter()
        .map(|(id, code)| FunctionCode {
            base: module.get_finalized_function(id) as usize,
            len: code.len,
            safepoints: code.safepoints.into_boxed_slice(),
        })
        .collect();
    wsharp_runtime::stackwalk::register_code(funcs);
}

/// Tell the runtime the shape of every struct, so the collector can trace
/// instances without knowing anything about W# types.
fn register_layouts(program: &hir::Program, store: &mut TypeStore) {
    for def in &program.structs {
        // A generic struct has no instances of its own; each instantiation is
        // registered separately, with the offsets its arguments imply.
        if !def.params.is_empty() {
            continue;
        }
        let mut ptr_offsets = Vec::new();
        for field in &def.fields {
            let ty = field.ty.clone();
            layout::ptr_offsets(store, &ty, field.offset, &mut ptr_offsets);
        }
        wsharp_runtime::register_type(
            def.type_id,
            TypeLayout::fixed(def.name.clone(), def.size, ptr_offsets),
        );
    }
}

/// The field types of one instantiation of a generic struct, with its
/// arguments substituted for the parameters the declaration wrote.
///
/// The identity for a non-generic struct, which is every struct until one is
/// declared `struct[T]`.
pub(crate) fn instance_field_types(
    store: &mut TypeStore,
    def: &hir::StructDef,
    ty: &Type,
) -> Vec<Type> {
    let args = match store.resolve(ty) {
        Type::Con(TyCon::Struct(_), args) => args,
        _ => Vec::new(),
    };
    if def.params.len() != args.len() {
        return def.fields.iter().map(|f| f.ty.clone()).collect();
    }
    let subst: HashMap<_, _> = def.params.iter().copied().zip(args).collect();
    def.fields
        .iter()
        .map(|f| store.subst_vars(&f.ty, &subst))
        .collect()
}

/// Give every array type and every generic-struct instantiation in the program
/// a runtime type id and a layout, so the collector can trace one.
///
/// Neither can be numbered with the ordinary structs. An array needs one id per
/// `[]T` rather than one for all arrays, because the layout is what carries the
/// element stride and where a reference sits inside an element -- `[]i64` and
/// `[]?str` are traced quite differently. A generic struct's instantiations are
/// not known until monomorphisation has picked their arguments, which is after
/// the lattice has been numbered; they take ids from above it, which is sound
/// because a generic struct stands outside the lattice and so is never the
/// subject of a range test.
fn register_instance_layouts(
    program: &hir::Program,
    store: &mut TypeStore,
    base: u32,
) -> HashMap<String, u32> {
    let mut ids = HashMap::new();
    for ty in collect_instance_types(program, store) {
        let key = store.show(&ty);
        if ids.contains_key(&key) {
            continue;
        }
        let type_id = base + ids.len() as u32;
        let layout = match store.resolve(&ty) {
            Type::Con(TyCon::Array, args) => {
                let (stride, elem_ptr_offsets) = layout::elem_layout(store, &args[0]);
                TypeLayout::elements(key.clone(), stride, elem_ptr_offsets)
            }
            Type::Con(TyCon::Struct(id), _) => {
                let def = program.strukt(id).clone();
                let field_types = instance_field_types(store, &def, &ty);
                let (offsets, end) =
                    layout::place(store, &field_types, wsharp_runtime::HEADER_SIZE);
                let mut ptr_offsets = Vec::new();
                for (offset, field_ty) in offsets.iter().zip(&field_types) {
                    layout::ptr_offsets(store, field_ty, *offset, &mut ptr_offsets);
                }
                TypeLayout::fixed(key.clone(), align_up(end), ptr_offsets)
            }
            other => unreachable!("`{}` is not an instance type", store.show(&other)),
        };
        wsharp_runtime::register_type(type_id, layout);
        ids.insert(key, type_id);
    }
    ids
}

/// Every array type and generic-struct instantiation mentioned anywhere in the
/// program, in a deterministic order so the ids do not depend on hashing.
///
/// Expression types are visited as well as declarations: monomorphisation
/// makes every intermediate type concrete, and `f(g())[0]` mentions an array
/// type that no local and no signature does.
fn collect_instance_types(program: &hir::Program, store: &mut TypeStore) -> Vec<Type> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |store: &mut TypeStore, ty: &Type, out: &mut Vec<Type>| {
        collect_instances_in(store, ty, out, &mut seen)
    };
    for def in &program.structs {
        for field in &def.fields {
            let ty = field.ty.clone();
            push(store, &ty, &mut out);
        }
    }
    // A top-level `const` array needs its type id even when nothing names it:
    // `define_arrays` emits one object per declaration, exactly as
    // `define_strings` emits one per interned literal, and neither asks
    // whether monomorphisation kept a use.
    for array in &program.arrays {
        let ty = array.ty.clone();
        push(store, &ty, &mut out);
    }
    for func in &program.funcs {
        let ret = func.ret.clone();
        push(store, &ret, &mut out);
        for local in &func.locals {
            let ty = local.ty.clone();
            push(store, &ty, &mut out);
        }
        let mut types = Vec::new();
        types_in_block(&func.body, &mut types);
        for ty in types {
            push(store, &ty, &mut out);
        }
    }
    out
}

/// Add every array type and generic-struct instantiation inside `ty`,
/// innermost first, so `[][]i64` registers `[]i64` as well as itself.
fn collect_instances_in(
    store: &mut TypeStore,
    ty: &Type,
    out: &mut Vec<Type>,
    seen: &mut HashSet<String>,
) {
    let resolved = store.resolve(ty);
    let Type::Con(con, args) = resolved.clone() else {
        return;
    };
    for arg in &args {
        collect_instances_in(store, arg, out, seen);
    }
    let is_instance = match con {
        TyCon::Array => true,
        // A struct with arguments is an instantiation of a generic one.
        TyCon::Struct(_) => !args.is_empty(),
        _ => false,
    };
    if is_instance && seen.insert(store.show(&resolved)) {
        out.push(resolved);
    }
}

fn types_in_block(block: &hir::Block, out: &mut Vec<Type>) {
    for stmt in &block.stmts {
        types_in_stmt(stmt, out);
    }
}

fn types_in_stmt(stmt: &hir::Stmt, out: &mut Vec<Type>) {
    match stmt {
        hir::Stmt::Let { init, .. } => types_in_expr(init, out),
        hir::Stmt::Assign { place, value } => {
            match place {
                hir::Place::Field { obj, .. } => types_in_expr(obj, out),
                hir::Place::Index { arr, index } => {
                    types_in_expr(arr, out);
                    types_in_expr(index, out);
                }
                hir::Place::Local(_) => {}
            }
            types_in_expr(value, out);
        }
        hir::Stmt::Expr(e) | hir::Stmt::Return(Some(e)) => types_in_expr(e, out),
        hir::Stmt::Return(None) | hir::Stmt::Break | hir::Stmt::Continue => {}
        hir::Stmt::If {
            cond, then, els, ..
        } => {
            types_in_expr(cond, out);
            types_in_block(then, out);
            if let Some(els) = els {
                types_in_block(els, out);
            }
        }
        hir::Stmt::While {
            cond, cont, body, ..
        } => {
            types_in_expr(cond, out);
            if let Some(cont) = cont {
                types_in_stmt(cont, out);
            }
            types_in_block(body, out);
        }
        hir::Stmt::Block(b) => types_in_block(b, out),
    }
}

fn types_in_expr(expr: &hir::Expr, out: &mut Vec<Type>) {
    out.push(expr.ty.clone());
    match &expr.kind {
        hir::ExprKind::Block { stmts, value } => {
            for stmt in stmts {
                types_in_stmt(stmt, out);
            }
            if let Some(v) = value {
                types_in_expr(v, out);
            }
        }
        hir::ExprKind::Spawn { args, .. } => {
            for arg in args {
                types_in_expr(arg, out);
            }
        }
        hir::ExprKind::Join(worker) => types_in_expr(worker, out),
        hir::ExprKind::Convert(expr)
        | hir::ExprKind::Unary { expr, .. }
        | hir::ExprKind::Some(expr)
        | hir::ExprKind::Ok(expr)
        | hir::ExprKind::Try(expr)
        | hir::ExprKind::Unwrap(expr)
        | hir::ExprKind::Field { obj: expr, .. } => types_in_expr(expr, out),
        hir::ExprKind::Binary { lhs, rhs, .. } | hir::ExprKind::Logical { lhs, rhs, .. } => {
            types_in_expr(lhs, out);
            types_in_expr(rhs, out);
        }
        hir::ExprKind::Orelse { expr, alt } | hir::ExprKind::Catch { expr, alt, .. } => {
            types_in_expr(expr, out);
            types_in_expr(alt, out);
        }
        hir::ExprKind::Call { callee, args } => {
            match callee {
                hir::Callee::Indirect(e) | hir::Callee::Rpc { worker: e, .. } => {
                    types_in_expr(e, out)
                }
                _ => {}
            }
            for arg in args {
                types_in_expr(arg, out);
            }
        }
        hir::ExprKind::StructNew { fields, .. } => {
            for f in fields {
                types_in_expr(f, out);
            }
        }
        hir::ExprKind::ArrayNew { elems } => {
            for e in elems {
                types_in_expr(e, out);
            }
        }
        hir::ExprKind::Index { arr, index } => {
            types_in_expr(arr, out);
            types_in_expr(index, out);
        }
        hir::ExprKind::ArrayLen { arr } => types_in_expr(arr, out),
        hir::ExprKind::If {
            cond, then, els, ..
        } => {
            types_in_expr(cond, out);
            types_in_expr(then, out);
            types_in_expr(els, out);
        }
        hir::ExprKind::Closure { captures, .. } => {
            for c in captures {
                types_in_expr(c, out);
            }
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

/// Closures are heap objects too, and each one has its own shape. Giving every
/// `fn` literal its own type id means the collector can find the captured
/// references inside a closure the same way it finds a struct's fields.
///
/// *Every* function gets one, not only the `fn` literals: a top-level function
/// used as a value -- `const g = inc;` -- is also handed to the allocator as a
/// closure, one with no captures. Leaving those unregistered gave them a type
/// id with no layout behind it, so the collector could neither size them nor
/// copy them, and evacuating one truncated it to its header and lost the code
/// pointer it existed to carry.
fn register_closure_layouts(program: &hir::Program, store: &mut TypeStore) -> Vec<u32> {
    // Struct ids run from TYPE_ID_FIRST_USER; closures continue past them, one
    // per function, so a function's id is its index.
    let base = TYPE_ID_FIRST_USER + program.structs.len() as u32;
    let mut ids = Vec::with_capacity(program.funcs.len());
    for (id, func) in program.funcs.iter().enumerate() {
        let type_id = base + id as u32;
        let (size, ptr_offsets) = lower::closure_layout(store, func);
        let what = if func.is_closure { "closure" } else { "fn" };
        wsharp_runtime::register_type(
            type_id,
            TypeLayout::fixed(format!("{what} {}", func.name), size, ptr_offsets),
        );
        ids.push(type_id);
    }
    ids
}

fn declare_all(
    module: &mut JITModule,
    program: &hir::Program,
    store: &mut TypeStore,
    builtins: &[wsharp_runtime::Builtin],
    call_conv: CallConv,
    closure_type_ids: Vec<u32>,
    instance_type_ids: HashMap<String, u32>,
) -> Result<lower::Decls, CodegenError> {
    // Monomorphisation produces several functions with the same source name, so
    // the linkage name carries the index too.
    let mut funcs = Vec::with_capacity(program.funcs.len());
    for (id, func) in program.funcs.iter().enumerate() {
        let sig = lower::signature_of(store, func, call_conv);
        let name = format!("{}${id}", func.name);
        let clif_id = module
            .declare_function(&name, Linkage::Local, &sig)
            .map_err(|e| err(&format!("could not declare `{}`", func.name), e))?;
        funcs.push(clif_id);
    }

    let mut builtin_ids = Vec::with_capacity(builtins.len());
    for builtin in builtins {
        let sig = lower::builtin_signature(builtin, call_conv);
        let id = module
            .declare_function(&builtin.symbol(), Linkage::Import, &sig)
            .map_err(|e| err(&format!("could not declare builtin `{}`", builtin.name), e))?;
        builtin_ids.push(id);
    }

    let mut alloc_sig = ir::Signature::new(call_conv);
    alloc_sig
        .params
        .push(ir::AbiParam::new(ir::types::I32).uext());
    alloc_sig.params.push(ir::AbiParam::new(ir::types::I64));
    // The element count, written into `aux` before the allocator's safepoint.
    alloc_sig.params.push(ir::AbiParam::new(ir::types::I64));
    alloc_sig.returns.push(ir::AbiParam::new(repr::PTR));
    let alloc = module
        .declare_function("ws_alloc", Linkage::Import, &alloc_sig)
        .map_err(|e| err("could not declare `ws_alloc`", e))?;

    let mut panic_sig = ir::Signature::new(call_conv);
    panic_sig.params.push(ir::AbiParam::new(ir::types::I64));
    let panic = module
        .declare_function("ws_panic", Linkage::Import, &panic_sig)
        .map_err(|e| err("could not declare `ws_panic`", e))?;

    // The out-of-bounds panic takes the index and the length, so that the
    // message can name them.
    let mut panic_index_sig = ir::Signature::new(call_conv);
    panic_index_sig
        .params
        .push(ir::AbiParam::new(ir::types::I64));
    panic_index_sig
        .params
        .push(ir::AbiParam::new(ir::types::I64));
    let panic_index = module
        .declare_function("ws_panic_index", Linkage::Import, &panic_index_sig)
        .map_err(|e| err("could not declare `ws_panic_index`", e))?;

    // The write barrier's slow path, called only the first time an object is
    // modified in a collection cycle.
    let mut log_sig = ir::Signature::new(call_conv);
    log_sig.params.push(ir::AbiParam::new(repr::PTR));
    let log_object = module
        .declare_function("ws_log_object", Linkage::Import, &log_sig)
        .map_err(|e| err("could not declare `ws_log_object`", e))?;

    // The load barrier's slow path: hand back where a reference lives now.
    // Called only while a trace is moving objects, which the flag it tests
    // says; the rest of the time the barrier is a load and a branch.
    let mut resolve_sig = ir::Signature::new(call_conv);
    resolve_sig.params.push(ir::AbiParam::new(repr::PTR));
    resolve_sig.returns.push(ir::AbiParam::new(repr::PTR));
    let resolve = module
        .declare_function("ws_resolve", Linkage::Import, &resolve_sig)
        .map_err(|e| err("could not declare `ws_resolve`", e))?;

    // The loop safepoint: no arguments, no result, called only when the poll
    // byte says a collection is wanted.
    let gc_poll = module
        .declare_function(
            "ws_gc_poll",
            Linkage::Import,
            &ir::Signature::new(call_conv),
        )
        .map_err(|e| err("could not declare `ws_gc_poll`", e))?;

    // Starting a worker, calling into one, and waiting for one. Each takes
    // its arguments as a buffer of machine words, because only the call site
    // knows what shape they are and only generated code may build one.
    let mut spawn_sig = ir::Signature::new(call_conv);
    spawn_sig
        .params
        .push(ir::AbiParam::new(ir::types::I32).uext());
    spawn_sig.params.push(ir::AbiParam::new(repr::PTR));
    spawn_sig.returns.push(ir::AbiParam::new(ir::types::I64));
    let spawn = module
        .declare_function("ws_spawn", Linkage::Import, &spawn_sig)
        .map_err(|e| err("could not declare `ws_spawn`", e))?;

    let mut rpc_sig = ir::Signature::new(call_conv);
    rpc_sig.params.push(ir::AbiParam::new(ir::types::I64));
    rpc_sig
        .params
        .push(ir::AbiParam::new(ir::types::I32).uext());
    rpc_sig
        .params
        .push(ir::AbiParam::new(ir::types::I32).uext());
    rpc_sig.params.push(ir::AbiParam::new(repr::PTR));
    rpc_sig.params.push(ir::AbiParam::new(repr::PTR));
    rpc_sig.returns.push(ir::AbiParam::new(ir::types::I64));
    let rpc_call = module
        .declare_function("ws_rpc_call", Linkage::Import, &rpc_sig)
        .map_err(|e| err("could not declare `ws_rpc_call`", e))?;

    let mut join_sig = ir::Signature::new(call_conv);
    join_sig.params.push(ir::AbiParam::new(ir::types::I64));
    join_sig.returns.push(ir::AbiParam::new(ir::types::I64));
    let join = module
        .declare_function("ws_join", Linkage::Import, &join_sig)
        .map_err(|e| err("could not declare `ws_join`", e))?;

    let strings = define_strings(module, program)?;
    let arrays = define_arrays(module, program, store, &instance_type_ids)?;
    let singletons = define_singletons(module, program)?;

    Ok(lower::Decls {
        funcs,
        builtins: builtin_ids,
        strings,
        arrays,
        singletons,
        closure_type_ids,
        instance_type_ids,
        alloc,
        panic,
        panic_index,
        log_object,
        gc_poll,
        resolve,
        spawn,
        rpc_call,
        join,
    })
}

/// Emit the sole instance of each struct that has no fields.
///
/// A zero-field struct is a name in the dispatch lattice -- `NotFound404` --
/// and mentioning it should not allocate. One static object per type, in the
/// data section beside the string literals and immortal for the same reason:
/// the collector must neither move nor free it.
fn define_singletons(
    module: &mut JITModule,
    program: &hir::Program,
) -> Result<Vec<Option<cranelift_module::DataId>>, CodegenError> {
    let mut ids = Vec::with_capacity(program.structs.len());
    for (i, def) in program.structs.iter().enumerate() {
        if !def.fields.is_empty() {
            ids.push(None);
            continue;
        }
        let mut bytes = Vec::with_capacity(wsharp_runtime::HEADER_SIZE as usize);
        bytes.extend_from_slice(&meta_word(def.type_id, FLAG_IMMORTAL).to_ne_bytes());
        // `aux` is the element count for strings and arrays; zero here.
        bytes.extend_from_slice(&0u64.to_ne_bytes());

        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        desc.set_align(wsharp_runtime::header::ALIGN as u64);

        let id = module
            .declare_data(
                &format!("wsharp$singleton${i}"),
                Linkage::Local,
                false,
                false,
            )
            .map_err(|e| err(&format!("could not declare `{}`", def.name), e))?;
        module
            .define_data(id, &desc)
            .map_err(|e| err(&format!("could not define `{}`", def.name), e))?;
        ids.push(Some(id));
    }
    Ok(ids)
}

/// Emit each top-level `const` array as a static object with a W# header.
///
/// `define_strings` with a wider element: the same immortal flag, for the same
/// reason, and the same padding so the next object would still be aligned. The
/// two differences are that the type id is the array type's rather than
/// `TYPE_ID_STR`, and that `aux` counts elements rather than bytes.
///
/// The elements are scalars, which inference has already insisted on -- so
/// nothing here holds a reference, and an object the collector never traces
/// cannot hide one.
fn define_arrays(
    module: &mut JITModule,
    program: &hir::Program,
    store: &mut TypeStore,
    instance_type_ids: &HashMap<String, u32>,
) -> Result<Vec<cranelift_module::DataId>, CodegenError> {
    let mut ids = Vec::with_capacity(program.arrays.len());
    for (i, array) in program.arrays.iter().enumerate() {
        let key = store.show(&array.ty);
        let type_id = *instance_type_ids
            .get(&key)
            .unwrap_or_else(|| panic!("no type id registered for `{key}`"));
        let elem_ty = match store.resolve(&array.ty) {
            Type::Con(TyCon::Array, args) => args[0].clone(),
            other => unreachable!("`{}` is not an array", store.show(&other)),
        };
        let stride = layout::size_of(store, &elem_ty) as usize;

        let mut bytes =
            Vec::with_capacity(wsharp_runtime::HEADER_SIZE as usize + array.values.len() * stride);
        bytes.extend_from_slice(&meta_word(type_id, FLAG_IMMORTAL).to_ne_bytes());
        bytes.extend_from_slice(&(array.values.len() as u64).to_ne_bytes());
        for value in &array.values {
            // The low `stride` bytes, in the machine's own order -- which is
            // little-endian on both architectures this collector supports,
            // since the stack walker reads the frame pointer with inline
            // assembly written for exactly those two.
            bytes.extend_from_slice(&value.to_ne_bytes()[..stride]);
        }
        bytes.resize(align_up(bytes.len() as u32) as usize, 0);

        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        desc.set_align(wsharp_runtime::header::ALIGN as u64);

        // Writable, though nothing should write it: inference rejects writing
        // an element through the `const`'s own name, but an alias -- `var a =
        // K;` -- is a local holding the same address and is not caught. A
        // read-only page would turn that mistake into a fault with no message,
        // which is worse than the mistake. See the ROADMAP.
        let id = module
            .declare_data(&format!("wsharp$array${i}"), Linkage::Local, true, false)
            .map_err(|e| err("could not declare a `const` array", e))?;
        module
            .define_data(id, &desc)
            .map_err(|e| err("could not define a `const` array", e))?;
        ids.push(id);
    }
    Ok(ids)
}

/// Emit each string literal as a static object with a W# header.
///
/// They carry the immortal flag: they live in the module's data section rather
/// than the W# heap, so a collector must neither move nor free them.
fn define_strings(
    module: &mut JITModule,
    program: &hir::Program,
) -> Result<Vec<cranelift_module::DataId>, CodegenError> {
    let mut ids = Vec::with_capacity(program.strings.len());
    for (i, text) in program.strings.iter().enumerate() {
        let mut bytes = Vec::with_capacity(wsharp_runtime::HEADER_SIZE as usize + text.len());
        bytes.extend_from_slice(&meta_word(TYPE_ID_STR, FLAG_IMMORTAL).to_ne_bytes());
        bytes.extend_from_slice(&(text.len() as u64).to_ne_bytes());
        bytes.extend_from_slice(text.as_bytes());
        // Pad so the next object would still be aligned, matching the heap.
        bytes.resize(align_up(bytes.len() as u32) as usize, 0);

        let mut desc = DataDescription::new();
        desc.define(bytes.into_boxed_slice());
        desc.set_align(wsharp_runtime::header::ALIGN as u64);

        let id = module
            .declare_data(&format!("wsharp$str${i}"), Linkage::Local, false, false)
            .map_err(|e| err("could not declare a string literal", e))?;
        module
            .define_data(id, &desc)
            .map_err(|e| err("could not define a string literal", e))?;
        ids.push(id);
    }
    Ok(ids)
}
