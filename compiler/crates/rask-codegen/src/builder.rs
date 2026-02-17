// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function builder — lowers MIR to Cranelift IR.

use cranelift::prelude::*;
use cranelift_codegen::ir::{FuncRef, Function, GlobalValue, InstBuilder, MemFlags, StackSlot, StackSlotData, StackSlotKind};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_frontend::{FunctionBuilder as ClifFunctionBuilder, FunctionBuilderContext};
use std::collections::{HashMap, HashSet};

use rask_mir::{BinOp, BlockId, LocalId, MirConst, MirFunction, MirOperand, MirRValue, MirStmt, MirTerminator, MirType, UnaryOp};
use rask_mono::{StructLayout, EnumLayout};
use crate::types::mir_to_cranelift_type;
use crate::{CodegenError, CodegenResult};

/// Result of adapting a stdlib call for the typed runtime API.
enum CallAdapt {
    /// No special post-call handling needed
    None,
    /// Result is void* — load the i64 value from the returned pointer
    DerefResult,
    /// Pop-style: value written to this stack slot by callee
    PopOutParam(StackSlot),
}

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
                    struct_layouts, enum_layouts, string_globals, func_refs,
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
                let val = Self::lower_operand(builder, value, var_map, string_globals, func_refs)?;

                let flags = MemFlags::new();
                builder.ins().store(flags, val, addr_val, *offset as i32);
            }

            MirStmt::Call { dst, func, args } => {
                // Builtin print/println — dispatch per-arg to typed runtime functions
                if func.name == "print" || func.name == "println" {
                    for (i, a) in args.iter().enumerate() {
                        if i > 0 {
                            let sp = Self::lower_operand_typed(
                                builder, &MirOperand::Constant(MirConst::String(" ".to_string())),
                                var_map, Some(types::I64), string_globals, func_refs,
                            )?;
                            let print_str = func_refs.get("rask_print_string")
                                .ok_or_else(|| CodegenError::FunctionNotFound("rask_print_string".into()))?;
                            builder.ins().call(*print_str, &[sp]);
                        }
                        let runtime_fn = Self::runtime_print_for_operand(a, locals);
                        let fr = func_refs.get(runtime_fn)
                            .ok_or_else(|| CodegenError::FunctionNotFound(runtime_fn.into()))?;
                        // Get the expected param type from the runtime function's signature
                        let ext_func = &builder.func.dfg.ext_funcs[*fr];
                        let sig = &builder.func.dfg.signatures[ext_func.signature];
                        let expected_ty = sig.params.first().map(|p| p.value_type);
                        let mut val = Self::lower_operand_typed(builder, a, var_map, expected_ty, string_globals, func_refs)?;
                        if let Some(expected) = expected_ty {
                            let actual = builder.func.dfg.value_type(val);
                            if actual != expected {
                                val = Self::convert_value(builder, val, actual, expected);
                            }
                        }
                        builder.ins().call(*fr, &[val]);
                    }
                    if func.name == "println" {
                        let nl = func_refs.get("rask_print_newline")
                            .ok_or_else(|| CodegenError::FunctionNotFound("rask_print_newline".into()))?;
                        builder.ins().call(*nl, &[]);
                    }
                    // print/println return void — define dest as zero if needed
                    if let Some(dst_id) = dst {
                        if let Some(var) = var_map.get(dst_id) {
                            let zero = builder.ins().iconst(types::I64, 0);
                            builder.def_var(*var, zero);
                        }
                    }
                } else if func.name == "assert" {
                    // assert(cond) — branch on condition, trap if false
                    if let Some(arg) = args.first() {
                        let cond = Self::lower_operand(builder, arg, var_map, string_globals, func_refs)?;
                        let ok_block = builder.create_block();
                        let fail_block = builder.create_block();
                        builder.ins().brif(cond, ok_block, &[], fail_block, &[]);

                        builder.seal_block(fail_block);
                        builder.switch_to_block(fail_block);
                        let assert_fn = func_refs.get("assert_fail")
                            .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                        builder.ins().call(*assert_fn, &[]);
                        builder.ins().trap(TrapCode::user(1).unwrap());

                        builder.seal_block(ok_block);
                        builder.switch_to_block(ok_block);
                    }
                    if let Some(dst_id) = dst {
                        if let Some(var) = var_map.get(dst_id) {
                            let zero = builder.ins().iconst(types::I64, 0);
                            builder.def_var(*var, zero);
                        }
                    }
                } else {
                    let func_ref = func_refs.get(&func.name)
                        .ok_or_else(|| CodegenError::FunctionNotFound(func.name.clone()))?;

                    // Lower MIR args to Cranelift values
                    let mut arg_vals = Vec::with_capacity(args.len());
                    for a in args.iter() {
                        let val = Self::lower_operand_typed(builder, a, var_map, Some(types::I64), string_globals, func_refs)?;
                        let actual = builder.func.dfg.value_type(val);
                        let converted = if actual != types::I64 && actual.is_int() {
                            Self::convert_value(builder, val, actual, types::I64)
                        } else {
                            val
                        };
                        arg_vals.push(converted);
                    }

                    // Adapt args for typed runtime API
                    let adapt = Self::adapt_stdlib_call(builder, &func.name, &mut arg_vals);

                    // Re-read signature after adaptation (arg count may have changed)
                    let ext_func = &builder.func.dfg.ext_funcs[*func_ref];
                    let sig = &builder.func.dfg.signatures[ext_func.signature];
                    let param_types: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();

                    // Convert arg types to match the declared signature
                    for (i, val) in arg_vals.iter_mut().enumerate() {
                        if let Some(&expected) = param_types.get(i) {
                            let actual = builder.func.dfg.value_type(*val);
                            if actual != expected {
                                *val = Self::convert_value(builder, *val, actual, expected);
                            }
                        }
                    }

                    let call_inst = builder.ins().call(*func_ref, &arg_vals);

                    if let Some(dst_id) = dst {
                        let var = var_map.get(dst_id)
                            .ok_or_else(|| CodegenError::UnsupportedFeature(
                                "Call destination variable not found".to_string()
                            ))?;

                        // Post-call result handling
                        let val = match adapt {
                            CallAdapt::DerefResult => {
                                // Result is void* — load the i64 value from it
                                let results = builder.inst_results(call_inst);
                                if !results.is_empty() {
                                    let ptr = results[0];
                                    builder.ins().load(types::I64, MemFlags::new(), ptr, 0)
                                } else {
                                    builder.ins().iconst(types::I64, 0)
                                }
                            }
                            CallAdapt::PopOutParam(ss) => {
                                // Value was written to stack slot by callee
                                builder.ins().stack_load(types::I64, ss, 0)
                            }
                            _ => {
                                let results = builder.inst_results(call_inst);
                                if !results.is_empty() {
                                    results[0]
                                } else {
                                    builder.ins().iconst(types::I64, 0)
                                }
                            }
                        };

                        let dst_local = locals.iter().find(|l| l.id == *dst_id);
                        let final_val = if let Some(local) = dst_local {
                            let dst_ty = mir_to_cranelift_type(&local.ty)?;
                            let val_ty = builder.func.dfg.value_type(val);
                            if val_ty != dst_ty {
                                Self::convert_value(builder, val, val_ty, dst_ty)
                            } else {
                                val
                            }
                        } else {
                            val
                        };
                        builder.def_var(*var, final_val);
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

            // ── Closure support ──────────────────────────────────────────

            MirStmt::ClosureCreate { dst, func_name, captures, heap } => {
                // Build environment layout from captures
                let env_layout = crate::closures::ClosureEnvLayout {
                    size: captures.last()
                        .map(|c| c.offset + c.size)
                        .unwrap_or(0),
                    captures: captures.iter().map(|c| crate::closures::CaptureInfo {
                        local_id: c.local_id,
                        offset: c.offset,
                        size: c.size,
                    }).collect(),
                };

                // Get function pointer for the closure function
                let func_ref = func_refs.get(func_name)
                    .ok_or_else(|| CodegenError::FunctionNotFound(func_name.clone()))?;
                let func_ptr = builder.ins().func_addr(types::I64, *func_ref);

                let closure_ptr = if *heap {
                    // Escaping closure: heap-allocate via rask_alloc
                    let alloc_ref = func_refs.get("rask_alloc")
                        .ok_or_else(|| CodegenError::FunctionNotFound("rask_alloc".to_string()))?;
                    crate::closures::allocate_closure_heap(
                        builder, func_ptr, &env_layout, var_map, *alloc_ref,
                    )?
                } else {
                    // Non-escaping closure: stack-allocate
                    crate::closures::allocate_closure_stack(
                        builder, func_ptr, &env_layout, var_map,
                    )?
                };

                let var = var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "ClosureCreate destination not found".to_string()
                    ))?;
                builder.def_var(*var, closure_ptr);
            }

            MirStmt::ClosureCall { dst, closure, args } => {
                let closure_val = builder.use_var(*var_map.get(closure)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Closure variable not found".to_string()
                    ))?);

                // Lower arg values
                let mut arg_vals = Vec::new();
                for a in args {
                    let val = Self::lower_operand(builder, a, var_map, string_globals, func_refs)?;
                    arg_vals.push(val);
                }

                // Build signature: (args...) -> ret
                // call_closure will prepend env_ptr automatically
                let mut sig = builder.func.signature.clone();
                sig.params.clear();
                sig.returns.clear();

                for val in &arg_vals {
                    let ty = builder.func.dfg.value_type(*val);
                    sig.params.push(AbiParam::new(ty));
                }

                if let Some(dst_id) = dst {
                    let dst_local = locals.iter().find(|l| l.id == *dst_id);
                    if let Some(local) = dst_local {
                        let ret_ty = mir_to_cranelift_type(&local.ty)?;
                        sig.returns.push(AbiParam::new(ret_ty));
                    }
                }

                let call_inst = crate::closures::call_closure(
                    builder, closure_val, sig, &arg_vals,
                );

                if let Some(dst_id) = dst {
                    let results = builder.inst_results(call_inst);
                    if !results.is_empty() {
                        let var = var_map.get(dst_id)
                            .ok_or_else(|| CodegenError::UnsupportedFeature(
                                "ClosureCall destination not found".to_string()
                            ))?;
                        builder.def_var(*var, results[0]);
                    }
                }
            }

            MirStmt::LoadCapture { dst, env_ptr, offset } => {
                let env_val = builder.use_var(*var_map.get(env_ptr)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture env_ptr not found".to_string()
                    ))?);
                let dst_local = locals.iter().find(|l| l.id == *dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture destination not found".to_string()
                    ))?;
                let load_ty = mir_to_cranelift_type(&dst_local.ty)?;
                let val = crate::closures::load_capture(builder, env_val, *offset, load_ty);
                let var = var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture destination variable not found".to_string()
                    ))?;
                builder.def_var(*var, val);
            }

            MirStmt::ClosureDrop { closure } => {
                let closure_val = builder.use_var(*var_map.get(closure)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "ClosureDrop closure variable not found".to_string()
                    ))?);
                let free_ref = func_refs.get("rask_free")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_free".to_string()))?;
                crate::closures::free_closure(builder, closure_val, *free_ref);
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

    /// Pick the runtime print function based on the MIR operand.
    fn runtime_print_for_operand(op: &MirOperand, locals: &[rask_mir::MirLocal]) -> &'static str {
        match op {
            MirOperand::Constant(c) => match c {
                MirConst::String(_) => "rask_print_string",
                MirConst::Bool(_) => "rask_print_bool",
                MirConst::Float(_) => "rask_print_f64",
                _ => "rask_print_i64",
            },
            MirOperand::Local(id) => {
                if let Some(local) = locals.iter().find(|l| l.id == *id) {
                    match &local.ty {
                        MirType::Bool => "rask_print_bool",
                        MirType::F32 => "rask_print_f32",
                        MirType::F64 => "rask_print_f64",
                        MirType::Char => "rask_print_char",
                        MirType::String => "rask_print_string",
                        MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64 => "rask_print_u64",
                        _ => "rask_print_i64",
                    }
                } else {
                    "rask_print_i64"
                }
            }
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
        func_refs: &HashMap<String, FuncRef>,
    ) -> CodegenResult<Value> {
        match rvalue {
            MirRValue::Use(op) => {
                Self::lower_operand_typed(builder, op, var_map, expected_ty, string_globals, func_refs)
            }

            MirRValue::BinaryOp { op, left, right } => {
                let is_comparison = matches!(op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                );

                let operand_ty = if is_comparison { None } else { expected_ty };
                let lhs_val = Self::lower_operand_typed(builder, left, var_map, operand_ty, string_globals, func_refs)?;
                let lhs_ty = builder.func.dfg.value_type(lhs_val);
                let rhs_val = Self::lower_operand_typed(builder, right, var_map, Some(lhs_ty), string_globals, func_refs)?;
                let rhs_ty = builder.func.dfg.value_type(rhs_val);

                let is_float = lhs_ty.is_float();

                // Check if the left operand has an unsigned MIR type
                let is_unsigned = Self::operand_mir_type(left, locals)
                    .map(|t| t.is_unsigned())
                    .unwrap_or(false);

                // Widen narrower operand if integer types differ
                let (lhs_val, rhs_val) = if lhs_ty != rhs_ty && lhs_ty.is_int() && rhs_ty.is_int() {
                    if lhs_ty.bits() < rhs_ty.bits() {
                        (Self::convert_value(builder, lhs_val, lhs_ty, rhs_ty), rhs_val)
                    } else {
                        (lhs_val, Self::convert_value(builder, rhs_val, rhs_ty, lhs_ty))
                    }
                } else if lhs_ty != rhs_ty && is_float {
                    // Promote narrower float
                    if lhs_ty.bits() < rhs_ty.bits() {
                        (builder.ins().fpromote(rhs_ty, lhs_val), rhs_val)
                    } else {
                        (lhs_val, builder.ins().fpromote(lhs_ty, rhs_val))
                    }
                } else {
                    (lhs_val, rhs_val)
                };

                let result = if is_float {
                    match op {
                        BinOp::Add => builder.ins().fadd(lhs_val, rhs_val),
                        BinOp::Sub => builder.ins().fsub(lhs_val, rhs_val),
                        BinOp::Mul => builder.ins().fmul(lhs_val, rhs_val),
                        BinOp::Div => builder.ins().fdiv(lhs_val, rhs_val),
                        BinOp::Mod => {
                            // fmod: a - trunc(a/b) * b
                            let div = builder.ins().fdiv(lhs_val, rhs_val);
                            let trunc = builder.ins().trunc(div);
                            let prod = builder.ins().fmul(trunc, rhs_val);
                            builder.ins().fsub(lhs_val, prod)
                        }
                        BinOp::Eq => builder.ins().fcmp(FloatCC::Equal, lhs_val, rhs_val),
                        BinOp::Ne => builder.ins().fcmp(FloatCC::NotEqual, lhs_val, rhs_val),
                        BinOp::Lt => builder.ins().fcmp(FloatCC::LessThan, lhs_val, rhs_val),
                        BinOp::Le => builder.ins().fcmp(FloatCC::LessThanOrEqual, lhs_val, rhs_val),
                        BinOp::Gt => builder.ins().fcmp(FloatCC::GreaterThan, lhs_val, rhs_val),
                        BinOp::Ge => builder.ins().fcmp(FloatCC::GreaterThanOrEqual, lhs_val, rhs_val),
                        BinOp::And | BinOp::Or => return Err(CodegenError::UnsupportedFeature(format!("Logical op {:?} should be lowered to branches in MIR", op))),
                        _ => return Err(CodegenError::UnsupportedFeature(format!("Bitwise op {:?} not valid on floats", op))),
                    }
                } else {
                    match op {
                        BinOp::Add => builder.ins().iadd(lhs_val, rhs_val),
                        BinOp::Sub => builder.ins().isub(lhs_val, rhs_val),
                        BinOp::Mul => builder.ins().imul(lhs_val, rhs_val),
                        BinOp::Div if is_unsigned => builder.ins().udiv(lhs_val, rhs_val),
                        BinOp::Div => builder.ins().sdiv(lhs_val, rhs_val),
                        BinOp::Mod if is_unsigned => builder.ins().urem(lhs_val, rhs_val),
                        BinOp::Mod => builder.ins().srem(lhs_val, rhs_val),
                        BinOp::BitAnd => builder.ins().band(lhs_val, rhs_val),
                        BinOp::BitOr => builder.ins().bor(lhs_val, rhs_val),
                        BinOp::BitXor => builder.ins().bxor(lhs_val, rhs_val),
                        BinOp::Shl => builder.ins().ishl(lhs_val, rhs_val),
                        BinOp::Shr if is_unsigned => builder.ins().ushr(lhs_val, rhs_val),
                        BinOp::Shr => builder.ins().sshr(lhs_val, rhs_val),
                        BinOp::Eq => builder.ins().icmp(IntCC::Equal, lhs_val, rhs_val),
                        BinOp::Ne => builder.ins().icmp(IntCC::NotEqual, lhs_val, rhs_val),
                        BinOp::Lt if is_unsigned => builder.ins().icmp(IntCC::UnsignedLessThan, lhs_val, rhs_val),
                        BinOp::Lt => builder.ins().icmp(IntCC::SignedLessThan, lhs_val, rhs_val),
                        BinOp::Le if is_unsigned => builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, lhs_val, rhs_val),
                        BinOp::Le => builder.ins().icmp(IntCC::SignedLessThanOrEqual, lhs_val, rhs_val),
                        BinOp::Gt if is_unsigned => builder.ins().icmp(IntCC::UnsignedGreaterThan, lhs_val, rhs_val),
                        BinOp::Gt => builder.ins().icmp(IntCC::SignedGreaterThan, lhs_val, rhs_val),
                        BinOp::Ge if is_unsigned => builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, lhs_val, rhs_val),
                        BinOp::Ge => builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, lhs_val, rhs_val),
                        BinOp::And | BinOp::Or => return Err(CodegenError::UnsupportedFeature(format!("Logical op {:?} should be lowered to branches in MIR", op))),
                    }
                };
                Ok(result)
            }

            MirRValue::UnaryOp { op, operand } => {
                let val = Self::lower_operand_typed(builder, operand, var_map, expected_ty, string_globals, func_refs)?;
                let val_ty = builder.func.dfg.value_type(val);

                let result = match op {
                    UnaryOp::Neg if val_ty.is_float() => builder.ins().fneg(val),
                    UnaryOp::Neg => builder.ins().ineg(val),
                    // Logical NOT: XOR with 1 to flip the boolean bit.
                    // bnot flips all bits which is wrong for booleans
                    // (e.g. bnot(1) = 0xFE, not 0).
                    UnaryOp::Not => {
                        let val_ty = builder.func.dfg.value_type(val);
                        let one = builder.ins().iconst(val_ty, 1);
                        builder.ins().bxor(val, one)
                    }
                    UnaryOp::BitNot => builder.ins().bnot(val),
                };
                Ok(result)
            }

            MirRValue::Cast { value, target_ty } => {
                let val = Self::lower_operand(builder, value, var_map, string_globals, func_refs)?;
                let target = mir_to_cranelift_type(target_ty)?;
                let val_ty = builder.func.dfg.value_type(val);
                Ok(Self::convert_value(builder, val, val_ty, target))
            }

            // Struct/enum field access: load from base pointer + field offset
            MirRValue::Field { base, field_index } => {
                let base_val = Self::lower_operand(builder, base, var_map, string_globals, func_refs)?;
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
                    // No layout available (Option/Result/Tuple lowered to MirType::Ptr).
                    // Currently only field_index 0 reaches here (enum payload extraction).
                    // Higher indices would need full layout info to account for alignment
                    // padding between heterogeneous fields.
                    _ => 0
                };

                let flags = MemFlags::new();
                Ok(builder.ins().load(load_ty, flags, base_val, offset))
            }

            // Enum discriminant extraction: load tag byte from base pointer
            MirRValue::EnumTag { value } => {
                let ptr_val = Self::lower_operand(builder, value, var_map, string_globals, func_refs)?;
                let base_ty = Self::operand_mir_type(value, locals);

                let (tag_offset, tag_cranelift_ty) = match &base_ty {
                    Some(MirType::Enum(id)) => {
                        if let Some(layout) = enum_layouts.get(id.0 as usize) {
                            let offset = layout.tag_offset as i32;
                            // Derive Cranelift type from tag type's size
                            let (tag_size, _) = rask_mono::type_size_align(&layout.tag_ty);
                            let ty = match tag_size {
                                2 => types::I16,
                                _ => types::I8,
                            };
                            (offset, ty)
                        } else {
                            (0, types::I8)
                        }
                    }
                    _ => (0, types::I8),
                };

                let flags = MemFlags::new();
                Ok(builder.ins().load(tag_cranelift_ty, flags, ptr_val, tag_offset))
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
                let ptr_val = Self::lower_operand(builder, operand, var_map, string_globals, func_refs)?;
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
                Self::emit_return(builder, value.as_ref(), ret_ty, var_map, string_globals, func_refs)?;
            }

            MirTerminator::Goto { target } => {
                let target_block = block_map.get(target)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Target block not found".to_string()))?;
                builder.ins().jump(*target_block, &[]);
            }

            MirTerminator::Branch { cond, then_block, else_block } => {
                let mut cond_val = Self::lower_operand(builder, cond, var_map, string_globals, func_refs)?;

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
                let raw_scrutinee = Self::lower_operand(builder, value, var_map, string_globals, func_refs)?;
                // Extend to i64 if the scrutinee is a narrower type (e.g. u8 enum tag)
                let scrutinee_val = {
                    let val_ty = builder.func.dfg.value_type(raw_scrutinee);
                    if val_ty != types::I64 && val_ty.is_int() {
                        builder.ins().uextend(types::I64, raw_scrutinee)
                    } else {
                        raw_scrutinee
                    }
                };

                // Create comparison chain: each case gets a brif, falling through to next
                // Don't seal MIR blocks here — the final seal-all loop handles them
                let mut comparison_blocks = Vec::new();

                for (value, target_id) in cases {
                    let target_block = block_map.get(target_id)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Switch target block not found".to_string()))?;

                    let cmp_val = builder.ins().iconst(types::I64, *value as i64);
                    let cond = builder.ins().icmp(IntCC::Equal, scrutinee_val, cmp_val);

                    let next_block = builder.create_block();
                    comparison_blocks.push(next_block);

                    builder.ins().brif(cond, *target_block, &[], next_block, &[]);
                    builder.switch_to_block(next_block);
                }

                let default_block = block_map.get(default)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Switch default block not found".to_string()))?;
                builder.ins().jump(*default_block, &[]);

                // Seal comparison chain blocks (these aren't MIR blocks)
                for block in comparison_blocks {
                    builder.seal_block(block);
                }
            }

            MirTerminator::Unreachable => {
                builder.ins().trap(TrapCode::user(1).unwrap());
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

                Self::emit_return(builder, value.as_ref(), ret_ty, var_map, string_globals, func_refs)?;
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
        func_refs: &HashMap<String, FuncRef>,
    ) -> CodegenResult<()> {
        if let Some(val_op) = value {
            let expected_ty = mir_to_cranelift_type(ret_ty)?;
            let val = Self::lower_operand_typed(builder, val_op, var_map, Some(expected_ty), string_globals, func_refs)?;
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

    /// Store a value to a stack slot and return its address.
    /// Used for pointer-based calling convention (typed runtime API).
    fn value_to_ptr(builder: &mut ClifFunctionBuilder, val: Value) -> Value {
        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot, 8, 0,
        ));
        builder.ins().stack_store(val, ss, 0);
        builder.ins().stack_addr(types::I64, ss, 0)
    }

    /// Adapt stdlib call args for the typed runtime API.
    /// Injects elem_size args, wraps values as pointers, adds out-params.
    /// Returns the post-call adaptation needed.
    fn adapt_stdlib_call(
        builder: &mut ClifFunctionBuilder,
        func_name: &str,
        args: &mut Vec<Value>,
    ) -> CallAdapt {
        match func_name {
            // Constructors: inject elem_size / key_size+val_size
            "Vec_new" => {
                let elem_size = builder.ins().iconst(types::I64, 8);
                args.insert(0, elem_size);
                CallAdapt::None
            }
            "Map_new" => {
                let key_size = builder.ins().iconst(types::I64, 8);
                let val_size = builder.ins().iconst(types::I64, 8);
                args.insert(0, key_size);
                args.insert(1, val_size);
                CallAdapt::None
            }
            "Pool_new" => {
                let elem_size = builder.ins().iconst(types::I64, 8);
                args.insert(0, elem_size);
                CallAdapt::None
            }

            // Vec push/set: wrap value arg as pointer
            "push" => {
                // args: [vec, value] → [vec, &value]
                if args.len() >= 2 {
                    let val = args[1];
                    args[1] = Self::value_to_ptr(builder, val);
                }
                CallAdapt::None
            }
            "set" => {
                // args: [vec, index, value] → [vec, index, &value]
                if args.len() >= 3 {
                    let val = args[2];
                    args[2] = Self::value_to_ptr(builder, val);
                }
                CallAdapt::None
            }

            // Vec pop: add out-param, load result from it
            "pop" => {
                // args: [vec] → [vec, &out]
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.push(addr);
                CallAdapt::PopOutParam(ss)
            }

            // Vec get/index: result is void*, deref to get value
            "get" | "index" => CallAdapt::DerefResult,

            // Map insert: wrap key and value as pointers
            "insert" => {
                // args: [map, key, value] → [map, &key, &value]
                if args.len() >= 3 {
                    let key = args[1];
                    let val = args[2];
                    args[1] = Self::value_to_ptr(builder, key);
                    args[2] = Self::value_to_ptr(builder, val);
                }
                CallAdapt::None
            }

            // Map contains_key/remove: wrap key as pointer
            "contains_key" | "map_remove" => {
                if args.len() >= 2 {
                    let key = args[1];
                    args[1] = Self::value_to_ptr(builder, key);
                }
                CallAdapt::None
            }

            // Map get: wrap key as pointer, deref result
            "map_get" => {
                if args.len() >= 2 {
                    let key = args[1];
                    args[1] = Self::value_to_ptr(builder, key);
                }
                CallAdapt::DerefResult
            }

            _ => CallAdapt::None,
        }
    }

    fn lower_operand(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        var_map: &HashMap<LocalId, Variable>,
        string_globals: &HashMap<String, GlobalValue>,
        func_refs: &HashMap<String, FuncRef>,
    ) -> CodegenResult<Value> {
        Self::lower_operand_typed(builder, op, var_map, None, string_globals, func_refs)
    }

    fn lower_operand_typed(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        var_map: &HashMap<LocalId, Variable>,
        expected_ty: Option<Type>,
        string_globals: &HashMap<String, GlobalValue>,
        func_refs: &HashMap<String, FuncRef>,
    ) -> CodegenResult<Value> {
        match op {
            MirOperand::Local(local_id) => {
                let var = var_map.get(local_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Local not found".to_string()))?;
                let val = builder.use_var(*var);
                // Widen to expected type if needed (e.g., i32 local used where i64 expected)
                if let Some(exp_ty) = expected_ty {
                    let actual_ty = builder.func.dfg.value_type(val);
                    if actual_ty != exp_ty && actual_ty.is_int() && exp_ty.is_int() {
                        return Ok(Self::convert_value(builder, val, actual_ty, exp_ty));
                    }
                }
                Ok(val)
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
                        // String constants: get raw char* from data section,
                        // then wrap in RaskString via rask_string_from().
                        if let Some(gv) = string_globals.get(s.as_str()) {
                            let raw_ptr = builder.ins().global_value(types::I64, *gv);
                            if let Some(string_from_ref) = func_refs.get("string_from") {
                                let call = builder.ins().call(*string_from_ref, &[raw_ptr]);
                                let results = builder.inst_results(call);
                                Ok(results[0])
                            } else {
                                Ok(raw_ptr)
                            }
                        } else {
                            Ok(builder.ins().iconst(types::I64, 0))
                        }
                    }
                }
            }
        }
    }
}
