// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! State machine transform for spawn closures.
//!
//! Converts spawn closure functions containing yield points into poll-based
//! state machines. A yield point is a call to a function that suspends the
//! green task (sleep, channel ops, async I/O).
//!
//! Without yield points, the closure runs to completion via the closure bridge
//! (`rask_green_closure_spawn`). With yield points, we generate a poll function
//! that can be suspended and resumed by the scheduler.
//!
//! The transform operates on an already-lowered MIR function (the synthesized
//! spawn closure). It produces a new MIR function with poll semantics plus a
//! state struct layout.

use std::collections::{HashMap, HashSet};

use crate::{
    BlockBuilder, BlockId, FunctionRef, LocalId, MirBlock, MirFunction, MirLocal, MirOperand,
    MirRValue, MirStmt, MirTerminator, MirType,
};
use crate::operand::MirConst;

// ── Yield point registry ────────────────────────────────────────────

/// Known functions that act as yield points in green tasks.
/// When the state machine hits one, it saves state and returns PENDING.
const YIELD_POINT_FUNCTIONS: &[&str] = &[
    // Core yield primitives
    "rask_sleep_ns",
    "rask_green_sleep_ns",
    "rask_yield_timeout",
    "rask_yield_read",
    "rask_yield_write",
    "rask_yield_accept",
    "rask_yield",
    // Dual-path async I/O
    "rask_async_read",
    "rask_async_write",
    "rask_async_accept",
    // Async channel ops
    "rask_channel_send_async",
    "rask_channel_recv_async",
];

/// Check if a function name is a yield point.
pub fn is_yield_point(name: &str) -> bool {
    YIELD_POINT_FUNCTIONS.contains(&name)
}

// ── Public API ──────────────────────────────────────────────────────

/// Result of the state machine transform.
pub struct StateMachineResult {
    /// The poll function: `fn poll(state_ptr: Ptr, task_ctx: Ptr) -> I32`
    pub poll_fn: MirFunction,
    /// Layout of the state struct (field offsets and types).
    /// The first field is always `state_tag: I32`.
    pub state_fields: Vec<StateField>,
    /// Total size of the state struct in bytes.
    pub state_size: u32,
}

/// A field in the generated state struct.
#[derive(Debug, Clone)]
pub struct StateField {
    /// Which local variable this saves (None for state_tag)
    pub local_id: Option<LocalId>,
    pub name: String,
    pub ty: MirType,
    pub offset: u32,
    pub size: u32,
}

/// Check if a spawn function contains yield points. If not, the closure
/// bridge can handle it without transformation.
pub fn has_yield_points(func: &MirFunction) -> bool {
    func.blocks.iter().any(|block| {
        block.statements.iter().any(|stmt| {
            if let MirStmt::Call { func: fref, .. } = stmt {
                is_yield_point(&fref.name)
            } else {
                false
            }
        })
    })
}

/// Transform a spawn closure function into a poll-based state machine.
///
/// The input function has signature `fn name(env_ptr: Ptr) -> Void`.
/// The output poll function has signature `fn name_poll(state_ptr: Ptr, task_ctx: Ptr) -> I32`.
///
/// Returns None if no yield points are found (use closure bridge instead).
pub fn transform(func: &MirFunction) -> Option<StateMachineResult> {
    if !has_yield_points(func) {
        return None;
    }

    // Flatten all statements from all blocks into a linear sequence.
    // Spawn closures are straight-line code (no branches from user code),
    // so we can safely linearize. If we encounter branches, we fall back
    // to the simple approach of treating the whole body as segment 0.
    let linear = linearize(func);

    // Find yield point indices in the linear sequence
    let yield_indices = find_yield_indices(&linear);
    if yield_indices.is_empty() {
        return None;
    }

    // Segment the linear body at yield points
    let segments = segment(&linear, &yield_indices);

    // Compute liveness: which locals are defined before a yield and used after
    let live_across = compute_liveness(&segments, func);

    // Build state struct layout
    let (state_fields, state_size) = build_state_layout(&live_across, func);

    // Generate the poll function
    let poll_fn = generate_poll_fn(func, &segments, &yield_indices, &state_fields, state_size, &live_across);

    Some(StateMachineResult {
        poll_fn,
        state_fields,
        state_size,
    })
}

// ── Linearization ───────────────────────────────────────────────────

/// A linear statement with its source block info.
#[derive(Debug, Clone)]
struct LinearStmt {
    stmt: MirStmt,
}

/// Flatten a function's blocks into a linear statement sequence.
/// For spawn closures this is straightforward since they're mostly linear.
fn linearize(func: &MirFunction) -> Vec<LinearStmt> {
    let mut result = Vec::new();

    // Walk blocks in order. Spawn closures have a simple linear structure.
    // We follow Goto chains but stop at branches/switches.
    let mut visited = HashSet::new();
    let mut current = Some(func.entry_block);

    while let Some(block_id) = current {
        if !visited.insert(block_id) {
            break; // avoid cycles
        }

        let block = &func.blocks[block_id.0 as usize];
        for stmt in &block.statements {
            result.push(LinearStmt { stmt: stmt.clone() });
        }

        current = match &block.terminator {
            MirTerminator::Goto { target } => Some(*target),
            MirTerminator::Return { .. } => None,
            _ => None, // branches/switches end linearization
        };
    }

    result
}

/// Find indices of yield point calls in the linear sequence.
fn find_yield_indices(stmts: &[LinearStmt]) -> Vec<usize> {
    stmts
        .iter()
        .enumerate()
        .filter_map(|(i, ls)| {
            if let MirStmt::Call { func, .. } = &ls.stmt {
                if is_yield_point(&func.name) {
                    return Some(i);
                }
            }
            None
        })
        .collect()
}

// ── Segmentation ────────────────────────────────────────────────────

/// A segment is a slice of statements between yield points.
/// Segment 0: [start .. first yield]
/// Segment 1: [first yield+1 .. second yield]
/// ...
/// Segment N: [last yield+1 .. end]
struct Segment {
    stmts: Vec<MirStmt>,
}

fn segment(linear: &[LinearStmt], yield_indices: &[usize]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut start = 0;

    for &yi in yield_indices {
        // Include the yield call itself in the segment (it's the last stmt)
        let end = yi + 1;
        segments.push(Segment {
            stmts: linear[start..end].iter().map(|ls| ls.stmt.clone()).collect(),
        });
        start = end;
    }

    // Final segment: everything after the last yield
    segments.push(Segment {
        stmts: linear[start..].iter().map(|ls| ls.stmt.clone()).collect(),
    });

    segments
}

// ── Liveness analysis ───────────────────────────────────────────────

/// Collect LocalIds that are defined (written) by a statement.
fn stmt_defs(stmt: &MirStmt) -> Vec<LocalId> {
    match stmt {
        MirStmt::Assign { dst, .. } => vec![*dst],
        MirStmt::Store { .. } => vec![],
        MirStmt::Call { dst: Some(d), .. } => vec![*d],
        MirStmt::Call { dst: None, .. } => vec![],
        MirStmt::ClosureCreate { dst, .. } => vec![*dst],
        MirStmt::ClosureCall { dst: Some(d), .. } => vec![*d],
        MirStmt::ClosureCall { dst: None, .. } => vec![],
        MirStmt::LoadCapture { dst, .. } => vec![*dst],
        MirStmt::ResourceRegister { dst, .. } => vec![*dst],
        MirStmt::PoolCheckedAccess { dst, .. } => vec![*dst],
        _ => vec![],
    }
}

/// Collect LocalIds that are used (read) by a statement.
fn stmt_uses(stmt: &MirStmt) -> Vec<LocalId> {
    let mut uses = Vec::new();
    match stmt {
        MirStmt::Assign { rvalue, .. } => rvalue_uses(rvalue, &mut uses),
        MirStmt::Store { addr, value, .. } => {
            uses.push(*addr);
            operand_uses(value, &mut uses);
        }
        MirStmt::Call { args, .. } => {
            for arg in args {
                operand_uses(arg, &mut uses);
            }
        }
        MirStmt::ClosureCreate { captures, .. } => {
            for cap in captures {
                uses.push(cap.local_id);
            }
        }
        MirStmt::ClosureCall { closure, args, .. } => {
            uses.push(*closure);
            for arg in args {
                operand_uses(arg, &mut uses);
            }
        }
        MirStmt::LoadCapture { env_ptr, .. } => {
            uses.push(*env_ptr);
        }
        MirStmt::ClosureDrop { closure } => {
            uses.push(*closure);
        }
        MirStmt::ResourceConsume { resource_id } => {
            uses.push(*resource_id);
        }
        MirStmt::PoolCheckedAccess { pool, handle, .. } => {
            uses.push(*pool);
            uses.push(*handle);
        }
        _ => {}
    }
    uses
}

fn operand_uses(op: &MirOperand, uses: &mut Vec<LocalId>) {
    if let MirOperand::Local(id) = op {
        uses.push(*id);
    }
}

fn rvalue_uses(rv: &MirRValue, uses: &mut Vec<LocalId>) {
    match rv {
        MirRValue::Use(op) => operand_uses(op, uses),
        MirRValue::Ref(id) => uses.push(*id),
        MirRValue::Deref(op) => operand_uses(op, uses),
        MirRValue::BinaryOp { left, right, .. } => {
            operand_uses(left, uses);
            operand_uses(right, uses);
        }
        MirRValue::UnaryOp { operand, .. } => operand_uses(operand, uses),
        MirRValue::Cast { value, .. } => operand_uses(value, uses),
        MirRValue::Field { base, .. } => operand_uses(base, uses),
        MirRValue::EnumTag { value } => operand_uses(value, uses),
    }
}

/// Compute the set of locals that are live across yield boundaries.
/// A local is "live across" if it's defined in segment i (or earlier)
/// and used in segment j where j > i.
fn compute_liveness(segments: &[Segment], _func: &MirFunction) -> HashSet<LocalId> {
    // Collect defs and uses per segment
    let mut defs_before: HashSet<LocalId> = HashSet::new();
    let mut uses_after: HashSet<LocalId> = HashSet::new();

    // For each yield boundary, accumulate defs from segments 0..i
    // and uses from segments i+1..N
    let n = segments.len();

    // Collect all defs per segment
    let seg_defs: Vec<HashSet<LocalId>> = segments
        .iter()
        .map(|seg| {
            let mut defs = HashSet::new();
            for stmt in &seg.stmts {
                for d in stmt_defs(stmt) {
                    defs.insert(d);
                }
            }
            defs
        })
        .collect();

    // Collect all uses per segment
    let seg_uses: Vec<HashSet<LocalId>> = segments
        .iter()
        .map(|seg| {
            let mut uses_set = HashSet::new();
            for stmt in &seg.stmts {
                for u in stmt_uses(stmt) {
                    uses_set.insert(u);
                }
            }
            uses_set
        })
        .collect();

    // For each yield boundary (between segment i and i+1):
    // Live = (union of defs in 0..=i) ∩ (union of uses in i+1..N)
    let mut live = HashSet::new();
    for boundary in 0..n.saturating_sub(1) {
        defs_before.extend(&seg_defs[boundary]);
        uses_after.clear();
        for j in (boundary + 1)..n {
            uses_after.extend(&seg_uses[j]);
        }
        for id in defs_before.intersection(&uses_after) {
            live.insert(*id);
        }
    }

    live
}

// ── State struct layout ─────────────────────────────────────────────

fn mir_type_size(ty: &MirType) -> u32 {
    match ty {
        MirType::Void => 0,
        MirType::Bool | MirType::I8 | MirType::U8 => 1,
        MirType::I16 | MirType::U16 => 2,
        MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
        MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr | MirType::FuncPtr(_) => 8,
        MirType::String => 16,
        MirType::Struct(_) | MirType::Enum(_) => 8,
        MirType::Array { elem, len } => mir_type_size(elem) * len,
    }
}

fn mir_type_align(ty: &MirType) -> u32 {
    match ty {
        MirType::Void => 1,
        MirType::Bool | MirType::I8 | MirType::U8 => 1,
        MirType::I16 | MirType::U16 => 2,
        MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
        _ => 8,
    }
}

fn align_up(offset: u32, align: u32) -> u32 {
    (offset + align - 1) & !(align - 1)
}

/// Build the state struct layout from the set of live-across locals.
fn build_state_layout(
    live_across: &HashSet<LocalId>,
    func: &MirFunction,
) -> (Vec<StateField>, u32) {
    let mut fields = Vec::new();

    // Field 0: state_tag (I32) — always at offset 0
    fields.push(StateField {
        local_id: None,
        name: "state_tag".to_string(),
        ty: MirType::I32,
        offset: 0,
        size: 4,
    });

    let mut offset: u32 = 4;

    // Add fields for each live-across local, sorted by LocalId for determinism
    let mut live_locals: Vec<LocalId> = live_across.iter().copied().collect();
    live_locals.sort_by_key(|id| id.0);

    for local_id in live_locals {
        // Look up the local's type
        let ty = func
            .locals
            .iter()
            .find(|l| l.id == local_id)
            .map(|l| l.ty.clone())
            .unwrap_or(MirType::I64);

        let size = mir_type_size(&ty);
        let align = mir_type_align(&ty);
        offset = align_up(offset, align);

        let name = func
            .locals
            .iter()
            .find(|l| l.id == local_id)
            .and_then(|l| l.name.clone())
            .unwrap_or_else(|| format!("_t{}", local_id.0));

        fields.push(StateField {
            local_id: Some(local_id),
            name,
            ty,
            offset,
            size,
        });

        offset += size;
    }

    // Final alignment to 8 bytes
    let total = align_up(offset, 8);
    (fields, total)
}

// ── Poll function generation ────────────────────────────────────────

/// Generate the poll function from segments.
///
/// Structure:
/// ```text
/// fn poll(state_ptr: Ptr, task_ctx: Ptr) -> I32 {
///   tag = load state_ptr[0] as I32
///   switch tag {
///     0 => { segment_0; save; tag=1; return PENDING }
///     1 => { restore; segment_1; save; tag=2; return PENDING }
///     ...
///     N => { restore; segment_N; return READY }
///   }
/// }
/// ```
fn generate_poll_fn(
    orig: &MirFunction,
    segments: &[Segment],
    _yield_indices: &[usize],
    state_fields: &[StateField],
    _state_size: u32,
    live_across: &HashSet<LocalId>,
) -> MirFunction {
    let poll_name = format!("{}_poll", orig.name);
    let mut builder = BlockBuilder::new(poll_name, MirType::I32);

    // Parameters: state_ptr and task_ctx
    let state_ptr = builder.add_param("__state_ptr".to_string(), MirType::Ptr);
    let _task_ctx = builder.add_param("__task_ctx".to_string(), MirType::Ptr);

    // Re-create all locals from the original function (except the env_ptr param)
    let mut local_map: HashMap<LocalId, LocalId> = HashMap::new();
    for local in &orig.locals {
        if local.is_param {
            continue; // skip env_ptr — we use state_ptr instead
        }
        let new_id = if let Some(ref name) = local.name {
            builder.alloc_local(name.clone(), local.ty.clone())
        } else {
            builder.alloc_temp(local.ty.clone())
        };
        local_map.insert(local.id, new_id);
    }

    // Build local-to-field mapping for save/restore
    let field_map: HashMap<LocalId, &StateField> = state_fields
        .iter()
        .filter_map(|f| f.local_id.map(|id| (id, f)))
        .collect();

    let n_segments = segments.len();

    // Load the state tag
    let tag_local = builder.alloc_temp(MirType::I32);
    builder.push_stmt(MirStmt::Assign {
        dst: tag_local,
        rvalue: MirRValue::Field {
            base: MirOperand::Local(state_ptr),
            field_index: 0, // state_tag is field 0
        },
    });

    // Create blocks for each segment
    let segment_blocks: Vec<BlockId> = (0..n_segments).map(|_| builder.create_block()).collect();
    let default_block = builder.create_block();

    // Switch on tag
    let cases: Vec<(u64, BlockId)> = segment_blocks
        .iter()
        .enumerate()
        .map(|(i, &block)| (i as u64, block))
        .collect();

    builder.terminate(MirTerminator::Switch {
        value: MirOperand::Local(tag_local),
        cases,
        default: default_block,
    });

    // Default block: return READY (shouldn't happen, but safe fallback)
    builder.switch_to_block(default_block);
    builder.terminate(MirTerminator::Return {
        value: Some(MirOperand::Constant(MirConst::Int(0))), // READY
    });

    // Generate each segment block
    for (seg_idx, segment) in segments.iter().enumerate() {
        builder.switch_to_block(segment_blocks[seg_idx]);
        let is_last = seg_idx == n_segments - 1;

        // Restore live locals from state (except for segment 0)
        if seg_idx > 0 {
            for (&orig_id, field) in &field_map {
                if let Some(&new_id) = local_map.get(&orig_id) {
                    builder.push_stmt(MirStmt::LoadCapture {
                        dst: new_id,
                        env_ptr: state_ptr,
                        offset: field.offset,
                    });
                }
            }
        }

        // Emit segment statements, remapping locals
        for stmt in &segment.stmts {
            let remapped = remap_stmt(stmt, &local_map);
            // Skip the yield call itself — it was just a marker
            if let MirStmt::Call { func: ref fref, .. } = remapped {
                if is_yield_point(&fref.name) {
                    // Emit the yield call (it submits I/O or timer)
                    builder.push_stmt(remapped);
                    continue;
                }
            }
            builder.push_stmt(remapped);
        }

        if is_last {
            // Final segment: return READY
            builder.terminate(MirTerminator::Return {
                value: Some(MirOperand::Constant(MirConst::Int(0))), // READY
            });
        } else {
            // Save live locals to state
            for (&orig_id, field) in &field_map {
                if live_across.contains(&orig_id) {
                    if let Some(&new_id) = local_map.get(&orig_id) {
                        builder.push_stmt(MirStmt::Store {
                            addr: state_ptr,
                            offset: field.offset,
                            value: MirOperand::Local(new_id),
                        });
                    }
                }
            }

            // Update state_tag to next segment
            let next_tag = (seg_idx + 1) as i64;
            builder.push_stmt(MirStmt::Store {
                addr: state_ptr,
                offset: 0, // state_tag offset
                value: MirOperand::Constant(MirConst::Int(next_tag)),
            });

            // Return PENDING
            builder.terminate(MirTerminator::Return {
                value: Some(MirOperand::Constant(MirConst::Int(1))), // PENDING
            });
        }
    }

    builder.finish()
}

/// Remap LocalIds in a statement using the local_map.
fn remap_stmt(stmt: &MirStmt, map: &HashMap<LocalId, LocalId>) -> MirStmt {
    match stmt {
        MirStmt::Assign { dst, rvalue } => MirStmt::Assign {
            dst: remap_id(*dst, map),
            rvalue: remap_rvalue(rvalue, map),
        },
        MirStmt::Store { addr, offset, value } => MirStmt::Store {
            addr: remap_id(*addr, map),
            offset: *offset,
            value: remap_operand(value, map),
        },
        MirStmt::Call { dst, func, args } => MirStmt::Call {
            dst: dst.map(|d| remap_id(d, map)),
            func: func.clone(),
            args: args.iter().map(|a| remap_operand(a, map)).collect(),
        },
        MirStmt::ClosureCreate { dst, func_name, captures, heap } => MirStmt::ClosureCreate {
            dst: remap_id(*dst, map),
            func_name: func_name.clone(),
            captures: captures
                .iter()
                .map(|c| crate::ClosureCapture {
                    local_id: remap_id(c.local_id, map),
                    offset: c.offset,
                    size: c.size,
                })
                .collect(),
            heap: *heap,
        },
        MirStmt::ClosureCall { dst, closure, args } => MirStmt::ClosureCall {
            dst: dst.map(|d| remap_id(d, map)),
            closure: remap_id(*closure, map),
            args: args.iter().map(|a| remap_operand(a, map)).collect(),
        },
        MirStmt::LoadCapture { dst, env_ptr, offset } => MirStmt::LoadCapture {
            dst: remap_id(*dst, map),
            env_ptr: remap_id(*env_ptr, map),
            offset: *offset,
        },
        MirStmt::ClosureDrop { closure } => MirStmt::ClosureDrop {
            closure: remap_id(*closure, map),
        },
        MirStmt::ResourceRegister { dst, type_name, scope_depth } => {
            MirStmt::ResourceRegister {
                dst: remap_id(*dst, map),
                type_name: type_name.clone(),
                scope_depth: *scope_depth,
            }
        }
        MirStmt::ResourceConsume { resource_id } => MirStmt::ResourceConsume {
            resource_id: remap_id(*resource_id, map),
        },
        MirStmt::PoolCheckedAccess { dst, pool, handle } => MirStmt::PoolCheckedAccess {
            dst: remap_id(*dst, map),
            pool: remap_id(*pool, map),
            handle: remap_id(*handle, map),
        },
        // Pass through unchanged
        other => other.clone(),
    }
}

fn remap_id(id: LocalId, map: &HashMap<LocalId, LocalId>) -> LocalId {
    map.get(&id).copied().unwrap_or(id)
}

fn remap_operand(op: &MirOperand, map: &HashMap<LocalId, LocalId>) -> MirOperand {
    match op {
        MirOperand::Local(id) => MirOperand::Local(remap_id(*id, map)),
        MirOperand::Constant(c) => MirOperand::Constant(c.clone()),
    }
}

fn remap_rvalue(rv: &MirRValue, map: &HashMap<LocalId, LocalId>) -> MirRValue {
    match rv {
        MirRValue::Use(op) => MirRValue::Use(remap_operand(op, map)),
        MirRValue::Ref(id) => MirRValue::Ref(remap_id(*id, map)),
        MirRValue::Deref(op) => MirRValue::Deref(remap_operand(op, map)),
        MirRValue::BinaryOp { op, left, right } => MirRValue::BinaryOp {
            op: *op,
            left: remap_operand(left, map),
            right: remap_operand(right, map),
        },
        MirRValue::UnaryOp { op, operand } => MirRValue::UnaryOp {
            op: *op,
            operand: remap_operand(operand, map),
        },
        MirRValue::Cast { value, target_ty } => MirRValue::Cast {
            value: remap_operand(value, map),
            target_ty: target_ty.clone(),
        },
        MirRValue::Field { base, field_index } => MirRValue::Field {
            base: remap_operand(base, map),
            field_index: *field_index,
        },
        MirRValue::EnumTag { value } => MirRValue::EnumTag {
            value: remap_operand(value, map),
        },
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_spawn_fn(stmts: Vec<MirStmt>) -> MirFunction {
        let mut builder = BlockBuilder::new("test__spawn_0".to_string(), MirType::Void);
        let _env = builder.add_param("__env".to_string(), MirType::Ptr);

        for stmt in stmts {
            builder.push_stmt(stmt);
        }

        builder.terminate(MirTerminator::Return { value: None });
        builder.finish()
    }

    #[test]
    fn no_yield_points_returns_none() {
        let func = make_test_spawn_fn(vec![MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "work".to_string() },
            args: vec![],
        }]);

        assert!(!has_yield_points(&func));
        assert!(transform(&func).is_none());
    }

    #[test]
    fn detects_sleep_as_yield_point() {
        let func = make_test_spawn_fn(vec![MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "rask_sleep_ns".to_string() },
            args: vec![MirOperand::Constant(MirConst::Int(1_000_000))],
        }]);

        assert!(has_yield_points(&func));
    }

    #[test]
    fn transform_produces_poll_fn() {
        let mut builder = BlockBuilder::new("test__spawn_0".to_string(), MirType::Void);
        let _env = builder.add_param("__env".to_string(), MirType::Ptr);

        let x = builder.alloc_local("x".to_string(), MirType::I64);
        builder.push_stmt(MirStmt::Assign {
            dst: x,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(42))),
        });

        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "rask_sleep_ns".to_string() },
            args: vec![MirOperand::Constant(MirConst::Int(1_000_000))],
        });

        // Use x after the yield
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "print_i64".to_string() },
            args: vec![MirOperand::Local(x)],
        });

        builder.terminate(MirTerminator::Return { value: None });
        let func = builder.finish();

        let result = transform(&func).expect("should transform");

        // Poll function should have state_ptr and task_ctx params
        assert_eq!(result.poll_fn.params.len(), 2);
        assert_eq!(result.poll_fn.ret_ty, MirType::I32);

        // State struct should have state_tag + x
        assert!(result.state_fields.len() >= 2);
        assert_eq!(result.state_fields[0].name, "state_tag");

        // State size should be reasonable (tag + one i64)
        assert!(result.state_size >= 12);

        // Should have at least 3 blocks: entry + 2 segments + default
        assert!(result.poll_fn.blocks.len() >= 3);
    }

    #[test]
    fn liveness_tracks_cross_yield_locals() {
        let mut builder = BlockBuilder::new("test__spawn_0".to_string(), MirType::Void);
        let _env = builder.add_param("__env".to_string(), MirType::Ptr);

        let x = builder.alloc_local("x".to_string(), MirType::I64);
        let y = builder.alloc_local("y".to_string(), MirType::I64);

        // x = 1 (used after yield)
        builder.push_stmt(MirStmt::Assign {
            dst: x,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(1))),
        });

        // y = 2 (NOT used after yield)
        builder.push_stmt(MirStmt::Assign {
            dst: y,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(2))),
        });

        // yield
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "rask_sleep_ns".to_string() },
            args: vec![MirOperand::Constant(MirConst::Int(1000))],
        });

        // use x (cross-yield)
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "print_i64".to_string() },
            args: vec![MirOperand::Local(x)],
        });

        builder.terminate(MirTerminator::Return { value: None });
        let func = builder.finish();

        let result = transform(&func).expect("should transform");

        // x should be in state struct, y should not
        let has_x = result
            .state_fields
            .iter()
            .any(|f| f.local_id == Some(x) && f.name == "x");
        let has_y = result
            .state_fields
            .iter()
            .any(|f| f.local_id == Some(y));

        assert!(has_x, "x should be in state struct (live across yield)");
        assert!(!has_y, "y should NOT be in state struct (dead after yield)");
    }

    #[test]
    fn multiple_yield_points_create_multiple_segments() {
        let mut builder = BlockBuilder::new("test__spawn_0".to_string(), MirType::Void);
        let _env = builder.add_param("__env".to_string(), MirType::Ptr);

        // work1
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "work1".to_string() },
            args: vec![],
        });

        // yield 1
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "rask_sleep_ns".to_string() },
            args: vec![MirOperand::Constant(MirConst::Int(1000))],
        });

        // work2
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "work2".to_string() },
            args: vec![],
        });

        // yield 2
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "rask_yield".to_string() },
            args: vec![],
        });

        // work3
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef { name: "work3".to_string() },
            args: vec![],
        });

        builder.terminate(MirTerminator::Return { value: None });
        let func = builder.finish();

        let result = transform(&func).expect("should transform");

        // 3 segments: [work1+yield1], [work2+yield2], [work3]
        // Poll function should have entry block + 3 segment blocks + default
        assert!(
            result.poll_fn.blocks.len() >= 4,
            "expected >=4 blocks for 3 segments, got {}",
            result.poll_fn.blocks.len()
        );
    }
}
