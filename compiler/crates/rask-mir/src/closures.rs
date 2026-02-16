// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Closure optimization pass — escape analysis, ownership transfer, and drop insertion.
//!
//! Entry point: `optimize_all_closures(fns)`.
//!
//! Per function, using cross-function callee escape info:
//! 1. Identifies closure locals (destinations of ClosureCreate)
//! 2. Determines which closures escape — passed to unknown or escaping callees,
//!    stored to memory, or returned. Borrow-only callees (param doesn't escape)
//!    don't count as escaping → closure stays on the stack.
//! 3. Downgrades non-escaping closures to stack allocation (heap: false)
//! 4. Identifies transferred closures (escaping Call arg or Store, no local use)
//! 5. Inserts ClosureDrop before Return terminators for heap-allocated
//!    closures that aren't returned and weren't transferred

use std::collections::{HashMap, HashSet};

use crate::{LocalId, MirFunction, MirOperand, MirStmt, MirTerminator};

/// Optimize closures across all functions with cross-function analysis.
///
/// Builds a callee escape map: for each function, which parameters escape?
/// This lets the per-function pass distinguish borrow (callee only calls the
/// closure locally → stack-allocate) from ownership transfer (callee stores/
/// returns/forwards → heap-allocate, suppress caller drop).
///
/// Unknown callees (runtime functions, external) are assumed to take ownership.
pub fn optimize_all_closures(fns: &mut [MirFunction]) {
    let callee_escapes = build_callee_escape_map(fns);
    for func in fns.iter_mut() {
        optimize_closures(func, &callee_escapes);
    }
}

fn optimize_closures(
    func: &mut MirFunction,
    callee_escapes: &HashMap<String, Vec<bool>>,
) {
    // Step 1: Find all closure locals
    let mut closure_locals: HashMap<LocalId, bool> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmt::ClosureCreate { dst, heap, .. } = stmt {
                closure_locals.insert(*dst, *heap);
            }
        }
    }

    if closure_locals.is_empty() {
        return;
    }

    // Step 2: Determine which closures escape (using callee info for Call args)
    let escaping = find_escaping_closures(func, &closure_locals, callee_escapes);

    // Step 3: Downgrade non-escaping closures to stack allocation
    for block in &mut func.blocks {
        for stmt in &mut block.statements {
            if let MirStmt::ClosureCreate { dst, heap, .. } = stmt {
                if !escaping.contains(dst) {
                    *heap = false;
                }
            }
        }
    }

    // Step 4: Find closures whose ownership was transferred
    let transferred = find_transferred_closures(func, &closure_locals, callee_escapes);

    // Step 5: Insert ClosureDrop before returns for remaining heap closures
    let heap_closures: HashSet<LocalId> = closure_locals.keys()
        .filter(|id| escaping.contains(id))
        .filter(|id| !transferred.contains(id))
        .copied()
        .collect();

    if heap_closures.is_empty() {
        return;
    }

    insert_closure_drops(func, &heap_closures);
}

/// Build a map of callee name → per-parameter escape info.
///
/// For each function, checks whether each parameter escapes (appears in
/// Call args, Store, or Return within the function body). A non-escaping
/// parameter means the function only uses it locally (e.g., via ClosureCall).
fn build_callee_escape_map(fns: &[MirFunction]) -> HashMap<String, Vec<bool>> {
    let mut map = HashMap::new();
    for func in fns {
        let escapes: Vec<bool> = func.params.iter()
            .map(|p| param_escapes_from(func, p.id))
            .collect();
        map.insert(func.name.clone(), escapes);
    }
    map
}

/// Check if a parameter escapes from its function.
///
/// A parameter "escapes" if it appears in a Call arg, Store value, or Return.
/// If it only appears in ClosureCall position, the function merely borrows it.
fn param_escapes_from(func: &MirFunction, param_id: LocalId) -> bool {
    for block in &func.blocks {
        for stmt in &block.statements {
            match stmt {
                MirStmt::Call { args, .. } => {
                    if args.iter().any(|a| matches!(a, MirOperand::Local(id) if *id == param_id)) {
                        return true;
                    }
                }
                MirStmt::Store { value: MirOperand::Local(id), .. } if *id == param_id => {
                    return true;
                }
                _ => {}
            }
        }
        match &block.terminator {
            MirTerminator::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminator::CleanupReturn { value: Some(MirOperand::Local(id)), .. }
                if *id == param_id => return true,
            _ => {}
        }
    }
    false
}

/// Scan all blocks to find closure locals that escape.
///
/// A closure escapes if it appears in:
/// - A Return/CleanupReturn terminator as the return value
/// - A Call arg where the callee is unknown or the param escapes from the callee
/// - A Store statement as the stored value
///
/// A closure passed to a known callee whose corresponding parameter doesn't
/// escape is NOT escaping — the callee merely borrows it.
fn find_escaping_closures(
    func: &MirFunction,
    closure_locals: &HashMap<LocalId, bool>,
    callee_escapes: &HashMap<String, Vec<bool>>,
) -> HashSet<LocalId> {
    let mut escaping = HashSet::new();

    for block in &func.blocks {
        for stmt in &block.statements {
            match stmt {
                MirStmt::Call { func: callee, args, .. } => {
                    for (arg_idx, arg) in args.iter().enumerate() {
                        if let MirOperand::Local(id) = arg {
                            if closure_locals.contains_key(id) {
                                let is_borrow = callee_escapes.get(&callee.name)
                                    .and_then(|e| e.get(arg_idx))
                                    .map(|escapes| !escapes)
                                    .unwrap_or(false);

                                if !is_borrow {
                                    escaping.insert(*id);
                                }
                            }
                        }
                    }
                }
                MirStmt::Store { value: MirOperand::Local(id), .. } => {
                    if closure_locals.contains_key(id) {
                        escaping.insert(*id);
                    }
                }
                _ => {}
            }
        }

        match &block.terminator {
            MirTerminator::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminator::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                if closure_locals.contains_key(id) {
                    escaping.insert(*id);
                }
            }
            _ => {}
        }
    }

    escaping
}

/// Find closures whose ownership was transferred out of the function.
///
/// A closure is "transferred" if it's passed to a callee that forwards/stores
/// the parameter, or to an unknown callee (runtime function). Closures passed
/// to callees that only use the parameter locally (borrow) are NOT transferred.
///
/// Closures also used locally via ClosureCall are excluded — the caller still
/// needs them, so we assume borrow semantics and keep the drop.
fn find_transferred_closures(
    func: &MirFunction,
    closure_locals: &HashMap<LocalId, bool>,
    callee_escapes: &HashMap<String, Vec<bool>>,
) -> HashSet<LocalId> {
    let mut passed_or_stored = HashSet::new();
    let mut used_locally = HashSet::new();

    for block in &func.blocks {
        for stmt in &block.statements {
            match stmt {
                MirStmt::Call { func: callee, args, .. } => {
                    for (arg_idx, arg) in args.iter().enumerate() {
                        if let MirOperand::Local(id) = arg {
                            if closure_locals.contains_key(id) {
                                let is_borrow = callee_escapes.get(&callee.name)
                                    .and_then(|e| e.get(arg_idx))
                                    .map(|escapes| !escapes)
                                    .unwrap_or(false);

                                if !is_borrow {
                                    passed_or_stored.insert(*id);
                                }
                            }
                        }
                    }
                }
                MirStmt::Store { value: MirOperand::Local(id), .. } => {
                    if closure_locals.contains_key(id) {
                        passed_or_stored.insert(*id);
                    }
                }
                MirStmt::ClosureCall { closure, .. } => {
                    if closure_locals.contains_key(closure) {
                        used_locally.insert(*closure);
                    }
                }
                _ => {}
            }
        }
    }

    passed_or_stored.difference(&used_locally).copied().collect()
}

/// Insert ClosureDrop statements before Return terminators for heap-allocated
/// closures that aren't the return value on that path.
fn insert_closure_drops(func: &mut MirFunction, heap_closures: &HashSet<LocalId>) {
    let mut drops_to_insert: Vec<(usize, Vec<LocalId>)> = Vec::new();

    for (block_idx, block) in func.blocks.iter().enumerate() {
        let returned_local = match &block.terminator {
            MirTerminator::Return { value: Some(MirOperand::Local(id)) } => Some(*id),
            MirTerminator::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => Some(*id),
            MirTerminator::Return { .. } | MirTerminator::CleanupReturn { .. } => None,
            _ => continue,
        };

        let to_drop: Vec<LocalId> = heap_closures.iter()
            .filter(|id| Some(**id) != returned_local)
            .copied()
            .collect();

        if !to_drop.is_empty() {
            drops_to_insert.push((block_idx, to_drop));
        }
    }

    for (block_idx, locals) in drops_to_insert {
        for local_id in locals {
            func.blocks[block_idx].statements.push(MirStmt::ClosureDrop {
                closure: local_id,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BlockId, MirBlock, MirLocal, MirType};
    use crate::operand::FunctionRef;

    fn temp(id: u32, ty: MirType) -> MirLocal {
        MirLocal { id: LocalId(id), name: None, ty, is_param: false }
    }

    fn param(id: u32, ty: MirType) -> MirLocal {
        MirLocal { id: LocalId(id), name: None, ty, is_param: true }
    }

    fn block(id: u32, stmts: Vec<MirStmt>, term: MirTerminator) -> MirBlock {
        MirBlock { id: BlockId(id), statements: stmts, terminator: term }
    }

    fn ret(val: Option<MirOperand>) -> MirTerminator {
        MirTerminator::Return { value: val }
    }

    fn get_heap(func: &MirFunction) -> bool {
        func.blocks[0].statements.iter().find_map(|s| {
            if let MirStmt::ClosureCreate { heap, .. } = s { Some(*heap) } else { None }
        }).unwrap()
    }

    fn has_drop(func: &MirFunction) -> bool {
        func.blocks[0].statements.iter().any(|s| matches!(s, MirStmt::ClosureDrop { .. }))
    }

    #[test]
    fn local_only_closure_gets_stack() {
        // Closure used only in ClosureCall → stack, no drop
        let func = MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![temp(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    },
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    },
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
        };

        let mut fns = vec![func];
        optimize_all_closures(&mut fns);
        let func = &fns[0];

        assert!(!get_heap(func), "non-escaping closure should be stack-allocated");
        assert!(!has_drop(func), "stack closure should not have drop");
    }

    #[test]
    fn returned_closure_stays_heap() {
        let mut fns = vec![MirFunction {
            name: "make".to_string(),
            params: vec![],
            ret_ty: MirType::Ptr,
            locals: vec![temp(0, MirType::Ptr)],
            blocks: vec![
                block(0, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "make__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    },
                ], ret(Some(MirOperand::Local(LocalId(0))))),
            ],
            entry_block: BlockId(0),
        }];

        optimize_all_closures(&mut fns);

        assert!(get_heap(&fns[0]), "returned closure must stay heap");
        assert!(!has_drop(&fns[0]), "returned closure should not be dropped");
    }

    #[test]
    fn unknown_callee_assumes_transfer() {
        // Closure passed to spawn (not in fn set) → heap, no drop
        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![temp(0, MirType::Ptr)],
            blocks: vec![
                block(0, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    },
                    MirStmt::Call {
                        dst: None,
                        func: FunctionRef { name: "spawn".to_string() },
                        args: vec![MirOperand::Local(LocalId(0))],
                    },
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        }];

        optimize_all_closures(&mut fns);

        assert!(get_heap(&fns[0]), "closure to unknown callee must be heap");
        assert!(!has_drop(&fns[0]), "ownership transferred to unknown callee");
    }

    #[test]
    fn borrow_callee_gets_stack_and_no_drop() {
        // apply() only does ClosureCall on its param → borrow.
        // Closure doesn't escape, gets stack-allocated. No drop needed.
        let apply_fn = MirFunction {
            name: "apply".to_string(),
            params: vec![param(0, MirType::Ptr)],
            ret_ty: MirType::I64,
            locals: vec![param(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    },
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
        };

        let caller_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![temp(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    },
                    MirStmt::Call {
                        dst: Some(LocalId(1)),
                        func: FunctionRef { name: "apply".to_string() },
                        args: vec![MirOperand::Local(LocalId(0))],
                    },
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
        };

        let mut fns = vec![apply_fn, caller_fn];
        optimize_all_closures(&mut fns);
        let main = fns.iter().find(|f| f.name == "main").unwrap();

        assert!(!get_heap(main), "closure to borrow-only callee should be stack");
        assert!(!has_drop(main), "stack closure needs no drop");
    }

    #[test]
    fn escaping_callee_gets_heap_and_no_drop() {
        // store_it() stores the param → escapes. Heap, ownership transferred, no drop.
        let store_fn = MirFunction {
            name: "store_it".to_string(),
            params: vec![param(0, MirType::Ptr)],
            ret_ty: MirType::Void,
            locals: vec![param(0, MirType::Ptr), temp(1, MirType::Ptr)],
            blocks: vec![
                block(0, vec![
                    MirStmt::Store {
                        addr: LocalId(1),
                        offset: 0,
                        value: MirOperand::Local(LocalId(0)),
                    },
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let caller_fn = MirFunction {
            name: "main".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![temp(0, MirType::Ptr)],
            blocks: vec![
                block(0, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "main__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    },
                    MirStmt::Call {
                        dst: None,
                        func: FunctionRef { name: "store_it".to_string() },
                        args: vec![MirOperand::Local(LocalId(0))],
                    },
                ], ret(None)),
            ],
            entry_block: BlockId(0),
        };

        let mut fns = vec![store_fn, caller_fn];
        optimize_all_closures(&mut fns);
        let main = fns.iter().find(|f| f.name == "main").unwrap();

        assert!(get_heap(main), "closure to escaping callee must be heap");
        assert!(!has_drop(main), "ownership transferred — no drop");
    }

    #[test]
    fn unknown_callee_plus_local_use_gets_drop() {
        // Closure passed to unknown `run` AND used via ClosureCall.
        // Unknown → escaping → heap. Also used locally → not transferred. Drop inserted.
        let mut fns = vec![MirFunction {
            name: "f".to_string(),
            params: vec![],
            ret_ty: MirType::I64,
            locals: vec![temp(0, MirType::Ptr), temp(1, MirType::I64)],
            blocks: vec![
                block(0, vec![
                    MirStmt::ClosureCreate {
                        dst: LocalId(0),
                        func_name: "f__closure_0".to_string(),
                        captures: vec![],
                        heap: true,
                    },
                    MirStmt::Call {
                        dst: None,
                        func: FunctionRef { name: "run".to_string() },
                        args: vec![MirOperand::Local(LocalId(0))],
                    },
                    MirStmt::ClosureCall {
                        dst: Some(LocalId(1)),
                        closure: LocalId(0),
                        args: vec![],
                    },
                ], ret(Some(MirOperand::Local(LocalId(1))))),
            ],
            entry_block: BlockId(0),
        }];

        optimize_all_closures(&mut fns);

        assert!(get_heap(&fns[0]), "unknown callee forces heap");
        assert!(has_drop(&fns[0]), "local use prevents transfer — drop needed");
    }
}
