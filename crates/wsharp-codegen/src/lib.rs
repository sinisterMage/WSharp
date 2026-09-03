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
use wsharp_sema::ty::TypeStore;

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
        jit_builder.symbol(builtin.name, builtin.ptr);
    }

    let mut module = JITModule::new(jit_builder);
    let call_conv = module.target_config().default_call_conv;

    register_layouts(program, store);
    let closure_type_ids = register_closure_layouts(program, store);
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

    module
        .finalize_definitions()
        .map_err(|e| err("could not finalize the module", e))?;

    register_stack_maps(&module, harvested);

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
        let mut ptr_offsets = Vec::new();
        for field in &def.fields {
            let ty = field.ty.clone();
            layout::ptr_offsets(store, &ty, field.offset, &mut ptr_offsets);
        }
        wsharp_runtime::register_type(
            def.type_id,
            TypeLayout {
                name: def.name.clone(),
                size: def.size,
                ptr_offsets,
            },
        );
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
            TypeLayout {
                name: format!("{what} {}", func.name),
                size,
                ptr_offsets,
            },
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
            .declare_function(builtin.name, Linkage::Import, &sig)
            .map_err(|e| err(&format!("could not declare builtin `{}`", builtin.name), e))?;
        builtin_ids.push(id);
    }

    let mut alloc_sig = ir::Signature::new(call_conv);
    alloc_sig
        .params
        .push(ir::AbiParam::new(ir::types::I32).uext());
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

    let strings = define_strings(module, program)?;
    let singletons = define_singletons(module, program)?;

    Ok(lower::Decls {
        funcs,
        builtins: builtin_ids,
        strings,
        singletons,
        closure_type_ids,
        alloc,
        panic,
        log_object,
        gc_poll,
        resolve,
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
