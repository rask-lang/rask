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
    /// Wrap raw return value as Ok(value) in a Result stack slot.
    /// Used for C functions that return a raw value where Rask expects a Result.
    WrapOkResult(StackSlot),
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
    /// Comptime global data (const name → GlobalValue for the data address)
    comptime_globals: &'a HashMap<String, GlobalValue>,
    /// MIR names of stdlib functions that can panic at runtime
    panicking_fns: &'a HashSet<String>,
    /// Names of functions compiled as Rask code (vs C stdlib)
    internal_fns: &'a HashSet<String>,

    /// Map MIR block IDs to Cranelift blocks
    block_map: HashMap<BlockId, Block>,
    /// Map MIR locals to Cranelift variables
    var_map: HashMap<LocalId, Variable>,

    /// Stack slots allocated for aggregate locals (struct, enum, result, etc.)
    /// Maps LocalId → (StackSlot, byte_size) so calls returning aggregates can
    /// memcpy into the caller's slot instead of storing a dangling callee pointer.
    stack_slot_map: HashMap<LocalId, (StackSlot, u32)>,

    /// Current source location tracked from SourceLocation statements
    current_line: u32,
    current_col: u32,
}

impl<'a> FunctionBuilder<'a> {
    pub fn new(
        func: &'a mut Function,
        mir_fn: &'a MirFunction,
        func_refs: &'a HashMap<String, FuncRef>,
        struct_layouts: &'a [StructLayout],
        enum_layouts: &'a [EnumLayout],
        string_globals: &'a HashMap<String, GlobalValue>,
        comptime_globals: &'a HashMap<String, GlobalValue>,
        panicking_fns: &'a HashSet<String>,
        internal_fns: &'a HashSet<String>,
    ) -> CodegenResult<Self> {
        Ok(FunctionBuilder {
            func,
            builder_ctx: FunctionBuilderContext::new(),
            mir_fn,
            func_refs,
            struct_layouts,
            enum_layouts,
            string_globals,
            comptime_globals,
            panicking_fns,
            internal_fns,
            block_map: HashMap::new(),
            var_map: HashMap::new(),
            stack_slot_map: HashMap::new(),
            current_line: 0,
            current_col: 0,
        })
    }

    /// Build the Cranelift IR from MIR.
    pub fn build(&mut self) -> CodegenResult<()> {
        // Pre-compute stack allocation sizes before builder borrows self.func.
        // Entries: (local_id, byte size) for each aggregate local.
        let stack_allocs: Vec<(LocalId, u32)> = self.mir_fn.locals.iter()
            .filter(|l| !l.is_param)
            .filter_map(|l| {
                let size = Self::resolve_type_alloc_size(
                    &l.ty, self.struct_layouts, self.enum_layouts,
                );
                size.filter(|&s| s > 0).map(|s| (l.id, s))
            })
            .collect();

        // Collect cleanup-only blocks (appear in CleanupReturn chains).
        // A single shared Cranelift block is created per unique cleanup
        // chain — all CleanupReturn sites with the same chain jump to
        // the shared block instead of inlining the cleanup statements.
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

        // Deduplicate cleanup chains: map each unique chain to a shared block.
        let mut cleanup_chain_blocks: HashMap<Vec<BlockId>, cranelift_codegen::ir::Block> =
            HashMap::new();

        let mut builder = ClifFunctionBuilder::new(self.func, &mut self.builder_ctx);

        // Create blocks (skip cleanup-only blocks — handled via shared cleanup blocks)
        for mir_block in &self.mir_fn.blocks {
            if cleanup_only.contains(&mir_block.id) {
                continue;
            }
            let block = builder.create_block();
            self.block_map.insert(mir_block.id, block);
        }

        // Create shared cleanup blocks for each unique chain.
        for mir_block in &self.mir_fn.blocks {
            if let MirTerminator::CleanupReturn { cleanup_chain, .. } = &mir_block.terminator {
                if !cleanup_chain.is_empty() && !cleanup_chain_blocks.contains_key(cleanup_chain) {
                    let shared_block = builder.create_block();
                    cleanup_chain_blocks.insert(cleanup_chain.clone(), shared_block);
                }
            }
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
            self.stack_slot_map.insert(*local_id, (ss, *size));
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
                // Track source location for runtime error messages
                if let MirStmt::SourceLocation { line, col } = stmt {
                    self.current_line = *line;
                    self.current_col = *col;
                    continue;
                }
                Self::lower_stmt(
                    &mut builder, stmt, &self.var_map, &self.mir_fn.locals,
                    self.func_refs, self.struct_layouts, self.enum_layouts,
                    self.string_globals, self.comptime_globals,
                    self.mir_fn.source_file.as_deref(),
                    self.current_line, self.current_col,
                    self.panicking_fns,
                    &self.stack_slot_map,
                    self.internal_fns,
                )?;
            }

            // Lower terminator
            Self::lower_terminator(
                &mut builder, &mir_block.terminator, &self.var_map,
                &self.block_map, &self.mir_fn.ret_ty,
                &self.mir_fn.blocks, &self.mir_fn.locals,
                self.func_refs, self.struct_layouts, self.enum_layouts,
                self.string_globals, self.comptime_globals,
                self.panicking_fns,
                &self.stack_slot_map,
                self.internal_fns,
                &cleanup_chain_blocks,
            )?;
        }

        // Emit shared cleanup blocks. Each unique cleanup chain gets one
        // Cranelift block that runs the cleanup statements and returns.
        for (chain, &shared_block) in &cleanup_chain_blocks {
            builder.switch_to_block(shared_block);

            // Add return value as block parameter if function returns a value
            let ret_param = if !matches!(self.mir_fn.ret_ty, MirType::Void) {
                let ret_cl_ty = mir_to_cranelift_type(&self.mir_fn.ret_ty)?;
                Some(builder.append_block_param(shared_block, ret_cl_ty))
            } else {
                None
            };

            // Emit cleanup statements from each block in the chain
            for block_id in chain {
                if let Some(mir_block) = self.mir_fn.blocks.iter().find(|b| b.id == *block_id) {
                    for stmt in &mir_block.statements {
                        Self::lower_stmt(
                            &mut builder, stmt, &self.var_map, &self.mir_fn.locals,
                            self.func_refs, self.struct_layouts, self.enum_layouts,
                            self.string_globals, self.comptime_globals,
                            None, 0, 0,
                            self.panicking_fns,
                            &self.stack_slot_map,
                            self.internal_fns,
                        )?;
                    }
                }
            }

            // Return
            if let Some(val) = ret_param {
                builder.ins().return_(&[val]);
            } else {
                builder.ins().return_(&[]);
            }
        }

        // Now seal all blocks (all predecessors are known)
        for mir_block in &self.mir_fn.blocks {
            if let Some(&cl_block) = self.block_map.get(&mir_block.id) {
                builder.seal_block(cl_block);
            }
        }
        for &shared_block in cleanup_chain_blocks.values() {
            builder.seal_block(shared_block);
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
        comptime_globals: &HashMap<String, GlobalValue>,
        source_file: Option<&str>,
        current_line: u32,
        current_col: u32,
        panicking_fns: &HashSet<String>,
        stack_slot_map: &HashMap<LocalId, (StackSlot, u32)>,
        internal_fns: &HashSet<String>,
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

                // If the value is a stack-allocated aggregate (struct/enum), copy its
                // data instead of storing the pointer. This handles Ok(struct_val) where
                // the struct data must be embedded in the Result's payload area.
                // Use the variable's current value (not the stack_slot address) because
                // the variable may alias another slot (e.g., p = struct_literal result).
                let is_aggregate = if let MirOperand::Local(src_id) = value {
                    if let Some((_src_slot, src_size)) = stack_slot_map.get(src_id) {
                        let src_var = var_map.get(src_id)
                            .ok_or_else(|| CodegenError::UnsupportedFeature("Aggregate source not found".to_string()))?;
                        let src_addr = builder.use_var(*src_var);
                        let mut byte_offset = 0i32;
                        let size = *src_size as i32;
                        while byte_offset + 8 <= size {
                            let word = builder.ins().load(types::I64, MemFlags::new(), src_addr, byte_offset);
                            builder.ins().store(MemFlags::new(), word, addr_val, *offset as i32 + byte_offset);
                            byte_offset += 8;
                        }
                        if size - byte_offset >= 4 {
                            let word = builder.ins().load(types::I32, MemFlags::new(), src_addr, byte_offset);
                            builder.ins().store(MemFlags::new(), word, addr_val, *offset as i32 + byte_offset);
                            byte_offset += 4;
                        }
                        if size - byte_offset >= 2 {
                            let word = builder.ins().load(types::I16, MemFlags::new(), src_addr, byte_offset);
                            builder.ins().store(MemFlags::new(), word, addr_val, *offset as i32 + byte_offset);
                            byte_offset += 2;
                        }
                        if size - byte_offset >= 1 {
                            let word = builder.ins().load(types::I8, MemFlags::new(), src_addr, byte_offset);
                            builder.ins().store(MemFlags::new(), word, addr_val, *offset as i32 + byte_offset);
                        }
                        true
                    } else { false }
                } else { false };

                if !is_aggregate {
                    let val = Self::lower_operand(builder, value, var_map, string_globals, func_refs)?;
                    let flags = MemFlags::new();
                    builder.ins().store(flags, val, addr_val, *offset as i32);
                }
            }

            // Array element store: base_ptr[index * elem_size] = value
            MirStmt::ArrayStore { base, index, elem_size, value } => {
                let base_val = builder.use_var(*var_map.get(base)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("ArrayStore: base not found".to_string()))?);
                let idx_val = Self::lower_operand_typed(builder, index, var_map, Some(types::I64), string_globals, func_refs)?;
                let val = Self::lower_operand(builder, value, var_map, string_globals, func_refs)?;
                let elem_sz = builder.ins().iconst(types::I64, *elem_size as i64);
                let offset = builder.ins().imul(idx_val, elem_sz);
                let addr = builder.ins().iadd(base_val, offset);
                let flags = MemFlags::new();
                builder.ins().store(flags, val, addr, 0);
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
                } else if func.name == "assert_fail" {
                    // MIR already handled branching; this is the fail path.
                    // Use location-aware variant when source info is available.
                    if let Some(file_str) = source_file {
                        if let (Some(func_ref), Some(gv)) = (
                            func_refs.get("assert_fail_at"),
                            string_globals.get(file_str),
                        ) {
                            let file_ptr = builder.ins().global_value(types::I64, *gv);
                            let line_val = builder.ins().iconst(types::I32, current_line as i64);
                            let col_val = builder.ins().iconst(types::I32, current_col as i64);
                            builder.ins().call(*func_ref, &[file_ptr, line_val, col_val]);
                        } else {
                            let assert_fn = func_refs.get("assert_fail")
                                .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                            builder.ins().call(*assert_fn, &[]);
                        }
                    } else {
                        let assert_fn = func_refs.get("assert_fail")
                            .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                        builder.ins().call(*assert_fn, &[]);
                    }
                } else if func.name == "panic_unwrap" {
                    // MIR already handled branching; this is the panic path.
                    if let Some(file_str) = source_file {
                        if let (Some(func_ref), Some(gv)) = (
                            func_refs.get("panic_unwrap_at"),
                            string_globals.get(file_str),
                        ) {
                            let file_ptr = builder.ins().global_value(types::I64, *gv);
                            let line_val = builder.ins().iconst(types::I32, current_line as i64);
                            let col_val = builder.ins().iconst(types::I32, current_col as i64);
                            builder.ins().call(*func_ref, &[file_ptr, line_val, col_val]);
                        } else {
                            let unwrap_fn = func_refs.get("panic_unwrap")
                                .ok_or_else(|| CodegenError::FunctionNotFound("panic_unwrap".into()))?;
                            builder.ins().call(*unwrap_fn, &[]);
                        }
                    } else {
                        let unwrap_fn = func_refs.get("panic_unwrap")
                            .ok_or_else(|| CodegenError::FunctionNotFound("panic_unwrap".into()))?;
                        builder.ins().call(*unwrap_fn, &[]);
                    }
                } else if func.name == "Ptr_add" || func.name == "Ptr_sub" || func.name == "Ptr_offset" {
                    // Pointer arithmetic: ptr.add(n) → ptr + n*8, ptr.sub(n) → ptr - n*8
                    // Hardcoded elem_size=8 (all values are i64 for now)
                    let ptr_val = Self::lower_operand(builder, &args[0], var_map, string_globals, func_refs)?;
                    let n_val = Self::lower_operand_typed(builder, &args[1], var_map, Some(types::I64), string_globals, func_refs)?;
                    let elem_size = builder.ins().iconst(types::I64, 8);
                    let byte_offset = builder.ins().imul(n_val, elem_size);
                    let result = if func.name == "Ptr_sub" {
                        builder.ins().isub(ptr_val, byte_offset)
                    } else {
                        builder.ins().iadd(ptr_val, byte_offset)
                    };
                    if let Some(dst_id) = dst {
                        if let Some(var) = var_map.get(dst_id) {
                            builder.def_var(*var, result);
                        }
                    }
                } else if func.name == "Ptr_is_null" {
                    // ptr.is_null() → ptr == 0 (returns I8 boolean)
                    let ptr_val = Self::lower_operand(builder, &args[0], var_map, string_globals, func_refs)?;
                    let result = builder.ins().icmp_imm(IntCC::Equal, ptr_val, 0);
                    if let Some(dst_id) = dst {
                        if let Some(var) = var_map.get(dst_id) {
                            builder.def_var(*var, result);
                        }
                    }
                } else if func.name == "Ptr_cast" {
                    // ptr.cast<U>() → identity (pointer is always i64)
                    let ptr_val = Self::lower_operand(builder, &args[0], var_map, string_globals, func_refs)?;
                    if let Some(dst_id) = dst {
                        if let Some(var) = var_map.get(dst_id) {
                            builder.def_var(*var, ptr_val);
                        }
                    }
                } else if func.is_extern {
                    // Extern "C" call — use declared signature directly, no stdlib adaptation
                    let func_ref = func_refs.get(&func.name)
                        .ok_or_else(|| CodegenError::FunctionNotFound(func.name.clone()))?;

                    // Read declared signature to get expected param types
                    let ext_func = &builder.func.dfg.ext_funcs[*func_ref];
                    let sig = &builder.func.dfg.signatures[ext_func.signature];
                    let param_types: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();

                    let mut arg_vals = Vec::with_capacity(args.len());
                    for (i, a) in args.iter().enumerate() {
                        let expected = param_types.get(i).copied();
                        let val = Self::lower_operand_typed(builder, a, var_map, expected, string_globals, func_refs)?;
                        let actual = builder.func.dfg.value_type(val);
                        if let Some(exp) = expected {
                            if actual != exp {
                                arg_vals.push(Self::convert_value(builder, val, actual, exp));
                            } else {
                                arg_vals.push(val);
                            }
                        } else {
                            arg_vals.push(val);
                        }
                    }

                    let call_inst = builder.ins().call(*func_ref, &arg_vals);

                    if let Some(dst_id) = dst {
                        let dst_local = locals.iter().find(|l| l.id == *dst_id);
                        let is_void = matches!(dst_local.map(|l| &l.ty), Some(MirType::Void));
                        if !is_void {
                            let var = var_map.get(dst_id)
                                .ok_or_else(|| CodegenError::UnsupportedFeature(
                                    "Call destination variable not found".to_string()
                                ))?;
                            let results = builder.inst_results(call_inst);
                            let val = if !results.is_empty() {
                                let dst_local = locals.iter().find(|l| l.id == *dst_id);
                                let result = results[0];
                                if let Some(local) = dst_local {
                                    let dst_ty = mir_to_cranelift_type(&local.ty)?;
                                    let val_ty = builder.func.dfg.value_type(result);
                                    if val_ty != dst_ty {
                                        Self::convert_value(builder, result, val_ty, dst_ty)
                                    } else {
                                        result
                                    }
                                } else {
                                    result
                                }
                            } else {
                                builder.ins().iconst(types::I64, 0)
                            };
                            if let Some((ss, _size)) = stack_slot_map.get(dst_id) {
                                // Extern C functions return plain values; wrap in Ok for Result destinations
                                Self::wrap_ok_into_slot(builder, val, *ss);
                            } else {
                                builder.def_var(*var, val);
                            }
                        }
                    }
                } else {
                    let func_ref = func_refs.get(&func.name)
                        .ok_or_else(|| CodegenError::FunctionNotFound(func.name.clone()))?;

                    // Lower MIR args to Cranelift values
                    let mut arg_vals = Vec::with_capacity(args.len());
                    for (arg_idx, a) in args.iter().enumerate() {
                        // string_append_cstr: second arg is raw char*, skip RaskString wrapping
                        let val = if func.name == "string_append_cstr" && arg_idx == 1 {
                            Self::lower_string_const_as_cstr(builder, a, string_globals)?
                        } else {
                            Self::lower_operand_typed(builder, a, var_map, Some(types::I64), string_globals, func_refs)?
                        };
                        let actual = builder.func.dfg.value_type(val);
                        let converted = if actual != types::I64 && actual.is_int() {
                            Self::convert_value(builder, val, actual, types::I64)
                        } else {
                            val
                        };
                        arg_vals.push(converted);
                    }

                    // Adapt args for typed runtime API
                    let adapt = Self::adapt_stdlib_call(builder, &func.name, &mut arg_vals, args, locals, struct_layouts);

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

                    // Store source location before calling panicking functions
                    if panicking_fns.contains(&func.name) {
                        if let Some(file_str) = source_file {
                            if let (Some(set_loc_fn), Some(gv)) = (
                                func_refs.get("set_panic_location"),
                                string_globals.get(file_str),
                            ) {
                                let file_ptr = builder.ins().global_value(types::I64, *gv);
                                let line_val = builder.ins().iconst(types::I32, current_line as i64);
                                let col_val = builder.ins().iconst(types::I32, current_col as i64);
                                builder.ins().call(*set_loc_fn, &[file_ptr, line_val, col_val]);
                            }
                        }
                    }

                    let call_inst = builder.ins().call(*func_ref, &arg_vals);

                    if let Some(dst_id) = dst {
                        // Skip void-typed destinations — nothing to store
                        let dst_local = locals.iter().find(|l| l.id == *dst_id);
                        let is_void = matches!(dst_local.map(|l| &l.ty), Some(MirType::Void));

                        if !is_void {
                        let var = var_map.get(dst_id)
                            .ok_or_else(|| CodegenError::UnsupportedFeature(
                                "Call destination variable not found".to_string()
                            ))?;

                        // Post-call result handling
                        let val = match adapt {
                            CallAdapt::DerefResult => {
                                // Result is void* — load the value from it.
                                // Use the destination type so f64 elements load as f64,
                                // not as i64 bit patterns that need conversion.
                                let load_ty = dst_local
                                    .and_then(|l| mir_to_cranelift_type(&l.ty).ok())
                                    .unwrap_or(types::I64);
                                let results = builder.inst_results(call_inst);
                                if !results.is_empty() {
                                    let ptr = results[0];
                                    builder.ins().load(load_ty, MemFlags::new(), ptr, 0)
                                } else {
                                    builder.ins().iconst(types::I64, 0)
                                }
                            }
                            CallAdapt::PopOutParam(ss) => {
                                // Value was written to stack slot by callee
                                builder.ins().stack_load(types::I64, ss, 0)
                            }
                            CallAdapt::WrapOkResult(ss) => {
                                // Wrap raw return value as Ok(value) in Result stack slot
                                let results = builder.inst_results(call_inst);
                                let raw_val = if !results.is_empty() {
                                    results[0]
                                } else {
                                    builder.ins().iconst(types::I64, 0)
                                };
                                // tag=0 (Ok) at offset 0
                                let tag = builder.ins().iconst(types::I64, 0);
                                builder.ins().stack_store(tag, ss, 0);
                                // payload at offset 8
                                builder.ins().stack_store(raw_val, ss, 8);
                                // Return pointer to the stack slot
                                builder.ins().stack_addr(types::I64, ss, 0)
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
                        // If destination has a stack slot (aggregate type), handle differently
                        // for internal Rask functions vs C stdlib functions.
                        if let Some((ss, size)) = stack_slot_map.get(dst_id) {
                            if internal_fns.contains(&func.name) {
                                // Internal function returns a pointer to its stack-allocated aggregate.
                                // Copy the data into our own stack slot before it goes stale.
                                Self::copy_aggregate(builder, final_val, *ss, *size);
                            } else {
                                // C stdlib function returns a plain value (not a pointer to an aggregate).
                                // Wrap it as Ok(value) in the destination Result slot.
                                Self::wrap_ok_into_slot(builder, final_val, *ss);
                            }
                        } else {
                            builder.def_var(*var, final_val);
                        }
                        } // !is_void
                    }
                }
            }

            MirStmt::SourceLocation { .. } => {
                // Source location tracking handled elsewhere
            }

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
                let pool_val = builder.use_var(*var_map.get(pool)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Pool variable not found".to_string()
                    ))?);
                let handle_val = builder.use_var(*var_map.get(handle)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Handle variable not found".to_string()
                    ))?);

                // Use location-aware function when source info is available
                let call_inst = if let Some(file_str) = source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        func_refs.get("pool_get_checked"),
                        string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, current_col as i64);
                        builder.ins().call(*func_ref, &[pool_val, handle_val, file_ptr, line_val, col_val])
                    } else {
                        let func_ref = func_refs.get("Pool_checked_access")
                            .ok_or_else(|| CodegenError::FunctionNotFound("Pool_checked_access".to_string()))?;
                        builder.ins().call(*func_ref, &[pool_val, handle_val])
                    }
                } else {
                    let func_ref = func_refs.get("Pool_checked_access")
                        .ok_or_else(|| CodegenError::FunctionNotFound("Pool_checked_access".to_string()))?;
                    builder.ins().call(*func_ref, &[pool_val, handle_val])
                };

                let results = builder.inst_results(call_inst);
                if !results.is_empty() {
                    let ptr = results[0]; // copy before mutable borrow
                    let var = var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature(
                            "Pool access destination not found".to_string()
                        ))?;
                    // Pool stores actual element data inline. pool_get returns
                    // void* pointing directly into the pool's data array.
                    // For struct elements, use this pointer directly (it IS the
                    // struct address). For scalar elements, load the value.
                    let dst_ty = locals.iter().find(|l| l.id == *dst).map(|l| &l.ty);
                    let is_struct = matches!(dst_ty, Some(MirType::Struct(_)));
                    if is_struct {
                        // Struct: pointer to pool data IS the struct address
                        builder.def_var(*var, ptr);
                    } else {
                        let load_ty = dst_ty
                            .and_then(|t| mir_to_cranelift_type(t).ok())
                            .unwrap_or(types::I64);
                        let val = builder.ins().load(load_ty, MemFlags::new(), ptr, 0);
                        builder.def_var(*var, val);
                    }
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

            MirStmt::GlobalRef { dst, name } => {
                let gv = comptime_globals.get(name.as_str())
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        format!("GlobalRef: comptime global '{}' not found", name)
                    ))?;
                let addr = builder.ins().global_value(types::I64, *gv);
                let var = var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "GlobalRef destination not found".to_string()
                    ))?;
                builder.def_var(*var, addr);
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

                let is_float = lhs_ty.is_float() || rhs_ty.is_float();

                // Check if the left operand has an unsigned MIR type
                let is_unsigned = Self::operand_mir_type(left, locals)
                    .map(|t| t.is_unsigned())
                    .unwrap_or(false);

                // Reconcile operand types
                let (lhs_val, rhs_val) = if lhs_ty == rhs_ty {
                    (lhs_val, rhs_val)
                } else if lhs_ty.is_int() && rhs_ty.is_int() {
                    // Widen narrower integer
                    if lhs_ty.bits() < rhs_ty.bits() {
                        (Self::convert_value(builder, lhs_val, lhs_ty, rhs_ty), rhs_val)
                    } else {
                        (lhs_val, Self::convert_value(builder, rhs_val, rhs_ty, lhs_ty))
                    }
                } else if lhs_ty.is_float() && rhs_ty.is_float() {
                    // Promote narrower float
                    if lhs_ty.bits() < rhs_ty.bits() {
                        (builder.ins().fpromote(rhs_ty, lhs_val), rhs_val)
                    } else {
                        (lhs_val, builder.ins().fpromote(lhs_ty, rhs_val))
                    }
                } else if lhs_ty.is_int() && rhs_ty.is_float() {
                    // Convert int to float to match rhs
                    (builder.ins().fcvt_from_sint(rhs_ty, lhs_val), rhs_val)
                } else if lhs_ty.is_float() && rhs_ty.is_int() {
                    // Convert int to float to match lhs
                    (lhs_val, builder.ins().fcvt_from_sint(lhs_ty, rhs_val))
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
                        BinOp::And => builder.ins().band(lhs_val, rhs_val),
                        BinOp::Or => builder.ins().bor(lhs_val, rhs_val),
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
                        BinOp::And => builder.ins().band(lhs_val, rhs_val),
                        BinOp::Or => builder.ins().bor(lhs_val, rhs_val),
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
            MirRValue::Field { base, field_index, byte_offset, field_size } => {
                let base_val = Self::lower_operand(builder, base, var_map, string_globals, func_refs)?;
                let base_ty = Self::operand_mir_type(base, locals);
                let load_ty = expected_ty.unwrap_or(types::I64);

                let offset = match &base_ty {
                    Some(MirType::Struct(id)) => {
                        if let Some(layout) = struct_layouts.get(id.0 as usize) {
                            if let Some(field) = layout.fields.get(*field_index as usize) {
                                // Aggregate field (embedded struct): return pointer, don't load
                                if field.size > 8 {
                                    let addr = builder.ins().iadd_imm(base_val, field.offset as i64);
                                    return Ok(addr);
                                }
                                field.offset as i32
                            } else {
                                0
                            }
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
                    // Tuple: compute offset from element types
                    Some(MirType::Tuple(fields)) => {
                        let mut off = 0u32;
                        for (i, f) in fields.iter().enumerate() {
                            let align = f.align();
                            off = (off + align - 1) & !(align - 1);
                            if i == *field_index as usize {
                                break;
                            }
                            off += f.size();
                        }
                        off as i32
                    }
                    // Option/Result: payload starts after 8-byte tag.
                    // MIR uses EnumTag for the tag; Field indices are payload-relative.
                    Some(MirType::Option(inner)) => {
                        // Aggregate payload (struct/enum): return address, not load
                        if matches!(inner.as_ref(), MirType::Struct(_) | MirType::Enum(_)) {
                            let payload_addr = builder.ins().iadd_imm(base_val, 8);
                            return Ok(payload_addr);
                        }
                        (8 + *field_index * 8) as i32
                    }
                    Some(MirType::Result { ok, .. }) => {
                        // Aggregate Ok payload: return address, not load
                        if *field_index == 0 && matches!(ok.as_ref(), MirType::Struct(_) | MirType::Enum(_)) {
                            let payload_addr = builder.ins().iadd_imm(base_val, 8);
                            return Ok(payload_addr);
                        }
                        (8 + *field_index * 8) as i32
                    }
                    // Fallback: use pre-computed byte offset from MIR when available
                    _ => byte_offset.map(|o| o as i32).unwrap_or((*field_index * 8) as i32)
                };

                // Aggregate field (embedded struct, size > 8): return pointer, don't load
                if field_size.map_or(false, |s| s > 8) {
                    let addr = builder.ins().iadd_imm(base_val, offset as i64);
                    return Ok(addr);
                }

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
                            let (tag_size, _) = rask_mono::type_size_align(&layout.tag_ty, &Default::default());
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
                    Some(MirType::Struct(_) | MirType::Enum(_) | MirType::Array { .. }
                         | MirType::Tuple(_) | MirType::Slice(_) | MirType::Option(_)
                         | MirType::Result { .. } | MirType::Union(_))
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

            // Array element access: base_ptr + index * elem_size → load
            MirRValue::ArrayIndex { base, index, elem_size } => {
                let base_val = Self::lower_operand(builder, base, var_map, string_globals, func_refs)?;
                let idx_val = Self::lower_operand_typed(builder, index, var_map, Some(types::I64), string_globals, func_refs)?;
                let elem_sz = builder.ins().iconst(types::I64, *elem_size as i64);
                let offset = builder.ins().imul(idx_val, elem_sz);
                let addr = builder.ins().iadd(base_val, offset);
                let load_ty = expected_ty.unwrap_or(types::I64);
                let flags = MemFlags::new();
                Ok(builder.ins().load(load_ty, flags, addr, 0))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
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
        comptime_globals: &HashMap<String, GlobalValue>,
        panicking_fns: &HashSet<String>,
        stack_slot_map: &HashMap<LocalId, (StackSlot, u32)>,
        internal_fns: &HashSet<String>,
        cleanup_chain_blocks: &HashMap<Vec<BlockId>, cranelift_codegen::ir::Block>,
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
                if !cleanup_chain.is_empty() {
                    if let Some(&shared_block) = cleanup_chain_blocks.get(cleanup_chain) {
                        // Jump to shared cleanup block, passing return value.
                        if let Some(val_op) = value {
                            let expected_ty = mir_to_cranelift_type(ret_ty)?;
                            let val = Self::lower_operand_typed(builder, val_op, var_map, Some(expected_ty), string_globals, func_refs)?;
                            let actual_ty = builder.func.dfg.value_type(val);
                            let final_val = if actual_ty != expected_ty {
                                Self::convert_value(builder, val, actual_ty, expected_ty)
                            } else {
                                val
                            };
                            builder.ins().jump(shared_block, &[final_val]);
                        } else {
                            builder.ins().jump(shared_block, &[]);
                        }
                    } else {
                        // Fallback: inline (shouldn't happen with the setup above)
                        Self::emit_return(builder, value.as_ref(), ret_ty, var_map, string_globals, func_refs)?;
                    }
                } else {
                    // Empty cleanup chain — just return directly
                    Self::emit_return(builder, value.as_ref(), ret_ty, var_map, string_globals, func_refs)?;
                }
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

    /// Compute the actual allocation size for a MirType, resolving struct/enum
    /// sizes from layouts. Unlike MirType::size() which returns 8 for Struct/Enum
    /// (pointer size), this returns the true layout size. Needed for stack slots
    /// that store aggregate values inline (Result<Struct, Enum>, Option<Struct>, etc.).
    fn resolve_type_alloc_size(
        ty: &MirType,
        struct_layouts: &[StructLayout],
        enum_layouts: &[EnumLayout],
    ) -> Option<u32> {
        match ty {
            MirType::Struct(id) => struct_layouts.get(id.0 as usize).map(|sl| sl.size),
            MirType::Enum(id) => enum_layouts.get(id.0 as usize).map(|el| el.size),
            MirType::Array { elem, len } => Some(elem.size() * len),
            MirType::Result { ok, err } => {
                let ok_size = Self::resolve_type_alloc_size(ok, struct_layouts, enum_layouts)
                    .unwrap_or(ok.size());
                let err_size = Self::resolve_type_alloc_size(err, struct_layouts, enum_layouts)
                    .unwrap_or(err.size());
                Some(8 + ok_size.max(err_size))
            }
            MirType::Option(inner) => {
                let inner_size = Self::resolve_type_alloc_size(inner, struct_layouts, enum_layouts)
                    .unwrap_or(inner.size());
                Some(8 + inner_size)
            }
            MirType::Tuple(fields) => {
                let mut offset = 0u32;
                for f in fields {
                    let f_size = Self::resolve_type_alloc_size(f, struct_layouts, enum_layouts)
                        .unwrap_or(f.size());
                    let align = f.align();
                    offset = (offset + align - 1) & !(align - 1);
                    offset += f_size;
                }
                let max_align = fields.iter().map(|f| f.align()).max().unwrap_or(1);
                Some((offset + max_align - 1) & !(max_align - 1))
            }
            MirType::Slice(_) => Some(ty.size()),
            MirType::Union(variants) => {
                let max = variants.iter()
                    .map(|v| Self::resolve_type_alloc_size(v, struct_layouts, enum_layouts)
                        .unwrap_or(v.size()))
                    .max()
                    .unwrap_or(0);
                Some(max)
            }
            _ => None,
        }
    }

    /// Copy aggregate data from a source pointer into a caller-owned stack slot.
    /// Emits 8-byte load/store pairs. Used after calls that return aggregate types
    /// (struct, enum, Result, etc.) to avoid dangling pointers to callee stack frames.
    fn copy_aggregate(builder: &mut ClifFunctionBuilder, src_ptr: Value, dst_slot: StackSlot, size: u32) {
        let mut offset = 0i32;
        while (offset as u32) + 8 <= size {
            let val = builder.ins().load(types::I64, MemFlags::new(), src_ptr, offset);
            builder.ins().stack_store(val, dst_slot, offset);
            offset += 8;
        }
        // Handle trailing bytes (1-7 remaining)
        let remaining = size as i32 - offset;
        if remaining >= 4 {
            let val = builder.ins().load(types::I32, MemFlags::new(), src_ptr, offset);
            builder.ins().stack_store(val, dst_slot, offset);
            offset += 4;
        }
        if (size as i32 - offset) >= 2 {
            let val = builder.ins().load(types::I16, MemFlags::new(), src_ptr, offset);
            builder.ins().stack_store(val, dst_slot, offset);
            offset += 2;
        }
        if (size as i32 - offset) >= 1 {
            let val = builder.ins().load(types::I8, MemFlags::new(), src_ptr, offset);
            builder.ins().stack_store(val, dst_slot, offset);
        }
    }

    /// Wrap a plain return value as Ok(value) in a Result stack slot.
    /// Stores tag=0 (Ok) at offset 0, payload at offset 8.
    fn wrap_ok_into_slot(builder: &mut ClifFunctionBuilder, value: Value, dst_slot: StackSlot) {
        let tag = builder.ins().iconst(types::I64, 0);
        builder.ins().stack_store(tag, dst_slot, 0);
        builder.ins().stack_store(value, dst_slot, 8);
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
        mir_args: &[MirOperand],
        locals: &[rask_mir::MirLocal],
        struct_layouts: &[StructLayout],
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
            "Vec_push" => {
                // args: [vec, value] → [vec, &value]
                if args.len() >= 2 {
                    let val = args[1];
                    args[1] = Self::value_to_ptr(builder, val);
                }
                CallAdapt::None
            }
            "Vec_set" => {
                // args: [vec, index, value] → [vec, index, &value]
                if args.len() >= 3 {
                    let val = args[2];
                    args[2] = Self::value_to_ptr(builder, val);
                }
                CallAdapt::None
            }

            // Vec pop: add out-param, load result from it
            "Vec_pop" => {
                // args: [vec] → [vec, &out]
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.push(addr);
                CallAdapt::PopOutParam(ss)
            }

            // Vec remove_at: add out-param for the removed element
            "Vec_remove" => {
                // args: [vec, index] → [vec, index, &out]
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.push(addr);
                CallAdapt::PopOutParam(ss)
            }

            // Vec get/index: result is void*, deref to get value
            "Vec_get" | "Vec_index" | "index" => CallAdapt::DerefResult,

            // Map insert: wrap key and value as pointers
            "Map_insert" => {
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
            "Map_contains_key" | "Map_remove" => {
                if args.len() >= 2 {
                    let key = args[1];
                    args[1] = Self::value_to_ptr(builder, key);
                }
                CallAdapt::None
            }

            // Map get: wrap key as pointer, deref result
            "Map_get" => {
                if args.len() >= 2 {
                    let key = args[1];
                    args[1] = Self::value_to_ptr(builder, key);
                }
                CallAdapt::DerefResult
            }

            // Pool insert: pass element pointer + size, wrap return as Ok Result
            "Pool_insert" => {
                // Determine element size from MIR type
                let elem_size = if mir_args.len() >= 2 {
                    let elem_ty = Self::operand_mir_type(&mir_args[1], locals);
                    elem_ty.and_then(|t| Self::resolve_type_alloc_size(
                        &t, struct_layouts, &[], // no enum_layouts needed
                    )).unwrap_or(8) as i64
                } else {
                    8i64
                };

                // args: [pool, value] → [pool, &value, elem_size]
                if args.len() >= 2 {
                    if elem_size <= 8 {
                        // Scalar: wrap in pointer
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                    // For structs (elem_size > 8), args[1] is already a pointer
                    // to the stack-allocated struct data — pass directly
                }
                let size_val = builder.ins().iconst(types::I64, elem_size);
                args.push(size_val);

                // Pool.insert returns raw handle i64, but Rask expects Handle or Error
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 16, 0, // tag(8) + payload(8)
                ));
                CallAdapt::WrapOkResult(ss)
            }

            // Vec insert: wrap value arg as pointer
            "Vec_insert" => {
                // args: [vec, index, value] → [vec, index, &value]
                if args.len() >= 3 {
                    let val = args[2];
                    args[2] = Self::value_to_ptr(builder, val);
                }
                CallAdapt::None
            }

            // Pool get/index: result is void*, deref to get value
            "Pool_get" | "Pool_index" | "Pool_checked_access" => CallAdapt::DerefResult,

            // Channel_unbuffered: MIR has no args, C expects capacity=0
            "Channel_unbuffered" => {
                let zero = builder.ins().iconst(types::I64, 0);
                args.push(zero);
                CallAdapt::None
            }

            // Atomic CAS: add out_ok pointer param
            _ if func_name.contains("_compare_exchange") => {
                // args: [ptr, expected, desired, success_ord, fail_ord]
                // → [ptr, expected, desired, success_ord, fail_ord, &out_ok]
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.push(addr);
                CallAdapt::PopOutParam(ss)
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

    /// Lower a string constant as a raw `const char*` pointer (no RaskString wrapping).
    /// Used by `string_append_cstr` to avoid allocating a temporary RaskString.
    fn lower_string_const_as_cstr(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        string_globals: &HashMap<String, GlobalValue>,
    ) -> CodegenResult<Value> {
        if let MirOperand::Constant(MirConst::String(s)) = op {
            if let Some(gv) = string_globals.get(s.as_str()) {
                return Ok(builder.ins().global_value(types::I64, *gv));
            }
        }
        // Shouldn't reach here — transform only emits cstr variant for constants
        Ok(builder.ins().iconst(types::I64, 0))
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
                        let ty = expected_ty.unwrap_or(types::I64);
                        Ok(builder.ins().iconst(ty, *n))
                    }
                    MirConst::Float(f) => {
                        // Only use expected_ty if it's a float type; ignore int expected types
                        let ty = match expected_ty {
                            Some(t) if t.is_float() => t,
                            _ => types::F64,
                        };
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
                                return Err(CodegenError::FunctionNotFound("string_from".to_string()))
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
