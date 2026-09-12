//! C ABI thunks. Each native signature gets an adapter from scalar slots.
//! The runtime invokes it inside a safe region, retaining the library until C
//! returns. A thunk never loads or allocates a W# heap reference.

use crate::{CodegenError, repr::PTR};
use cranelift_codegen::ir::{self, AbiParam, InstBuilder, MemFlagsData, Signature};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{FuncId, Linkage, Module};
use std::collections::HashMap;
use wsharp_sema::{
    hir,
    ty::{TyCon, Type},
};

fn foreign(func: &hir::FuncDef) -> bool {
    matches!(
        func.body.stmts.as_slice(),
        [hir::Stmt::Return(Some(hir::Expr {
            kind: hir::ExprKind::Call {
                callee: hir::Callee::Foreign,
                ..
            },
            ..
        }))]
    )
}

pub(crate) fn scalar(ty: &Type) -> AbiParam {
    if let Some(int) = ty.as_int() {
        let param = AbiParam::new(ir::Type::int(int.bits as u16).unwrap());
        return if int.bits < 64 {
            if int.signed {
                param.sext()
            } else {
                param.uext()
            }
        } else {
            param
        };
    }
    match ty {
        Type::Con(TyCon::Bool, _) => AbiParam::new(ir::types::I8).uext(),
        Type::Con(TyCon::F64, _) => AbiParam::new(ir::types::F64),
        _ => unreachable!("FFI types were validated by monomorphisation"),
    }
}

pub(crate) fn define<M: Module>(
    module: &mut M,
    program: &hir::Program,
) -> Result<HashMap<String, FuncId>, CodegenError> {
    let mut result = HashMap::new();
    let cc = module.target_config().default_call_conv;
    for func in program.funcs.iter().filter(|f| foreign(f)) {
        let mut adapter = Signature::new(cc);
        adapter
            .params
            .extend([AbiParam::new(PTR), AbiParam::new(PTR)]);
        let id = module
            .declare_function(&format!("{}_c", func.name), Linkage::Local, &adapter)
            .map_err(|e| crate::err("could not declare C thunk", e))?;
        let mut ctx = module.make_context();
        ctx.func.signature = adapter;
        ctx.func.name = ir::UserFuncName::user(0, id.as_u32());
        let mut fb_ctx = FunctionBuilderContext::new();
        {
            let mut b = FunctionBuilder::new(&mut ctx.func, &mut fb_ctx);
            let entry = b.create_block();
            b.append_block_params_for_function_params(entry);
            b.switch_to_block(entry);
            let address = b.block_params(entry)[0];
            let data = b.block_params(entry)[1];
            let mut native = Signature::new(cc);
            let mut args = Vec::new();
            for (i, &local) in func.params.iter().enumerate() {
                let param = scalar(&func.locals[local as usize].ty);
                args.push(b.ins().load(
                    param.value_type,
                    MemFlagsData::trusted(),
                    data,
                    (i * 8) as i32,
                ));
                native.params.push(param);
            }
            let returns = func.ret != Type::void();
            if returns {
                native.returns.push(scalar(&func.ret));
            }
            let signature = b.import_signature(native);
            let call = b.ins().call_indirect(signature, address, &args);
            if returns {
                let mut value = b.inst_results(call)[0];
                if func.ret == Type::bool() {
                    value = b.ins().icmp_imm_s(ir::condcodes::IntCC::NotEqual, value, 0);
                }
                b.ins().store(
                    MemFlagsData::trusted(),
                    value,
                    data,
                    (func.params.len() * 8) as i32,
                );
            }
            b.ins().return_(&[]);
            b.seal_all_blocks();
            b.finalize(module.target_config());
        }
        module
            .define_function(id, &mut ctx)
            .map_err(|e| crate::err("could not compile C thunk", e))?;
        result.insert(func.name.clone(), id);
    }
    Ok(result)
}
