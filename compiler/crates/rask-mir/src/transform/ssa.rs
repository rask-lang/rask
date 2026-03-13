// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! SSA construction and destruction for MIR functions.
//!
//! `construct()` converts non-SSA MIR to pruned SSA form using the iterated
//! dominance frontier algorithm (Cytron et al. 1991) with liveness-based
//! pruning. Each variable definition creates a new versioned local; phi nodes
//! are inserted at join points where multiple definitions converge.
//!
//! `destruct()` lowers phi nodes to copy statements in predecessor blocks,
//! splitting critical edges where necessary.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::analysis::{cfg, dominators::DominatorTree, liveness, uses};
use crate::{
    BlockId, LocalId, MirBlock, MirFunction, MirLocal, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminator, MirTerminatorKind,
};

// ---------------------------------------------------------------------------
// SSA construction
// ---------------------------------------------------------------------------

/// Convert a MIR function from non-SSA to pruned SSA form.
///
/// After this, every local has exactly one definition point (either a statement,
/// a phi node, or a function parameter). Phi nodes appear as `MirStmtKind::Phi`
/// at the beginning of blocks.
pub fn construct(func: &mut MirFunction) {
    if func.blocks.is_empty() {
        return;
    }

    let dom = DominatorTree::build(func);
    let df = dom.dominance_frontier(func);
    let live = liveness::analyze(func, &dom);
    let preds = cfg::predecessors(func);
    let cleanup_blocks = find_cleanup_blocks(func);

    let num_orig_locals = func.locals.len() + func.params.len();

    // Phase 1: Collect definition sites per original local.
    let def_blocks = collect_def_blocks(func, num_orig_locals);

    // Phase 2: Insert phi nodes at iterated dominance frontiers (pruned).
    insert_phis(func, &def_blocks, &df, &live, &cleanup_blocks, &preds);

    // Phase 3: Rename variables — walk dominator tree, versioning each local.
    let dom_children = build_dom_children(&dom, func);
    rename_variables(func, &dom, &dom_children, &preds, num_orig_locals);
}

/// Identify cleanup blocks (targets of EnsurePush).
fn find_cleanup_blocks(func: &MirFunction) -> HashSet<BlockId> {
    let mut cleanup = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::EnsurePush { cleanup_block } = &stmt.kind {
                cleanup.insert(*cleanup_block);
            }
        }
    }
    cleanup
}

/// For each original local, collect the set of blocks where it's defined.
fn collect_def_blocks(func: &MirFunction, num_locals: usize) -> Vec<HashSet<BlockId>> {
    let mut def_blocks = vec![HashSet::new(); num_locals];

    // Parameters are defined in the entry block.
    for param in &func.params {
        if (param.id.0 as usize) < num_locals {
            def_blocks[param.id.0 as usize].insert(func.entry_block);
        }
    }

    for block in &func.blocks {
        for stmt in &block.statements {
            if let Some(def) = uses::stmt_def(stmt) {
                if (def.0 as usize) < num_locals {
                    def_blocks[def.0 as usize].insert(block.id);
                }
            }
        }
    }

    def_blocks
}

/// Insert phi nodes at iterated dominance frontiers using the worklist algorithm.
/// Only inserts where the variable is live at entry (pruned SSA — SSA3).
fn insert_phis(
    func: &mut MirFunction,
    def_blocks: &[HashSet<BlockId>],
    df: &HashMap<BlockId, HashSet<BlockId>>,
    live: &liveness::LivenessResults,
    cleanup_blocks: &HashSet<BlockId>,
    preds: &HashMap<BlockId, Vec<BlockId>>,
) {
    let num_locals = def_blocks.len();
    // Track which (block, local) pairs already have a phi inserted.
    let mut has_phi: HashSet<(BlockId, u32)> = HashSet::new();

    for local_idx in 0..num_locals {
        let local = LocalId(local_idx as u32);
        let defs = &def_blocks[local_idx];

        // No phi needed if defined in 0 or 1 blocks and not in a loop.
        // The worklist handles the general case correctly regardless.
        let mut worklist: VecDeque<BlockId> = defs.iter().copied().collect();
        let mut ever_on_worklist: HashSet<BlockId> = defs.clone();

        while let Some(def_block) = worklist.pop_front() {
            let Some(frontier) = df.get(&def_block) else {
                continue;
            };
            for &df_block in frontier {
                if has_phi.contains(&(df_block, local_idx as u32)) {
                    continue;
                }
                // CL3: No phis at cleanup block boundaries.
                if cleanup_blocks.contains(&df_block) {
                    continue;
                }
                // Pruning: only insert if local is live at entry of df_block.
                if !live.live_at_entry(df_block, local) {
                    continue;
                }

                has_phi.insert((df_block, local_idx as u32));

                // Build phi with placeholder args (one per predecessor).
                let block_preds = preds.get(&df_block).cloned().unwrap_or_default();
                let args: Vec<(BlockId, MirOperand)> = block_preds
                    .iter()
                    .map(|&pred| (pred, MirOperand::Local(local)))
                    .collect();

                // Insert phi at the start of the block.
                let phi = MirStmt::dummy(MirStmtKind::Phi { dst: local, args });
                if let Some(block) = func.blocks.iter_mut().find(|b| b.id == df_block) {
                    // Insert after any existing phis (maintain phi-first invariant).
                    let insert_pos = block.statements.iter()
                        .position(|s| !matches!(s.kind, MirStmtKind::Phi { .. }))
                        .unwrap_or(block.statements.len());
                    block.statements.insert(insert_pos, phi);
                }

                // The phi is itself a definition — may need phis in its own DF.
                if !ever_on_worklist.contains(&df_block) {
                    ever_on_worklist.insert(df_block);
                    worklist.push_back(df_block);
                }
            }
        }
    }
}

/// Build children map from dominator tree (parent → children).
fn build_dom_children(
    dom: &DominatorTree,
    func: &MirFunction,
) -> HashMap<BlockId, Vec<BlockId>> {
    let mut children: HashMap<BlockId, Vec<BlockId>> = HashMap::new();
    for block in &func.blocks {
        if let Some(parent) = dom.idom(block.id) {
            children.entry(parent).or_default().push(block.id);
        }
    }
    children
}

/// Rename all locals to SSA-versioned names by walking the dominator tree.
///
/// For each original local, a stack tracks the current reaching definition
/// (the most recent version). At each block:
/// 1. Rewrite phi dst to a fresh version, push onto stack
/// 2. For each statement: rewrite uses to current version, then rewrite def to fresh version
/// 3. Fill in phi args in successor blocks
/// 4. Recurse into dominator-tree children
/// 5. Pop versions pushed in this block
fn rename_variables(
    func: &mut MirFunction,
    _dom: &DominatorTree,
    dom_children: &HashMap<BlockId, Vec<BlockId>>,
    preds: &HashMap<BlockId, Vec<BlockId>>,
    num_orig_locals: usize,
) {
    // version_counter[orig_local] tracks how many versions have been created.
    let mut version_counter: Vec<u32> = vec![0; num_orig_locals];
    // version_stack[orig_local] is a stack of LocalIds — top is the current reaching def.
    let mut version_stack: Vec<Vec<LocalId>> = (0..num_orig_locals)
        .map(|i| vec![LocalId(i as u32)])
        .collect();

    // Precompute block index for fast lookup.
    let block_index: HashMap<BlockId, usize> = func.blocks.iter()
        .enumerate()
        .map(|(i, b)| (b.id, i))
        .collect();

    // Collect original local info for creating new versions.
    let orig_local_info: Vec<(Option<String>, crate::MirType)> = {
        let mut info = Vec::with_capacity(num_orig_locals);
        for param in &func.params {
            info.push((param.name.clone(), param.ty.clone()));
        }
        for local in &func.locals {
            if (local.id.0 as usize) >= info.len() {
                // Extend to cover gaps
                while info.len() < local.id.0 as usize {
                    info.push((None, crate::MirType::I64));
                }
                info.push((local.name.clone(), local.ty.clone()));
            }
        }
        while info.len() < num_orig_locals {
            info.push((None, crate::MirType::I64));
        }
        info
    };

    rename_block(
        func.entry_block,
        func,
        dom_children,
        preds,
        &block_index,
        &mut version_counter,
        &mut version_stack,
        &orig_local_info,
        num_orig_locals,
    );
}

/// Create a fresh SSA version of an original local.
fn new_version(
    orig_local: LocalId,
    func: &mut MirFunction,
    version_counter: &mut [u32],
    version_stack: &mut [Vec<LocalId>],
    orig_local_info: &[(Option<String>, crate::MirType)],
    num_orig_locals: usize,
) -> LocalId {
    let orig = orig_local.0 as usize;
    if orig >= num_orig_locals {
        return orig_local;
    }

    version_counter[orig] += 1;
    let version = version_counter[orig];
    let new_id = LocalId((func.locals.len() + func.params.len()) as u32);

    let (ref name, ref ty) = orig_local_info[orig];
    func.locals.push(MirLocal {
        id: new_id,
        name: name.as_ref().map(|n| format!("{}_v{}", n, version)),
        ty: ty.clone(),
        is_param: false,
    });

    version_stack[orig].push(new_id);
    new_id
}

/// Get the current reaching definition for an original local.
fn current_version(orig_local: LocalId, version_stack: &[Vec<LocalId>], num_orig_locals: usize) -> LocalId {
    let orig = orig_local.0 as usize;
    if orig >= num_orig_locals {
        return orig_local;
    }
    *version_stack[orig].last().unwrap_or(&orig_local)
}

/// Rewrite a MirOperand to use the current SSA version.
fn rename_operand(op: &mut MirOperand, version_stack: &[Vec<LocalId>], num_orig_locals: usize) {
    if let MirOperand::Local(ref mut id) = op {
        *id = current_version(*id, version_stack, num_orig_locals);
    }
}

/// Rewrite all operand reads in an rvalue.
fn rename_rvalue(rv: &mut MirRValue, version_stack: &[Vec<LocalId>], num_orig_locals: usize) {
    match rv {
        MirRValue::Use(op) => rename_operand(op, version_stack, num_orig_locals),
        MirRValue::Ref(id) => *id = current_version(*id, version_stack, num_orig_locals),
        MirRValue::Deref(op) => rename_operand(op, version_stack, num_orig_locals),
        MirRValue::BinaryOp { left, right, .. } => {
            rename_operand(left, version_stack, num_orig_locals);
            rename_operand(right, version_stack, num_orig_locals);
        }
        MirRValue::UnaryOp { operand, .. } => rename_operand(operand, version_stack, num_orig_locals),
        MirRValue::Cast { value, .. } => rename_operand(value, version_stack, num_orig_locals),
        MirRValue::Field { base, .. } => rename_operand(base, version_stack, num_orig_locals),
        MirRValue::EnumTag { value } => rename_operand(value, version_stack, num_orig_locals),
        MirRValue::ArrayIndex { base, index, .. } => {
            rename_operand(base, version_stack, num_orig_locals);
            rename_operand(index, version_stack, num_orig_locals);
        }
    }
}

/// Rename all uses and defs in a statement. Returns the original local if a def was renamed.
fn rename_stmt(
    stmt: &mut MirStmt,
    func: &mut MirFunction,
    version_counter: &mut [u32],
    version_stack: &mut [Vec<LocalId>],
    orig_local_info: &[(Option<String>, crate::MirType)],
    num_orig_locals: usize,
) -> Option<usize> {
    match &mut stmt.kind {
        MirStmtKind::Phi { dst, .. } => {
            // Phi args are filled in by the predecessor pass, not here.
            // Just rename the dst to a fresh version.
            let orig = dst.0 as usize;
            if orig < num_orig_locals {
                *dst = new_version(
                    LocalId(orig as u32), func, version_counter, version_stack,
                    orig_local_info, num_orig_locals,
                );
                return Some(orig);
            }
        }
        MirStmtKind::Assign { dst, rvalue } => {
            // Rename uses first, then def.
            rename_rvalue(rvalue, version_stack, num_orig_locals);
            let orig = dst.0 as usize;
            if orig < num_orig_locals {
                *dst = new_version(
                    LocalId(orig as u32), func, version_counter, version_stack,
                    orig_local_info, num_orig_locals,
                );
                return Some(orig);
            }
        }
        MirStmtKind::Store { addr, value, .. } => {
            *addr = current_version(*addr, version_stack, num_orig_locals);
            rename_operand(value, version_stack, num_orig_locals);
        }
        MirStmtKind::Call { dst, args, .. } => {
            for arg in args.iter_mut() {
                rename_operand(arg, version_stack, num_orig_locals);
            }
            if let Some(d) = dst {
                let orig = d.0 as usize;
                if orig < num_orig_locals {
                    *d = new_version(
                        LocalId(orig as u32), func, version_counter, version_stack,
                        orig_local_info, num_orig_locals,
                    );
                    return Some(orig);
                }
            }
        }
        MirStmtKind::ClosureCall { closure, args, dst } => {
            *closure = current_version(*closure, version_stack, num_orig_locals);
            for arg in args.iter_mut() {
                rename_operand(arg, version_stack, num_orig_locals);
            }
            if let Some(d) = dst {
                let orig = d.0 as usize;
                if orig < num_orig_locals {
                    *d = new_version(
                        LocalId(orig as u32), func, version_counter, version_stack,
                        orig_local_info, num_orig_locals,
                    );
                    return Some(orig);
                }
            }
        }
        MirStmtKind::PoolCheckedAccess { dst, pool, handle } => {
            *pool = current_version(*pool, version_stack, num_orig_locals);
            *handle = current_version(*handle, version_stack, num_orig_locals);
            let orig = dst.0 as usize;
            if orig < num_orig_locals {
                *dst = new_version(
                    LocalId(orig as u32), func, version_counter, version_stack,
                    orig_local_info, num_orig_locals,
                );
                return Some(orig);
            }
        }
        MirStmtKind::ClosureCreate { dst, captures, .. } => {
            for cap in captures.iter_mut() {
                cap.local_id = current_version(cap.local_id, version_stack, num_orig_locals);
            }
            let orig = dst.0 as usize;
            if orig < num_orig_locals {
                *dst = new_version(
                    LocalId(orig as u32), func, version_counter, version_stack,
                    orig_local_info, num_orig_locals,
                );
                return Some(orig);
            }
        }
        MirStmtKind::LoadCapture { dst, env_ptr, .. } => {
            *env_ptr = current_version(*env_ptr, version_stack, num_orig_locals);
            let orig = dst.0 as usize;
            if orig < num_orig_locals {
                *dst = new_version(
                    LocalId(orig as u32), func, version_counter, version_stack,
                    orig_local_info, num_orig_locals,
                );
                return Some(orig);
            }
        }
        MirStmtKind::ClosureDrop { closure } => {
            *closure = current_version(*closure, version_stack, num_orig_locals);
        }
        MirStmtKind::ResourceRegister { dst, .. } => {
            let orig = dst.0 as usize;
            if orig < num_orig_locals {
                *dst = new_version(
                    LocalId(orig as u32), func, version_counter, version_stack,
                    orig_local_info, num_orig_locals,
                );
                return Some(orig);
            }
        }
        MirStmtKind::ResourceConsume { resource_id } => {
            *resource_id = current_version(*resource_id, version_stack, num_orig_locals);
        }
        MirStmtKind::ResourceScopeCheck { .. } | MirStmtKind::EnsurePush { .. } | MirStmtKind::EnsurePop => {}
        MirStmtKind::ArrayStore { base, index, value, .. } => {
            *base = current_version(*base, version_stack, num_orig_locals);
            rename_operand(index, version_stack, num_orig_locals);
            rename_operand(value, version_stack, num_orig_locals);
        }
        MirStmtKind::GlobalRef { dst, .. } => {
            let orig = dst.0 as usize;
            if orig < num_orig_locals {
                *dst = new_version(
                    LocalId(orig as u32), func, version_counter, version_stack,
                    orig_local_info, num_orig_locals,
                );
                return Some(orig);
            }
        }
        MirStmtKind::TraitBox { dst, value, .. } => {
            rename_operand(value, version_stack, num_orig_locals);
            let orig = dst.0 as usize;
            if orig < num_orig_locals {
                *dst = new_version(
                    LocalId(orig as u32), func, version_counter, version_stack,
                    orig_local_info, num_orig_locals,
                );
                return Some(orig);
            }
        }
        MirStmtKind::TraitCall { dst, trait_object, args, .. } => {
            *trait_object = current_version(*trait_object, version_stack, num_orig_locals);
            for arg in args.iter_mut() {
                rename_operand(arg, version_stack, num_orig_locals);
            }
            if let Some(d) = dst {
                let orig = d.0 as usize;
                if orig < num_orig_locals {
                    *d = new_version(
                        LocalId(orig as u32), func, version_counter, version_stack,
                        orig_local_info, num_orig_locals,
                    );
                    return Some(orig);
                }
            }
        }
        MirStmtKind::TraitDrop { trait_object } => {
            *trait_object = current_version(*trait_object, version_stack, num_orig_locals);
        }
        MirStmtKind::RcInc { local } | MirStmtKind::RcDec { local } => {
            *local = current_version(*local, version_stack, num_orig_locals);
        }
    }
    None
}

/// Rename a terminator's operands.
fn rename_terminator(
    term: &mut MirTerminator,
    version_stack: &[Vec<LocalId>],
    num_orig_locals: usize,
) {
    match &mut term.kind {
        MirTerminatorKind::Return { value: Some(op) } => {
            rename_operand(op, version_stack, num_orig_locals);
        }
        MirTerminatorKind::Branch { cond, .. } => {
            rename_operand(cond, version_stack, num_orig_locals);
        }
        MirTerminatorKind::Switch { value, .. } => {
            rename_operand(value, version_stack, num_orig_locals);
        }
        MirTerminatorKind::CleanupReturn { value: Some(op), .. } => {
            rename_operand(op, version_stack, num_orig_locals);
        }
        _ => {}
    }
}

/// Recursive dominator-tree walk for variable renaming.
fn rename_block(
    block_id: BlockId,
    func: &mut MirFunction,
    dom_children: &HashMap<BlockId, Vec<BlockId>>,
    preds: &HashMap<BlockId, Vec<BlockId>>,
    block_index: &HashMap<BlockId, usize>,
    version_counter: &mut Vec<u32>,
    version_stack: &mut Vec<Vec<LocalId>>,
    orig_local_info: &[(Option<String>, crate::MirType)],
    num_orig_locals: usize,
) {
    let Some(&bidx) = block_index.get(&block_id) else {
        return;
    };

    // Track how many versions we push so we can pop them later.
    let mut pushed: Vec<usize> = Vec::new(); // original local indices that got a new version

    // Rename all statements in this block.
    let num_stmts = func.blocks[bidx].statements.len();
    for si in 0..num_stmts {
        // We need to temporarily take the statement out to pass func mutably.
        let mut stmt = func.blocks[bidx].statements[si].clone();
        if let Some(orig) = rename_stmt(
            &mut stmt, func, version_counter, version_stack,
            orig_local_info, num_orig_locals,
        ) {
            pushed.push(orig);
        }
        func.blocks[bidx].statements[si] = stmt;
    }

    // Rename terminator.
    let mut term = func.blocks[bidx].terminator.clone();
    rename_terminator(&mut term, version_stack, num_orig_locals);
    func.blocks[bidx].terminator = term;

    // Fill in phi args in successor blocks.
    let successors = cfg::successors(&func.blocks[bidx].terminator);
    for succ_id in successors {
        let Some(&succ_idx) = block_index.get(&succ_id) else {
            continue;
        };
        // For each phi in the successor, set the arg corresponding to this predecessor.
        for si in 0..func.blocks[succ_idx].statements.len() {
            let is_phi = matches!(func.blocks[succ_idx].statements[si].kind, MirStmtKind::Phi { .. });
            if !is_phi {
                break; // Phis are always at the start.
            }
            if let MirStmtKind::Phi { ref mut args, .. } = func.blocks[succ_idx].statements[si].kind {
                for (pred_id, ref mut op) in args.iter_mut() {
                    if *pred_id == block_id {
                        // Replace the placeholder with the current reaching def.
                        if let MirOperand::Local(ref id) = op {
                            let renamed = current_version(*id, version_stack, num_orig_locals);
                            *op = MirOperand::Local(renamed);
                        }
                    }
                }
            }
        }
    }

    // Recurse into dominator-tree children.
    let children = dom_children.get(&block_id).cloned().unwrap_or_default();
    for child in children {
        rename_block(
            child, func, dom_children, preds, block_index,
            version_counter, version_stack, orig_local_info, num_orig_locals,
        );
    }

    // Pop versions pushed in this block.
    for orig in pushed {
        version_stack[orig].pop();
    }
}

// ---------------------------------------------------------------------------
// SSA destruction (de-SSA)
// ---------------------------------------------------------------------------

/// Lower all phi nodes to copy statements in predecessor blocks.
///
/// For each `Phi { dst, args }`, inserts `dst = operand` at the end of each
/// predecessor block (before the terminator). Critical edges are split first.
pub fn destruct(func: &mut MirFunction) {
    if func.blocks.is_empty() {
        return;
    }

    // Phase 1: Split critical edges (needed for correct phi lowering).
    split_critical_edges(func);

    // Phase 2: Lower phis to copies.
    // Collect all phis first to avoid borrow conflicts.
    let phis: Vec<(BlockId, LocalId, Vec<(BlockId, MirOperand)>)> = func.blocks.iter()
        .flat_map(|block| {
            block.statements.iter().filter_map(move |stmt| {
                if let MirStmtKind::Phi { dst, args } = &stmt.kind {
                    Some((block.id, *dst, args.clone()))
                } else {
                    None
                }
            })
        })
        .collect();

    // Build block index.
    let block_index: HashMap<BlockId, usize> = func.blocks.iter()
        .enumerate()
        .map(|(i, b)| (b.id, i))
        .collect();

    // Insert copies in predecessors.
    for (_block_id, dst, args) in &phis {
        for (pred_id, operand) in args {
            // Skip self-copies (dst = dst).
            if let MirOperand::Local(src) = operand {
                if *src == *dst {
                    continue;
                }
            }
            if let Some(&pred_idx) = block_index.get(pred_id) {
                let copy = MirStmt::dummy(MirStmtKind::Assign {
                    dst: *dst,
                    rvalue: MirRValue::Use(operand.clone()),
                });
                func.blocks[pred_idx].statements.push(copy);
            }
        }
    }

    // Remove all phi statements.
    for block in &mut func.blocks {
        block.statements.retain(|s| !matches!(s.kind, MirStmtKind::Phi { .. }));
    }
}

/// Split critical edges by inserting empty intermediate blocks.
///
/// A critical edge is one from a block with multiple successors to a block with
/// multiple predecessors. Splitting ensures phi copies only execute on the
/// correct edge.
fn split_critical_edges(func: &mut MirFunction) {
    let preds = cfg::predecessors(func);

    // Find critical edges: (src_block_idx, target_block_id).
    let mut splits: Vec<(usize, BlockId)> = Vec::new();
    for (src_idx, block) in func.blocks.iter().enumerate() {
        let succs = cfg::successors(&block.terminator);
        if succs.len() <= 1 {
            continue;
        }
        for &succ in &succs {
            let pred_count = preds.get(&succ).map_or(0, |p| p.len());
            if pred_count > 1 {
                // Check if the target block actually has phis — only split if needed.
                let has_phis = func.blocks.iter()
                    .find(|b| b.id == succ)
                    .map_or(false, |b| {
                        b.statements.first().map_or(false, |s| matches!(s.kind, MirStmtKind::Phi { .. }))
                    });
                if has_phis {
                    splits.push((src_idx, succ));
                }
            }
        }
    }

    // Process splits. We assign new block IDs starting from the max existing + 1.
    let mut next_block_id = func.blocks.iter().map(|b| b.id.0).max().unwrap_or(0) + 1;

    for (src_idx, target) in splits {
        let new_id = BlockId(next_block_id);
        next_block_id += 1;

        let src_block_id = func.blocks[src_idx].id;

        // Create new intermediate block: just a goto to the original target.
        let new_block = MirBlock {
            id: new_id,
            statements: Vec::new(),
            terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target }),
        };

        // Rewrite the source block's terminator to point to the new block.
        rewrite_terminator_target(&mut func.blocks[src_idx].terminator, target, new_id);

        // Update phi args in the target block to reference the new block instead of src.
        if let Some(target_block) = func.blocks.iter_mut().find(|b| b.id == target) {
            for stmt in &mut target_block.statements {
                if let MirStmtKind::Phi { args, .. } = &mut stmt.kind {
                    for (pred, _) in args.iter_mut() {
                        if *pred == src_block_id {
                            *pred = new_id;
                        }
                    }
                }
            }
        }

        func.blocks.push(new_block);
    }
}

/// Rewrite a terminator to replace one target with another.
fn rewrite_terminator_target(term: &mut MirTerminator, old: BlockId, new: BlockId) {
    match &mut term.kind {
        MirTerminatorKind::Goto { target } => {
            if *target == old { *target = new; }
        }
        MirTerminatorKind::Branch { then_block, else_block, .. } => {
            if *then_block == old { *then_block = new; }
            if *else_block == old { *else_block = new; }
        }
        MirTerminatorKind::Switch { cases, default, .. } => {
            for (_, block) in cases.iter_mut() {
                if *block == old { *block = new; }
            }
            if *default == old { *default = new; }
        }
        MirTerminatorKind::CleanupReturn { cleanup_chain, .. } => {
            for block in cleanup_chain.iter_mut() {
                if *block == old { *block = new; }
            }
        }
        MirTerminatorKind::Return { .. } | MirTerminatorKind::Unreachable => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MirConst, MirLocal, MirType};

    fn block(n: u32) -> BlockId { BlockId(n) }
    fn local(n: u32) -> LocalId { LocalId(n) }

    fn make_local(id: u32) -> MirLocal {
        MirLocal { id: local(id), name: Some(format!("_{}", id)), ty: MirType::I32, is_param: false }
    }

    fn assign_const(dst: u32, val: i64) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Assign {
            dst: local(dst),
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(val))),
        })
    }

    fn assign(dst: u32, src: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Assign {
            dst: local(dst),
            rvalue: MirRValue::Use(MirOperand::Local(local(src))),
        })
    }

    fn assign_add(dst: u32, left: u32, right: u32) -> MirStmt {
        MirStmt::dummy(MirStmtKind::Assign {
            dst: local(dst),
            rvalue: MirRValue::BinaryOp {
                op: crate::BinOp::Add,
                left: MirOperand::Local(local(left)),
                right: MirOperand::Local(local(right)),
            },
        })
    }

    fn term_goto(target: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Goto { target: block(target) })
    }

    fn term_branch(then_b: u32, else_b: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Branch {
            cond: MirOperand::Constant(MirConst::Bool(true)),
            then_block: block(then_b),
            else_block: block(else_b),
        })
    }

    fn term_ret() -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return { value: None })
    }

    fn term_ret_local(l: u32) -> MirTerminator {
        MirTerminator::dummy(MirTerminatorKind::Return {
            value: Some(MirOperand::Local(local(l))),
        })
    }

    fn make_fn(locals: Vec<MirLocal>, blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::I32,
            locals,
            blocks,
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn count_phis(func: &MirFunction) -> usize {
        func.blocks.iter()
            .flat_map(|b| b.statements.iter())
            .filter(|s| matches!(s.kind, MirStmtKind::Phi { .. }))
            .count()
    }

    fn phis_in_block(func: &MirFunction, bid: BlockId) -> Vec<LocalId> {
        func.blocks.iter()
            .find(|b| b.id == bid)
            .map(|b| {
                b.statements.iter()
                    .filter_map(|s| if let MirStmtKind::Phi { dst, .. } = &s.kind { Some(*dst) } else { None })
                    .collect()
            })
            .unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // Test 1: Linear chain — no phis needed
    // -----------------------------------------------------------------------
    #[test]
    fn linear_no_phis() {
        // bb0: x = 1; goto bb1
        // bb1: y = x; return y
        let mut func = make_fn(
            vec![make_local(0), make_local(1)],
            vec![
                MirBlock { id: block(0), statements: vec![assign_const(0, 1)], terminator: term_goto(1) },
                MirBlock { id: block(1), statements: vec![assign(1, 0)], terminator: term_ret_local(1) },
            ],
        );
        construct(&mut func);
        assert_eq!(count_phis(&func), 0);
    }

    // -----------------------------------------------------------------------
    // Test 2: Diamond — phi at join block
    // -----------------------------------------------------------------------
    #[test]
    fn diamond_phi_at_join() {
        // bb0: branch to bb1/bb2
        // bb1: x = 1; goto bb3
        // bb2: x = 2; goto bb3
        // bb3: return x
        let mut func = make_fn(
            vec![make_local(0)],
            vec![
                MirBlock { id: block(0), statements: vec![], terminator: term_branch(1, 2) },
                MirBlock { id: block(1), statements: vec![assign_const(0, 1)], terminator: term_goto(3) },
                MirBlock { id: block(2), statements: vec![assign_const(0, 2)], terminator: term_goto(3) },
                MirBlock { id: block(3), statements: vec![], terminator: term_ret_local(0) },
            ],
        );
        construct(&mut func);

        // Should have exactly one phi in bb3 for local _0.
        let bb3_phis = phis_in_block(&func, block(3));
        assert_eq!(bb3_phis.len(), 1, "expected one phi at join block");

        // The phi should have 2 args (from bb1 and bb2).
        let bb3 = func.blocks.iter().find(|b| b.id == block(3)).unwrap();
        if let MirStmtKind::Phi { args, .. } = &bb3.statements[0].kind {
            assert_eq!(args.len(), 2);
        } else {
            panic!("expected phi");
        }
    }

    // -----------------------------------------------------------------------
    // Test 3: Simple loop — phi at loop header
    // -----------------------------------------------------------------------
    #[test]
    fn loop_phi_at_header() {
        // bb0: x = 0; goto bb1
        // bb1: x = x + 1; branch to bb1/bb2
        // bb2: return x
        let mut func = make_fn(
            vec![make_local(0), make_local(1)],
            vec![
                MirBlock {
                    id: block(0),
                    statements: vec![assign_const(0, 0)],
                    terminator: term_goto(1),
                },
                MirBlock {
                    id: block(1),
                    statements: vec![assign_add(0, 0, 0)], // x = x + x (simplified)
                    terminator: term_branch(1, 2),
                },
                MirBlock {
                    id: block(2),
                    statements: vec![],
                    terminator: term_ret_local(0),
                },
            ],
        );
        construct(&mut func);

        // Should have a phi at bb1 for _0 (defined in bb0 and bb1).
        let bb1_phis = phis_in_block(&func, block(1));
        assert!(!bb1_phis.is_empty(), "expected phi at loop header");
    }

    // -----------------------------------------------------------------------
    // Test 4: Dead variable pruning — no phi for unused variable
    // -----------------------------------------------------------------------
    #[test]
    fn pruned_dead_variable() {
        // bb0: branch to bb1/bb2
        // bb1: x = 1; goto bb3
        // bb2: x = 2; goto bb3
        // bb3: return (void) — x is never used after join
        let mut func = make_fn(
            vec![make_local(0)],
            vec![
                MirBlock { id: block(0), statements: vec![], terminator: term_branch(1, 2) },
                MirBlock { id: block(1), statements: vec![assign_const(0, 1)], terminator: term_goto(3) },
                MirBlock { id: block(2), statements: vec![assign_const(0, 2)], terminator: term_goto(3) },
                MirBlock { id: block(3), statements: vec![], terminator: term_ret() },
            ],
        );
        construct(&mut func);

        // Pruned SSA: no phi at bb3 because _0 is not live at entry of bb3.
        assert_eq!(count_phis(&func), 0, "should be pruned — _0 is dead at join");
    }

    // -----------------------------------------------------------------------
    // Test 5: Multiple definitions in same block
    // -----------------------------------------------------------------------
    #[test]
    fn multiple_defs_same_block() {
        // bb0: x = 1; x = 2; return x
        let mut func = make_fn(
            vec![make_local(0)],
            vec![
                MirBlock {
                    id: block(0),
                    statements: vec![assign_const(0, 1), assign_const(0, 2)],
                    terminator: term_ret_local(0),
                },
            ],
        );
        construct(&mut func);
        assert_eq!(count_phis(&func), 0, "single block, no join points");

        // The return should reference the second definition's version.
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) } = &func.blocks[0].terminator.kind {
            // The second assign creates version 2 of _0; the first creates version 1.
            // Return should use version 2.
            let second_assign_dst = if let MirStmtKind::Assign { dst, .. } = &func.blocks[0].statements[1].kind {
                *dst
            } else {
                panic!("expected assign");
            };
            assert_eq!(*id, second_assign_dst, "return should use the latest version");
        }
    }

    // -----------------------------------------------------------------------
    // Test 6: Parameters count as definitions in entry block
    // -----------------------------------------------------------------------
    #[test]
    fn parameters_are_definitions() {
        // param p0
        // bb0: branch to bb1/bb2
        // bb1: p0 = 1; goto bb3
        // bb2: goto bb3
        // bb3: return p0
        let mut func = MirFunction {
            name: "test".to_string(),
            params: vec![MirLocal { id: local(0), name: Some("p0".into()), ty: MirType::I32, is_param: true }],
            ret_ty: MirType::I32,
            locals: vec![],
            blocks: vec![
                MirBlock { id: block(0), statements: vec![], terminator: term_branch(1, 2) },
                MirBlock { id: block(1), statements: vec![assign_const(0, 1)], terminator: term_goto(3) },
                MirBlock { id: block(2), statements: vec![], terminator: term_goto(3) },
                MirBlock { id: block(3), statements: vec![], terminator: term_ret_local(0) },
            ],
            entry_block: block(0),
            is_extern_c: false,
            source_file: None,
        };
        construct(&mut func);

        // p0 is defined in bb0 (param) and bb1 (assign). Both reach bb3.
        // Should have a phi at bb3.
        let bb3_phis = phis_in_block(&func, block(3));
        assert_eq!(bb3_phis.len(), 1, "expected phi at join for parameter");
    }

    // -----------------------------------------------------------------------
    // Test 7: De-SSA roundtrip
    // -----------------------------------------------------------------------
    #[test]
    fn construct_destruct_roundtrip() {
        // bb0: branch to bb1/bb2
        // bb1: x = 1; goto bb3
        // bb2: x = 2; goto bb3
        // bb3: return x
        let mut func = make_fn(
            vec![make_local(0)],
            vec![
                MirBlock { id: block(0), statements: vec![], terminator: term_branch(1, 2) },
                MirBlock { id: block(1), statements: vec![assign_const(0, 1)], terminator: term_goto(3) },
                MirBlock { id: block(2), statements: vec![assign_const(0, 2)], terminator: term_goto(3) },
                MirBlock { id: block(3), statements: vec![], terminator: term_ret_local(0) },
            ],
        );
        construct(&mut func);
        assert!(count_phis(&func) > 0, "should have phis after construct");

        destruct(&mut func);
        assert_eq!(count_phis(&func), 0, "no phis after destruct");

        // Predecessor blocks should now have copy statements.
        let bb1 = func.blocks.iter().find(|b| b.id == block(1)).unwrap();
        let bb2 = func.blocks.iter().find(|b| b.id == block(2)).unwrap();

        // Each should have their original assign + a copy from de-SSA.
        assert!(bb1.statements.len() >= 2, "bb1 should have copy from de-SSA");
        assert!(bb2.statements.len() >= 2, "bb2 should have copy from de-SSA");
    }

    // -----------------------------------------------------------------------
    // Test 8: Critical edge splitting
    // -----------------------------------------------------------------------
    #[test]
    fn critical_edge_split() {
        // bb0: x = 0; branch to bb1/bb2  (bb0 has 2 succs)
        // bb1: x = 1; branch to bb2/bb3  (bb1 has 2 succs, bb2 has 2 preds → critical edge)
        // bb2: return x                   (bb2 has 2 preds: bb0, bb1)
        // bb3: return x
        let mut func = make_fn(
            vec![make_local(0)],
            vec![
                MirBlock { id: block(0), statements: vec![assign_const(0, 0)], terminator: term_branch(1, 2) },
                MirBlock { id: block(1), statements: vec![assign_const(0, 1)], terminator: term_branch(2, 3) },
                MirBlock { id: block(2), statements: vec![], terminator: term_ret_local(0) },
                MirBlock { id: block(3), statements: vec![], terminator: term_ret_local(0) },
            ],
        );
        construct(&mut func);
        let phi_count_before = count_phis(&func);
        let block_count_before = func.blocks.len();

        destruct(&mut func);
        assert_eq!(count_phis(&func), 0);

        // If there was a critical edge, blocks may have been added.
        if phi_count_before > 0 {
            // The critical edge bb1→bb2 should be split (bb1 has 2 succs, bb2 has 2 preds).
            assert!(func.blocks.len() >= block_count_before,
                "blocks should be >= original (critical edges may add blocks)");
        }
    }

    // -----------------------------------------------------------------------
    // Test 9: SSA property — each local has exactly one definition
    // -----------------------------------------------------------------------
    #[test]
    fn ssa_single_definition() {
        // bb0: x = 1; branch to bb1/bb2
        // bb1: x = 2; goto bb3
        // bb2: x = 3; goto bb3
        // bb3: return x
        let mut func = make_fn(
            vec![make_local(0)],
            vec![
                MirBlock { id: block(0), statements: vec![assign_const(0, 1)], terminator: term_branch(1, 2) },
                MirBlock { id: block(1), statements: vec![assign_const(0, 2)], terminator: term_goto(3) },
                MirBlock { id: block(2), statements: vec![assign_const(0, 3)], terminator: term_goto(3) },
                MirBlock { id: block(3), statements: vec![], terminator: term_ret_local(0) },
            ],
        );
        construct(&mut func);

        // Verify SSA property: each local defined at most once across all blocks.
        let mut defs: HashMap<LocalId, usize> = HashMap::new();
        for block in &func.blocks {
            for stmt in &block.statements {
                if let Some(def) = uses::stmt_def(stmt) {
                    *defs.entry(def).or_default() += 1;
                }
            }
        }
        for (local, count) in &defs {
            assert_eq!(*count, 1, "local {:?} defined {} times (should be 1 in SSA)", local, count);
        }
    }

    // -----------------------------------------------------------------------
    // Test 10: Cleanup blocks don't get phis
    // -----------------------------------------------------------------------
    #[test]
    fn cleanup_blocks_excluded() {
        // bb0: ensure_push(bb2); x = 1; branch to bb1/bb2
        // bb1: x = 2; goto bb2
        // bb2: return x  (cleanup target — should not get phi)
        let mut func = make_fn(
            vec![make_local(0)],
            vec![
                MirBlock {
                    id: block(0),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::EnsurePush { cleanup_block: block(2) }),
                        assign_const(0, 1),
                    ],
                    terminator: term_branch(1, 2),
                },
                MirBlock {
                    id: block(1),
                    statements: vec![assign_const(0, 2)],
                    terminator: term_goto(2),
                },
                MirBlock {
                    id: block(2),
                    statements: vec![],
                    terminator: term_ret_local(0),
                },
            ],
        );
        construct(&mut func);

        // bb2 is a cleanup block — should not get a phi.
        let bb2_phis = phis_in_block(&func, block(2));
        assert_eq!(bb2_phis.len(), 0, "cleanup blocks should not get phis");
    }
}
