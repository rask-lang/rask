// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function builder — lowers MIR to Cranelift IR.

use cranelift::prelude::*;
use cranelift_codegen::ir::{Function, InstBuilder};
use cranelift_frontend::{FunctionBuilder as ClifFunctionBuilder, FunctionBuilderContext};
use std::collections::HashMap;

use rask_mir::{BlockId, LocalId, MirConst, MirFunction, MirOperand, MirStmt, MirTerminator};
use crate::types::mir_to_cranelift_type;
use crate::{CodegenError, CodegenResult};

pub struct FunctionBuilder<'a> {
    func: &'a mut Function,
    builder_ctx: FunctionBuilderContext,
    mir_fn: &'a MirFunction,

    /// Map MIR block IDs to Cranelift blocks
    block_map: HashMap<BlockId, Block>,
    /// Map MIR locals to Cranelift variables
    var_map: HashMap<LocalId, Variable>,
}

impl<'a> FunctionBuilder<'a> {
    pub fn new(func: &'a mut Function, mir_fn: &'a MirFunction) -> CodegenResult<Self> {
        Ok(FunctionBuilder {
            func,
            builder_ctx: FunctionBuilderContext::new(),
            mir_fn,
            block_map: HashMap::new(),
            var_map: HashMap::new(),
        })
    }

    /// Build the Cranelift IR from MIR.
    pub fn build(&mut self) -> CodegenResult<()> {
        let mut builder = ClifFunctionBuilder::new(self.func, &mut self.builder_ctx);

        // Create all blocks first
        for mir_block in &self.mir_fn.blocks {
            let block = builder.create_block();
            self.block_map.insert(mir_block.id, block);
        }

        // Declare all variables (locals)
        for (idx, local) in self.mir_fn.locals.iter().enumerate() {
            let var = Variable::new(idx);
            let ty = mir_to_cranelift_type(&local.ty)?;
            builder.declare_var(var, ty);
            self.var_map.insert(local.id, var);
        }

        // Entry block
        let entry_block = self.block_map.get(&self.mir_fn.entry_block)
            .ok_or_else(|| CodegenError::UnsupportedFeature("Entry block not found".to_string()))?;
        builder.switch_to_block(*entry_block);
        builder.seal_block(*entry_block);

        // Lower each block
        for mir_block in &self.mir_fn.blocks {
            let cl_block = self.block_map[&mir_block.id];

            if mir_block.id != self.mir_fn.entry_block {
                builder.switch_to_block(cl_block);
            }

            // Lower statements
            for stmt in &mir_block.statements {
                Self::lower_stmt(&mut builder, stmt, &self.var_map)?;
            }

            // Lower terminator
            Self::lower_terminator(&mut builder, &mir_block.terminator, &self.var_map, &self.block_map)?;

            if mir_block.id != self.mir_fn.entry_block {
                builder.seal_block(cl_block);
            }
        }

        builder.finalize();
        Ok(())
    }

    fn lower_stmt(
        builder: &mut ClifFunctionBuilder,
        stmt: &MirStmt,
        var_map: &HashMap<LocalId, Variable>,
    ) -> CodegenResult<()> {
        match stmt {
            MirStmt::Assign { dst, rvalue } => {
                // For now, just handle simple constants
                // TODO: Implement full rvalue lowering
                if let rask_mir::MirRValue::Use(op) = rvalue {
                    let val = Self::lower_operand(builder, op, var_map)?;
                    let var = var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                    builder.def_var(*var, val);
                }
            }

            _ => {
                // TODO: Implement other statement types
                return Err(CodegenError::UnsupportedFeature(format!("Statement not implemented: {:?}", stmt)));
            }
        }
        Ok(())
    }

    fn lower_terminator(
        builder: &mut ClifFunctionBuilder,
        term: &MirTerminator,
        var_map: &HashMap<LocalId, Variable>,
        block_map: &HashMap<BlockId, Block>,
    ) -> CodegenResult<()> {
        match term {
            MirTerminator::Return { value } => {
                if let Some(val_op) = value {
                    let val = Self::lower_operand(builder, val_op, var_map)?;
                    builder.ins().return_(&[val]);
                } else {
                    builder.ins().return_(&[]);
                }
            }

            MirTerminator::Goto { target } => {
                let target_block = block_map.get(target)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Target block not found".to_string()))?;
                builder.ins().jump(*target_block, &[]);
            }

            MirTerminator::Branch { cond, then_block, else_block } => {
                let cond_val = Self::lower_operand(builder, cond, var_map)?;
                let then_cl = block_map.get(then_block)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Then block not found".to_string()))?;
                let else_cl = block_map.get(else_block)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Else block not found".to_string()))?;
                builder.ins().brif(cond_val, *then_cl, &[], *else_cl, &[]);
            }

            _ => {
                return Err(CodegenError::UnsupportedFeature(format!("Terminator not implemented: {:?}", term)));
            }
        }
        Ok(())
    }

    fn lower_operand(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        var_map: &HashMap<LocalId, Variable>,
    ) -> CodegenResult<Value> {
        match op {
            MirOperand::Local(local_id) => {
                let var = var_map.get(local_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Local not found".to_string()))?;
                Ok(builder.use_var(*var))
            }

            MirOperand::Constant(const_val) => {
                // For now, just handle i32 constants
                // TODO: Implement other constant types
                match const_val {
                    MirConst::Int(n) => {
                        Ok(builder.ins().iconst(types::I32, *n as i64))
                    }
                    _ => Err(CodegenError::UnsupportedFeature("Constant type not implemented".to_string())),
                }
            }
        }
    }
}
