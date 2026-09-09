// SPDX-License-Identifier: (MIT OR Apache-2.0)
//
// Free what module-level consts hold, once the program is done with them.
//
// A `const` holding a container or a box was never freed. The drop pass places
// a release at the end of the scope that owns the value, and a module const has
// no such scope — its initializer runs before `main` and its lifetime is the
// process, so there was nowhere to put one (#1116).
//
// The frees go in a generated `rask_const_free`, which the runtime calls after
// `rask_main` returns *and after detached tasks have been awaited* — a detached
// task can still be reading a const while main is returning, so freeing at
// main's own exit would be too early.
//
// A panicking run doesn't reach it, deliberately: the process is going away, and
// walking half-built state during an abort buys nothing the OS isn't already
// doing. What freeing buys at all is the leak gate's signal — the OS reclaims
// the pages either way — and every file with a module-level container was on
// `known_leaks.txt` for this and nothing else, which hid the leaks that are real
// bugs.

use std::collections::HashMap;

use crate::function::{BlockId, LocalId, MirBlock, MirFunction, MirLocal};
use crate::lower::{const_slot_name, CONST_SLOT_PREFIX};
use crate::operand::{FunctionRef, MirOperand, MirRValue};
use crate::stmt::{MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind};
use crate::types::MirType;

/// The name the runtime calls. Declared in `runtime.c`, always emitted — an
/// empty body when the program has no const worth freeing, so the runtime has
/// one symbol to link against rather than a conditional one.
pub const CONST_FREE_FN: &str = "rask_const_free";

/// Add `rask_const_free` to the program.
pub fn add_const_free(fns: &mut Vec<MirFunction>) {
    if fns.iter().any(|f| f.name == CONST_FREE_FN) {
        return;
    }
    let mut owned: Vec<(String, &'static str, MirType)> = Vec::new();
    for func in fns.iter() {
        if let Some((slot, free_fn, ty)) = const_a_thunk_owns(func) {
            owned.push((slot, free_fn, ty));
        }
    }
    // Last declared, first freed — the order a scope exit uses. Thunks come out
    // in declaration order, so reversing them is that.
    owned.reverse();
    fns.push(build(&owned));
}

/// The const this init thunk builds, if the thing it built is ours to free.
///
/// Reads the thunk rather than the declaration: which release a value wants is
/// decided by the constructor that made it (`elem_strs::CTORS`), and the
/// declaration only says what type it is. `const S: Shared<i64> = Shared.local(0)`
/// and `Shared.new(0)` are one declared type over two runtime objects with two
/// different releases.
fn const_a_thunk_owns(func: &MirFunction) -> Option<(String, &'static str, MirType)> {
    if !func.name.starts_with("__rask_const_init__") {
        return None;
    }

    // Which locals hold the address of which slot, and which hold the result of
    // which call.
    let mut slot_of: HashMap<LocalId, String> = HashMap::new();
    let mut made_by: HashMap<LocalId, &str> = HashMap::new();
    let mut copied_from: HashMap<LocalId, LocalId> = HashMap::new();
    let mut stored: Option<(String, LocalId)> = None;

    for block in &func.blocks {
        for stmt in &block.statements {
            match &stmt.kind {
                MirStmtKind::GlobalRef { dst, name } => {
                    if let Some(rest) = name.strip_prefix(CONST_SLOT_PREFIX) {
                        slot_of.insert(*dst, rest.to_string());
                    }
                }
                MirStmtKind::Call { dst: Some(d), func: f, .. } => {
                    made_by.insert(*d, f.name.as_str());
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                    copied_from.insert(*dst, *src);
                }
                MirStmtKind::Store { addr, value: MirOperand::Local(v), .. } => {
                    if let Some(slot) = slot_of.get(addr) {
                        stored = Some((slot.clone(), *v));
                    }
                }
                _ => {}
            }
        }
    }

    let (slot, mut value) = stored?;
    // Walk back through the copies to the call that produced it.
    for _ in 0..func.locals.len().max(1) {
        if made_by.contains_key(&value) {
            break;
        }
        match copied_from.get(&value) {
            Some(prev) => value = *prev,
            None => return None,
        }
    }
    let ctor = made_by.get(&value)?;
    let ty = func
        .locals
        .iter()
        .find(|l| l.id == value)
        .map(|l| l.ty.clone())
        .unwrap_or(MirType::I64);

    // A const whose value doesn't fit the slot is copied to the heap and the
    // slot holds that block — a `string` const is a 16-byte header, a struct
    // const its fields. Readers share the block rather than owning it, so
    // nothing freed it: `const LABEL: string = "n is {N}"` was one 16-byte
    // block, definitely lost, on every run.
    //
    // Only the block. What the block *points at* — a Vec inside a struct
    // const, say — is the "where did a value go after it crossed into an
    // aggregate" question (#1035), and freeing the header doesn't answer it or
    // make it worse.
    if *ctor == "rask_alloc" {
        return Some((slot, "rask_free", MirType::Ptr));
    }

    let free_fn = crate::elem_strs::free_fn(ctor)?;
    Some((slot, free_fn, ty))
}

fn build(owned: &[(String, &'static str, MirType)]) -> MirFunction {
    let mut locals: Vec<MirLocal> = Vec::new();
    let mut statements: Vec<MirStmt> = Vec::new();
    let mut next = 0u32;
    let mut fresh = |locals: &mut Vec<MirLocal>, ty: MirType| {
        let id = LocalId(next);
        next += 1;
        locals.push(MirLocal { id, name: None, ty, is_param: false, container: None });
        id
    };

    for (slot, free_fn, ty) in owned {
        let addr = fresh(&mut locals, MirType::Ptr);
        statements.push(MirStmt::dummy(MirStmtKind::GlobalRef {
            dst: addr,
            name: const_slot_name(slot),
        }));
        let value = fresh(&mut locals, ty.clone());
        statements.push(MirStmt::dummy(MirStmtKind::Assign {
            dst: value,
            rvalue: MirRValue::Deref(MirOperand::Local(addr)),
        }));
        statements.push(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal(free_fn.to_string()),
            args: vec![MirOperand::Local(value)],
        }));
    }

    MirFunction {
        name: CONST_FREE_FN.to_string(),
        params: Vec::new(),
        ret_ty: MirType::Void,
        locals,
        blocks: vec![MirBlock {
            id: BlockId(0),
            statements,
            terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
        }],
        entry_block: BlockId(0),
        is_extern_c: true,
        source_file: None,
    }
}
