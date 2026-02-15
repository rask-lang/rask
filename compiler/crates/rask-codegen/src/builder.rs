// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function builder — lowers MIR to Cranelift IR.

use cranelift::prelude::*;
use cranelift_codegen::ir::{FuncRef, Function, GlobalValue, InstBuilder, MemFlags, StackSlotData, StackSlotKind};
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_frontend::{FunctionBuilder as ClifFunctionBuilder, FunctionBuilderContext};
use std::collections::{HashMap, HashSet};

use rask_mir::{BinOp, BlockId, LocalId, MirConst, MirFunction, MirOperand, MirRValue, MirStmt, MirTerminator, MirType, UnaryOp};
use rask_mono::{StructLayout, EnumLayout};
use crate::types::mir_to_cranelift_type;
use crate::{CodegenError, CodegenResult};

pub struct FunctionBuilder<'a> {
    func: &'a mut Function,
    builder_ctx: FunctionBuilderContext,
    mir_fn: &'a MirFunction,
    /// Pre-imported function references (MIR name → Cranelift FuncRef)
    func_refs: &'a HashMap<String, FuncRef>,
    /// Struct layouts from monomorphization
    struct_layouts: &'a [StructLayout],
    /// Enum layouts from monomorphization
    enum_layouts: &'a [EnumLayout],
    /// String literal data (content → GlobalValue for the data address)
    string_globals: &'a HashMap<String, GlobalValue>,

    /// Map MIR block IDs to Cranelift blocks
    block_map: HashMap<BlockId, Block>,
    /// Map MIR locals to Cranelift variables
    var_map: HashMap<LocalId, Variable>,
}

impl<'a> FunctionBuilder<'a> {
    pub fn new(
        func: &'a mut Function,
        mir_fn: &'a MirFunction,
        func_refs: &'a HashMap<String, FuncRef>,
        struct_layouts: &'a [StructLayout],
        enum_layouts: &'a [EnumLayout],
        string_globals: &'a HashMap<String, GlobalValue>,
    ) -> CodegenResult<Self> {
        Ok(FunctionBuilder {
            func,
            builder_ctx: FunctionBuilderContext::new(),
            mir_fn,
            func_refs,
            struct_layouts,
            enum_layouts,
            string_globals,
            block_map: HashMap::new(),
            var_map: HashMap::new(),
        })
    }

    /// Build the Cranelift IR from MIR.
    pub fn build(&mut self) -> CodegenResult<()> {
        // Pre-compute stack allocation sizes before builder borrows self.func.
        // Entries: (local_id, byte size) for each aggregate local.
        let stack_allocs: Vec<(LocalId, u32)> = self.mir_fn.locals.iter()
            .filter(|l| !l.is_param)
            .filter_map(|l| {
                let size = match &l.ty {
                    MirType::Struct(id) => self.struct_layouts.get(id.0 as usize).map(|sl| sl.size),
                    MirType::Enum(id) => self.enum_layouts.get(id.0 as usize).map(|el| el.size),
                    MirType::Array { elem, len } => Some(crate::types::mir_type_size(elem) * len),
                    _ => None,
                };
                size.filter(|&s| s > 0).map(|s| (l.id, s))
            })
            .collect();

        // Collect cleanup-only blocks (appear in CleanupReturn chains).
        // These are inlined at the CleanupReturn site, not lowered as
        // standalone Cranelift blocks.
        let cleanup_only: HashSet<BlockId> = self.mir_fn.blocks.iter()
            .filter_map(|b| {
                if let MirTerminator::CleanupReturn { cleanup_chain, .. } = &b.terminator {
                    Some(cleanup_chain.iter().copied())
                } else {
                    None
                }
            })
            .flatten()
            .collect();

        let mut builder = ClifFunctionBuilder::new(self.func, &mut self.builder_ctx);

        // Create blocks (skip cleanup-only blocks — their code is inlined)
        for mir_block in &self.mir_fn.blocks {
            if cleanup_only.contains(&mir_block.id) {
                continue;
            }
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

        // Allocate stack slots for aggregate locals (structs, enums, arrays).
        // These types are represented as pointers (i64) — the variable holds
        // the address of the stack-allocated storage.
        for (local_id, size) in &stack_allocs {
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                *size,
                0, // align_shift: natural alignment
            ));
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            let var = self.var_map[local_id];
            builder.def_var(var, addr);
        }

        // Lower each block (skip cleanup-only blocks)
        for mir_block in &self.mir_fn.blocks {
            if cleanup_only.contains(&mir_block.id) {
                continue;
            }

            let cl_block = self.block_map[&mir_block.id];

            if mir_block.id != self.mir_fn.entry_block {
                builder.switch_to_block(cl_block);
            }

            // Lower statements
            for stmt in &mir_block.statements {
                Self::lower_stmt(
                    &mut builder, stmt, &self.var_map, &self.mir_fn.locals,
                    self.func_refs, self.struct_layouts, self.enum_layouts,
                    self.string_globals,
                )?;
            }

            // Lower terminator
            Self::lower_terminator(
                &mut builder, &mir_block.terminator, &self.var_map,
                &self.block_map, &self.mir_fn.ret_ty,
                &self.mir_fn.blocks, &self.mir_fn.locals,
                self.func_refs, self.struct_layouts, self.enum_layouts,
                self.string_globals,
            )?;
        }

        // Now seal all blocks (all predecessors are known)
        for mir_block in &self.mir_fn.blocks {
            if let Some(&cl_block) = self.block_map.get(&mir_block.id) {
                builder.seal_block(cl_block);
            }
        }

        builder.finalize();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_stmt(
        builder: &mut ClifFunctionBuilder,
        stmt: &MirStmt,
        var_map: &HashMap<LocalId, Variable>,
        locals: &[rask_mir::MirLocal],
        func_refs: &HashMap<String, FuncRef>,
        struct_layouts: &[StructLayout],
        enum_layouts: &[EnumLayout],
        string_globals: &HashMap<String, GlobalValue>,
    ) -> CodegenResult<()> {
        match stmt {
            MirStmt::Assign { dst, rvalue } => {
                let dst_local = locals.iter().find(|l| l.id == *dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Destination variable not found".to_string()))?;
                let dst_ty = mir_to_cranelift_type(&dst_local.ty)?;

                let mut val = Self::lower_rvalue(
                    builder, rvalue, var_map, locals, Some(dst_ty),
                    struct_layouts, enum_layouts, string_globals,
                )?;

                let val_ty = builder.func.dfg.value_type(val);
                if val_ty != dst_ty {
                    val = Self::convert_value(builder, val, val_ty, dst_ty);
                }

                let var = var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                builder.def_var(*var, val);
            }

            MirStmt::Store { addr, offset, value } => {
                let addr_val = builder.use_var(*var_map.get(addr)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Address variable not found".to_string()))?);
                let val = Self::lower_operand(builder, value, var_map, string_globals)?;

                let flags = MemFlags::new();
                builder.ins().store(flags, val, addr_val, *offset as i32);
            }

            MirStmt::Call { dst, func, args } => {
                let func_ref = func_refs.get(&func.name)
                    .ok_or_else(|| CodegenError::FunctionNotFound(func.name.clone()))?;

                // Look up the callee's signature to get expected parameter types
                let ext_func = &builder.func.dfg.ext_funcs[*func_ref];
                let sig = &builder.func.dfg.signatures[ext_func.signature];
                let param_types: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();

                let mut arg_vals = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let expected_ty = param_types.get(i).copied();
                    let mut val = Self::lower_operand_typed(builder, a, var_map, expected_ty, string_globals)?;
                    // Convert if the actual type doesn't match the expected parameter type
                    if let Some(expected) = expected_ty {
                        let actual = builder.func.dfg.value_type(val);
                        if actual != expected {
                            val = Self::convert_value(builder, val, actual, expected);
                        }
                    }
                    arg_vals.push(val);
                }

                let call_inst = builder.ins().call(*func_ref, &arg_vals);

                if let Some(dst_id) = dst {
                    let results = builder.inst_results(call_inst);
                    if !results.is_empty() {
                        let result_val = results[0];
                        let var = var_map.get(dst_id)
                            .ok_or_else(|| CodegenError::UnsupportedFeature(
                                "Call destination variable not found".to_string()
                            ))?;

                        let dst_local = locals.iter().find(|l| l.id == *dst_id);
                        let mut val = result_val;
                        if let Some(local) = dst_local {
                            let dst_ty = mir_to_cranelift_type(&local.ty)?;
                            let val_ty = builder.func.dfg.value_type(val);
                            if val_ty != dst_ty {
                                val = Self::convert_value(builder, val, val_ty, dst_ty);
                            }
                        }
                        builder.def_var(*var, val);
                    }
                }
            }

            // Debug info — no codegen needed
            MirStmt::SourceLocation { .. } => {}

            // ── Resource tracking ──────────────────────────────────────
            // Calls C runtime functions for runtime must-consume checks.

            MirStmt::ResourceRegister { dst, scope_depth, .. } => {
                // rask_resource_register(scope_depth) → resource_id
                let func_ref = func_refs.get("rask_resource_register")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_register".to_string()))?;
                let depth_val = builder.ins().iconst(types::I64, *scope_depth as i64);
                let call_inst = builder.ins().call(*func_ref, &[depth_val]);

                let results = builder.inst_results(call_inst);
                if !results.is_empty() {
                    let var = var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature(
                            "Resource register destination not found".to_string()
                        ))?;
                    builder.def_var(*var, results[0]);
                }
            }

            MirStmt::ResourceConsume { resource_id } => {
                // rask_resource_consume(resource_id)
                let func_ref = func_refs.get("rask_resource_consume")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_consume".to_string()))?;
                let id_val = builder.use_var(*var_map.get(resource_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Resource ID variable not found".to_string()
                    ))?);
                builder.ins().call(*func_ref, &[id_val]);
            }

            MirStmt::ResourceScopeCheck { scope_depth } => {
                // rask_resource_scope_check(scope_depth)
                let func_ref = func_refs.get("rask_resource_scope_check")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_scope_check".to_string()))?;
                let depth_val = builder.ins().iconst(types::I64, *scope_depth as i64);
                builder.ins().call(*func_ref, &[depth_val]);
            }

            // ── Cleanup stack ──────────────────────────────────────────
            // EnsurePush/Pop track the cleanup scope during MIR construction.
            // At codegen time, the cleanup chain is already materialized in
            // CleanupReturn terminators, so these are no-ops.
            MirStmt::EnsurePush { .. } | MirStmt::EnsurePop => {}

            // ── Pool checked access ────────────────────────────────────
            MirStmt::PoolCheckedAccess { dst, pool, handle } => {
                // rask_pool_checked_access(pool, handle) → element_ptr
                let func_ref = func_refs.get("rask_pool_checked_access")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_pool_checked_access".to_string()))?;
                let pool_val = builder.use_var(*var_map.get(pool)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Pool variable not found".to_string()
                    ))?);
                let handle_val = builder.use_var(*var_map.get(handle)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Handle variable not found".to_string()
                    ))?);
                let call_inst = builder.ins().call(*func_ref, &[pool_val, handle_val]);

                let results = builder.inst_results(call_inst);
                if !results.is_empty() {
                    let var = var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature(
                            "Pool access destination not found".to_string()
                        ))?;
                    builder.def_var(*var, results[0]);
                }
            }
        }
        Ok(())
    }

    /// Convert a value between Cranelift types (integer widening/narrowing, float conversion).
    fn convert_value(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        from_ty: Type,
        to_ty: Type,
    ) -> Value {
        if from_ty == to_ty {
            return val;
        }

        if from_ty.is_int() && to_ty.is_int() {
            let from_bits = from_ty.bits();
            let to_bits = to_ty.bits();
            if from_bits == 1 {
                builder.ins().uextend(to_ty, val)
            } else if to_bits == 1 {
                builder.ins().icmp_imm(IntCC::NotEqual, val, 0)
            } else if from_bits > to_bits {
                builder.ins().ireduce(to_ty, val)
            } else {
                builder.ins().sextend(to_ty, val)
            }
        } else if from_ty.is_float() && to_ty.is_float() {
            if from_ty.bits() > to_ty.bits() {
                builder.ins().fdemote(to_ty, val)
            } else {
                builder.ins().fpromote(to_ty, val)
            }
        } else if from_ty.is_int() && to_ty.is_float() {
            builder.ins().fcvt_from_sint(to_ty, val)
        } else if from_ty.is_float() && to_ty.is_int() {
            builder.ins().fcvt_to_sint_sat(to_ty, val)
        } else {
            builder.ins().bitcast(to_ty, MemFlags::new(), val)
        }
    }

    /// Look up the MirType of an operand from the locals table.
    fn operand_mir_type(operand: &MirOperand, locals: &[rask_mir::MirLocal]) -> Option<MirType> {
        match operand {
            MirOperand::Local(id) => locals.iter().find(|l| l.id == *id).map(|l| l.ty.clone()),
            MirOperand::Constant(_) => None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_rvalue(
        builder: &mut ClifFunctionBuilder,
        rvalue: &MirRValue,
        var_map: &HashMap<LocalId, Variable>,
        locals: &[rask_mir::MirLocal],
        expected_ty: Option<Type>,
        struct_layouts: &[StructLayout],
        enum_layouts: &[EnumLayout],
        string_globals: &HashMap<String, GlobalValue>,
    ) -> CodegenResult<Value> {
        match rvalue {
            MirRValue::Use(op) => {
                Self::lower_operand_typed(builder, op, var_map, expected_ty, string_globals)
            }

            MirRValue::BinaryOp { op, left, right } => {
                let is_comparison = matches!(op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                );

                let operand_ty = if is_comparison { None } else { expected_ty };
                let lhs_val = Self::lower_operand_typed(builder, left, var_map, operand_ty, string_globals)?;
                let lhs_ty = builder.func.dfg.value_type(lhs_val);
                let rhs_val = Self::lower_operand_typed(builder, right, var_map, Some(lhs_ty), string_globals)?;

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

            MirRValue::UnaryOp { op, operand } => {
                let val = Self::lower_operand_typed(builder, operand, var_map, expected_ty, string_globals)?;

                let result = match op {
                    UnaryOp::Neg => builder.ins().ineg(val),
                    UnaryOp::Not => builder.ins().bnot(val),
                    UnaryOp::BitNot => builder.ins().bnot(val),
                };
                Ok(result)
            }

            MirRValue::Cast { value, target_ty } => {
                let val = Self::lower_operand(builder, value, var_map, string_globals)?;
                let target = mir_to_cranelift_type(target_ty)?;
                let val_ty = builder.func.dfg.value_type(val);
                Ok(Self::convert_value(builder, val, val_ty, target))
            }

            // Struct/enum field access: load from base pointer + field offset
            MirRValue::Field { base, field_index } => {
                let base_val = Self::lower_operand(builder, base, var_map, string_globals)?;
                let base_ty = Self::operand_mir_type(base, locals);
                let load_ty = expected_ty.unwrap_or(types::I64);

                let offset = match &base_ty {
                    Some(MirType::Struct(id)) => {
                        if let Some(layout) = struct_layouts.get(id.0 as usize) {
                            layout.fields
                                .get(*field_index as usize)
                                .map(|f| f.offset as i32)
                                .unwrap_or(0)
                        } else {
                            0
                        }
                    }
                    Some(MirType::Enum(id)) => {
                        if let Some(layout) = enum_layouts.get(id.0 as usize) {
                            // Payload starts at payload_offset; field is relative within payload.
                            // Use the first variant with enough fields for the offset.
                            let variant = layout.variants.iter()
                                .find(|v| v.fields.len() > *field_index as usize);
                            match variant {
                                Some(v) => (v.payload_offset + v.fields[*field_index as usize].offset) as i32,
                                None => layout.variants.first()
                                    .map(|v| v.payload_offset as i32)
                                    .unwrap_or(0),
                            }
                        } else {
                            0
                        }
                    }
                    _ => (*field_index as i32) * 8, // fallback: assume 8-byte stride
                };

                let flags = MemFlags::new();
                Ok(builder.ins().load(load_ty, flags, base_val, offset))
            }

            // Enum discriminant extraction: load tag byte from base pointer
            MirRValue::EnumTag { value } => {
                let ptr_val = Self::lower_operand(builder, value, var_map, string_globals)?;
                let base_ty = Self::operand_mir_type(value, locals);

                let tag_offset = match &base_ty {
                    Some(MirType::Enum(id)) => {
                        enum_layouts.get(id.0 as usize)
                            .map(|l| l.tag_offset as i32)
                            .unwrap_or(0)
                    }
                    _ => 0,
                };

                let flags = MemFlags::new();
                // Tag is always u8
                Ok(builder.ins().load(types::I8, flags, ptr_val, tag_offset))
            }

            // Address-of: return the pointer that the local already holds (for aggregates)
            // or spill a scalar to a stack slot and return its address.
            MirRValue::Ref(local_id) => {
                let var = var_map.get(local_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Ref: local not found".to_string()))?;
                let val = builder.use_var(*var);

                // For aggregate types the variable already IS a pointer
                let local_ty = locals.iter().find(|l| l.id == *local_id).map(|l| &l.ty);
                let is_aggregate = matches!(
                    local_ty,
                    Some(MirType::Struct(_) | MirType::Enum(_) | MirType::Array { .. })
                );

                if is_aggregate {
                    Ok(val)
                } else {
                    // Scalar: spill to a stack slot, return the address
                    let val_ty = builder.func.dfg.value_type(val);
                    let size = val_ty.bytes();
                    let ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        size,
                        0, // align_shift: natural alignment
                    ));
                    let addr = builder.ins().stack_addr(types::I64, ss, 0);
                    builder.ins().store(MemFlags::new(), val, addr, 0);
                    Ok(addr)
                }
            }

            // Pointer dereference: load the value pointed to by the operand
            MirRValue::Deref(operand) => {
                let ptr_val = Self::lower_operand(builder, operand, var_map, string_globals)?;
                let load_ty = expected_ty.unwrap_or(types::I64);
                let flags = MemFlags::new();
                Ok(builder.ins().load(load_ty, flags, ptr_val, 0))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn lower_terminator(
        builder: &mut ClifFunctionBuilder,
        term: &MirTerminator,
        var_map: &HashMap<LocalId, Variable>,
        block_map: &HashMap<BlockId, Block>,
        ret_ty: &MirType,
        mir_blocks: &[rask_mir::MirBlock],
        locals: &[rask_mir::MirLocal],
        func_refs: &HashMap<String, FuncRef>,
        struct_layouts: &[StructLayout],
        enum_layouts: &[EnumLayout],
        string_globals: &HashMap<String, GlobalValue>,
    ) -> CodegenResult<()> {
        match term {
            MirTerminator::Return { value } => {
                Self::emit_return(builder, value.as_ref(), ret_ty, var_map, string_globals)?;
            }

            MirTerminator::Goto { target } => {
                let target_block = block_map.get(target)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Target block not found".to_string()))?;
                builder.ins().jump(*target_block, &[]);
            }

            MirTerminator::Branch { cond, then_block, else_block } => {
                let mut cond_val = Self::lower_operand(builder, cond, var_map, string_globals)?;

                let cond_ty = builder.func.dfg.value_type(cond_val);
                if cond_ty == types::I8 {
                    cond_val = builder.ins().icmp_imm(IntCC::NotEqual, cond_val, 0);
                }

                let then_cl = block_map.get(then_block)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Then block not found".to_string()))?;
                let else_cl = block_map.get(else_block)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Else block not found".to_string()))?;
                builder.ins().brif(cond_val, *then_cl, &[], *else_cl, &[]);
            }

            MirTerminator::Switch { value, cases, default } => {
                let scrutinee_val = Self::lower_operand(builder, value, var_map, string_globals)?;

                let mut current_block = builder.current_block().unwrap();

                for (value, target_id) in cases {
                    let target_block = block_map.get(target_id)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Switch target block not found".to_string()))?;

                    let cmp_val = builder.ins().iconst(types::I64, *value as i64);
                    let cond = builder.ins().icmp(IntCC::Equal, scrutinee_val, cmp_val);

                    let next_block = builder.create_block();

                    builder.ins().brif(cond, *target_block, &[], next_block, &[]);
                    builder.switch_to_block(next_block);
                    builder.seal_block(current_block);
                    current_block = next_block;
                }

                let default_block = block_map.get(default)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Switch default block not found".to_string()))?;
                builder.ins().jump(*default_block, &[]);
                builder.seal_block(current_block);
            }

            MirTerminator::Unreachable => {
                builder.ins().trap(TrapCode::user(0).unwrap());
            }

            MirTerminator::CleanupReturn { value, cleanup_chain } => {
                // Run cleanup block statements inline, then return.
                // Cleanup blocks are not lowered as standalone Cranelift blocks
                // — their statements are inlined here to avoid CFG complexity.
                for block_id in cleanup_chain {
                    if let Some(mir_block) = mir_blocks.iter().find(|b| b.id == *block_id) {
                        for stmt in &mir_block.statements {
                            Self::lower_stmt(
                                builder, stmt, var_map, locals,
                                func_refs, struct_layouts, enum_layouts,
                                string_globals,
                            )?;
                        }
                    }
                }

                Self::emit_return(builder, Some(value), ret_ty, var_map, string_globals)?;
            }
        }
        Ok(())
    }

    /// Emit a return instruction.
    fn emit_return(
        builder: &mut ClifFunctionBuilder,
        value: Option<&MirOperand>,
        ret_ty: &MirType,
        var_map: &HashMap<LocalId, Variable>,
        string_globals: &HashMap<String, GlobalValue>,
    ) -> CodegenResult<()> {
        if let Some(val_op) = value {
            let expected_ty = mir_to_cranelift_type(ret_ty)?;
            let val = Self::lower_operand_typed(builder, val_op, var_map, Some(expected_ty), string_globals)?;
            let actual_ty = builder.func.dfg.value_type(val);
            let final_val = if actual_ty != expected_ty {
                Self::convert_value(builder, val, actual_ty, expected_ty)
            } else {
                val
            };
            builder.ins().return_(&[final_val]);
        } else {
            builder.ins().return_(&[]);
        }
        Ok(())
    }

    fn lower_operand(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        var_map: &HashMap<LocalId, Variable>,
        string_globals: &HashMap<String, GlobalValue>,
    ) -> CodegenResult<Value> {
        Self::lower_operand_typed(builder, op, var_map, None, string_globals)
    }

    fn lower_operand_typed(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        var_map: &HashMap<LocalId, Variable>,
        expected_ty: Option<Type>,
        string_globals: &HashMap<String, GlobalValue>,
    ) -> CodegenResult<Value> {
        match op {
            MirOperand::Local(local_id) => {
                let var = var_map.get(local_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Local not found".to_string()))?;
                Ok(builder.use_var(*var))
            }

            MirOperand::Constant(const_val) => {
                match const_val {
                    MirConst::Int(n) => {
                        let ty = expected_ty.unwrap_or(types::I32);
                        Ok(builder.ins().iconst(ty, *n))
                    }
                    MirConst::Float(f) => {
                        let ty = expected_ty.unwrap_or(types::F64);
                        if ty == types::F32 {
                            Ok(builder.ins().f32const(*f as f32))
                        } else {
                            Ok(builder.ins().f64const(*f))
                        }
                    }
                    MirConst::Bool(b) => {
                        Ok(builder.ins().iconst(types::I8, if *b { 1 } else { 0 }))
                    }
                    MirConst::Char(c) => {
                        Ok(builder.ins().iconst(types::I32, *c as i64))
                    }
                    MirConst::String(s) => {
                        // Look up the pre-created data section global for this string.
                        // Returns a pointer (i64) to null-terminated bytes.
                        if let Some(gv) = string_globals.get(s.as_str()) {
                            Ok(builder.ins().global_value(types::I64, *gv))
                        } else {
                            // Fallback: no data registered — emit null pointer
                            Ok(builder.ins().iconst(types::I64, 0))
                        }
                    }
                }
            }
        }
    }
}
