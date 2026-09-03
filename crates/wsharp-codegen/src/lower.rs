//! Lowering typed HIR to Cranelift IR, one function at a time.
//!
//! Locals are Cranelift `Variable`s, one per slot, so the frontend builds SSA
//! for us and `if`/`while` need no phi bookkeeping beyond block parameters.
//!
//! Every W# function takes a leading environment pointer. Top-level functions
//! ignore it and are called with null; closures receive their own object and
//! read captured values out of it. Making the calling convention uniform is
//! what lets a plain `fn` be passed around as a value without a wrapper.

use cranelift_codegen::ir::{
    self, AbiParam, BlockArg, InstBuilder, MemFlagsData, Signature, types,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, Variable};
use cranelift_jit::JITModule;
use cranelift_module::{DataId, FuncId, Module};
use smallvec::SmallVec;
use wsharp_runtime::builtins::{
    BuiltinTy, PANIC_DIVIDE_BY_ZERO, PANIC_DIVIDE_OVERFLOW, PANIC_NO_METHOD, PANIC_UNWRAP_NULL,
};
use wsharp_runtime::header::{
    FLAG_LOGGED, FLAG_SHIFT, META_OFFSET, TYPE_ID_CLOSURE, TYPE_ID_MASK, align_up,
};
use wsharp_sema::hir;
use wsharp_sema::layout;
use wsharp_sema::ty::{StructId, TyCon, Type, TypeStore};
use wsharp_syntax::ast::{BinOp, UnOp};

use crate::repr::{
    self, ERROR_OK, ERROR_TAG, OPTION_NULL, OPTION_SOME, PTR, SlotTypes, Slots, error_tag_value,
};

/// A closure object is a header, then the code pointer, then the captures.
pub const CLOSURE_CODE_OFFSET: u32 = wsharp_runtime::HEADER_SIZE;
pub const CLOSURE_CAPTURES_OFFSET: u32 = CLOSURE_CODE_OFFSET + 8;

const NO_ARGS: &[BlockArg] = &[];

/// Cranelift function ids for everything generated code can call.
pub struct Decls {
    /// Indexed by `hir::FuncId`.
    pub funcs: Vec<FuncId>,
    /// Indexed by `hir::BuiltinId`.
    pub builtins: Vec<FuncId>,
    /// Indexed by `hir::StrId`.
    pub strings: Vec<DataId>,
    /// Indexed by `StructId`; only zero-field structs have one. Their sole
    /// instance lives in the data section, so naming a status type is free.
    pub singletons: Vec<Option<DataId>>,
    /// Runtime type id per `hir::FuncId`, meaningful only for `fn` literals.
    /// Each closure gets its own so the collector can find its captures.
    pub closure_type_ids: Vec<u32>,
    pub alloc: FuncId,
    pub panic: FuncId,
    /// The write barrier's slow path.
    pub log_object: FuncId,
    /// The loop safepoint's slow path.
    pub gc_poll: FuncId,
}

/// The heap layout of the closure object for a `fn` literal: its total size,
/// and the offsets of the captured values that are heap references.
pub fn closure_layout(store: &mut TypeStore, func: &hir::FuncDef) -> (u32, Vec<u32>) {
    let mut size = CLOSURE_CAPTURES_OFFSET;
    let mut ptr_offsets = Vec::new();
    for capture in &func.captures {
        let ty = func.locals[*capture as usize].ty.clone();
        layout::ptr_offsets(store, &ty, size, &mut ptr_offsets);
        size += layout::size_of(store, &ty);
    }
    (align_up(size), ptr_offsets)
}

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Collect at every allocation, and check every root the stack walk finds.
    ///
    /// Ruinous for performance and invaluable for correctness: a missed root
    /// becomes a loud, immediate, reproducible failure instead of a rare
    /// corruption. The whole end-to-end suite runs a second time under it.
    pub gc_stress: bool,
}

/// The Cranelift signature of a W# function: environment pointer, then each
/// parameter's slots, returning the return type's slots.
pub fn signature_of(store: &mut TypeStore, func: &hir::FuncDef, call_conv: CallConv) -> Signature {
    let mut sig = Signature::new(call_conv);
    sig.params.push(AbiParam::new(PTR));
    for param in &func.params {
        let ty = func.locals[*param as usize].ty.clone();
        for slot in repr::slot_types(store, &ty) {
            sig.params.push(AbiParam::new(slot));
        }
    }
    for slot in repr::slot_types(store, &func.ret) {
        sig.returns.push(AbiParam::new(slot));
    }
    sig
}

/// The signature to call a value of function type through.
pub fn indirect_signature(store: &mut TypeStore, fn_ty: &Type, call_conv: CallConv) -> Signature {
    let mut sig = Signature::new(call_conv);
    sig.params.push(AbiParam::new(PTR));
    let (params, ret) = fn_ty.as_fn().expect("callee has a function type");
    let params = params.to_vec();
    let ret = ret.clone();
    for param in &params {
        for slot in repr::slot_types(store, param) {
            sig.params.push(AbiParam::new(slot));
        }
    }
    for slot in repr::slot_types(store, &ret) {
        sig.returns.push(AbiParam::new(slot));
    }
    sig
}

pub fn builtin_signature(builtin: &wsharp_runtime::Builtin, call_conv: CallConv) -> Signature {
    let mut sig = Signature::new(call_conv);
    for param in builtin.params {
        if let Some(p) = abi_of(*param) {
            sig.params.push(p);
        }
    }
    if let Some(r) = abi_of(builtin.ret) {
        sig.returns.push(r);
    }
    sig
}

fn abi_of(ty: BuiltinTy) -> Option<AbiParam> {
    Some(match ty {
        BuiltinTy::I64 => AbiParam::new(types::I64),
        BuiltinTy::F64 => AbiParam::new(types::F64),
        // C promotes narrow integer arguments, so say so explicitly.
        BuiltinTy::Bool => AbiParam::new(types::I8).uext(),
        BuiltinTy::Str => AbiParam::new(PTR),
        BuiltinTy::Void => return None,
    })
}

struct LoopCtx {
    /// Where `continue` goes: the latch, which runs the continue expression
    /// before re-testing the condition.
    continue_to: ir::Block,
    break_to: ir::Block,
}

/// Lower one function's body into `builder`.
pub fn translate(
    builder: FunctionBuilder<'_>,
    module: &mut JITModule,
    program: &hir::Program,
    store: &mut TypeStore,
    decls: &Decls,
    func: &hir::FuncDef,
) {
    let frontend_config = module.target_config();
    let call_conv = frontend_config.default_call_conv;
    let mut trans = Trans {
        b: builder,
        module,
        program,
        store,
        decls,
        func,
        call_conv,
        locals: Vec::new(),
        loops: Vec::new(),
        terminated: false,
    };
    trans.build();
    // Finalising is what releases the shared `FunctionBuilderContext` for the
    // next function; without it the next `FunctionBuilder::new` asserts.
    trans.b.finalize(frontend_config);
}

struct Trans<'a, 'f> {
    b: FunctionBuilder<'f>,
    module: &'a mut JITModule,
    program: &'a hir::Program,
    store: &'a mut TypeStore,
    decls: &'a Decls,
    func: &'a hir::FuncDef,
    call_conv: CallConv,
    /// One Cranelift variable per slot, per HIR local.
    locals: Vec<SmallVec<[Variable; 2]>>,
    loops: Vec<LoopCtx>,
    /// Whether the current block already ends in a terminator.
    terminated: bool,
}

impl Trans<'_, '_> {
    // ---- small helpers --------------------------------------------------

    fn slots_of(&mut self, ty: &Type) -> SlotTypes {
        repr::slot_types(self.store, ty)
    }

    /// Declare every heap pointer in `values` a collector root.
    ///
    /// Cranelift spills a declared value to the stack before each safepoint and
    /// reloads it afterwards, which is what puts it in the stack map -- and
    /// what lets a moving collector rewrite it while it is spilled.
    ///
    /// This is called on the result of [`Trans::expr`], which every expression
    /// flows through. Rooting only HIR locals would not be enough: a struct
    /// literal evaluates its field values *before* calling the allocator, a
    /// call evaluates its arguments left to right, and merge blocks pass
    /// pointers as block parameters -- none of those are locals, and all of
    /// them are live across a call.
    fn gc_root(&mut self, ty: &Type, values: &Slots) {
        for slot in repr::pointer_slots(self.store, ty) {
            if let Some(value) = values.get(slot) {
                self.b.declare_value_needs_stack_map(*value);
            }
        }
    }

    fn local_ty(&self, id: hir::LocalId) -> Type {
        self.func.locals[id as usize].ty.clone()
    }

    fn switch(&mut self, block: ir::Block) {
        self.b.switch_to_block(block);
        self.terminated = false;
    }

    /// Jump unless the current block already ended.
    fn jump_to(&mut self, block: ir::Block, args: &[BlockArg]) {
        if !self.terminated {
            self.b.ins().jump(block, args);
            self.terminated = true;
        }
    }

    fn brif(
        &mut self,
        cond: ir::Value,
        t: ir::Block,
        ta: &[BlockArg],
        e: ir::Block,
        ea: &[BlockArg],
    ) {
        if !self.terminated {
            self.b.ins().brif(cond, t, ta, e, ea);
            self.terminated = true;
        }
    }

    fn zero(&mut self, ty: ir::Type) -> ir::Value {
        if ty == types::F64 {
            self.b.ins().f64const(0.0)
        } else {
            self.b.ins().iconst(ty, 0)
        }
    }

    /// Placeholder values for a payload that is not present -- the null side of
    /// an optional, or the payload of an error.
    fn zeros(&mut self, tys: &[ir::Type]) -> Slots {
        tys.iter().map(|t| self.zero(*t)).collect()
    }

    fn args_of(values: &[ir::Value]) -> SmallVec<[BlockArg; 2]> {
        values.iter().map(|v| BlockArg::Value(*v)).collect()
    }

    fn func_ref(&mut self, id: hir::FuncId) -> ir::FuncRef {
        let clif = self.decls.funcs[id as usize];
        self.module.declare_func_in_func(clif, self.b.func)
    }

    fn def_local(&mut self, id: hir::LocalId, values: &[ir::Value]) {
        let vars = self.locals[id as usize].clone();
        debug_assert_eq!(
            vars.len(),
            values.len(),
            "slot count mismatch for local {id}"
        );
        for (var, value) in vars.iter().zip(values) {
            self.b.def_var(*var, *value);
        }
    }

    fn use_local(&mut self, id: hir::LocalId) -> Slots {
        let vars = self.locals[id as usize].clone();
        vars.iter().map(|v| self.b.use_var(*v)).collect()
    }

    // ---- memory ---------------------------------------------------------

    fn load_at(&mut self, obj: ir::Value, offset: u32, ty: &Type) -> Slots {
        let tys = self.slots_of(ty);
        let flags = MemFlagsData::trusted();
        tys.iter()
            .enumerate()
            .map(|(i, t)| {
                let off = (offset + i as u32 * layout::SLOT_SIZE) as i32;
                self.b.ins().load(*t, flags, obj, off)
            })
            .collect()
    }

    /// The single place generated code writes a field of a heap object, and so
    /// the single place the collector's write barrier lives.
    ///
    /// Note that the initialising stores of a struct literal are barriered too,
    /// even though the object was allocated moments earlier. It is tempting to
    /// skip them -- the object is born logged, so there is nothing to snapshot
    /// -- but `ws_alloc` is a call, hence a safepoint, and a collection there
    /// clears the logged bit before the fields are written. Skipping the
    /// barrier would then lose those references entirely, and the objects they
    /// point at would be freed while still in use.
    fn emit_store_field(&mut self, obj: ir::Value, offset: u32, ty: &Type, values: &[ir::Value]) {
        // Only a store that can create a heap-to-heap reference needs logging;
        // overwriting an `i64` field changes no one's reference count.
        if !repr::pointer_slots(self.store, ty).is_empty() {
            self.emit_log_barrier(obj);
        }
        let flags = MemFlagsData::trusted();
        for (i, value) in values.iter().enumerate() {
            let off = (offset + i as u32 * layout::SLOT_SIZE) as i32;
            self.b.ins().store(flags, *value, obj, off);
        }
    }

    /// LXR's field-logging barrier: record an object's references the first
    /// time it is modified in a collection cycle.
    ///
    /// This is coalescing reference counting (Levanoni and Petrank). Rather
    /// than adjusting counts on every store -- two atomic updates per write --
    /// the barrier snapshots an object's outgoing references once, and the
    /// collector later compares that snapshot against the object's current
    /// state. A field written a thousand times between collections costs one
    /// decrement and one increment, not a thousand of each.
    ///
    /// The snapshot doubles as the concurrent mark's snapshot-at-the-beginning
    /// barrier, since it is exactly the set of references that existed when the
    /// cycle started.
    ///
    /// Fast path: one load, one test, one not-taken branch.
    fn emit_log_barrier(&mut self, obj: ir::Value) {
        let meta = self
            .b
            .ins()
            .load(types::I64, MemFlagsData::trusted(), obj, META_OFFSET);
        let logged = self
            .b
            .ins()
            .band_imm_u(meta, (FLAG_LOGGED << FLAG_SHIFT) as i64);

        let log = self.b.create_block();
        let done = self.b.create_block();
        // Already logged -- the overwhelmingly common case -- falls straight
        // through to the store.
        self.brif(logged, done, NO_ARGS, log, NO_ARGS);

        self.switch(log);
        let func = self
            .module
            .declare_func_in_func(self.decls.log_object, self.b.func);
        self.b.ins().call(func, &[obj]);
        self.jump_to(done, NO_ARGS);

        self.switch(done);
    }

    /// A safepoint at a loop's back edge.
    ///
    /// Cranelift makes every non-tail call a safepoint and nothing else, so a
    /// loop that calls nothing -- `while (i < n) : (i += 1) { sum += i; }` --
    /// contains no point at which the collector could ever stop the program.
    /// It would run to completion no matter how badly the heap needed
    /// collecting, and a collector thread waiting for it would wait for ever.
    ///
    /// The check is a load of one byte and a not-taken branch. The byte's
    /// address is baked in as a constant, which is sound here for the same
    /// reason string literals are: the JIT resolves everything to absolute
    /// addresses in this process (`is_pic` is off).
    fn emit_gc_poll(&mut self) {
        let addr = self
            .b
            .ins()
            .iconst(PTR, wsharp_runtime::gc::poll_flag_address() as i64);
        let requested = self
            .b
            .ins()
            .load(types::I8, MemFlagsData::trusted(), addr, 0);

        let poll = self.b.create_block();
        let done = self.b.create_block();
        self.brif(requested, poll, NO_ARGS, done, NO_ARGS);

        self.switch(poll);
        let func = self
            .module
            .declare_func_in_func(self.decls.gc_poll, self.b.func);
        self.b.ins().call(func, &[]);
        self.jump_to(done, NO_ARGS);

        self.switch(done);
    }

    /// Call the runtime allocator. Every heap object in the language is born
    /// here, which is what makes the collector a drop-in replacement later.
    fn alloc(&mut self, type_id: u32, size: u32) -> ir::Value {
        let func = self
            .module
            .declare_func_in_func(self.decls.alloc, self.b.func);
        let id = self.b.ins().iconst(types::I32, type_id as i64);
        let size = self.b.ins().iconst(types::I64, align_up(size) as i64);
        let call = self.b.ins().call(func, &[id, size]);
        self.b.inst_results(call)[0]
    }

    fn panic_with(&mut self, code: i64) {
        let func = self
            .module
            .declare_func_in_func(self.decls.panic, self.b.func);
        let code = self.b.ins().iconst(types::I64, code);
        self.b.ins().call(func, &[code]);
    }

    // ---- function ------------------------------------------------------

    fn build(&mut self) {
        let entry = self.b.create_block();
        self.b.append_block_params_for_function_params(entry);
        self.switch(entry);

        for i in 0..self.func.locals.len() {
            let ty = self.func.locals[i].ty.clone();
            let slot_tys = self.slots_of(&ty);
            let pointer_slots = repr::pointer_slots(self.store, &ty);
            let mut vars: SmallVec<[Variable; 2]> = SmallVec::new();
            for (slot, slot_ty) in slot_tys.iter().enumerate() {
                let var = self.b.declare_var(*slot_ty);
                // Must be declared before the variable is ever defined.
                if pointer_slots.contains(&slot) {
                    self.b.declare_var_needs_stack_map(var);
                }
                vars.push(var);
            }
            self.locals.push(vars);
        }

        let params: Vec<ir::Value> = self.b.block_params(entry).to_vec();
        let env = params[0];
        let mut next = 1;
        for i in 0..self.func.params.len() {
            let local = self.func.params[i];
            let width = self.locals[local as usize].len();
            let values: Slots = params[next..next + width].iter().copied().collect();
            self.def_local(local, &values);
            next += width;
        }

        // Captured values are copied out of the closure environment on entry.
        let mut offset = CLOSURE_CAPTURES_OFFSET;
        for i in 0..self.func.captures.len() {
            let local = self.func.captures[i];
            let ty = self.local_ty(local);
            let values = self.load_at(env, offset, &ty);
            self.def_local(local, &values);
            offset += layout::size_of(self.store, &ty);
        }

        let body = &self.func.body;
        self.block(body);

        // Falling off the end is only legal for a `void` function -- inference
        // rejects anything else, which is also what guarantees every block here
        // ends up terminated.
        if !self.terminated {
            self.b.ins().return_(&[]);
            self.terminated = true;
        }

        self.b.seal_all_blocks();
    }

    // ---- statements -----------------------------------------------------

    fn block(&mut self, block: &hir::Block) {
        for stmt in &block.stmts {
            if self.terminated {
                // Everything after a `return`, `break` or `continue` in the same
                // block is unreachable; emitting it would append to a finished
                // Cranelift block.
                break;
            }
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &hir::Stmt) {
        match stmt {
            hir::Stmt::Let { local, init } => {
                let values = self.expr(init);
                self.def_local(*local, &values);
            }

            hir::Stmt::Assign { place, value } => {
                let values = self.expr(value);
                match place {
                    hir::Place::Local(id) => self.def_local(*id, &values),
                    hir::Place::Field {
                        obj, strukt, index, ..
                    } => {
                        let ptr = self.expr(obj)[0];
                        let field = &self.program.strukt(*strukt).fields[*index as usize];
                        let (offset, ty) = (field.offset, field.ty.clone());
                        self.emit_store_field(ptr, offset, &ty, &values);
                    }
                }
            }

            hir::Stmt::Expr(e) => {
                self.expr(e);
            }

            hir::Stmt::Return(value) => {
                let values = match value {
                    Some(e) => self.expr(e),
                    None => Slots::new(),
                };
                if !self.terminated {
                    self.b.ins().return_(&values);
                    self.terminated = true;
                }
            }

            hir::Stmt::If {
                cond,
                capture,
                then,
                els,
            } => {
                let (test, payload) = self.test_of(cond, capture.is_some());
                let then_block = self.b.create_block();
                let else_block = self.b.create_block();
                let merge = self.b.create_block();

                self.brif(test, then_block, NO_ARGS, else_block, NO_ARGS);

                self.switch(then_block);
                if let Some(local) = capture {
                    self.def_local(*local, &payload);
                }
                self.block(then);
                self.jump_to(merge, NO_ARGS);

                self.switch(else_block);
                if let Some(els) = els {
                    self.block(els);
                }
                self.jump_to(merge, NO_ARGS);

                self.switch(merge);
            }

            hir::Stmt::While {
                cond,
                capture,
                cont,
                body,
            } => {
                let header = self.b.create_block();
                let body_block = self.b.create_block();
                let latch = self.b.create_block();
                let exit = self.b.create_block();

                self.jump_to(header, NO_ARGS);

                self.switch(header);
                let (test, payload) = self.test_of(cond, capture.is_some());
                self.brif(test, body_block, NO_ARGS, exit, NO_ARGS);

                self.switch(body_block);
                if let Some(local) = capture {
                    self.def_local(*local, &payload);
                }
                self.loops.push(LoopCtx {
                    continue_to: latch,
                    break_to: exit,
                });
                self.block(body);
                self.loops.pop();
                self.jump_to(latch, NO_ARGS);

                // The continue expression runs after the body and after an
                // explicit `continue`, before the condition is retested.
                self.switch(latch);
                if let Some(cont) = cont {
                    self.stmt(cont);
                }
                self.emit_gc_poll();
                self.jump_to(header, NO_ARGS);

                self.switch(exit);
            }

            hir::Stmt::Block(b) => self.block(b),

            hir::Stmt::Break => {
                let target = self.loops.last().expect("`break` inside a loop").break_to;
                self.jump_to(target, NO_ARGS);
            }
            hir::Stmt::Continue => {
                let target = self
                    .loops
                    .last()
                    .expect("`continue` inside a loop")
                    .continue_to;
                self.jump_to(target, NO_ARGS);
            }
        }
    }

    /// Evaluate a condition into a branch test. With a capture the condition is
    /// an optional, and its payload is returned for the taken branch to bind.
    fn test_of(&mut self, cond: &hir::Expr, has_capture: bool) -> (ir::Value, Slots) {
        let values = self.expr(cond);
        if has_capture {
            let payload = values[1..].iter().copied().collect();
            (values[0], payload)
        } else {
            (values[0], Slots::new())
        }
    }

    // ---- expressions ----------------------------------------------------

    /// Lower an expression, and declare whatever heap pointers it produced as
    /// collector roots.
    ///
    /// Every expression in the language flows through here, which is what makes
    /// this the one place rooting has to be right. See [`Trans::gc_root`].
    fn expr(&mut self, expr: &hir::Expr) -> Slots {
        let values = self.expr_inner(expr);
        self.gc_root(&expr.ty, &values);
        values
    }

    fn expr_inner(&mut self, expr: &hir::Expr) -> Slots {
        match &expr.kind {
            hir::ExprKind::Int(v) => SmallVec::from_slice(&[self.b.ins().iconst(types::I64, *v)]),
            hir::ExprKind::Float(v) => SmallVec::from_slice(&[self.b.ins().f64const(*v)]),
            hir::ExprKind::Bool(v) => {
                SmallVec::from_slice(&[self.b.ins().iconst(types::I8, i64::from(*v))])
            }
            hir::ExprKind::Str(id) => {
                let data = self.decls.strings[*id as usize];
                let gv = self.module.declare_data_in_func(data, self.b.func);
                SmallVec::from_slice(&[self.b.ins().symbol_value(PTR, gv)])
            }
            hir::ExprKind::Singleton(id) => {
                // Exactly like a string literal: one static object in the data
                // section, referenced by address. No allocation.
                let data = self.decls.singletons[*id as usize]
                    .expect("only zero-field structs produce a Singleton");
                let gv = self.module.declare_data_in_func(data, self.b.func);
                SmallVec::from_slice(&[self.b.ins().symbol_value(PTR, gv)])
            }
            hir::ExprKind::Local(id) => self.use_local(*id),

            hir::ExprKind::Null => {
                let tys = self.slots_of(&expr.ty);
                let tag = self.b.ins().iconst(tys[0], OPTION_NULL);
                let mut out: Slots = SmallVec::from_slice(&[tag]);
                let payload = self.zeros(&tys[1..]);
                out.extend(payload);
                out
            }
            hir::ExprKind::Some(inner) => {
                let payload = self.expr(inner);
                let tag = self.b.ins().iconst(repr::OPTION_TAG, OPTION_SOME);
                let mut out: Slots = SmallVec::from_slice(&[tag]);
                out.extend(payload);
                out
            }
            hir::ExprKind::Ok(inner) => {
                let payload = self.expr(inner);
                let tag = self.b.ins().iconst(ERROR_TAG, ERROR_OK);
                let mut out: Slots = SmallVec::from_slice(&[tag]);
                out.extend(payload);
                out
            }
            hir::ExprKind::Err(id) => {
                let tys = self.slots_of(&expr.ty);
                let tag = self.b.ins().iconst(ERROR_TAG, error_tag_value(*id));
                let mut out: Slots = SmallVec::from_slice(&[tag]);
                let payload = self.zeros(&tys[1..]);
                out.extend(payload);
                out
            }

            hir::ExprKind::Unary { op, expr: inner } => {
                let value = self.expr(inner)[0];
                let out = match op {
                    UnOp::Neg => {
                        if self.b.func.dfg.value_type(value) == types::F64 {
                            self.b.ins().fneg(value)
                        } else {
                            self.b.ins().ineg(value)
                        }
                    }
                    // `!b` is `b == 0`, which yields the I8 a W# bool is.
                    UnOp::Not => self
                        .b
                        .ins()
                        .icmp_imm_s(ir::condcodes::IntCC::Equal, value, 0),
                };
                SmallVec::from_slice(&[out])
            }

            hir::ExprKind::Binary { op, lhs, rhs } => {
                let l = self.expr(lhs)[0];
                let r = self.expr(rhs)[0];
                let is_float = self.b.func.dfg.value_type(l) == types::F64;
                SmallVec::from_slice(&[self.binary(*op, l, r, is_float)])
            }

            hir::ExprKind::Logical { op, lhs, rhs } => self.logical(*op, lhs, rhs),

            hir::ExprKind::If {
                cond,
                capture,
                then,
                els,
            } => self.if_expr(&expr.ty, cond, *capture, then, els),

            hir::ExprKind::Call { callee, args } => self.call(&expr.ty, callee, args),

            hir::ExprKind::StructNew { strukt, fields } => self.struct_new(*strukt, fields),

            hir::ExprKind::Field {
                obj, strukt, index, ..
            } => {
                let ptr = self.expr(obj)[0];
                let field = &self.program.strukt(*strukt).fields[*index as usize];
                let (offset, ty) = (field.offset, field.ty.clone());
                self.load_at(ptr, offset, &ty)
            }

            hir::ExprKind::Closure { func, captures, .. } => self.closure(*func, captures),

            hir::ExprKind::Orelse { expr: inner, alt } => {
                // Present is a non-zero tag, so the payload can be handed to the
                // merge block straight from the branch -- no extra block needed.
                let value = self.expr(inner);
                let tag = value[0];
                let payload: Slots = value[1..].iter().copied().collect();
                self.tagged_or_else(&expr.ty, tag, true, &payload, alt, None)
            }

            hir::ExprKind::Catch {
                expr: inner,
                capture,
                alt,
            } => {
                let value = self.expr(inner);
                let tag = value[0];
                let payload: Slots = value[1..].iter().copied().collect();
                self.tagged_or_else(&expr.ty, tag, false, &payload, alt, *capture)
            }

            hir::ExprKind::Try(inner) => self.try_expr(inner),

            hir::ExprKind::Unwrap(inner) => self.unwrap(&expr.ty, inner),
        }
    }

    fn binary(&mut self, op: BinOp, l: ir::Value, r: ir::Value, is_float: bool) -> ir::Value {
        use ir::condcodes::{FloatCC, IntCC};
        if is_float {
            match op {
                BinOp::Add => self.b.ins().fadd(l, r),
                BinOp::Sub => self.b.ins().fsub(l, r),
                BinOp::Mul => self.b.ins().fmul(l, r),
                BinOp::Div => self.b.ins().fdiv(l, r),
                // Cranelift has no float remainder; inference rejects `%` on
                // floats before this point.
                BinOp::Rem => unreachable!("`%` on floats is rejected by inference"),
                BinOp::Eq => self.b.ins().fcmp(FloatCC::Equal, l, r),
                BinOp::Ne => self.b.ins().fcmp(FloatCC::NotEqual, l, r),
                BinOp::Lt => self.b.ins().fcmp(FloatCC::LessThan, l, r),
                BinOp::Le => self.b.ins().fcmp(FloatCC::LessThanOrEqual, l, r),
                BinOp::Gt => self.b.ins().fcmp(FloatCC::GreaterThan, l, r),
                BinOp::Ge => self.b.ins().fcmp(FloatCC::GreaterThanOrEqual, l, r),
                BinOp::And | BinOp::Or => unreachable!("logical operators are lowered separately"),
            }
        } else {
            match op {
                BinOp::Add => self.b.ins().iadd(l, r),
                BinOp::Sub => self.b.ins().isub(l, r),
                BinOp::Mul => self.b.ins().imul(l, r),
                BinOp::Div | BinOp::Rem => self.checked_div(op, l, r),
                BinOp::Eq => self.b.ins().icmp(IntCC::Equal, l, r),
                BinOp::Ne => self.b.ins().icmp(IntCC::NotEqual, l, r),
                BinOp::Lt => self.b.ins().icmp(IntCC::SignedLessThan, l, r),
                BinOp::Le => self.b.ins().icmp(IntCC::SignedLessThanOrEqual, l, r),
                BinOp::Gt => self.b.ins().icmp(IntCC::SignedGreaterThan, l, r),
                BinOp::Ge => self.b.ins().icmp(IntCC::SignedGreaterThanOrEqual, l, r),
                BinOp::And | BinOp::Or => unreachable!("logical operators are lowered separately"),
            }
        }
    }

    /// Integer `/` and `%`, with the two inputs the hardware cannot answer
    /// turned into panics: a zero divisor, and `i64::MIN / -1`, whose result
    /// does not fit. Cranelift's `sdiv` traps on both, but a trap is a SIGILL
    /// with no message; a panic says what happened. `srem` defines
    /// `i64::MIN % -1` as 0, so only `/` needs the second check.
    fn checked_div(&mut self, op: BinOp, l: ir::Value, r: ir::Value) -> ir::Value {
        use ir::condcodes::IntCC;
        let ok = self.b.create_block();

        let by_zero = self.b.create_block();
        let nonzero = if op == BinOp::Div {
            self.b.create_block()
        } else {
            ok
        };
        let is_zero = self.b.ins().icmp_imm_s(IntCC::Equal, r, 0);
        self.brif(is_zero, by_zero, NO_ARGS, nonzero, NO_ARGS);
        self.switch(by_zero);
        self.panic_with(PANIC_DIVIDE_BY_ZERO);
        // `ws_panic` never returns; the jump only gives the block a terminator.
        self.jump_to(ok, NO_ARGS);

        if op == BinOp::Div {
            self.switch(nonzero);
            let is_min = self.b.ins().icmp_imm_s(IntCC::Equal, l, i64::MIN);
            let is_neg_one = self.b.ins().icmp_imm_s(IntCC::Equal, r, -1);
            let overflows = self.b.ins().band(is_min, is_neg_one);
            let overflow = self.b.create_block();
            self.brif(overflows, overflow, NO_ARGS, ok, NO_ARGS);
            self.switch(overflow);
            self.panic_with(PANIC_DIVIDE_OVERFLOW);
            self.jump_to(ok, NO_ARGS);
        }

        self.switch(ok);
        if op == BinOp::Div {
            self.b.ins().sdiv(l, r)
        } else {
            self.b.ins().srem(l, r)
        }
    }

    /// `and` and `or` must not evaluate their right operand unless they have to.
    fn logical(&mut self, op: BinOp, lhs: &hir::Expr, rhs: &hir::Expr) -> Slots {
        let l = self.expr(lhs)[0];
        let rhs_block = self.b.create_block();
        let merge = self.b.create_block();
        self.b.append_block_param(merge, types::I8);

        let short = self.b.ins().iconst(types::I8, i64::from(op == BinOp::Or));
        let short_args = [BlockArg::Value(short)];
        match op {
            BinOp::And => self.brif(l, rhs_block, NO_ARGS, merge, &short_args),
            _ => self.brif(l, merge, &short_args, rhs_block, NO_ARGS),
        }

        self.switch(rhs_block);
        let r = self.expr(rhs)[0];
        let args = Self::args_of(&[r]);
        self.jump_to(merge, &args);

        self.switch(merge);
        SmallVec::from_slice(&[self.b.block_params(merge)[0]])
    }

    fn if_expr(
        &mut self,
        ty: &Type,
        cond: &hir::Expr,
        capture: Option<hir::LocalId>,
        then: &hir::Expr,
        els: &hir::Expr,
    ) -> Slots {
        let (test, payload) = self.test_of(cond, capture.is_some());
        let then_block = self.b.create_block();
        let else_block = self.b.create_block();
        let merge = self.b.create_block();
        for slot in self.slots_of(ty) {
            self.b.append_block_param(merge, slot);
        }

        self.brif(test, then_block, NO_ARGS, else_block, NO_ARGS);

        self.switch(then_block);
        if let Some(local) = capture {
            self.def_local(local, &payload);
        }
        let value = self.expr(then);
        let args = Self::args_of(&value);
        self.jump_to(merge, &args);

        self.switch(else_block);
        let value = self.expr(els);
        let args = Self::args_of(&value);
        self.jump_to(merge, &args);

        self.switch(merge);
        self.b.block_params(merge).iter().copied().collect()
    }

    /// The shared shape of `orelse` and `catch`: a tag decides between using the
    /// payload already in hand and evaluating a fallback.
    ///
    /// `taken_when_nonzero` distinguishes them -- an optional is present when
    /// its tag is non-zero, an error union succeeded when its tag is zero.
    fn tagged_or_else(
        &mut self,
        ty: &Type,
        tag: ir::Value,
        taken_when_nonzero: bool,
        payload: &[ir::Value],
        alt: &hir::Expr,
        capture: Option<hir::LocalId>,
    ) -> Slots {
        let alt_block = self.b.create_block();
        let merge = self.b.create_block();
        for slot in self.slots_of(ty) {
            self.b.append_block_param(merge, slot);
        }

        let payload_args = Self::args_of(payload);
        if taken_when_nonzero {
            self.brif(tag, merge, &payload_args, alt_block, NO_ARGS);
        } else {
            self.brif(tag, alt_block, NO_ARGS, merge, &payload_args);
        }

        self.switch(alt_block);
        if let Some(local) = capture {
            // `catch |e|` binds the error, which is the tag less the offset that
            // keeps zero meaning success.
            let code = self.b.ins().iadd_imm_s(tag, -1);
            self.def_local(local, &[code]);
        }
        let value = self.expr(alt);
        let args = Self::args_of(&value);
        self.jump_to(merge, &args);

        self.switch(merge);
        self.b.block_params(merge).iter().copied().collect()
    }

    /// `try e` -- yield the payload, or return the error from this function.
    fn try_expr(&mut self, inner: &hir::Expr) -> Slots {
        let value = self.expr(inner);
        let tag = value[0];
        let payload: Slots = value[1..].iter().copied().collect();

        let err_block = self.b.create_block();
        let ok_block = self.b.create_block();
        for v in &payload {
            let ty = self.b.func.dfg.value_type(*v);
            self.b.append_block_param(ok_block, ty);
        }

        let ok_args = Self::args_of(&payload);
        self.brif(tag, err_block, NO_ARGS, ok_block, &ok_args);

        // Propagate: rebuild this function's return value carrying the same tag.
        self.switch(err_block);
        let ret_tys = self.slots_of(&self.func.ret.clone());
        let mut ret: Slots = SmallVec::from_slice(&[tag]);
        let rest = self.zeros(&ret_tys[1..]);
        ret.extend(rest);
        self.b.ins().return_(&ret);
        self.terminated = true;

        self.switch(ok_block);
        self.b.block_params(ok_block).iter().copied().collect()
    }

    /// `e.?` -- yield the payload, or abort.
    fn unwrap(&mut self, ty: &Type, inner: &hir::Expr) -> Slots {
        let value = self.expr(inner);
        let tag = value[0];
        let payload: Slots = value[1..].iter().copied().collect();

        let null_block = self.b.create_block();
        let ok_block = self.b.create_block();
        for slot in self.slots_of(ty) {
            self.b.append_block_param(ok_block, slot);
        }

        let ok_args = Self::args_of(&payload);
        self.brif(tag, ok_block, &ok_args, null_block, NO_ARGS);

        self.switch(null_block);
        self.panic_with(PANIC_UNWRAP_NULL);
        // `ws_panic` aborts, so this jump never runs; it exists only to give
        // the block a terminator without needing a trap opcode.
        let tys = self.slots_of(ty);
        let dead = self.zeros(&tys);
        let dead_args = Self::args_of(&dead);
        self.jump_to(ok_block, &dead_args);

        self.switch(ok_block);
        self.b.block_params(ok_block).iter().copied().collect()
    }

    fn call(&mut self, ty: &Type, callee: &hir::Callee, args: &[hir::Expr]) -> Slots {
        match callee {
            hir::Callee::Static { func, .. } => {
                let fr = self.func_ref(*func);
                // Top-level functions ignore the environment pointer.
                let null_env = self.b.ins().iconst(PTR, 0);
                let mut values = vec![null_env];
                for arg in args {
                    values.extend(self.expr(arg));
                }
                let call = self.b.ins().call(fr, &values);
                self.b.inst_results(call).iter().copied().collect()
            }

            hir::Callee::Builtin(id) => {
                let clif = self.decls.builtins[*id as usize];
                let fr = self.module.declare_func_in_func(clif, self.b.func);
                let mut values = Vec::new();
                for arg in args {
                    values.extend(self.expr(arg));
                }
                let call = self.b.ins().call(fr, &values);
                self.b.inst_results(call).iter().copied().collect()
            }

            hir::Callee::Indirect(target) => {
                let fn_ty = target.ty.clone();
                let closure = self.expr(target)[0];

                // Arguments first, then the code pointer. An argument may
                // call, a call may collect, and a collection may move the
                // closure -- so a code pointer loaded before them could be read
                // out of an object that no longer lives there. `closure` itself
                // is a root and is updated in place if it moves; the loaded
                // word would not be.
                let mut values = vec![closure];
                for arg in args {
                    values.extend(self.expr(arg));
                }

                let code = self.b.ins().load(
                    PTR,
                    MemFlagsData::trusted(),
                    closure,
                    CLOSURE_CODE_OFFSET as i32,
                );
                let sig = indirect_signature(self.store, &fn_ty, self.call_conv);
                let sig_ref = self.b.import_signature(sig);
                let call = self.b.ins().call_indirect(sig_ref, code, &values);
                let _ = ty;
                self.b.inst_results(call).iter().copied().collect()
            }

            hir::Callee::Dynamic { cases } => self.dispatch(ty, cases, args),
        }
    }

    /// Multiple dispatch: pick the overload by the arguments' runtime types.
    ///
    /// `cases` arrive most specific first, so a first-match chain *is* Julia's
    /// specificity rule -- all the reasoning happened in inference. Because
    /// type ids are assigned in a preorder walk of the lattice, "is this a
    /// subtype of T?" is one subtract and one unsigned compare against T's
    /// subtree range, which is cheaper than an inline cache would be to probe.
    fn dispatch(&mut self, ty: &Type, cases: &[hir::DispatchCase], args: &[hir::Expr]) -> Slots {
        // Arguments are evaluated once, before any branching.
        let arg_slots: Vec<Slots> = args.iter().map(|a| self.expr(a)).collect();

        // One type-id load per dispatched position, shared by every case.
        let mut type_ids: Vec<Option<ir::Value>> = vec![None; args.len()];
        for case in cases {
            for (i, test) in case.params.iter().enumerate() {
                if test.is_some() && type_ids[i].is_none() {
                    let obj = arg_slots[i][0];
                    let meta =
                        self.b
                            .ins()
                            .load(types::I64, MemFlagsData::trusted(), obj, META_OFFSET);
                    type_ids[i] = Some(self.b.ins().band_imm_u(meta, TYPE_ID_MASK as i64));
                }
            }
        }

        let done = self.b.create_block();
        for slot in self.slots_of(ty) {
            self.b.append_block_param(done, slot);
        }

        let mut flat_args = vec![self.b.ins().iconst(PTR, 0)];
        for slots in &arg_slots {
            flat_args.extend(slots.iter().copied());
        }

        for case in cases {
            match self.case_matches(case, &type_ids) {
                Some(cond) => {
                    let body = self.b.create_block();
                    let next = self.b.create_block();
                    self.brif(cond, body, NO_ARGS, next, NO_ARGS);
                    self.switch(body);
                    self.emit_case_call(case, &flat_args, done);
                    self.switch(next);
                }
                None => {
                    // No test: this case applies to every value that reaches
                    // here, so it always wins and the chain ends. Emit the call
                    // in place rather than in a block of its own.
                    self.emit_case_call(case, &flat_args, done);
                    self.switch(done);
                    return self.b.block_params(done).iter().copied().collect();
                }
            }
        }

        // Every case was guarded and none matched. Inference proves this
        // unreachable whenever the lattice has a catch-all overload; when it
        // cannot, this is where the program finds out.
        self.panic_with(PANIC_NO_METHOD);
        let tys = self.slots_of(ty);
        let dead = self.zeros(&tys);
        let dead_args = Self::args_of(&dead);
        self.jump_to(done, &dead_args);

        self.switch(done);
        self.b.block_params(done).iter().copied().collect()
    }

    /// The condition under which `case` applies, or `None` if it always does.
    fn case_matches(
        &mut self,
        case: &hir::DispatchCase,
        type_ids: &[Option<ir::Value>],
    ) -> Option<ir::Value> {
        let mut cond: Option<ir::Value> = None;
        for (i, test) in case.params.iter().enumerate() {
            let Some(strukt) = *test else { continue };
            let def = self.program.strukt(strukt);
            let (start, len) = (def.type_id, def.subtree_len);
            let id = type_ids[i].expect("a tested position has its type id loaded");
            let rel = self.b.ins().iadd_imm_s(id, -(start as i64));
            let hit =
                self.b
                    .ins()
                    .icmp_imm_u(ir::condcodes::IntCC::UnsignedLessThan, rel, len as i64);
            cond = Some(match cond {
                None => hit,
                // Both operands are pure comparisons, so `and`-ing beats
                // chaining branches: no extra blocks, no extra jumps.
                Some(prev) => self.b.ins().band(prev, hit),
            });
        }
        cond
    }

    fn emit_case_call(&mut self, case: &hir::DispatchCase, args: &[ir::Value], done: ir::Block) {
        let fr = self.func_ref(case.func);
        let call = self.b.ins().call(fr, args);
        let results: Slots = self.b.inst_results(call).iter().copied().collect();
        let block_args = Self::args_of(&results);
        self.jump_to(done, &block_args);
    }

    fn struct_new(&mut self, strukt: StructId, fields: &[hir::Expr]) -> Slots {
        // Evaluate the field values first, so nothing can allocate while a
        // partly initialised object is live.
        let values: Vec<Slots> = fields.iter().map(|f| self.expr(f)).collect();

        let def = self.program.strukt(strukt);
        let (type_id, size) = (def.type_id, def.size);
        let layouts: Vec<(u32, Type)> = def
            .fields
            .iter()
            .map(|f| (f.offset, f.ty.clone()))
            .collect();

        let ptr = self.alloc(type_id, size);
        for ((offset, ty), value) in layouts.iter().zip(&values) {
            self.emit_store_field(ptr, *offset, ty, value);
        }
        SmallVec::from_slice(&[ptr])
    }

    fn closure(&mut self, func: hir::FuncId, captures: &[hir::Expr]) -> Slots {
        let values: Vec<Slots> = captures.iter().map(|c| self.expr(c)).collect();
        let types: Vec<Type> = captures.iter().map(|c| c.ty.clone()).collect();

        // Size comes from the target's own capture list, so it always matches
        // the layout registered with the runtime for this closure's type id.
        let program = self.program;
        let (size, _) = closure_layout(self.store, program.func(func));
        let type_id = self.decls.closure_type_ids[func as usize];
        debug_assert_ne!(type_id, TYPE_ID_CLOSURE, "closures get a per-site type id");

        let ptr = self.alloc(type_id, size);
        let fr = self.func_ref(func);
        let code = self.b.ins().func_addr(PTR, fr);
        self.b.ins().store(
            MemFlagsData::trusted(),
            code,
            ptr,
            CLOSURE_CODE_OFFSET as i32,
        );

        let mut offset = CLOSURE_CAPTURES_OFFSET;
        for (ty, value) in types.iter().zip(&values) {
            self.emit_store_field(ptr, offset, ty, value);
            offset += layout::size_of(self.store, ty);
        }
        SmallVec::from_slice(&[ptr])
    }
}

/// Whether a function returns nothing, so its call sites expect no values.
pub fn returns_nothing(store: &mut TypeStore, ty: &Type) -> bool {
    matches!(store.resolve(ty), Type::Con(TyCon::Void, _))
}
