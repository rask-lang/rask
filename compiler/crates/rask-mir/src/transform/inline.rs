// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function inlining — cross-function MIR transform (Phase F).
//!
//! Replaces Call statements with the callee's body, renumbering all
//! BlockIds and LocalIds to avoid conflicts. Source spans are preserved
//! on inlined code (IN4).
//!
//! Heuristics (from comp.architecture):
//! - IN2: Leaf functions ≤ MAX_INLINE_STMTS → inline
//! - IN3: Functions called once → always inline (no code size cost)
//! - Recursive functions → never inline
//! - Extern functions → never inline

use std::collections::HashMap;

use crate::analysis::call_graph::CallGraph;
use crate::{
    BlockId, FunctionRef, LocalId, MirBlock, MirFunction, MirLocal, MirOperand,
    MirRValue, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, MirType, Span,
};

/// DI5: metadata for a region of inlined code within a function.
/// Returned as a side-channel from the inlining pass — not stored on MirFunction.
#[derive(Debug, Clone)]
pub struct InlineRegion {
    /// Name of the callee function that was inlined.
    pub callee_name: String,
    /// Span of the call site in the caller (for DW_AT_call_line).
    pub call_site: Span,
    /// Source byte offset range of the callee's body.
    /// Used to identify which native srclocs belong to this inline region.
    pub callee_body_span: Span,
}

/// Maximum MIR statement count for size-based inlining (IN2).
const MAX_INLINE_STMTS: usize = 20;

/// Maximum inlining depth to prevent unbounded growth from chains of
/// always-inline (called-once) functions.
const MAX_INLINE_DEPTH: usize = 8;

/// Run the inlining pass over all functions.
///
/// Returns DI5 inline region metadata as a side-channel (caller name → regions).
/// This keeps debug concerns out of MirFunction.
pub fn inline_functions(fns: &mut Vec<MirFunction>) -> HashMap<String, Vec<InlineRegion>> {
    let mut inline_metadata: HashMap<String, Vec<InlineRegion>> = HashMap::new();

    // Build call graph for heuristics
    let cg = CallGraph::build(fns);

    // Snapshot callee bodies (we read callees while mutating callers).
    // Only snapshot functions that are candidates for inlining.
    let callee_bodies: HashMap<String, MirFunction> = fns
        .iter()
        .filter(|f| should_inline(&cg, &f.name))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();

    if callee_bodies.is_empty() {
        return inline_metadata;
    }

    // Inline into each function
    for func in fns.iter_mut() {
        inline_into_function(func, &callee_bodies, &cg, 0, &mut inline_metadata);
    }

    inline_metadata
}

/// Determine if a function is a candidate for inlining at any call site.
fn should_inline(cg: &CallGraph, callee_name: &str) -> bool {
    if cg.is_recursive(callee_name) {
        return false;
    }

    let stmt_count = cg.statement_count(callee_name);

    // IN3: called once → always inline
    if cg.called_once(callee_name) {
        return true;
    }

    // IN2: small functions → inline
    stmt_count <= MAX_INLINE_STMTS
}

/// Inline eligible calls within a single function.
///
/// Processes blocks iteratively. When a Call is inlined, the current block
/// is split: pre-call statements stay, callee blocks are appended, and a
/// merge block receives post-call statements.
fn inline_into_function(
    func: &mut MirFunction,
    callee_bodies: &HashMap<String, MirFunction>,
    cg: &CallGraph,
    depth: usize,
    inline_metadata: &mut HashMap<String, Vec<InlineRegion>>,
) {
    if depth >= MAX_INLINE_DEPTH {
        return;
    }

    // Process block by block. We may add new blocks during iteration,
    // but only process the original ones for inlining candidates.
    let mut block_idx = 0;
    while block_idx < func.blocks.len() {
        let mut stmt_idx = 0;
        while stmt_idx < func.blocks[block_idx].statements.len() {
            let should = {
                let stmt = &func.blocks[block_idx].statements[stmt_idx];
                match &stmt.kind {
                    MirStmtKind::Call { func: callee_ref, .. } => {
                        !callee_ref.is_extern && callee_bodies.contains_key(&callee_ref.name)
                    }
                    _ => false,
                }
            };

            if should {
                let did_inline = try_inline_call(func, block_idx, stmt_idx, callee_bodies, cg, depth, inline_metadata);
                if did_inline {
                    // After inlining, the block was split. The current block_idx
                    // now ends with a Goto into the inlined code. Don't advance
                    // stmt_idx — re-examine from the same position in case the
                    // merge block has further inline candidates (handled when we
                    // reach that block_idx later).
                    break;
                }
            }
            stmt_idx += 1;
        }
        block_idx += 1;
    }
}

/// Try to inline a specific call. Returns true if inlining happened.
fn try_inline_call(
    caller: &mut MirFunction,
    block_idx: usize,
    stmt_idx: usize,
    callee_bodies: &HashMap<String, MirFunction>,
    _cg: &CallGraph,
    _depth: usize,
    inline_metadata: &mut HashMap<String, Vec<InlineRegion>>,
) -> bool {
    // Extract call info
    let (dst, callee_name, args, call_span) = {
        let stmt = &caller.blocks[block_idx].statements[stmt_idx];
        match &stmt.kind {
            MirStmtKind::Call { dst, func, args } => {
                (dst.clone(), func.name.clone(), args.clone(), stmt.span)
            }
            _ => return false,
        }
    };

    let callee = match callee_bodies.get(&callee_name) {
        Some(c) => c,
        None => return false,
    };

    // Don't inline into self
    if caller.name == callee_name {
        return false;
    }

    // Don't inline empty functions (no blocks)
    if callee.blocks.is_empty() {
        return false;
    }

    // --- Renumbering ---

    // Find the next available LocalId and BlockId in the caller
    let mut next_local = next_local_id(caller);
    let mut next_block = next_block_id(caller);

    // Map callee LocalIds → new caller LocalIds
    let mut local_map: HashMap<LocalId, LocalId> = HashMap::new();

    // Map callee params to argument operands (we'll assign args to new locals)
    for (i, param) in callee.params.iter().enumerate() {
        let new_id = LocalId(next_local);
        next_local += 1;
        local_map.insert(param.id, new_id);
        caller.locals.push(MirLocal {
            id: new_id,
            name: param.name.clone(),
            ty: param.ty.clone(),
            is_param: false,
        });
    }

    // Map callee locals to new caller locals
    for local in &callee.locals {
        if local_map.contains_key(&local.id) {
            continue; // Already mapped as param
        }
        let new_id = LocalId(next_local);
        next_local += 1;
        local_map.insert(local.id, new_id);
        caller.locals.push(MirLocal {
            id: new_id,
            name: local.name.clone(),
            ty: local.ty.clone(),
            is_param: false,
        });
    }

    // Map callee BlockIds → new caller BlockIds
    let mut block_map: HashMap<BlockId, BlockId> = HashMap::new();
    for block in &callee.blocks {
        let new_id = BlockId(next_block);
        next_block += 1;
        block_map.insert(block.id, new_id);
    }

    // Merge block: where control flows after the inlined body returns
    let merge_block_id = BlockId(next_block);

    // Return value local (if callee returns something)
    let ret_local = dst.map(|d| d);

    // --- Split the caller block ---

    // Statements after the call go to the merge block
    let post_stmts: Vec<MirStmt> = caller.blocks[block_idx]
        .statements
        .split_off(stmt_idx + 1);

    // Remove the call statement itself
    caller.blocks[block_idx].statements.pop();

    // Save the original terminator for the merge block
    let original_terminator = caller.blocks[block_idx].terminator.clone();

    // The current block now jumps to the callee's entry
    let callee_entry = block_map[&callee.entry_block];
    caller.blocks[block_idx].terminator = MirTerminator::new(
        MirTerminatorKind::Goto { target: callee_entry },
        call_span,
    );

    // --- Copy callee blocks with renumbering ---

    // First, assign args to param locals
    let mut entry_prefix: Vec<MirStmt> = Vec::new();
    for (i, param) in callee.params.iter().enumerate() {
        if i < args.len() {
            let new_param_local = local_map[&param.id];
            entry_prefix.push(MirStmt::new(
                MirStmtKind::Assign {
                    dst: new_param_local,
                    rvalue: MirRValue::Use(args[i].clone()),
                },
                call_span,
            ));
        }
    }

    for callee_block in &callee.blocks {
        let new_block_id = block_map[&callee_block.id];

        // Remap statements
        let mut new_stmts: Vec<MirStmt> = Vec::new();

        // Prepend arg assignments to entry block
        if callee_block.id == callee.entry_block {
            new_stmts.extend(entry_prefix.drain(..));
        }

        for stmt in &callee_block.statements {
            new_stmts.push(remap_stmt(stmt, &local_map, &block_map));
        }

        // Remap terminator — Return becomes Goto merge_block + optional assign
        let new_terminator = remap_terminator(
            &callee_block.terminator,
            &local_map,
            &block_map,
            merge_block_id,
            ret_local,
        );

        caller.blocks.push(MirBlock {
            id: new_block_id,
            statements: new_stmts,
            terminator: new_terminator,
        });
    }

    // --- Create merge block ---
    caller.blocks.push(MirBlock {
        id: merge_block_id,
        statements: post_stmts,
        terminator: original_terminator,
    });

    // --- Fixup return values ---
    // For callee blocks that ended with Return { value: Some(op) },
    // insert an Assign to the caller's destination local before the Goto.
    if let Some(ret_dst) = ret_local {
        fixup_return_values(caller, callee, &block_map, &local_map, ret_dst);
    }

    // DI5: record inline region so DWARF can emit DW_TAG_inlined_subroutine
    let callee_body_span = compute_body_span(callee);
    if callee_body_span.end > 0 {
        inline_metadata
            .entry(caller.name.clone())
            .or_default()
            .push(InlineRegion {
                callee_name: callee_name.clone(),
                call_site: call_span,
                callee_body_span,
            });
    }

    true
}

/// Compute the min..max source byte offset range across all statements
/// and terminators in a function. Returns Span(0, 0) if no real spans found.
fn compute_body_span(func: &MirFunction) -> Span {
    let mut min_start = u32::MAX;
    let mut max_end = 0u32;
    for block in &func.blocks {
        for stmt in &block.statements {
            let s = stmt.span;
            if s.end > 0 {
                min_start = min_start.min(s.start as u32);
                max_end = max_end.max(s.end as u32);
            }
        }
        let t = block.terminator.span;
        if t.end > 0 {
            min_start = min_start.min(t.start as u32);
            max_end = max_end.max(t.end as u32);
        }
    }
    if max_end == 0 {
        Span::new(0, 0)
    } else {
        Span::new(min_start as usize, max_end as usize)
    }
}

// --- Renumbering helpers ---

fn next_local_id(func: &MirFunction) -> u32 {
    let from_locals = func.locals.iter().map(|l| l.id.0 + 1).max().unwrap_or(0);
    let from_params = func.params.iter().map(|p| p.id.0 + 1).max().unwrap_or(0);
    from_locals.max(from_params)
}

fn next_block_id(func: &MirFunction) -> u32 {
    func.blocks.iter().map(|b| b.id.0 + 1).max().unwrap_or(0)
}

fn remap_operand(op: &MirOperand, local_map: &HashMap<LocalId, LocalId>) -> MirOperand {
    match op {
        MirOperand::Local(id) => {
            MirOperand::Local(local_map.get(id).copied().unwrap_or(*id))
        }
        MirOperand::Constant(c) => MirOperand::Constant(c.clone()),
    }
}

fn remap_rvalue(rv: &MirRValue, local_map: &HashMap<LocalId, LocalId>) -> MirRValue {
    match rv {
        MirRValue::Use(op) => MirRValue::Use(remap_operand(op, local_map)),
        MirRValue::Ref(id) => MirRValue::Ref(local_map.get(id).copied().unwrap_or(*id)),
        MirRValue::Deref(op) => MirRValue::Deref(remap_operand(op, local_map)),
        MirRValue::BinaryOp { op, left, right } => MirRValue::BinaryOp {
            op: *op,
            left: remap_operand(left, local_map),
            right: remap_operand(right, local_map),
        },
        MirRValue::UnaryOp { op, operand } => MirRValue::UnaryOp {
            op: *op,
            operand: remap_operand(operand, local_map),
        },
        MirRValue::Cast { value, target_ty } => MirRValue::Cast {
            value: remap_operand(value, local_map),
            target_ty: target_ty.clone(),
        },
        MirRValue::Field {
            base,
            field_index,
            byte_offset,
            field_size,
        } => MirRValue::Field {
            base: remap_operand(base, local_map),
            field_index: *field_index,
            byte_offset: *byte_offset,
            field_size: *field_size,
        },
        MirRValue::EnumTag { value } => MirRValue::EnumTag {
            value: remap_operand(value, local_map),
        },
        MirRValue::ArrayIndex {
            base,
            index,
            elem_size,
        } => MirRValue::ArrayIndex {
            base: remap_operand(base, local_map),
            index: remap_operand(index, local_map),
            elem_size: *elem_size,
        },
    }
}

fn remap_stmt(
    stmt: &MirStmt,
    local_map: &HashMap<LocalId, LocalId>,
    block_map: &HashMap<BlockId, BlockId>,
) -> MirStmt {
    let kind = match &stmt.kind {
        MirStmtKind::Assign { dst, rvalue } => MirStmtKind::Assign {
            dst: local_map.get(dst).copied().unwrap_or(*dst),
            rvalue: remap_rvalue(rvalue, local_map),
        },
        MirStmtKind::Store {
            addr,
            offset,
            value,
            store_size,
        } => MirStmtKind::Store {
            addr: local_map.get(addr).copied().unwrap_or(*addr),
            offset: *offset,
            value: remap_operand(value, local_map),
            store_size: *store_size,
        },
        MirStmtKind::Call { dst, func, args } => MirStmtKind::Call {
            dst: dst.map(|d| local_map.get(&d).copied().unwrap_or(d)),
            func: func.clone(),
            args: args.iter().map(|a| remap_operand(a, local_map)).collect(),
        },
        MirStmtKind::ResourceRegister {
            dst,
            type_name,
            scope_depth,
        } => MirStmtKind::ResourceRegister {
            dst: local_map.get(dst).copied().unwrap_or(*dst),
            type_name: type_name.clone(),
            scope_depth: *scope_depth,
        },
        MirStmtKind::ResourceConsume { resource_id } => MirStmtKind::ResourceConsume {
            resource_id: local_map.get(resource_id).copied().unwrap_or(*resource_id),
        },
        MirStmtKind::ResourceScopeCheck { scope_depth } => MirStmtKind::ResourceScopeCheck {
            scope_depth: *scope_depth,
        },
        MirStmtKind::EnsurePush { cleanup_block } => MirStmtKind::EnsurePush {
            cleanup_block: block_map.get(cleanup_block).copied().unwrap_or(*cleanup_block),
        },
        MirStmtKind::EnsurePop => MirStmtKind::EnsurePop,
        MirStmtKind::PoolCheckedAccess { dst, pool, handle } => MirStmtKind::PoolCheckedAccess {
            dst: local_map.get(dst).copied().unwrap_or(*dst),
            pool: local_map.get(pool).copied().unwrap_or(*pool),
            handle: local_map.get(handle).copied().unwrap_or(*handle),
        },
        MirStmtKind::ClosureCreate {
            dst,
            func_name,
            captures,
            heap,
        } => MirStmtKind::ClosureCreate {
            dst: local_map.get(dst).copied().unwrap_or(*dst),
            func_name: func_name.clone(),
            captures: captures
                .iter()
                .map(|c| crate::ClosureCapture {
                    local_id: local_map.get(&c.local_id).copied().unwrap_or(c.local_id),
                    offset: c.offset,
                    size: c.size,
                })
                .collect(),
            heap: *heap,
        },
        MirStmtKind::ClosureCall { dst, closure, args } => MirStmtKind::ClosureCall {
            dst: dst.map(|d| local_map.get(&d).copied().unwrap_or(d)),
            closure: local_map.get(closure).copied().unwrap_or(*closure),
            args: args.iter().map(|a| remap_operand(a, local_map)).collect(),
        },
        MirStmtKind::LoadCapture {
            dst,
            env_ptr,
            offset,
        } => MirStmtKind::LoadCapture {
            dst: local_map.get(dst).copied().unwrap_or(*dst),
            env_ptr: local_map.get(env_ptr).copied().unwrap_or(*env_ptr),
            offset: *offset,
        },
        MirStmtKind::ClosureDrop { closure } => MirStmtKind::ClosureDrop {
            closure: local_map.get(closure).copied().unwrap_or(*closure),
        },
        MirStmtKind::ArrayStore {
            base,
            index,
            elem_size,
            value,
        } => MirStmtKind::ArrayStore {
            base: local_map.get(base).copied().unwrap_or(*base),
            index: remap_operand(index, local_map),
            elem_size: *elem_size,
            value: remap_operand(value, local_map),
        },
        MirStmtKind::GlobalRef { dst, name } => MirStmtKind::GlobalRef {
            dst: local_map.get(dst).copied().unwrap_or(*dst),
            name: name.clone(),
        },
        MirStmtKind::TraitBox {
            dst,
            value,
            concrete_type,
            trait_name,
            concrete_size,
            vtable_name,
        } => MirStmtKind::TraitBox {
            dst: local_map.get(dst).copied().unwrap_or(*dst),
            value: remap_operand(value, local_map),
            concrete_type: concrete_type.clone(),
            trait_name: trait_name.clone(),
            concrete_size: *concrete_size,
            vtable_name: vtable_name.clone(),
        },
        MirStmtKind::TraitCall {
            dst,
            trait_object,
            method_name,
            vtable_offset,
            args,
        } => MirStmtKind::TraitCall {
            dst: dst.map(|d| local_map.get(&d).copied().unwrap_or(d)),
            trait_object: local_map.get(trait_object).copied().unwrap_or(*trait_object),
            method_name: method_name.clone(),
            vtable_offset: *vtable_offset,
            args: args.iter().map(|a| remap_operand(a, local_map)).collect(),
        },
        MirStmtKind::TraitDrop { trait_object } => MirStmtKind::TraitDrop {
            trait_object: local_map.get(trait_object).copied().unwrap_or(*trait_object),
        },
        MirStmtKind::Phi { dst, args } => MirStmtKind::Phi {
            dst: local_map.get(dst).copied().unwrap_or(*dst),
            args: args
                .iter()
                .map(|(bid, op)| {
                    (
                        block_map.get(bid).copied().unwrap_or(*bid),
                        remap_operand(op, local_map),
                    )
                })
                .collect(),
        },
        MirStmtKind::RcInc { local } => MirStmtKind::RcInc {
            local: local_map.get(local).copied().unwrap_or(*local),
        },
        MirStmtKind::RcDec { local } => MirStmtKind::RcDec {
            local: local_map.get(local).copied().unwrap_or(*local),
        },
    };

    // IN4: preserve original spans from callee
    MirStmt::new(kind, stmt.span)
}

fn remap_terminator(
    term: &MirTerminator,
    local_map: &HashMap<LocalId, LocalId>,
    block_map: &HashMap<BlockId, BlockId>,
    merge_block: BlockId,
    ret_local: Option<LocalId>,
) -> MirTerminator {
    let kind = match &term.kind {
        MirTerminatorKind::Return { value } => {
            // Callee return → assign return value + goto merge block
            // The assignment is handled by prepending to the merge block
            // or by emitting it as the last statement before the goto.
            // For simplicity, we create a block that assigns and gotos.
            // Actually, we handle this inline: just goto merge.
            // The return value assignment is handled below.
            if let (Some(dst), Some(val)) = (ret_local, value) {
                // We can't emit a statement in the terminator, so we need
                // the caller to handle this. For now, we rely on the fact
                // that the callee should have assigned to its return local
                // already, and we map that local. But Return { value } means
                // the operand IS the return value.
                //
                // We need to emit an Assign before the Goto. This is a
                // slight hack — we'll handle it by returning a special
                // terminator and post-processing.
                //
                // Actually, the cleanest approach: convert Return to a
                // block with an Assign + Goto. But we're constructing the
                // block here... Let me handle it at block construction time.
                //
                // For now: we'll place the assignment as a statement in the
                // block and use Goto as the terminator.
                return MirTerminator::new(MirTerminatorKind::Goto { target: merge_block }, term.span);
            }
            MirTerminatorKind::Goto { target: merge_block }
        }
        MirTerminatorKind::Goto { target } => MirTerminatorKind::Goto {
            target: block_map.get(target).copied().unwrap_or(*target),
        },
        MirTerminatorKind::Branch {
            cond,
            then_block,
            else_block,
        } => MirTerminatorKind::Branch {
            cond: remap_operand(cond, local_map),
            then_block: block_map.get(then_block).copied().unwrap_or(*then_block),
            else_block: block_map.get(else_block).copied().unwrap_or(*else_block),
        },
        MirTerminatorKind::Switch {
            value,
            cases,
            default,
        } => MirTerminatorKind::Switch {
            value: remap_operand(value, local_map),
            cases: cases
                .iter()
                .map(|(v, bid)| (*v, block_map.get(bid).copied().unwrap_or(*bid)))
                .collect(),
            default: block_map.get(default).copied().unwrap_or(*default),
        },
        MirTerminatorKind::Unreachable => MirTerminatorKind::Unreachable,
        MirTerminatorKind::CleanupReturn {
            value,
            cleanup_chain,
        } => MirTerminatorKind::CleanupReturn {
            value: value.as_ref().map(|v| remap_operand(v, local_map)),
            cleanup_chain: cleanup_chain
                .iter()
                .map(|bid| block_map.get(bid).copied().unwrap_or(*bid))
                .collect(),
        },
    };

    MirTerminator::new(kind, term.span)
}

/// Post-process inlined blocks: for Return terminators that had a value,
/// insert an Assign statement at the end of the block (before the Goto).
///
/// This is called during try_inline_call after block construction.
/// Actually, we handle this differently — see the revised approach below.

// --- Revised approach for return values ---
//
// When the callee has `Return { value: Some(op) }`, we need to assign
// that operand to the caller's destination local. We do this by appending
// an Assign statement to the block just before the Goto terminator.
//
// This is handled inside try_inline_call by post-processing the newly
// created blocks.

/// Fixup: for each inlined block whose original callee terminator was
/// Return { value: Some(op) }, insert an Assign to the caller's dst local.
fn fixup_return_values(
    caller: &mut MirFunction,
    callee: &MirFunction,
    block_map: &HashMap<BlockId, BlockId>,
    local_map: &HashMap<LocalId, LocalId>,
    ret_dst: LocalId,
) {
    for callee_block in &callee.blocks {
        if let MirTerminatorKind::Return { value: Some(ref op) } = callee_block.terminator.kind {
            let new_block_id = block_map[&callee_block.id];
            // Find this block in the caller
            if let Some(block) = caller.blocks.iter_mut().find(|b| b.id == new_block_id) {
                let remapped_op = remap_operand(op, local_map);
                block.statements.push(MirStmt::new(
                    MirStmtKind::Assign {
                        dst: ret_dst,
                        rvalue: MirRValue::Use(remapped_op),
                    },
                    callee_block.terminator.span,
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MirConst, BinOp};

    fn make_local(id: u32, name: &str, ty: MirType, is_param: bool) -> MirLocal {
        MirLocal {
            id: LocalId(id),
            name: Some(name.to_string()),
            ty,
            is_param,
        }
    }

    fn make_simple_callee() -> MirFunction {
        // func double(x: i32) -> i32 { return x * 2 }
        MirFunction {
            name: "double".to_string(),
            params: vec![make_local(0, "x", MirType::I32, true)],
            ret_ty: MirType::I32,
            locals: vec![
                make_local(0, "x", MirType::I32, true),
                make_local(1, "_ret", MirType::I32, false),
            ],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                    dst: LocalId(1),
                    rvalue: MirRValue::BinaryOp {
                        op: BinOp::Mul,
                        left: MirOperand::Local(LocalId(0)),
                        right: MirOperand::Constant(MirConst::Int(2)),
                    },
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(LocalId(1))),
                }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,

        }
    }

    fn make_caller() -> MirFunction {
        // func main() -> i32 { const y = double(5); return y }
        MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals: vec![make_local(0, "y", MirType::I32, false)],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(LocalId(0)),
                        func: FunctionRef::internal("double".to_string()),
                        args: vec![MirOperand::Constant(MirConst::Int(5))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(LocalId(0))),
                }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,

        }
    }

    #[test]
    fn basic_inline() {
        let mut fns = vec![make_caller(), make_simple_callee()];
        let _ = inline_functions(&mut fns);

        let main = &fns[0];
        // Original block should now end with Goto (not the call)
        assert!(matches!(
            main.blocks[0].terminator.kind,
            MirTerminatorKind::Goto { .. }
        ));
        // Should have more blocks than before (inlined body + merge)
        assert!(main.blocks.len() >= 3, "expected ≥3 blocks, got {}", main.blocks.len());
        // Should have more locals (callee's locals remapped)
        assert!(main.locals.len() > 1, "expected >1 locals, got {}", main.locals.len());
    }

    #[test]
    fn no_inline_recursive() {
        let mut fns = vec![
            MirFunction {
                name: "factorial".to_string(),
                params: vec![make_local(0, "n", MirType::I32, true)],
                ret_ty: MirType::I32,
                locals: vec![
                    make_local(0, "n", MirType::I32, true),
                    make_local(1, "_ret", MirType::I32, false),
                ],
                blocks: vec![MirBlock {
                    id: BlockId(0),
                    statements: vec![MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(LocalId(1)),
                        func: FunctionRef::internal("factorial".to_string()),
                        args: vec![MirOperand::Local(LocalId(0))],
                    })],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Local(LocalId(1))),
                    }),
                }],
                entry_block: BlockId(0),
                is_extern_c: false,
                source_file: None,
    
            },
        ];
        let blocks_before = fns[0].blocks.len();
        let _ = inline_functions(&mut fns);
        // Should NOT have inlined — block count unchanged
        assert_eq!(fns[0].blocks.len(), blocks_before);
    }

    #[test]
    fn no_inline_extern() {
        let mut fns = vec![MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::extern_c("puts".to_string()),
                    args: vec![],
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,

        }];
        let blocks_before = fns[0].blocks.len();
        let _ = inline_functions(&mut fns);
        assert_eq!(fns[0].blocks.len(), blocks_before);
    }

    #[test]
    fn should_inline_called_once_even_if_large() {
        let cg_call_count = {
            let mut m = HashMap::new();
            m.insert("big_helper".to_string(), 1u32);
            m
        };
        // A function called once with 50 statements should still be inlined (IN3)
        let cg = CallGraph {
            callees: HashMap::new(),
            call_count: cg_call_count,
            stmt_count: {
                let mut m = HashMap::new();
                m.insert("big_helper".to_string(), 50);
                m
            },
            recursive: std::collections::HashSet::new(),
        };
        assert!(should_inline(&cg, "big_helper"));
    }

    #[test]
    fn should_not_inline_large_multi_call() {
        let cg = CallGraph {
            callees: HashMap::new(),
            call_count: {
                let mut m = HashMap::new();
                m.insert("big_fn".to_string(), 3u32);
                m
            },
            stmt_count: {
                let mut m = HashMap::new();
                m.insert("big_fn".to_string(), 50);
                m
            },
            recursive: std::collections::HashSet::new(),
        };
        assert!(!should_inline(&cg, "big_fn"));
    }
}
