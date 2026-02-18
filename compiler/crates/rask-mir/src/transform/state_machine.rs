// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! State machine transform for spawn closures.
//!
//! Converts spawn closure functions containing yield points into poll-based
//! state machines. A yield point is a call to a function that suspends the
//! green task (sleep, I/O yield helpers).
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
    BlockBuilder, LocalId, MirFunction, MirOperand,
    MirRValue, MirStmt, MirTerminator, MirType,
};
use crate::operand::MirConst;

// ── Yield point registry ────────────────────────────────────────────

/// Known functions that act as yield points in green tasks.
/// Each of these submits work to the scheduler/I/O engine and expects the
/// poll function to return PENDING afterward.
///
/// NOT included: `rask_channel_send_async`/`recv_async` — they handle
/// yielding internally via retry loops and complete before returning.
const YIELD_POINT_FUNCTIONS: &[&str] = &[
    "rask_green_sleep_ns",
    "rask_yield_timeout",
    "rask_yield_read",
    "rask_yield_write",
    "rask_yield_accept",
    "rask_yield",
];

pub fn is_yield_point(name: &str) -> bool {
    YIELD_POINT_FUNCTIONS.contains(&name)
}

// ── Public API ──────────────────────────────────────────────────────

/// Result of the state machine transform.
pub struct StateMachineResult {
    /// The poll function: `fn poll(state_ptr: Ptr, task_ctx: Ptr) -> I32`
    pub poll_fn: MirFunction,
    /// Layout of the state struct (field offsets and types).
    pub state_fields: Vec<StateField>,
    /// Total size of the state struct in bytes.
    pub state_size: u32,
    /// Captures the parent must store into the state struct at spawn time.
    /// Each entry: (env_offset in original closure, offset in state struct).
    pub capture_stores: Vec<(u32, u32)>,
}

/// A field in the generated state struct.
#[derive(Debug, Clone)]
pub struct StateField {
    /// Which local variable this saves (None for state_tag).
    pub local_id: Option<LocalId>,
    pub name: String,
    pub ty: MirType,
    pub offset: u32,
    pub size: u32,
}

/// Check if a spawn function contains yield points.
pub fn has_yield_points(func: &MirFunction) -> bool {
    func.blocks.iter().any(|block| {
        block.statements.iter().any(|stmt| matches!(
            stmt,
            MirStmt::Call { func: fref, .. } if is_yield_point(&fref.name)
        ))
    })
}

/// Transform a spawn closure function into a poll-based state machine.
///
/// Input: `fn name(env_ptr: Ptr) -> Void`
/// Output: `fn name_poll(state_ptr: Ptr, task_ctx: Ptr) -> I32`
///
/// Returns None if no yield points found.
pub fn transform(func: &MirFunction) -> Option<StateMachineResult> {
    if !has_yield_points(func) {
        return None;
    }

    let linear = linearize(func);

    let yield_indices = find_yield_indices(&linear);
    if yield_indices.is_empty() {
        return None;
    }

    let segments = segment(&linear, &yield_indices);

    // Extract capture info from LoadCapture instructions
    let captures = extract_captures(&linear, func);

    // Locals live across yield boundaries
    let live_across = compute_liveness(&segments);

    // Build state struct: tag + captures + live-across locals
    let (state_fields, state_size, capture_stores) =
        build_state_layout(&captures, &live_across, func);

    // Build capture offset remap: env_offset → state_struct_offset
    let capture_remap: HashMap<u32, u32> = capture_stores
        .iter()
        .map(|&(env_off, state_off)| (env_off, state_off))
        .collect();

    let poll_fn = generate_poll_fn(
        func, &segments, &state_fields, &live_across, &capture_remap,
    );

    Some(StateMachineResult {
        poll_fn,
        state_fields,
        state_size,
        capture_stores,
    })
}

// ── Linearization ───────────────────────────────────────────────────

/// Flatten a function's blocks into a linear statement sequence.
/// Spawn closures have simple linear structure — follow Goto chains,
/// stop at branches.
fn linearize(func: &MirFunction) -> Vec<MirStmt> {
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    let mut current = Some(func.entry_block);

    while let Some(block_id) = current {
        if !visited.insert(block_id) {
            break;
        }

        let block = &func.blocks[block_id.0 as usize];
        for stmt in &block.statements {
            result.push(stmt.clone());
        }

        current = match &block.terminator {
            MirTerminator::Goto { target } => Some(*target),
            _ => None,
        };
    }

    result
}

fn find_yield_indices(stmts: &[MirStmt]) -> Vec<usize> {
    stmts
        .iter()
        .enumerate()
        .filter_map(|(i, stmt)| match stmt {
            MirStmt::Call { func, .. } if is_yield_point(&func.name) => Some(i),
            _ => None,
        })
        .collect()
}

// ── Capture extraction ──────────────────────────────────────────────

/// A captured variable loaded via LoadCapture in the spawn body.
struct CaptureInfo {
    /// Local in the spawn function that receives this capture.
    dst_local: LocalId,
    /// Offset in the original closure environment.
    env_offset: u32,
    ty: MirType,
}

/// Find all LoadCapture instructions to identify captured variables.
fn extract_captures(stmts: &[MirStmt], func: &MirFunction) -> Vec<CaptureInfo> {
    stmts
        .iter()
        .filter_map(|stmt| match stmt {
            MirStmt::LoadCapture { dst, offset, .. } => {
                let ty = func.locals.iter()
                    .find(|l| l.id == *dst)
                    .map(|l| l.ty.clone())
                    .unwrap_or(MirType::I64);
                Some(CaptureInfo {
                    dst_local: *dst,
                    env_offset: *offset,
                    ty,
                })
            }
            _ => None,
        })
        .collect()
}

// ── Segmentation ────────────────────────────────────────────────────

/// A slice of statements between yield points.
struct Segment {
    stmts: Vec<MirStmt>,
}

fn segment(linear: &[MirStmt], yield_indices: &[usize]) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut start = 0;

    for &yi in yield_indices {
        // Include the yield call in this segment (last stmt before suspension)
        segments.push(Segment {
            stmts: linear[start..=yi].to_vec(),
        });
        start = yi + 1;
    }

    // Final segment: everything after the last yield
    segments.push(Segment {
        stmts: linear[start..].to_vec(),
    });

    segments
}

// ── Liveness analysis ───────────────────────────────────────────────

/// Collect locals defined by a statement.
fn stmt_defs(stmt: &MirStmt) -> Vec<LocalId> {
    match stmt {
        MirStmt::Assign { dst, .. }
        | MirStmt::ClosureCreate { dst, .. }
        | MirStmt::LoadCapture { dst, .. }
        | MirStmt::ResourceRegister { dst, .. }
        | MirStmt::PoolCheckedAccess { dst, .. } => vec![*dst],
        MirStmt::Call { dst: Some(d), .. }
        | MirStmt::ClosureCall { dst: Some(d), .. } => vec![*d],
        _ => vec![],
    }
}

/// Collect locals used by a statement.
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
        MirStmt::LoadCapture { env_ptr, .. } => uses.push(*env_ptr),
        MirStmt::ClosureDrop { closure } => uses.push(*closure),
        MirStmt::ResourceConsume { resource_id } => uses.push(*resource_id),
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
        MirRValue::Use(op) | MirRValue::Deref(op) => operand_uses(op, uses),
        MirRValue::Ref(id) => uses.push(*id),
        MirRValue::BinaryOp { left, right, .. } => {
            operand_uses(left, uses);
            operand_uses(right, uses);
        }
        MirRValue::UnaryOp { operand, .. } => operand_uses(operand, uses),
        MirRValue::Cast { value, .. } => operand_uses(value, uses),
        MirRValue::Field { base, .. } => operand_uses(base, uses),
        MirRValue::EnumTag { value } => operand_uses(value, uses),
        MirRValue::ArrayIndex { base, index, .. } => {
            operand_uses(base, uses);
            operand_uses(index, uses);
        }
    }
}

/// Compute locals live across at least one yield boundary.
///
/// A local is live-across if it's defined in segments 0..=i and used in
/// segments i+1..N for any boundary i. Since we only need the flat set
/// (not per-boundary), this simplifies to:
///   defs_in(0..N-1) ∩ uses_in(1..N)
fn compute_liveness(segments: &[Segment]) -> HashSet<LocalId> {
    if segments.len() < 2 {
        return HashSet::new();
    }

    // Defs in all segments except the last
    let mut defs_before_last: HashSet<LocalId> = HashSet::new();
    for seg in &segments[..segments.len() - 1] {
        for stmt in &seg.stmts {
            for d in stmt_defs(stmt) {
                defs_before_last.insert(d);
            }
        }
    }

    // Uses in all segments except the first
    let mut uses_after_first: HashSet<LocalId> = HashSet::new();
    for seg in &segments[1..] {
        for stmt in &seg.stmts {
            for u in stmt_uses(stmt) {
                uses_after_first.insert(u);
            }
        }
    }

    defs_before_last.intersection(&uses_after_first).copied().collect()
}

// ── State struct layout ─────────────────────────────────────────────

fn align_up(offset: u32, align: u32) -> u32 {
    (offset + align - 1) & !(align - 1)
}

/// Build the state struct layout.
///
/// Layout: [state_tag: I32] [captures...] [live-across locals...]
///
/// Returns: (fields, total_size, capture_stores)
/// capture_stores: Vec<(env_offset, state_offset)> — what the parent must init.
fn build_state_layout(
    captures: &[CaptureInfo],
    live_across: &HashSet<LocalId>,
    func: &MirFunction,
) -> (Vec<StateField>, u32, Vec<(u32, u32)>) {
    let mut fields = Vec::new();
    let mut capture_stores = Vec::new();

    // Field 0: state_tag (I32) at offset 0
    fields.push(StateField {
        local_id: None,
        name: "state_tag".to_string(),
        ty: MirType::I32,
        offset: 0,
        size: 4,
    });
    let mut offset: u32 = 4;

    // Track which locals are already covered by captures
    let mut captured_locals: HashSet<LocalId> = HashSet::new();

    // Captures — always included (needed by segment 0)
    for cap in captures {
        let size = cap.ty.size();
        let align = cap.ty.align();
        offset = align_up(offset, align);

        let name = func.locals.iter()
            .find(|l| l.id == cap.dst_local)
            .and_then(|l| l.name.clone())
            .unwrap_or_else(|| format!("_cap{}", cap.dst_local.0));

        capture_stores.push((cap.env_offset, offset));
        captured_locals.insert(cap.dst_local);

        fields.push(StateField {
            local_id: Some(cap.dst_local),
            name,
            ty: cap.ty.clone(),
            offset,
            size,
        });
        offset += size;
    }

    // Live-across locals (excluding those already added as captures)
    let mut live_locals: Vec<LocalId> = live_across.iter()
        .filter(|id| !captured_locals.contains(id))
        .copied()
        .collect();
    live_locals.sort_by_key(|id| id.0);

    for local_id in live_locals {
        let local = func.locals.iter().find(|l| l.id == local_id);
        let ty = local.map(|l| l.ty.clone()).unwrap_or(MirType::I64);
        let size = ty.size();
        let align = ty.align();
        offset = align_up(offset, align);

        let name = local
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

    let total = align_up(offset, 8);
    (fields, total, capture_stores)
}

// ── Poll function generation ────────────────────────────────────────

/// Generate the poll function from segments.
///
/// ```text
/// fn poll(state_ptr: Ptr, task_ctx: Ptr) -> I32 {
///   tag = load state_ptr+0
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
    state_fields: &[StateField],
    _live_across: &HashSet<LocalId>,
    capture_remap: &HashMap<u32, u32>,
) -> MirFunction {
    let poll_name = format!("{}_poll", orig.name);
    let mut builder = BlockBuilder::new(poll_name, MirType::I32);

    let state_ptr = builder.add_param("__state_ptr".to_string(), MirType::Ptr);
    let _task_ctx = builder.add_param("__task_ctx".to_string(), MirType::Ptr);

    // Find the original env_ptr param id (to remap LoadCapture instructions)
    let env_param_id = orig.params.iter()
        .find(|p| p.name.as_deref() == Some("__env"))
        .map(|p| p.id);

    // Re-create non-param locals from the original function
    let mut local_map: HashMap<LocalId, LocalId> = HashMap::new();
    for local in &orig.locals {
        if local.is_param {
            continue;
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

    // Load state tag from state_ptr+0
    let tag_local = builder.alloc_temp(MirType::I32);
    builder.push_stmt(MirStmt::LoadCapture {
        dst: tag_local,
        env_ptr: state_ptr,
        offset: 0,
    });

    // Create segment blocks + default
    let segment_blocks: Vec<_> = (0..n_segments).map(|_| builder.create_block()).collect();
    let default_block = builder.create_block();

    let cases: Vec<_> = segment_blocks
        .iter()
        .enumerate()
        .map(|(i, &block)| (i as u64, block))
        .collect();

    builder.terminate(MirTerminator::Switch {
        value: MirOperand::Local(tag_local),
        cases,
        default: default_block,
    });

    // Default: return READY (unreachable in correct code)
    builder.switch_to_block(default_block);
    builder.terminate(MirTerminator::Return {
        value: Some(MirOperand::Constant(MirConst::Int(0))),
    });

    for (seg_idx, seg) in segments.iter().enumerate() {
        builder.switch_to_block(segment_blocks[seg_idx]);
        let is_last = seg_idx == n_segments - 1;

        // Restore live locals from state (segments 1+)
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

        // Emit segment statements with local remapping
        for stmt in &seg.stmts {
            let remapped = remap_stmt(
                stmt, &local_map, env_param_id, state_ptr, capture_remap,
            );
            builder.push_stmt(remapped);
        }

        if is_last {
            builder.terminate(MirTerminator::Return {
                value: Some(MirOperand::Constant(MirConst::Int(0))), // READY
            });
        } else {
            // Save live locals to state
            for (&orig_id, field) in &field_map {
                if let Some(&new_id) = local_map.get(&orig_id) {
                    builder.push_stmt(MirStmt::Store {
                        addr: state_ptr,
                        offset: field.offset,
                        value: MirOperand::Local(new_id),
                    });
                }
            }

            // Advance state_tag
            builder.push_stmt(MirStmt::Store {
                addr: state_ptr,
                offset: 0,
                value: MirOperand::Constant(MirConst::Int((seg_idx + 1) as i64)),
            });

            builder.terminate(MirTerminator::Return {
                value: Some(MirOperand::Constant(MirConst::Int(1))), // PENDING
            });
        }
    }

    let mut poll_fn = builder.finish();
    poll_fn.source_file = orig.source_file.clone();
    poll_fn
}

// ── Statement remapping ─────────────────────────────────────────────

/// Remap locals in a statement. Also rewrites LoadCapture instructions
/// that reference the original env_ptr to use state_ptr with the
/// capture's state-struct offset.
fn remap_stmt(
    stmt: &MirStmt,
    map: &HashMap<LocalId, LocalId>,
    env_param_id: Option<LocalId>,
    state_ptr: LocalId,
    capture_remap: &HashMap<u32, u32>,
) -> MirStmt {
    // Rewrite LoadCapture from closure env → load from state struct
    if let MirStmt::LoadCapture { dst, env_ptr, offset } = stmt {
        if env_param_id == Some(*env_ptr) {
            if let Some(&state_offset) = capture_remap.get(offset) {
                return MirStmt::LoadCapture {
                    dst: remap_id(*dst, map),
                    env_ptr: state_ptr,
                    offset: state_offset,
                };
            }
        }
    }

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
        MirStmt::ArrayStore { base, index, elem_size, value } => MirStmt::ArrayStore {
            base: remap_id(*base, map),
            index: remap_operand(index, map),
            elem_size: *elem_size,
            value: remap_operand(value, map),
        },
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
        MirRValue::Use(op) | MirRValue::Deref(op) => {
            let remapped = remap_operand(op, map);
            if matches!(rv, MirRValue::Deref(_)) {
                MirRValue::Deref(remapped)
            } else {
                MirRValue::Use(remapped)
            }
        }
        MirRValue::Ref(id) => MirRValue::Ref(remap_id(*id, map)),
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
        MirRValue::ArrayIndex { base, index, elem_size } => MirRValue::ArrayIndex {
            base: remap_operand(base, map),
            index: remap_operand(index, map),
            elem_size: *elem_size,
        },
    }
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::FunctionRef;

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
            func: FunctionRef::internal("work".to_string()),
            args: vec![],
        }]);
        assert!(!has_yield_points(&func));
        assert!(transform(&func).is_none());
    }

    #[test]
    fn detects_yield_point() {
        let func = make_test_spawn_fn(vec![MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("rask_green_sleep_ns".to_string()),
            args: vec![MirOperand::Constant(MirConst::Int(1_000_000))],
        }]);
        assert!(has_yield_points(&func));
    }

    #[test]
    fn blocking_channel_ops_are_not_yield_points() {
        let func = make_test_spawn_fn(vec![MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("rask_channel_send_async".to_string()),
            args: vec![
                MirOperand::Constant(MirConst::Int(0)),
                MirOperand::Constant(MirConst::Int(42)),
            ],
        }]);
        assert!(!has_yield_points(&func));
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
            func: FunctionRef::internal("rask_green_sleep_ns".to_string()),
            args: vec![MirOperand::Constant(MirConst::Int(1_000_000))],
        });
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("print_i64".to_string()),
            args: vec![MirOperand::Local(x)],
        });
        builder.terminate(MirTerminator::Return { value: None });
        let func = builder.finish();

        let result = transform(&func).expect("should transform");

        assert_eq!(result.poll_fn.params.len(), 2);
        assert_eq!(result.poll_fn.ret_ty, MirType::I32);
        assert!(result.state_fields.len() >= 2);
        assert_eq!(result.state_fields[0].name, "state_tag");
        // tag(4) + pad(4) + x(8) = 16
        assert!(result.state_size >= 16);
        // entry + 2 segments + default = 4 blocks
        assert!(result.poll_fn.blocks.len() >= 4,
            "got {} blocks", result.poll_fn.blocks.len());
    }

    #[test]
    fn liveness_tracks_cross_yield_locals() {
        let mut builder = BlockBuilder::new("test__spawn_0".to_string(), MirType::Void);
        let _env = builder.add_param("__env".to_string(), MirType::Ptr);

        let x = builder.alloc_local("x".to_string(), MirType::I64);
        let y = builder.alloc_local("y".to_string(), MirType::I64);

        builder.push_stmt(MirStmt::Assign {
            dst: x,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(1))),
        });
        builder.push_stmt(MirStmt::Assign {
            dst: y,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(2))),
        });
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("rask_green_sleep_ns".to_string()),
            args: vec![MirOperand::Constant(MirConst::Int(1000))],
        });
        // Only x used after yield
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("print_i64".to_string()),
            args: vec![MirOperand::Local(x)],
        });
        builder.terminate(MirTerminator::Return { value: None });
        let func = builder.finish();

        let result = transform(&func).expect("should transform");

        let has_x = result.state_fields.iter()
            .any(|f| f.local_id == Some(x) && f.name == "x");
        let has_y = result.state_fields.iter()
            .any(|f| f.local_id == Some(y));

        assert!(has_x, "x should be in state struct (live across yield)");
        assert!(!has_y, "y should NOT be in state struct (dead after yield)");
    }

    #[test]
    fn multiple_yield_points_create_multiple_segments() {
        let mut builder = BlockBuilder::new("test__spawn_0".to_string(), MirType::Void);
        let _env = builder.add_param("__env".to_string(), MirType::Ptr);

        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("work1".to_string()),
            args: vec![],
        });
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("rask_green_sleep_ns".to_string()),
            args: vec![MirOperand::Constant(MirConst::Int(1000))],
        });
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("work2".to_string()),
            args: vec![],
        });
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("rask_yield".to_string()),
            args: vec![],
        });
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("work3".to_string()),
            args: vec![],
        });
        builder.terminate(MirTerminator::Return { value: None });
        let func = builder.finish();

        let result = transform(&func).expect("should transform");

        // entry + 3 segments + default = 5
        assert!(result.poll_fn.blocks.len() >= 5,
            "expected >=5 blocks for 3 segments, got {}",
            result.poll_fn.blocks.len());
    }

    #[test]
    fn captures_included_in_state_struct() {
        let mut builder = BlockBuilder::new("test__spawn_0".to_string(), MirType::Void);
        let env = builder.add_param("__env".to_string(), MirType::Ptr);

        // LoadCapture: load captured var from env at offset 0
        let captured = builder.alloc_local("captured".to_string(), MirType::I64);
        builder.push_stmt(MirStmt::LoadCapture {
            dst: captured,
            env_ptr: env,
            offset: 0,
        });
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("rask_green_sleep_ns".to_string()),
            args: vec![MirOperand::Constant(MirConst::Int(1000))],
        });
        // Use captured var after yield
        builder.push_stmt(MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("print_i64".to_string()),
            args: vec![MirOperand::Local(captured)],
        });
        builder.terminate(MirTerminator::Return { value: None });
        let func = builder.finish();

        let result = transform(&func).expect("should transform");

        let has_cap = result.state_fields.iter()
            .any(|f| f.name == "captured");
        assert!(has_cap, "captured var should be in state struct");

        assert!(!result.capture_stores.is_empty(),
            "should have capture stores");
        let (env_off, _state_off) = result.capture_stores[0];
        assert_eq!(env_off, 0, "capture env offset should be 0");
    }

    #[test]
    fn poll_fn_loads_tag_via_load_capture() {
        let func = make_test_spawn_fn(vec![MirStmt::Call {
            dst: None,
            func: FunctionRef::internal("rask_green_sleep_ns".to_string()),
            args: vec![MirOperand::Constant(MirConst::Int(100))],
        }]);

        let result = transform(&func).expect("should transform");

        // Entry block should start with LoadCapture (not Field) for the tag
        let entry = &result.poll_fn.blocks[0];
        let first_stmt = &entry.statements[0];
        assert!(
            matches!(first_stmt, MirStmt::LoadCapture { offset: 0, .. }),
            "tag should be loaded via LoadCapture at offset 0, got {:?}",
            first_stmt
        );
    }
}
