// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function builder — lowers MIR to Cranelift IR.

use cranelift::prelude::*;
use cranelift_codegen::ir::{Function, InstBuilder, MemFlags};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_frontend::{FunctionBuilder as ClifFunctionBuilder, FunctionBuilderContext};
use std::collections::HashMap;

use rask_mir::{BinOp, BlockId, LocalId, MirConst, MirFunction, MirOperand, MirStmt, MirTerminator, UnaryOp};
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

        // Entry block - add parameters as block params
        let entry_block = self.block_map.get(&self.mir_fn.entry_block)
            .ok_or_else(|| CodegenError::UnsupportedFeature("Entry block not found".to_string()))?;
        builder.switch_to_block(*entry_block);

        // Append parameters to entry block and bind to variables
        for param in &self.mir_fn.params {
            let param_ty = mir_to_cranelift_type(&param.ty)?;
            let block_param = builder.append_block_param(*entry_block, param_ty);
            let var = self.var_map.get(&param.id)
                .ok_or_else(|| CodegenError::UnsupportedFeature("Parameter variable not found".to_string()))?;
            builder.def_var(*var, block_param);
        }

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
                let val = Self::lower_rvalue(builder, rvalue, var_map)?;
                let var = var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                builder.def_var(*var, val);
            }

            MirStmt::Store { addr, offset, value } => {
                let addr_val = builder.use_var(*var_map.get(addr)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Address variable not found".to_string()))?);
                let val = Self::lower_operand(builder, value, var_map)?;

                // Store with offset
                let flags = MemFlags::new();
                builder.ins().store(flags, val, addr_val, *offset as i32);
            }

            MirStmt::Call { dst, func, args } => {
                // For now, stub out calls - we'll implement this after we have runtime functions
                let _ = (dst, func, args);
                return Err(CodegenError::UnsupportedFeature("Call not yet implemented".to_string()));
            }

            _ => {
                // TODO: Implement other statement types (resource tracking, etc.)
                return Err(CodegenError::UnsupportedFeature(format!("Statement not implemented: {:?}", stmt)));
            }
        }
        Ok(())
    }

    fn lower_rvalue(
        builder: &mut ClifFunctionBuilder,
        rvalue: &rask_mir::MirRValue,
        var_map: &HashMap<LocalId, Variable>,
    ) -> CodegenResult<Value> {
        match rvalue {
            rask_mir::MirRValue::Use(op) => {
                Self::lower_operand(builder, op, var_map)
            }

            rask_mir::MirRValue::BinaryOp { op, left, right } => {
                let lhs_val = Self::lower_operand(builder, left, var_map)?;
                let rhs_val = Self::lower_operand(builder, right, var_map)?;

                let result = match op {
                    BinOp::Add => builder.ins().iadd(lhs_val, rhs_val),
                    BinOp::Sub => builder.ins().isub(lhs_val, rhs_val),
                    BinOp::Mul => builder.ins().imul(lhs_val, rhs_val),
                    BinOp::Div => builder.ins().sdiv(lhs_val, rhs_val),
                    BinOp::Mod => builder.ins().srem(lhs_val, rhs_val),
                    BinOp::BitAnd => builder.ins().band(lhs_val, rhs_val),
                    BinOp::BitOr => builder.ins().bor(lhs_val, rhs_val),
                    BinOp::BitXor => builder.ins().bxor(lhs_val, rhs_val),
                    BinOp::Shl => builder.ins().ishl(lhs_val, rhs_val),
                    BinOp::Shr => builder.ins().sshr(lhs_val, rhs_val),
                    BinOp::Eq => builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val),
                    BinOp::Ne => builder.ins().icmp(IntCC::NotEqual, lhs_val, rhs_val),
                    BinOp::Lt => builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val),
                    BinOp::Le => builder.ins().icmp(IntCC::SignedLessThanOrEqual, lhs_val, rhs_val),
                    BinOp::Gt => builder.ins().icmp(IntCC::SignedGreaterThan, lhs_val, rhs_val),
                    BinOp::Ge => builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, lhs_val, rhs_val),
                    BinOp::And | BinOp::Or => return Err(CodegenError::UnsupportedFeature(format!("Logical op {:?} should be lowered to branches in MIR", op))),
                };
                Ok(result)
            }

            rask_mir::MirRValue::UnaryOp { op, operand } => {
                let val = Self::lower_operand(builder, operand, var_map)?;

                let result = match op {
                    UnaryOp::Neg => builder.ins().ineg(val),
                    UnaryOp::Not => builder.ins().bnot(val),
                    UnaryOp::BitNot => builder.ins().bnot(val),
                };
                Ok(result)
            }

            _ => {
                Err(CodegenError::UnsupportedFeature(format!("RValue not implemented: {:?}", rvalue)))
            }
        }
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

            MirTerminator::Switch { value, cases, default } => {
                let scrutinee_val = Self::lower_operand(builder, value, var_map)?;

                // Build switch using br_table for dense cases, or chain of branches for sparse
                // For now, use simple branch chain
                let mut current_block = builder.current_block().unwrap();

                for (value, target_id) in cases {
                    let target_block = block_map.get(target_id)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Switch target block not found".to_string()))?;

                    let cmp_val = builder.ins().iconst(types::I64, *value as i64);
                    let cond = builder.ins().icmp(IntCC::Equal, scrutinee_val, cmp_val);

                    // Create a block for the next comparison
                    let next_block = builder.create_block();

                    builder.ins().brif(cond, *target_block, &[], next_block, &[]);
                    builder.switch_to_block(next_block);
                    builder.seal_block(current_block);
                    current_block = next_block;
                }

                // Default case
                let default_block = block_map.get(default)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Switch default block not found".to_string()))?;
                builder.ins().jump(*default_block, &[]);
                builder.seal_block(current_block);
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
