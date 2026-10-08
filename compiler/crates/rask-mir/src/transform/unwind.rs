// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Frame unwind records (ctrl.panic/U6).
//!
//! A panic leaves its frames by `longjmp`, straight past every release the
//! compiler put at their ends. The release passes arm a slot of the frame's
//! unwind record while a value is the frame's and disarm it when it stops
//! being (`analysis::ownership::place_unwind`); this pass, last, does the same
//! for strings and then builds what reads the record: `<fn>__unwind`, which
//! releases each slot still armed. Codegen pushes the record on entry, after
//! nothing, so the frame's own `ensure`s run first and still see what it owns.
//!
//! Strings aren't on the ownership plan. Each string local holds a reference
//! of its own from where it is written to its `rc_dec`, and `rc_elide` has the
//! last word on which of those survive, so they are read off the finished
//! MIR: a string is the frame's wherever one of its `rc_dec`s is still ahead.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::analysis::{cfg, uses};
use crate::{
    BlockId, LocalId, MirBlock, MirConst, MirFunction, MirLocal, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminator, MirTerminatorKind, MirType, UnwindRelease,
};

/// The suffix of the function that releases what a frame's record holds.
/// Codegen looks it up by name, the way it finds `__env_drop` glue.
pub const UNWIND_SUFFIX: &str = "__unwind";

/// Bytes ahead of the first slot: the record's link and its `run` pointer.
pub const RECORD_HEADER: u32 = 16;

/// Arm the strings, then build every function's unwind glue.
pub fn build_unwind_records(fns: &mut Vec<MirFunction>) {
    let mut glue = Vec::new();
    for func in fns.iter_mut() {
        arm_strings(func);
        drop_unreleasable_slots(func);
        if let Some(g) = unwind_glue(func) {
            glue.push(g);
        }
    }
    fns.extend(glue);
}

/// Arm each string local wherever an `rc_dec` of it is still ahead.
///
/// Backwards first: a string is pending where some path reaches an `rc_dec`
/// of it without writing it again. Then forwards: armed after each write it
/// is pending past, disarmed before each `rc_dec`, and disarmed on an edge
/// into a block where it isn't pending any more (it went to a keeper on that
/// side, or into a phi).
fn arm_strings(func: &mut MirFunction) {
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    let addr = crate::transform::addr_taken::analyze(func);
    let strings: BTreeSet<LocalId> = func
        .locals_of_type(&MirType::String)
        .into_iter()
        .filter(|l| !params.contains(l) && !addr.contains(*l))
        .collect();
    let released: BTreeSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|s| match &s.kind {
            MirStmtKind::RcDec { local } if strings.contains(local) => Some(*local),
            _ => None,
        })
        .collect();
    if released.is_empty() {
        return;
    }

    let index: HashMap<BlockId, usize> = func.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let succs: Vec<Vec<usize>> = func
        .blocks
        .iter()
        .map(|b| cfg::successors(&b.terminator).iter().filter_map(|s| index.get(s).copied()).collect())
        .collect();
    // Before statement `si` of a block, from what is pending after it.
    let step = |stmt: &MirStmt, after: &mut BTreeSet<LocalId>| {
        if let Some(d) = uses::stmt_def(stmt) {
            after.remove(&d);
        }
        if let MirStmtKind::RcDec { local } = &stmt.kind {
            if released.contains(local) {
                after.insert(*local);
            }
        }
    };
    let n = func.blocks.len();
    let mut pending_in: Vec<BTreeSet<LocalId>> = vec![BTreeSet::new(); n];
    let mut changed = true;
    while changed {
        changed = false;
        for bi in (0..n).rev() {
            let mut p: BTreeSet<LocalId> = BTreeSet::new();
            for &s in &succs[bi] {
                p.extend(pending_in[s].iter().copied());
            }
            for stmt in func.blocks[bi].statements.iter().rev() {
                step(stmt, &mut p);
            }
            if p != pending_in[bi] {
                pending_in[bi] = p;
                changed = true;
            }
        }
    }

    // Nothing is armed in an `ensure` body a cleanup return runs: the frame
    // released what it owned at the cleanup return.
    let cleanup = cfg::cleanup_only_blocks(func);
    let base = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|s| match &s.kind {
            MirStmtKind::UnwindArm { slot, .. } | MirStmtKind::UnwindDisarm { slot } => Some(*slot + 1),
            _ => None,
        })
        .max()
        .unwrap_or(0);
    let slot_of: HashMap<LocalId, u32> = released.iter().enumerate().map(|(i, l)| (*l, base + i as u32)).collect();
    let arm = |l: LocalId| MirStmt::dummy(MirStmtKind::UnwindArm {
        slot: slot_of[&l],
        value: l,
        release: UnwindRelease {
            placeholder: l,
            stmts: vec![MirStmt::dummy(MirStmtKind::RcDec { local: l })],
        },
    });
    let disarm = |l: LocalId| MirStmt::dummy(MirStmtKind::UnwindDisarm { slot: slot_of[&l] });

    let mut edges: Vec<(BlockId, BlockId, Vec<MirStmt>)> = Vec::new();
    for bi in 0..n {
        let id = func.blocks[bi].id;
        let mut out_pending: BTreeSet<LocalId> = BTreeSet::new();
        for &s in &succs[bi] {
            out_pending.extend(pending_in[s].iter().copied());
        }
        if !matches!(func.blocks[bi].terminator.kind, MirTerminatorKind::CleanupReturn { .. }) {
            for &s in &succs[bi] {
                let gone: Vec<MirStmt> =
                    out_pending.difference(&pending_in[s]).map(|l| disarm(*l)).collect();
                if !gone.is_empty() {
                    edges.push((id, func.blocks[s].id, gone));
                }
            }
        }
        // Pending after each statement, back to front.
        let stmts = &func.blocks[bi].statements;
        let mut after: Vec<BTreeSet<LocalId>> = vec![BTreeSet::new(); stmts.len()];
        let mut p = out_pending;
        for (si, stmt) in stmts.iter().enumerate().rev() {
            after[si] = p.clone();
            step(stmt, &mut p);
        }
        let phis = stmts.iter().take_while(|s| matches!(s.kind, MirStmtKind::Phi { .. })).count();
        let armable = !cleanup.contains(&id);
        let old = std::mem::take(&mut func.blocks[bi].statements);
        let mut out: Vec<MirStmt> = Vec::with_capacity(old.len());
        let mut phi_arms: Vec<MirStmt> = Vec::new();
        for (si, stmt) in old.into_iter().enumerate() {
            if let MirStmtKind::RcDec { local } = &stmt.kind {
                if released.contains(local) {
                    out.push(disarm(*local));
                }
            }
            let def = uses::stmt_def(&stmt).filter(|d| armable && released.contains(d) && after[si].contains(d));
            out.push(stmt);
            if let Some(d) = def {
                if si < phis {
                    phi_arms.push(arm(d));
                } else {
                    out.push(arm(d));
                }
            }
            if si + 1 == phis {
                out.append(&mut phi_arms);
            }
        }
        func.blocks[bi].statements = out;
    }
    crate::analysis::ownership::insert_on_edges(func, edges);
}

/// Locals a release reads that it doesn't write first, other than the value.
fn reads_outside(release: &UnwindRelease) -> bool {
    let mut defined: HashSet<LocalId> = HashSet::from([release.placeholder]);
    for stmt in &release.stmts {
        if uses::stmt_uses(stmt).iter().any(|l| !defined.contains(l)) {
            return true;
        }
        if let Some(d) = uses::stmt_def(stmt) {
            defined.insert(d);
        }
    }
    false
}

/// Take out every slot whose release needs more of the frame than its value:
/// a closure freed along with the environments it swallowed names each of
/// them. Left unarmed, a panic leaks what it holds rather than reading a local
/// the glue doesn't have.
fn drop_unreleasable_slots(func: &mut MirFunction) {
    let bad: HashSet<u32> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|s| match &s.kind {
            MirStmtKind::UnwindArm { slot, release, .. } if reads_outside(release) => Some(*slot),
            _ => None,
        })
        .collect();
    if bad.is_empty() {
        return;
    }
    for block in &mut func.blocks {
        block.statements.retain(|s| match &s.kind {
            MirStmtKind::UnwindArm { slot, .. } | MirStmtKind::UnwindDisarm { slot } => !bad.contains(slot),
            _ => true,
        });
    }
}

/// `<fn>__unwind(record)`: for each slot, if it holds something, release it
/// the way the frame would have. `None` for a frame that arms nothing, which
/// then gets no record at all.
fn unwind_glue(func: &MirFunction) -> Option<MirFunction> {
    let mut releases: BTreeMap<u32, &UnwindRelease> = BTreeMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::UnwindArm { slot, release, .. } = &stmt.kind {
            releases.entry(*slot).or_insert(release);
        }
    }
    if releases.is_empty() {
        return None;
    }
    let slots = releases.keys().max().map_or(0, |m| m + 1);
    let original: HashMap<LocalId, &MirLocal> = func.locals.iter().chain(func.params.iter()).map(|l| (l.id, l)).collect();

    let record = LocalId(0);
    let record_local =
        MirLocal { id: record, name: Some("record".to_string()), ty: MirType::Ptr, is_param: true, unerased: None };
    let mut locals: Vec<MirLocal> = vec![record_local.clone()];
    let mut fresh = |ty: MirType, unerased: Option<MirType>| -> LocalId {
        let id = LocalId(locals.len() as u32);
        locals.push(MirLocal { id, name: None, ty, is_param: false, unerased });
        id
    };

    let mut blocks: Vec<MirBlock> = Vec::new();
    let check = |i: u32| BlockId(2 * i);
    let body = |i: u32| BlockId(2 * i + 1);
    for i in 0..slots {
        let next = check(i + 1);
        let Some(release) = releases.get(&i) else {
            blocks.push(MirBlock {
                id: check(i),
                statements: Vec::new(),
                terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: next }),
            });
            continue;
        };
        let raw = fresh(MirType::I64, None);
        let armed = fresh(MirType::Bool, None);
        blocks.push(MirBlock {
            id: check(i),
            statements: vec![
                MirStmt::dummy(MirStmtKind::Assign {
                    dst: raw,
                    rvalue: MirRValue::ArrayIndex {
                        base: MirOperand::Local(record),
                        index: MirOperand::Constant(MirConst::Int((RECORD_HEADER / 8 + i) as i64)),
                        elem_size: 8,
                    },
                }),
                MirStmt::dummy(MirStmtKind::Assign {
                    dst: armed,
                    rvalue: MirRValue::BinaryOp {
                        op: crate::BinOp::Ne,
                        left: MirOperand::Local(raw),
                        right: MirOperand::Constant(MirConst::Int(0)),
                    },
                }),
            ],
            terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(armed),
                then_block: body(i),
                else_block: next,
            }),
        });

        // The value is one word whatever it is: a handle, a string pointer,
        // or the address of what the frame holds in place. A local of an
        // aggregate type would get a slot of its own and be copied into, so
        // it is a pointer here, with the full type kept for a walk that needs
        // it.
        let mut map: HashMap<LocalId, LocalId> = HashMap::new();
        let ph = original.get(&release.placeholder);
        let ph_ty = ph.map(|l| l.ty.clone()).unwrap_or(MirType::Ptr);
        let (ty, unerased) = match ph_ty {
            MirType::String => (MirType::String, None),
            other => (MirType::Ptr, ph.and_then(|l| l.unerased.clone()).or(Some(other))),
        };
        let value = fresh(ty, unerased);
        map.insert(release.placeholder, value);
        let mut stmts = vec![MirStmt::dummy(MirStmtKind::Assign {
            dst: value,
            rvalue: MirRValue::Use(MirOperand::Local(raw)),
        })];
        for stmt in &release.stmts {
            let mut stmt = stmt.clone();
            if let Some(d) = uses::stmt_def(&stmt) {
                let l = original.get(&d);
                let id = fresh(
                    l.map(|l| l.ty.clone()).unwrap_or(MirType::Ptr),
                    l.and_then(|l| l.unerased.clone()),
                );
                map.insert(d, id);
            }
            rename(&mut stmt, &map);
            stmts.push(stmt);
        }
        blocks.push(MirBlock {
            id: body(i),
            statements: stmts,
            terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: next }),
        });
    }
    blocks.push(MirBlock {
        id: check(slots),
        statements: Vec::new(),
        terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
    });

    let params = vec![record_local];
    Some(MirFunction {
        name: format!("{}{}", func.name, UNWIND_SUFFIX),
        params,
        ret_ty: MirType::Void,
        locals,
        blocks,
        entry_block: check(0),
        is_extern_c: false,
        source_file: func.source_file.clone(),
    })
}

/// Point a copied release statement at the glue's own locals.
fn rename(stmt: &mut MirStmt, map: &HashMap<LocalId, LocalId>) {
    uses::visit_stmt_use_locals_mut(stmt, &mut |id, _| {
        if let Some(n) = map.get(id) {
            *id = *n;
        }
    });
    let def = match &mut stmt.kind {
        MirStmtKind::Assign { dst, .. } => Some(dst),
        MirStmtKind::Call { dst: Some(d), .. } => Some(d),
        _ => None,
    };
    if let Some(d) = def {
        if let Some(n) = map.get(d) {
            *d = *n;
        }
    }
}
