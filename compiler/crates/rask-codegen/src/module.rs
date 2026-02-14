// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Cranelift module setup and code generation orchestration.

use cranelift::prelude::*;
use cranelift_module::{Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use std::collections::HashMap;

use rask_mir::MirFunction;
use rask_mono::MonoProgram;
use crate::builder::FunctionBuilder;
use crate::types::mir_to_cranelift_type;
use crate::{CodegenError, CodegenResult};

pub struct CodeGenerator {
    module: ObjectModule,
    ctx: codegen::Context,
    func_ids: HashMap<String, cranelift_module::FuncId>,
}

impl CodeGenerator {
    pub fn new() -> CodegenResult<Self> {
        let isa_builder = cranelift_native::builder()
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
        let isa = isa_builder.finish(settings::Flags::new(settings::builder()))
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        let builder = ObjectBuilder::new(
            isa,
            "rask_module",
            cranelift_module::default_libcall_names(),
        ).map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        let module = ObjectModule::new(builder);

        Ok(CodeGenerator {
            module,
            ctx: codegen::Context::new(),
            func_ids: HashMap::new(),
        })
    }

    /// Declare all functions first (for forward references).
    pub fn declare_functions(&mut self, _mono: &MonoProgram, mir_functions: &[MirFunction]) -> CodegenResult<()> {
        for mir_fn in mir_functions {
            let mut sig = self.module.make_signature();

            // Build parameter list
            for param in &mir_fn.params {
                let param_ty = mir_to_cranelift_type(&param.ty)?;
                sig.params.push(AbiParam::new(param_ty));
            }

            // Build return type
            let ret_ty = mir_to_cranelift_type(&mir_fn.ret_ty)?;
            if !matches!(mir_fn.ret_ty, rask_mir::MirType::Void) {
                sig.returns.push(AbiParam::new(ret_ty));
            }

            let func_id = self.module
                .declare_function(&mir_fn.name, Linkage::Export, &sig)
                .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

            self.func_ids.insert(mir_fn.name.clone(), func_id);
        }
        Ok(())
    }

    /// Generate code for a single MIR function.
    pub fn gen_function(&mut self, mir_fn: &MirFunction) -> CodegenResult<()> {
        let func_id = self.func_ids.get(&mir_fn.name)
            .ok_or_else(|| CodegenError::FunctionNotFound(mir_fn.name.clone()))?;

        self.ctx.clear();

        // Build the signature (must match declaration)
        let mut sig = self.module.make_signature();
        for param in &mir_fn.params {
            let param_ty = mir_to_cranelift_type(&param.ty)?;
            sig.params.push(AbiParam::new(param_ty));
        }
        let ret_ty = mir_to_cranelift_type(&mir_fn.ret_ty)?;
        if !matches!(mir_fn.ret_ty, rask_mir::MirType::Void) {
            sig.returns.push(AbiParam::new(ret_ty));
        }
        self.ctx.func.signature = sig;

        // Build the function
        let mut builder = FunctionBuilder::new(&mut self.ctx.func, mir_fn, &self.func_ids)?;
        builder.build()?;

        // Define the function in the module
        self.module
            .define_function(*func_id, &mut self.ctx)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        Ok(())
    }

    /// Emit the final object file. Consumes self because finish() takes ownership.
    pub fn emit_object(self, path: &str) -> CodegenResult<()> {
        let product = self.module.finish();
        let bytes = product.emit()
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        std::fs::write(path, bytes)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;

        Ok(())
    }
}
