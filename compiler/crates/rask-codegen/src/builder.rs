// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function builder — lowers MIR to Cranelift IR.

use cranelift::prelude::*;
use cranelift_codegen::ir::{FuncRef, Function, GlobalValue, InstBuilder, MemFlags, SourceLoc, StackSlot, StackSlotData, StackSlotKind};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_frontend::{FunctionBuilder as ClifFunctionBuilder, FunctionBuilderContext};
use std::collections::{HashMap, HashSet};

use rask_mir::FieldAccess;
use rask_mir::{BinOp, BlockId, LocalId, MirConst, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType, UnaryOp};
use rask_mono::{StructLayout, EnumLayout};
use rask_types::Type as RaskType;
use crate::dispatch::{ArgAdapt, RetAdapt};
use crate::types::mir_to_cranelift_type;
use crate::{BuildMode, CodegenError, CodegenResult};

/// Copy `size` bytes from `src_ptr + src_off` to `dst_ptr + dst_off`.
///
/// The one canonical aggregate byte-copy in the backend. It emits the full
/// 8→4→2→1 ladder, so every trailing size is covered. A hand-inlined copy that
/// skipped the 2-byte step silently dropped a byte for any size ≡ 2,3,6,7
/// (mod 8) (#365) — which is exactly the kind of bug that stays invisible
/// until someone adds a struct with an odd layout.
///
/// Route ALL aggregate copies through this. Do not re-inline the ladder.
pub(crate) fn copy_bytes(
    builder: &mut ClifFunctionBuilder,
    src_ptr: Value,
    src_off: i32,
    dst_ptr: Value,
    dst_off: i32,
    size: u32,
) {
    let size = size as i32;
    let mut off = 0i32;
    for (chunk, ty) in [(8, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
        while size - off >= chunk {
            let val = builder.ins().load(ty, MemFlags::new(), src_ptr, src_off + off);
            builder.ins().store(MemFlags::new(), val, dst_ptr, dst_off + off);
            off += chunk;
        }
    }
}

// Checked-arithmetic panic messages (type.overflow). Registered as string
// globals unconditionally (see `register_strings`) so the message prints in
// both debug and release builds — OV4 requires consistent behavior.
pub(crate) const OV_ADD: &str = "integer overflow in addition";
pub(crate) const OV_SUB: &str = "integer overflow in subtraction";
pub(crate) const OV_MUL: &str = "integer overflow in multiplication";
pub(crate) const OV_NEG: &str = "integer overflow in negation";
pub(crate) const OV_DIV_ZERO: &str = "division by zero";
pub(crate) const OV_DIV_OVERFLOW: &str = "integer overflow in division (MIN / -1)";
pub(crate) const OV_SHIFT: &str = "shift amount exceeds bit width";

/// All overflow panic messages, registered up front by codegen.
pub(crate) const OVERFLOW_MESSAGES: &[&str] = &[
    OV_ADD, OV_SUB, OV_MUL, OV_NEG, OV_DIV_ZERO, OV_DIV_OVERFLOW, OV_SHIFT,
];

/// Read-only context bundling parameters for lowering functions.
struct CodegenCtx<'a> {
    var_map: &'a HashMap<LocalId, Variable>,
    locals: &'a [rask_mir::MirLocal],
    /// Parameters live in their own list, so `locals` alone never resolves one.
    params: &'a [rask_mir::MirLocal],
    /// Declared param types of every Rask function, by MIR name
    fn_param_types: &'a HashMap<String, Vec<MirType>>,
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
    line_map: Option<&'a rask_ast::LineMap>,
    current_line: u32,
    current_col: u32,
    /// Byte offset of the current MIR statement being lowered
    current_span_start: u32,
    ret_ty: &'a MirType,
    is_main: bool,
    adapt_table: &'a HashMap<String, (ArgAdapt, RetAdapt)>,
}

/// How to compare one slot of an aggregate. Struct and enum-payload fields
/// carry a Rask type from the layout; tuple and array elements carry a MIR one.
enum FieldKind {
    Rask(RaskType, u32),
    Mir(MirType),
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
    /// The callee wrote the payload straight into the destination `T?`'s slot
    /// and returned 1 (wrote) or 0 (nothing). Only the tag is left to set.
    OptionOutParam(StackSlot),
    /// String out-param: callee wrote 16-byte RaskStr to this slot.
    /// Result is the slot address (pointer), not a loaded value.
    StringOutParam(StackSlot),
    /// Result is void* pointing to 16-byte string element in Vec.
    /// Copy to dst's stack slot.
    DerefStringElement,
    /// Receiver.try_recv: call returned a channel status; the payload was
    /// written into the given slot. Build a `T or E` Result in dst —
    /// status==OK → Ok(payload of `elem_size` bytes), else → Err.
    TryRecvResult(StackSlot, u32),
    /// Receiver.receive on a struct element: the call wrote the value into the
    /// buffer it returns (and panicked if the channel was closed), so the result
    /// is always Ok. Copy `elem_size` bytes out of it into dst's payload.
    RecvStructOk(u32),
    /// parse: the call returned 0/1; the value was written into the given slot.
    /// Build a `T or ParseError` — status==0 → Ok(value), else Err.
    /// Carries (slot, type the runtime wrote, type the destination wants).
    ParseResult(StackSlot, Type, Type),
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
    /// Declared param types of every Rask function, by MIR name
    fn_param_types: &'a HashMap<String, Vec<MirType>>,
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

    /// Line map for converting byte offsets → line:col
    line_map: Option<&'a rask_ast::LineMap>,

    /// Table-driven call adaptation (populated from dispatch::stdlib_entries)
    adapt_table: HashMap<String, (ArgAdapt, RetAdapt)>,
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
        fn_param_types: &'a HashMap<String, Vec<MirType>>,
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
            fn_param_types,
            build_mode,
            block_map: HashMap::new(),
            var_map: HashMap::new(),
            stack_slot_map: HashMap::new(),
            current_line: 0,
            current_col: 0,
            line_map: None,
            adapt_table: crate::dispatch::build_adapt_table(),
        })
    }

    /// Set the line map for converting byte offsets to line:col in assert messages.
    pub fn set_line_map(&mut self, line_map: &'a rask_ast::LineMap) {
        self.line_map = Some(line_map);
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

        // Collect cleanup-only blocks (appear in CleanupReturn chains)
        // and their transitive sub-blocks (handler/done blocks reachable
        // from cleanup blocks). These are excluded from normal codegen
        // and processed as part of shared cleanup blocks instead.
        //
        // A block is cleanup-only IFF every path reaching it goes through a
        // CleanupReturn terminator. Blocks reachable from both cleanup and
        // normal paths (e.g. post-cleanup continuation points listed in a
        // chain) must stay in the normal block_map so non-cleanup Gotos can
        // still target them.
        let chain_members: HashSet<BlockId> = self.mir_fn.blocks.iter()
            .filter_map(|b| {
                if let MirTerminatorKind::CleanupReturn { cleanup_chain, .. } = &b.terminator.kind {
                    Some(cleanup_chain.iter().copied())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        // Walk forward from the entry over ordinary edges. Anything found this
        // way is on a normal path and keeps its place in the main block map.
        let mut normal_reachable: HashSet<BlockId> = HashSet::new();
        {
            let mut queue = vec![self.mir_fn.entry_block];
            while let Some(bid) = queue.pop() {
                if !normal_reachable.insert(bid) {
                    continue;
                }
                if let Some(b) = self.mir_fn.blocks.iter().find(|b| b.id == bid) {
                    // A CleanupReturn's chain is the cleanup path, not a normal
                    // successor — that's the whole distinction being drawn here.
                    if !matches!(b.terminator.kind, MirTerminatorKind::CleanupReturn { .. }) {
                        queue.extend(rask_mir::analysis::cfg::successors(&b.terminator));
                    }
                }
            }
        }

        // Then forward from each cleanup chain member. Everything reachable
        // that isn't also on a normal path belongs to the cleanup sub-CFG.
        //
        // This used to run backwards — a block joined the set once *all* its
        // predecessors were in it. A loop inside the cleanup path defeats that:
        // the loop header's back edge comes from a block that can't be admitted
        // until the header is, so neither ever is. The body stayed in the main
        // block map while its entry moved to the cleanup map, and the jump
        // between them was emitted into nothing — leaving a block with no
        // terminator for Cranelift to reject (#538).
        let mut cleanup_only: HashSet<BlockId> = HashSet::new();
        {
            let mut queue: Vec<BlockId> = chain_members.iter().copied().collect();
            while let Some(bid) = queue.pop() {
                if normal_reachable.contains(&bid) || !cleanup_only.insert(bid) {
                    continue;
                }
                if let Some(b) = self.mir_fn.blocks.iter().find(|b| b.id == bid) {
                    queue.extend(rask_mir::analysis::cfg::successors(&b.terminator));
                }
            }
        }

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

        // For main(): emit rask_set_origin_file(source_file) so .origin() includes file name
        if self.mir_fn.name == "main" {
            if let Some(file_name) = self.mir_fn.source_file.as_deref() {
                if let Some(gv) = self.string_globals.get(file_name) {
                    let file_ptr = builder.ins().global_value(types::I64, *gv);
                    if let Some(func_ref) = self.func_refs.get("rask_set_origin_file") {
                        builder.ins().call(*func_ref, &[file_ptr]);
                    }
                }
            }
        }

        let mut ctx = CodegenCtx {
            var_map: &self.var_map,
            locals: &self.mir_fn.locals,
            params: &self.mir_fn.params,
            fn_param_types: self.fn_param_types,
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
            line_map: self.line_map,
            current_line: self.current_line,
            current_col: self.current_col,
            current_span_start: 0,
            ret_ty: &self.mir_fn.ret_ty,
            is_main: self.mir_fn.name == "main",
            adapt_table: &self.adapt_table,
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
                Self::apply_srcloc(&mut builder, stmt.span);
                ctx.current_span_start = stmt.span.start as u32;
                // Update line:col from span if we have a line map
                if let Some(lm) = ctx.line_map {
                    let (line, col) = lm.offset_to_line_col(stmt.span.start);
                    ctx.current_line = line as u32;
                    ctx.current_col = col as u32;
                }
                Self::lower_stmt(&mut builder, stmt, &ctx)?;
            }

            // Lower terminator
            Self::apply_srcloc(&mut builder, mir_block.terminator.span);
            Self::lower_terminator(&mut builder, &mir_block.terminator, &ctx, &cleanup_chain_blocks)?;
        }

        // Emit shared cleanup blocks. Each unique cleanup chain gets one
        // entry Cranelift block. Cleanup blocks may contain sub-CFGs
        // (e.g. else handler branching for ER2), which get their own
        // Cranelift blocks.
        //
        // Create Cranelift blocks for all cleanup sub-blocks first so
        // Branch terminators can reference them.
        let mut cleanup_block_map: HashMap<BlockId, cranelift_codegen::ir::Block> = HashMap::new();
        for &bid in &cleanup_only {
            let cl_block = builder.create_block();
            cleanup_block_map.insert(bid, cl_block);
        }

        for (chain, &shared_block) in &cleanup_chain_blocks {
            builder.switch_to_block(shared_block);

            // Add return value as block parameter if function returns a value
            // (main is called from C as void — never returns a value)
            let is_main = self.mir_fn.name == "main";
            let ret_param = if !matches!(self.mir_fn.ret_ty, MirType::Void) && !is_main {
                let ret_cl_ty = mir_to_cranelift_type(&self.mir_fn.ret_ty)?;
                Some(builder.append_block_param(shared_block, ret_cl_ty))
            } else {
                None
            };

            let cleanup_ctx = CodegenCtx {
                source_file: None,
                line_map: None,
                current_line: 0,
                current_col: 0,
                current_span_start: 0,
                ..ctx
            };

            // Jump from shared entry to the first cleanup block in the chain
            if let Some(&first_block) = chain.first().and_then(|bid| cleanup_block_map.get(bid)) {
                builder.ins().jump(first_block, &[]);
            } else {
                // Empty chain — just return
                if let Some(val) = ret_param {
                    builder.ins().return_(&[val]);
                } else {
                    builder.ins().return_(&[]);
                }
                continue;
            }

            // Process each cleanup block in the chain as a real CFG.
            // Unreachable sentinels → jump to next chain block or return.
            for (i, block_id) in chain.iter().enumerate() {
                let Some(mir_block) = self.mir_fn.blocks.iter().find(|b| b.id == *block_id) else {
                    continue;
                };
                let Some(&cl_block) = cleanup_block_map.get(block_id) else {
                    continue;
                };

                builder.switch_to_block(cl_block);

                // Lower statements
                for stmt in &mir_block.statements {
                    Self::lower_stmt(&mut builder, stmt, &cleanup_ctx)?;
                }

                // Lower terminator — Unreachable means "continue chain or return"
                match &mir_block.terminator.kind {
                    MirTerminatorKind::Unreachable => {
                        // End of this ensure's sub-CFG. Jump to next chain block or return.
                        if let Some(next_bid) = chain.get(i + 1) {
                            if let Some(&next_cl) = cleanup_block_map.get(next_bid) {
                                builder.ins().jump(next_cl, &[]);
                            } else if let Some(val) = ret_param {
                                builder.ins().return_(&[val]);
                            } else {
                                builder.ins().return_(&[]);
                            }
                        } else if let Some(val) = ret_param {
                            builder.ins().return_(&[val]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }
                    MirTerminatorKind::Branch { cond, then_block, else_block } => {
                        let cond_val = Self::lower_operand_typed(&mut builder, cond, Some(types::I8), &cleanup_ctx)?;
                        let actual_ty = builder.func.dfg.value_type(cond_val);
                        let cond_final = if actual_ty != types::I8 {
                            Self::convert_value(&mut builder, cond_val, actual_ty, types::I8, None)
                        } else {
                            cond_val
                        };
                        let then_cl = cleanup_block_map.get(then_block).copied()
                            .unwrap_or_else(|| builder.create_block());
                        let else_cl = cleanup_block_map.get(else_block).copied()
                            .unwrap_or_else(|| builder.create_block());
                        builder.ins().brif(cond_final, then_cl, &[], else_cl, &[]);
                    }
                    MirTerminatorKind::Goto { target } => {
                        // Cleanup blocks first, then the main map. A target in
                        // neither would leave this block with no terminator,
                        // which Cranelift rejects with a message that says
                        // nothing about where it came from — so say it here.
                        let tgt = cleanup_block_map.get(target)
                            .or_else(|| self.block_map.get(target))
                            .copied()
                            .ok_or_else(|| CodegenError::UnsupportedFeature(format!(
                                "cleanup block jumps to {:?}, which has no Cranelift block",
                                target,
                            )))?;
                        builder.ins().jump(tgt, &[]);
                    }
                    _ => {
                        // Other terminators in cleanup blocks: treat as return
                        if let Some(val) = ret_param {
                            builder.ins().return_(&[val]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }
                }
            }

            // Process sub-blocks (handler blocks, done blocks) that aren't
            // in the chain but are reachable from chain blocks.
            let chain_set: HashSet<BlockId> = chain.iter().copied().collect();
            for &bid in &cleanup_only {
                if chain_set.contains(&bid) {
                    continue; // Already processed above
                }
                // Only process sub-blocks reachable from THIS chain's blocks
                let Some(mir_block) = self.mir_fn.blocks.iter().find(|b| b.id == bid) else {
                    continue;
                };
                let Some(&cl_block) = cleanup_block_map.get(&bid) else {
                    continue;
                };

                // Check if this sub-block is reachable from any block in THIS chain
                let reachable = chain.iter().any(|chain_bid| {
                    let mut visited = HashSet::new();
                    let mut queue = vec![*chain_bid];
                    while let Some(qid) = queue.pop() {
                        if qid == bid { return true; }
                        if !visited.insert(qid) { continue; }
                        if let Some(qb) = self.mir_fn.blocks.iter().find(|b| b.id == qid) {
                            for succ in rask_mir::analysis::cfg::successors(&qb.terminator) {
                                if cleanup_only.contains(&succ) {
                                    queue.push(succ);
                                }
                            }
                        }
                    }
                    false
                });
                if !reachable { continue; }

                builder.switch_to_block(cl_block);
                for stmt in &mir_block.statements {
                    Self::lower_stmt(&mut builder, stmt, &cleanup_ctx)?;
                }

                match &mir_block.terminator.kind {
                    MirTerminatorKind::Unreachable => {
                        // End of sub-CFG — jump to next chain block or return.
                        // Find which chain block this sub-block belongs to.
                        let chain_idx = chain.iter().position(|cid| {
                            let mut visited = HashSet::new();
                            let mut queue = vec![*cid];
                            while let Some(qid) = queue.pop() {
                                if qid == bid { return true; }
                                if !visited.insert(qid) { continue; }
                                if let Some(qb) = self.mir_fn.blocks.iter().find(|b| b.id == qid) {
                                    for succ in rask_mir::analysis::cfg::successors(&qb.terminator) {
                                        if cleanup_only.contains(&succ) {
                                            queue.push(succ);
                                        }
                                    }
                                }
                            }
                            false
                        });
                        let next_chain_idx = chain_idx.map(|i| i + 1);
                        if let Some(next_bid) = next_chain_idx.and_then(|i| chain.get(i)) {
                            if let Some(&next_cl) = cleanup_block_map.get(next_bid) {
                                builder.ins().jump(next_cl, &[]);
                            } else if let Some(val) = ret_param {
                                builder.ins().return_(&[val]);
                            } else {
                                builder.ins().return_(&[]);
                            }
                        } else if let Some(val) = ret_param {
                            builder.ins().return_(&[val]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }
                    MirTerminatorKind::Goto { target } => {
                        // Cleanup blocks first, then the main map. A target in
                        // neither would leave this block with no terminator,
                        // which Cranelift rejects with a message that says
                        // nothing about where it came from — so say it here.
                        let tgt = cleanup_block_map.get(target)
                            .or_else(|| self.block_map.get(target))
                            .copied()
                            .ok_or_else(|| CodegenError::UnsupportedFeature(format!(
                                "cleanup block jumps to {:?}, which has no Cranelift block",
                                target,
                            )))?;
                        builder.ins().jump(tgt, &[]);
                    }
                    MirTerminatorKind::Branch { cond, then_block, else_block } => {
                        let cond_val = Self::lower_operand_typed(&mut builder, cond, Some(types::I8), &cleanup_ctx)?;
                        let actual_ty = builder.func.dfg.value_type(cond_val);
                        let cond_final = if actual_ty != types::I8 {
                            Self::convert_value(&mut builder, cond_val, actual_ty, types::I8, None)
                        } else {
                            cond_val
                        };
                        let then_cl = cleanup_block_map.get(then_block).copied()
                            .unwrap_or_else(|| builder.create_block());
                        let else_cl = cleanup_block_map.get(else_block).copied()
                            .unwrap_or_else(|| builder.create_block());
                        builder.ins().brif(cond_final, then_cl, &[], else_cl, &[]);
                    }
                    _ => {
                        if let Some(val) = ret_param {
                            builder.ins().return_(&[val]);
                        } else {
                            builder.ins().return_(&[]);
                        }
                    }
                }
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
        for &cl_block in cleanup_block_map.values() {
            builder.seal_block(cl_block);
        }

        builder.finalize();
        Ok(())
    }

    /// Set Cranelift source location from a MIR span.
    /// Real spans (end > 0) encode as SourceLoc(start + 1) to avoid the
    /// SourceLoc(0) value which Cranelift reserves internally.
    /// Dummy spans (0..0) clear the location.
    fn apply_srcloc(builder: &mut ClifFunctionBuilder, span: rask_mir::Span) {
        if span.end > 0 {
            // +1 so that byte offset 0 becomes SourceLoc(1), avoiding any
            // ambiguity with "no location" values.
            builder.set_srcloc(SourceLoc::new(span.start as u32 + 1));
        } else {
            builder.set_srcloc(SourceLoc::default());
        }
    }

    fn lower_stmt(
        builder: &mut ClifFunctionBuilder,
        stmt: &MirStmt,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue } => Self::lower_assign(builder, dst, rvalue, ctx)?,

            MirStmtKind::Store { addr, offset, value, store_size } => Self::lower_store(builder, addr, offset, value, store_size, ctx)?,

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

            // ── Runtime ensure hooks (panic unwind) ────────────────────
            // Register a hook so the cleanup runs if the scope unwinds on a
            // panic (ctrl.panic/U1). The env is a frame-local array of 8-byte
            // slots holding each capture's value (aggregate → address).
            MirStmtKind::EnsureHookRegister { thunk, captures } => {
                let slots = captures.len().max(1) as u32;
                let env_ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slots * 8,
                    3, // 8-byte alignment
                ));
                for c in captures {
                    let var = ctx.var_map.get(&c.local_id).ok_or_else(|| {
                        CodegenError::UnsupportedFeature("EnsureHookRegister capture not found".to_string())
                    })?;
                    let val = builder.use_var(*var);
                    let vty = builder.func.dfg.value_type(val);
                    let val64 = if vty == types::I64 {
                        val
                    } else if vty.is_int() && vty.bytes() < 8 {
                        builder.ins().uextend(types::I64, val)
                    } else {
                        val
                    };
                    builder.ins().stack_store(val64, env_ss, c.offset as i32);
                }
                let env_addr = builder.ins().stack_addr(types::I64, env_ss, 0);
                let thunk_ref = ctx.func_refs.get(thunk)
                    .ok_or_else(|| CodegenError::FunctionNotFound(thunk.clone()))?;
                let thunk_ptr = builder.ins().func_addr(types::I64, *thunk_ref);
                let push_ref = ctx.func_refs.get("rask_ensure_push")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_ensure_push".to_string()))?;
                builder.ins().call(*push_ref, &[thunk_ptr, env_addr]);
            }

            // Deregister the most recent hook (normal exit runs the inline path).
            MirStmtKind::EnsureHookPop => {
                let pop_ref = ctx.func_refs.get("rask_ensure_pop")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_ensure_pop".to_string()))?;
                builder.ins().call(*pop_ref, &[]);
            }

            // ── Pool checked access ────────────────────────────────────
            MirStmtKind::PoolCheckedAccess { dst, pool, handle } => Self::lower_pool_checked_access(builder, dst, pool, handle, ctx)?,

            // ── Closure support ──────────────────────────────────────────

            MirStmtKind::ClosureCreate { dst, func_name, captures, heap } => Self::lower_closure_create(builder, dst, func_name, captures, heap, ctx)?,

            MirStmtKind::ClosureCall { dst, closure, args } => Self::lower_closure_call(builder, dst, closure, args, ctx)?,

            MirStmtKind::LoadCapture { dst, env_ptr, offset, by_ref } => {
                let env_val = builder.use_var(*ctx.var_map.get(env_ptr)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture env_ptr not found".to_string()
                    ))?);
                let dst_local = ctx.locals.iter().find(|l| l.id == *dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture destination not found".to_string()
                    ))?;
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "LoadCapture destination variable not found".to_string()
                    ))?;

                if *by_ref {
                    // Ensure-hook capture: the env slot holds an 8-byte pointer to
                    // the original value. Point the local's variable straight at it
                    // (for aggregates the variable already IS the address), so
                    // cleanup runs against the live resource rather than a copy.
                    let val = crate::closures::load_capture(builder, env_val, *offset, types::I64);
                    builder.def_var(*var, val);
                } else if let Some((ss, size)) = ctx.stack_slot_map.get(dst) {
                    // Aggregate types (String, Struct, etc.) were deep-copied into
                    // the closure environment. Copy into the local stack slot and
                    // set the variable to the local slot address.
                    let env_addr = builder.ins().iadd_imm(env_val, *offset as i64);
                    Self::copy_aggregate(builder, env_addr, *ss, *size);
                    let local_addr = builder.ins().stack_addr(types::I64, *ss, 0);
                    builder.def_var(*var, local_addr);
                } else {
                    // Scalar: load the value directly
                    let load_ty = mir_to_cranelift_type(&dst_local.ty)?;
                    let val = crate::closures::load_capture(builder, env_val, *offset, load_ty);
                    builder.def_var(*var, val);
                }
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

            MirStmtKind::TraitBox { dst, value, vtable_name, concrete_size, .. } => Self::lower_trait_box(builder, dst, value, vtable_name, concrete_size, ctx)?,

            MirStmtKind::TraitCall { dst, trait_object, method_name, vtable_offset, args } => Self::lower_trait_call(builder, dst, trait_object, method_name, vtable_offset, args, ctx)?,

            MirStmtKind::TraitDrop { trait_object } => {
                let obj_val = builder.use_var(*ctx.var_map.get(trait_object)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitDrop: trait object variable not found".to_string()
                    ))?);

                // Load data_ptr and vtable_ptr
                let data_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, crate::layouts::FAT_PTR_DATA_OFFSET);
                let vtable_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, crate::layouts::FAT_PTR_VTABLE_OFFSET);

                // Load drop_fn from vtable
                let drop_fn = builder.ins().load(types::I64, MemFlags::new(), vtable_ptr, crate::vtable::VTABLE_DROP_OFFSET as i32);

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

            MirStmtKind::RcInc { local } => {
                // Increment string refcount: rask_string_clone(local)
                let val = builder.use_var(*ctx.var_map.get(local)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "RcInc local variable not found".to_string()
                    ))?);
                let clone_ref = ctx.func_refs.get("rask_string_clone")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_string_clone".to_string()))?;
                builder.ins().call(*clone_ref, &[val]);
            }

            MirStmtKind::RcDec { local } => {
                // Decrement string refcount: rask_string_free(local)
                let val = builder.use_var(*ctx.var_map.get(local)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "RcDec local variable not found".to_string()
                    ))?);
                let free_ref = ctx.func_refs.get("rask_string_free")
                    .ok_or_else(|| CodegenError::FunctionNotFound("rask_string_free".to_string()))?;
                builder.ins().call(*free_ref, &[val]);
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
        if Self::try_lower_builtin_call(builder, dst, func, args, ctx)? {
            return Ok(());
        }
        if func.is_extern {
            Self::lower_extern_call(builder, dst, func, args, ctx)
        } else {
            Self::lower_ordinary_call(builder, dst, func, args, ctx)
        }
    }

    /// Convert a value between Cranelift types (integer widening/narrowing, float conversion).
    /// `from_mir` is the source's MIR type where the caller has it. A Cranelift
    /// type carries no signedness, so without it every widening sign-extends,
    /// and `x as u64` on a `u16` holding 60000 came out as 2^64 - 5536 (#326).
    /// Callers used to patch that themselves; two did, the rest didn't.
    fn convert_value(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        from_ty: Type,
        to_ty: Type,
        from_mir: Option<&MirType>,
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
            } else if from_mir.is_some_and(|t| t.is_unsigned()) {
                builder.ins().uextend(to_ty, val)
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

    /// Lower an explicit conversion form (type.primitives CV5–CV10).
    fn lower_convert(
        builder: &mut ClifFunctionBuilder,
        value: &MirOperand,
        source_ty: &MirType,
        target_ty: &MirType,
        kind: rask_ast::expr::ConvertKind,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        use rask_ast::expr::ConvertKind::*;

        // Normalize the operand to the source type's Cranelift width.
        let src_clif = mir_to_cranelift_type(source_ty)?;
        let raw = Self::lower_operand(builder, value, ctx)?;
        let raw_ty = builder.func.dfg.value_type(raw);
        let val = if raw_ty != src_clif {
            Self::convert_value(builder, raw, raw_ty, src_clif, None)
        } else {
            raw
        };
        let tgt_clif = mir_to_cranelift_type(target_ty)?;

        match kind {
            // CV5: bit-preserving resize.
            Truncate => Ok(Self::resize_int(builder, val, source_ty, target_ty)),
            // CV6: clamp to target range.
            Saturate => Ok(Self::saturate_int(builder, val, source_ty, target_ty)),
            // CV8/CV9: float → int.
            FloatToInt => {
                // Trapping conversion — NaN/inf/overflow abort the task.
                if target_ty.is_unsigned() {
                    Ok(builder.ins().fcvt_to_uint(tgt_clif, val))
                } else {
                    Ok(builder.ins().fcvt_to_sint(tgt_clif, val))
                }
            }
            FloatToIntSat => {
                if target_ty.is_unsigned() {
                    Ok(builder.ins().fcvt_to_uint_sat(tgt_clif, val))
                } else {
                    Ok(builder.ins().fcvt_to_sint_sat(tgt_clif, val))
                }
            }
            // CV7/CV10: build Option<T> in a stack slot, branchlessly.
            TryConvert => {
                // char.from_u32 lowers here with a Char target — validity is
                // "valid Unicode scalar", not a contiguous integer range.
                if matches!(target_ty, MirType::Char) {
                    let valid = Self::char_is_valid(builder, val);
                    return Ok(Self::build_option(builder, val, valid, tgt_clif));
                }
                let payload = Self::resize_int(builder, val, source_ty, target_ty);
                let in_range = Self::int_in_range(builder, val, source_ty, target_ty);
                Ok(Self::build_option(builder, payload, in_range, tgt_clif))
            }
            TryFloatToInt => {
                // Saturating conversion gives a defined payload; validity gates the tag.
                let payload = if target_ty.is_unsigned() {
                    builder.ins().fcvt_to_uint_sat(tgt_clif, val)
                } else {
                    builder.ins().fcvt_to_sint_sat(tgt_clif, val)
                };
                let valid = Self::float_in_range(builder, val, target_ty);
                Ok(Self::build_option(builder, payload, valid, tgt_clif))
            }
        }
    }

    /// Bit-preserving integer resize. Widening extends by the source's signedness.
    fn resize_int(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        target_ty: &MirType,
    ) -> Value {
        let from = mir_to_cranelift_type(source_ty).unwrap_or(types::I64);
        let to = mir_to_cranelift_type(target_ty).unwrap_or(types::I64);
        if from == to {
            return val;
        }
        if from.bits() > to.bits() {
            builder.ins().ireduce(to, val)
        } else if source_ty.is_unsigned() {
            builder.ins().uextend(to, val)
        } else {
            builder.ins().sextend(to, val)
        }
    }

    /// Widen an integer value to I64 for comparison, per source signedness.
    /// No-op when the value is already 64-bit.
    fn widen_i64(builder: &mut ClifFunctionBuilder, val: Value, source_ty: &MirType) -> Value {
        if builder.func.dfg.value_type(val) == types::I64 {
            return val;
        }
        if source_ty.is_unsigned() {
            builder.ins().uextend(types::I64, val)
        } else {
            builder.ins().sextend(types::I64, val)
        }
    }

    /// Clamp `val` (interpreted per source signedness) to the target range.
    fn saturate_int(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        target_ty: &MirType,
    ) -> Value {
        let (min, max) = Self::int_bounds(target_ty);
        // Widen to I64 for the comparison, then reduce.
        let mut v64 = Self::widen_i64(builder, val, source_ty);

        // Source and target don't have to share a signedness, and one
        // comparison mode for both is wrong whenever they don't. Clamping
        // `u64` to `i64` compared unsigned against `i64::MIN`, which reads as
        // 2^63 unsigned — so every value below it, meaning every ordinary
        // value, "underflowed" and came out as `i64::MIN`. `42 saturate to
        // i64` was -9223372036854775808 (#495).
        if source_ty.is_unsigned() {
            // Nothing unsigned is below a target minimum; every one of those
            // is zero or negative. Only the ceiling can bite, unsigned.
            if max < u64::MAX as i128 {
                let maxc = builder.ins().iconst(types::I64, max as i64);
                let too_big = builder.ins().icmp(IntCC::UnsignedGreaterThan, v64, maxc);
                v64 = builder.ins().select(too_big, maxc, v64);
            }
        } else {
            let minc = builder.ins().iconst(types::I64, min as i64);
            let too_small = builder.ins().icmp(IntCC::SignedLessThan, v64, minc);
            v64 = builder.ins().select(too_small, minc, v64);
            // A ceiling above `i64::MAX` — only `u64`'s — is out of a signed
            // value's reach, so there's nothing to clamp against.
            if max <= i64::MAX as i128 {
                let maxc = builder.ins().iconst(types::I64, max as i64);
                let too_big = builder.ins().icmp(IntCC::SignedGreaterThan, v64, maxc);
                v64 = builder.ins().select(too_big, maxc, v64);
            }
        }

        let to = mir_to_cranelift_type(target_ty).unwrap_or(types::I64);
        if to.bits() < 64 {
            builder.ins().ireduce(to, v64)
        } else {
            v64
        }
    }

    /// True when `val` fits in the target integer range (for `try convert`).
    fn int_in_range(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        source_ty: &MirType,
        target_ty: &MirType,
    ) -> Value {
        let (min, max) = Self::int_bounds(target_ty);
        let v64 = Self::widen_i64(builder, val, source_ty);
        let t = builder.func.dfg.value_type(v64);
        let always = |b: &mut ClifFunctionBuilder| b.ins().iconst(types::I8, 1);

        // Same asymmetry as the saturating form: which comparison is right
        // depends on the *source*'s signedness, and which bound can bite at
        // all depends on the target's.
        let (ge_min, le_max) = if source_ty.is_unsigned() {
            let ge_min = always(builder); // never below a target minimum
            let le_max = if max < u64::MAX as i128 {
                let maxc = builder.ins().iconst(t, max as i64);
                builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, v64, maxc)
            } else {
                always(builder)
            };
            (ge_min, le_max)
        } else {
            let minc = builder.ins().iconst(t, min as i64);
            let ge_min = builder.ins().icmp(IntCC::SignedGreaterThanOrEqual, v64, minc);
            let le_max = if max <= i64::MAX as i128 {
                let maxc = builder.ins().iconst(t, max as i64);
                builder.ins().icmp(IntCC::SignedLessThanOrEqual, v64, maxc)
            } else {
                always(builder)
            };
            (ge_min, le_max)
        };
        builder.ins().band(ge_min, le_max)
    }

    /// True when the float is finite and its truncation fits the target range.
    fn float_in_range(
        builder: &mut ClifFunctionBuilder,
        val: Value,
        target_ty: &MirType,
    ) -> Value {
        let (min, max) = Self::int_bounds_i64(target_ty);
        let ft = builder.func.dfg.value_type(val);
        let minf = builder.ins().f64const(min as f64);
        let maxf = builder.ins().f64const(max as f64);
        let (minf, maxf) = if ft == types::F32 {
            (builder.ins().fdemote(types::F32, minf), builder.ins().fdemote(types::F32, maxf))
        } else {
            (minf, maxf)
        };
        // NaN fails both comparisons (ordered), so `ge_min & le_max` is false.
        let ge_min = builder.ins().fcmp(FloatCC::GreaterThanOrEqual, val, minf);
        let le_max = builder.ins().fcmp(FloatCC::LessThanOrEqual, val, maxf);
        builder.ins().band(ge_min, le_max)
    }

    /// True (I64 0/1) when `val` is a valid Unicode scalar (≤ 0x10FFFF, not a
    /// surrogate 0xD800..=0xDFFF).
    fn char_is_valid(builder: &mut ClifFunctionBuilder, val: Value) -> Value {
        let vt = builder.func.dfg.value_type(val);
        let n64 = if vt == types::I64 { val } else { builder.ins().uextend(types::I64, val) };
        let max = builder.ins().iconst(types::I64, 0x10FFFF);
        let sur_lo = builder.ins().iconst(types::I64, 0xD800);
        let sur_hi = builder.ins().iconst(types::I64, 0xDFFF);
        let le_max = builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, n64, max);
        let ge_lo = builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, n64, sur_lo);
        let le_hi = builder.ins().icmp(IntCC::UnsignedLessThanOrEqual, n64, sur_hi);
        let le_max = builder.ins().uextend(types::I64, le_max);
        let ge_lo = builder.ins().uextend(types::I64, ge_lo);
        let le_hi = builder.ins().uextend(types::I64, le_hi);
        let in_sur = builder.ins().band(ge_lo, le_hi);
        let not_sur = builder.ins().bxor_imm(in_sur, 1);
        builder.ins().band(le_max, not_sur)
    }

    /// Build `Option<T>` in a stack slot: tag 0 (Some) if `present`, else 1 (None).
    /// Layout matches `build_some`: [tag:8][payload:8].
    fn build_option(
        builder: &mut ClifFunctionBuilder,
        payload: Value,
        present: Value,
        payload_ty: Type,
    ) -> Value {
        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot, 16, 0,
        ));
        let some_tag = builder.ins().iconst(types::I64, 0);
        let none_tag = builder.ins().iconst(types::I64, 1);
        let tag = builder.ins().select(present, some_tag, none_tag);
        builder.ins().stack_store(tag, ss, crate::layouts::TAG_OFFSET);
        // Store the payload at its natural width; widen scalars < 8 bytes to I64 slot.
        let store_val = if payload_ty.is_int() && payload_ty.bits() < 64 {
            builder.ins().uextend(types::I64, payload)
        } else {
            payload
        };
        builder.ins().stack_store(store_val, ss, crate::layouts::PAYLOAD_OFFSET);
        builder.ins().stack_addr(types::I64, ss, 0)
    }

    /// Target integer range as i64 constants. 64-bit unsigned upper bound is
    /// approximated by i64::MAX (saturate/try to u64 from ≤64-bit rarely overflows).
    fn int_bounds_i64(t: &MirType) -> (i64, i64) {
        let (lo, hi) = Self::int_bounds(t);
        (lo as i64, hi.min(i64::MAX as i128) as i64)
    }

    /// A target integer type's true range. `u64`'s maximum doesn't fit in an
    /// `i64`, so the i64-shaped version above reported `i64::MAX` for it —
    /// which is a real clamp point rather than the type's actual ceiling.
    fn int_bounds(t: &MirType) -> (i128, i128) {
        match t {
            MirType::I8 => (i8::MIN as i128, i8::MAX as i128),
            MirType::I16 => (i16::MIN as i128, i16::MAX as i128),
            MirType::I32 => (i32::MIN as i128, i32::MAX as i128),
            MirType::I64 => (i64::MIN as i128, i64::MAX as i128),
            MirType::U8 => (0, u8::MAX as i128),
            MirType::U16 => (0, u16::MAX as i128),
            MirType::U32 => (0, u32::MAX as i128),
            MirType::U64 => (0, u64::MAX as i128),
            _ => (i64::MIN as i128, i64::MAX as i128),
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
    /// Byte size of a user struct/enum by name, or 0 if there's no layout for
    /// it (a runtime-opaque handle, or a type that never got one).
    fn named_layout_size(name: &str, ctx: &CodegenCtx) -> u32 {
        if let Some(l) = ctx.struct_layouts.iter().find(|l| l.name == name) {
            return l.size;
        }
        if let Some(l) = ctx.enum_layouts.iter().find(|l| l.name == name) {
            return l.size;
        }
        0
    }

    fn is_aggregate_field_type(ty: &RaskType, ctx: &CodegenCtx) -> bool {
        match ty {
            // Primitives, opaque pointers — scalar
            RaskType::Unit | RaskType::Bool
            | RaskType::I8 | RaskType::I16 | RaskType::I32 | RaskType::I64 | RaskType::I128
            | RaskType::U8 | RaskType::U16 | RaskType::U32 | RaskType::U64 | RaskType::U128
            | RaskType::F32 | RaskType::F64
            | RaskType::Char
            | RaskType::Fn { .. } | RaskType::Slice(_) => false,
            // Runtime-opaque pointer types (Vec, Map, Pool, Handle, Channel, ...)
            RaskType::UnresolvedGeneric { .. } | RaskType::Generic { .. } => false,
            // A named type is an aggregate when it's a user struct or enum —
            // one with real bytes. Runtime-opaque handles (TcpListener, File,
            // Instant) have empty layouts and stay pointer-sized scalars.
            RaskType::UnresolvedNamed(n) => Self::named_layout_size(n, ctx) > 0,
            RaskType::Named(_) => false,
            // Niche-optimized Option<Handle<T>> — scalar (sentinel value, no tag)
            ty if ty.is_option() && matches!(ty.as_option().unwrap(), RaskType::UnresolvedGeneric { name, .. } if name == "Handle") =>
            {
                false
            }
            // User-defined enums/structs, tuples, arrays, Option (T or none), Result — aggregate
            _ => true,
        }
    }

    /// Get actual (size, align) for a MirType, looking up struct/enum layouts.
    fn real_type_size_align(ty: &MirType, ctx: &CodegenCtx) -> (u32, u32) {
        match ty {
            MirType::Struct(id) => {
                if let Some(layout) = ctx.struct_layouts.get(id.id as usize) {
                    (layout.size as u32, layout.align as u32)
                } else {
                    (8, 8)
                }
            }
            MirType::Enum(id) => {
                if let Some(layout) = ctx.enum_layouts.get(id.id as usize) {
                    (layout.size as u32, layout.align as u32)
                } else {
                    (8, 8)
                }
            }
            _ => (ty.size(), ty.align()),
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

            MirRValue::BinaryOp { op, left, right } => Self::lower_binary_op(builder, op, left, right, expected_ty, ctx),

            MirRValue::UnaryOp { op, operand } => {
                let val = Self::lower_operand_typed(builder, operand, expected_ty, ctx)?;
                let val_ty = builder.func.dfg.value_type(val);

                let result = match op {
                    UnaryOp::Neg if val_ty.is_float() => builder.ins().fneg(val),
                    // A negated integer literal (e.g. `-2147483648`) is a valid
                    // constant, not a runtime negation — fold it so the literal
                    // form of a type's MIN doesn't trip the overflow guard.
                    UnaryOp::Neg if matches!(operand, MirOperand::Constant(MirConst::Int(_))) => {
                        let n = match operand {
                            MirOperand::Constant(MirConst::Int(n)) => *n,
                            _ => unreachable!(),
                        };
                        builder.ins().iconst(val_ty, n.wrapping_neg())
                    }
                    UnaryOp::Neg => {
                        // OV1: negation overflows at signed MIN (and for any
                        // nonzero unsigned value).
                        let unsigned = Self::operand_mir_type(operand, ctx.locals)
                            .map(|t| t.is_unsigned())
                            .unwrap_or(false);
                        let overflowed = if unsigned {
                            let zero = builder.ins().iconst(val_ty, 0);
                            builder.ins().icmp(IntCC::NotEqual, val, zero)
                        } else {
                            let min = builder.ins().iconst(val_ty, Self::type_min_i64(val_ty));
                            builder.ins().icmp(IntCC::Equal, val, min)
                        };
                        Self::guard_overflow(builder, ctx, overflowed, OV_NEG);
                        builder.ins().ineg(val)
                    }
                    // Logical NOT: XOR with 1 to flip the boolean bit.
                    // bnot flips all bits which is wrong for booleans
                    // (e.g. bnot(1) = 0xFE, not 0).
                    UnaryOp::Not => {
                        let val_ty = builder.func.dfg.value_type(val);
                        let one = builder.ins().iconst(val_ty, 1);
                        builder.ins().bxor(val, one)
                    }
                    UnaryOp::BitNot => builder.ins().bnot(val),
                    // Counts come back as the operand's own type, so a
                    // `count_ones()` on an i32 answers in i32 — matching the
                    // checker, which gives these the receiver's type.
                    UnaryOp::CountOnes => builder.ins().popcnt(val),
                    UnaryOp::LeadingZeros => builder.ins().clz(val),
                    UnaryOp::TrailingZeros => builder.ins().ctz(val),
                    UnaryOp::ReverseBits => builder.ins().bitrev(val),
                    UnaryOp::SwapBytes => builder.ins().bswap(val),
                };
                Ok(result)
            }

            MirRValue::Cast { value, target_ty } => {
                if target_ty == &MirType::String {
                    // Casting to string requires a runtime call that writes 16-byte
                    // RaskStr into an out-param. A plain type-cast (convert_value)
                    // would return a scalar, which copy_aggregate would dereference
                    // as a pointer — causing a segfault.
                    let src_mir_ty = Self::operand_mir_type(value, ctx.locals);
                    let (fn_name, arg_ty) = match src_mir_ty.as_ref() {
                        Some(MirType::Bool) => ("rask_bool_to_string", types::I64),
                        Some(MirType::F64) => ("rask_f64_to_string", types::F64),
                        Some(MirType::F32) => ("rask_f32_to_string", types::F32),
                        Some(MirType::Char) => ("rask_char_to_string", types::I32),
                        _ => match value {
                            MirOperand::Constant(MirConst::Bool(_)) => ("rask_bool_to_string", types::I64),
                            MirOperand::Constant(MirConst::Float(_)) => ("rask_f64_to_string", types::F64),
                            MirOperand::Constant(MirConst::Char(_)) => ("rask_char_to_string", types::I32),
                            _ => ("rask_i64_to_string", types::I64),
                        }
                    };

                    let fr = ctx.func_refs.get(fn_name)
                        .ok_or_else(|| CodegenError::FunctionNotFound(fn_name.to_string()))?;

                    let out_ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    ));
                    let out_ptr = builder.ins().stack_addr(types::I64, out_ss, 0);

                    let mut val = Self::lower_operand_typed(builder, value, Some(arg_ty), ctx)?;
                    let val_ty = builder.func.dfg.value_type(val);
                    if val_ty != arg_ty {
                        let src_mir = Self::operand_mir_type(value, ctx.locals);
                        val = Self::convert_value(builder, val, val_ty, arg_ty, src_mir.as_ref());
                    }

                    builder.ins().call(*fr, &[out_ptr, val]);
                    return Ok(out_ptr);
                }

                let val = Self::lower_operand(builder, value, ctx)?;
                let target = mir_to_cranelift_type(target_ty)?;
                let val_ty = builder.func.dfg.value_type(val);
                let src_mir = Self::operand_mir_type(value, ctx.locals);
                Ok(Self::convert_value(builder, val, val_ty, target, src_mir.as_ref()))
            }

            MirRValue::Convert { value, source_ty, target_ty, kind } => {
                Self::lower_convert(builder, value, source_ty, target_ty, *kind, ctx)
            }

            // Struct/enum field access: load from base pointer + field offset
            MirRValue::Field { base, field_index, byte_offset, access } => Self::field_address_and_load(builder, base, field_index, byte_offset, access, expected_ty, ctx),

            // Enum discriminant extraction: load tag byte from base pointer
            MirRValue::EnumTag { value } => {
                let ptr_val = Self::lower_operand(builder, value, ctx)?;
                let base_ty = Self::operand_mir_type(value, ctx.locals);

                let (tag_offset, tag_cranelift_ty) = match &base_ty {
                    Some(MirType::Enum(id)) => {
                        if let Some(layout) = ctx.enum_layouts.get(id.id as usize) {
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

    fn lower_assign(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        rvalue: &MirRValue,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let dst_local = ctx.locals.iter().find(|l| l.id == *dst)
            .ok_or_else(|| CodegenError::UnsupportedFeature("Destination variable not found".to_string()))?;
        let dst_ty = mir_to_cranelift_type(&dst_local.ty)?;

        let mut val = Self::lower_rvalue(builder, rvalue, Some(dst_ty), ctx)?;

        let val_ty = builder.func.dfg.value_type(val);
        if val_ty != dst_ty {
            val = Self::convert_value(builder, val, val_ty, dst_ty, None);
        }

        // #493: an Option destination deeper than its source gains the layers
        // it's missing, rather than being copied over as if the depths matched.
        // `const inner: T?? = slot` where `slot: T?` means "the container had
        // something, and that something is this slot" (type.optionals/OPT28) —
        // copying the 16 bytes straight across would silently reinterpret the
        // inner layer as the outer one.
        if let Some(()) = Self::try_widen_option_depth(builder, dst, &dst_local.ty, rvalue, val, ctx)? {
            return Ok(());
        }

        // When dest is Option(T) and the source is already Option-typed,
        // copy the struct. When the source is a scalar, wrap as Some.
        let src_option_ty = if let MirType::Option(_) = &dst_local.ty {
            if let MirRValue::Use(MirOperand::Local(src_id)) = rvalue {
                ctx.locals.iter().find(|l| l.id == *src_id)
                    .map(|l| matches!(l.ty, MirType::Option(_)))
                    .unwrap_or(false)
            } else {
                false
            }
        } else {
            false
        };

        // Aggregate assignment: when the destination has a stack slot and
        // the rvalue produces a pointer to aggregate data, copy the data
        // into the destination's stack slot rather than aliasing pointers.
        // This covers String (always 16 bytes) and Field extractions from
        // Struct/Tuple/Result/Option that return aggregate sub-fields.
        //
        // Whole-aggregate assignment (`p = q` where both are Struct/Tuple/etc.)
        // also needs a memcpy: aliasing the pointers means a subsequent
        // `mutate p` write would land in `q`'s storage. mem.borrowing/M-rules
        // require `mutate` writes to flow back to the caller, which only
        // works if `p = ...` copies bytes into `*p`'s slot.
        let needs_copy = match (&dst_local.ty, rvalue) {
            (MirType::String, _) => true,
            // Field on aggregate base returns pointer for aggregate elements
            (MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) |
             MirType::Result { .. } | MirType::Option(_), MirRValue::Field { .. }) => true,
            // Whole-aggregate copy: rvalue produces a pointer to the source
            // aggregate, dst has its own storage (either a stack slot or an
            // external pointer for mutate-params).
            (MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_),
             MirRValue::Use(MirOperand::Local(_))) => true,
            // Result/Option whole-aggregate copy: only when src and dst
            // have the same general shape. Avoid clobbering layout when
            // src is Result and dst is Option (different payload offsets).
            (MirType::Result { .. }, MirRValue::Use(MirOperand::Local(src_id))) => {
                ctx.locals.iter().find(|l| l.id == *src_id)
                    .map_or(false, |l| matches!(l.ty, MirType::Result { .. }))
            }
            (MirType::Option(_), MirRValue::Use(MirOperand::Local(src_id))) => {
                ctx.locals.iter().find(|l| l.id == *src_id)
                    .map_or(false, |l| matches!(l.ty, MirType::Option(_)))
            }
            // `try convert`/`try float to int` builds an Option slot and
            // returns its pointer — copy the 16-byte struct into dst.
            (MirType::Option(_), MirRValue::Convert { kind, .. }) => kind.is_optional(),
            // Option(T) assigned from an Option-typed local: copy the 16-byte struct
            (MirType::Option(_), _) if src_option_ty => true,
            _ => false,
        };

        // Option(T) assigned from a non-Option source: wrap as Some
        // in the stack slot. Scalars need this so `const x: i32? = 42`
        // doesn't overwrite x's slot-address with the scalar 42 (later
        // tag loads would dereference 42 as a pointer and SIGSEGV).
        // Aggregates (Struct/Enum/Tuple/String) need it so the bytes
        // land at PAYLOAD_OFFSET of the Option slot, not just the
        // pointer in the first 8 bytes — otherwise field reads
        // through the Option's payload return garbage.
        let wrap_as_some = matches!(&dst_local.ty, MirType::Option(_))
            && !needs_copy
            && ctx.stack_slot_map.contains_key(dst);
        // If the source is an aggregate and dst is Option<aggregate>,
        // we need full-aggregate wrap (tag + memcpy payload), not the
        // scalar wrap.
        let wrap_as_some_aggregate = wrap_as_some
            && matches!(rvalue, MirRValue::Use(MirOperand::Local(_)))
            && if let MirType::Option(inner) = &dst_local.ty {
                matches!(inner.as_ref(),
                    MirType::Struct(_) | MirType::Enum(_) |
                    MirType::Tuple(_) | MirType::String)
            } else { false };

        if needs_copy {
            if let Some((dst_ss, dst_size)) = ctx.stack_slot_map.get(dst) {
                Self::copy_aggregate(builder, val, *dst_ss, *dst_size);
            } else if matches!(&dst_local.ty,
                MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_))
            {
                // Dst variable holds an external pointer (mutate-param) —
                // copy bytes through it instead of overwriting the pointer.
                let size = Self::resolve_type_alloc_size(
                    &dst_local.ty, ctx.struct_layouts, ctx.enum_layouts,
                ).unwrap_or(0);
                if size > 0 {
                    let var = ctx.var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                    let dst_ptr = builder.use_var(*var);
                    Self::copy_aggregate_to_ptr(builder, val, dst_ptr, size);
                } else {
                    let var = ctx.var_map.get(dst)
                        .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                    builder.def_var(*var, val);
                }
            } else {
                let var = ctx.var_map.get(dst)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
                builder.def_var(*var, val);
            }
        } else if wrap_as_some_aggregate {
            // Some(aggregate): tag + payload bytes copied at PAYLOAD_OFFSET.
            let (dst_ss, _) = ctx.stack_slot_map.get(dst).unwrap();
            let inner_size = if let MirType::Option(inner) = &dst_local.ty {
                Self::resolve_type_alloc_size(
                    inner.as_ref(), ctx.struct_layouts, ctx.enum_layouts,
                ).unwrap_or(inner.size())
            } else { 0 };
            Self::build_wrapped_aggregate(builder, *dst_ss, false, 0, val, inner_size);
        } else if wrap_as_some {
            let (dst_ss, _) = ctx.stack_slot_map.get(dst).unwrap();
            Self::build_some(builder, *dst_ss, val);
        } else {
            let var = ctx.var_map.get(dst)
                .ok_or_else(|| CodegenError::UnsupportedFeature("Variable not found".to_string()))?;
            builder.def_var(*var, val);
        }
        Ok(())
    }

    fn lower_store(
        builder: &mut ClifFunctionBuilder,
        addr: &LocalId,
        offset: &u32,
        value: &MirOperand,
        store_size: &Option<u32>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let addr_val = builder.use_var(*ctx.var_map.get(addr)
            .ok_or_else(|| CodegenError::UnsupportedFeature("Address variable not found".to_string()))?);

        // If the value is a stack-allocated aggregate (struct/enum), copy its
        // data instead of storing the pointer. This handles Ok(struct_val) where
        // the struct data must be embedded in the Result's payload area.
        // Use the variable's current value (not the stack_slot address) because
        // the variable may alias another slot (e.g., p = struct_literal result).
        let is_aggregate = if let MirOperand::Local(src_id) = value {
            if let Some((_src_slot, src_size)) = ctx.stack_slot_map.get(src_id) {
                // Use store_size when available to avoid overflowing the
                // destination.
                let effective_size = store_size
                    .map(|ss| ss.min(*src_size))
                    .unwrap_or(*src_size);
                // Copy the bytes even when the aggregate fits in 8 bytes.
                // Storing the pointer instead left the field aimed at the
                // constructing function's frame; it read back fine until
                // something reused that frame, which is how `Request.method`
                // arrived at the middleware with an out-of-range tag (#474).
                // Fields larger than 8 bytes were always copied — this makes
                // the small ones behave the same.
                let src_var = ctx.var_map.get(src_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature("Aggregate source not found".to_string()))?;
                let src_addr = builder.use_var(*src_var);
                Self::copy_bytes(builder, src_addr, 0, addr_val, *offset as i32, effective_size);
                true
            } else { false }
        } else { false };

        // An aggregate-typed *parameter* is a pointer to the caller's data.
        // Params get no stack slot of their own, so they miss the copy above and
        // the pointer itself would land in the field — `create_task(priority:
        // Priority)` stored the caller's stack address where the enum tag
        // belongs, and reading it back trapped on an out-of-range tag. Copy the
        // pointee, same as a slot-backed aggregate.
        let is_aggregate = is_aggregate || Self::copy_from_aggregate_param(
            builder, addr_val, *offset, value, store_size, ctx,
        );

        if !is_aggregate {
            let val = Self::lower_operand(builder, value, ctx)?;

            // store_size > 8: the lowered value is a pointer to aggregate data
            // (e.g., string constant → 16-byte SSO). Copy word-by-word from
            // the source pointer instead of storing the pointer itself.
            if store_size.map_or(false, |s| s > 8) {
                let size = store_size.unwrap();
                Self::copy_bytes(builder, val, 0, addr_val, *offset as i32, size);
            } else {
                let val_ty = builder.func.dfg.value_type(val);
                let flags = MemFlags::new();

                // A field the layout packed into fewer than 8 bytes gets a
                // store that wide. An 8-byte store into a 4-byte slot walks
                // into whatever follows: a two-i32 tuple wrote its second
                // element across the frame's edge and took the return address
                // with it, so the test binary jumped into nowhere (#548).
                if let Some(size @ (1 | 2 | 4)) = *store_size {
                    let narrow = match size {
                        1 => types::I8,
                        2 => types::I16,
                        _ => types::I32,
                    };
                    let val = if val_ty.is_float() {
                        // Only f32 is narrower than a word, and it's already
                        // the right width — store its bits.
                        builder.ins().bitcast(types::I32, MemFlags::new(), val)
                    } else if val_ty.bits() > narrow.bits() {
                        builder.ins().ireduce(narrow, val)
                    } else if val_ty.bits() < narrow.bits() {
                        builder.ins().uextend(narrow, val)
                    } else {
                        val
                    };
                    builder.ins().store(flags, val, addr_val, *offset as i32);
                    return Ok(());
                }

                // Otherwise the slot is a full word. Widen sub-8-byte values to
                // fill it — a 4-byte f32 store would leave stale upper bytes
                // that corrupt the f64 read-back.
                let val = if val_ty == types::F32 {
                    builder.ins().fpromote(types::F64, val)
                } else if val_ty.is_int() && val_ty.bits() < 64 {
                    let src_mir = Self::operand_mir_type(value, ctx.locals);
                    Self::convert_value(builder, val, val_ty, types::I64, src_mir.as_ref())
                } else {
                    val
                };

                builder.ins().store(flags, val, addr_val, *offset as i32);
            }
        }
        Ok(())
    }

    /// Aggregate params are passed by pointer. Where the callee declares a
    /// struct/enum/tuple but the caller's own local is a scalar, spill the value
    /// into a stack slot and pass its address, so the callee's "this is a
    /// pointer" assumption always holds.
    ///
    /// MIR sometimes types an 8-byte struct as a bare `i64` — a payload
    /// extraction that had no checker type to read falls back to it — and then a
    /// value reaches a by-pointer param. Fixing every such fallback is the real
    /// cure; this makes the boundary safe either way.
    fn spill_scalars_for_aggregate_params(
        builder: &mut ClifFunctionBuilder,
        callee: &str,
        arg_vals: &mut [Value],
        mir_args: &[MirOperand],
        ctx: &CodegenCtx,
    ) {
        let Some(param_tys) = ctx.fn_param_types.get(callee) else { return };
        for (i, param_ty) in param_tys.iter().enumerate() {
            if i >= arg_vals.len() {
                break;
            }
            if !matches!(
                param_ty,
                MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::Array { .. }
            ) {
                continue;
            }
            // The operand's own type decides. Aggregate-typed operands already
            // hold a pointer; a scalar-typed one holds the value itself.
            let arg_is_aggregate = Self::operand_arg_type(&mir_args[i], ctx).is_none_or(|ty| {
                matches!(
                    ty,
                    MirType::Struct(_)
                        | MirType::Enum(_)
                        | MirType::Tuple(_)
                        | MirType::Array { .. }
                        | MirType::Ptr
                        | MirType::Handle
                )
            });
            if arg_is_aggregate {
                continue;
            }
            let (size, _) = Self::real_type_size_align(param_ty, ctx);
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                size.max(8),
                0,
            ));
            builder.ins().stack_store(arg_vals[i], ss, 0);
            arg_vals[i] = builder.ins().stack_addr(types::I64, ss, 0);
        }
    }

    /// MIR type of a call argument, looking in both locals and params.
    fn operand_arg_type(operand: &MirOperand, ctx: &CodegenCtx) -> Option<MirType> {
        match operand {
            MirOperand::Local(id) => ctx
                .locals
                .iter()
                .chain(ctx.params.iter())
                .find(|l| l.id == *id)
                .map(|l| l.ty.clone()),
            MirOperand::Constant(_) => None,
        }
    }

    /// Copy a struct/enum/tuple parameter's bytes into a field. Returns false
    /// when `value` isn't such a parameter and the caller should store a scalar.
    ///
    /// Only parameters qualify: every other aggregate local is slot-backed, and
    /// one that isn't means the MIR type doesn't match the representation —
    /// dereferencing it would turn a wrong value into a crash.
    fn copy_from_aggregate_param(
        builder: &mut ClifFunctionBuilder,
        addr_val: Value,
        offset: u32,
        value: &MirOperand,
        store_size: &Option<u32>,
        ctx: &CodegenCtx,
    ) -> bool {
        let MirOperand::Local(src_id) = value else { return false };
        if ctx.stack_slot_map.contains_key(src_id) {
            return false;
        }
        let Some(local) = ctx.params.iter().find(|l| l.id == *src_id) else { return false };
        if !matches!(
            local.ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::Array { .. }
        ) {
            return false;
        }
        let (real_size, _) = Self::real_type_size_align(&local.ty, ctx);
        // Layout gives fields 8-byte slots, so store_size can exceed the type's
        // real size. Copy only what the value actually has.
        let size = store_size.map_or(real_size, |ss| ss.min(real_size));
        if size == 0 {
            return false;
        }
        let Some(src_var) = ctx.var_map.get(src_id) else { return false };
        let src_addr = builder.use_var(*src_var);
        Self::copy_bytes(builder, src_addr, 0, addr_val, offset as i32, size);
        true
    }

    fn lower_pool_checked_access(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        pool: &LocalId,
        handle: &LocalId,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
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
            use crate::layouts::*;

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

            Self::emit_panic_block(builder, panic_block, "pool access with invalid handle", ctx);

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

            Self::emit_panic_block(builder, gen_panic_block, "pool access with invalid handle", ctx);

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
        Ok(())
    }

    fn lower_closure_create(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        func_name: &String,
        captures: &[rask_mir::ClosureCapture],
        heap: &bool,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        // Build environment layout from captures, using real aggregate
        // sizes from codegen layouts instead of MIR fallbacks.
        // MirType::Struct.size() returns 8 (pointer), but actual structs
        // may be 16+ bytes. Escaping closures must deep-copy aggregate
        // data so it survives after the parent's stack is reused.
        let mut env_layout = crate::closures::ClosureEnvLayout::new();
        for c in captures {
            let local = ctx.locals.iter().find(|l| l.id == c.local_id);
            let (real_size, is_aggregate) = if let Some(l) = local {
                if let Some(alloc_size) = Self::resolve_type_alloc_size(
                    &l.ty, ctx.struct_layouts, ctx.enum_layouts,
                ) {
                    (alloc_size, true)
                } else {
                    (c.size, false)
                }
            } else {
                (c.size, false)
            };
            env_layout.add_capture(c.local_id, real_size, is_aggregate);
        }

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
        Ok(())
    }

    fn lower_closure_call(
        builder: &mut ClifFunctionBuilder,
        dst: &Option<LocalId>,
        closure: &LocalId,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
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
        Ok(())
    }

    fn lower_trait_box(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        value: &MirOperand,
        vtable_name: &String,
        concrete_size: &u32,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let alloc_ref = ctx.func_refs.get("rask_alloc")
            .ok_or_else(|| CodegenError::FunctionNotFound("rask_alloc".to_string()))?;

        // Allocate heap memory for the concrete value (min 8 to avoid null from zero-size alloc)
        let alloc_size = std::cmp::max(*concrete_size, 8) as i64;
        let size_val = builder.ins().iconst(types::I64, alloc_size);
        let call_inst = builder.ins().call(*alloc_ref, &[size_val]);
        let data_ptr = builder.inst_results(call_inst)[0];

        // Copy concrete value to heap
        if let MirOperand::Local(src_id) = value {
            if ctx.stack_slot_map.contains_key(src_id) {
                // Aggregate: memcpy from the source pointer the var holds —
                // not the local's own slot, which may be uninitialized when
                // the var aliases another slot (e.g. `_1 = _0`).
                let src_var = ctx.var_map.get(src_id)
                    .ok_or_else(|| CodegenError::UnsupportedFeature(
                        "TraitBox: source variable not found".to_string()
                    ))?;
                let src_ptr = builder.use_var(*src_var);
                let sz = *concrete_size;
                let mut off = 0i32;
                while (off as u32) + 8 <= sz {
                    let word = builder.ins().load(types::I64, MemFlags::new(), src_ptr, off);
                    builder.ins().store(MemFlags::new(), word, data_ptr, off);
                    off += 8;
                }
                if (off as u32) < sz {
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
        builder.ins().store(MemFlags::new(), data_ptr, dst_addr, crate::layouts::FAT_PTR_DATA_OFFSET);
        builder.ins().store(MemFlags::new(), vtable_ptr, dst_addr, crate::layouts::FAT_PTR_VTABLE_OFFSET);

        // Set the variable to point to the stack slot
        let var = ctx.var_map.get(dst)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "TraitBox destination variable not found".to_string()
            ))?;
        builder.def_var(*var, dst_addr);
        Ok(())
    }

    fn lower_trait_call(
        builder: &mut ClifFunctionBuilder,
        dst: &Option<LocalId>,
        trait_object: &LocalId,
        method_name: &String,
        vtable_offset: &u32,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        // Load fat pointer components from trait object stack slot
        let obj_val = builder.use_var(*ctx.var_map.get(trait_object)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                "TraitCall: trait object variable not found".to_string()
            ))?);
        let data_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, crate::layouts::FAT_PTR_DATA_OFFSET);
        let vtable_ptr = builder.ins().load(types::I64, MemFlags::new(), obj_val, crate::layouts::FAT_PTR_VTABLE_OFFSET);

        // Load function pointer from vtable
        let func_ptr = builder.ins().load(
            types::I64, MemFlags::new(), vtable_ptr, *vtable_offset as i32,
        );

        // Build signature: (data_ptr, args...) -> ret.
        //
        // From the operands' own types, not I64 for everything. An aggregate
        // travels as a pointer, but a float travels in a float register — an
        // `f64`-returning trait method declared as returning I64 put the ABI
        // and the callee on different registers entirely.
        let abi_ty = |ty: Option<&MirType>| -> Type {
            match ty {
                Some(
                    MirType::Struct(_)
                    | MirType::Enum(_)
                    | MirType::Tuple(_)
                    | MirType::Array { .. }
                    | MirType::Result { .. }
                    | MirType::Option(_)
                    | MirType::String
                    | MirType::TraitObject { .. },
                ) => types::I64,
                Some(t) => mir_to_cranelift_type(t).unwrap_or(types::I64),
                None => types::I64,
            }
        };

        let mut sig = Signature::new(isa::CallConv::SystemV);
        sig.params.push(AbiParam::new(types::I64)); // data_ptr (self)
        let arg_tys: Vec<MirType> = args
            .iter()
            .map(|a| Self::operand_mir_type(a, ctx.locals).unwrap_or(MirType::I64))
            .collect();
        for ty in &arg_tys {
            sig.params.push(AbiParam::new(abi_ty(Some(ty))));
        }
        let ret_mir = dst
            .as_ref()
            .and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
            .map(|l| l.ty.clone());
        if !matches!(ret_mir, Some(MirType::Void)) {
            sig.returns.push(AbiParam::new(abi_ty(ret_mir.as_ref())));
        }

        // Build argument values
        let mut call_args = Vec::with_capacity(1 + args.len());
        call_args.push(data_ptr);
        for (arg, want) in args.iter().zip(arg_tys.iter()) {
            let want_cl = abi_ty(Some(want));
            let val = Self::lower_operand_typed(builder, arg, Some(want_cl), ctx)?;
            let val_ty = builder.func.dfg.value_type(val);
            let val = if val_ty != want_cl {
                Self::convert_value(builder, val, val_ty, want_cl, Some(want))
            } else {
                val
            };
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
            // Aggregate-returning methods hand back a pointer to data in the
            // callee frame, so copy into the dst's slot before that frame goes
            // away — except when the aggregate fits in a word, where a Rask
            // function returns the value itself (see the Return terminator).
            // Dereferencing that was the crash: a fieldless enum through a
            // vtable read address 1 (#474).
            if let Some((dst_ss, dst_size)) = ctx.stack_slot_map.get(dst_id) {
                if *dst_size <= 8 {
                    builder.ins().stack_store(result, *dst_ss, 0);
                } else {
                    Self::copy_aggregate(builder, result, *dst_ss, *dst_size);
                }
                let addr = builder.ins().stack_addr(types::I64, *dst_ss, 0);
                builder.def_var(*var, addr);
            } else {
                builder.def_var(*var, result);
            }
        }
        Ok(())
    }

    fn lower_binary_op(
        builder: &mut ClifFunctionBuilder,
        op: &BinOp,
        left: &MirOperand,
        right: &MirOperand,
        expected_ty: Option<Type>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        let is_comparison = matches!(op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        );

        // Enums, structs, tuples and unions are held in stack slots and passed
        // around by pointer. A plain `icmp` on the two operands would compare
        // the slot addresses — always unequal for distinct values — so `==`
        // and `!=` must walk the contents instead (#399). This matches the
        // interpreter's `value_eq`: tags then payloads for enums, every field
        // for structs/tuples, and content (not pointer) equality for strings.
        if matches!(op, BinOp::Eq | BinOp::Ne) {
            let agg_ty = Self::operand_mir_type(left, ctx.locals)
                .filter(|t| Self::is_structural_eq_type(t))
                .or_else(|| Self::operand_mir_type(right, ctx.locals)
                    .filter(|t| Self::is_structural_eq_type(t)));
            if let Some(ty) = agg_ty {
                let lhs_ptr = Self::lower_operand(builder, left, ctx)?;
                let rhs_ptr = Self::lower_operand(builder, right, ctx)?;
                let eq = Self::emit_aggregate_eq(builder, ctx, lhs_ptr, rhs_ptr, &ty)?;
                return Ok(if matches!(op, BinOp::Ne) {
                    builder.ins().bxor_imm(eq, 1)
                } else {
                    eq
                });
            }
        }

        // Same reasoning for `<`/`<=`/`>`/`>=`: comparing the two slot
        // addresses answered from allocation order, so `a < b` on a struct was
        // whichever local was declared first. Walk the contents instead —
        // fields in declaration order, enum variant first then payload.
        if matches!(op, BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge) {
            let agg_ty = Self::operand_mir_type(left, ctx.locals)
                .filter(|t| Self::is_structural_ord_type(t))
                .or_else(|| Self::operand_mir_type(right, ctx.locals)
                    .filter(|t| Self::is_structural_ord_type(t)));
            if let Some(ty) = agg_ty {
                let lhs_ptr = Self::lower_operand(builder, left, ctx)?;
                let rhs_ptr = Self::lower_operand(builder, right, ctx)?;
                let cmp = Self::emit_aggregate_cmp(builder, ctx, lhs_ptr, rhs_ptr, &ty)?;
                let cc = match op {
                    BinOp::Lt => IntCC::SignedLessThan,
                    BinOp::Le => IntCC::SignedLessThanOrEqual,
                    BinOp::Gt => IntCC::SignedGreaterThan,
                    _ => IntCC::SignedGreaterThanOrEqual,
                };
                return Ok(builder.ins().icmp_imm(cc, cmp, 0));
            }
        }

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
                (Self::convert_value(builder, lhs_val, lhs_ty, rhs_ty, None), rhs_val)
            } else {
                (lhs_val, Self::convert_value(builder, rhs_val, rhs_ty, lhs_ty, None))
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
            // Checked integer arithmetic (type.overflow OV1–OV4, SH1).
            // The *_overflow instructions give a result + overflow flag;
            // div/shift guards branch to a panic block. Type is the
            // reconciled operand type, so checks are width-correct.
            let int_ty = builder.func.dfg.value_type(lhs_val);
            match op {
                BinOp::Add => {
                    let (res, of) = if is_unsigned {
                        builder.ins().uadd_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().sadd_overflow(lhs_val, rhs_val)
                    };
                    Self::guard_overflow(builder, ctx, of, OV_ADD);
                    res
                }
                BinOp::Sub => {
                    let (res, of) = if is_unsigned {
                        builder.ins().usub_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().ssub_overflow(lhs_val, rhs_val)
                    };
                    Self::guard_overflow(builder, ctx, of, OV_SUB);
                    res
                }
                BinOp::Mul => {
                    let (res, of) = if is_unsigned {
                        builder.ins().umul_overflow(lhs_val, rhs_val)
                    } else {
                        builder.ins().smul_overflow(lhs_val, rhs_val)
                    };
                    Self::guard_overflow(builder, ctx, of, OV_MUL);
                    res
                }
                BinOp::Div if is_unsigned => {
                    if let Some(k) = Self::const_power_of_two(right) {
                        builder.ins().ushr_imm(lhs_val, k as i64)
                    } else {
                        Self::guard_div_zero(builder, ctx, rhs_val, int_ty);
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
                        Self::guard_div_zero(builder, ctx, rhs_val, int_ty);
                        Self::guard_div_overflow(builder, ctx, lhs_val, rhs_val, int_ty);
                        builder.ins().sdiv(lhs_val, rhs_val)
                    }
                }
                BinOp::Mod if is_unsigned => {
                    if let Some(k) = Self::const_power_of_two(right) {
                        let ty = builder.func.dfg.value_type(lhs_val);
                        let mask = builder.ins().iconst(ty, (1i64 << k) - 1);
                        builder.ins().band(lhs_val, mask)
                    } else {
                        Self::guard_div_zero(builder, ctx, rhs_val, int_ty);
                        builder.ins().urem(lhs_val, rhs_val)
                    }
                }
                BinOp::Mod => {
                    Self::guard_div_zero(builder, ctx, rhs_val, int_ty);
                    Self::guard_div_overflow(builder, ctx, lhs_val, rhs_val, int_ty);
                    builder.ins().srem(lhs_val, rhs_val)
                }
                BinOp::BitAnd => builder.ins().band(lhs_val, rhs_val),
                BinOp::BitOr => builder.ins().bor(lhs_val, rhs_val),
                BinOp::BitXor => builder.ins().bxor(lhs_val, rhs_val),
                BinOp::Shl => {
                    Self::guard_shift(builder, ctx, rhs_val, int_ty);
                    builder.ins().ishl(lhs_val, rhs_val)
                }
                BinOp::Shr if is_unsigned => {
                    Self::guard_shift(builder, ctx, rhs_val, int_ty);
                    builder.ins().ushr(lhs_val, rhs_val)
                }
                BinOp::Shr => {
                    Self::guard_shift(builder, ctx, rhs_val, int_ty);
                    builder.ins().sshr(lhs_val, rhs_val)
                }
                // Rotation wraps within the width, so it needs no shift guard:
                // any amount is well-defined.
                BinOp::RotateLeft => builder.ins().rotl(lhs_val, rhs_val),
                BinOp::RotateRight => builder.ins().rotr(lhs_val, rhs_val),
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

    // ─── Structural equality for aggregates (#399) ──────────────────────
    // `==`/`!=` on an aggregate compares contents, not the slot address.
    // Every comparison returns an i8 (0/1). Scalars load at their storage
    // width and `icmp`/`fcmp`; strings and other heap values go through the
    // runtime content comparison; nested aggregates recurse.

    /// Aggregate types whose `==`/`!=` needs a structural (not pointer)
    /// comparison. Strings are already broken into `string_eq` calls during
    /// MIR lowering, so they never reach here as a top-level operand.
    fn is_structural_eq_type(ty: &MirType) -> bool {
        matches!(ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_)
            | MirType::Union(_) | MirType::Array { .. })
    }

    /// Compare two aggregates (pointed to by `lhs`/`rhs`) for structural
    /// equality. Returns an i8 (1 = equal).
    fn emit_aggregate_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::Struct(id) => Self::emit_struct_eq(builder, ctx, lhs, rhs, id.id as usize),
            MirType::Enum(id) => Self::emit_enum_eq(builder, ctx, lhs, rhs, id.id as usize),
            MirType::Tuple(elems) => {
                // Tuple elements are packed at their natural offsets (see the
                // tuple-literal lowering); mirror that packing here.
                let elems = elems.clone();
                let mut acc = builder.ins().iconst(types::I8, 1);
                let mut offset = 0u32;
                for e in &elems {
                    let align = e.align().max(1);
                    offset = (offset + align - 1) & !(align - 1);
                    let l = builder.ins().iadd_imm(lhs, offset as i64);
                    let r = builder.ins().iadd_imm(rhs, offset as i64);
                    let eeq = Self::emit_field_eq_mir(builder, ctx, l, r, e)?;
                    acc = builder.ins().band(acc, eeq);
                    offset += e.size();
                }
                Ok(acc)
            }
            MirType::Array { elem, len } => {
                let stride = elem.size();
                let mut acc = builder.ins().iconst(types::I8, 1);
                for i in 0..*len {
                    let off = (i * stride) as i64;
                    let l = builder.ins().iadd_imm(lhs, off);
                    let r = builder.ins().iadd_imm(rhs, off);
                    let eeq = Self::emit_field_eq_mir(builder, ctx, l, r, elem)?;
                    acc = builder.ins().band(acc, eeq);
                }
                Ok(acc)
            }
            // Unions carry no active-variant tag, so the only defined
            // comparison is over the raw bytes of the widest variant.
            MirType::Union(variants) => {
                let size = variants.iter().map(|v| v.size()).max().unwrap_or(0);
                Ok(Self::emit_bytes_eq(builder, lhs, rhs, size))
            }
            _ => Ok(Self::emit_bytes_eq(builder, lhs, rhs, ty.size())),
        }
    }

    /// Compare every field of the struct at layout index `idx`.
    fn emit_struct_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        idx: usize,
    ) -> CodegenResult<Value> {
        // Snapshot field descriptors so no borrow of `ctx` is held across the
        // recursive field comparisons (which also read `ctx`).
        let fields: Vec<(u32, RaskType, u32)> = {
            let layout = ctx.struct_layouts.get(idx).ok_or_else(|| {
                CodegenError::UnsupportedFeature("struct layout missing for equality".into())
            })?;
            layout.fields.iter().map(|f| (f.offset, f.ty.clone(), f.size)).collect()
        };
        let mut acc = builder.ins().iconst(types::I8, 1);
        for (off, fty, sz) in fields {
            let l = builder.ins().iadd_imm(lhs, off as i64);
            let r = builder.ins().iadd_imm(rhs, off as i64);
            let feq = Self::emit_field_eq_rask(builder, ctx, l, r, &fty, sz)?;
            acc = builder.ins().band(acc, feq);
        }
        Ok(acc)
    }

    /// Compare two enums: tags first, then the payload of the shared variant.
    /// Different tags short-circuit to "not equal".
    fn emit_enum_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        idx: usize,
    ) -> CodegenResult<Value> {
        // (tag value, payload offset, [(field offset, field type, field size)])
        let (tag_off, variants): (i32, Vec<(u64, u32, Vec<(u32, RaskType, u32)>)>) = {
            let layout = ctx.enum_layouts.get(idx).ok_or_else(|| {
                CodegenError::UnsupportedFeature("enum layout missing for equality".into())
            })?;
            let vs = layout.variants.iter().map(|v| {
                let fields = v.fields.iter()
                    .map(|f| (f.offset, f.ty.clone(), f.size))
                    .collect::<Vec<_>>();
                (v.tag, v.payload_offset, fields)
            }).collect();
            (layout.tag_offset as i32, vs)
        };

        let tag_l = builder.ins().load(types::I64, MemFlags::new(), lhs, tag_off);
        let tag_r = builder.ins().load(types::I64, MemFlags::new(), rhs, tag_off);
        let tags_eq = builder.ins().icmp(IntCC::Equal, tag_l, tag_r);

        // Fieldless enum (plain tag union): equality is just tag equality.
        if variants.iter().all(|(_, _, f)| f.is_empty()) {
            return Ok(tags_eq);
        }

        // result = tags_eq && payload matches for the (now shared) variant.
        let merge = builder.create_block();
        builder.append_block_param(merge, types::I8);
        let cmp_payload = builder.create_block();
        let false_v = builder.ins().iconst(types::I8, 0);
        builder.ins().brif(tags_eq, cmp_payload, &[], merge, &[false_v]);

        builder.switch_to_block(cmp_payload);
        builder.seal_block(cmp_payload);
        // `equal_v` dominates the whole chain below (all reached only through
        // cmp_payload), so it is valid to reuse in the trailing jump.
        let equal_v = builder.ins().iconst(types::I8, 1);
        let mut chain_blocks = Vec::new();
        for (tag_val, poff, fields) in variants.iter().filter(|(_, _, f)| !f.is_empty()) {
            let var_block = builder.create_block();
            let next_block = builder.create_block();
            let tv = builder.ins().iconst(types::I64, *tag_val as i64);
            let is_this = builder.ins().icmp(IntCC::Equal, tag_l, tv);
            builder.ins().brif(is_this, var_block, &[], next_block, &[]);

            builder.switch_to_block(var_block);
            builder.seal_block(var_block);
            let mut acc = builder.ins().iconst(types::I8, 1);
            for (foff, fty, sz) in fields {
                let field_off = (*poff + *foff) as i64;
                let l = builder.ins().iadd_imm(lhs, field_off);
                let r = builder.ins().iadd_imm(rhs, field_off);
                let feq = Self::emit_field_eq_rask(builder, ctx, l, r, fty, *sz)?;
                acc = builder.ins().band(acc, feq);
            }
            builder.ins().jump(merge, &[acc]);

            builder.switch_to_block(next_block);
            chain_blocks.push(next_block);
        }
        // Tags matched but the variant carries no payload → equal.
        builder.ins().jump(merge, &[equal_v]);
        for b in chain_blocks {
            builder.seal_block(b);
        }

        builder.switch_to_block(merge);
        builder.seal_block(merge);
        Ok(builder.block_params(merge)[0])
    }

    /// Compare a value of MIR type `ty` held at `lhs`/`rhs`.
    fn emit_field_eq_mir(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::F32 | MirType::F64 => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                Ok(builder.ins().fcmp(FloatCC::Equal, a, b))
            }
            MirType::String => Self::emit_string_eq(builder, ctx, lhs, rhs),
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_)
            | MirType::Union(_) | MirType::Array { .. } => {
                Self::emit_aggregate_eq(builder, ctx, lhs, rhs, ty)
            }
            MirType::Bool | MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
            | MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64
            | MirType::Char | MirType::Handle | MirType::Ptr => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                Ok(builder.ins().icmp(IntCC::Equal, a, b))
            }
            // Option/Result/Slice and friends as a nested element: compare the
            // raw slot bytes. Correct for POD payloads; heap payloads nested
            // this deep aren't content-compared yet.
            _ => Ok(Self::emit_bytes_eq(builder, lhs, rhs, ty.size())),
        }
    }

    /// Compare a struct/enum field of Rask type `ty` held at `lhs`/`rhs`.
    /// Struct and enum-payload fields sit in 8-byte slots, so scalars load as
    /// i64/f64 (see `lower_store`). `size` is the field's slot size, used for
    /// the byte-compare fallback.
    fn emit_field_eq_rask(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &RaskType,
        size: u32,
    ) -> CodegenResult<Value> {
        match ty {
            RaskType::F32 | RaskType::F64 => {
                // Stored promoted to f64 in the 8-byte slot.
                let a = builder.ins().load(types::F64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::F64, MemFlags::new(), rhs, 0);
                Ok(builder.ins().fcmp(FloatCC::Equal, a, b))
            }
            RaskType::Bool
            | RaskType::I8 | RaskType::I16 | RaskType::I32 | RaskType::I64
            | RaskType::U8 | RaskType::U16 | RaskType::U32 | RaskType::U64
            | RaskType::Char => {
                let a = builder.ins().load(types::I64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::I64, MemFlags::new(), rhs, 0);
                Ok(builder.ins().icmp(IntCC::Equal, a, b))
            }
            RaskType::String => Self::emit_string_eq(builder, ctx, lhs, rhs),
            // Nested struct or enum — look it up by name and recurse.
            RaskType::UnresolvedNamed(name) => {
                if let Some(sidx) = ctx.struct_layouts.iter().position(|l| l.name == *name) {
                    Self::emit_struct_eq(builder, ctx, lhs, rhs, sidx)
                } else if let Some(eidx) = ctx.enum_layouts.iter().position(|l| l.name == *name) {
                    Self::emit_enum_eq(builder, ctx, lhs, rhs, eidx)
                } else {
                    // Opaque named type (runtime pointer) — compare the slot.
                    Ok(Self::emit_bytes_eq(builder, lhs, rhs, size))
                }
            }
            // Tuples/arrays/options/opaque pointers: the field occupies exactly
            // `size` bytes and equal values produce identical bytes, so a byte
            // compare is a safe default. Heap contents nested here aren't
            // content-compared yet.
            _ => Ok(Self::emit_bytes_eq(builder, lhs, rhs, size)),
        }
    }

    /// Aggregate types with a defined ordering: structs lexicographically by
    /// declaration order (CO3), enums by variant order then payload (CO1),
    /// tuples and arrays elementwise. Unions have no active-variant tag, so
    /// there's nothing to order them by.
    fn is_structural_ord_type(ty: &MirType) -> bool {
        matches!(ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::Array { .. })
    }

    /// A field with no ordering stops the whole comparison. Returning "equal"
    /// instead would let `<` on the containing struct quietly skip that field
    /// and answer from the next one — a wrong sort with nothing to notice.
    fn unorderable_field(what: &str) -> CodegenError {
        CodegenError::UnsupportedFeature(format!(
            "ordering comparison on a field of type {what} — there is no order \
             defined for it, so the struct can't be ordered either"
        ))
    }

    /// Three-way compare of two aggregates behind `lhs`/`rhs`. Returns i64:
    /// negative, zero, positive. Without this, `<` on a struct compared the two
    /// stack-slot addresses, so the answer depended on allocation order.
    fn emit_aggregate_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::Struct(id) => Self::emit_struct_cmp(builder, ctx, lhs, rhs, id.id as usize),
            MirType::Tuple(elems) => {
                let elems = elems.clone();
                let mut parts = Vec::new();
                let mut offset = 0u32;
                for e in &elems {
                    let align = e.align().max(1);
                    offset = (offset + align - 1) & !(align - 1);
                    parts.push((offset as i64, FieldKind::Mir(e.clone())));
                    offset += e.size();
                }
                Self::emit_lexicographic_cmp(builder, ctx, lhs, rhs, &parts)
            }
            MirType::Array { elem, len } => {
                let stride = elem.size();
                let parts = (0..*len)
                    .map(|i| ((i * stride) as i64, FieldKind::Mir((**elem).clone())))
                    .collect::<Vec<_>>();
                Self::emit_lexicographic_cmp(builder, ctx, lhs, rhs, &parts)
            }
            MirType::Enum(id) => Self::emit_enum_cmp(builder, ctx, lhs, rhs, id.id as usize),
            _ => Err(CodegenError::UnsupportedFeature(
                "ordering comparison on this aggregate type".into(),
            )),
        }
    }

    /// Compare every field of the struct at layout index `idx`, in declaration
    /// order, stopping at the first that differs (CO3).
    fn emit_struct_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        idx: usize,
    ) -> CodegenResult<Value> {
        let parts: Vec<(i64, FieldKind)> = {
            let layout = ctx.struct_layouts.get(idx).ok_or_else(|| {
                CodegenError::UnsupportedFeature("struct layout missing for ordering".into())
            })?;
            layout.fields.iter()
                .map(|f| (f.offset as i64, FieldKind::Rask(f.ty.clone(), f.size)))
                .collect()
        };
        Self::emit_lexicographic_cmp(builder, ctx, lhs, rhs, &parts)
    }

    /// Walk `parts` in order, stopping at the first that differs. Each entry is
    /// a byte offset from the aggregate base plus how to compare what's there.
    fn emit_lexicographic_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        parts: &[(i64, FieldKind)],
    ) -> CodegenResult<Value> {
        let merge = builder.create_block();
        builder.append_block_param(merge, types::I64);

        let mut pending = Vec::new();
        for (off, kind) in parts {
            let l = builder.ins().iadd_imm(lhs, *off);
            let r = builder.ins().iadd_imm(rhs, *off);
            let c = match kind {
                FieldKind::Rask(fty, sz) => {
                    Self::emit_field_cmp_rask(builder, ctx, l, r, fty, *sz)?
                }
                FieldKind::Mir(mty) => Self::emit_field_cmp_mir(builder, ctx, l, r, mty)?,
            };
            let next = builder.create_block();
            let is_eq = builder.ins().icmp_imm(IntCC::Equal, c, 0);
            builder.ins().brif(is_eq, next, &[], merge, &[c]);
            builder.switch_to_block(next);
            pending.push(next);
        }

        let equal = builder.ins().iconst(types::I64, 0);
        builder.ins().jump(merge, &[equal]);
        for b in pending {
            builder.seal_block(b);
        }
        builder.switch_to_block(merge);
        builder.seal_block(merge);
        Ok(builder.block_params(merge)[0])
    }

    /// Enums order by variant first, then by the shared variant's payload (CO1).
    fn emit_enum_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        idx: usize,
    ) -> CodegenResult<Value> {
        let (tag_off, variants): (i32, Vec<(u64, u32, Vec<(u32, RaskType, u32)>)>) = {
            let layout = ctx.enum_layouts.get(idx).ok_or_else(|| {
                CodegenError::UnsupportedFeature("enum layout missing for ordering".into())
            })?;
            let vs = layout.variants.iter().map(|v| {
                let fields = v.fields.iter()
                    .map(|f| (f.offset, f.ty.clone(), f.size))
                    .collect::<Vec<_>>();
                (v.tag, v.payload_offset, fields)
            }).collect();
            (layout.tag_offset as i32, vs)
        };

        let tag_l = builder.ins().load(types::I64, MemFlags::new(), lhs, tag_off);
        let tag_r = builder.ins().load(types::I64, MemFlags::new(), rhs, tag_off);
        let tag_cmp = Self::emit_signed_three_way(builder, tag_l, tag_r);

        if variants.iter().all(|(_, _, f)| f.is_empty()) {
            return Ok(tag_cmp);
        }

        let merge = builder.create_block();
        builder.append_block_param(merge, types::I64);
        let same_tag = builder.create_block();
        let tags_eq = builder.ins().icmp_imm(IntCC::Equal, tag_cmp, 0);
        builder.ins().brif(tags_eq, same_tag, &[], merge, &[tag_cmp]);

        builder.switch_to_block(same_tag);
        builder.seal_block(same_tag);
        let mut chain_blocks = Vec::new();
        for (tag_val, poff, fields) in variants.iter().filter(|(_, _, f)| !f.is_empty()) {
            let var_block = builder.create_block();
            let next_block = builder.create_block();
            let tv = builder.ins().iconst(types::I64, *tag_val as i64);
            let is_this = builder.ins().icmp(IntCC::Equal, tag_l, tv);
            builder.ins().brif(is_this, var_block, &[], next_block, &[]);

            builder.switch_to_block(var_block);
            builder.seal_block(var_block);
            let parts = fields
                .iter()
                .map(|(foff, fty, sz)| ((*poff + *foff) as i64, FieldKind::Rask(fty.clone(), *sz)))
                .collect::<Vec<_>>();
            let c = Self::emit_lexicographic_cmp(builder, ctx, lhs, rhs, &parts)?;
            builder.ins().jump(merge, &[c]);

            builder.switch_to_block(next_block);
            chain_blocks.push(next_block);
        }
        // Same tag, and that variant has no payload → equal.
        let equal = builder.ins().iconst(types::I64, 0);
        builder.ins().jump(merge, &[equal]);
        for b in chain_blocks {
            builder.seal_block(b);
        }

        builder.switch_to_block(merge);
        builder.seal_block(merge);
        Ok(builder.block_params(merge)[0])
    }

    /// `(a > b) - (a < b)` as an i64.
    fn emit_signed_three_way(builder: &mut ClifFunctionBuilder, a: Value, b: Value) -> Value {
        let gt = builder.ins().icmp(IntCC::SignedGreaterThan, a, b);
        let lt = builder.ins().icmp(IntCC::SignedLessThan, a, b);
        let gt = builder.ins().uextend(types::I64, gt);
        let lt = builder.ins().uextend(types::I64, lt);
        builder.ins().isub(gt, lt)
    }

    /// Three-way compare of a struct/enum field of Rask type `ty`. Scalars sit
    /// in 8-byte slots, same as `emit_field_eq_rask`.
    fn emit_field_cmp_rask(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &RaskType,
        size: u32,
    ) -> CodegenResult<Value> {
        match ty {
            RaskType::F32 | RaskType::F64 => {
                let a = builder.ins().load(types::F64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::F64, MemFlags::new(), rhs, 0);
                let gt = builder.ins().fcmp(FloatCC::GreaterThan, a, b);
                let lt = builder.ins().fcmp(FloatCC::LessThan, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            RaskType::U8 | RaskType::U16 | RaskType::U32 | RaskType::U64 => {
                let a = builder.ins().load(types::I64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::I64, MemFlags::new(), rhs, 0);
                let gt = builder.ins().icmp(IntCC::UnsignedGreaterThan, a, b);
                let lt = builder.ins().icmp(IntCC::UnsignedLessThan, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            RaskType::Bool
            | RaskType::I8 | RaskType::I16 | RaskType::I32 | RaskType::I64
            | RaskType::Char => {
                let a = builder.ins().load(types::I64, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(types::I64, MemFlags::new(), rhs, 0);
                Ok(Self::emit_signed_three_way(builder, a, b))
            }
            RaskType::String => Self::emit_string_cmp(builder, ctx, lhs, rhs),
            RaskType::UnresolvedNamed(name) => {
                if let Some(sidx) = ctx.struct_layouts.iter().position(|l| l.name == *name) {
                    Self::emit_struct_cmp(builder, ctx, lhs, rhs, sidx)
                } else if let Some(eidx) = ctx.enum_layouts.iter().position(|l| l.name == *name) {
                    Self::emit_enum_cmp(builder, ctx, lhs, rhs, eidx)
                } else {
                    let _ = size;
                    Err(Self::unorderable_field(&format!("`{}`", name)))
                }
            }
            other => Err(Self::unorderable_field(&format!("`{:?}`", other))),
        }
    }

    /// Three-way compare of a value of MIR type `ty` at `lhs`/`rhs`.
    fn emit_field_cmp_mir(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
        ty: &MirType,
    ) -> CodegenResult<Value> {
        match ty {
            MirType::F32 | MirType::F64 => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                let gt = builder.ins().fcmp(FloatCC::GreaterThan, a, b);
                let lt = builder.ins().fcmp(FloatCC::LessThan, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            MirType::String => Self::emit_string_cmp(builder, ctx, lhs, rhs),
            t if Self::is_structural_ord_type(t) => {
                Self::emit_aggregate_cmp(builder, ctx, lhs, rhs, t)
            }
            MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64 => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                let a = builder.ins().uextend(types::I64, a);
                let b = builder.ins().uextend(types::I64, b);
                let gt = builder.ins().icmp(IntCC::UnsignedGreaterThan, a, b);
                let lt = builder.ins().icmp(IntCC::UnsignedLessThan, a, b);
                let gt = builder.ins().uextend(types::I64, gt);
                let lt = builder.ins().uextend(types::I64, lt);
                Ok(builder.ins().isub(gt, lt))
            }
            MirType::Bool | MirType::I8 | MirType::I16 | MirType::I32 | MirType::I64
            | MirType::Char | MirType::Handle => {
                let lty = mir_to_cranelift_type(ty)?;
                let a = builder.ins().load(lty, MemFlags::new(), lhs, 0);
                let b = builder.ins().load(lty, MemFlags::new(), rhs, 0);
                let a = builder.ins().sextend(types::I64, a);
                let b = builder.ins().sextend(types::I64, b);
                Ok(Self::emit_signed_three_way(builder, a, b))
            }
            other => Err(Self::unorderable_field(&format!("`{:?}`", other))),
        }
    }

    /// Lexicographic string compare via the runtime. Returns i64 (-1/0/1).
    fn emit_string_cmp(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
    ) -> CodegenResult<Value> {
        let fr = ctx.func_refs.get("string_compare")
            .ok_or_else(|| CodegenError::FunctionNotFound("string_compare".into()))?;
        let call = builder.ins().call(*fr, &[lhs, rhs]);
        Ok(builder.inst_results(call)[0])
    }

    /// Content equality of two strings via the runtime. Returns i8 (1 = equal).
    fn emit_string_eq(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        lhs: Value,
        rhs: Value,
    ) -> CodegenResult<Value> {
        let fr = ctx.func_refs.get("string_eq")
            .ok_or_else(|| CodegenError::FunctionNotFound("string_eq".into()))?;
        let call = builder.ins().call(*fr, &[lhs, rhs]);
        let res = builder.inst_results(call)[0];
        Ok(builder.ins().icmp_imm(IntCC::NotEqual, res, 0))
    }

    /// Byte-wise equality of `size` bytes at `lhs`/`rhs`. Returns i8 (1 = equal).
    fn emit_bytes_eq(
        builder: &mut ClifFunctionBuilder,
        lhs: Value,
        rhs: Value,
        size: u32,
    ) -> Value {
        let mut acc = builder.ins().iconst(types::I8, 1);
        let size = size as i32;
        let mut off = 0i32;
        for (chunk, ty) in [(8, types::I64), (4, types::I32), (2, types::I16), (1, types::I8)] {
            while size - off >= chunk {
                let a = builder.ins().load(ty, MemFlags::new(), lhs, off);
                let b = builder.ins().load(ty, MemFlags::new(), rhs, off);
                let e = builder.ins().icmp(IntCC::Equal, a, b);
                acc = builder.ins().band(acc, e);
                off += chunk;
            }
        }
        acc
    }

    fn field_address_and_load(
        builder: &mut ClifFunctionBuilder,
        base: &MirOperand,
        field_index: &u32,
        byte_offset: &Option<u32>,
        access: &FieldAccess,
        expected_ty: Option<Type>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        let base_val = Self::lower_operand(builder, base, ctx)?;
        let base_ty = Self::operand_mir_type(base, ctx.locals);
        let mut load_ty = expected_ty.unwrap_or(types::I64);
        let offset = match &base_ty {
            Some(MirType::Struct(id)) => {
                if let Some(layout) = ctx.struct_layouts.get(id.id as usize) {
                    if let Some(field) = layout.fields.get(*field_index as usize) {
                        // Aggregate field: return pointer into parent struct.
                        // Covers both >8-byte structs and ≤8-byte enums/structs
                        // that use stack-slot representation in codegen.
                        if field.size > 8 || Self::is_aggregate_field_type(&field.ty, ctx) {
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
                // Prefer the exact payload offset match_lower computed for this
                // arm's variant. Guessing "first variant with enough fields"
                // picks the wrong payload shape when variants differ at the same
                // field index (e.g. Pos(Pt) vs Scalar(i32)). An aggregate payload
                // returns a pointer via the field_size > 8 check below (#347).
                if let Some(off) = byte_offset {
                    *off as i32
                } else if let Some(layout) = ctx.enum_layouts.get(id.id as usize) {
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
            // Tuple: compute offset from element types, using actual
            // struct/enum layout sizes instead of MirType::size() fallbacks.
            Some(MirType::Tuple(fields)) => {
                let mut off = 0u32;
                for (i, f) in fields.iter().enumerate() {
                    let (elem_size, elem_align) = Self::real_type_size_align(f, ctx);
                    off = (off + elem_align - 1) & !(elem_align - 1);
                    if i == *field_index as usize {
                        // Aggregate element: return pointer, don't load scalar
                        if elem_size > 8 || matches!(f, MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_)) {
                            let addr = builder.ins().iadd_imm(base_val, off as i64);
                            return Ok(addr);
                        }
                        break;
                    }
                    off += elem_size;
                }
                off as i32
            }
            // Option/Result: payload starts after tag.
            // MIR uses EnumTag for the tag; Field indices are payload-relative.
            Some(MirType::Option(inner)) => {
                // Aggregate payload: return address, not load. A nested Option
                // counts — the payload of a `T??` is a whole `T?` slot, and
                // loading its first 8 bytes as a scalar would hand `tag` a
                // number to dereference (#493).
                if Self::is_boxed_payload(inner.as_ref()) {
                    let payload_addr = builder.ins().iadd_imm(base_val, crate::layouts::PAYLOAD_OFFSET as i64);
                    return Ok(payload_addr);
                }
                crate::layouts::PAYLOAD_OFFSET + (*field_index * 8) as i32
            }
            Some(MirType::Result { ok, err }) => {
                // Use explicit byte_offset when provided (e.g., origin field reads)
                if let Some(off) = byte_offset {
                    // A payload read still hands back the address when the
                    // payload is an aggregate. MIR passes a field_size for
                    // exactly that case and leaves it None for a scalar, which
                    // is what tells the two apart when ok and err disagree
                    // (#389). Without this, unwrapping a `T? or E` loaded the
                    // T?'s first 8 bytes and dereferenced them as a pointer.
                    if *off as i32 == crate::layouts::RESULT_PAYLOAD_OFFSET
                        && matches!(access, FieldAccess::InPlace(_))
                    {
                        let payload_addr = builder
                            .ins()
                            .iadd_imm(base_val, crate::layouts::RESULT_PAYLOAD_OFFSET as i64);
                        return Ok(payload_addr);
                    }
                    *off as i32
                } else {
                    // Aggregate payload (Ok or Err): return address, not load.
                    // MIR uses field_index 0 for both Ok and Err payloads — check both.
                    let is_aggregate = Self::is_boxed_payload;
                    if *field_index == 0 && (is_aggregate(ok.as_ref()) || is_aggregate(err.as_ref())) {
                        let payload_addr = builder.ins().iadd_imm(base_val, crate::layouts::RESULT_PAYLOAD_OFFSET as i64);
                        // If the caller expects a scalar (non-I64), this is a scalar
                        // payload extraction (e.g., Ok value from Result<I32, SomeEnum>).
                        // Load from the payload address instead of returning the address.
                        // Without this, convert_value would truncate the address to I32.
                        if let Some(exp) = expected_ty {
                            if exp != types::I64 {
                                let loaded = builder.ins().load(exp, MemFlags::new(), payload_addr, 0);
                                return Ok(loaded);
                            }
                        }
                        return Ok(payload_addr);
                    }
                    crate::layouts::RESULT_PAYLOAD_OFFSET + (*field_index * 8) as i32
                }
            }
            // Fallback: use pre-computed byte offset from MIR when available
            _ => byte_offset.map(|o| o as i32).unwrap_or((*field_index * 8) as i32)
        };

        // A field that lives in place hands back its address, not a load.
        if access.is_address() {
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
                Self::convert_value(builder, loaded, loaded_ty, exp, None)
            } else {
                loaded
            }
        } else {
            loaded
        };
        Ok(result)
    }

    /// Builtin/intrinsic call dispatch (print, panic, assert_*, Ptr_*, ...).
    /// Returns Ok(true) when the call was a recognized builtin, Ok(false)
    /// otherwise so the caller falls through to extern/ordinary emission.
    fn try_lower_builtin_call(
        builder: &mut ClifFunctionBuilder,
        dst: Option<&LocalId>,
        func: &rask_mir::FunctionRef,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<bool> {
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
                        val = Self::convert_value(builder, val, actual, expected, None);
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
        } else if func.name == "panic" || func.name == "todo" || func.name == "unreachable" {
            // User-level diverging builtin: emit rask_panic_at with the
            // message string and trap. todo/unreachable map to fixed
            // messages when called without args.
            let msg_ptr = if let Some(arg) = args.first() {
                Self::lower_operand_as_cstr(builder, arg, ctx)?
            } else {
                let label = match func.name.as_str() {
                    "todo" => "not yet implemented",
                    "unreachable" => "entered unreachable code",
                    _ => "panic",
                };
                ctx.string_globals.get(label)
                    .map(|gv| builder.ins().global_value(types::I64, *gv))
                    .unwrap_or_else(|| builder.ins().iconst(types::I64, 0))
            };
            if let Some(panic_ref) = ctx.func_refs.get("panic_at") {
                let file_ptr = ctx.source_file.and_then(|f| ctx.string_globals.get(f))
                    .map(|gv| builder.ins().global_value(types::I64, *gv))
                    .unwrap_or_else(|| builder.ins().iconst(types::I64, 0));
                let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                builder.ins().call(*panic_ref, &[file_ptr, line_val, col_val, msg_ptr]);
            }
            builder.ins().trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
            // After a trap, codegen needs to enter an unreachable block
            // so subsequent Cranelift instructions are still well-formed.
            let unreach_block = builder.create_block();
            builder.switch_to_block(unreach_block);
            builder.seal_block(unreach_block);
            return Ok(true);
        } else if func.name == "assert_fail" {
            // MIR already handled branching; this is the fail path.
            // If a message arg is provided, pass it as raw C string pointer.
            if !args.is_empty() {
                let msg_val = Self::lower_operand_as_cstr(builder, &args[0], ctx)?;
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get("assert_fail_msg_at"),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[msg_val, file_ptr, line_val, col_val]);
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
            } else if let Some(file_str) = ctx.source_file {
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
        } else if func.name == "assert_fail_cmp_i64" || func.name == "assert_fail_cmp_char" {
            // Comparison assert failure with scalar values: args = [left, right, op_str].
            // Same shape for both; the char helper formats the codepoints as
            // characters instead of numbers.
            if args.len() >= 3 {
                let left_val = Self::lower_operand_typed(builder, &args[0], Some(types::I64), ctx)?;
                let right_val = Self::lower_operand_typed(builder, &args[1], Some(types::I64), ctx)?;
                let op_val = Self::lower_operand_as_cstr(builder, &args[2], ctx)?;
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get(func.name.as_str()),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[left_val, right_val, op_val, file_ptr, line_val, col_val]);
                    }
                }
            }
        } else if func.name == "assert_fail_cmp_str" {
            // Comparison assert failure with string values: args = [left, right, op_str]
            if args.len() >= 3 {
                let left_val = Self::lower_operand_as_cstr(builder, &args[0], ctx)?;
                let right_val = Self::lower_operand_as_cstr(builder, &args[1], ctx)?;
                let op_val = Self::lower_operand_as_cstr(builder, &args[2], ctx)?;
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get("assert_fail_cmp_str"),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[left_val, right_val, op_val, file_ptr, line_val, col_val]);
                    }
                }
            }
        } else if func.name == "assert_fail_cmp_f64" {
            // Comparison assert failure with f64 values: args = [left, right, op_str]
            if args.len() >= 3 {
                let left_val = Self::lower_operand_typed(builder, &args[0], Some(types::F64), ctx)?;
                let right_val = Self::lower_operand_typed(builder, &args[1], Some(types::F64), ctx)?;
                let op_val = Self::lower_operand_as_cstr(builder, &args[2], ctx)?;
                if let Some(file_str) = ctx.source_file {
                    if let (Some(func_ref), Some(gv)) = (
                        ctx.func_refs.get("assert_fail_cmp_f64"),
                        ctx.string_globals.get(file_str),
                    ) {
                        let file_ptr = builder.ins().global_value(types::I64, *gv);
                        let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                        let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                        builder.ins().call(*func_ref, &[left_val, right_val, op_val, file_ptr, line_val, col_val]);
                    }
                }
            }
        } else if func.name == "check_fail" {
            // check failed — record failure, don't unwind
            let msg = if !args.is_empty() {
                Self::lower_operand_as_cstr(builder, &args[0], ctx)?
            } else {
                // Create a default "check failed" message
                if let Some(gv) = ctx.string_globals.get("check failed") {
                    builder.ins().global_value(types::I64, *gv)
                } else {
                    builder.ins().iconst(types::I64, 0)
                }
            };
            let func_ref = ctx.func_refs.get("rask_check_fail")
                .ok_or_else(|| CodegenError::FunctionNotFound("rask_check_fail".into()))?;
            builder.ins().call(*func_ref, &[msg]);
        } else if func.name == "rask_test_skip" {
            // skip("reason") — pass reason as C string, calls rask_test_skip
            let reason = if !args.is_empty() {
                Self::lower_operand_as_cstr(builder, &args[0], ctx)?
            } else {
                builder.ins().iconst(types::I64, 0)
            };
            let func_ref = ctx.func_refs.get("rask_test_skip")
                .ok_or_else(|| CodegenError::FunctionNotFound("rask_test_skip".into()))?;
            builder.ins().call(*func_ref, &[reason]);
        } else if func.name == "rask_test_expect_fail" {
            // expect_fail() — set thread-local flag
            let func_ref = ctx.func_refs.get("rask_test_expect_fail")
                .ok_or_else(|| CodegenError::FunctionNotFound("rask_test_expect_fail".into()))?;
            builder.ins().call(*func_ref, &[]);
            if let Some(dst_id) = dst {
                if let Some(var) = ctx.var_map.get(dst_id) {
                    let zero = builder.ins().iconst(types::I64, 0);
                    builder.def_var(*var, zero);
                }
            }
        } else if func.name.starts_with("assert_eq_fail") {
            // assert_eq failure: args = [got, expected] (empty for aggregates).
            // MIR already emitted the comparison and branched here.
            let value_args: Vec<Value> = match func.name.as_str() {
                "assert_eq_fail_str" => vec![
                    Self::lower_operand_as_cstr(builder, &args[0], ctx)?,
                    Self::lower_operand_as_cstr(builder, &args[1], ctx)?,
                ],
                "assert_eq_fail_f64" => vec![
                    Self::lower_operand_typed(builder, &args[0], Some(types::F64), ctx)?,
                    Self::lower_operand_typed(builder, &args[1], Some(types::F64), ctx)?,
                ],
                "assert_eq_fail" => Vec::new(),
                _ => vec![
                    Self::lower_operand_typed(builder, &args[0], Some(types::I64), ctx)?,
                    Self::lower_operand_typed(builder, &args[1], Some(types::I64), ctx)?,
                ],
            };
            if let Some(file_str) = ctx.source_file {
                if let (Some(func_ref), Some(gv)) = (
                    ctx.func_refs.get(func.name.as_str()),
                    ctx.string_globals.get(file_str),
                ) {
                    let file_ptr = builder.ins().global_value(types::I64, *gv);
                    let line_val = builder.ins().iconst(types::I32, ctx.current_line as i64);
                    let col_val = builder.ins().iconst(types::I32, ctx.current_col as i64);
                    let mut call_args = value_args;
                    call_args.extend_from_slice(&[file_ptr, line_val, col_val]);
                    builder.ins().call(*func_ref, &call_args);
                }
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
            // Pointer arithmetic: ptr.add(n) → ptr + n*elem_size
            // Element size is passed as the third arg by MIR lowering.
            let ptr_val = Self::lower_operand(builder, &args[0], ctx)?;
            let n_val = Self::lower_operand_typed(builder, &args[1], Some(types::I64), ctx)?;
            let elem_size = if args.len() > 2 {
                Self::lower_operand_typed(builder, &args[2], Some(types::I64), ctx)?
            } else {
                builder.ins().iconst(types::I64, 8)
            };
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
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    /// Extern "C" call — declared signature drives arg types; handles the
    /// string out-param ABI.
    fn lower_extern_call(
        builder: &mut ClifFunctionBuilder,
        dst: Option<&LocalId>,
        func: &rask_mir::FunctionRef,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        // Extern "C" call — use declared signature directly, no stdlib adaptation
        // EXCEPT for string-out-param functions where the C ABI uses an out-param
        // that the Rask source doesn't expose.
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
                    arg_vals.push(Self::convert_value(builder, val, actual, exp, None));
                } else {
                    arg_vals.push(val);
                }
            } else {
                arg_vals.push(val);
            }
        }

        // Inject string out-param for extern C functions that use the
        // out-param ABI (declared with N+1 params, called with N args)
        let needs_out_param = param_types.len() == arg_vals.len() + 1
            && ctx.adapt_table.get(func.name.as_str())
                .map(|(a, _)| *a == ArgAdapt::StringOutParam)
                .unwrap_or(false);
        let out_param_slot = if needs_out_param {
            let ss = dst
                .and_then(|id| ctx.stack_slot_map.get(id))
                .map(|(ss, _)| *ss)
                .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 16, 0,
                )));
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            arg_vals.insert(0, addr);
            Some(ss)
        } else {
            None
        };

        let call_inst = builder.ins().call(*func_ref, &arg_vals);

        if let Some(ss) = out_param_slot {
            // String out-param: result is in the stack slot, define dst var as pointer
            if let Some(dst_id) = dst {
                if let Some(var) = ctx.var_map.get(dst_id) {
                    let addr = builder.ins().stack_addr(types::I64, ss, 0);
                    builder.def_var(*var, addr);
                }
            }
        } else if let Some(dst_id) = dst {
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
                            Self::convert_value(builder, result, val_ty, dst_ty, None)
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
                    let dst_is_option = ctx.locals.iter()
                        .find(|l| l.id == *dst_id)
                        .map(|l| matches!(l.ty, MirType::Option(_)))
                        .unwrap_or(false);
                    if dst_is_option {
                        Self::build_some(builder, *ss, val);
                    } else {
                        Self::build_ok(builder, *ss, val);
                    }
                } else {
                    builder.def_var(*var, val);
                }
            }
        }
        Ok(())
    }

    /// Ordinary Rask call — lowers args, applies stdlib arg/return adaptation.
    fn lower_ordinary_call(
        builder: &mut ClifFunctionBuilder,
        dst: Option<&LocalId>,
        func: &rask_mir::FunctionRef,
        args: &[MirOperand],
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
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
                Self::convert_value(builder, val, actual, types::I64, None)
            } else {
                val
            };
            arg_vals.push(converted);
        }

        Self::spill_scalars_for_aggregate_params(builder, &func.name, &mut arg_vals, args, ctx);

        // Adapt args for typed runtime API
        let adapt = Self::adapt_stdlib_call(builder, &func.name, &mut arg_vals, args, dst, ctx, ctx.adapt_table);

        // Re-read signature after adaptation (arg count may have changed)
        let ext_func = &builder.func.dfg.ext_funcs[*func_ref];
        let sig = &builder.func.dfg.signatures[ext_func.signature];
        let param_types: Vec<Type> = sig.params.iter().map(|p| p.value_type).collect();

        // Convert arg types to match the declared signature
        for (i, val) in arg_vals.iter_mut().enumerate() {
            if let Some(&expected) = param_types.get(i) {
                let actual = builder.func.dfg.value_type(*val);
                if actual != expected {
                    *val = Self::convert_value(builder, *val, actual, expected, None);
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

            // Lock-acquire calls return a pointer to the box's inner value.
            // For a struct payload, bind the dst straight to that pointer — a
            // pointer-alias, exactly like a pool access — so the following
            // method/field access hits the real value and a `mutate` lands in
            // the box rather than a copied slot.
            //
            // Any other payload is stored indirectly: Mutex_new/Shared_new take
            // an address to memcpy from, so a non-struct value gets spilled to a
            // slot first and the box ends up holding the value itself. Binding
            // the pointer there handed `self.counters.lock()` a pointer to the
            // map pointer, and rask_map_get crashed on it (#477) — that payload
            // needs one load. The struct test mirrors the one Mutex_new uses, so
            // the two sides agree on which payloads are indirect.
            if matches!(func.name.as_str(),
                "Mutex_acquire" | "Shared_read_acquire" | "Shared_write_acquire")
            {
                let results = builder.inst_results(call_inst);
                let ptr = if !results.is_empty() {
                    results[0]
                } else {
                    builder.ins().iconst(types::I64, 0)
                };
                let payload_by_address =
                    dst_local.is_some_and(|l| l.ty.passed_by_address());
                let bound = if payload_by_address {
                    ptr
                } else {
                    let load_ty = dst_local
                        .and_then(|l| mir_to_cranelift_type(&l.ty).ok())
                        .unwrap_or(types::I64);
                    builder.ins().load(load_ty, MemFlags::new(), ptr, 0)
                };
                builder.def_var(*var, bound);
                return Ok(());
            }

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

                        // NULL path: none
                        builder.switch_to_block(then_block);
                        builder.seal_block(then_block);
                        Self::build_none(builder, *ss);
                        builder.ins().jump(merge_block, &[]);

                        // non-NULL path: Some(payload copied from ptr)
                        builder.switch_to_block(else_block);
                        builder.seal_block(else_block);
                        let payload_size = *slot_size - crate::layouts::PAYLOAD_OFFSET as u32;
                        Self::build_wrapped_aggregate(builder, *ss, false, 0, ptr, payload_size);
                        builder.ins().jump(merge_block, &[]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        // Return dummy value — real data is in the stack slot
                        builder.ins().iconst(types::I64, 0)
                    } else {
                        // No slot means a niche `Handle?`: the handle itself is
                        // the value and `none` is the all-ones sentinel. A miss
                        // still comes back NULL, so answer with the sentinel
                        // instead of loading through it — `Map<K, Handle<T>>`
                        // segfaulted on every lookup that found nothing (#561).
                        let miss_block = builder.create_block();
                        let hit_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.append_block_param(merge_block, types::I64);

                        let is_null = builder.ins().icmp_imm(IntCC::Equal, ptr, 0);
                        builder.ins().brif(is_null, miss_block, &[], hit_block, &[]);

                        builder.switch_to_block(miss_block);
                        builder.seal_block(miss_block);
                        let sentinel = builder.ins()
                            .iconst(types::I64, crate::layouts::HANDLE_NONE_SENTINEL);
                        builder.ins().jump(merge_block, &[sentinel]);

                        builder.switch_to_block(hit_block);
                        builder.seal_block(hit_block);
                        let loaded = builder.ins().load(types::I64, MemFlags::new(), ptr, 0);
                        builder.ins().jump(merge_block, &[loaded]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                        builder.block_params(merge_block)[0]
                    }
                }
                CallAdapt::PopOutParam(ss) => {
                    // Value was written to stack slot by callee
                    builder.ins().stack_load(types::I64, ss, 0)
                }
                CallAdapt::OptionOutParam(ss) => {
                    // Payload is already in place; 1 means it's there (tag 0),
                    // 0 means there was nothing (tag 1).
                    let results = builder.inst_results(call_inst);
                    let wrote = if !results.is_empty() {
                        results[0]
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    let some_tag = builder.ins().iconst(types::I64, 0);
                    let none_tag = builder.ins().iconst(types::I64, 1);
                    let tag = builder.ins().select(wrote, some_tag, none_tag);
                    builder.ins().stack_store(tag, ss, crate::layouts::TAG_OFFSET);
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id) {
                        if *dst_ss == ss {
                            slot_already_written = true;
                        }
                    }
                    builder.ins().iconst(types::I64, 0)
                }
                CallAdapt::StringOutParam(ss) => {
                    // 16-byte RaskStr written to stack slot — return slot address.
                    // If this slot is the dst's own slot, mark as already written.
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id) {
                        if *dst_ss == ss {
                            slot_already_written = true;
                        }
                    }
                    builder.ins().stack_addr(types::I64, ss, 0)
                }
                CallAdapt::DerefStringElement => {
                    // void* pointing to aggregate data in collection.
                    // Copy `slot_size` bytes into the dst's own slot.
                    let results = builder.inst_results(call_inst);
                    let ptr = if !results.is_empty() { results[0] } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    if let Some((ss, slot_size)) = ctx.stack_slot_map.get(dst_id) {
                        Self::copy_aggregate(builder, ptr, *ss, *slot_size);
                        slot_already_written = true;
                    }
                    ptr
                }
                CallAdapt::RecvStructOk(elem_size) => {
                    // The value is in the buffer the call returns; wrap it as Ok.
                    // Storing the pointer instead left the payload holding an
                    // address, so the received struct read as garbage (#463).
                    let results = builder.inst_results(call_inst);
                    let ptr = if !results.is_empty() { results[0] } else {
                        builder.ins().iconst(types::I64, 0)
                    };
                    if let Some((ss, _)) = ctx.stack_slot_map.get(dst_id) {
                        let is_result = ctx.locals.iter()
                            .find(|l| l.id == *dst_id)
                            .map(|l| matches!(l.ty, MirType::Result { .. }))
                            .unwrap_or(false);
                        Self::build_wrapped_aggregate(builder, *ss, is_result, 0, ptr, elem_size);
                        slot_already_written = true;
                    }
                    ptr
                }
                CallAdapt::TryRecvResult(payload_ss, elem_size) => {
                    // Channel status → `T or E` Result. status==OK(0) →
                    // Ok(payload); anything else (EMPTY/CLOSED) → Err.
                    let results = builder.inst_results(call_inst);
                    let status = if !results.is_empty() { results[0] } else {
                        builder.ins().iconst(types::I64, crate::layouts::TAG_OFFSET as i64)
                    };
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id).copied() {
                        slot_already_written = true;
                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_ok = builder.ins().icmp(IntCC::Equal, status, zero);
                        let ok_block = builder.create_block();
                        let err_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

                        // Ok(payload) copied into the Result slot.
                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let payload_addr = builder.ins().stack_addr(types::I64, payload_ss, 0);
                        Self::build_wrapped_aggregate(
                            builder, dst_ss, true, 0, payload_addr, elem_size,
                        );
                        builder.ins().jump(merge_block, &[]);

                        // Err: tag only (recv failure carries no payload).
                        builder.switch_to_block(err_block);
                        builder.seal_block(err_block);
                        let one = builder.ins().iconst(types::I64, 1);
                        builder.ins().stack_store(one, dst_ss, crate::layouts::TAG_OFFSET);
                        builder.ins().jump(merge_block, &[]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                    }
                    builder.ins().iconst(types::I64, 0)
                }
                CallAdapt::ParseResult(value_ss, writer_ty, ok_ty) => {
                    // parse status → `T or ParseError`. 0 → Ok(value read from
                    // the out-param slot), 1 → Err. The old entry points
                    // returned the value itself with no way to fail, so garbage
                    // input came back as Ok(0) (#472).
                    let results = builder.inst_results(call_inst);
                    let status = if !results.is_empty() {
                        results[0]
                    } else {
                        builder.ins().iconst(types::I64, 1)
                    };
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id).copied() {
                        slot_already_written = true;
                        let zero = builder.ins().iconst(types::I64, 0);
                        let is_ok = builder.ins().icmp(IntCC::Equal, status, zero);
                        let ok_block = builder.create_block();
                        let err_block = builder.create_block();
                        let merge_block = builder.create_block();
                        builder.ins().brif(is_ok, ok_block, &[], err_block, &[]);

                        builder.switch_to_block(ok_block);
                        builder.seal_block(ok_block);
                        let raw = builder.ins().stack_load(writer_ty, value_ss, 0);
                        let value = if writer_ty == ok_ty {
                            raw
                        } else {
                            Self::convert_value(builder, raw, writer_ty, ok_ty, None)
                        };
                        Self::build_ok(builder, dst_ss, value);
                        builder.ins().jump(merge_block, &[]);

                        // Err carries no payload — ParseError is fieldless.
                        builder.switch_to_block(err_block);
                        builder.seal_block(err_block);
                        let one = builder.ins().iconst(types::I64, 1);
                        builder.ins().stack_store(one, dst_ss, crate::layouts::TAG_OFFSET);
                        Self::zero_result_origin(builder, dst_ss);
                        builder.ins().jump(merge_block, &[]);

                        builder.switch_to_block(merge_block);
                        builder.seal_block(merge_block);
                    }
                    builder.ins().iconst(types::I64, 0)
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
                    Self::convert_value(builder, val, val_ty, dst_ty, None)
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
                } else if ctx.adapt_table.get(&func.name)
                    .map(|(_, r)| *r == RetAdapt::NegErr)
                    .unwrap_or(false)
                {
                    // C function uses negative return = error convention
                    // (declared as RetAdapt::NegErr on the dispatch entry).
                    Self::wrap_result_into_slot(builder, final_val, *ss);
                } else if ctx.adapt_table.get(&func.name)
                    .map(|(_, r)| *r == RetAdapt::NegNone)
                    .unwrap_or(false)
                {
                    // Negative return = `none` (find/rfind's -1).
                    Self::wrap_option_into_slot(builder, final_val, *ss);
                } else {
                    // C stdlib function returns a plain value (not a pointer to an aggregate).
                    // Wrap as Some/Ok depending on destination type.
                    let dst_is_option = ctx.locals.iter()
                        .find(|l| l.id == *dst_id)
                        .map(|l| matches!(l.ty, MirType::Option(_)))
                        .unwrap_or(false);
                    if dst_is_option {
                        Self::build_some(builder, *ss, final_val);
                    } else {
                        Self::build_ok(builder, *ss, final_val);
                    }
                }
            } else {
                builder.def_var(*var, final_val);
            }
            } // !is_void
        }
        Ok(())
    }

    fn lower_terminator(
        builder: &mut ClifFunctionBuilder,
        term: &MirTerminator,
        ctx: &CodegenCtx,
        cleanup_chain_blocks: &HashMap<Vec<BlockId>, cranelift_codegen::ir::Block>,
    ) -> CodegenResult<()> {
        match &term.kind {
            MirTerminatorKind::Return { value } => {
                // main is called from C as void rask_main(void) — always return
                // void. A `void or E` main still has to report its error branch,
                // though: exit 1, not the silent 0 it used to give (#345).
                if ctx.is_main {
                    Self::emit_main_error_check(builder, value.as_ref(), ctx)?;
                    builder.ins().return_(&[]);
                } else if let Some(stack_info) = Self::return_stack_info(value.as_ref(), ctx.stack_slot_map) {
                    // For small aggregate return values (≤8 bytes) in stack slots:
                    //   - If the function return type is Result/Option, the small value
                    //     is a component (ok or err), not a Result — must be wrapped.
                    //   - Otherwise load the scalar and return it directly.
                    // For larger aggregates, return the stack slot address. The caller
                    // copies from it immediately via copy_aggregate (the callee stack
                    // is still accessible at that point on x86-64).
                    let (local_id, ss, size) = stack_info;
                    if size <= 8 {
                        let loaded = builder.ins().stack_load(types::I64, ss, 0);
                        if matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_)) {
                            // Small component in a Result/Option-returning function.
                            // Wrap it as Ok or Err by checking the local's type.
                            let slot_size = Self::resolve_type_alloc_size(
                                ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts
                            ).unwrap_or(16);
                            let ret_ss = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot, slot_size, 0,
                            ));
                            let local_ty = ctx.locals.iter()
                                .find(|l| l.id == local_id)
                                .map(|l| &l.ty);
                            if Self::is_err_component(ctx.ret_ty, local_ty) {
                                Self::build_err(builder, ret_ss, loaded);
                            } else if matches!(ctx.ret_ty, MirType::Option(_)) {
                                Self::build_some(builder, ret_ss, loaded);
                            } else {
                                Self::build_ok(builder, ret_ss, loaded);
                            }
                            let addr = builder.ins().stack_addr(types::I64, ret_ss, 0);
                            builder.ins().return_(&[addr]);
                        } else {
                            builder.ins().return_(&[loaded]);
                        }
                    } else {
                        // >8-byte aggregate in a stack slot. If the function returns
                        // Result/Option but this local is a bare ok/err component
                        // (not an already-wrapped Result), wrap it — otherwise the
                        // callee hands back the raw payload and the caller reads the
                        // tag from the payload's first bytes (#347).
                        let local_ty = ctx.locals.iter()
                            .find(|l| l.id == local_id)
                            .map(|l| l.ty.clone());
                        let already_wrapped = local_ty.as_ref() == Some(ctx.ret_ty);
                        let needs_wrap = !already_wrapped
                            && matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_));
                        if needs_wrap {
                            let is_err = Self::is_err_component(ctx.ret_ty, local_ty.as_ref());
                            let is_result = matches!(ctx.ret_ty, MirType::Result { .. });
                            let slot_size = Self::resolve_type_alloc_size(
                                ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts,
                            ).unwrap_or(16);
                            let ret_ss = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot, slot_size, 0,
                            ));
                            let src = builder.ins().stack_addr(types::I64, ss, 0);
                            Self::build_wrapped_aggregate(
                                builder, ret_ss, is_result, if is_err { 1 } else { 0 }, src, size,
                            );
                            let addr = builder.ins().stack_addr(types::I64, ret_ss, 0);
                            builder.ins().return_(&[addr]);
                        } else {
                            // Return pointer to stack slot data for copy_aggregate
                            Self::emit_return(builder, value.as_ref(), ctx)?;
                        }
                    }
                } else if matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_)) {
                    // Function returns Result/Option but value is a plain scalar
                    // or non-stack-slotted local (e.g. `return 42` or
                    // `return DivError {}` from a `i32 or DivError` fn).
                    // Wrap as Ok/Some — or Err when the value's MIR type matches
                    // the Result's err side. Without the Err detection, returning
                    // a struct-typed error from a Result-returning function
                    // wrapped it as Ok and caused `is X` checks at the call site
                    // to silently take the wrong branch (#259 family).
                    let slot_size = Self::resolve_type_alloc_size(ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts)
                        .unwrap_or(16);
                    let ss = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        slot_size,
                        0,
                    ));
                    let val_ty = value.as_ref().and_then(|v| match v {
                        MirOperand::Local(id) => ctx.locals.iter()
                            .find(|l| l.id == *id)
                            .map(|l| l.ty.clone()),
                        _ => None,
                    });
                    let is_err_value = Self::is_err_component(ctx.ret_ty, val_ty.as_ref());
                    let val = if let Some(val_op) = value.as_ref() {
                        Self::lower_operand_typed(builder, val_op, Some(types::I64), ctx)?
                    } else {
                        builder.ins().iconst(types::I64, 0)
                    };

                    // Aggregate payload (string/struct/...) — `val` is a pointer to
                    // 16+ bytes. Copy into the payload area instead of storing the
                    // pointer at offset 8 (which leaves the rest of the slot
                    // uninitialized).
                    let inner_branch = if is_err_value {
                        match ctx.ret_ty {
                            MirType::Result { err, .. } => Some(err.as_ref()),
                            _ => None,
                        }
                    } else {
                        match ctx.ret_ty {
                            MirType::Option(inner) => Some(inner.as_ref()),
                            MirType::Result { ok, .. } => Some(ok.as_ref()),
                            _ => None,
                        }
                    };
                    let inner_aggregate = inner_branch.filter(|t| matches!(t,
                        MirType::String | MirType::Struct(_) | MirType::Enum(_)
                        | MirType::Tuple(_) | MirType::Result { .. } | MirType::Option(_)
                    ));
                    if let Some(inner) = inner_aggregate {
                        let inner_size = Self::resolve_type_alloc_size(
                            inner, ctx.struct_layouts, ctx.enum_layouts,
                        ).unwrap_or(inner.size());
                        let is_result = matches!(ctx.ret_ty, MirType::Result { .. });
                        Self::build_wrapped_aggregate(
                            builder, ss, is_result, if is_err_value { 1 } else { 0 }, val, inner_size,
                        );
                    } else if is_err_value {
                        Self::build_err(builder, ss, val);
                    } else if matches!(ctx.ret_ty, MirType::Option(_)) {
                        Self::build_some(builder, ss, val);
                    } else {
                        Self::build_ok(builder, ss, val);
                    }
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
                        // main is void — never pass a return value.
                        if ctx.is_main {
                            builder.ins().jump(shared_block, &[]);
                        } else if let Some(val_op) = value {
                            let expected_ty = mir_to_cranelift_type(ctx.ret_ty)?;
                            let val = Self::lower_operand_typed(builder, val_op, Some(expected_ty), ctx)?;
                            let actual_ty = builder.func.dfg.value_type(val);
                            let final_val = if actual_ty != expected_ty {
                                Self::convert_value(builder, val, actual_ty, expected_ty, None)
                            } else {
                                val
                            };
                            builder.ins().jump(shared_block, &[final_val]);
                        } else if matches!(ctx.ret_ty, MirType::Void) {
                            builder.ins().jump(shared_block, &[]);
                        } else {
                            // A bare `return` in a value-returning function, e.g.
                            // the success exit of a `void or E`. The shared cleanup
                            // block takes the return value as a block parameter, so
                            // this jump has to supply one too — several
                            // cleanup_returns share one block and the ones carrying
                            // an error do pass it. Jumping with no argument left the
                            // block signature unsatisfied and Cranelift's verifier
                            // rejected the function (#463).
                            let placeholder = Self::empty_return_value(builder, ctx)?;
                            builder.ins().jump(shared_block, &[placeholder]);
                        }
                    } else {
                        // Fallback: inline (shouldn't happen with the setup above)
                        if ctx.is_main {
                            builder.ins().return_(&[]);
                        } else {
                            Self::emit_return(builder, value.as_ref(), ctx)?;
                        }
                    }
                } else {
                    // Empty cleanup chain — just return directly
                    if ctx.is_main {
                        builder.ins().return_(&[]);
                    } else {
                        Self::emit_return(builder, value.as_ref(), ctx)?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Branch to a cold panic block when `overflowed` is nonzero, then continue
    /// in a fresh block. Used for the checked-arithmetic guards (type.overflow).
    fn guard_overflow(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
        overflowed: Value,
        msg: &str,
    ) {
        let panic_block = builder.create_block();
        let cont_block = builder.create_block();
        builder.ins().brif(overflowed, panic_block, &[], cont_block, &[]);
        Self::emit_panic_block(builder, panic_block, msg, ctx);
        builder.switch_to_block(cont_block);
        builder.seal_block(cont_block);
    }

    /// OV2: panic (with a message) when the divisor is zero.
    fn guard_div_zero(builder: &mut ClifFunctionBuilder, ctx: &CodegenCtx, rhs: Value, ty: Type) {
        let zero = builder.ins().iconst(ty, 0);
        let is_zero = builder.ins().icmp(IntCC::Equal, rhs, zero);
        Self::guard_overflow(builder, ctx, is_zero, OV_DIV_ZERO);
    }

    /// OV3: panic when a signed division would overflow (`MIN / -1`).
    fn guard_div_overflow(builder: &mut ClifFunctionBuilder, ctx: &CodegenCtx, lhs: Value, rhs: Value, ty: Type) {
        let min = builder.ins().iconst(ty, Self::type_min_i64(ty));
        let neg1 = builder.ins().iconst(ty, -1);
        let l_is_min = builder.ins().icmp(IntCC::Equal, lhs, min);
        let r_is_neg1 = builder.ins().icmp(IntCC::Equal, rhs, neg1);
        let both = builder.ins().band(l_is_min, r_is_neg1);
        Self::guard_overflow(builder, ctx, both, OV_DIV_OVERFLOW);
    }

    /// SH1: panic when the shift amount is >= the operand's bit width.
    /// Unsigned comparison also catches negative amounts.
    fn guard_shift(builder: &mut ClifFunctionBuilder, ctx: &CodegenCtx, amount: Value, ty: Type) {
        let bits = builder.ins().iconst(ty, ty.bits() as i64);
        let bad = builder.ins().icmp(IntCC::UnsignedGreaterThanOrEqual, amount, bits);
        Self::guard_overflow(builder, ctx, bad, OV_SHIFT);
    }

    /// Signed minimum of an integer type as an i64 immediate.
    fn type_min_i64(ty: Type) -> i64 {
        match ty.bits() {
            8 => i8::MIN as i64,
            16 => i16::MIN as i64,
            32 => i32::MIN as i64,
            _ => i64::MIN,
        }
    }

    /// Emit a cold panic block: call rask_panic_at with the given message, then trap.
    /// The block is sealed immediately (single predecessor expected).
    fn emit_panic_block(
        builder: &mut ClifFunctionBuilder,
        block: cranelift_codegen::ir::Block,
        msg: &str,
        ctx: &CodegenCtx,
    ) {
        builder.switch_to_block(block);
        builder.seal_block(block);
        builder.set_cold_block(block);
        if let (Some(panic_ref), Some(msg_gv)) = (
            ctx.func_refs.get("panic_at"),
            ctx.string_globals.get(msg),
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
    }

    /// Emit a return instruction.
    /// The value a bare `return` yields in a function that returns something.
    ///
    /// For `T or E` / `T?` that's a wrapped ok/some with a zero payload — the
    /// shape the plain `Return` path builds — handed back as the address of the
    /// result slot. Anything else gets a zero of the return type.
    fn empty_return_value(
        builder: &mut ClifFunctionBuilder,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        if matches!(ctx.ret_ty, MirType::Result { .. } | MirType::Option(_)) {
            let slot_size = Self::resolve_type_alloc_size(
                ctx.ret_ty, ctx.struct_layouts, ctx.enum_layouts,
            ).unwrap_or(16);
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot, slot_size, 0,
            ));
            let zero = builder.ins().iconst(types::I64, 0);
            if matches!(ctx.ret_ty, MirType::Option(_)) {
                Self::build_some(builder, ss, zero);
            } else {
                Self::build_ok(builder, ss, zero);
            }
            return Ok(builder.ins().stack_addr(types::I64, ss, 0));
        }
        let ret_cl_ty = mir_to_cranelift_type(ctx.ret_ty)?;
        Ok(if ret_cl_ty.is_float() {
            builder.ins().f64const(0.0)
        } else {
            builder.ins().iconst(ret_cl_ty, 0)
        })
    }

    /// struct.targets/EX4: `func main() -> void or E` returning its error branch
    /// exits 1. Reads the Result tag; on the error side, calls the error type's
    /// `message()` when it has one and hands the text to the runtime, which
    /// prints it and exits. The ok side falls through to the normal return.
    fn emit_main_error_check(
        builder: &mut ClifFunctionBuilder,
        value: Option<&MirOperand>,
        ctx: &CodegenCtx,
    ) -> CodegenResult<()> {
        let MirType::Result { err, .. } = ctx.ret_ty else {
            return Ok(());
        };
        let Some(val_op) = value else { return Ok(()) };
        let Some(&exit_fr) = ctx.func_refs.get("main_error_exit") else {
            return Ok(());
        };

        let base = Self::lower_operand(builder, val_op, ctx)?;
        let msg_fr = Self::aggregate_type_name(err, ctx)
            .and_then(|name| ctx.func_refs.get(&format!("{}_message", name)).copied());

        // `return SomeError` in a `void or E` main lowers to the bare error
        // value, not a wrapped Result — that path is unconditionally an error.
        let local_ty = Self::operand_mir_type(val_op, ctx.locals);
        if Self::is_err_component(ctx.ret_ty, local_ty.as_ref()) {
            let msg = Self::call_message(builder, msg_fr, base);
            builder.ins().call(exit_fr, &[msg]);
            return Ok(());
        }

        // Wrapped Result: branch on the tag.
        if !matches!(local_ty, Some(MirType::Result { .. })) {
            return Ok(());
        }
        let tag = builder.ins().load(types::I64, MemFlags::new(), base, 0);
        let is_err = builder.ins().icmp_imm(IntCC::NotEqual, tag, 0);

        let err_block = builder.create_block();
        let ok_block = builder.create_block();
        builder.ins().brif(is_err, err_block, &[], ok_block, &[]);

        builder.switch_to_block(err_block);
        builder.seal_block(err_block);
        // `{ErrType}_message(payload) -> string` when the error type defines one.
        // Without it there's nothing to print but the fact, so pass null.
        let payload = builder
            .ins()
            .iadd_imm(base, crate::layouts::RESULT_PAYLOAD_OFFSET as i64);
        let msg = Self::call_message(builder, msg_fr, payload);
        builder.ins().call(exit_fr, &[msg]);
        // rask_main_error_exit is _Noreturn, but the block still needs a
        // terminator for the verifier.
        builder.ins().trap(TrapCode::user(1).unwrap());

        builder.switch_to_block(ok_block);
        builder.seal_block(ok_block);
        Ok(())
    }

    /// Call `{ErrType}_message(err) -> string` and copy the 16 bytes out.
    ///
    /// An aggregate return is a pointer to the callee's own storage, so the
    /// convention everywhere is: copy before doing anything else. Calls with a
    /// MIR destination get that copy for free — `stack_slot_map` gives them a
    /// caller-owned slot. This one is hand-rolled and has no destination local,
    /// so it does its own copy; the next call would otherwise reuse the frame
    /// the pointer names, and the next call here is the one that prints it.
    ///
    /// Returns a null pointer when the error type has no `message()`.
    fn call_message(
        builder: &mut ClifFunctionBuilder,
        msg_fr: Option<FuncRef>,
        err_ptr: Value,
    ) -> Value {
        let Some(fr) = msg_fr else {
            return builder.ins().iconst(types::I64, 0);
        };
        let call = builder.ins().call(fr, &[err_ptr]);
        let src = builder.inst_results(call)[0];
        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot, 16, 0,
        ));
        for off in [0i32, 8] {
            let word = builder.ins().load(types::I64, MemFlags::new(), src, off);
            builder.ins().stack_store(word, ss, off);
        }
        builder.ins().stack_addr(types::I64, ss, 0)
    }

    /// The declared name behind a struct or enum MIR type, for mangled-name
    /// lookups like `{Type}_message`.
    fn aggregate_type_name(ty: &MirType, ctx: &CodegenCtx) -> Option<String> {
        match ty {
            MirType::Struct(id) => {
                ctx.struct_layouts.get(id.id as usize).map(|l| l.name.clone())
            }
            MirType::Enum(id) => ctx.enum_layouts.get(id.id as usize).map(|l| l.name.clone()),
            _ => None,
        }
    }

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
                Self::convert_value(builder, val, actual_ty, expected_ty, None)
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
            MirType::Struct(id) => struct_layouts.get(id.id as usize).map(|sl| sl.size),
            MirType::Enum(id) => enum_layouts.get(id.id as usize).map(|el| el.size),
            MirType::Array { elem, len } => Some(elem.size() * len),
            MirType::Result { ok, err } => {
                let ok_size = Self::resolve_type_alloc_size(ok, struct_layouts, enum_layouts)
                    .unwrap_or(ok.size());
                let err_size = Self::resolve_type_alloc_size(err, struct_layouts, enum_layouts)
                    .unwrap_or(err.size());
                // tag (8) + origin_file (8) + origin_line (8) + payload.
                // Scalars are stored as 8-byte values in codegen; .max(8) prevents OOB writes.
                Some(crate::layouts::RESULT_PAYLOAD_OFFSET as u32 + ok_size.max(8).max(err_size.max(8)))
            }
            // A `Handle?` is a niche: `none` is a sentinel handle, so the value
            // is one word with no tag and needs no slot. Giving it the tagged
            // layout made the local hold a slot address, and comparing that
            // against the sentinel was always false (#438).
            MirType::Option(inner) if **inner == MirType::Handle => None,
            MirType::Option(inner) => {
                let inner_size = Self::resolve_type_alloc_size(inner, struct_layouts, enum_layouts)
                    .unwrap_or(inner.size());
                // Scalars are stored as 8-byte values in codegen; .max(8) prevents OOB writes.
                Some(8 + inner_size.max(8))
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
            MirType::String => Some(16),
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

    /// Copy `size` bytes from `src_ptr + src_off` to `dst_ptr + dst_off`.
    /// Delegates to the crate-wide [`copy_bytes`] — see it for why there's
    /// exactly one of these.
    fn copy_bytes(
        builder: &mut ClifFunctionBuilder,
        src_ptr: Value,
        src_off: i32,
        dst_ptr: Value,
        dst_off: i32,
        size: u32,
    ) {
        copy_bytes(builder, src_ptr, src_off, dst_ptr, dst_off, size);
    }

    /// Copy aggregate data from a source pointer into a caller-owned stack slot.
    /// Used after calls that return aggregate types (struct, enum, Result, etc.)
    /// to avoid dangling pointers to callee stack frames.
    fn copy_aggregate(builder: &mut ClifFunctionBuilder, src_ptr: Value, dst_slot: StackSlot, size: u32) {
        Self::copy_aggregate_at(builder, src_ptr, dst_slot, 0, size);
    }

    /// Copy `size` bytes from `src_ptr` into `dst_slot` starting at `dst_off`.
    fn copy_aggregate_at(
        builder: &mut ClifFunctionBuilder,
        src_ptr: Value,
        dst_slot: StackSlot,
        dst_off: i32,
        size: u32,
    ) {
        let dst_ptr = builder.ins().stack_addr(types::I64, dst_slot, 0);
        Self::copy_bytes(builder, src_ptr, 0, dst_ptr, dst_off, size);
    }

    /// Copy `size` bytes from `src_ptr` to `dst_ptr`. Mirror of `copy_aggregate`
    /// but for through-pointer destinations (mutate-params, where the dst's
    /// variable holds an external pointer instead of a stack-slot address).
    fn copy_aggregate_to_ptr(builder: &mut ClifFunctionBuilder, src_ptr: Value, dst_ptr: Value, size: u32) {
        Self::copy_bytes(builder, src_ptr, 0, dst_ptr, 0, size);
    }

    // ─── Option/Result slot constructors ────────────────────────
    // One place that knows the tag + origin-fields + payload layout, keyed off
    // the rask_mono::abi offsets. Every wrapping site routes through these
    // instead of re-emitting the store sequence inline.
    //
    //   Result slot: [tag:8][origin_file:8][origin_line:8][payload]
    //   Option slot: [tag:8][payload]     (no origin fields)
    // Tag 0 = Ok/Some, tag 1 = Err/none.

    /// Zero the Result origin-file/line fields. Ok and C-FFI errors have no
    /// Rask origin; real Err origins are filled by the error-construction path.
    fn zero_result_origin(builder: &mut ClifFunctionBuilder, slot: StackSlot) {
        let zero = builder.ins().iconst(types::I64, 0);
        builder.ins().stack_store(zero, slot, crate::layouts::ORIGIN_FILE_OFFSET);
        builder.ins().stack_store(zero, slot, crate::layouts::ORIGIN_LINE_OFFSET);
    }

    /// Ok(scalar) into a Result slot.
    fn build_ok(builder: &mut ClifFunctionBuilder, slot: StackSlot, payload: Value) {
        let tag = builder.ins().iconst(types::I64, 0);
        builder.ins().stack_store(tag, slot, crate::layouts::TAG_OFFSET);
        Self::zero_result_origin(builder, slot);
        builder.ins().stack_store(payload, slot, crate::layouts::RESULT_PAYLOAD_OFFSET);
    }

    /// Err(scalar) into a Result slot (origin zeroed — no source location here).
    fn build_err(builder: &mut ClifFunctionBuilder, slot: StackSlot, payload: Value) {
        let tag = builder.ins().iconst(types::I64, 1);
        builder.ins().stack_store(tag, slot, crate::layouts::TAG_OFFSET);
        Self::zero_result_origin(builder, slot);
        builder.ins().stack_store(payload, slot, crate::layouts::RESULT_PAYLOAD_OFFSET);
    }

    /// Some(scalar) into an Option slot. Option layout has no origin fields —
    /// using the Result constructor here would write past the end of the slot.
    fn build_some(builder: &mut ClifFunctionBuilder, slot: StackSlot, payload: Value) {
        let tag = builder.ins().iconst(types::I64, 0);
        builder.ins().stack_store(tag, slot, crate::layouts::TAG_OFFSET);
        builder.ins().stack_store(payload, slot, crate::layouts::PAYLOAD_OFFSET);
    }

    /// none into an Option slot (tag only — payload is dead).
    fn build_none(builder: &mut ClifFunctionBuilder, slot: StackSlot) {
        let tag = builder.ins().iconst(types::I64, 1);
        builder.ins().stack_store(tag, slot, crate::layouts::TAG_OFFSET);
    }

    /// Payload types that live in their own storage, so extracting one yields
    /// an address rather than a loaded scalar. Nested `Option`/`Result` belong
    /// here: a `T??` payload is a whole 16-byte `T?` slot (#493).
    fn is_boxed_payload(ty: &MirType) -> bool {
        matches!(
            ty,
            MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::String
                | MirType::Option(_)
                | MirType::Result { .. }
        )
    }

    /// How many optional layers a MIR type carries.
    fn option_depth(ty: &MirType) -> usize {
        match ty {
            MirType::Option(inner) => 1 + Self::option_depth(inner),
            _ => 0,
        }
    }

    /// `ty` peeled back to `depth` optional layers.
    fn option_type_at_depth(ty: &MirType, depth: usize) -> &MirType {
        let mut cur = ty;
        let mut d = Self::option_depth(ty);
        while d > depth {
            match cur {
                MirType::Option(inner) => {
                    cur = inner;
                    d -= 1;
                }
                _ => break,
            }
        }
        cur
    }

    /// Optional depth of an rvalue's source, when it can be known statically.
    /// `None` means "can't tell" — callers leave such assignments alone.
    fn rvalue_option_depth(rvalue: &MirRValue, ctx: &CodegenCtx) -> Option<usize> {
        match rvalue {
            MirRValue::Use(MirOperand::Local(id)) => ctx
                .locals
                .iter()
                .find(|l| l.id == *id)
                .map(|l| Self::option_depth(&l.ty)),
            MirRValue::Use(MirOperand::Constant(_)) => Some(0),
            _ => None,
        }
    }

    /// #493: give an Option destination the layers its source is missing.
    ///
    /// `const inner: T?? = slot` where `slot: T?` has to gain one layer — the
    /// outer says the container held something, the inner carries the slot
    /// (type.optionals/OPT28). Copying the source's bytes straight into the
    /// destination would read the inner layer's tag as the outer one.
    ///
    /// Only fires at depth 2 and beyond; a bare `T` into a `T?` slot is the
    /// ordinary `wrap_as_some` path below and stays there. A bare `none` is
    /// already typed at the annotation's depth by MIR, so it arrives with
    /// matching depth and never widens (OPT29).
    ///
    /// Returns `Some(())` when it handled the assignment.
    fn try_widen_option_depth(
        builder: &mut ClifFunctionBuilder,
        dst: &LocalId,
        dst_ty: &MirType,
        rvalue: &MirRValue,
        val: Value,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Option<()>> {
        let dst_depth = Self::option_depth(dst_ty);
        if dst_depth < 2 {
            return Ok(None);
        }
        // Niche-optimized Option<Handle> has no tag to write (mem.pools).
        if matches!(dst_ty, MirType::Option(inner) if matches!(**inner, MirType::Handle)) {
            return Ok(None);
        }
        let Some(src_depth) = Self::rvalue_option_depth(rvalue, ctx) else {
            return Ok(None);
        };
        if src_depth >= dst_depth {
            return Ok(None);
        }
        let Some((dst_ss, dst_size)) = ctx.stack_slot_map.get(dst).copied() else {
            return Ok(None);
        };

        // Build outwards. The innermost added layer takes the value itself
        // (a scalar when the source carried no layers); every layer above it
        // wraps the aggregate slot built beneath.
        let mut payload_ptr: Option<Value> = (src_depth > 0).then_some(val);
        for depth in (src_depth + 1)..=dst_depth {
            let size = Self::option_type_at_depth(dst_ty, depth).size() as u32;
            let slot = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                size,
                0,
            ));
            match payload_ptr {
                None => Self::build_some(builder, slot, val),
                Some(ptr) => {
                    let inner_size = Self::option_type_at_depth(dst_ty, depth - 1).size() as u32;
                    Self::build_wrapped_aggregate(builder, slot, false, 0, ptr, inner_size);
                }
            }
            payload_ptr = Some(builder.ins().stack_addr(types::I64, slot, 0));
        }

        let src_ptr = payload_ptr.expect("widening builds at least one layer");
        Self::copy_aggregate(builder, src_ptr, dst_ss, dst_size);
        Ok(Some(()))
    }

    /// Wrap an aggregate payload (copied from `src_ptr`) into an Option/Result
    /// slot. `is_result` picks the payload offset and whether origin is zeroed;
    /// `tag` is 0 (Ok/Some) or 1 (Err/none).
    fn build_wrapped_aggregate(
        builder: &mut ClifFunctionBuilder,
        slot: StackSlot,
        is_result: bool,
        tag: i64,
        src_ptr: Value,
        size: u32,
    ) {
        let tag_v = builder.ins().iconst(types::I64, tag);
        builder.ins().stack_store(tag_v, slot, crate::layouts::TAG_OFFSET);
        let payload_off = if is_result {
            Self::zero_result_origin(builder, slot);
            crate::layouts::RESULT_PAYLOAD_OFFSET
        } else {
            crate::layouts::PAYLOAD_OFFSET
        };
        Self::copy_aggregate_at(builder, src_ptr, slot, payload_off, size);
    }

    /// True when `local_ty` is the err side of `ret_ty` (a Result). Drives the
    /// Ok-vs-Err tag choice when a bare component is returned/assigned into a
    /// Result — the type identity, not name capitalization, decides (#259).
    fn is_err_component(ret_ty: &MirType, local_ty: Option<&MirType>) -> bool {
        match ret_ty {
            MirType::Result { err, .. } => local_ty == Some(err.as_ref()),
            _ => false,
        }
    }

    /// C functions that use "negative return = error" convention.
    /// If value < 0: tag=1 (Err), payload=value. Otherwise: tag=0 (Ok), payload=value.
    /// Note: fs_open/fs_create return NULL (0) for errors, not -1 — handled separately.
    /// Negative scalar → `none`, otherwise `some(value)`. The Option twin of
    /// `wrap_result_into_slot`; Option's payload sits at its own offset.
    fn wrap_option_into_slot(builder: &mut ClifFunctionBuilder, value: Value, dst_slot: StackSlot) {
        let zero = builder.ins().iconst(types::I64, 0);
        let is_none = builder.ins().icmp(IntCC::SignedLessThan, value, zero);
        let tag = builder.ins().uextend(types::I64, is_none);
        builder.ins().stack_store(tag, dst_slot, crate::layouts::TAG_OFFSET);
        builder.ins().stack_store(value, dst_slot, crate::layouts::PAYLOAD_OFFSET);
    }

    fn wrap_result_into_slot(builder: &mut ClifFunctionBuilder, value: Value, dst_slot: StackSlot) {
        let zero = builder.ins().iconst(types::I64, 0);
        let is_err = builder.ins().icmp(IntCC::SignedLessThan, value, zero);
        let tag = builder.ins().uextend(types::I64, is_err);
        builder.ins().stack_store(tag, dst_slot, crate::layouts::TAG_OFFSET);
        Self::zero_result_origin(builder, dst_slot);
        builder.ins().stack_store(value, dst_slot, crate::layouts::RESULT_PAYLOAD_OFFSET);
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

    /// Check if MIR arg at `index` is a string type.
    fn is_string_arg(mir_args: &[MirOperand], index: usize, locals: &[rask_mir::MirLocal]) -> bool {
        match mir_args.get(index) {
            Some(MirOperand::Local(id)) => locals
                .iter()
                .find(|l| l.id == *id)
                .map(|l| l.ty == MirType::String)
                .unwrap_or(false),
            Some(MirOperand::Constant(rask_mir::MirConst::String(_))) => true,
            _ => false,
        }
    }

    /// MIR arg already lives behind a pointer — its i64 value is a pointer to
    /// the data, not the data itself. Strings, structs, enums, tuples, options,
    /// results, slices, and trait objects all qualify.
    fn is_by_ptr_arg(mir_args: &[MirOperand], index: usize, locals: &[rask_mir::MirLocal]) -> bool {
        match mir_args.get(index) {
            Some(MirOperand::Local(id)) => locals
                .iter()
                .find(|l| l.id == *id)
                .map(|l| matches!(l.ty,
                    MirType::String
                    | MirType::Struct(_)
                    | MirType::Enum(_)
                    | MirType::Tuple(_)
                    | MirType::Option(_)
                    | MirType::Result { .. }
                    | MirType::Slice(_)
                    | MirType::Union(_)
                    | MirType::TraitObject { .. }
                ))
                .unwrap_or(false),
            Some(MirOperand::Constant(rask_mir::MirConst::String(_))) => true,
            _ => false,
        }
    }

    /// Check if destination local is a string type.
    fn is_string_dst(dst: Option<&LocalId>, ctx: &CodegenCtx) -> bool {
        dst.and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
            .map(|l| l.ty == MirType::String)
            .unwrap_or(false)
    }

    /// Destination that owns storage the callee should write into directly —
    /// anything wider than a word, or with its own layout.
    fn is_aggregate_dst(dst: Option<&LocalId>, ctx: &CodegenCtx) -> bool {
        dst.and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
            .map(|l| matches!(l.ty,
                MirType::String
                | MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::Option(_)
                | MirType::Result { .. }
                | MirType::Union(_)
                | MirType::TraitObject { .. }))
            .unwrap_or(false)
    }

    /// Wrap args[index] as a pointer unless it's already a pointer to data
    /// (string, struct, enum, tuple, option, result, etc.).
    fn wrap_arg_as_ptr(
        builder: &mut ClifFunctionBuilder,
        args: &mut Vec<Value>,
        mir_args: &[MirOperand],
        index: usize,
        locals: &[rask_mir::MirLocal],
    ) {
        if args.len() > index && !Self::is_by_ptr_arg(mir_args, index, locals) {
            let val = args[index];
            args[index] = Self::value_to_ptr(builder, val);
        }
    }

    /// Add out-param for pop/remove-style calls. Returns StringOutParam for string
    /// destinations, PopOutParam otherwise.
    fn append_out_param(
        builder: &mut ClifFunctionBuilder,
        args: &mut Vec<Value>,
        dst: Option<&LocalId>,
        ctx: &CodegenCtx,
    ) -> CallAdapt {
        // Any aggregate destination, not just a string: `Vec.remove` on a
        // `Vec<Pair>` was handed an 8-byte scratch slot, so the struct came back
        // with only its first word and every field after that read as zero.
        // The destination's own slot is the right target — it's already the
        // right size, and writing into it directly skips a copy.
        if Self::is_aggregate_dst(dst, ctx) {
            let fallback_size = dst
                .and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
                .map(|l| l.ty.size().max(16))
                .unwrap_or(16);
            let ss = dst
                .and_then(|id| ctx.stack_slot_map.get(id))
                .map(|(ss, _)| *ss)
                .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, fallback_size, 0,
                )));
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            args.push(addr);
            CallAdapt::StringOutParam(ss)
        } else {
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot, 8, 0,
            ));
            let addr = builder.ins().stack_addr(types::I64, ss, 0);
            args.push(addr);
            CallAdapt::PopOutParam(ss)
        }
    }

    /// Deref result, but copy through dst's slot for aggregate destinations
    /// (string, struct, tuple, ...). The collection holds the data inline; the
    /// runtime returns a pointer into that storage which we must copy into the
    /// caller's slot before the collection moves.
    fn deref_or_string(dst: Option<&LocalId>, ctx: &CodegenCtx) -> CallAdapt {
        let is_aggregate = dst
            .and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
            .map(|l| matches!(l.ty,
                MirType::String
                | MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Tuple(_)
                | MirType::Option(_)
                | MirType::Result { .. }
                | MirType::Slice(_)
                | MirType::Union(_)
                | MirType::TraitObject { .. }
            ))
            .unwrap_or(false);
        if is_aggregate { CallAdapt::DerefStringElement } else { CallAdapt::DerefResult }
    }

    /// Look up struct layout size for a MIR arg, returning (elem_size, is_struct).
    /// (byte size, already-a-pointer) for an aggregate argument.
    ///
    /// Every aggregate is passed as an address, so a caller that spills
    /// "scalars" through `value_to_ptr` must not touch these — doing so stores
    /// the address itself and hands the runtime a pointer to a pointer. Only
    /// structs used to count, so sending an enum over a channel copied the
    /// pointer's bytes as if they were the value (#360).
    fn struct_elem_size(mir_args: &[MirOperand], arg_index: usize, ctx: &CodegenCtx) -> (i64, bool) {
        match mir_args.get(arg_index) {
            Some(MirOperand::Local(arg_id)) => {
                if let Some(local) = ctx.locals.iter().find(|l| l.id == *arg_id) {
                    match &local.ty {
                        MirType::Struct(layout_id) => {
                            if let Some(layout) = ctx.struct_layouts.get(layout_id.id as usize) {
                                return (layout.size as i64, true);
                            }
                        }
                        MirType::Enum(layout_id) => {
                            if let Some(layout) = ctx.enum_layouts.get(layout_id.id as usize) {
                                return (layout.size as i64, true);
                            }
                        }
                        // Same question the lock side asks: does the address
                        // name the value? The two used to spell out different
                        // lists, and the short one made a `Mutex<string>` look
                        // word-sized.
                        ty if ty.passed_by_address() => return (ty.size() as i64, true),
                        _ => {}
                    }
                }
                (8, false)
            }
            // A string constant is already the address of its 16 bytes — same
            // shape as a string local. Reading only the operand kind, it looked
            // like a scalar and `Mutex.new("hello")` copied 8 of them.
            Some(MirOperand::Constant(MirConst::String(_))) => (16, true),
            _ => (8, false),
        }
    }

    /// Adapt stdlib call args for the typed runtime API.
    /// Looks up adaptation from the dispatch table, applies mechanically.
    /// Custom entries fall through to hand-written code.
    fn adapt_stdlib_call(
        builder: &mut ClifFunctionBuilder,
        func_name: &str,
        args: &mut Vec<Value>,
        mir_args: &[MirOperand],
        dst: Option<&LocalId>,
        ctx: &CodegenCtx,
        adapt_table: &HashMap<String, (ArgAdapt, RetAdapt)>,
    ) -> CallAdapt {
        let (arg_adapt, ret_adapt) = adapt_table
            .get(func_name)
            .copied()
            .unwrap_or((ArgAdapt::None, RetAdapt::None));

        // Apply arg adaptation
        let call_adapt = match arg_adapt {
            ArgAdapt::None => CallAdapt::None,

            ArgAdapt::InjectOneSize => {
                if args.is_empty() {
                    args.insert(0, builder.ins().iconst(types::I64, 8));
                }
                CallAdapt::None
            }

            ArgAdapt::InjectTwoSizes => {
                if args.is_empty() {
                    args.insert(0, builder.ins().iconst(types::I64, 8));
                    args.insert(1, builder.ins().iconst(types::I64, 8));
                }
                CallAdapt::None
            }

            ArgAdapt::WrapArg1 => {
                Self::wrap_arg_as_ptr(builder, args, mir_args, 1, ctx.locals);
                CallAdapt::None
            }

            ArgAdapt::WrapArg2 => {
                Self::wrap_arg_as_ptr(builder, args, mir_args, 2, ctx.locals);
                CallAdapt::None
            }

            ArgAdapt::WrapArg1And2 => {
                Self::wrap_arg_as_ptr(builder, args, mir_args, 1, ctx.locals);
                Self::wrap_arg_as_ptr(builder, args, mir_args, 2, ctx.locals);
                CallAdapt::None
            }

            ArgAdapt::StringOutParam => {
                let ss = dst
                    .and_then(|id| ctx.stack_slot_map.get(id))
                    .map(|(ss, _)| *ss)
                    .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    )));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.insert(0, addr);
                CallAdapt::StringOutParam(ss)
            }

            ArgAdapt::StringClone => {
                if let Some(dst_id) = dst {
                    if let Some((dst_ss, _)) = ctx.stack_slot_map.get(dst_id) {
                        if !args.is_empty() {
                            let src_ptr = args[0];
                            Self::copy_aggregate(builder, src_ptr, *dst_ss, 16);
                        }
                        let dst_addr = builder.ins().stack_addr(types::I64, *dst_ss, 0);
                        args[0] = dst_addr;
                        CallAdapt::StringOutParam(*dst_ss)
                    } else {
                        CallAdapt::None
                    }
                } else {
                    CallAdapt::None
                }
            }

            ArgAdapt::InPlaceStringMut => {
                let ss = mir_args.first()
                    .and_then(|op| if let MirOperand::Local(id) = op { Some(id) } else { None })
                    .and_then(|id| ctx.stack_slot_map.get(id))
                    .map(|(ss, _)| *ss)
                    .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    )));
                // C signature: push_str(out, s, other) — prepend out-param address
                let out_addr = builder.ins().stack_addr(types::I64, ss, 0);
                args.insert(0, out_addr);
                CallAdapt::StringOutParam(ss)
            }

            ArgAdapt::AppendOutParam => {
                return Self::append_out_param(builder, args, dst, ctx);
            }

            ArgAdapt::AppendZero => {
                args.push(builder.ins().iconst(types::I64, 0));
                CallAdapt::None
            }

            ArgAdapt::AppendElemSize => {
                args.push(builder.ins().iconst(types::I64, 8));
                CallAdapt::None
            }

            ArgAdapt::AtomicCas => {
                // compare-exchange writes the observed value through an out_ok
                // pointer; the call returns success as its scalar result.
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                args.push(builder.ins().stack_addr(types::I64, ss, 0));
                CallAdapt::PopOutParam(ss)
            }

            ArgAdapt::OptionOutParam => {
                // Hand the callee the destination's payload address so it can
                // copy the element out while it's still live. The old shape —
                // return a pointer and have codegen copy afterwards — meant the
                // pointer named storage the pool had already put on its free
                // list by the time anyone read it.
                let ss = dst
                    .and_then(|id| ctx.stack_slot_map.get(id))
                    .map(|(ss, _)| *ss)
                    .unwrap_or_else(|| builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot, 16, 0,
                    )));
                let addr = builder.ins().stack_addr(types::I64, ss, crate::layouts::PAYLOAD_OFFSET);
                args.push(addr);
                CallAdapt::OptionOutParam(ss)
            }

            ArgAdapt::ParseOutParam => {
                // The value comes back through an out-param so the return value
                // can carry the 0/1 status.
                //
                // Load it with the type the runtime actually wrote — the float
                // entry points store a double, the integer ones an i64 — then
                // convert to what the destination wants. Loading with the
                // destination type instead read a double's bits as an integer,
                // so "42".parse() came out as 4631107791820423168.
                let writer_ty = if matches!(func_name,
                    "string_parse_float" | "string_parse_f32" | "string_parse_f64")
                {
                    types::F64
                } else {
                    types::I64
                };
                let ok_ty = dst
                    .and_then(|id| ctx.locals.iter().find(|l| l.id == *id))
                    .and_then(|l| match &l.ty {
                        MirType::Result { ok, .. } => mir_to_cranelift_type(ok).ok(),
                        other => mir_to_cranelift_type(other).ok(),
                    })
                    .unwrap_or(writer_ty);
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, 8, 0,
                ));
                args.push(builder.ins().stack_addr(types::I64, ss, 0));
                CallAdapt::ParseResult(ss, writer_ty, ok_ty)
            }

            ArgAdapt::Custom => {
                return Self::adapt_stdlib_custom(builder, func_name, args, mir_args, dst, ctx);
            }
        };

        // Apply return adaptation (override if ret_adapt specifies something)
        match ret_adapt {
            RetAdapt::None => call_adapt,
            RetAdapt::DerefOrString => Self::deref_or_string(dst, ctx),
            RetAdapt::DerefOption => CallAdapt::DerefOption,
            RetAdapt::FromArgAdapt => call_adapt,
            // Negative-return=Err wrapping happens in the result-store path,
            // keyed off the entry's RetAdapt::NegErr — arg handling is untouched.
            RetAdapt::NegErr | RetAdapt::NegNone => call_adapt,
        }
    }

    /// Hand-written adaptation for complex cases that need runtime type inspection.
    fn adapt_stdlib_custom(
        builder: &mut ClifFunctionBuilder,
        func_name: &str,
        args: &mut Vec<Value>,
        mir_args: &[MirOperand],
        dst: Option<&LocalId>,
        ctx: &CodegenCtx,
    ) -> CallAdapt {
        match func_name {
            // Vec.contains: the runtime compares elem_size bytes through a
            // pointer. An aggregate argument is already an address; a scalar
            // has to be spilled so there's something to point at.
            "Vec_contains" => {
                if args.len() >= 2 {
                    let is_aggregate = matches!(
                        mir_args.get(1),
                        Some(MirOperand::Local(id)) if ctx.locals.iter()
                            .find(|l| l.id == *id)
                            .map(|l| Self::resolve_type_alloc_size(
                                &l.ty, ctx.struct_layouts, ctx.enum_layouts,
                            ).is_some())
                            .unwrap_or(false)
                    ) || Self::is_string_arg(mir_args, 1, ctx.locals);
                    if !is_aggregate {
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                }
                CallAdapt::None
            }

            // Pool insert: wrap value as pointer, append elem_size
            "Pool_insert" | "Pool_try_insert" => {
                let (elem_size, is_struct) = Self::struct_elem_size(mir_args, 1, ctx);
                if args.len() >= 2 && !is_struct {
                    let val = args[1];
                    args[1] = Self::value_to_ptr(builder, val);
                }
                args.push(builder.ins().iconst(types::I64, elem_size));
                CallAdapt::None
            }

            // Cell_new(value, size) / Cell_set(cell, value): both take the
            // value by pointer, so a scalar has to be spilled to a slot first.
            "Cell_new" => {
                if !args.is_empty() {
                    let (data_size, is_aggregate) = Self::struct_elem_size(mir_args, 0, ctx);
                    if !is_aggregate {
                        let val = args[0];
                        args[0] = Self::value_to_ptr(builder, val);
                    }
                    let size = builder.ins().iconst(types::I64, data_size);
                    if args.len() >= 2 { args[1] = size; } else { args.push(size); }
                }
                CallAdapt::None
            }
            "Cell_set" => {
                if args.len() >= 2 {
                    let (_, is_aggregate) = Self::struct_elem_size(mir_args, 1, ctx);
                    if !is_aggregate {
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                }
                CallAdapt::None
            }

            // Shared_new / Mutex_new: ensure data is pointer, compute actual data_size
            "Shared_new" | "Mutex_new" => {
                if args.len() >= 2 {
                    let (data_size, is_struct) = Self::struct_elem_size(mir_args, 0, ctx);
                    if !is_struct {
                        let val = args[0];
                        args[0] = Self::value_to_ptr(builder, val);
                    }
                    args[1] = builder.ins().iconst(types::I64, data_size);
                }
                CallAdapt::None
            }

            // Sender_send: wrap value as pointer (structs already are)
            "Sender_send" | "send" => {
                if args.len() >= 2 {
                    let (_, is_struct) = Self::struct_elem_size(mir_args, 1, ctx);
                    if !is_struct {
                        let val = args[1];
                        args[1] = Self::value_to_ptr(builder, val);
                    }
                }
                CallAdapt::None
            }

            // Receiver_receive_struct: replace elem_size arg with stack buffer address
            "Receiver_receive_struct" => {
                let elem_size = match mir_args.get(1) {
                    Some(MirOperand::Constant(MirConst::Int(size))) => *size as u32,
                    _ => 8,
                };
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, elem_size, 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                if args.len() >= 2 { args[1] = addr; } else { args.push(addr); }
                CallAdapt::RecvStructOk(elem_size)
            }

            // Receiver_try_receive: recv into a buffer of the element's real size;
            // the C call returns the channel status. Args in: [rx, elem_size].
            // Args out: [rx, out_ptr]. Post-call builds the `T or E` Result.
            "Receiver_try_receive" => {
                let elem_size = match mir_args.get(1) {
                    Some(MirOperand::Constant(MirConst::Int(size))) => *size as u32,
                    _ => 8,
                };
                let ss = builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot, elem_size.max(8), 0,
                ));
                let addr = builder.ins().stack_addr(types::I64, ss, 0);
                if args.len() >= 2 { args[1] = addr; } else { args.push(addr); }
                CallAdapt::TryRecvResult(ss, elem_size)
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

    /// Lower a MIR operand to a raw C string pointer (for runtime functions
    /// that take `const char*`). For MirConst::String, returns the data section
    /// pointer directly instead of constructing a Rask string object.
    fn lower_operand_as_cstr(
        builder: &mut ClifFunctionBuilder,
        op: &MirOperand,
        ctx: &CodegenCtx,
    ) -> CodegenResult<Value> {
        if let MirOperand::Constant(MirConst::String(s)) = op {
            if let Some(gv) = ctx.string_globals.get(s.as_str()) {
                return Ok(builder.ins().global_value(types::I64, *gv));
            }
        }
        // Fallback: treat as i64 (pointer)
        Self::lower_operand_typed(builder, op, Some(types::I64), ctx)
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
                        // Cranelift's integer types carry no signedness, so
                        // `convert_value` widens by sign-extending. An unsigned
                        // value has to zero-extend or its top bit reads as a
                        // sign — `u8` 200 arrived as -56 (#326).
                        let is_unsigned = ctx.locals.iter()
                            .find(|l| l.id == *local_id)
                            .map_or(false, |l| matches!(l.ty,
                                MirType::U8 | MirType::U16 | MirType::U32 | MirType::U64));
                        if is_unsigned && exp_ty.bits() > actual_ty.bits() {
                            return Ok(builder.ins().uextend(exp_ty, val));
                        }
                        return Ok(Self::convert_value(builder, val, actual_ty, exp_ty, None));
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
                        let ty = if matches!(expected_ty, Some(t) if t.is_int() && t != types::I8) {
                            expected_ty.unwrap()
                        } else {
                            types::I8
                        };
                        Ok(builder.ins().iconst(ty, if *b { 1 } else { 0 }))
                    }
                    MirConst::Char(c) => {
                        Ok(builder.ins().iconst(types::I32, *c as i64))
                    }
                    MirConst::String(s) => {
                        // String constants: allocate a 16-byte stack slot,
                        // get raw char* from data section, call rask_string_from(out, cstr).
                        if let Some(gv) = ctx.string_globals.get(s.as_str()) {
                            let raw_ptr = builder.ins().global_value(types::I64, *gv);
                            let tmp_slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot, 16, 0,
                            ));
                            let out_ptr = builder.ins().stack_addr(types::I64, tmp_slot, 0);
                            if let Some(string_from_ref) = ctx.func_refs.get("string_from") {
                                builder.ins().call(*string_from_ref, &[out_ptr, raw_ptr]);
                                Ok(out_ptr)
                            } else {
                                return Err(CodegenError::FunctionNotFound("string_from".to_string()))
                            }
                        } else {
                            // Empty string: SSO with remaining=15
                            let tmp_slot = builder.create_sized_stack_slot(StackSlotData::new(
                                StackSlotKind::ExplicitSlot, 16, 0,
                            ));
                            let lo = builder.ins().iconst(types::I64, crate::layouts::EMPTY_STRING_LO);
                            builder.ins().stack_store(lo, tmp_slot, 0);
                            let hi = builder.ins().iconst(types::I64, crate::layouts::EMPTY_STRING_HI);
                            builder.ins().stack_store(hi, tmp_slot, 8);
                            let out_ptr = builder.ins().stack_addr(types::I64, tmp_slot, 0);
                            Ok(out_ptr)
                        }
                    }
                }
            }
        }
    }
}
