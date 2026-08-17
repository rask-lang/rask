// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Trait-object drop insertion.
//!
//! `TraitBox` heap-allocates and copies a concrete value once to build an
//! `any Trait` fat pointer; `TraitCall` reads through it without consuming.
//! Nothing else in the pipeline ever dropped it (#366) — every trait object
//! leaked its heap allocation unconditionally.
//!
//! Mirrors `closures.rs`'s escape analysis, applied to trait-object-typed
//! locals instead of closure locals: a trait object escapes by being
//! returned, stored, or passed as a call/method argument — any of those hand
//! ownership to something else, so dropping the original name would
//! double-free. A plain move to another local (`dst = Use(src)`, or merging
//! through a `Phi`) is not an escape — it's the same value under a new name,
//! so tracking follows the new name instead (a moved-from name is excluded
//! so only the name still holding the value at its death gets dropped). What's
//! left (created, only ever read through `TraitCall`, never hands off
//! ownership) gets a `TraitDrop` before the function returns and before each
//! loop back-edge it's still alive at.
//!
//! Tracking starts from `TraitBox` destinations only — not every local typed
//! as a trait object. Reading one back out of existing storage (a struct
//! field, a `Vec` element, an `Option` payload) produces a local with the
//! same type but not fresh ownership: it's the same heap pointer the
//! container still holds, so a temp created by `r.inner.handle()` and
//! another by `run(r.inner)` right after are two aliases of the one box, not
//! two owners. Treating both as droppable-because-typed double-freed it
//! (`tests/suite/t62_trait_object_positions.rk`'s struct-field test). Only
//! `TraitBox`, and whatever a chain of moves/phis carries forward from it, is
//! a fresh allocation this pass may decide to free.
//!
//! Unlike the closure pass, this doesn't refine call-argument escapes with
//! per-callee borrow info — any appearance as a call/method argument counts
//! as escaping. That undercounts what could safely be dropped (a value
//! borrowed by a callee and not stored still "escapes" here), but never
//! drops something a callee kept, which is the risk worth avoiding.
//!
//! Function parameters are excluded — a `TraitDrop` needs the block where its
//! value was defined (for the loop back-edge check below), and a param's
//! "definition" is the call site, not a statement in this function's body.
//! A parameter that isn't returned/stored/passed onward still leaks; narrower
//! than the general case, and left for a follow-up.

use std::collections::{HashMap, HashSet};

use crate::{
    BlockId, LocalId, MirBlock, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind,
    MirTerminatorKind, MirType,
};

/// Insert `TraitDrop` for every non-escaping trait-object local, across all functions.
pub fn insert_trait_drops(fns: &mut [MirFunction]) {
    for func in fns.iter_mut() {
        insert_for_function(func);
    }
}

fn insert_for_function(func: &mut MirFunction) {
    let trait_locals = collect_fresh_trait_locals(func);
    if trait_locals.is_empty() {
        return;
    }

    let escaping = find_escaping(func, &trait_locals);
    let moved_away = find_moved_away(func, &trait_locals);

    let droppable: HashSet<LocalId> = trait_locals.iter()
        .filter(|id| !escaping.contains(id) && !moved_away.contains(id))
        .copied()
        .collect();
    if droppable.is_empty() {
        return;
    }

    insert_drops(func, &droppable);
}

/// Locals that hold a fresh trait-object allocation: `TraitBox` destinations,
/// plus anything a chain of plain moves or `Phi` merges carries forward from
/// one. A local typed as a trait object but reached only through a `Field`
/// read, a container access, or a call/method return is deliberately left
/// out — see the module doc for why aliasing one of those as "droppable"
/// double-frees.
fn collect_fresh_trait_locals(func: &MirFunction) -> HashSet<LocalId> {
    // SSA renaming (loop variables especially) can leave a pre-rename local
    // declaration behind in `func.locals` even once nothing in the CFG
    // defines it anymore, so start from actual definition sites, not the
    // declared list — a `TraitDrop` for a local nothing ever writes reads
    // garbage (Cranelift's verifier catches it as a block-argument mismatch).
    let mut fresh: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::TraitBox { dst, .. } = &stmt.kind {
                fresh.insert(*dst);
            }
        }
    }
    if fresh.is_empty() {
        return fresh;
    }

    // Propagate through moves and phi-merges to a fixed point: `_4 = _3`
    // (real lowering copies a `TraitBox` result into the source-named local
    // before first use) or a multi-hop chain both carry the same fresh
    // allocation to a new name.
    loop {
        let mut added = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                        if fresh.contains(src) && !fresh.contains(dst) =>
                    {
                        fresh.insert(*dst);
                        added = true;
                    }
                    MirStmtKind::Phi { dst, args } if !fresh.contains(dst) => {
                        if args.iter().any(|(_, op)| matches!(op, MirOperand::Local(id) if fresh.contains(id))) {
                            fresh.insert(*dst);
                            added = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        if !added {
            break;
        }
    }

    fresh
}

/// A trait object escapes if it's returned, stored, or passed as a call or
/// method argument. Being read through `TraitCall`'s receiver position is a
/// borrow, not an escape.
fn find_escaping(func: &MirFunction, trait_locals: &HashSet<LocalId>) -> HashSet<LocalId> {
    let mut escaping = HashSet::new();

    let mark_args = |args: &[MirOperand], escaping: &mut HashSet<LocalId>| {
        for arg in args {
            if let MirOperand::Local(id) = arg {
                if trait_locals.contains(id) {
                    escaping.insert(*id);
                }
            }
        }
    };

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Call { args, .. } | MirStmtKind::ClosureCall { args, .. } => {
                    mark_args(args, &mut escaping);
                }
                MirStmtKind::TraitCall { args, .. } => {
                    mark_args(args, &mut escaping);
                }
                MirStmtKind::Store { value: MirOperand::Local(id), .. }
                | MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. } => {
                    if trait_locals.contains(id) {
                        escaping.insert(*id);
                    }
                }
                _ => {}
            }
        }

        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                if trait_locals.contains(id) {
                    escaping.insert(*id);
                }
            }
            _ => {}
        }
    }

    escaping
}

/// A trait object is "moved away" when it's copied into a different local
/// (a plain move — the new name owns the value now) or merged through a
/// `Phi`. Either way, the old name's death isn't a drop point; whichever
/// name still holds the value when *it* dies is the one that gets dropped.
fn find_moved_away(func: &MirFunction, trait_locals: &HashSet<LocalId>) -> HashSet<LocalId> {
    let mut moved = HashSet::new();

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if trait_locals.contains(src) && src != dst =>
                {
                    moved.insert(*src);
                }
                MirStmtKind::Phi { args, .. } => {
                    for (_, op) in args {
                        if let MirOperand::Local(id) = op {
                            if trait_locals.contains(id) {
                                moved.insert(*id);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    moved
}

/// Insert `TraitDrop` before every return and every loop back-edge, for each
/// droppable trait object still alive at that point.
fn insert_drops(func: &mut MirFunction, droppable: &HashSet<LocalId>) {
    let dom = crate::analysis::dominators::DominatorTree::build(func);

    let mut defined_in_block: HashMap<LocalId, usize> = HashMap::new();
    for (idx, block) in func.blocks.iter().enumerate() {
        for stmt in &block.statements {
            if let Some(dst) = crate::analysis::uses::stmt_def(stmt) {
                if droppable.contains(&dst) {
                    defined_in_block.insert(dst, idx);
                }
            }
        }
    }

    let mut drops_to_insert: Vec<(usize, Vec<LocalId>)> = Vec::new();

    for (block_idx, block) in func.blocks.iter().enumerate() {
        match &block.terminator.kind {
            MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. } => {
                // A trait object created inside a loop (or either arm of a
                // branch) doesn't reach every return in the function — only
                // a return this local's definition actually dominates can
                // rely on it being live. Dropping it at a return it doesn't
                // dominate reads a local nothing wrote on that path, which
                // is exactly the stale-SSA-name crash this pass had before.
                let to_drop: Vec<LocalId> = droppable.iter()
                    .copied()
                    .filter(|id| {
                        defined_in_block.get(id)
                            .is_some_and(|&def_idx| dom.dominates(func.blocks[def_idx].id, block.id))
                    })
                    .collect();
                if !to_drop.is_empty() {
                    drops_to_insert.push((block_idx, to_drop));
                }
            }
            MirTerminatorKind::Goto { target } => {
                collect_backedge_drops(
                    &mut drops_to_insert, block_idx, block.id, *target, &func.blocks, &dom, &defined_in_block,
                );
            }
            MirTerminatorKind::Branch { then_block, else_block, .. } => {
                collect_backedge_drops(
                    &mut drops_to_insert, block_idx, block.id, *then_block, &func.blocks, &dom, &defined_in_block,
                );
                collect_backedge_drops(
                    &mut drops_to_insert, block_idx, block.id, *else_block, &func.blocks, &dom, &defined_in_block,
                );
            }
            _ => {}
        }
    }

    for (block_idx, locals) in drops_to_insert {
        for trait_object in locals {
            func.blocks[block_idx].statements.push(MirStmt::dummy(MirStmtKind::TraitDrop { trait_object }));
        }
    }
}

fn collect_backedge_drops(
    out: &mut Vec<(usize, Vec<LocalId>)>,
    block_idx: usize,
    source: BlockId,
    target: BlockId,
    blocks: &[MirBlock],
    dom: &crate::analysis::dominators::DominatorTree,
    defined_in_block: &HashMap<LocalId, usize>,
) {
    // A genuine loop back-edge is one whose target dominates its source —
    // every path to `source` passes through `target` first, i.e. `target`
    // is the loop header. Block *index* order isn't a safe proxy for this:
    // `assert`'s desugared success/failure blocks get allocated (and so
    // numbered) before the main computation that jumps to them, which
    // looked exactly like a back-edge under an index check and produced a
    // double-drop (drop at the "back-edge", drop again at the real return).
    if !dom.dominates(target, source) {
        return;
    }
    // Drop trait objects whose definition is inside the loop. Two conditions,
    // and the second was missing:
    //
    //   The header dominates the definition — so it isn't something created
    //   before the loop, which is still live after it.
    //
    //   The definition dominates the block that jumps back — so the value really
    //   is written on every iteration. The loop's *exit* block is dominated by
    //   the header too, so the first check alone claimed anything defined after
    //   the loop. `let c: any Shape = …` written after a `while` was dropped on
    //   every back-edge, freeing whatever the uninitialised slot held; the second
    //   iteration then double-freed and the process segfaulted at the `i = i + 1`
    //   line (#764's neighbour).
    //
    // A trait object created in only one arm of a branch inside the loop doesn't
    // dominate the back-edge and so leaks rather than being dropped on a path
    // that never wrote it — the same trade the return path takes.
    let to_drop: Vec<LocalId> = defined_in_block.iter()
        .filter(|(_, &def_idx)| {
            let def = blocks[def_idx].id;
            dom.dominates(target, def) && dom.dominates(def, source)
        })
        .map(|(&id, _)| id)
        .collect();
    if !to_drop.is_empty() {
        out.push((block_idx, to_drop));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MirLocal, MirTerminator};

    fn local(id: u32) -> LocalId { LocalId(id) }
    fn block_id(id: u32) -> BlockId { BlockId(id) }

    fn trait_local(id: u32) -> MirLocal {
        MirLocal { id: local(id), name: None, ty: MirType::TraitObject { trait_name: "Speaker".into() }, is_param: false }
    }

    fn make_fn(locals: Vec<MirLocal>, blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals,
            blocks,
            entry_block: block_id(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn has_trait_drop(stmts: &[MirStmt], target: LocalId) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, MirStmtKind::TraitDrop { trait_object } if *trait_object == target))
    }

    fn trait_box(dst: LocalId) -> MirStmt {
        MirStmt::dummy(MirStmtKind::TraitBox {
            dst,
            value: MirOperand::Local(local(99)),
            concrete_type: "Loud".into(),
            trait_name: "Speaker".into(),
            concrete_size: 32,
            vtable_name: ".vtable.Loud__Speaker".into(),
        })
    }

    fn trait_call(receiver: LocalId) -> MirStmt {
        MirStmt::dummy(MirStmtKind::TraitCall {
            dst: None,
            trait_object: receiver,
            method_name: "speak".into(),
            vtable_offset: 24,
            args: vec![],
        })
    }

    #[test]
    fn non_escaping_dropped_before_return() {
        let mut f = make_fn(
            vec![trait_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![trait_box(local(0)), trait_call(local(0))],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(has_trait_drop(&f.blocks[0].statements, local(0)));
    }

    /// Reproduces the shape real lowering emits for `let s: any Speaker = ...`:
    /// the `TraitBox` result gets copied into a second local before use.
    #[test]
    fn moved_through_copy_drops_the_final_name_not_the_original() {
        let mut f = make_fn(
            vec![trait_local(0), trait_local(1)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    trait_box(local(0)),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    trait_call(local(1)),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)), "moved-from name should not be dropped");
        assert!(has_trait_drop(&f.blocks[0].statements, local(1)), "the name actually holding the value should be dropped");
    }

    #[test]
    fn returned_trait_object_not_dropped() {
        let mut f = make_fn(
            vec![trait_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![trait_box(local(0))],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(local(0))),
                }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn stored_trait_object_not_dropped() {
        let mut f = make_fn(
            vec![trait_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    trait_box(local(0)),
                    MirStmt::dummy(MirStmtKind::Store {
                        addr: local(1),
                        offset: 0,
                        value: MirOperand::Local(local(0)),
                        store_size: None,
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn call_argument_not_dropped() {
        let mut f = make_fn(
            vec![trait_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    trait_box(local(0)),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: crate::FunctionRef::internal("consume".into()),
                        args: vec![MirOperand::Local(local(0))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn loop_body_dropped_at_back_edge() {
        // block 0: entry -> goto 1
        // block 1: loop header -> branch 2/3
        // block 2: body — TraitBox local 0 (moved into local 1), TraitCall, goto 1 (back-edge)
        // block 3: exit — return
        let mut f = make_fn(
            vec![trait_local(0), trait_local(1)],
            vec![
                MirBlock {
                    id: block_id(0),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: block_id(1) }),
                },
                MirBlock {
                    id: block_id(1),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(local(50)),
                        then_block: block_id(2),
                        else_block: block_id(3),
                    }),
                },
                MirBlock {
                    id: block_id(2),
                    statements: vec![
                        trait_box(local(0)),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(1),
                            rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                        }),
                        trait_call(local(1)),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: block_id(1) }),
                },
                MirBlock {
                    id: block_id(3),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
            ],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(has_trait_drop(&f.blocks[2].statements, local(1)), "back-edge block should drop the loop-local trait object");
        assert!(!has_trait_drop(&f.blocks[2].statements, local(0)), "moved-from name should not be dropped");
    }

    /// Reproduces the shape `assert`'s desugaring produces: the success and
    /// failure blocks are allocated (and so numbered) before the block that
    /// computes the condition and branches to them. A block-index check for
    /// "is this a back-edge" sees block 1 as jumped-to from a higher-numbered
    /// block 2 and mistakes it for a loop, inserting a second `TraitDrop` at
    /// block 2 on top of the one already correctly placed at block 1's
    /// return — a double free (#366 follow-up: this exact shape crashed
    /// `tests/suite/t11_traits.rk`'s "trait object dispatch" test in CI).
    #[test]
    fn assert_style_branch_to_lower_numbered_blocks_is_not_a_back_edge() {
        // block 0: entry — TraitBox local 0 (moved to local 1), goto 2
        // block 1: success — TraitDrop already placed here, return
        // (no block 1 predecessor other than block 2 — not a loop header)
        // block 2: TraitCall, branch to 1 (success) or 1 (success, for simplicity)
        let mut f = make_fn(
            vec![trait_local(0), trait_local(1)],
            vec![
                MirBlock {
                    id: block_id(0),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: block_id(2) }),
                },
                MirBlock {
                    id: block_id(1),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
                MirBlock {
                    id: block_id(2),
                    statements: vec![
                        trait_box(local(0)),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(1),
                            rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                        }),
                        trait_call(local(1)),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(local(50)),
                        then_block: block_id(1),
                        else_block: block_id(1),
                    }),
                },
            ],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(has_trait_drop(&f.blocks[1].statements, local(1)), "the return block should get the drop");
        assert!(!has_trait_drop(&f.blocks[2].statements, local(1)), "the branch is not a back-edge — no second drop here");
    }

    /// Reproduces `tests/suite/t62_trait_object_positions.rk`'s struct-field
    /// test: reading a trait object back out of a container (here, a struct
    /// field) twice produces two locals of the same type aliasing one heap
    /// box. Treating either as a fresh, droppable allocation — as a plain
    /// "is this local typed as a trait object" check would — drops the same
    /// pointer twice.
    #[test]
    fn field_read_trait_object_is_not_tracked_as_fresh() {
        let mut f = make_fn(
            vec![trait_local(0), trait_local(1)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(0),
                        rvalue: MirRValue::Field {
                            base: MirOperand::Local(local(2)),
                            field_index: 0,
                            byte_offset: Some(0),
                            access: crate::FieldAccess::Sized(16),
                        },
                    }),
                    trait_call(local(0)),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Field {
                            base: MirOperand::Local(local(2)),
                            field_index: 0,
                            byte_offset: Some(0),
                            access: crate::FieldAccess::Sized(16),
                        },
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: crate::FunctionRef::internal("run".into()),
                        args: vec![MirOperand::Local(local(1))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_trait_drops(std::slice::from_mut(&mut f));
        assert!(!has_trait_drop(&f.blocks[0].statements, local(0)), "a field read is a borrow, not a fresh allocation");
        assert!(!has_trait_drop(&f.blocks[0].statements, local(1)), "same here — this pass must not touch the struct's own field");
    }
}
