// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Escape analysis for string-typed locals.
//!
//! Determines which string locals may escape the function scope. A string
//! escapes when it's returned, stored in a collection passed out, captured
//! by an escaping closure, or sent cross-task. Non-escaping strings can
//! skip atomic refcount operations entirely (RE2).
//!
//! See `comp.string-refcount-elision` spec, "Escape Analysis" section.

use crate::{LocalId, MirFunction, MirOperand, MirStmtKind, MirTerminatorKind, MirType};
use std::collections::HashSet;

/// Functions that take ownership of a string argument (the string escapes).
const ESCAPE_FUNCTIONS: &[&str] = &[
    "rask_vec_push",
    "rask_vec_set",
    "rask_map_insert",
    "rask_map_insert_str",
    "rask_channel_send",
    "rask_shared_new",
    "rask_mutex_new",
];

/// Returns the set of string-typed locals that may escape the function.
///
/// A local escapes if:
/// - Returned from the function
/// - Passed to a function that takes ownership (collections, channels, shared)
/// - Captured by an escaping (heap) closure
/// - Stored via pointer (conservative: any Store of a string)
pub fn escaping_strings(func: &MirFunction) -> HashSet<LocalId> {
    let mut escaped = HashSet::new();

    let string_locals: HashSet<LocalId> = func.locals.iter()
        .chain(func.params.iter())
        .filter(|l| l.ty == MirType::String)
        .map(|l| l.id)
        .collect();

    if string_locals.is_empty() {
        return escaped;
    }

    for block in &func.blocks {
        // Check statements
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::Call { func: fref, args, .. } => {
                    let is_escape_fn = ESCAPE_FUNCTIONS.iter()
                        .any(|name| fref.name.contains(name));
                    if is_escape_fn {
                        for arg in args {
                            if let MirOperand::Local(id) = arg {
                                if string_locals.contains(id) {
                                    escaped.insert(*id);
                                }
                            }
                        }
                    }
                }
                // String stored via pointer — conservative escape
                MirStmtKind::Store { value: MirOperand::Local(id), .. } => {
                    if string_locals.contains(id) {
                        escaped.insert(*id);
                    }
                }
                // Captured by escaping closure
                MirStmtKind::ClosureCreate { captures, heap: true, .. } => {
                    for cap in captures {
                        if string_locals.contains(&cap.local_id) {
                            escaped.insert(cap.local_id);
                        }
                    }
                }
                // Passed to trait boxing (data escapes to heap)
                MirStmtKind::TraitBox { value: MirOperand::Local(id), .. } => {
                    if string_locals.contains(id) {
                        escaped.insert(*id);
                    }
                }
                // Passed via array store
                MirStmtKind::ArrayStore { value: MirOperand::Local(id), .. } => {
                    if string_locals.contains(id) {
                        escaped.insert(*id);
                    }
                }
                _ => {}
            }
        }

        // Check terminator — returned values escape
        match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) } => {
                if string_locals.contains(id) {
                    escaped.insert(*id);
                }
            }
            MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => {
                if string_locals.contains(id) {
                    escaped.insert(*id);
                }
            }
            _ => {}
        }
    }

    escaped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlockId, FunctionRef, MirBlock, MirLocal, MirOperand, MirRValue, MirStmt,
        MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
    };

    fn local(id: u32) -> LocalId { LocalId(id) }

    fn make_fn(locals: Vec<MirLocal>, blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals,
            blocks,
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    fn string_local(id: u32, name: &str) -> MirLocal {
        MirLocal { id: local(id), name: Some(name.into()), ty: MirType::String, is_param: false }
    }

    fn int_local(id: u32) -> MirLocal {
        MirLocal { id: local(id), name: None, ty: MirType::I64, is_param: false }
    }

    #[test]
    fn returned_string_escapes() {
        let f = make_fn(
            vec![string_local(0, "s")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(local(0))),
                }),
            }],
        );
        let esc = escaping_strings(&f);
        assert!(esc.contains(&local(0)));
    }

    #[test]
    fn local_only_string_does_not_escape() {
        let f = make_fn(
            vec![string_local(0, "s"), string_local(1, "t")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("print_string".to_string()),
                        args: vec![MirOperand::Local(local(1))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let esc = escaping_strings(&f);
        assert!(esc.is_empty());
    }

    #[test]
    fn string_pushed_to_vec_escapes() {
        let f = make_fn(
            vec![string_local(0, "s"), int_local(1)],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal("rask_vec_push".to_string()),
                    args: vec![MirOperand::Local(local(1)), MirOperand::Local(local(0))],
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let esc = escaping_strings(&f);
        assert!(esc.contains(&local(0)));
    }

    #[test]
    fn heap_closure_capture_escapes() {
        let f = make_fn(
            vec![string_local(0, "s"), int_local(1)],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::ClosureCreate {
                    dst: local(1),
                    func_name: "lambda".to_string(),
                    captures: vec![crate::ClosureCapture {
                        local_id: local(0),
                        offset: 8,
                        size: 16,
                    }],
                    heap: true,
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let esc = escaping_strings(&f);
        assert!(esc.contains(&local(0)));
    }

    #[test]
    fn stack_closure_capture_does_not_escape() {
        let f = make_fn(
            vec![string_local(0, "s"), int_local(1)],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::ClosureCreate {
                    dst: local(1),
                    func_name: "lambda".to_string(),
                    captures: vec![crate::ClosureCapture {
                        local_id: local(0),
                        offset: 8,
                        size: 16,
                    }],
                    heap: false,
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        let esc = escaping_strings(&f);
        assert!(esc.is_empty());
    }

    #[test]
    fn non_string_return_does_not_escape() {
        let f = make_fn(
            vec![int_local(0)],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(local(0))),
                }),
            }],
        );
        let esc = escaping_strings(&f);
        assert!(esc.is_empty());
    }
}
