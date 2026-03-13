// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function builder — lowers MIR to Cranelift IR.

use cranelift::prelude::*;
use cranelift_codegen::ir::{FuncRef, Function, GlobalValue, InstBuilder, MemFlags, StackSlot, StackSlotData, StackSlotKind};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_frontend::{FunctionBuilder as ClifFunctionBuilder, FunctionBuilderContext};
use std::collections::{HashMap, HashSet};

use rask_mir::{BinOp, BlockId, LocalId, MirConst, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType, UnaryOp};
use rask_mono::{StructLayout, EnumLayout};
use rask_types::Type as RaskType;
use crate::types::mir_to_cranelift_type;
use crate::{BuildMode, CodegenError, CodegenResult};

/// Read-only context bundling parameters for lowering functions.
struct CodegenCtx<'a> {
    var_map: &'a HashMap<LocalId, Variable>,
    locals: &'a [rask_mir::MirLocal],
    func_refs: &'a HashMap<String, FuncRef>,
    struct_layouts: &'a [StructLayout],
    enum_layouts: &'a [EnumLayout],
    string_globals: &'a HashMap<String, GlobalValue>,
    comptime_globals: &'a HashMap<String, GlobalValue>,
    vtable_globals: &'a HashMap<String, GlobalValue>,
    panicking_fns: &'a HashSet<String>,
    internal_fns: &'a HashSet<String>,
    stack_slot_map: &'a HashMap<LocalId, (StackSlot, u32)>,
    block_map: &'a HashMap<BlockId, Block>,
    build_mode: BuildMode,
    source_file: Option<&'a str>,
    current_line: u32,
    current_col: u32,
    ret_ty: &'a MirType,
}

/// Result of adapting a stdlib call for the typed runtime API.
enum CallAdapt {
    /// No special post-call handling needed
    None,
    /// Result is void* — load the i64 value from the returned pointer
    DerefResult,
    /// Result is void* — wrap as Option: NULL→None(tag=1), non-NULL→Some(tag=0, deref)
    DerefOption,
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
    /// Comptime global data (const name → GlobalValue for the data address)
    comptime_globals: &'a HashMap<String, GlobalValue>,
    /// VTable data globals (vtable name → GlobalValue for the vtable address)
    vtable_globals: &'a HashMap<String, GlobalValue>,
    /// MIR names of stdlib functions that can panic at runtime
    panicking_fns: &'a HashSet<String>,
    /// Names of functions compiled as Rask code (vs C stdlib)
    internal_fns: &'a HashSet<String>,
    /// Debug vs Release — controls whether pool access is inlined
    build_mode: BuildMode,

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
        vtable_globals: &'a HashMap<String, GlobalValue>,
        panicking_fns: &'a HashSet<String>,
        internal_fns: &'a HashSet<String>,
        build_mode: BuildMode,
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
            vtable_globals,
            panicking_fns,
            internal_fns,
            build_mode,
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
                if let MirTerminatorKind::CleanupReturn { cleanup_chain, .. } = &b.terminator.kind {
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
            if let MirTerminatorKind::CleanupReturn { cleanup_chain, .. } = &mir_block.terminator.kind {
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

        let mut ctx = CodegenCtx {
            var_map: &self.var_map,
            locals: &self.mir_fn.locals,
            func_refs: self.func_refs,
            struct_layouts: self.struct_layouts,
            enum_layouts: self.enum_layouts,
            string_globals: self.string_globals,
            comptime_globals: self.comptime_globals,
            vtable_globals: self.vtable_globals,
            panicking_fns: self.panicking_fns,
            internal_fns: self.internal_fns,
            stack_slot_map: &self.stack_slot_map,
            block_map: &self.block_map,
            build_mode: self.build_mode,
            source_file: self.mir_fn.source_file.as_deref(),
            current_line: self.current_line,
            current_col: self.current_col,
            ret_ty: &self.mir_fn.ret_ty,
        };

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
                Self::lower_stmt(&mut builder, stmt, &ctx)?;
            }

            // Lower terminator
            Self::lower_terminator(&mut builder, &mir_block.terminator, &ctx, &cleanup_chain_blocks)?;
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

            let cleanup_ctx = CodegenCtx {
                source_file: None,
                current_line: 0,
                current_col: 0,
                ..ctx
            };
            // Emit cleanup statements from each block in the chain
            for block_id in chain {
                if let Some(mir_block) = self.mir_fn.blocks.iter().find(|b| b.id == *block_id) {
                    for stmt in &mir_block.statements {
                        Self::lower_stmt(&mut builder, stmt, &cleanup_ctx)?;
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

    fn lower_stmt(
        builder: &mut ClifFunctionBuilder,
        stmt: &MirStmt,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue } => {
                let dst_local = ctx.locals.iter().find(|l| l.id == *dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Destination variable not found".to_string()))?;
                let dst_ty = mir_to_cranelift_type(&dst_local.ty)?;

                let mut val = Self::lower_rvalue(builder, rvalue, Some(dst_ty), ctx)?;

                let val_ty = builder.func.dfg.value_type(val);
                if val_ty != dst_ty {
                    val = Self::convert_value(builder, val, val_ty, dst_ty);
                }

                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                builder.def_var(*var, val);
            }

            MirStmtKind::Store { addr, offset, value, store_size } => {
                let addr_val = builder.use_var(*ctx.var_map.get(addr)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Address variable not found".to_string()))?);

                // If the value is a stack-allocated aggregate (struct/enum), copy its
                // data instead of storing the pointer. This handles Ok(struct_val) where
                // the struct data must be embedded in the Result's payload area.
                // Use the variable's current value (not the stack_slot address) because
                // the variable may alias another slot (e.g., p = struct_literal result).
                let is_aggregate = if let MirOperand::Local(src_id) = value {
                    if let Some((_src_slot, src_size)) = ctx.stack_slot_map.get(src_id) {
                        let src_var = ctx.var_map.get(src_id)
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
                    let val = Self::lower_operand(builder, value, ctx)?;
                    let val_ty = builder.func.dfg.value_type(val);

                    // Layout uses 8-byte slots for all scalars. Widen sub-8-byte
                    // values to fill the full slot — otherwise a 4-byte f32 store
                    // leaves stale upper bytes that corrupt the f64 read-back.
                    let val = if val_ty == types::F32 {
                        builder.ins().fpromote(types::F64, val)
                    } else if val_ty.is_int() && val_ty.bits() < 64 {
                        Self::convert_value(builder, val, val_ty, types::I64)
                    } else {
                        val
                    };

                    let flags = MemFlags::new();
                    builder.ins().store(flags, val, addr_val, *offset as i32);
                }
            }

            // Array element store: base_ptr[index * elem_size] = value
            MirStmtKind::ArrayStore { base, index, elem_size, value } => {
                let base_val = builder.use_var(*ctx.var_map.get(base)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("ArrayStore: base not found".to_string()))?);
                let idx_val = Self::lower_operand_typed(builder, index, Some(types::I64), ctx)?;
                let val = Self::lower_operand(builder, value, ctx)?;
                let elem_sz = builder.ins().iconst(types::I64, *elem_size as i64);
                let offset = builder.ins().imul(idx_val, elem_sz);
                let addr = builder.ins().iadd(base_val, offset);
                let flags = MemFlags::new();
                builder.ins().store(flags, val, addr, 0);
            }

            MirStmtKind::Call { dst, func, args } => {
                Self::lower_call(builder, dst.as_ref(), func, args, ctx)?;
            }

            // ── Resource tracking ──────────────────────────────────────
            // Calls C runtime functions for runtime must-consume checks.

            MirStmtKind::ResourceRegister { dst, scope_depth, .. } => {
                // rask_resource_register(scope_depth) → resource_id
                let func_ref = ctx.func_refs.get("rask_resource_register")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_register".to_string()))?;
                let depth_val = builder.ins().iconst(types::I64, *scope_depth as i64);
                let call_inst = builder.ins().call(*func_ref, &[depth_val]);

                let results = builder.inst_results(call_inst);
                if !results.is_empty() {
                    let var = ctx.var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature(
                            "Resource register destination not found".to_string()
                        ))?;
                    builder.def_var(*var, results[0]);
                }
            }

            MirStmtKind::ResourceConsume { resource_id } => {
                // rask_resource_consume(resource_id)
                let func_ref = ctx.func_refs.get("rask_resource_consume")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_consume".to_string()))?;
                let id_val = builder.use_var(*ctx.var_map.get(resource_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Resource ID variable not found".to_string()
                    ))?);
                builder.ins().call(*func_ref, &[id_val]);
            }

            MirStmtKind::ResourceScopeCheck { scope_depth } => {
                // rask_resource_scope_check(scope_depth)
                let func_ref = ctx.func_refs.get("rask_resource_scope_check")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_resource_scope_check".to_string()))?;
                let depth_val = builder.ins().iconst(types::I64, *scope_depth as i64);
                builder.ins().call(*func_ref, &[depth_val]);
            }

            // ── Cleanup stack ──────────────────────────────────────────
            // EnsurePush/Pop track the cleanup scope during MIR construction.
            // At codegen time, the cleanup chain is already materialized in
            // CleanupReturn terminators, so these are no-ops.
            MirStmtKind::EnsurePush { .. } | MirStmtKind::EnsurePop => {}

            // ── Pool checked access ────────────────────────────────────
            MirStmtKind::PoolCheckedAccess { dst, pool, handle } => {
                let pool_val = builder.use_var(*ctx.var_map.get(pool)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Pool variable not found".to_string()
                    ))?);
                let handle_val = builder.use_var(*ctx.var_map.get(handle)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Handle variable not found".to_string()
                    ))?);

                // Determine result type before emitting IR
                let is_struct = ctx.locals.iter()
                    .find(|l| l.id == *dst)
                    .map(|l| matches!(&l.ty, MirType::Struct(_)))
                    .unwrap_or(false);
                let load_ty = ctx.locals.iter()
                    .find(|l| l.id == *dst)
                    .and_then(|l| mir_to_cranelift_type(&l.ty).ok())
                    .unwrap_or(types::I64);

                if ctx.build_mode == BuildMode::Release {
                    // ── Inline pool access (release mode) ──────────────
                    // Emits bounds check + generation check + data load directly
                    // as Cranelift IR, avoiding the C function call overhead.
                    //
                    // Pool layout (verified by _Static_assert in pool.c):
                    //   offset 16: slot_stride (i64)
                    //   offset 24: cap (i64)
                    //   offset 40: slots (ptr)
                    // Slot layout (stride varies by elem_size):
                    //   offset 0: generation (u32)
                    //   offset 8: data (elem_size bytes)
                    const POOL_STRIDE_OFFSET: i32 = 16;
                    const POOL_CAP_OFFSET: i32 = 24;
                    const POOL_SLOTS_OFFSET: i32 = 40;
                    const SLOT_GEN_OFFSET: i32 = 0;
                    const SLOT_DATA_OFFSET: i32 = 8;

                    // 1. Extract index and generation from packed i64 handle
                    //    handle = index:32 | generation:32
                    let index = builder.ins().band_imm(handle_val, 0xFFFF_FFFF_i64);
                    let gen_i64 = builder.ins().ushr_imm(handle_val, 32);
                    let gen = builder.ins().ireduce(types::I32, gen_i64);

                    // 2. Bounds check: index < cap
                    let cap = builder.ins().load(types::I64, MemFlags::new(), pool_val, POOL_CAP_OFFSET);
                    let oob = builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, index, cap);

                    let panic_block = builder.create_block();
                    let bounds_ok = builder.create_block();
                    builder.ins().brif(oob, panic_block, &[], bounds_ok, &[]);

                    // panic_block: call rask_panic_at (single predecessor, seal immediately)
                    builder.switch_to_block(panic_block);
                    builder.seal_block(panic_block);
                    builder.set_cold_block(panic_block);
                    if let (Some(panic_ref), Some(msg_gv)) = (
                        ctx.func_refs.get("panic_at"),
                        ctx.string_globals.get("pool access with invalid handle"),
                    ) {
                        let file_gv = ctx.source_file.and_then(|f| ctx.string_globals.get(f));
                        let file_ptr = if let Some(gv) = file_gv {
                            builder.ins().global_value(types::I64, *gv)
                        } else {
                            builder.ins().iconst(types::I64, 0)
                        };
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        let msg_ptr = builder.ins().global_value(types::I64, *msg_gv);
                        builder.ins().call(*panic_ref, &[file_ptr, line_val, col_val, msg_ptr]);
                    }
                    builder.ins().trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));

                    // bounds_ok: load slots pointer and stride, compute slot address
                    builder.switch_to_block(bounds_ok);
                    builder.seal_block(bounds_ok);
                    let slots = builder.ins().load(types::I64, MemFlags::new(), pool_val, POOL_SLOTS_OFFSET);
                    let stride = builder.ins().load(types::I64, MemFlags::new(), pool_val, POOL_STRIDE_OFFSET);
                    let slot_offset = builder.ins().imul(index, stride);
                    let slot_addr = builder.ins().iadd(slots, slot_offset);

                    // 3. Generation check
                    let slot_gen = builder.ins().load(types::I32, MemFlags::new(), slot_addr, SLOT_GEN_OFFSET);
                    let gen_mismatch = builder.ins().icmp(IntCC::NotEqual, gen, slot_gen);

                    let gen_panic_block = builder.create_block();
                    let ok_block = builder.create_block();
                    builder.ins().brif(gen_mismatch, gen_panic_block, &[], ok_block, &[]);

                    // gen_panic_block: same panic (single predecessor, seal immediately)
                    builder.switch_to_block(gen_panic_block);
                    builder.seal_block(gen_panic_block);
                    builder.set_cold_block(gen_panic_block);
                    if let (Some(panic_ref), Some(msg_gv)) = (
                        ctx.func_refs.get("panic_at"),
                        ctx.string_globals.get("pool access with invalid handle"),
                    ) {
                        let file_gv = ctx.source_file.and_then(|f| ctx.string_globals.get(f));
                        let file_ptr = if let Some(gv) = file_gv {
                            builder.ins().global_value(types::I64, *gv)
                        } else {
                            builder.ins().iconst(types::I64, 0)
                        };
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        let msg_ptr = builder.ins().global_value(types::I64, *msg_gv);
                        builder.ins().call(*panic_ref, &[file_ptr, line_val, col_val, msg_ptr]);
                    }
                    builder.ins().trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));

                    // ok_block: load data (single predecessor, seal immediately)
                    builder.switch_to_block(ok_block);
                    builder.seal_block(ok_block);
                    let var = ctx.var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature(
                            "Pool access destination not found".to_string()
                        ))?;
                    // Always return pointer to slot data — pool[h] is used
                    // for mutation, so callers need the address.
                    let data_ptr = builder.ins().iadd_imm(slot_addr, SLOT_DATA_OFFSET as i64);
                    builder.def_var(*var, data_ptr);
                } else {
                    // ── Debug mode: call C function ──────────────────────
                    let call_inst = if let Some(file_str) = ctx.source_file {
                        if let (Some(func_ref), Some(gv)) = (
                            ctx.func_refs.get("pool_get_checked"),
                            ctx.string_globals.get(file_str),
                        ) {
                            let file_ptr = builder.ins().global_value(types::I64, *gv);
                            let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                            let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                            builder.ins().call(*func_ref, &[pool_val, handle_val, file_ptr, line_val, col_val])
                        } else {
                            let func_ref = ctx.func_refs.get("Pool_checked_access")
                                .ok_or_else(|| CodegenError::FunctionNotFound("Pool_checked_access".to_string()))?;
                            builder.ins().call(*func_ref, &[pool_val, handle_val])
                        }
                    } else {
                        let func_ref = ctx.func_refs.get("Pool_checked_access")
                            .ok_or_else(|| CodegenError::FunctionNotFound("Pool_checked_access".to_string()))?;
                        builder.ins().call(*func_ref, &[pool_val, handle_val])
                    };

                    let results = builder.inst_results(call_inst);
                    if !results.is_empty() {
                        let ptr = results[0];
                        let var = ctx.var_map.get(dst)
                            .ok_or_else(|| CodegenError::UnsupportedFeature(
                                "Pool access destination not found".to_string()
                            ))?;
                        // Always return raw pointer — pool[h] is used for
                        // mutation (pool[h].field = val), so callers need
                        // the address, not the loaded value.
                        builder.def_var(*var, ptr);
                    }
                }
            }

            // ── Closure support ──────────────────────────────────────────

            MirStmtKind::ClosureCreate { dst, func_name, captures, heap } => {
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
                let func_ref = ctx.func_refs.get(func_name)
                    .ok_or_else(|| CodegenError::FunctionNotFound(func_name.clone()))?;
                let func_ptr = builder.ins().func_addr(types::I64, *func_ref);

                let closure_ptr = if *heap {
                    // Escaping closure: heap-allocate via rask_alloc
                    let alloc_ref = ctx.func_refs.get("rask_alloc")
                        .ok_or_else(|| CodegenError::FunctionNotFound("rask_alloc".to_string()))?;
                    crate::closures::allocate_closure_heap(
                        builder, func_ptr, &env_layout, ctx.var_map, *alloc_ref,
                    )?
                } else {
                    // Non-escaping closure: stack-allocate
                    crate::closures::allocate_closure_stack(
                        builder, func_ptr, &env_layout, ctx.var_map,
                    )?
                };

                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "ClosureCreate destination not found".to_string()
                    ))?;
                builder.def_var(*var, closure_ptr);
            }

            MirStmtKind::ClosureCall { dst, closure, args } => {
                let closure_val = builder.use_var(*ctx.var_map.get(closure)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "Closure variable not found".to_string()
                    ))?);

                // Lower arg values
                let mut arg_vals = Vec::new();
                for a in args {
                    let val = Self::lower_operand(builder, a, ctx)?;
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
                    let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
                    if let Some(local) = dst_local {
                        let cl_ret_ty = mir_to_cranelift_type(&local.ty)?;
                        sig.returns.push(AbiParam::new(cl_ret_ty));
                    }
                }

                let call_inst = crate::closures::call_closure(
                    builder, closure_val, sig, &arg_vals,
                );

                if let Some(dst_id) = dst {
                    let results = builder.inst_results(call_inst);
                    if !results.is_empty() {
                        let var = ctx.var_map.get(dst_id)
                            .ok_or_else(|| CodegenError::UnsupportedFeature(
                                "ClosureCall destination not found".to_string()
                            ))?;
                        builder.def_var(*var, results[0]);
                    }
                }
            }

            MirStmtKind::LoadCapture { dst, env_ptr, offset } => {
                let env_val = builder.use_var(*ctx.var_map.get(env_ptr)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture env_ptr not found".to_string()
                    ))?);
                let dst_local = ctx.locals.iter().find(|l| l.id == *dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture destination not found".to_string()
                    ))?;
                let load_ty = mir_to_cranelift_type(&dst_local.ty)?;
                let val = crate::closures::load_capture(builder, env_val, *offset, load_ty);
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture destination variable not found".to_string()
                    ))?;
                builder.def_var(*var, val);
            }

            MirStmtKind::ClosureDrop { closure } => {
                let closure_val = builder.use_var(*ctx.var_map.get(closure)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "ClosureDrop closure variable not found".to_string()
                    ))?);
                let free_ref = ctx.func_refs.get("rask_free")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_free".to_string()))?;
                crate::closures::free_closure(builder, closure_val, *free_ref);
            }

            MirStmtKind::GlobalRef { dst, name } => {
                let gv = ctx.comptime_globals.get(name.as_str())
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        format!("GlobalRef: comptime global '{}' not found", name)
                    ))?;
                let addr = builder.ins().global_value(types::I64, *gv);
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "GlobalRef destination not found".to_string()
                    ))?;
                builder.def_var(*var, addr);
            }

            // ── Trait object support ──────────────────────────────────

            MirStmtKind::TraitBox { dst, value, vtable_name, concrete_size, .. } => {
                let alloc_ref = ctx.func_refs.get("rask_alloc")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_alloc".to_string()))?;

                // Allocate heap memory for the concrete value (min 8 to avoid null from zero-size alloc)
                let alloc_size = std::cmp::max(*concrete_size, 8) as i64;
                let size_val = builder.ins().iconst(types::I64, alloc_size);
                let call_inst = builder.ins().call(*alloc_ref, &[size_val]);
                let data_ptr = builder.inst_results(call_inst)[0];

                // Copy concrete value to heap
                if let MirOperand::Local(src_id) = value {
                    if let Some((ss, sz)) = ctx.stack_slot_map.get(src_id) {
                        // Aggregate: memcpy from stack slot
                        let src_ptr = builder.ins().stack_addr(types::I64, *ss, 0);
                        let mut off = 0i32;
                        while (off as u32) + 8 <= *sz {
                            let word = builder.ins().load(types::I64, MemFlags::new(), src_ptr, off);
                            builder.ins().store(MemFlags::new(), word, data_ptr, off);
                            off += 8;
                        }
                        if (off as u32) < *sz {
                            let word = builder.ins().load(types::I64, MemFlags::new(), src_ptr, off);
                            builder.ins().store(MemFlags::new(), word, data_ptr, off);
                        }
                    } else {
                        // Scalar: load from variable, store to heap
                        let src_val = builder.use_var(*ctx.var_map.get(src_id)
                            .ok_or_else(|| CodegenError::UnsupportedFeature(
                                "TraitBox: source variable not found".to_string()
                            ))?);
                        builder.ins().store(MemFlags::new(), src_val, data_ptr, 0);
                    }
                } else {
                    // Constant: lower and store
                    let src_val = Self::lower_operand(builder, value, ctx)?;
                    builder.ins().store(MemFlags::new(), src_val, data_ptr, 0);
                }

                // Get vtable address
                let gv = ctx.vtable_globals.get(vtable_name.as_str())
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        format!("TraitBox: vtable '{}' not found", vtable_name)
                    ))?;
                let vtable_ptr = builder.ins().global_value(types::I64, *gv);

                // Store fat pointer into destination stack slot: [data_ptr, vtable_ptr]
                let (ss, _) = ctx.stack_slot_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitBox destination stack slot not found".to_string()
                    ))?;
                let dst_addr = builder.ins().stack_addr(types::I64, *ss, 0);
                builder.ins().store(MemFlags::new(), data_ptr, dst_addr, 0);
                builder.ins().store(MemFlags::new(), vtable_ptr, dst_addr, 8);

                // Set the variable to point to the stack slot
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitBox destination variable not found".to_string()
                    ))?;
                builder.def_var(*var, dst_addr);
            }

            MirStmtKind::TraitCall { dst, trait_object, method_name, vtable_offset, args } => {
                // Load fat pointer components from trait object stack slot
                let obj_val = builder.use_var(*ctx.var_map.get(trait_object)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitCall: trait object variable not found".to_string()
                    ))?);
                let data_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, 0);
                let vtable_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, 8);

                // Load function pointer from vtable
                let func_ptr = builder.ins().load(
                    types::I64, MemFlags::new(), vtable_ptr, *vtable_offset as i32,
                );

                // Build signature: (data_ptr, args...) -> ret
                let mut sig = Signature::new(isa::CallConv::SystemV);
                sig.params.push(AbiParam::new(types::I64)); // data_ptr (self)
                for _ in args.iter() {
                    sig.params.push(AbiParam::new(types::I64));
                }
                sig.returns.push(AbiParam::new(types::I64));

                // Build argument values
                let mut call_args = Vec::with_capacity(1 + args.len());
                call_args.push(data_ptr);
                for arg in args.iter() {
                    let val = Self::lower_operand(builder, arg, ctx)?;
                    call_args.push(val);
                }

                let sig_ref = builder.import_signature(sig);
                let call_inst = builder.ins().call_indirect(sig_ref, func_ptr, &call_args);

                if let Some(dst_id) = dst {
                    let result = builder.inst_results(call_inst)[0];
                    let var = ctx.var_map.get(dst_id)
                        .ok_or_else(|| CodegenError::UnsupportedFeature(
                            format!("TraitCall destination for '{}' not found", method_name)
                        ))?;
                    builder.def_var(*var, result);
                }
            }

            MirStmtKind::TraitDrop { trait_object } => {
                let obj_val = builder.use_var(*ctx.var_map.get(trait_object)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitDrop: trait object variable not found".to_string()
                    ))?);

                // Load data_ptr and vtable_ptr
                let data_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, 0);
                let vtable_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, 8);

                // Load drop_fn from vtable offset 16
                let drop_fn = builder.ins().load(types::I64, MemFlags::new(), vtable_ptr, 16);

                // If drop_fn != null, call it
                let null = builder.ins().iconst(types::I64, 0);
                let is_null = builder.ins().icmp(IntCC::Equal, drop_fn, null);

                let drop_block = builder.create_block();
                let free_block = builder.create_block();

                builder.ins().brif(is_null, free_block, &[], drop_block, &[]);

                // Drop block: call drop_fn(data_ptr), then fall through to free
                builder.switch_to_block(drop_block);
                let mut drop_sig = Signature::new(isa::CallConv::SystemV);
                drop_sig.params.push(AbiParam::new(types::I64));
                let sig_ref = builder.import_signature(drop_sig);
                builder.ins().call_indirect(sig_ref, drop_fn, &[data_ptr]);
                builder.ins().jump(free_block, &[]);

                // Free block: rask_free(data_ptr)
                builder.switch_to_block(free_block);
                let free_ref = ctx.func_refs.get("rask_free")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_free".to_string()))?;
                builder.ins().call(*free_ref, &[data_ptr]);
            }

            MirStmtKind::Phi { .. } => {
                panic!("Phi nodes must be lowered by de-SSA before codegen");
            }
        }
        Ok(())
    }

    /// Lower a `MirStmtKind::Call` — dispatches builtins, extern calls, and regular calls.
    fn lower_call(
        builder: &mut ClifFunctionBuilder,
        dst: Option<&LocalId>,
        func: &rask_mir::FunctionRef,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
            // Builtin print/println — dispatch per-arg to typed runtime functions
            if func.name == "print" || func.name == "println" {
                for (i, a) in args.iter().enumerate() {
                    if i > 0 {
                        let sp = Self::lower_operand_typed(
                            builder, &MirOperand::Constant(MirConst::String(" ".to_string())),
                            Some(types::I64), ctx,
                        )?;
                        let print_str = ctx.func_refs.get("rask_print_string")
                            .ok_or_else(|| CodegenError::FunctionNotFound("rask_print_string".into()))?;
                        builder.ins().call(*print_str, &[sp]);
                    }
                    let runtime_fn = Self::runtime_print_for_operand(a, ctx.locals);
                    let fr = ctx.func_refs.get(runtime_fn)
                        .ok_or_else(|| CodegenError::FunctionNotFound(runtime_fn.into()))?;
                    // Get the expected param type from the runtime function's signature
                    let ext_func = &builder.func.dfg.ext_funcs[*fr];
                    let sig = &builder.func.dfg.signatures[ext_func.signature];
                    let expected_ty = sig.params.first().map(|p| p.value_type);
                    let mut val = Self::lower_operand_typed(builder, a, expected_ty, ctx)?;
                    if let Some(expected) = expected_ty {
                        let actual = builder.func.dfg.value_type(val);
                        if actual != expected {
                            val = Self::convert_value(builder, val, actual, expected);
                        }
                    }
                    builder.ins().call(*fr, &[val]);
                }
                if func.name == "println" {
                    let nl = ctx.func_refs.get("rask_print_newline")
                        .ok_or_else(|| CodegenError::FunctionNotFound("rask_print_newline".into()))?;
                    builder.ins().call(*nl, &[]);
                }
                // print/println return void — define dest as zero if needed
                if let Some(dst_id) = dst {
                    if let Some(var) = ctx.var_map.get(dst_id) {
                        let zero = builder.ins().iconst(types::I64, 0);
                        builder.def_var(*var, zero);
                    }
                }
            } else if func.name == "assert_fail" {
                // MIR already handled branching; this is the fail path.
                // Use location-aware variant when source info is available.
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get("assert_fail_at"),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[file_ptr, line_val, col_val]);
                    } else {
                        let assert_fn = ctx.func_refs.get("assert_fail")
                            .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                        builder.ins().call(*assert_fn, &[]);
                    }
                } else {
                    let assert_fn = ctx.func_refs.get("assert_fail")
                        .ok_or_else(|| CodegenError::FunctionNotFound("assert_fail".into()))?;
                    builder.ins().call(*assert_fn, &[]);
                }
            } else if func.name == "panic_unwrap" {
                // MIR already handled branching; this is the panic path.
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get("panic_unwrap_at"),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[file_ptr, line_val, col_val]);
                    } else {
                        let unwrap_fn = ctx.func_refs.get("panic_unwrap")
                            .ok_or_else(|| CodegenError::FunctionNotFound("panic_unwrap".into()))?;
                        builder.ins().call(*unwrap_fn, &[]);
                    }
                } else {
                    let unwrap_fn = ctx.func_refs.get("panic_unwrap")
                        .ok_or_else(|| CodegenError::FunctionNotFound("panic_unwrap".into()))?;
                    builder.ins().call(*unwrap_fn, &[]);
                }
            } else if func.name == "Ptr_add" || func.name == "Ptr_sub" || func.name == "Ptr_offset" {
                // Pointer arithmetic: ptr.add(n) → ptr + n*8, ptr.sub(n) → ptr - n*8
                // Hardcoded elem_size=8 (all values are i64 for now)
                let ptr_val = Self::lower_operand(builder, &args[0], ctx)?;
                let n_val = Self::lower_operand_typed(builder, &args[1], Some(types::I64), ctx)?;
                let elem_size = builder.ins().iconst(types::I64, 8);
                let byte_offset = builder.ins().imul(n_val, elem_size);
                let result = if func.name == "Ptr_sub" {
                    builder.ins().isub(ptr_val, byte_offset)
                } else {
                    builder.ins().iadd(ptr_val, byte_offset)
                };
                if let Some(dst_id) = dst {
                    if let Some(var) = ctx.var_map.get(dst_id) {
                        builder.def_var(*var, result);
                    }
                }
            } else if func.name == "Ptr_is_null" {
                // ptr.is_null() → ptr == 0 (returns I8 boolean)
                let ptr_val = Self::lower_operand(builder, &args[0], ctx)?;
                let result = builder.ins().icmp_imm(IntCC::Equal, ptr_val, 0);
                if let Some(dst_id) = dst {
                    if let Some(var) = ctx.var_map.get(dst_id) {
                        builder.def_var(*var, result);
                    }
                }
            } else if func.name == "Ptr_cast" {
                // ptr.cast<U>() → identity (pointer is always i64)
                let ptr_val = Self::lower_operand(builder, &args[0], ctx)?;
                if let Some(dst_id) = dst {
                    if let Some(var) = ctx.var_map.get(dst_id) {
                        builder.def_var(*var, ptr_val);
                    }
                }
            } else if func.is_extern {
                // Extern "C" call — use declared signature directly, no stdlib adaptation
                let func_ref = ctx.func_refs.get(&func.name)
                    .ok_or_else(|| CodegenError::FunctionNotFound(func.name.clone()))?;

                // Read declared signature to get expected param types
                let ext_func = &builder.func.dfg.ext_funcs[*func_ref];
                let sig = &builder.func.dfg.signatures[ext_func.signature];
                let param_types: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();

                let mut arg_vals = Vec::with_capacity(args.len());
                for (i, a) in args.iter().enumerate() {
                    let expected = param_types.get(i).copied();
                    let val = Self::lower_operand_typed(builder, a, expected, ctx)?;
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
                    let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
                    let is_void = matches!(dst_local.map(|l| &l.ty), Some(MirType::Void));
                    if !is_void {
                        let var = ctx.var_map.get(dst_id)
                            .ok_or_else(|| CodegenError::UnsupportedFeature(
                                "Call destination variable not found".to_string()
                            ))?;
                        let results = builder.inst_results(call_inst);
                        let val = if !results.is_empty() {
                            let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
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
                        if let Some((ss, _size)) = ctx.stack_slot_map.get(dst_id) {
                            // Extern C functions return plain values; wrap in Ok for Result destinations
                            Self::wrap_ok_into_slot(builder, val, *ss);
                        } else {
                            builder.def_var(*var, val);
                        }
                    }
                }
            } else {
                let func_ref = ctx.func_refs.get(&func.name)
                    .ok_or_else(|| CodegenError::FunctionNotFound(func.name.clone()))?;

                // Lower MIR args to Cranelift values
                let mut arg_vals = Vec::with_capacity(args.len());
                for (arg_idx, a) in args.iter().enumerate() {
                    // string_append_cstr: second arg is raw char*, skip RaskString wrapping
                    let val = if func.name == "string_append_cstr" && arg_idx == 1 {
                        Self::lower_string_const_as_cstr(builder, a, ctx)?
                    } else {
                        Self::lower_operand_typed(builder, a, Some(types::I64), ctx)?
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
                let adapt = Self::adapt_stdlib_call(builder, &func.name, &mut arg_vals, args, ctx);

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
                if ctx.panicking_fns.contains(&func.name) {
                    if let Some(file_str) = ctx.source_file {
                        if let (Some(set_loc_fn), Some(gv)) = (
                            ctx.func_refs.get("set_panic_location"),
                            ctx.string_globals.get(file_str),
                        ) {
                            let file_ptr = builder.ins().global_value(types::I64, *gv);
                            let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                            let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                            builder.ins().call(*set_loc_fn, &[file_ptr, line_val, col_val]);
                        }
                    }
                }

                let call_inst = builder.ins().call(*func_ref, &arg_vals);

                if let Some(dst_id) = dst {
                    // Skip void-typed destinations — nothing to store
                    let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
                    let is_void = matches!(dst_local.map(|l| &l.ty), Some(MirType::Void));

                    if !is_void {
                    let var = ctx.var_map.get(dst_id)
                        .ok_or_else(|| CodegenError::UnsupportedFeature(
                            "Call destination variable not found".to_string()
                        ))?;

                    // Post-call result handling
                    let mut slot_already_written = false;
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
                        CallAdapt::DerefOption => {
                            // Result is void*: NULL → None, non-NULL → Some(deref).
                            // Write tag+payload into the destination stack slot.
                            let results = builder.inst_results(call_inst);
                            let ptr = if !results.is_empty() { results[0] } else {
                                builder.ins().iconst(types::I64, 0)
                            };
                            if let Some((ss, slot_size)) = ctx.stack_slot_map.get(dst_id) {
                                slot_already_written = true;
                                let zero = builder.ins().iconst(types::I64, 0);
                                let is_null = builder.ins().icmp(IntCC::Equal, ptr, zero);
                                let then_block = builder.create_block();
                                let else_block = builder.create_block();
                                let merge_block = builder.create_block();
                                builder.ins().brif(is_null, then_block, &[], else_block, &[]);

                                // NULL path: tag = 1 (None)
                                builder.switch_to_block(then_block);
                                builder.seal_block(then_block);
                                let one = builder.ins().iconst(types::I64, 1);
                                builder.ins().stack_store(one, *ss, 0);
                                builder.ins().jump(merge_block, &[]);

                                // non-NULL path: tag = 0 (Some), payload copied from ptr
                                builder.switch_to_block(else_block);
                                builder.seal_block(else_block);
                                let tag_some = builder.ins().iconst(types::I64, 0);
                                builder.ins().stack_store(tag_some, *ss, 0);
                                // Copy payload: for scalars (slot_size=16) just load one word;
                                // for aggregates copy word-by-word from ptr into slot at offset 8+.
                                let payload_size = *slot_size as i32 - 8;
                                let mut off = 0i32;
                                while off + 8 <= payload_size {
                                    let word = builder.ins().load(types::I64, MemFlags::new(), ptr, off);
                                    builder.ins().stack_store(word, *ss, 8 + off);
                                    off += 8;
                                }
                                if payload_size - off >= 4 {
                                    let word = builder.ins().load(types::I32, MemFlags::new(), ptr, off);
                                    builder.ins().stack_store(word, *ss, 8 + off);
                                    off += 4;
                                }
                                if payload_size - off >= 2 {
                                    let word = builder.ins().load(types::I16, MemFlags::new(), ptr, off);
                                    builder.ins().stack_store(word, *ss, 8 + off);
                                    off += 2;
                                }
                                if payload_size - off >= 1 {
                                    let word = builder.ins().load(types::I8, MemFlags::new(), ptr, off);
                                    builder.ins().stack_store(word, *ss, 8 + off);
                                }
                                builder.ins().jump(merge_block, &[]);

                                builder.switch_to_block(merge_block);
                                builder.seal_block(merge_block);
                                // Return dummy value — real data is in the stack slot
                                builder.ins().iconst(types::I64, 0)
                            } else {
                                // No stack slot — just deref like DerefResult
                                builder.ins().load(types::I64, MemFlags::new(), ptr, 0)
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

                    let dst_local = ctx.locals.iter().find(|l| l.id == *dst_id);
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
                    // DerefOption already wrote directly to the stack slot.
                    if slot_already_written {
                        // Nothing to do — DerefOption already populated the slot
                    } else if let Some((ss, size)) = ctx.stack_slot_map.get(dst_id) {
                        if ctx.internal_fns.contains(&func.name) {
                            // Internal function returns aggregate data loaded from its stack.
                            // Store directly into our stack slot (value, not pointer).
                            if *size <= 8 {
                                builder.ins().stack_store(final_val, *ss, 0);
                            } else {
                                // Larger aggregates: copy from returned pointer
                                Self::copy_aggregate(builder, final_val, *ss, *size);
                            }
                        } else if Self::is_negative_err_fn(&func.name) {
                            // C function uses negative return = error convention.
                            Self::wrap_result_into_slot(builder, final_val, *ss);
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

    /// If the operand is a constant integer that's a power of 2, return the exponent.
    fn const_power_of_two(operand: &MirOperand) -> Option<u32> {
        if let MirOperand::Constant(MirConst::Int(n)) = operand {
            let n = *n;
            if n > 0 && (n & (n - 1)) == 0 {
                return Some(n.trailing_zeros());
            }
        }
        None
    }

    /// Look up the MirType of an operand from the locals table.
    fn operand_mir_type(operand: &MirOperand, locals: &[rask_mir::MirLocal]) -> Option<MirType> {
        match operand {
            MirOperand::Local(id) => locals.iter().find(|l| l.id == *id).map(|l| l.ty.clone()),
            MirOperand::Constant(_) => None,
        }
    }

    /// True when a struct field's declared type uses stack-slot (aggregate)
    /// representation in codegen. These fields return a pointer into the parent
    /// struct rather than a loaded scalar.
    fn is_aggregate_field_type(ty: &RaskType) -> bool {
        match ty {
            // Primitives, opaque pointers — scalar
            RaskType::Unit | RaskType::Bool
            | RaskType::I8 | RaskType::I16 | RaskType::I32 | RaskType::I64 | RaskType::I128
            | RaskType::U8 | RaskType::U16 | RaskType::U32 | RaskType::U64 | RaskType::U128
            | RaskType::F32 | RaskType::F64
            | RaskType::Char | RaskType::String
            | RaskType::Fn { .. } | RaskType::Slice(_) => false,
            // Runtime-opaque pointer types (Vec, Map, Pool, Handle, Channel, ...)
            RaskType::UnresolvedGeneric { .. } | RaskType::Generic { .. } => false,
            // Niche-optimized Option<Handle<T>> — scalar (sentinel value, no tag)
            RaskType::Option(inner)
                if matches!(inner.as_ref(), RaskType::UnresolvedGeneric { name, .. } if name == "Handle") =>
            {
                false
            }
            // User-defined enums/structs, tuples, arrays, Option, Result — aggregate
            _ => true,
        }
    }

    fn lower_rvalue(
        builder: &mut ClifFunctionBuilder,
        rvalue: &MirRValue,
        expected_ty: Option<Type>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        match rvalue {
            MirRValue::Use(op) => {
                Self::lower_operand_typed(builder, op, expected_ty, ctx)
            }

            MirRValue::BinaryOp { op, left, right } => {
                let is_comparison = matches!(op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                );

                let operand_ty = if is_comparison { None } else { expected_ty };
                let lhs_val = Self::lower_operand_typed(builder, left, operand_ty, ctx)?;
                let lhs_ty = builder.func.dfg.value_type(lhs_val);
                let rhs_val = Self::lower_operand_typed(builder, right, Some(lhs_ty), ctx)?;
                let rhs_ty = builder.func.dfg.value_type(rhs_val);

                let is_float = lhs_ty.is_float() || rhs_ty.is_float();

                // Check if the left operand has an unsigned MIR type
                let is_unsigned = Self::operand_mir_type(left, ctx.locals)
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
                        BinOp::Div if is_unsigned => {
                            if let Some(k) = Self::const_power_of_two(right) {
                                builder.ins().ushr_imm(lhs_val, k as i64)
                            } else {
                                builder.ins().udiv(lhs_val, rhs_val)
                            }
                        }
                        BinOp::Div => {
                            if let Some(k) = Self::const_power_of_two(right) {
                                // Signed div by 2^k: (value + ((value >> 63) >>> (64-k))) >> k
                                let bits = builder.func.dfg.value_type(lhs_val).bits() as i64;
                                let sign = builder.ins().sshr_imm(lhs_val, bits - 1);
                                let correction = builder.ins().ushr_imm(sign, bits - k as i64);
                                let adjusted = builder.ins().iadd(lhs_val, correction);
                                builder.ins().sshr_imm(adjusted, k as i64)
                            } else {
                                builder.ins().sdiv(lhs_val, rhs_val)
                            }
                        }
                        BinOp::Mod if is_unsigned => {
                            if let Some(k) = Self::const_power_of_two(right) {
                                let ty = builder.func.dfg.value_type(lhs_val);
                                let mask = builder.ins().iconst(ty, (1i64 << k) - 1);
                                builder.ins().band(lhs_val, mask)
                            } else {
                                builder.ins().urem(lhs_val, rhs_val)
                            }
                        }
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
                let val = Self::lower_operand_typed(builder, operand, expected_ty, ctx)?;
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
                let val = Self::lower_operand(builder, value, ctx)?;
                let target = mir_to_cranelift_type(target_ty)?;
                let val_ty = builder.func.dfg.value_type(val);
                Ok(Self::convert_value(builder, val, val_ty, target))
            }

            // Struct/enum field access: load from base pointer + field offset
            MirRValue::Field { base, field_index, byte_offset, field_size } => {
                let base_val = Self::lower_operand(builder, base, ctx)?;
                let base_ty = Self::operand_mir_type(base, ctx.locals);
                let mut load_ty = expected_ty.unwrap_or(types::I64);

                let offset = match &base_ty {
                    Some(MirType::Struct(id)) => {
                        if let Some(layout) = ctx.struct_layouts.get(id.0 as usize) {
                            if let Some(field) = layout.fields.get(*field_index as usize) {
                                // Aggregate field: return pointer into parent struct.
                                // Covers both >8-byte structs and ≤8-byte enums/structs
                                // that use stack-slot representation in codegen.
                                if field.size > 8 || Self::is_aggregate_field_type(&field.ty) {
                                    let addr = builder.ins().iadd_imm(base_val, field.offset as i64);
                                    return Ok(addr);
                                }
                                // Scalar field. Layout uses 8-byte slots; load at storage
                                // width to avoid reading wrong bytes (e.g. lower f64 half).
                                load_ty = match &field.ty {
                                    RaskType::F64 | RaskType::F32 => types::F64,
                                    _ => types::I64,
                                };
                                field.offset as i32
                            } else {
                                0
                            }
                        } else {
                            0
                        }
                    }
                    Some(MirType::Enum(id)) => {
                        if let Some(layout) = ctx.enum_layouts.get(id.0 as usize) {
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
                let loaded = builder.ins().load(load_ty, flags, base_val, offset);

                // Narrow from storage type to declared type when needed.
                // E.g., f32 field stored as f64 in 8-byte slot → fdemote.
                let result = if let Some(exp) = expected_ty {
                    let loaded_ty = builder.func.dfg.value_type(loaded);
                    if loaded_ty != exp {
                        Self::convert_value(builder, loaded, loaded_ty, exp)
                    } else {
                        loaded
                    }
                } else {
                    loaded
                };
                Ok(result)
            }

            // Enum discriminant extraction: load tag byte from base pointer
            MirRValue::EnumTag { value } => {
                let ptr_val = Self::lower_operand(builder, value, ctx)?;
                let base_ty = Self::operand_mir_type(value, ctx.locals);

                let (tag_offset, tag_cranelift_ty) = match &base_ty {
                    Some(MirType::Enum(id)) => {
                        if let Some(layout) = ctx.enum_layouts.get(id.0 as usize) {
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
                let var = ctx.var_map.get(local_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Ref: local not found".to_string()))?;
                let val = builder.use_var(*var);

                // For aggregate types the variable already IS a pointer
                let local_ty = ctx.locals.iter().find(|l| l.id == *local_id).map(|l| &l.ty);
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
                let ptr_val = Self::lower_operand(builder, operand, ctx)?;
                let load_ty = expected_ty.unwrap_or(types::I64);
                let flags = MemFlags::new();
                Ok(builder.ins().load(load_ty, flags, ptr_val, 0))
            }

            // Array element access: base_ptr + index * elem_size → load
            MirRValue::ArrayIndex { base, index, elem_size } => {
                let base_val = Self::lower_operand(builder, base, ctx)?;
                let idx_val = Self::lower_operand_typed(builder, index, Some(types::I64), ctx)?;
                let elem_sz = builder.ins().iconst(types::I64, *elem_size as i64);
                let offset = builder.ins().imul(idx_val, elem_sz);
                let addr = builder.ins().iadd(base_val, offset);
                let load_ty = expected_ty.unwrap_or(types::I64);
                let flags = MemFlags::new();
                Ok(builder.ins().load(load_ty, flags, addr, 0))
            }
        }
    }

    fn lower_terminator(
        builder: &mut ClifFunctionBuilder,
        term: &MirTerminator,
        ctx: &CodegenCtx,
        cleanup_chain_blocks: &HashMap<Vec<BlockId>, cranelift_codegen::ir::Block>,
    ) -> CodegenResult<()> {
        match &term.kind {
            MirTerminatorKind::Return { value } => {
                // For small aggregate return values (≤8 bytes) in stack slots,
                // load the data and return it directly.
                // For larger aggregates, return the stack slot address. The caller
                // copies from it immediately via copy_aggregate (the callee stack
                // is still accessible at that point on x86-64).
                if let Some(stack_info) = Self::return_stack_info(value.as_ref(), ctx.stack_slot_map) {
                    let (_local_id, ss, size) = stack_info;
                    if size <= 8 {
                        let loaded = builder.ins().stack_load(types::I64, ss, 0);
                        builder.ins().return_(&[loaded]);
                    } else {
                        // Return pointer to stack slot data for copy_aggregate
                        Self::emit_return(builder, value.as_ref(), ctx)?;
                    }
                } else if matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_)) {
                    // Function returns Result/Option but value is a plain scalar
                    // (e.g. `return 42` in a function returning `i32 or string`).
                    // Wrap the value as Ok/Some in a temporary stack slot and return
                    // the slot address so the caller can copy_aggregate.
                    let slot_size = Self::resolve_type_alloc_size(ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts)
                        .unwrap_or(16);
                    let ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        slot_size,
                        0,
                    ));
                    let val = if let Some(val_op) = value.as_ref() {
                        Self::lower_operand_typed(builder, val_op, Some(types::I64), ctx)?
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    Self::wrap_ok_into_slot(builder, val, ss);
                    let addr = builder.ins().stack_addr(types::I64, ss, 0);
                    builder.ins().return_(&[addr]);
                } else {
                    Self::emit_return(builder, value.as_ref(), ctx)?;
                }
            }

            MirTerminatorKind::Goto { target } => {
                let target_block = ctx.block_map.get(target)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Target block not found".to_string()))?;
                builder.ins().jump(*target_block, &[]);
            }

            MirTerminatorKind::Branch { cond, then_block, else_block } => {
                let mut cond_val = Self::lower_operand(builder, cond, ctx)?;

                let cond_ty = builder.func.dfg.value_type(cond_val);
                if cond_ty == types::I8 {
                    cond_val = builder.ins().icmp_imm(IntCC::NotEqual, cond_val, 0);
                }

                let then_cl = ctx.block_map.get(then_block)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Then block not found".to_string()))?;
                let else_cl = ctx.block_map.get(else_block)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Else block not found".to_string()))?;
                builder.ins().brif(cond_val, *then_cl, &[], *else_cl, &[]);
            }

            MirTerminatorKind::Switch { value, cases, default } => {
                let raw_scrutinee = Self::lower_operand(builder, value, ctx)?;
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
                    let target_block = ctx.block_map.get(target_id)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Switch target block not found".to_string()))?;

                    let cmp_val = builder.ins().iconst(types::I64, *value as i64);
                    let cond = builder.ins().icmp(IntCC::Equal, scrutinee_val, cmp_val);

                    let next_block = builder.create_block();
                    comparison_blocks.push(next_block);

                    builder.ins().brif(cond, *target_block, &[], next_block, &[]);
                    builder.switch_to_block(next_block);
                }

                let default_block = ctx.block_map.get(default)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Switch default block not found".to_string()))?;
                builder.ins().jump(*default_block, &[]);

                // Seal comparison chain blocks (these aren't MIR blocks)
                for block in comparison_blocks {
                    builder.seal_block(block);
                }
            }

            MirTerminatorKind::Unreachable => {
                builder.ins().trap(TrapCode::user(1).unwrap());
            }

            MirTerminatorKind::CleanupReturn { value, cleanup_chain } => {
                if !cleanup_chain.is_empty() {
                    if let Some(&shared_block) = cleanup_chain_blocks.get(cleanup_chain) {
                        // Jump to shared cleanup block, passing return value.
                        if let Some(val_op) = value {
                            let expected_ty = mir_to_cranelift_type(ctx.ret_ty)?;
                            let val = Self::lower_operand_typed(builder, val_op, Some(expected_ty), ctx)?;
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
                        Self::emit_return(builder, value.as_ref(), ctx)?;
                    }
                } else {
                    // Empty cleanup chain — just return directly
                    Self::emit_return(builder, value.as_ref(), ctx)?;
                }
            }
        }
        Ok(())
    }

    /// Emit a return instruction.
    fn emit_return(
        builder: &mut ClifFunctionBuilder,
        value: Option<&MirOperand>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        if let Some(val_op) = value {
            let expected_ty = mir_to_cranelift_type(ctx.ret_ty)?;
            let val = Self::lower_operand_typed(builder, val_op, Some(expected_ty), ctx)?;
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

    /// Check if a return value comes from a stack-allocated aggregate local.
    /// Returns the (stack_slot, size) if so.
    fn return_stack_info(
        value: Option<&MirOperand>,
        stack_slot_map: &HashMap<LocalId, (StackSlot, u32)>,
    ) -> Option<(LocalId, StackSlot, u32)> {
        if let Some(MirOperand::Local(id)) = value {
            if let Some((ss, size)) = stack_slot_map.get(id) {
                return Some((*id, *ss, *size));
            }
        }
        None
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
            MirType::Slice(_) | MirType::TraitObject { .. } => Some(ty.size()),
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

    /// C functions that use "negative return = error" convention.
    /// For these, return value < 0 maps to Err(value), >= 0 maps to Ok(value).
    /// Note: fs_open/fs_create return NULL (0) for errors, not -1 — handled separately.
    fn is_negative_err_fn(name: &str) -> bool {
        matches!(name,
            "net_tcp_listen" | "TcpListener_accept" |
            "TcpConnection_read_http_request" | "TcpConnection_write_http_response" |
            "Sender_send" | "Sender_try_send" |
            "ThreadHandle_join" | "Thread_join"
        )
    }

    /// Wrap a C return value into a Result stack slot, checking for errors.
    /// If value < 0: tag=1 (Err), payload=value. Otherwise: tag=0 (Ok), payload=value.
    fn wrap_result_into_slot(builder: &mut ClifFunctionBuilder, value: Value, dst_slot: StackSlot) {
        let zero = builder.ins().iconst(types::I64, 0);
        let is_err = builder.ins().icmp(IntCC::SignedLessThan, value, zero);
        let tag = builder.ins().uextend(types::I64, is_err);
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
        ctx: &CodegenCtx,
    ) -> CallAdapt {
        match func_name {
            // Constructors: inject elem_size / key_size+val_size
            "Vec_new" => {
                let elem_size = builder.ins().iconst(types::I64, 8);
                args.insert(0, elem_size);
                CallAdapt::None
            }
            "Map_new" | "Map_new_string_keys" => {
                let key_size = builder.ins().iconst(types::I64, 8);
                let val_size = builder.ins().iconst(types::I64, 8);
                args.insert(0, key_size);
                args.insert(1, val_size);
                CallAdapt::None
            }
            // Pool_new: MIR injects elem_size as first arg.
            // No special codegen handling needed.
            "Pool_new" => CallAdapt::None,

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
            "Vec_remove" | "Vec_remove_at" => {
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

            // Map get: wrap key as pointer, wrap result as Option
            "Map_get" => {
                if args.len() >= 2 {
                    let key = args[1];
                    args[1] = Self::value_to_ptr(builder, key);
                }
                CallAdapt::DerefOption
            }
            "Map_get_unwrap" => {
                if args.len() >= 2 {
                    let key = args[1];
                    args[1] = Self::value_to_ptr(builder, key);
                }
                CallAdapt::DerefResult
            }

            // Pool insert: wrap value as pointer, wrap return as Ok Result
            "Pool_insert" => {
                // Determine actual element size from MIR arg type.
                // Structs store inline data (size > 8); scalars store as i64 (size = 8).
                let mut elem_size: i64 = 8;
                let mut is_struct_elem = false;
                if let Some(MirOperand::Local(arg_id)) = mir_args.get(1) {
                    if let Some(local) = ctx.locals.iter().find(|l| l.id == *arg_id) {
                        if let MirType::Struct(layout_id) = &local.ty {
                            if let Some(layout) = ctx.struct_layouts.get(layout_id.0 as usize) {
                                elem_size = layout.size as i64;
                                is_struct_elem = true;
                            }
                        }
                    }
                }

                if args.len() >= 2 {
                    if !is_struct_elem {
                        // Scalar: put value on stack and pass pointer to it
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                    // Struct: args[1] is already a pointer to the struct data on stack
                }
                let size_val = builder.ins().iconst(types::I64, elem_size);
                args.push(size_val);

                // Pool.insert returns raw handle i64. The destination local
                // has MirType::Result and a pre-allocated stack slot, so
                // the post-call handler (wrap_ok_into_slot) wraps the raw
                // return value as Ok(handle) into that slot.
                CallAdapt::None
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

            // Pool get: result is void*, wrap as Option storing the
            // pointer itself (not dereferenced) so field access works
            // through it regardless of element struct size.
            "Pool_get" => CallAdapt::DerefOption,
            // Pool index: return raw pointer for mutation (pool[h].field = val)
            "Pool_index" | "Pool_checked_access" => CallAdapt::None,

            // Channel_unbuffered: MIR injects elem_size; builder appends capacity=0
            "Channel_unbuffered" => {
                let zero = builder.ins().iconst(types::I64, 0);
                args.push(zero);
                CallAdapt::None
            }

            // Shared_new: args are [data, data_size]. Ensure data is a pointer.
            // Compute actual data_size from struct layout (overrides MIR default
            // which may be 8 when Shared.new(val) lacks explicit generic arg).
            "Shared_new" => {
                if args.len() >= 2 {
                    let mut data_size: i64 = 8;
                    let mut is_struct = false;
                    if let Some(MirOperand::Local(arg_id)) = mir_args.first() {
                        if let Some(local) = ctx.locals.iter().find(|l| l.id == *arg_id) {
                            if let MirType::Struct(layout_id) = &local.ty {
                                if let Some(layout) = ctx.struct_layouts.get(layout_id.0 as usize) {
                                    data_size = layout.size as i64;
                                    is_struct = true;
                                }
                            }
                        }
                    }
                    if !is_struct {
                        let val = args[0];
                        args[0] = Self::value_to_ptr(builder, val);
                    }
                    // Override data_size with actual computed value
                    args[1] = builder.ins().iconst(types::I64, data_size);
                }
                CallAdapt::None
            }

            // Sender_send / send: wrap value as pointer (scalars need value_to_ptr,
            // structs are already pointers in the all-i64 model).
            "Sender_send" | "send" => {
                if args.len() >= 2 {
                    let mut is_struct = false;
                    if let Some(MirOperand::Local(arg_id)) = mir_args.get(1) {
                        if let Some(local) = ctx.locals.iter().find(|l| l.id == *arg_id) {
                            if matches!(&local.ty, MirType::Struct(_)) {
                                is_struct = true;
                            }
                        }
                    }
                    if !is_struct {
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                }
                CallAdapt::None
            }

            // Receiver_recv_struct: allocate buffer for received struct data.
            // MIR args: [rx, elem_size]. Replace elem_size with stack buffer address.
            "Receiver_recv_struct" => {
                let elem_size = match mir_args.get(1) {
                    Some(MirOperand::Constant(MirConst::Int(size))) => *size as u32,
                    _ => 8,
                };
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, elem_size, 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                if args.len() >= 2 {
                    args[1] = addr;
                } else {
                    args.push(addr);
                }
                // recv_ptr returns out_ptr, which IS the struct pointer
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
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        Self::lower_operand_typed(builder, op, None, ctx)
    }

    /// Lower a string constant as a raw `const char*` pointer (no RaskString wrapping).
    /// Used by `string_append_cstr` to avoid allocating a temporary RaskString.
    fn lower_string_const_as_cstr(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        if let MirOperand::Constant(MirConst::String(s)) = op {
            if let Some(gv) = ctx.string_globals.get(s.as_str()) {
                return Ok(builder.ins().global_value(types::I64, *gv));
            }
        }
        // Shouldn't reach here — transform only emits cstr variant for constants
        Ok(builder.ins().iconst(types::I64, 0))
    }

    fn lower_operand_typed(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        expected_ty: Option<Type>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        match op {
            MirOperand::Local(local_id) => {
                let var = ctx.var_map.get(local_id)
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
                        if let Some(gv) = ctx.string_globals.get(s.as_str()) {
                            let raw_ptr = builder.ins().global_value(types::I64, *gv);
                            if let Some(string_from_ref) = ctx.func_refs.get("string_from") {
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
