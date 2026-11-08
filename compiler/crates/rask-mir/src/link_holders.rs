// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! A local struct holding links forgets its edges when its frame ends.
//!
//! A struct bound in a function (`let c = Cursor(at: b)`) registers each link
//! slot with the target's rack (`Link_register_struct`), keyed by the slot's
//! address on the stack. The frame then returns and the address is reused by
//! whatever runs next, but the record stayed on the target: a later `delete`
//! read a dead frame to ask whether the slot still named the node, and would
//! null a word that happened to (#1811).
//!
//! So every local that registers gets the matching forget before each return.
//! It is zeroed on entry first: a path that returns before the struct was built
//! then forgets a slot reading `none`, which records nothing.

use std::collections::BTreeSet;

use crate::{
    FunctionRef, LocalId, MirConst, MirFunction, MirOperand, MirStmt, MirStmtKind, MirTerminatorKind,
};

const REGISTER: &str = "Link_register_struct";
const FORGET: &str = "Link_forget_struct";

pub fn forget_local_links(functions: &mut [MirFunction]) {
    for func in functions {
        forget_in(func);
    }
}

fn forget_in(func: &mut MirFunction) {
    let params: BTreeSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    // A parameter's storage is the caller's (a `mutate` struct) or lives as
    // long as the caller's copy; the caller forgets its own.
    let holders: BTreeSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Call { func: f, args, .. } if f.name == REGISTER => match args.first() {
                Some(MirOperand::Local(id)) if !params.contains(id) => Some(*id),
                _ => None,
            },
            _ => None,
        })
        .collect();
    if holders.is_empty() {
        return;
    }

    // Plain stores rather than `ZeroAggregate`, which codegen leaves out
    // wherever no unwind glue reads the zeroes.
    let sizes: Vec<(LocalId, u32)> = holders
        .iter()
        .filter_map(|id| func.locals.iter().find(|l| l.id == *id).map(|l| (*id, l.ty.size())))
        .collect();
    let mut zeroes = Vec::new();
    for (local, size) in sizes {
        let mut offset = 0;
        while offset < size {
            let width = if size - offset >= 8 { 8 } else if size - offset >= 4 { 4 } else { 1 };
            zeroes.push(MirStmt::dummy(MirStmtKind::Store {
                addr: local,
                offset,
                value: MirOperand::Constant(MirConst::Int(0)),
                store_size: width,
                field: None,
            }));
            offset += width;
        }
    }
    let entry = func.entry_block;
    if let Some(block) = func.blocks.iter_mut().find(|b| b.id == entry) {
        block.statements.splice(0..0, zeroes);
    }

    for block in &mut func.blocks {
        if !matches!(
            block.terminator.kind,
            MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. }
        ) {
            continue;
        }
        for local in &holders {
            block.statements.push(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(FORGET.to_string()),
                args: vec![MirOperand::Local(*local)],
            }));
        }
    }
}
