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
//! so tracking follows the new name instead (every local typed as a trait
//! object is a drop candidate, not just `TraitBox` destinations; a moved-from
//! name is excluded so only the name still holding the value at its death
//! gets dropped). What's left (created, only ever read through `TraitCall`,
//! never hands off ownership) gets a `TraitDrop` before the function returns
//! and before each loop back-edge it's still alive at.
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
    // SSA renaming (loop variables especially) leaves the pre-rename local
    // declaration behind in `func.locals` even once nothing in the CFG
    // defines it anymore. Filtering by declared type alone picked those
    // stale entries up as "droppable", and inserting a `TraitDrop` for a
    // local nothing ever writes reads garbage — Cranelift's verifier caught
    // it as a block-argument mismatch. Requiring an actual definition site
    // keeps this to locals that are live in the CFG being examined.
    let types: HashMap<LocalId, &MirType> = func.locals.iter().map(|l| (l.id, &l.ty)).collect();
    let mut trait_locals: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let Some(id) = crate::analysis::uses::stmt_def(stmt) {
                if matches!(types.get(&id), Some(MirType::TraitObject { .. })) {
                    trait_locals.insert(id);
                }
            }
        }
    }
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
                    &mut drops_to_insert, block_idx, *target, &func.blocks, &defined_in_block,
                );
            }
            MirTerminatorKind::Branch { then_block, else_block, .. } => {
                collect_backedge_drops(
                    &mut drops_to_insert, block_idx, *then_block, &func.blocks, &defined_in_block,
                );
                collect_backedge_drops(
                    &mut drops_to_insert, block_idx, *else_block, &func.blocks, &defined_in_block,
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
    target: BlockId,
    blocks: &[MirBlock],
    defined_in_block: &HashMap<LocalId, usize>,
) {
    let Some(target_idx) = blocks.iter().position(|b| b.id == target) else { return };
    if target_idx > block_idx {
        return; // forward edge, not a loop back-edge
    }
    // Back-edge: drop trait objects defined between the target and here (the loop body).
    let to_drop: Vec<LocalId> = defined_in_block.iter()
        .filter(|(_, &cidx)| cidx >= target_idx && cidx <= block_idx)
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
}
