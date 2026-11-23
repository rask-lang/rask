// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! String RC insertion — adds explicit `RcInc` and `RcDec` operations for
//! string-typed locals.
//!
//! Runs after SSA conversion. For each string-typed local:
//! - Insert `RcInc` after each copy (assignment from another string local)
//! - Insert `RcDec` at each last-use point (from liveness analysis)
//!
//! This makes refcount operations explicit in MIR so subsequent passes
//! (rc_elide) can analyze and eliminate them.
//!
//! See `comp.architecture/RC1-RC2` and `comp.string-refcount-elision`.

use std::collections::{HashMap, HashSet};
use rask_ast::ty::ParamMode;

use crate::analysis::addr_alias::AddrAliases;
use crate::analysis::cfg;
use crate::analysis::liveness;
use crate::analysis::ownership;
use crate::analysis::uses;
use crate::{
    MirBlock,
    BlockId, LocalId, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminatorKind, MirType,
};

/// Whether a call through a function value keeps argument `i` of `n`, asked
/// with the calling function's name and the local holding the value.
pub type ClosureKeeps<'a> = dyn Fn(&str, LocalId, usize, usize) -> bool + 'a;

/// Insert explicit RcInc/RcDec for all string-typed locals in a function.
///
/// `kept` says, per callee, which of its parameters it holds on to — see
/// `insert_aggregate_release`, which is the only part that needs it.
pub fn insert_rc_ops(
    func: &mut MirFunction,
    kept: &HashMap<String, Vec<bool>>,
    writes: &HashMap<String, HashSet<usize>>,
    own: &HashSet<String>,
    closure_keeps: &ClosureKeeps,
    rack_holders: &HashSet<u32>,
    copy_layouts: &crate::CopyLayouts,
    target: rask_ast::Target,
) {
    let string_locals: Vec<LocalId> = func.locals_of_type(&MirType::Buffer);

    // Aggregates lowering already gave references of their own: a Copy
    // element copied out of a collection (`retain_copy_out_of_collection`).
    // Collected before this pass adds retains of its own, which cover one
    // hand-off each rather than the name's whole life.
    //
    // Not a closure's name for a variable it reaches through its environment
    // (`load_capture_ref`, `load_capture_own`). The name is the variable's
    // address, so a retain after a store through it is the variable's
    // reference, and the variable's home releases it: the frame that built
    // the closure, or the environment's drop glue. Counted here, the closure
    // released it again when it returned, and `best = s` inside a loop over a
    // sequence kept a `string?` with no reference left to its string (#2011).
    let places: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::LoadCapture {
                dst,
                access: crate::CaptureAccess::Borrowed | crate::CaptureAccess::Owned,
                ..
            } => Some(*dst),
            _ => None,
        })
        .collect();
    let owns_copy: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::RcIncContents { local, for_hand_off: false } => Some(*local),
            _ => None,
        })
        .filter(|l| !places.contains(l))
        .collect();

    // The three string steps only have work when there is a string. The
    // aggregate walk does not: a struct holding a `Vec` and no string needs its
    // release just the same, and bailing out here meant whether that happened
    // depended on whether the function *happened* to mention a string —
    // `println("{h.items[0]}")` released the vector and
    // `assert h.items[0] == 7` leaked it, in bodies that are otherwise the same.
    if !string_locals.is_empty() {
        // Insert RcInc after string copies
        insert_rc_inc(func, &string_locals);

        // A string handed to a call that keeps it, and still used after,
        // gives the keeper a reference of its own.
        retain_locals_handed_over_while_live(func, &string_locals, own);

        // Insert RcDec at last-use points
        insert_rc_dec(func, &string_locals, own);

        // A returned parameter is handed out, not owned — take a reference for it.
        retain_returned_params(func, &string_locals, own);

        // So is one handed to a call that keeps it.
        retain_params_handed_over(func, &string_locals, own);

    }

    // A copy of an aggregate read out of somebody else's storage, given to a
    // keeper, takes references of its own to what it holds.
    let built_from_views = retain_views_handed_over(func, kept, writes, own, &owns_copy, closure_keeps, copy_layouts, target);

    // A join that might hand the frame's own value on gets a copy of it with
    // references of its own, so the join always owns what it holds.
    own_joins_of_kept_values(func, &owns_copy, copy_layouts);

    // And a Copy aggregate copied into a closure's environment.
    retain_copy_captures(func);

    // And the aggregates: a struct field or a wrapper's payload owns a string —
    // or a container — just as much as a local does.
    insert_aggregate_release(func, kept, writes, own, &owns_copy, &built_from_views, closure_keeps, rack_holders, copy_layouts);
}

/// The type of a closure's capture when it copies a Copy aggregate in.
///
/// The value stays the frame's (`mem.closures/CM2`): `let held = Line { … }`
/// captured by `spawn { println(held.text) }` and printed again afterwards, or
/// carried once per loop turn. So the environment's copy is a second holder of
/// the strings in it. A Copy aggregate holds no container (the checker's rule),
/// so a reference per string is all it needs, the same as a captured string.
fn copied_aggregate(func: &MirFunction, c: &crate::ClosureCapture) -> Option<MirType> {
    if c.by_ref || !c.copy {
        return None;
    }
    let l = func.locals.iter().chain(func.params.iter()).find(|l| l.id == c.local_id)?;
    let ty = l.ty.clone();
    aggregate_may_hold_string(&ty).then_some(ty)
}

/// Take references for each Copy aggregate a closure copies in. The
/// environment gives them back when it dies (its owned-entry list), and the
/// frame releases its own value wherever it would have anyway.
fn retain_copy_captures(func: &mut MirFunction) {
    for bi in 0..func.blocks.len() {
        let mut insertions: Vec<(usize, LocalId)> = Vec::new();
        for (si, stmt) in func.blocks[bi].statements.iter().enumerate() {
            let MirStmtKind::ClosureCreate { captures, .. } = &stmt.kind else { continue };
            for c in captures {
                if copied_aggregate(func, c).is_some() {
                    insertions.push((si, c.local_id));
                }
            }
        }
        for (idx, local) in insertions.into_iter().rev() {
            let span = func.blocks[bi].statements[idx].span;
            func.blocks[bi].statements.insert(idx, MirStmt::new(MirStmtKind::RcIncContents { local, for_hand_off: true }, span));
        }
    }
}

/// Give each way into a join of Copy aggregates that carries a value the frame
/// keeps its own copy of, so the join owns what it holds on every way in.
///
/// ```text
/// let p = Pt(s: mk(1))
/// for i in 0..4 {
///     let x = if i % 2 == 0 { p } else { Pt(s: mk(10 + i)) }
///     keep.push(x)
/// }
/// println(p.s)
/// ```
///
/// `x` is `p` on one way in and a fresh value on the other. The frame reads
/// `p` afterwards, so `x` can't be `p`'s own reference, and on the fresh way
/// it can't be released unless it is released on both: on the `p` way `x`
/// *is* `p`, and a second release of it frees what `p` still holds. The
/// release pass can't say which way it came and leaves `x` alone, which leaks
/// the fresh value on every other turn (#1904).
///
/// Retaining on the edge where `p` flows in makes `x` a new value on that way
/// too, with references of its own. Then it is the frame's to release or to
/// hand to a keeper on both, and nothing needs a per-path answer.
///
/// Only a Copy aggregate: that is what a keeper copies. A non-Copy value read
/// after it moved is the checker's error already.
fn own_joins_of_kept_values(
    func: &mut MirFunction,
    owns_copy: &HashSet<LocalId>,
    copy_layouts: &crate::CopyLayouts,
) {
    if !func.blocks.iter().flat_map(|b| b.statements.iter()).any(|s| matches!(s.kind, MirStmtKind::Phi { .. })) {
        return;
    }
    let ty_of: HashMap<LocalId, MirType> =
        func.locals.iter().chain(func.params.iter()).map(|l| (l.id, l.ty.clone())).collect();
    let copy_aggregate = |l: &LocalId| {
        ty_of.get(l).is_some_and(|t| aggregate_may_hold_string(t) && copy_layouts.is_copy(t))
    };
    let live = liveness::analyze_phis_on_edges(func);
    let aliases = AddrAliases::build(func);
    let mut source_of: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } = &stmt.kind {
            if dst != src {
                source_of.entry(*dst).or_default().push(*src);
            }
        }
    }

    // (block, statement, argument, the name flowing in)
    let mut rewrites: Vec<(usize, usize, usize, LocalId)> = Vec::new();
    for (bi, block) in func.blocks.iter().enumerate() {
        let after_phis = block.statements.iter().take_while(|s| matches!(s.kind, MirStmtKind::Phi { .. })).count();
        for (si, stmt) in block.statements.iter().take(after_phis).enumerate() {
            let MirStmtKind::Phi { dst, args } = &stmt.kind else { continue };
            if !copy_aggregate(dst) || args.len() < 2 {
                continue;
            }
            // Read once the join is behind us: the frame keeps its own.
            let kept_after = |name: LocalId| {
                block.statements[after_phis..].iter().any(|later| aliases.stmt_reads(later, name))
                    || aliases.terminator_reads(&block.terminator, name)
                    || live.live_at_exit(block.id, name)
            };
            for (ai, (_, op)) in args.iter().enumerate() {
                let Some(a) = uses::operand_local(op) else { continue };
                if !copy_aggregate(&a) || owns_copy.contains(&a) {
                    continue;
                }
                let mut seen: HashSet<LocalId> = HashSet::new();
                let mut todo = vec![a];
                let mut kept = false;
                while let Some(n) = todo.pop() {
                    if !seen.insert(n) || seen.len() > 64 {
                        continue;
                    }
                    if kept_after(n) {
                        kept = true;
                        break;
                    }
                    todo.extend(source_of.get(&n).into_iter().flatten().copied());
                }
                if kept {
                    rewrites.push((bi, si, ai, a));
                }
            }
        }
    }
    if rewrites.is_empty() {
        return;
    }

    let mut next = func.locals.iter().chain(func.params.iter()).map(|l| l.id.0).max().map_or(0, |m| m + 1);
    let mut edges: Vec<(crate::BlockId, crate::BlockId, Vec<MirStmt>)> = Vec::new();
    for (bi, si, ai, a) in rewrites {
        let id = LocalId(next);
        next += 1;
        let ty = ty_of[&a].clone();
        func.locals.push(crate::MirLocal { id, name: None, ty, is_param: false });
        let to = func.blocks[bi].id;
        let span = func.blocks[bi].statements[si].span;
        let MirStmtKind::Phi { args, .. } = &mut func.blocks[bi].statements[si].kind else { continue };
        let from = args[ai].0;
        args[ai].1 = MirOperand::Local(id);
        edges.push((
            from,
            to,
            vec![
                MirStmt::new(MirStmtKind::Assign { dst: id, rvalue: MirRValue::Use(MirOperand::Local(a)) }, span),
                MirStmt::new(MirStmtKind::RcIncContents { local: id, for_hand_off: false }, span),
            ],
        ));
    }
    ownership::insert_on_edges(func, edges);
}

/// Does the call keep its argument at `i`, rather than only read it?
///
/// From the body when this pass can read one (`kept`), from the declaration
/// when it can't. A runtime call lowering makes for itself
/// (`FunctionRef::runtime`) reads what it's handed. Unknown is "keeps": the
/// caller then releases nothing it passed, which leaks rather than
/// double-frees.
fn call_keeps_arg(fref: &crate::FunctionRef, i: usize, kept: &HashMap<String, Vec<bool>>) -> bool {
    if fref.runtime {
        return false;
    }
    match kept.get(fref.name.as_str()) {
        Some(v) => v.get(i).copied().unwrap_or(true),
        None => fref.gives(i) || fref.shape.is_none(),
    }
}

/// Take a reference to what an aggregate holds, before a copy of one read out
/// of storage this frame doesn't own is handed to something that keeps it.
///
/// ```text
/// for t in tags { out.push(t) }
/// ```
///
/// `t` is a copy of an element of `tags`: same bytes, so the same string
/// buffer. `Vec_push` keeps what it's given and `out` releases its elements
/// when it goes, as `tags` does, so both released `name` (#1414). A bare
/// string element has always had this covered: copying a string local takes
/// a reference (`insert_rc_inc`). An aggregate copy took none.
///
/// A keeper is a call that keeps the argument, a store into memory, or the
/// caller, when the view is returned.
///
/// Only a copy of a *view* — something read out of storage a receiver keeps
/// owning (`returns_a_view`), a part of one, or a copy of either. A value the
/// frame owns is handed over instead, and `insert_aggregate_release` stops
/// releasing it. The checker only lets a Copy value be given away from a
/// borrow (`mem.parameters/PM6`, `type.sequence/SEQ47`), and a Copy aggregate
/// holds no container, so what this retains is strings — and the count on a
/// box, which a derived container shares rather than copies.
fn retain_views_handed_over(
    func: &mut MirFunction,
    kept: &HashMap<String, Vec<bool>>,
    writes: &HashMap<String, HashSet<usize>>,
    own: &HashSet<String>,
    owns_copy: &HashSet<LocalId>,
    closure_keeps: &ClosureKeeps,
    copy_layouts: &crate::CopyLayouts,
    target: rask_ast::Target,
) -> HashSet<LocalId> {
    let func_name = func.name.clone();
    let ty_of: HashMap<LocalId, MirType> = func
        .locals
        .iter()
        .chain(func.params.iter())
        .map(|l| (l.id, l.ty.clone()))
        .collect();
    // A box read out of a container is the same case: the element is a
    // pointer to a counted block, and a second owner needs a count of its own
    // (`boxes.skip(1).to_vec()`). So is a container under a tag or in a
    // tuple: the release walks into it, so a `Vec<i64>?` element copied into
    // a pick's answer was freed under the array that held it.
    let is_aggregate = |l: &LocalId| {
        ty_of.get(l).is_some_and(|t| {
            aggregate_may_hold_string(t)
                || wraps_a_container(t)
                || matches!(t, MirType::InterfaceObject { .. })
        })
    };

    // Names that hold a view, grown to a fixed point through copies, parts
    // and joins. A join counts only when every way in is a view: retaining
    // one that might be the frame's own is a leak, and that is the side to
    // err on. Workaround: in a loop that leaks the fresh value on the other
    // way in; retaining on the view's edge would make every join owned
    // (#1904).
    let mut views: HashSet<LocalId> = HashSet::new();
    // A capture a loop body's `return` moves out is handed over, not lent.
    let moved_captures: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|s| match s.kind {
            MirStmtKind::MovedOut { local } => Some(local),
            _ => None,
        })
        .collect();
    // A parameter the caller didn't give up is the caller's storage: a
    // borrowed or `mutate` one reaches a keeper only as a copy of a Copy
    // value (or, `mutate`, moved out and refilled), and the caller still
    // releases what it holds. Only a `take` parameter is the frame's to hand
    // on. Without the reference, `h.one = a` in a callee and a `mutate`
    // array pushed then refilled shared the caller's strings uncounted.
    for (i, p) in func.params.iter().enumerate() {
        if retains_what_it_hands_on(func, i, copy_layouts) {
            views.insert(p.id);
        }
    }
    let fills = filled_in_place(func, kept, writes, own, closure_keeps);
    let mut built: HashSet<LocalId> = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        // Filled only with views and plain values, it holds nothing of its
        // own: a view of the same storage (#2048).
        for (agg, values) in &fills {
            use crate::operand::MirConst;
            let all_views = values.iter().all(|v| match v {
                MirOperand::Local(l) => views.contains(l) || ty_of.get(l).is_some_and(|t| t.is_plain_scalar()),
                MirOperand::Constant(c) => matches!(
                    c,
                    MirConst::Int(_) | MirConst::Int128(_) | MirConst::Float(_) | MirConst::Bool(_) | MirConst::Char(_)
                ),
                _ => false,
            });
            let any_view = values.iter().any(|v| matches!(v, MirOperand::Local(l) if views.contains(l)));
            if all_views && any_view && is_aggregate(agg) && !owns_copy.contains(agg) && views.insert(*agg) {
                built.insert(*agg);
                changed = true;
            }
        }
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            let found = match &stmt.kind {
                MirStmtKind::Call { func: fref, dst: Some(dst), args } => {
                    (crate::own_names::returns_a_view(fref, own) && !args.is_empty())
                        .then_some(*dst)
                }
                // A capture stays where it was captured: in the frame that
                // built the closure, or in the environment that carries it.
                // A closure hands back what it returns (#1441), so `|| held`
                // gives the caller references of its own. A closure that
                // borrows a non-Copy capture can't return it (E0907,
                // #1449); one that carries it still can, and then each call
                // deep-clones it. A task block's capture is the block's own:
                // the environment frees only what the block didn't hand on
                // (`container_drop::task_captures_given_away`), so handing it
                // on is a move.
                MirStmtKind::LoadCapture { dst, access, .. } => {
                    (*access != crate::CaptureAccess::Taken && !moved_captures.contains(dst)).then_some(*dst)
                }
                // An element bound where it sits: the collection keeps it.
                MirStmtKind::BindPlace { dst, .. } => Some(*dst),
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                    views.contains(src).then_some(*dst)
                }
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Field { base, .. } | MirRValue::ArrayIndex { base, .. },
                } => uses::operand_local(base).filter(|b| views.contains(b)).map(|_| *dst),
                MirStmtKind::Phi { dst, args } => (!args.is_empty()
                    && args.iter().all(|(_, op)| {
                        uses::operand_local(op).is_some_and(|l| views.contains(&l))
                    }))
                .then_some(*dst),
                _ => None,
            };
            // A copy that took references of its own isn't a view: handing
            // it over hands those references on.
            if let Some(d) = found.filter(|d| is_aggregate(d) && !owns_copy.contains(d)) {
                if views.insert(d) {
                    changed = true;
                }
            }
        }
    }
    let live = liveness::analyze_phis_on_edges(func);
    let aliases = AddrAliases::build(func);
    // What each name was read out of or copied from: `_22 = _19.0` is part
    // of `_19`. Handing a part to a keeper hands on the value it is part of,
    // so the part is a copy while that value is still read afterwards
    // (`c.one = a.one` then `a.one = …`).
    // A join (`if c { a } else { s.one }`) has a source per way in.
    let mut source_of: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
            | MirStmtKind::Assign {
                dst,
                rvalue: MirRValue::Field { base: MirOperand::Local(src), .. }
                    | MirRValue::ArrayIndex { base: MirOperand::Local(src), .. },
            } if dst != src => {
                source_of.entry(*dst).or_default().push(*src);
            }
            MirStmtKind::Phi { dst, args } => {
                for (_, op) in args {
                    if let Some(src) = uses::operand_local(op).filter(|s| s != dst) {
                        source_of.entry(*dst).or_default().push(src);
                    }
                }
            }
            _ => {}
        }
    }

    for block in &mut func.blocks {
        let mut insertions: Vec<(usize, LocalId)> = Vec::new();
        // A value the frame owns, handed to a keeper while the frame still
        // reads it afterwards, is a Copy value copied there: the checker
        // rejects any use of a non-Copy value after it moves. The keeper
        // gets references of its own and the frame keeps its value
        // (`insert_aggregate_release` reads the retain and doesn't count the
        // hand-off as a move). `h.one = x` then `println(x[0])`.
        let used_after = |si: usize, local: LocalId, stmts: &[MirStmt], term: &crate::MirTerminator| {
            for later in &stmts[si + 1..] {
                if aliases.stmt_reads(later, local) {
                    return true;
                }
                if uses::stmt_def(later) == Some(local) {
                    return false;
                }
            }
            aliases.terminator_reads(term, local) || live.live_at_exit(block.id, local)
        };
        // `into` is the storage the value is going into, for a store: a value
        // read out of that storage (`s.one = if c { … } else { s.one }`) is
        // what the slot's release frees, so it has to be a copy.
        let copies = |si: usize, v: LocalId, into: Option<LocalId>, stmts: &[MirStmt], term: &crate::MirTerminator| {
            if views.contains(&v) {
                return true;
            }
            // Only a Copy value: a part of a value moved out and refilled
            // (`let old = self.log`) has its source read afterwards too.
            if !is_aggregate(&v) || !ty_of.get(&v).is_some_and(|t| copy_layouts.is_copy(t)) {
                return false;
            }
            // A copy with references of its own hands them on, unless the
            // frame reads it again: then the keeper needs its own
            // (`let q = items[0]; keep.push(q); println(q.s)`).
            if owns_copy.contains(&v) {
                return used_after(si, v, stmts, term);
            }
            // The value itself, or anything it was read out of, read again.
            let mut seen: HashSet<LocalId> = HashSet::new();
            let mut todo = vec![v];
            while let Some(name) = todo.pop() {
                if !seen.insert(name) || seen.len() > 64 {
                    continue;
                }
                if into == Some(name) || used_after(si, name, stmts, term) {
                    return true;
                }
                for src in source_of.get(&name).into_iter().flatten() {
                    // A field read out of a larger value that holds more: the
                    // copy needs references of its own whether or not the
                    // value is read again. Moved, the keeper took the whole
                    // value's reference to it, and what else the value held
                    // was released by nobody (`let v = [tw.a]` leaked
                    // `tw.b`'s string).
                    if is_aggregate(src)
                        && !owns_copy.contains(src)
                        && ty_of.get(src).zip(ty_of.get(&name)).is_some_and(|(whole, part)| rask_layout::size_of(whole, target) > rask_layout::size_of(part, target))
                    {
                        return true;
                    }
                    if !owns_copy.contains(src) {
                        todo.push(*src);
                    }
                }
            }
            false
        };
        for (si, stmt) in block.statements.iter().enumerate() {
            match &stmt.kind {
                MirStmtKind::Call { func: fref, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        if call_keeps_arg(fref, i, kept)
                            && copies(si, id, None, &block.statements, &block.terminator)
                        {
                            insertions.push((si, id));
                        }
                    }
                }
                // The same through a closure value: an argument the bodies
                // behind it take.
                MirStmtKind::ClosureCall { closure, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        if closure_keeps(&func_name, *closure, i, args.len())
                            && copies(si, id, None, &block.statements, &block.terminator)
                        {
                            insertions.push((si, id));
                        }
                    }
                }
                // Copied whole into memory that outlives the statement: a
                // struct literal's field, an array slot. Not into the slot it
                // came from (`p = p`): that copies nothing.
                MirStmtKind::Store { addr, value: MirOperand::Local(v), .. }
                | MirStmtKind::ArrayStore { base: addr, value: MirOperand::Local(v), .. }
                    if addr != v
                        && !built.contains(addr)
                        && !store_is_narrow(stmt, &ty_of)
                        && lowering_copy_retain(&block.statements, si, owns_copy).is_none()
                        && copies(si, *v, Some(*addr), &block.statements, &block.terminator) =>
                {
                    // Before the release of the slot it is about to fill, if
                    // the statement before gives that slot back: the value
                    // can be what the slot held (`s.one = if c { s.one } else
                    // { … }`), and retained after the release it was freed.
                    let at = match (si.checked_sub(1).map(|p| &block.statements[p].kind), &stmt.kind) {
                        (Some(MirStmtKind::ReleaseSlot { addr: ra, offset: ro, .. }), MirStmtKind::Store { addr, offset, .. })
                            if ra == addr && ro == offset => si - 1,
                        (Some(MirStmtKind::ReleaseSlot { .. }), MirStmtKind::ArrayStore { .. }) => si - 1,
                        _ => si,
                    };
                    insertions.push((at, *v));
                }
                _ => {}
            }
        }
        // Returned: the caller is the keeper. Unless this function is itself
        // one that hands out a view, whose callers release nothing.
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } =
            &block.terminator.kind
        {
            if views.contains(id) {
                insertions.push((block.statements.len(), *id));
            }
        }
        for (idx, local) in insertions.into_iter().rev() {
            let span = block.statements.get(idx).map(|s| s.span).unwrap_or(block.terminator.span);
            block.statements.insert(idx, MirStmt::new(MirStmtKind::RcIncContents { local, for_hand_off: true }, span));
        }
    }
    built
}

/// Aggregates a literal builds in place (field stores, after a
/// `ZeroAggregate` or not) and nothing else writes, with the values stored into each. A tuple or
/// option wrapped around an element (`enumerate`'s pair, a pick's answer) is
/// one. Anything that could put a value there some other way leaves it out:
/// a definition, an address taken, an array store, a call that may write
/// through it.
fn filled_in_place(
    func: &MirFunction,
    kept: &HashMap<String, Vec<bool>>,
    writes: &HashMap<String, HashSet<usize>>,
    own: &HashSet<String>,
    closure_keeps: &ClosureKeeps,
) -> HashMap<LocalId, Vec<MirOperand>> {
    let mut fills: HashMap<LocalId, Vec<MirOperand>> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::ZeroAggregate { local } | MirStmtKind::Store { addr: local, .. } = &stmt.kind {
            fills.entry(*local).or_default();
        }
    }
    if fills.is_empty() {
        return fills;
    }
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    let mut out: HashSet<LocalId> = params;
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let Some(d) = uses::stmt_def(stmt) {
            out.insert(d);
        }
        match &stmt.kind {
            MirStmtKind::Store { addr, value, .. } => {
                if let Some(list) = fills.get_mut(addr) {
                    list.push(value.clone());
                }
            }
            MirStmtKind::ArrayStore { base, .. } | MirStmtKind::ReleaseSlot { addr: base, .. } => {
                out.insert(*base);
            }
            MirStmtKind::Assign { rvalue: MirRValue::Ref(src), .. } => {
                out.insert(*src);
            }
            MirStmtKind::Call { func: fref, args, .. } => {
                for (i, arg) in args.iter().enumerate() {
                    let Some(id) = uses::operand_local(arg) else { continue };
                    let may_write = !own.contains(&fref.name)
                        || writes.get(&fref.name).is_some_and(|set| set.contains(&i));
                    if !call_keeps_arg(fref, i, kept) && may_write && !fref.runtime {
                        out.insert(id);
                    }
                }
            }
            MirStmtKind::ClosureCall { closure, args, .. } => {
                for (i, arg) in args.iter().enumerate() {
                    if let Some(id) = uses::operand_local(arg) {
                        if !closure_keeps(&func.name, *closure, i, args.len()) {
                            out.insert(id);
                        }
                    }
                }
            }
            MirStmtKind::InterfaceCall { .. } | MirStmtKind::ClosureCreate { .. } => {
                for id in fills.keys() {
                    if uses::stmt_reads(stmt, *id) {
                        out.insert(*id);
                    }
                }
            }
            _ => {}
        }
    }
    fills.retain(|l, _| !out.contains(l));
    fills
}

/// Insert `RcInc` after each assignment that copies a string local.
///
/// Pattern: `dst = src` where both are string-typed → insert `RcInc { local: dst }`
/// after the assignment. The inc goes on `dst` because dst is the new reference
/// sharing the same string data.
fn insert_rc_inc(func: &mut MirFunction, string_locals: &[LocalId]) {
    let string_set: std::collections::HashSet<LocalId> = string_locals.iter().copied().collect();

    for block_idx in 0..func.blocks.len() {
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();

        for (si, stmt) in func.blocks[block_idx].statements.iter().enumerate() {
            match &stmt.kind {
                // Copy from another string local
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if string_set.contains(dst) && string_set.contains(src) =>
                {
                    insertions.push((si + 1, MirStmt::new(
                        MirStmtKind::RcInc { local: *dst },
                        stmt.span,
                    )));
                }
                // Phi producing a string — the incoming value already has a refcount,
                // but the phi creates a new name that needs to be tracked. The actual
                // inc happens at the copy site in the predecessor. No inc here.
                MirStmtKind::Phi { dst, .. } if string_set.contains(dst) => {}

                // Call returning a string — new allocation, refcount starts at 1. No inc.
                MirStmtKind::Call { dst: Some(dst), .. } if string_set.contains(dst) => {}

                // Field or array element extracting a string — this is a copy of
                // the string out of an aggregate, needs inc.
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Field { .. } | MirRValue::ArrayIndex { .. },
                } if string_set.contains(dst) =>
                {
                    insertions.push((si + 1, MirStmt::new(
                        MirStmtKind::RcInc { local: *dst },
                        stmt.span,
                    )));
                }

                // Stored into memory — a struct field, a Result payload. That
                // location now holds its own reference, so it needs its own
                // count. Without the inc, the dec at the local's last use freed
                // a buffer the field still points at: `Body { error: "no route
                // for {path}" }` printed whatever the next allocation put
                // there (#501). An array element is such a location too:
                // `a[0] = s` on a `[string; 2]` left the slot holding a string
                // the dec at `s`'s last use had freed.
                MirStmtKind::Store { value: MirOperand::Local(src), .. }
                | MirStmtKind::ArrayStore { value: MirOperand::Local(src), .. }
                    if string_set.contains(src) =>
                {
                    insertions.push((si, MirStmt::new(
                        MirStmtKind::RcInc { local: *src },
                        stmt.span,
                    )));
                }

                // Copied into a closure's environment, which is the same thing
                // one step further out: the environment holds a reference of
                // its own and gives it back when it dies (`mem.closures/CM2`).
                //
                // Without the retain, the dec at the string's last use — which
                // *is* this statement, nothing after it names the string — freed
                // the buffer while the closure still pointed at it, and
                // `fns.push(|x| "{prefix}:{x}")` printed an empty line (#1160).
                MirStmtKind::ClosureCreate { captures, .. } => {
                    for cap in captures.iter().filter(|c| !c.by_ref) {
                        if string_set.contains(&cap.local_id) {
                            insertions.push((si, MirStmt::new(
                                MirStmtKind::RcInc { local: cap.local_id },
                                stmt.span,
                            )));
                        }
                    }
                }

                _ => {}
            }

        }

        // Apply insertions in reverse to preserve indices
        for (idx, stmt) in insertions.into_iter().rev() {
            func.blocks[block_idx].statements.insert(idx, stmt);
        }
    }
}

/// Whether a value of `ty` holds a rack by value (`rack_holding_structs`).
fn holds_rack(ty: &MirType, rack_holders: &HashSet<u32>) -> bool {
    match ty {
        MirType::Struct(id) => rack_holders.contains(&id.id),
        MirType::Tuple(parts) => parts.iter().any(|p| holds_rack(p, rack_holders)),
        MirType::Array { elem, .. } | MirType::Option(elem) => holds_rack(elem, rack_holders),
        MirType::Result { ok, err } => holds_rack(ok, rack_holders) || holds_rack(err, rack_holders),
        _ => false,
    }
}

/// Release the strings an aggregate holds when the aggregate dies.
///
/// `Holder { text: "…" }` retains the string on the way into the field, and
/// nothing gave it back: the local's own release covers the local, not the
/// field. Same for a `T?` payload and a `T or E` payload — the wrapper owns a
/// reference and the wrapper going out of scope was silent. That was the last
/// ~7 MB per 200k turns after the plain cases were fixed (#1024).
///
/// This pass has no layouts, so it can't tell which aggregates hold strings.
/// It marks the death of every one it's sure about and lets codegen decide;
/// codegen has the layouts and emits nothing for the majority that hold none.
///
/// Where each one dies, and whether it is still this frame's there, is
/// `analysis::ownership`'s answer, per path. This function says what each
/// statement does to the aggregates: makes one, renames one, reads into one,
/// or hands one over. A value is released only where it is certainly ours
/// and certainly finished with; anything this can't see clearly is left
/// alone, because releasing what somebody else still holds is a
/// use-after-free and leaving it is a leak.
fn insert_aggregate_release(
    func: &mut MirFunction,
    kept: &HashMap<String, Vec<bool>>,
    writes: &HashMap<String, HashSet<usize>>,
    own: &HashSet<String>,
    owns_copy: &HashSet<LocalId>,
    built_from_views: &HashSet<LocalId>,
    closure_keeps: &ClosureKeeps,
    rack_holders: &HashSet<u32>,
    copy_layouts: &crate::CopyLayouts,
) {
    let func_name = func.name.clone();
    let ty_of: HashMap<LocalId, MirType> = func
        .locals
        .iter()
        .chain(func.params.iter())
        .map(|l| (l.id, l.ty.clone()))
        .collect();
    let aggregates: HashSet<LocalId> = func
        .locals
        .iter()
        // A wrapper around a container holds something worth releasing even
        // when nothing in it is a string: `Vec<i64>?` is a tag beside a handle,
        // and the vector behind that tag was nobody's. A bare container's
        // handle is `container_drop`'s to free.
        .filter(|l| aggregate_may_hold_string(&l.ty) || wraps_a_container(&l.ty))
        .map(|l| l.id)
        .collect();
    if aggregates.is_empty() {
        return;
    }

    // Closures this frame drops, and boxes it drops.
    //
    // `closures::insert_drops` and `interface_drop` emit a drop
    // only for a closure or box the frame owns, and this pass runs after both,
    // so the drop's presence answers "does the frame outlive it". One the
    // frame drops reads into what it holds and keeps it needed until the drop;
    // one it doesn't can outlive the frame and takes what it holds away.
    //
    // The frame owns a boxed value's contents; the box borrows them
    // (mem.shared-rack-heap, #1144). `InterfaceBox` copies the value
    // *shallowly*, so the box and the frame's own local hold the same
    // container handle, and two boxes of one value hold it twice: a free has
    // to happen exactly once, and the frame is where that can be arranged.
    //
    // Unless the drop says the box owns its contents: `interface_drop` only
    // says so when the value was built for the box and nothing else reads it,
    // and then the box's release is the one free and the value is handed to
    // it like any other store (#1424).
    let boxes_dropped: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::InterfaceDrop { interface_object, release: crate::BoxRelease::Borrows } => Some(*interface_object),
            _ => None,
        })
        .collect();
    // The drop is rarely on the boxing site's own name. Inlining copies the box
    // into the callee's parameter local and the drop lands there, so
    // `describe_one(one)` boxes into `_22` and drops `_32`.
    let mut copied_into: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } =
            &stmt.kind
        {
            copied_into.entry(*src).or_default().push(*dst);
        }
    }
    let reaches_a_drop = |start: LocalId| {
        let mut seen: HashSet<LocalId> = HashSet::new();
        let mut frontier = vec![start];
        while let Some(id) = frontier.pop() {
            if !seen.insert(id) {
                continue;
            }
            if boxes_dropped.contains(&id) {
                return true;
            }
            if let Some(next) = copied_into.get(&id) {
                frontier.extend(next.iter().copied());
            }
        }
        false
    };

    // The values a box only views, by every name the box goes by. A `take self`
    // call through the box consumes what is inside, and those values are given
    // up there on that path (the box was dropped as a borrower on the others).
    let lent_to_box: HashMap<LocalId, Vec<LocalId>> = {
        let mut lent: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::InterfaceBox { dst, value, .. } = &stmt.kind {
                if let Some(v) = uses::operand_local(value).filter(|_| reaches_a_drop(*dst)) {
                    lent.entry(*dst).or_default().push(v);
                }
            }
        }
        let mut changed = !lent.is_empty();
        while changed {
            changed = false;
            for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                let (dst, srcs): (LocalId, Vec<LocalId>) = match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                        (*dst, vec![*src])
                    }
                    MirStmtKind::Phi { dst, args } => {
                        (*dst, args.iter().filter_map(|(_, op)| uses::operand_local(op)).collect())
                    }
                    _ => continue,
                };
                for src in srcs {
                    for v in lent.get(&src).cloned().unwrap_or_default() {
                        let into = lent.entry(dst).or_default();
                        if !into.contains(&v) {
                            into.push(v);
                            changed = true;
                        }
                    }
                }
            }
        }
        lent
    };

    // Where a `Field_take` reads from: `dst = base + offset`, handed to it as
    // the field's address. The take happens at the call, so the aggregate the
    // address points into has to stay alive until then, not just until the
    // addition that made the address.
    let take_addresses: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Call { func: fref, args, .. } if fref.name == "Field_take" => {
                args.first().and_then(uses::operand_local)
            }
            _ => None,
        })
        .collect();
    // Addresses into an aggregate: the ones a take goes through, and the
    // ones an element is bound at (`arr + i * 24` for an array walk). Either
    // reaches into the aggregate, which has to outlive it.
    let inner_addresses: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::BindPlace { addr, .. } => Some(*addr),
            _ => None,
        })
        .chain(take_addresses.iter().copied())
        .collect();
    // Elements bound where they sit (`BindPlace`): a store through one writes
    // the collection's element, never a value of this frame's. A scalar one
    // is memory-resident (`addr_taken`), so its reads arrive as
    // `Deref(place)`, which reads the element exactly as `Use(place)` does.
    let places: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::BindPlace { dst, .. } => Some(*dst),
            _ => None,
        })
        .collect();
    // The aggregate each take address points into.
    let take_bases: HashMap<LocalId, LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Assign {
                dst,
                rvalue: MirRValue::BinaryOp { op: crate::operand::BinOp::Add, left: MirOperand::Local(base), .. },
            } if take_addresses.contains(dst) => Some((*dst, *base)),
            _ => None,
        })
        .collect();

    // Names for storage inside an aggregate: a nested struct or array field
    // read out of one (`_19 = _17.0`), an element's address inside it
    // (`_21 = _19 + 16`), mapped to the aggregate they sit in. A write
    // through one is a write into that aggregate, so a copy of it made
    // earlier is stale and the written one is the value from there on.
    let inline_roots: HashMap<LocalId, LocalId> = {
        let mut roots: HashMap<LocalId, LocalId> = HashMap::new();
        let mut changed = true;
        while changed {
            changed = false;
            for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                let found = match &st.kind {
                    MirStmtKind::Assign {
                        dst,
                        rvalue: MirRValue::Field { base: MirOperand::Local(b), .. }
                            | MirRValue::ArrayIndex { base: MirOperand::Local(b), .. },
                    } if aggregates.contains(dst) => Some((*dst, *b)),
                    MirStmtKind::Assign {
                        dst,
                        rvalue: MirRValue::BinaryOp {
                            op: crate::operand::BinOp::Add,
                            left: MirOperand::Local(b),
                            ..
                        },
                    } if roots.contains_key(b) => Some((*dst, *b)),
                    _ => None,
                };
                let Some((dst, b)) = found else { continue };
                let root = roots.get(&b).copied().unwrap_or(b);
                if aggregates.contains(&root) && roots.insert(dst, root) != Some(root) {
                    changed = true;
                }
            }
        }
        roots
    };

    // Names that may come to name an aggregate or read into one: the
    // aggregates, and whatever is copied, read or parked out of them.
    let mut tracked: HashSet<LocalId> = aggregates.clone();
    let mut changed = true;
    while changed {
        changed = false;
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            let reached = match &stmt.kind {
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                    tracked.contains(src).then_some(*dst)
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Deref(MirOperand::Local(src)) }
                    if places.contains(src) =>
                {
                    tracked.contains(src).then_some(*dst)
                }
                // An array element is a part the same way a field is (#1403).
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Field { base, .. } | MirRValue::ArrayIndex { base, .. },
                } => {
                    uses::operand_local(base)
                        .filter(|b| tracked.contains(b))
                        .and_then(|b| {
                            (part_of_field(*dst, b, &aggregates, &ty_of)
                                || view_of_field(*dst, b, &aggregates, &ty_of))
                            .then_some(*dst)
                        })
                }
                // `v[1]` on a `Vec<i64>` is a view call, but the element comes
                // back as a copied number that keeps nothing alive. Tracking
                // it made `return b[1]` read as returning the vector, and the
                // option holding it was never released (#2008).
                MirStmtKind::Call { dst: Some(dst), .. } if ty_of.get(dst).is_some_and(|t| t.is_plain_scalar()) => None,
                MirStmtKind::Call { func: fref, args, dst: Some(dst), .. } => {
                    let views_receiver = crate::own_names::viewed_argument(fref, args, own)
                        .and_then(uses::operand_local)
                        .is_some_and(|r| tracked.contains(&r));
                    let points_into = fref.points_into_args
                        && args.iter().filter_map(uses::operand_local).any(|a| tracked.contains(&a));
                    (views_receiver || points_into).then_some(*dst)
                }
                MirStmtKind::Store { addr, value, .. } => uses::operand_local(value)
                    .filter(|v| tracked.contains(v) && !aggregates.contains(v) && !aggregates.contains(addr))
                    .map(|_| *addr),
                MirStmtKind::BindPlace { dst, addr } => tracked.contains(addr).then_some(*dst),
                // An address into a tracked aggregate.
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::BinaryOp { op: crate::operand::BinOp::Add, left: MirOperand::Local(base), .. },
                } if inner_addresses.contains(dst) => tracked.contains(base).then_some(*dst),
                MirStmtKind::ClosureCreate { dst, .. } | MirStmtKind::InterfaceBox { dst, .. } => {
                    Some(*dst)
                }
                MirStmtKind::Phi { dst, args } => args
                    .iter()
                    .any(|(_, op)| uses::operand_local(op).is_some_and(|l| tracked.contains(&l)))
                    .then_some(*dst),
                _ => None,
            };
            if let Some(d) = reached {
                if tracked.insert(d) {
                    changed = true;
                }
            }
        }
    }

    // A slot a part was parked in, and the part: the one whole-width store of
    // a tracked name into a slot that is not itself an aggregate. A load back
    // out of the slot is the same value again. The drop pass frees a
    // container a stack closure captured by loading it from its slot
    // (`container_drop`'s lent slots), and read as anything else that free
    // handed nothing over, so the wrapper the container came out of released
    // it a second time (#1887).
    let parked: HashMap<LocalId, LocalId> = {
        let mut seen: HashMap<LocalId, Option<LocalId>> = HashMap::new();
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Store { addr, offset, value, .. } = &stmt.kind {
                let one = uses::operand_local(value).filter(|v| {
                    *offset == 0 && tracked.contains(v) && !aggregates.contains(v) && !aggregates.contains(addr)
                });
                match seen.get(addr) {
                    None => {
                        seen.insert(*addr, one);
                    }
                    Some(_) => {
                        seen.insert(*addr, None);
                    }
                }
            }
        }
        seen.into_iter().filter_map(|(a, v)| Some((a, v?))).collect()
    };
    if !parked.is_empty() {
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: MirRValue::Deref(MirOperand::Local(a)) } = &stmt.kind {
                if parked.contains_key(a) {
                    tracked.insert(*dst);
                }
            }
        }
    }

    // Locals holding the address of a place inside other storage: `base + k`
    // where the base is a link or an aggregate. Copied into an aggregate
    // name, that name borrows the place; it doesn't own it.
    let place_addresses: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Assign {
                dst,
                rvalue: MirRValue::BinaryOp { op: crate::operand::BinOp::Add, left: MirOperand::Local(base), .. },
            } if matches!(ty_of.get(base), Some(MirType::Link(_)))
                || aggregates.contains(base) =>
            {
                Some(*dst)
            }
            _ => None,
        })
        .collect();
    // Addresses a collection lent for a `mutate` argument (`v[i]`), and the
    // names that copy one. An inlined callee binds its parameter to one of
    // these; it names the element where it sits, which the collection owns.
    let lent_slots: HashSet<LocalId> = {
        let mut lent: HashSet<LocalId> = func
            .blocks
            .iter()
            .flat_map(|b| b.statements.iter())
            .filter_map(|st| match &st.kind {
                MirStmtKind::Call { func: fref, dst: Some(d), .. }
                    if fref.name == "Vec_borrow_elem" || fref.name == "Map_borrow_elem" =>
                {
                    Some(*d)
                }
                _ => None,
            })
            .collect();
        let mut changed = !lent.is_empty();
        while changed {
            changed = false;
            for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } = &st.kind {
                    if lent.contains(src) && lent.insert(*dst) {
                        changed = true;
                    }
                }
            }
        }
        lent
    };
    let params: Vec<LocalId> = func.params.iter().map(|p| p.id).collect();
    // A parameter every caller gave up at the call (`params_a_callee_keeps`:
    // one written `take`) is ours from entry, and released here unless it is
    // handed on. Not in a function C may call, which hands over nothing.
    let given_up = |i: usize| {
        !func.is_extern_c && kept.get(&func_name).and_then(|v| v.get(i)).copied().unwrap_or(false)
    };
    // A `mutate` one that isn't Copy is the caller's slot, held by the frame
    // for the call and handed back full at every exit (`held_mutate_params`).
    // A move out of it is a hand-over like any other, and a refill makes a
    // new value there, releasing the old one exactly when the frame still
    // holds it: on every path, or under a drop flag on some (#1903).
    let held = held_mutate_params(func, copy_layouts);
    let owned_params: Vec<LocalId> = params
        .iter()
        .enumerate()
        .filter(|(i, p)| (given_up(*i) || held.contains(p)) && tracked.contains(p))
        .map(|(_, p)| *p)
        .collect();
    let slot_parts = struct_field_parts(func, &ty_of, &held);
    // A field returned leaves the frame at the `return`, which is after the
    // last place the struct could be released. The marker puts the
    // hand-over on a statement, ahead of the release.
    for block in func.blocks.iter_mut() {
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(n)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(n)), .. } = &block.terminator.kind
        {
            if slot_parts.contains_key(n) {
                let marker = MirStmt::new(MirStmtKind::MovedOut { local: *n }, block.terminator.span);
                block.statements.push(marker);
            }
        }
    }
    let holds_own_reference = own_reference_points(func, &tracked);
    let dom = crate::analysis::dominators::DominatorTree::build(func);
    let mut facts = ownership::Facts {
        moved_parts: slot_parts.keys().copied().collect(),
        names: tracked.iter().copied().collect(),
        events: Vec::new(),
        terminator_events: Vec::new(),
        reads: Vec::new(),
        kills: Vec::new(),
        terminator_reads: Vec::new(),
        foreign: params.iter().copied().filter(|p| tracked.contains(p) && !owned_params.contains(p)).collect(),
        owned: owned_params.clone(),
        caller_slots: held.iter().copied().filter(|p| tracked.contains(p)).collect(),
    };

    // The variables a stack closure holds the address of, by closure name
    // (and its copies): whatever uses the closure may read them.
    let slot_captures: HashMap<LocalId, Vec<LocalId>> = {
        let mut map: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
        for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::ClosureCreate { dst, captures, heap, .. } = &st.kind {
                if !*heap {
                    let slots: Vec<LocalId> = captures
                        .iter()
                        .filter(|c| c.by_ref && aggregates.contains(&c.local_id) && tracked.contains(&c.local_id))
                        .map(|c| c.local_id)
                        .collect();
                    if !slots.is_empty() {
                        map.insert(*dst, slots);
                    }
                }
            }
        }
        let mut changed = !map.is_empty();
        while changed {
            changed = false;
            for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
                // The closure itself, a copy of it, or a handle that points
                // into it (a borrowing task's, joined later: the body runs
                // against the slots until then).
                let (dst, from): (LocalId, Vec<LocalId>) = match &st.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                        (*dst, vec![*src])
                    }
                    MirStmtKind::Call { func: fref, dst: Some(dst), args } if fref.points_into_args => {
                        (*dst, args.iter().filter_map(uses::operand_local).collect())
                    }
                    MirStmtKind::Call { func: fref, dst: Some(dst), args }
                        if crate::own_names::returns_a_view(fref, own) =>
                    {
                        let base = crate::own_names::viewed_argument(fref, args, own);
                        (*dst, base.and_then(uses::operand_local).into_iter().collect())
                    }
                    _ => continue,
                };
                let mut slots: Vec<LocalId> = from
                    .iter()
                    .filter_map(|src| map.get(src))
                    .flatten()
                    .copied()
                    .collect();
                slots.sort();
                slots.dedup();
                if slots.is_empty() {
                    continue;
                }
                let have = map.entry(dst).or_default();
                let before = have.len();
                have.extend(slots);
                have.sort();
                have.dedup();
                if have.len() != before {
                    changed = true;
                }
            }
        }
        map
    };

    for block in &func.blocks {
        // Slots this block has given back and not yet written over. A store
        // that follows a release is a *replacement*, not the end of the value:
        // what was there has just been freed by name, and what lands next is
        // the value's as much as the old one was (#1198). Only the store that
        // follows it: counting every store in the block, the one that first
        // filled `held` read as a replacement too, so nothing ever made the
        // value and nothing released it — `held.text = t` on a struct a
        // closure captured leaked the struct (#1448).
        let mut released_pending: HashSet<(LocalId, u32)> = HashSet::new();
        let mut events = Vec::with_capacity(block.statements.len());
        let mut reads = Vec::with_capacity(block.statements.len());
        let mut kills = Vec::with_capacity(block.statements.len());
        // The locals the statements just before this one took references
        // for: a run of retains, and a slot release between them and the
        // store they are for (`retain_views_handed_over` puts the retain
        // ahead of the release, so the value can be what the slot held).
        //
        // Not a retain lowering made (`owns_copy`): that one says the local
        // owns a copy for its whole life, and the hand-off after it moves
        // that copy on.
        let mut just_retained: HashSet<LocalId> = HashSet::new();
        // Copied by lowering (`lowering_copy_retain`): the retain's index,
        // and the source, which has to stay alive until it.
        let mut copy_reads: HashMap<usize, LocalId> = HashMap::new();
        for (si, stmt) in block.statements.iter().enumerate() {
            let retained_for_this: HashSet<LocalId> = match &stmt.kind {
                MirStmtKind::RcIncContents { local, for_hand_off } => {
                    if *for_hand_off {
                        just_retained.insert(*local);
                    }
                    HashSet::new()
                }
                MirStmtKind::ReleaseSlot { .. } => HashSet::new(),
                _ => std::mem::take(&mut just_retained),
            };
            let replaces = match &stmt.kind {
                MirStmtKind::ReleaseSlot { addr, offset, .. } => {
                    released_pending.insert((*addr, *offset));
                    false
                }
                MirStmtKind::Store { addr, offset, .. } => released_pending.remove(&(*addr, *offset)),
                _ => false,
            };
            let mut ev: Vec<ownership::Event> = Vec::new();
            let is_tracked = |l: &LocalId| tracked.contains(l);
            let hand_over = |ev: &mut Vec<ownership::Event>, l: LocalId| {
                if tracked.contains(&l) {
                    ev.push(ownership::Event::HandOver(l));
                }
            };
            match &stmt.kind {
                MirStmtKind::Phi { .. } => {}
                MirStmtKind::Assign { dst, rvalue: MirRValue::Deref(MirOperand::Local(a)) }
                    if is_tracked(dst) && parked.contains_key(a) =>
                {
                    ev.push(ownership::Event::Alias { dst: *dst, src: parked[a] });
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Deref(MirOperand::Local(src)) }
                    if is_tracked(dst) && is_tracked(src) && places.contains(src) =>
                {
                    ev.push(ownership::Event::Alias { dst: *dst, src: *src });
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if is_tracked(dst) =>
                {
                    if is_tracked(src) {
                        ev.push(ownership::Event::Alias { dst: *dst, src: *src });
                    } else if matches!(ty_of.get(src), Some(MirType::Link(_)))
                        || place_addresses.contains(src)
                        || lent_slots.contains(src)
                    {
                        // Or an address into storage something else owns: a
                        // field of a node or of a struct, handed to an
                        // inlined `mutate` parameter (`set(mutate h.inner)`
                        // binds `i = h + 24`). Taken over, the frame released
                        // the node's own vector (#1809).
                        // The node a link points at (`mem.racks/RK2`): the rack
                        // owns it, and a name reaching it through the link
                        // borrows. Inlining `a.sum()` on a `Link<Task>` binds
                        // `self` this way, and taken over it released the node's
                        // fields while the rack still held them (#1285).
                        ev.push(ownership::Event::Other(*dst));
                    } else if aggregates.contains(dst) {
                        // Copied in from a local this pass can't see into: a
                        // wrapper lowering didn't mark as holding a container
                        // (`_19 = _40` for `maybe_words(1)? as words`). Taken
                        // over here, as anything an aggregate is built from is.
                        ev.push(ownership::Event::Make(*dst));
                    } else {
                        ev.push(ownership::Event::Other(*dst));
                    }
                }
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Field { base, .. } | MirRValue::ArrayIndex { base, .. },
                } if is_tracked(dst) => {
                    match uses::operand_local(base).filter(|b| is_tracked(b)) {
                        Some(b) if part_of_field(*dst, b, &aggregates, &ty_of) => {
                            ev.push(ownership::Event::Part { dst: *dst, base: b })
                        }
                        Some(b) if view_of_field(*dst, b, &aggregates, &ty_of) => {
                            ev.push(ownership::Event::View { dst: *dst, base: b })
                        }
                        _ => ev.push(ownership::Event::Other(*dst)),
                    }
                }
                MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::BinaryOp { op: crate::operand::BinOp::Add, left: MirOperand::Local(base), .. },
                } if inner_addresses.contains(dst) && is_tracked(dst) && is_tracked(base) => {
                    ev.push(ownership::Event::View { dst: *dst, base: *base });
                }
                // An element bound where it sits reaches into the collection
                // it sits in, which has to outlive the binding.
                MirStmtKind::BindPlace { dst, addr } if is_tracked(dst) => {
                    ev.push(ownership::Event::View { dst: *dst, base: *addr });
                }
                // `Ref` hands out the address.
                MirStmtKind::Assign { dst, rvalue: MirRValue::Ref(src) } => {
                    hand_over(&mut ev, *src);
                    if is_tracked(dst) {
                        ev.push(ownership::Event::Other(*dst));
                    }
                }
                MirStmtKind::Call { func: fref, args, dst } => {
                    // A take zeroes the field in the aggregate it was read
                    // from, so that copy is the value from here on.
                    if fref.name == "Field_take" {
                        if let Some(base) = args
                            .first()
                            .and_then(uses::operand_local)
                            .and_then(|a| take_bases.get(&a).copied())
                            .filter(|b| is_tracked(b))
                        {
                            ev.push(ownership::Event::WriteThrough(base));
                        }
                    }
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        if !is_tracked(&id) {
                            continue;
                        }
                        // Any argument retained right before the call
                        // (`retain_views_handed_over`): the keeper got a copy
                        // with references of its own.
                        if retained_for_this.contains(&id) {
                            continue;
                        }
                        // What the callee does with it: from the body when this
                        // pass can read one (`kept`), from the declaration when
                        // it can't. Both sides refusing is how a `take self`
                        // struct's `Vec` came to be freed by nobody
                        // (`os.Command.spawn`), and a struct handed to `m.get`
                        // left its strings to nobody (#1394).
                        if !call_keeps_arg(fref, i, kept) {
                            // It may still write into it: a `mutate`
                            // parameter is the caller's slot, by address.
                            // Unless the body is known and never writes the
                            // aggregate itself, only what its handles point
                            // at (`w.pos[i] = p`): the copy then holds the
                            // same bytes as the name it was made from, and
                            // calling that a write left no name the frame
                            // could release the value under (#1927).
                            let writes_it = !own.contains(&fref.name)
                                || writes.get(&fref.name).is_some_and(|set| set.contains(&i));
                            if aggregates.contains(&id) && writes_it {
                                ev.push(ownership::Event::WriteThrough(id));
                            }
                            continue;
                        }
                        ev.push(ownership::Event::HandOver(id));
                    }
                    if let Some(dst) = dst.filter(|d| is_tracked(d)) {
                        // A call gives up what it returns, unless it hands back
                        // a view into storage an argument keeps, the way
                        // `v.get(i)` points into the vector's own buffer.
                        if crate::own_names::returns_a_view(fref, own) {
                            let base = crate::own_names::viewed_argument(fref, args, own);
                            match base.and_then(uses::operand_local).filter(|r| is_tracked(r)) {
                                Some(recv) => ev.push(ownership::Event::View { dst, base: recv }),
                                None => ev.push(ownership::Event::Other(dst)),
                            }
                        } else if fref.points_into_args
                            && args.iter().filter_map(uses::operand_local).any(|a| is_tracked(&a))
                        {
                            // The result points into its arguments, so it keeps
                            // them needed for as long as it is in use: a pointer
                            // out of `as_ptr`, a borrowing task's handle.
                            for a in args.iter().filter_map(uses::operand_local).filter(|a| is_tracked(a)) {
                                ev.push(ownership::Event::View { dst, base: a });
                            }
                        } else if aggregates.contains(&dst) {
                            ev.push(ownership::Event::Make(dst));
                        } else {
                            ev.push(ownership::Event::Other(dst));
                        }
                    }
                }
                MirStmtKind::Store { addr, value, .. } => {
                    let copied = lowering_copy_retain(&block.statements, si, owns_copy);
                    if let (Some(at), Some(v)) = (copied, uses::operand_local(value)) {
                        copy_reads.insert(at, v);
                    }
                    if let Some(v) = uses::operand_local(value).filter(|v| {
                        is_tracked(v) && !retained_for_this.contains(v) && copied.is_none()
                    }) {
                        // A retain of the slot's own follows in this block.
                        // `owns_copy` alone says only that one is made
                        // somewhere in the function: the first store into the
                        // slot of `mut t = [s]` (no retain before the slot is
                        // written again) is a hand-over.
                        let retain_follows = block.statements[si + 1..]
                            .iter()
                            .find_map(|later| match &later.kind {
                                MirStmtKind::RcIncContents { local, for_hand_off: false } if local == addr => Some(true),
                                MirStmtKind::Store { addr: a, .. } if a == addr => Some(false),
                                MirStmtKind::ZeroAggregate { local } if local == addr => Some(false),
                                _ => None,
                            })
                            .unwrap_or(false);
                        if built_from_views.contains(addr) {
                            // A literal holding only views is one too.
                            ev.push(ownership::Event::ViewAlso { dst: *addr, base: v });
                        } else if owns_copy.contains(addr) && retain_follows {
                            // A Copy value bound into a slot lowering retains
                            // (`retain_bound_copy`): the slot takes references
                            // of its own, so this is a copy and the source
                            // keeps what it had. `let half = p.left`, with
                            // `half` in memory because a closure points at it,
                            // read as handing `p` away and leaked it.
                        } else if !aggregates.contains(&v) && !aggregates.contains(addr) {
                            // A handle parked in a buffer so a call can point
                            // at it: whoever reads the buffer reads through the
                            // aggregate. `json.encode(p)` on a `struct { counts:
                            // Map }` hands the encoder the field's handle this
                            // way.
                            ev.push(ownership::Event::ViewAlso { dst: *addr, base: v });
                        } else {
                            // Copied whole into memory: whatever the
                            // destination is, it holds the value now.
                            ev.push(ownership::Event::HandOver(v));
                        }
                    }
                    // A held `mutate` parameter is the caller's slot: a store
                    // writes into it and never starts a value of its own. Its
                    // only new value is a refill's `ZeroAggregate`. Read as a
                    // fill, a field store after a join that took a field out
                    // on one way in made a new value there, and the old one
                    // was released while the caller still held it.
                    let into_slot = held.contains(addr);
                    if aggregates.contains(addr)
                        && !built_from_views.contains(addr)
                        && !replaces
                        && !into_slot
                        && !places.contains(addr)
                        && !store_is_narrow(stmt, &ty_of)
                    {
                        ev.push(ownership::Event::Fill(*addr));
                    }
                    // A replacement writes the slot in place: what it held was
                    // just released, so a copy made earlier holds a freed
                    // value. `mut p = …; p.run = || …` stores into `p`'s new
                    // version, and releasing the struct under the old name
                    // freed the replaced closure a second time.
                    if aggregates.contains(addr) && (replaces || into_slot) {
                        ev.push(ownership::Event::WriteThrough(*addr));
                    }
                }
                MirStmtKind::ArrayStore { base, value, .. } => {
                    if let Some(v) = uses::operand_local(value)
                        .filter(|v| !retained_for_this.contains(v))
                    {
                        hand_over(&mut ev, v);
                    }
                    if let Some(root) = inline_roots.get(base).filter(|r| is_tracked(r)) {
                        ev.push(ownership::Event::WriteThrough(*root));
                    }
                }
                MirStmtKind::ReleaseSlot { addr, .. } => {
                    if let Some(root) = inline_roots.get(addr).filter(|r| is_tracked(r)) {
                        ev.push(ownership::Event::WriteThrough(*root));
                    }
                }
                MirStmtKind::InterfaceCall { dst, interface_object, takes_receiver, .. } => {
                    if *takes_receiver {
                        for v in lent_to_box.get(interface_object).into_iter().flatten() {
                            hand_over(&mut ev, *v);
                        }
                    }
                    // What it returns is the frame's, as from any call. With no
                    // event the `any Label or Refused` from `m.make(n)` was
                    // released by nobody: the box in it, and `Refused`'s string.
                    if let Some(dst) = dst.filter(|d| aggregates.contains(d)) {
                        ev.push(ownership::Event::Make(dst));
                    }
                }
                // A box read out of a wrapper and dropped by `interface_drop`
                // was taken out of it: that drop is the box's one free, and the
                // wrapper holds nothing more on this path. Released as well,
                // the wrapper's walk would free the box a second time.
                MirStmtKind::InterfaceDrop { interface_object, .. } => {
                    hand_over(&mut ev, *interface_object);
                }
                MirStmtKind::InterfaceBox { dst, value, .. } => {
                    let v = uses::operand_local(value).filter(|v| is_tracked(v));
                    if reaches_a_drop(*dst) {
                        match v {
                            Some(v) => ev.push(ownership::Event::View { dst: *dst, base: v }),
                            None => ev.push(ownership::Event::Other(*dst)),
                        }
                    } else {
                        if let Some(v) = v {
                            hand_over(&mut ev, v);
                        }
                        ev.push(ownership::Event::Other(*dst));
                    }
                }
                // What a closure carries moves into its environment, which
                // releases it when the closure dies (`mem.closures/CM2`). A
                // Copy capture is copied in with references of its own
                // (`retain_copy_captures`, and `insert_rc_inc` for a string),
                // so for this frame it is a read. A lent one stays the
                // caller's (CM3), viewed for as long as the closure lives.
                //
                // A stack closure's by-reference capture holds the variable's
                // address, not a value: it reads whatever the slot holds when
                // it is called, so it keeps the *name* needed for as long as it
                // is used (`slot_captures`, above) and isn't a view of the
                // value the slot held when the closure was made. A view of that
                // value outlived the slot's next value, and nothing was left to
                // release it (#1839).
                MirStmtKind::ClosureCreate { dst, captures, heap, .. } => {
                    let mut any = false;
                    for c in captures.iter().filter(|c| is_tracked(&c.local_id)) {
                        let id = c.local_id;
                        if c.by_ref && !*heap && aggregates.contains(&id) {
                            continue;
                        }
                        if c.lent || (c.by_ref && !*heap) {
                            ev.push(ownership::Event::View { dst: *dst, base: id });
                            any = true;
                        } else if !c.copy {
                            // Carried in (CM2), and whatever it points at the
                            // new closure points at too: a `map` over a
                            // `walk()` that points at `self` needs `self` for
                            // as long as the `map` lives. Over a value it
                            // owns, the view is of something handed over and
                            // keeps nothing.
                            ev.push(ownership::Event::View { dst: *dst, base: id });
                            any = true;
                            hand_over(&mut ev, id);
                        }
                    }
                    if !any {
                        ev.push(ownership::Event::Other(*dst));
                    }
                }
                // A closure gives up what it returns, the same as a function
                // does. Counted as nobody's, `match mk()` on a closure that
                // builds a `Slot.Full(line)` left the line's string to no one
                // (#1441).
                //
                // An argument goes to the callee the way it does on a direct
                // call: when the parameter the bodies behind the call declare
                // is `take`. Counted as a loan, a `Heap` read out of an
                // optional and handed to a `func(take Heap<i64>)` value was
                // freed by the callee and then again with the optional.
                //
                // The bodies' modes, not the function value's type: a `for`
                // body over a sequence is called through `func(T) -> bool`
                // (the yield lends, SEQ34) and still takes an item the chain
                // owns when it keeps it (SEQ47). Requiring the type to say
                // `take` as well left a struct item the body returned to be
                // released by the sequence after the call.
                MirStmtKind::ClosureCall { dst, closure, args, .. } => {
                    for (i, arg) in args.iter().enumerate() {
                        let Some(id) = uses::operand_local(arg) else { continue };
                        if is_tracked(&id)
                            && !retained_for_this.contains(&id)
                            && closure_keeps(&func_name, *closure, i, args.len())
                        {
                            ev.push(ownership::Event::HandOver(id));
                        } else if aggregates.contains(&id) && !retained_for_this.contains(&id) {
                            // Not kept, but a `mutate` parameter is the
                            // caller's slot by address: the call writes into
                            // this copy, so it is the value from here on
                            // (the same as a direct call, above). Unmarked, a
                            // panic in the closure left the unwind record
                            // armed on the stale original (#1937).
                            ev.push(ownership::Event::WriteThrough(id));
                        }
                    }
                    if let Some(dst) = dst.filter(|d| aggregates.contains(d)) {
                        ev.push(ownership::Event::Make(dst));
                    }
                }
                // Released already, by whoever lowered it: `drop(p)` on a
                // `Heap<T>` releases the payload's contents before giving the
                // block back.
                MirStmtKind::RcDecContents { local } => hand_over(&mut ev, *local),
                // A loop body's `return` took it.
                MirStmtKind::MovedOut { local } => hand_over(&mut ev, *local),
                // A literal starts here, empty, and its field stores fill it.
                MirStmtKind::ZeroAggregate { local } if built_from_views.contains(local) => {
                    ev.push(ownership::Event::Other(*local))
                }
                MirStmtKind::ZeroAggregate { local } if aggregates.contains(local) => {
                    ev.push(ownership::Event::Make(*local))
                }
                // Retained by lowering: from here on the name holds references
                // of its own, and is ours to release.
                MirStmtKind::RcIncContents { local, for_hand_off: false } if is_tracked(local) =>
                {
                    ev.push(ownership::Event::Make(*local))
                }
                _ => {
                    if let Some(d) = uses::stmt_def(stmt).filter(|d| is_tracked(d)) {
                        ev.push(ownership::Event::Other(d));
                    }
                }
            }

            // Reads and writes, for liveness. A store through a pointer value
            // (`Cell_data(c)`, an address into another value's storage) uses
            // the pointer and keeps what it points into alive. Taken for a
            // scratch slot's write, `*(Cell_data(c)) = n` as the last use of
            // a buffer released the buffer first and wrote into freed memory.
            let through_pointer = |addr: LocalId| matches!(ty_of.get(&addr), Some(MirType::Ptr));
            let store_into = match &stmt.kind {
                MirStmtKind::Store { addr, offset, value, .. } => Some((*addr, *offset, value)),
                _ => None,
            };
            let mut r = Vec::new();
            let mut k = Vec::new();
            if !matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                for name in &facts.names {
                    let reads = match store_into {
                        // A replacement keeps the aggregate going: what lands
                        // in the slot is the aggregate's to release. Unread,
                        // `a.text = t` as the last thing done to `a` put the
                        // release between the slot's release and the store,
                        // freeing the old string twice and leaking `t`.
                        Some((addr, _, value)) if addr == *name => {
                            uses::operand_local(value) == Some(*name) || replaces || through_pointer(addr)
                        }
                        _ => uses::stmt_reads(stmt, *name),
                    } || copy_reads.get(&si) == Some(name)
                        || slot_captures
                            .iter()
                            .any(|(cl, slots)| slots.contains(name) && uses::stmt_reads(stmt, *cl));
                    if reads {
                        r.push(*name);
                    }
                    // A store into a scratch slot writes it; one into an
                    // aggregate is a `Fill`, which the analysis decides.
                    let writes = match (&stmt.kind, store_into) {
                        (MirStmtKind::ZeroAggregate { local }, _) => local == name,
                        (_, Some((addr, _, _))) => {
                            addr == *name && !aggregates.contains(name) && !through_pointer(addr)
                        }
                        (_, None) => uses::stmt_def(stmt) == Some(*name),
                    };
                    if writes {
                        k.push(*name);
                    }
                }
            }
            events.push(ev);
            reads.push(r);
            kills.push(k);
        }
        facts.events.push(events);
        facts.reads.push(reads);
        facts.kills.push(kills);
        // An element the frame took its own reference to before returning it
        // is that reference being returned, not a view still reading its
        // base: `return b[0]` out of a `Vec<string>?` retains the element, so
        // the option is the frame's to release before the return (#2008).
        let returned = match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => Some(*id),
            _ => None,
        };
        let own_reference_returned = returned.filter(|id| {
            holds_own_reference.get(id).is_some_and(|&(rb, ri)| {
                if rb == block.id {
                    ri < block.statements.len()
                } else {
                    dom.dominates(rb, block.id)
                }
            })
        });
        let mut term_ev = Vec::new();
        if let Some(id) = returned.filter(|id| tracked.contains(id) && own_reference_returned.is_none()) {
            term_ev.push(ownership::Event::HandOver(id));
        }
        // The caller gets its slot back with whatever the frame left in it.
        // A path that never returns (a panic) leaves it to the caller too:
        // the caller's own unwinding releases it, and a release here was a
        // second free of the caller's value.
        let exits = matches!(
            block.terminator.kind,
            MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. } | MirTerminatorKind::Unreachable
        );
        let handed_back: Vec<LocalId> =
            if exits { held.iter().copied().filter(|p| tracked.contains(p)).collect() } else { Vec::new() };
        term_ev.extend(handed_back.iter().map(|p| ownership::Event::HandOver(*p)));
        facts.terminator_events.push(term_ev);
        facts.terminator_reads.push(
            facts
                .names
                .iter()
                .copied()
                .filter(|n| {
                    (uses::terminator_reads(&block.terminator, *n) && own_reference_returned != Some(*n))
                        || handed_back.contains(n)
                        || slot_captures
                            .iter()
                            .any(|(cl, slots)| slots.contains(n) && uses::terminator_reads(&block.terminator, *cl))
                })
                .collect(),
        );
    }

    // A value holding a rack has readers this analysis can't see: links
    // stored elsewhere point into its rack, and none of them is a read of the
    // name. Released at its last read, a later write of one of those links
    // walked the freed rack (#1587). Such a frame releases at scope ends, the
    // placement for values with unseen readers.
    let placement = if aggregates.iter().any(|a| ty_of.get(a).is_some_and(|t| holds_rack(t, rack_holders))) {
        ownership::Placement::ScopeEnd
    } else {
        ownership::Placement::LastUse
    };
    let plan = ownership::plan(func, &facts, placement);

    // Insert back to front so earlier indices stay put. Step over the retains
    // already sitting at the spot: the last use of a wrapper is usually the
    // read that pulls its payload out, and the retain on that payload is the
    // next statement — releasing first frees the buffer the retain is about to
    // touch.
    //
    // The unwind marks go in first, against the blocks as planned. A literal
    // is armed from its `ZeroAggregate`, which leaves every field it hasn't
    // stored yet reading as empty (#1518).
    let guards = ownership::Guards::new(func, &plan);
    let (shifted, unwind_edges) = ownership::place_unwind(
        func,
        plan.unwind,
        &guards,
        &mut |_: &mut MirFunction, name: LocalId, _| {
            vec![MirStmt::dummy(MirStmtKind::RcDecContents { local: name })]
        },
    );
    let mut by_block: HashMap<usize, Vec<(usize, LocalId, Option<usize>)>> = HashMap::new();
    // The slot a field left empty when it was handed on, so that releasing
    // the struct skips it. In front of the hand-over, and in front of a
    // release at the same spot.
    let mut clears: HashMap<usize, Vec<(usize, MirStmt)>> = HashMap::new();
    for (bi, si, name, holders, guard) in &plan.moved_out {
        let Some((root, offset)) = slot_parts.get(name) else { continue };
        let block = &func.blocks[*bi];
        let span = block.statements.get(*si).map(|s| s.span).unwrap_or(block.terminator.span);
        let mut copies: Vec<LocalId> =
            holders.iter().copied().filter(|h| ty_of.get(h) == ty_of.get(root)).collect();
        if !copies.contains(root) {
            copies.push(*root);
        }
        let empty: Vec<MirStmt> = copies
            .into_iter()
            .map(|addr| {
                MirStmt::new(
                    MirStmtKind::Store {
                        addr,
                        offset: *offset,
                        value: MirOperand::Constant(crate::operand::MirConst::Int(0)),
                        store_size: 8,
                        field: None,
                    },
                    span,
                )
            })
            .collect();
        for stmt in guards.wrap(*guard, empty) {
            clears.entry(*bi).or_default().push((shifted.at(*bi, *si), stmt));
        }
    }
    let mut on_edges: Vec<(BlockId, BlockId, Vec<MirStmt>)> = Vec::new();
    for r in plan.releases {
        match r {
            ownership::Release::At { block, at, name, guard, .. } => {
                let stmts = &func.blocks[block].statements;
                let mut at = shifted.at(block, at);
                while at < stmts.len() && matches!(stmts[at].kind, MirStmtKind::RcInc { .. }) {
                    at += 1;
                }
                by_block.entry(block).or_default().push((at, name, guard));
            }
            ownership::Release::OnEdge { from, to, name, guard, .. } => {
                let span = func
                    .blocks
                    .iter()
                    .find(|b| b.id == from)
                    .map(|b| b.terminator.span)
                    .unwrap_or(crate::Span::new(0, 0));
                let release = vec![MirStmt::new(MirStmtKind::RcDecContents { local: name }, span)];
                on_edges.push((from, to, guards.wrap(guard, release)));
            }
        }
    }
    for (bi, mut list) in by_block {
        list.sort_by(|a, b| b.0.cmp(&a.0).then(b.1 .0.cmp(&a.1 .0)));
        for (at, name, guard) in list {
            let block = &mut func.blocks[bi];
            let span = block
                .statements
                .get(at.saturating_sub(1))
                .map(|s| s.span)
                .unwrap_or(block.terminator.span);
            let release = vec![MirStmt::new(MirStmtKind::RcDecContents { local: name }, span)];
            for (k, stmt) in guards.wrap(guard, release).into_iter().enumerate() {
                block.statements.insert(at + k, stmt);
            }
        }
    }
    for (bi, mut list) in clears {
        list.sort_by(|a, b| b.0.cmp(&a.0));
        for (at, stmt) in list {
            func.blocks[bi].statements.insert(at, stmt);
        }
    }
    // After the in-block ones: those index into the blocks as they were.
    let flag_edges = guards.edges(func);
    ownership::insert_on_edges(
        func,
        ownership::merge_edges(ownership::merge_edges(unwind_edges, on_edges), flag_edges),
    );
    guards.finish(func);
}

/// Names that hold a container or `Heap` block read out of a field of a
/// struct (`_5 = _0.1`), or a copy of one, with the struct's local and the
/// field's byte offset. Each is defined once, so the name says which field it
/// is wherever it is handed on.
///
/// Only a field no other name can be reading a copy of: a `Copy` field is
/// read again and again, and a closure or raw pointer read out of a struct is
/// a view of what the struct keeps. A container or a `Heap` block is moved
/// when it is handed on, and the struct is not the one to release it then.
///
/// Not a field of a held `mutate` parameter, which is the caller's slot with
/// its own rules for moving out and refilling (`held_mutate_params`).
fn struct_field_parts(
    func: &MirFunction,
    ty_of: &HashMap<LocalId, MirType>,
    held: &[LocalId],
) -> HashMap<LocalId, (LocalId, u32)> {
    let mut defs: HashMap<LocalId, usize> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let Some(d) = uses::stmt_def(stmt) {
            *defs.entry(d).or_default() += 1;
        }
    }
    let once = |l: &LocalId| defs.get(l).copied().unwrap_or(0) <= 1;
    // The local a name is a copy of, followed back.
    let mut copy_of: HashMap<LocalId, LocalId> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } = &stmt.kind {
            if once(dst) {
                copy_of.insert(*dst, *src);
            }
        }
    }
    let origin = |mut l: LocalId| {
        let mut hops = 0;
        while let Some(src) = copy_of.get(&l) {
            l = *src;
            hops += 1;
            if hops > copy_of.len() {
                break;
            }
        }
        l
    };
    let mut out: HashMap<LocalId, (LocalId, u32)> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Assign {
            dst,
            rvalue: MirRValue::Field { base: MirOperand::Local(root), byte_offset: Some(offset), .. },
        } = &stmt.kind
        else {
            continue;
        };
        let moves_out = matches!(ty_of.get(dst), Some(MirType::Container(_) | MirType::Heap(_) | MirType::InterfaceObject { .. }))
            && matches!(ty_of.get(root), Some(MirType::Struct(_)))
            && once(dst)
            && !held.contains(&origin(*root));
        if moves_out {
            out.insert(*dst, (*root, *offset));
        }
    }
    // A phi, or a name written more than once, is a join: an inlined
    // callee's result, or an `if` that gives the field on one branch and
    // something else on the other. It stands for the field when every copy
    // into it that is a field is that one field; the planner says on which
    // paths it holds it.
    let mut join_copies: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } if !once(dst) => {
                join_copies.entry(*dst).or_default().push(*src);
            }
            MirStmtKind::Phi { dst, args } => {
                let srcs = args.iter().filter_map(|(_, a)| match a {
                    MirOperand::Local(l) => Some(*l),
                    _ => None,
                });
                join_copies.entry(*dst).or_default().extend(srcs);
            }
            _ => {}
        }
    }
    // A copy of one is the same field.
    loop {
        let mut grew = false;
        for (dst, src) in &copy_of {
            if let Some(part) = out.get(src).copied() {
                if !out.contains_key(dst) {
                    out.insert(*dst, part);
                    grew = true;
                }
            }
        }
        for (dst, srcs) in &join_copies {
            if out.contains_key(dst) {
                continue;
            }
            let parts: HashSet<(LocalId, u32)> = srcs.iter().filter_map(|s| out.get(s).copied()).collect();
            if parts.len() == 1 {
                out.insert(*dst, *parts.iter().next().unwrap());
                grew = true;
            }
        }
        if !grew {
            return out;
        }
    }
}

/// Whether reading a field of `base` into `dst` gives a part of what `base`
/// holds, so that handing `dst` on hands the whole on.
///
/// - A payload that is itself an aggregate names the same bytes: `v = r.0`
///   doesn't copy the strings, it points at where they already are.
/// - A container handle or a `Heap` block out of an aggregate is the
///   container the aggregate owns. `*h.inner` read after the release read
///   freed memory (#1256).
/// - A box out of an aggregate is the box the aggregate owns, the same way:
///   its release walk frees it. Counted as a view, `let l = try direct(n)`
///   stored the box in a new result and released the old one whole, so the
///   walk freed the box the new result held.
/// - A closure out of an aggregate is the closure the aggregate owns, the
///   same way: the struct frees the block when it dies, so `let g = h.f`
///   has to keep `h` alive for as long as `g` is called (#1334).
/// - An aggregate with one of those in it, for the same reason: `(n, rest)`
///   read off a `(u32, Vec<u8>) or E` is the vector the result owns, and
///   counted as unrelated the result was released, vector and all, before
///   `rest` was read.
fn part_of_field(
    dst: LocalId,
    base: LocalId,
    aggregates: &HashSet<LocalId>,
    ty_of: &HashMap<LocalId, MirType>,
) -> bool {
    if !aggregates.contains(&base) {
        return false;
    }
    match ty_of.get(&dst) {
        Some(t) if aggregates.contains(&dst) && (aggregate_may_hold_string(t) || holds_a_handle(t)) => true,
        Some(MirType::Ptr | MirType::Container(_) | MirType::Heap(_) | MirType::FuncPtr(_) | MirType::InterfaceObject { .. }) => true,
        _ => false,
    }
}

/// A tuple or array with a container handle in it.
///
/// Not a `T?` or `T or E`: one of those read off an aggregate is a view of it
/// (`view_of_field`), and counted as a part as well it was handed on and
/// never released.
fn holds_a_handle(ty: &MirType) -> bool {
    let slot = |t: &MirType| matches!(t, MirType::Ptr | MirType::Container(_)) || holds_a_handle(t);
    match ty {
        MirType::Tuple(elems) => elems.iter().any(slot),
        MirType::Array { elem, .. } => slot(elem),
        _ => false,
    }
}

/// Whether reading a field of `base` into `dst` reaches into `base`'s storage
/// without being a part of it, rather than copying a scalar out of it.
///
/// - A wrapper read off one carries a handle with it:
///   `h.v!` on a `Vec<i64>?` field reaches the vector through it.
/// - Anything read off something that already reads into an aggregate is
///   still reading into it, whatever MIR types it as: `h.nested.get(0)? as
///   first` read `first.len()` after the release had freed `h` and gave
///   5775375445721207872.
///
/// A plain scalar is none of these, and admitting one holds the release back
/// to that scalar's last use for nothing.
fn view_of_field(
    dst: LocalId,
    base: LocalId,
    aggregates: &HashSet<LocalId>,
    ty_of: &HashMap<LocalId, MirType>,
) -> bool {
    if !aggregates.contains(&base) {
        return true;
    }
    matches!(
        ty_of.get(&dst),
        Some(MirType::Option(_)) | Some(MirType::Result { .. })
    )
}

/// A store too narrow to be replacing anything the release walks.
///
/// The narrowest thing that walk ever frees is a container handle at eight
/// bytes; a string's header is sixteen. So a one-byte store is a flag being
/// written and the strings and containers beside it are exactly where they
/// were — `opts.ignore_case = true` is not the end of the six-field struct
/// around it, and counting it as one put the release inside the loop that
/// writes the flags (#1198).
///
/// Asked of the store's own recorded width rather than the operand's type,
/// because that width is what codegen copies and a `None` there means "as wide
/// as the value", which is not a claim this can act on.
///
/// A tag is the exception: an enum's, a `T?`'s or `T or E`'s, a union's
/// member index. It is a byte or two, and writing it is how the value is made
/// (or switched to another variant), whatever its width: `Thing.Plain` and
/// `none` are that one store and nothing else. Read as a flag update, the
/// value was never made on that path, owned on the other, and left unreleased
/// at the join. The tag is always at offset 0, where no payload can be.
fn store_is_narrow(stmt: &MirStmt, ty_of: &HashMap<LocalId, MirType>) -> bool {
    match &stmt.kind {
        MirStmtKind::Store { addr, offset: 0, .. }
            if matches!(
                ty_of.get(addr),
                Some(MirType::Enum(_) | MirType::Option(_) | MirType::Result { .. } | MirType::Union(_))
            ) => false,
        MirStmtKind::Store { store_size: n, .. } => *n < 8,
        _ => false,
    }
}

/// Shapes that can hold a string somewhere inside them. The layouts that would
/// settle it live in codegen, so this only rules out what it can.
pub(crate) fn aggregate_may_hold_string(ty: &MirType) -> bool {
    match ty {
        MirType::Struct(_) | MirType::Enum(_) => true,
        MirType::Tuple(elems) => elems.iter().any(slot_is_releasable),
        MirType::Array { elem, .. } => slot_is_releasable(elem),
        MirType::Option(inner) => slot_is_releasable(inner),
        MirType::Result { ok, err } => slot_is_releasable(ok) || slot_is_releasable(err),
        _ => false,
    }
}

/// A container somewhere inside this type, under a tag or at an offset —
/// not a bare one, which is `container_drop`'s.
fn wraps_a_container(ty: &MirType) -> bool {
    let part = |t: &MirType| matches!(t, MirType::Container(_)) || wraps_a_container(t);
    match ty {
        MirType::Option(inner) => part(inner),
        MirType::Result { ok, err } => part(ok) || part(err),
        MirType::Tuple(elems) | MirType::Union(elems) => elems.iter().any(part),
        MirType::Array { elem, .. } => part(elem),
        _ => false,
    }
}

/// Is there something to give back in a slot of this type?
///
/// A `Heap<T>` slot is: storing a block in an aggregate moves it in
/// (mem.heap/HP4), so the aggregate gives it back, and a tuple of them had no
/// release emitted at all. A *bare* `Heap` local is not — that one is the
/// obligation itself and `drop` is what discharges it. Releasing it here as
/// well freed an `own` closure's captured block a second time.
fn slot_is_releasable(ty: &MirType) -> bool {
    *ty == MirType::Buffer
        || matches!(ty, MirType::Heap(_))
        // A closure in a slot is the same case as a block in one. A *bare*
        // closure local isn't — `ClosureDrop` discharges that one, and the
        // escape analysis has already said the frame no longer owns a closure
        // it stored into an aggregate, so this is the only release it gets
        // (#1253).
        || matches!(ty, MirType::FuncPtr(_))
        || aggregate_may_hold_string(ty)
}

/// Take a reference before handing a borrowed parameter back to the caller.
///
/// The caller keeps its own and releases it at its own last use, so `return s`
/// on a `s: string` parameter would give the caller a second name for a buffer
/// with one reference — `let b = id(a)` then frees it twice. Anything else
/// returned is a value this function owns, and returning it moves that
/// ownership out, which is why `insert_rc_dec` skips the release there.
///
/// A string read out of a container is the same: `return names[0]` hands
/// the caller the vector's own reference, and the caller releases it as its
/// own.
/// A closure body's names for variables it reaches through its environment.
/// The variable's home holds the reference: the frame that built the
/// closure, or the environment's drop glue. A task block's capture
/// (`Taken`) isn't one; the block owns it.
fn env_names(func: &MirFunction) -> HashSet<LocalId> {
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::LoadCapture { dst, access, .. } if *access != crate::CaptureAccess::Taken => Some(*dst),
            _ => None,
        })
        .collect()
}

fn retain_returned_params(func: &mut MirFunction, string_locals: &[LocalId], own: &HashSet<String>) {
    let strings: HashSet<LocalId> = string_locals.iter().copied().collect();
    let mut params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    params.extend(string_views(func, &strings, own));
    // So is a closure's name for a variable in its environment: the
    // environment keeps its reference for the next call. Returned without
    // one, `g(s)` returning `|| s` gave each caller the environment's only
    // reference, and the second call returned freed bytes.
    params.extend(env_names(func));

    for block in &mut func.blocks {
        let returned = match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => *id,
            _ => continue,
        };
        if !params.contains(&returned) || !strings.contains(&returned) {
            continue;
        }
        let span = block.terminator.span;
        block.statements.push(MirStmt::new(MirStmtKind::RcInc { local: returned }, span));
    }
}

/// Names given their own reference after their one definition, where the
/// analysis still sees the name as a view of what it was read from: `rc_inc`
/// of a string, and an `rc_inc_contents` taken for a hand-off. Each maps to
/// where the retain is, block and statement. With a single definition before
/// it, every path through the retain has the retained value from there on.
///
/// Not a lowering's `rc_inc_contents` of a tracked aggregate: that one is a
/// `Make`, so the name already stands for a value of its own, and returning
/// it is an ordinary hand-over. Counted here, the return stopped handing it
/// over and the pass released it just before `return`: `return l` out of a
/// Copy struct parameter gave the caller freed strings.
fn own_reference_points(func: &MirFunction, tracked: &HashSet<LocalId>) -> HashMap<LocalId, (BlockId, usize)> {
    let mut defs: HashMap<LocalId, (u32, BlockId, usize)> = HashMap::new();
    for block in &func.blocks {
        for (si, st) in block.statements.iter().enumerate() {
            if let Some(d) = uses::stmt_def(st) {
                let e = defs.entry(d).or_insert((0, block.id, si));
                e.0 += 1;
            }
        }
    }
    let mut out = HashMap::new();
    for block in &func.blocks {
        for (si, st) in block.statements.iter().enumerate() {
            let local = match st.kind {
                MirStmtKind::RcIncContents { local, for_hand_off: false } if tracked.contains(&local) => continue,
                MirStmtKind::RcInc { local } | MirStmtKind::RcIncContents { local, .. } => local,
                _ => continue,
            };
            let Some(&(1, db, di)) = defs.get(&local) else { continue };
            if db == block.id && di > si {
                continue;
            }
            out.entry(local).or_insert((block.id, si));
        }
    }
    out
}

/// String locals that point into storage a receiver keeps: the result of a
/// call that hands out a view.
///
/// Not copies of one. `insert_rc_inc` gives every copy a reference of its own,
/// so a copy is the frame's to hand on; the call result is the only name that
/// holds none.
fn string_views(func: &MirFunction, strings: &HashSet<LocalId>, own: &HashSet<String>) -> HashSet<LocalId> {
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|stmt| match &stmt.kind {
            MirStmtKind::Call { func: fref, dst: Some(dst), args }
                if strings.contains(dst)
                    && !args.is_empty()
                    && crate::own_names::returns_a_view(fref, own) =>
            {
                Some(*dst)
            }
            // An element bound where it sits: the collection keeps it.
            MirStmtKind::BindPlace { dst, .. } if strings.contains(dst) => Some(*dst),
            _ => None,
        })
        .collect()
}

/// Take a reference before giving a borrowed string parameter to a call that
/// keeps it.
///
/// A local string handed to `v.push(s)` moves its reference into the vector:
/// nothing is retained and nothing released after. A parameter has no
/// reference of its own to move, since the caller keeps one and releases it
/// at its own last use. So `func add(mutate v: Vec<string>, s: string) {
/// v.push(s) }` gave the vector a string with one count and two owners, and
/// once the caller let go, `v[0]` read freed bytes.
fn retain_params_handed_over(
    func: &mut MirFunction,
    string_locals: &[LocalId],
    own: &HashSet<String>,
) {
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    let strings: HashSet<LocalId> = string_locals.iter().copied().collect();
    for block in &mut func.blocks {
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();
        for (si, stmt) in block.statements.iter().enumerate() {
            let MirStmtKind::Call { func: fref, args, .. } = &stmt.kind else { continue };
            for (i, arg) in args.iter().enumerate() {
                let Some(id) = uses::operand_local(arg) else { continue };
                if params.contains(&id)
                    && strings.contains(&id)
                    && crate::own_names::keeps_argument(fref, i, own)
                {
                    insertions.push((si, MirStmt::new(MirStmtKind::RcInc { local: id }, stmt.span)));
                }
            }
        }
        for (idx, stmt) in insertions.into_iter().rev() {
            block.statements.insert(idx, stmt);
        }
    }
}

/// How many of `stmt`'s arguments pass `local` to a parameter the callee
/// keeps. `m.insert(x, x)` is two: the map holds two references.
fn keeper_slots(stmt: &MirStmt, local: LocalId, own: &HashSet<String>) -> usize {
    let MirStmtKind::Call { func: fref, args, .. } = &stmt.kind else { return 0 };
    args.iter()
        .enumerate()
        .filter(|(i, arg)| uses::operand_local(arg) == Some(local) && crate::own_names::keeps_argument(fref, *i, own))
        .count()
}

/// Take a reference before giving a string this frame owns to a call that
/// keeps it, when the frame still uses the string afterwards.
///
/// The hand-over moves the frame's reference into the keeper, which is right
/// at the string's last use and is why `insert_rc_dec` releases nothing there.
/// Anywhere earlier, the frame still needs its own: `v.push(s)` in a loop, or
/// `v.push(s)` then `println(s)`, gave the vector copies of one reference and
/// the second free read a dead buffer (#1341). The frame's reference covers
/// one slot, so a call that keeps the string twice (`m.insert(x, x)`) takes
/// one more for each further slot (#2029).
fn retain_locals_handed_over_while_live(
    func: &mut MirFunction,
    string_locals: &[LocalId],
    own: &HashSet<String>,
) {
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    let strings: HashSet<LocalId> =
        string_locals.iter().copied().filter(|l| !params.contains(l)).collect();
    // A view has no reference to move, so the keeper always needs one taken
    // for it: `out.push(names[0])` gave `out` the vector's own reference, and
    // both vectors released it.
    let views = string_views(func, &strings, own);
    let live = liveness::analyze_phis_on_edges(func);
    let aliases = AddrAliases::build(func);
    for block in &mut func.blocks {
        let mut insertions: Vec<(usize, MirStmt)> = Vec::new();
        for (si, stmt) in block.statements.iter().enumerate() {
            let MirStmtKind::Call { .. } = &stmt.kind else { continue };
            for &local in &strings {
                let slots = keeper_slots(stmt, local, own);
                if slots == 0 {
                    continue;
                }
                let retain = MirStmt::new(MirStmtKind::RcInc { local }, stmt.span);
                for _ in 1..slots {
                    insertions.push((si, retain.clone()));
                }
                if views.contains(&local) {
                    insertions.push((si, retain));
                    continue;
                }
                let mut used_after = None;
                for later in &block.statements[si + 1..] {
                    if aliases.stmt_reads(later, local) {
                        used_after = Some(true);
                        break;
                    }
                    if uses::stmt_def(later) == Some(local) {
                        used_after = Some(false);
                        break;
                    }
                }
                let used_after = used_after.unwrap_or_else(|| {
                    aliases.terminator_reads(&block.terminator, local)
                        || live.live_at_exit(block.id, local)
                });
                if used_after {
                    insertions.push((si, retain));
                }
            }
        }
        for (idx, stmt) in insertions.into_iter().rev() {
            block.statements.insert(idx, stmt);
        }
    }
}

/// Release each string where it stops being live.
///
/// A string holds one reference, and the reference is given back at the
/// point its value dies. There are two kinds of place that can be:
///
/// - Inside a block, just after the statement that reads it for the last
///   time, or just after the one that makes it when nothing ever reads it.
/// - On an edge. A block with several successors can have a string live on
///   its way out because one successor reads it, while another doesn't: the
///   value dies on the way into that one. A `mut` string reassigned in a loop
///   and not read after it is the shape: live out of the loop header, dead in
///   the block after the loop. So is a string an `assert` would print if it
///   failed, dead on the branch that carries on.
///
/// Both come from one liveness answer, the edge-aware one: a phi reads its
/// operand on the edge it arrives by, and takes over that operand's reference
/// there rather than copying it (`insert_rc_inc` adds no count for a phi). So
/// a value handed to a phi is not dead on that edge, and one the phi doesn't
/// take is.
///
/// This used to be four rules: a last use in a block, a release before a
/// redefinition, one at a loop's back edge, and one on the branch beside an
/// abort. The last three were each a case of dying on an edge, placed
/// somewhere that happened to work for the shapes that found them; two of
/// them fired for the same death in a loop that overwrote a string, and freed
/// it twice.
fn insert_rc_dec(func: &mut MirFunction, string_locals: &[LocalId], own: &HashSet<String>) {
    // Where each string dies is asked of a view in which filling a variable a
    // closure captures by reference *defines* it (`places_as_definitions`).
    // The view has the same statements at the same positions, so the answers
    // place releases in `func` itself.
    let (view, places) = places_as_definitions(func);
    let live = liveness::analyze_phis_on_edges(&view);
    // `s as i64` into an unsafe call hands out the address of `s`, and the
    // native callee reads the buffer through it. Counting only the cast as a
    // use released the buffer one statement before the call read it (#1036).
    let aliases = AddrAliases::build(&view);
    // A string parameter is borrowed from the caller, which keeps its own
    // reference and releases it at its own last use. Releasing here as well is
    // two releases for one reference; a callee that needs to outlive the call
    // takes its own reference (storing incs, and returning a parameter incs in
    // `retain_returned_params`).
    let params: HashSet<LocalId> = func.params.iter().map(|p| p.id).collect();
    // Nor a view: `v[i]` points at the vector's own reference and holds none.
    // Releasing one at its last use gave back the vector's — `rc_elide` took
    // most of those back out again, but not one whose view was stored, so
    // `words.enumerate()` built a pair holding a string with no reference of
    // its own and freed the vector's string with the pair.
    let strings: HashSet<LocalId> = string_locals.iter().copied().collect();
    let views = string_views(func, &strings, own);
    // A variable an `ensure` body reads is still needed when the cleanup
    // chain runs, after every exit; `container_drop`'s cell rule releases it
    // there instead.
    let ensured: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::EnsureHookRegister { captures, .. } => Some(captures),
            _ => None,
        })
        .flatten()
        .map(|c| c.local_id)
        .collect();
    // Nor a captured variable the frame never fills. The slot a `for` body's
    // `return` leaves its answer in is filled by the body, if at all, and
    // handed straight to the caller; on every other path it holds whatever
    // the stack did.
    let defined: HashSet<LocalId> = view
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(uses::stmt_def)
        .collect();
    let unfilled = |l: &LocalId| places.contains(l) && !defined.contains(l);
    let locals: Vec<LocalId> = string_locals
        .iter()
        .copied()
        .filter(|l| !params.contains(l) && !views.contains(l) && !ensured.contains(l) && !unfilled(l))
        .collect();
    // A closure's name for a variable it reaches through its environment
    // holds what the variable holds, and the variable's home releases that:
    // the frame that built the closure, or the environment's drop glue. The
    // name's last use isn't the value's. Only a task block's capture is its
    // own (`Taken`). A value the body writes over is still released where
    // it is overwritten (`overwritten_places`), since the home never sees it.
    //
    // A closure built inside the body that borrows such a name made it look
    // like a variable of this frame, and `|| { let inner = || best.len() … }`
    // released the outer frame's `best` each time it ran.
    let env_names = env_names(func);
    let dying: Vec<LocalId> = locals.iter().copied().filter(|l| !env_names.contains(l)).collect();

    let mut in_block: Vec<(usize, usize, LocalId)> = Vec::new();
    for (bi, block) in view.blocks.iter().enumerate() {
        for &local in &dying {
            if let Some(at) =
                dies_in_block(block, local, live.live_at_exit(block.id, local), &aliases, own)
            {
                in_block.push((bi, at, local));
            }
        }
    }

    let overwritten: Vec<LocalId> = locals.iter().copied().filter(|l| places.contains(l)).collect();
    in_block.extend(overwritten_places(&view, &overwritten, &aliases, own));

    let edges = edge_deaths(&view, &live, &dying);

    // Positions descending within a block, so an insertion doesn't move the
    // next one's index.
    in_block.sort_by(|a, b| (a.0, b.1).cmp(&(b.0, a.1)));
    for (bi, at, local) in in_block {
        let span = func.blocks[bi]
            .statements
            .get(at.saturating_sub(1))
            .map(|s| s.span)
            .unwrap_or(func.blocks[bi].terminator.span);
        func.blocks[bi].statements.insert(at, MirStmt::new(MirStmtKind::RcDec { local }, span));
    }
    let edges = edges
        .into_iter()
        .map(|(from, to, locals)| {
            let span = func
                .blocks
                .iter()
                .find(|b| b.id == from)
                .map(|b| b.terminator.span)
                .unwrap_or(crate::Span::new(0, 0));
            let releases =
                locals.iter().map(|&local| MirStmt::new(MirStmtKind::RcDec { local }, span)).collect();
            (from, to, releases)
        })
        .collect();
    ownership::insert_on_edges(func, edges);
}

/// `func` with each whole-width store into a string a closure captures by
/// reference written as the assignment it is.
///
/// A borrowing closure needs the variable at a fixed address, so
/// `addr_taken` turns every write to it into a store through that address.
/// To liveness a store is a use, never a definition, so such a variable
/// looked never defined: `mut seen = ""` captured by a loop body was
/// released by nobody once the loop was done with it. The variable is the
/// same variable either way; filling it defines it, and its last use (by name
/// or through the closure, `AddrAliases`) is where the value it holds dies,
/// whichever value the closure left there.
fn places_as_definitions(func: &MirFunction) -> (MirFunction, HashSet<LocalId>) {
    let places: HashSet<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::ClosureCreate { captures, .. } => Some(captures),
            _ => None,
        })
        .flatten()
        .filter(|c| c.by_ref && func.local_ty(c.local_id) == Some(&MirType::Buffer))
        .map(|c| c.local_id)
        .collect();
    let mut view = func.clone();
    if places.is_empty() {
        return (view, places);
    }
    for stmt in view.blocks.iter_mut().flat_map(|b| b.statements.iter_mut()) {
        if let MirStmtKind::Store { addr, offset: 0, value, .. } = &stmt.kind {
            if places.contains(addr) && uses::operand_local(value) != Some(*addr) {
                stmt.kind = MirStmtKind::Assign { dst: *addr, rvalue: MirRValue::Use(value.clone()) };
            }
        }
    }
    (view, places)
}

/// Deaths of values a captured variable held before it was filled again in
/// the same block. Only such a variable is filled more than once under one
/// name (`places_as_definitions`); every other local is renamed per
/// definition by SSA, so `dies_in_block`, which finds the death of the value
/// a block ends with, is all it needs.
///
/// Each value dies at its last read before the next filling — by name or
/// through the closure — or, if nothing reads it, right after it was made.
/// A value the block starts with is only there if something reads it first.
fn overwritten_places(
    view: &MirFunction,
    locals: &[LocalId],
    aliases: &AddrAliases,
    own: &HashSet<String>,
) -> Vec<(usize, usize, LocalId)> {
    let mut out = Vec::new();
    for (bi, block) in view.blocks.iter().enumerate() {
        for &local in locals {
            let defs: Vec<usize> = block
                .statements
                .iter()
                .enumerate()
                .filter(|(_, st)| uses::stmt_def(st) == Some(local))
                .map(|(i, _)| i)
                .collect();
            if defs.is_empty() {
                continue;
            }
            // Segments ending at each filling: from the previous filling (or
            // the block start) up to it.
            let mut start: Option<usize> = None;
            for &d in &defs {
                let from = start.map_or(0, |s| s + 1);
                let last_read = (from..d)
                    .rev()
                    .find(|&i| !matches!(block.statements[i].kind, MirStmtKind::Phi { .. })
                        && aliases.stmt_reads(&block.statements[i], local));
                match (last_read, start) {
                    (Some(i), _) => {
                        let stmt = &block.statements[i];
                        if keeper_slots(stmt, local, own) > 0 {
                            // Its last use gave the reference away.
                        } else {
                            out.push((bi, after_increments(block, i + 1), local));
                        }
                    }
                    (None, Some(s)) => out.push((bi, after_increments(block, s + 1), local)),
                    (None, None) => {}
                }
                start = Some(d);
            }
        }
    }
    out
}

/// Where in `block` the value of `local` dies, as the index to insert its
/// release at, or `None` when it doesn't die here: it's live on the way out,
/// it's returned, or this block never had it.
///
/// Scanned backwards from the block's exit, where the value is dead unless
/// `live_out`. The first statement met that reads it, or failing that the one
/// that makes it, is where it dies.
fn dies_in_block(
    block: &MirBlock,
    local: LocalId,
    live_out: bool,
    aliases: &AddrAliases,
    own: &HashSet<String>,
) -> Option<usize> {
    if live_out {
        return None;
    }
    // A returned string is handed to the caller, not dropped. Decrementing it
    // here freed the buffer while the caller still held the only reference —
    // `return json.encode(v)` from a `string or E` function came back with its
    // first eight bytes overwritten by whatever the caller allocated next
    // (#499).
    let returned = matches!(
        &block.terminator.kind,
        MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. }
            if id == &local
    );
    if returned {
        return None;
    }
    let n = block.statements.len();
    if aliases.terminator_reads(&block.terminator, local) {
        return Some(n);
    }
    for (si, stmt) in block.statements.iter().enumerate().rev() {
        // A phi reads its operand on the incoming edge, not here, and hands
        // the reference over to its own name there.
        let phi = matches!(stmt.kind, MirStmtKind::Phi { .. });
        let reads = !phi && aliases.stmt_reads(stmt, local);
        let defines = uses::stmt_def(stmt) == Some(local);
        if reads && defines {
            // Reads the old value and writes the new one in one statement, so
            // no point after it names the old one. Lowering puts a fresh
            // local between the two, so this doesn't arise.
            return None;
        }
        if reads {
            // A read through an address counts (`AddrAliases`): `s.as_ptr()`
            // or `s as i64` hands out the buffer's address, and whatever reads
            // that address later reads `s`. Releasing at the hand-out freed
            // the buffer under it; `write_raw` in `stdlib/http.rk` answered
            // with eight bytes of allocator free-list where `HTTP/1.1` should
            // be (#1036, #1118).
            //
            // The last use gives the reference to a call that keeps it, so
            // there is nothing left here to release.
            if keeper_slots(stmt, local, own) > 0 {
                return None;
            }
            return Some(after_increments(block, si + 1));
        }
        if defines {
            // Made and never read: dead as soon as it exists.
            return Some(after_increments(block, si + 1));
        }
    }
    None
}

/// Step over the increments the copy pass put at `at`. At `dst = src`, `src`'s
/// last use is the copy itself, so the naive spot is directly between
/// `dst = src` and `RcInc(dst)` — the release runs first, the buffer hits
/// zero, and the increment that was meant to keep it alive touches freed
/// memory.
fn after_increments(block: &MirBlock, mut at: usize) -> usize {
    while at < block.statements.len() && matches!(block.statements[at].kind, MirStmtKind::RcInc { .. }) {
        at += 1;
    }
    at
}

/// Each `(from, to)` edge a string dies on, with the strings.
///
/// Dead on the edge means live on the way out of `from`, because some
/// successor needs it, and not needed by `to`: not live into it, and not an
/// operand its phis take along this edge.
///
/// Only where the local is certainly assigned on the way out of `from`, so it
/// holds a value whichever path got there. A local SSA renamed has one
/// definition that dominates every use; one it left alone can be written in
/// each arm of a branch and read after the join, with no single definition
/// dominating anything, and one read on a path that never wrote it (a match's
/// impossible default arm) must not be released there.
///
/// Not the edges a `CleanupReturn` names. Those run the `ensure` chain on the
/// way out of the function, and a value returned through one flows through
/// them to the caller.
fn edge_deaths(
    func: &MirFunction,
    live: &liveness::LivenessResults,
    locals: &[LocalId],
) -> Vec<(BlockId, BlockId, Vec<LocalId>)> {
    let mut phi_takes: HashMap<(BlockId, BlockId), HashSet<LocalId>> = HashMap::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            let MirStmtKind::Phi { args, .. } = &stmt.kind else { continue };
            for (from, op) in args {
                if let Some(id) = uses::operand_local(op) {
                    phi_takes.entry((*from, block.id)).or_default().insert(id);
                }
            }
        }
    }
    let assigned = assigned_on_exit(func, locals);

    let mut out = Vec::new();
    for block in &func.blocks {
        if matches!(block.terminator.kind, MirTerminatorKind::CleanupReturn { .. }) {
            continue;
        }
        let mut succs = cfg::successors(&block.terminator);
        succs.sort_by_key(|b| b.0);
        succs.dedup();
        if succs.len() < 2 {
            continue; // one way out: live out and live in are the same set
        }
        for &succ in &succs {
            let taken = phi_takes.get(&(block.id, succ));
            let dying: Vec<LocalId> = locals
                .iter()
                .copied()
                .filter(|&l| live.live_at_exit(block.id, l))
                .filter(|&l| !live.live_at_entry(succ, l) && !taken.is_some_and(|t| t.contains(&l)))
                .filter(|l| assigned.get(&block.id).is_some_and(|a| a.contains(l)))
                .collect();
            if !dying.is_empty() {
                out.push((block.id, succ, dying));
            }
        }
    }
    out
}

/// For each block, the `locals` certainly assigned on the way out of it: on
/// every path from the entry, something wrote them. Forward, and an
/// intersection over predecessors, so a path that skips the write keeps the
/// local out.
fn assigned_on_exit(func: &MirFunction, locals: &[LocalId]) -> HashMap<BlockId, HashSet<LocalId>> {
    let preds = cfg::predecessors(func);
    let everything: HashSet<LocalId> = locals.iter().copied().collect();
    let writes: HashMap<BlockId, HashSet<LocalId>> = func
        .blocks
        .iter()
        .map(|b| {
            let w = b.statements.iter().filter_map(uses::stmt_def).filter(|d| everything.contains(d)).collect();
            (b.id, w)
        })
        .collect();
    // Start from "everything" and shrink, so a loop's back edge doesn't wipe
    // out what the way in established.
    let mut out: HashMap<BlockId, HashSet<LocalId>> =
        func.blocks.iter().map(|b| (b.id, everything.clone())).collect();
    loop {
        let mut changed = false;
        for block in &func.blocks {
            let mut inn: HashSet<LocalId> = if block.id == func.entry_block {
                HashSet::new()
            } else {
                let mut sets = preds.get(&block.id).into_iter().flatten().filter_map(|p| out.get(p));
                match sets.next() {
                    Some(first) => sets.fold(first.clone(), |acc, s| acc.intersection(s).copied().collect()),
                    // No path reaches it, so "on every path" holds for
                    // anything. Lowering leaves such a block after a
                    // `continue` that falls into the code below; counting it
                    // as assigning nothing kept every string out of the
                    // join, and `let t = …; x ?? { continue }` never
                    // released `t` on the way out (markdown_renderer).
                    None => everything.clone(),
                }
            };
            inn.extend(writes[&block.id].iter().copied());
            if out.get(&block.id) != Some(&inn) {
                out.insert(block.id, inn);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlockId, FunctionRef, MirBlock, MirConst, MirLocal, MirOperand, MirRValue, MirStmt,
        MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
    };

    fn local(id: u32) -> LocalId { LocalId(id) }

    fn string_local(id: u32, name: &str) -> MirLocal {
        MirLocal { id: local(id), name: Some(name.into()), ty: MirType::Buffer, is_param: false }
    }

    fn make_fn(locals: Vec<MirLocal>, blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals,
            blocks,
            entry_block: BlockId(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
            source_file: None,
        }
    }

    fn has_rc_inc(stmts: &[MirStmt], target: LocalId) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, MirStmtKind::RcInc { local } if *local == target))
    }

    fn has_rc_dec(stmts: &[MirStmt], target: LocalId) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, MirStmtKind::RcDec { local } if *local == target))
    }

    fn count_rc_inc(stmts: &[MirStmt]) -> usize {
        stmts.iter().filter(|s| matches!(&s.kind, MirStmtKind::RcInc { .. })).count()
    }

    fn count_rc_dec(stmts: &[MirStmt]) -> usize {
        stmts.iter().filter(|s| matches!(&s.kind, MirStmtKind::RcDec { .. })).count()
    }

    /// A string parameter gets no release at all, and storing it takes a
    /// reference of its own.
    ///
    /// It used to get one, which was already one too many: the caller keeps its
    /// own reference and releases it at its own last use, so a callee that
    /// releases as well is two releases for one reference. (Before that it got
    /// *two*, because the pass walked `params` and `locals` back to back and
    /// visited every parameter twice — #698. That was the visible half of the
    /// same mistake; the elision pass hid the other half by deleting both.)
    ///
    /// `self.last = title` still needs the increment: the field outlives the
    /// call, so it takes a reference the caller isn't going to give up.
    #[test]
    fn string_param_is_borrowed_and_stores_retain() {
        let param = MirLocal {
            id: local(1),
            name: Some("title".into()),
            ty: MirType::Buffer,
            is_param: true,
        };
        let mut f = MirFunction {
            name: "put".to_string(),
            params: vec![param.clone()],
            ret_ty: MirType::Void,
            locals: vec![
                MirLocal { id: local(0), name: Some("self".into()), ty: MirType::Ptr, is_param: true },
                param,
            ],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Store {
                    addr: local(0),
                    offset: 0,
                    value: MirOperand::Local(local(1)),
                    store_size: 16,
                    field: None,
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
            source_file: None,
        };
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());
        let stmts = &f.blocks[0].statements;
        assert_eq!(count_rc_inc(stmts), 1, "one inc for the store: {stmts:?}");
        assert_eq!(count_rc_dec(stmts), 0, "a parameter is borrowed: {stmts:?}");
    }

    /// `for p in rows.enumerate()`: the pair is the turn's index beside an
    /// element bound where it sits. It holds nothing of its own, so it is
    /// neither retained nor released. Retaining a pair whose element holds a
    /// `Vec` deep-cloned the vector each turn (#2048).
    #[test]
    fn a_pair_around_a_bound_element_is_a_view() {
        let elem_ty = MirType::Tuple(vec![MirType::Buffer, MirType::Buffer]);
        let pair_ty = MirType::Tuple(vec![MirType::I64, elem_ty.clone()]);
        let mut f = make_fn(
            vec![
                MirLocal { id: local(0), name: Some("at".into()), ty: MirType::Ptr, is_param: false },
                MirLocal { id: local(1), name: Some("row".into()), ty: elem_ty, is_param: false },
                MirLocal { id: local(2), name: Some("i".into()), ty: MirType::I64, is_param: false },
                MirLocal { id: local(3), name: Some("p".into()), ty: pair_ty, is_param: false },
                MirLocal { id: local(4), name: Some("n".into()), ty: MirType::I64, is_param: false },
            ],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(0)),
                        func: FunctionRef::internal("slot".into()),
                        args: vec![],
                    }),
                    MirStmt::dummy(MirStmtKind::BindPlace { dst: local(1), addr: local(0) }),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(2),
                        rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(0))),
                    }),
                    MirStmt::dummy(MirStmtKind::Store {
                        addr: local(3),
                        offset: 0,
                        value: MirOperand::Local(local(2)),
                        store_size: 8,
                        field: None,
                    }),
                    MirStmt::dummy(MirStmtKind::Store {
                        addr: local(3),
                        offset: 8,
                        value: MirOperand::Local(local(1)),
                        store_size: 32,
                        field: None,
                    }),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(4),
                        rvalue: MirRValue::Field {
                            base: MirOperand::Local(local(3)),
                            field_index: 0,
                            byte_offset: Some(0),
                            access: crate::operand::FieldAccess::Word,
                        },
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());
        let stmts = &f.blocks[0].statements;
        assert!(
            !stmts.iter().any(|s| matches!(s.kind, MirStmtKind::RcIncContents { .. } | MirStmtKind::RcDecContents { .. })),
            "the pair only views the element: {stmts:?}"
        );
    }

    /// `unsafe { native_fn(fd, s as i64) }` — the release belongs after the
    /// call, not after the cast.
    ///
    /// The cast is the last statement that names `s`, so the naive last-use
    /// scan put the release between the cast and the call. The buffer hit zero
    /// and the allocator wrote its free-list link into the first eight bytes,
    /// which the native call then wrote to the socket: every HTTP response
    /// started with eight bytes of garbage (#1036).
    #[test]
    fn release_follows_the_call_that_reads_the_address() {
        let mut f = make_fn(
            vec![
                string_local(0, "s"),
                MirLocal { id: local(1), name: Some("addr".into()), ty: MirType::I64, is_param: false },
            ],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(0)),
                        func: FunctionRef::internal("build".into()),
                        args: vec![],
                    }),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Cast {
                            value: MirOperand::Local(local(0)),
                            target_ty: MirType::I64,
                        },
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::extern_c("rask_io_write_string".into()),
                        args: vec![
                            MirOperand::Constant(MirConst::Int(1)),
                            MirOperand::Local(local(1)),
                        ],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());

        let stmts = &f.blocks[0].statements;
        let dec = stmts
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::RcDec { local: l } if *l == local(0)))
            .expect("string is released somewhere: {stmts:?}");
        let write = stmts
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::Call { func, .. } if func.name == "rask_io_write_string"))
            .unwrap();
        assert!(dec > write, "release must follow the native write: {stmts:?}");
    }

    /// `unsafe { strlen(s.as_ptr()) }` — the method form of the same thing.
    ///
    /// `as_ptr` is a call, not a cast, so the rule above never covered it: the
    /// release landed between taking the address and using it, and libc read a
    /// freed buffer. Most of the time freed memory still holds the same bytes,
    /// which is why this went unseen — `strlen` on a string with a NUL at byte
    /// 3 answered 21 (#1118).
    #[test]
    fn release_follows_the_call_that_reads_a_pointer_from_as_ptr() {
        let mut f = make_fn(
            vec![
                string_local(0, "s"),
                MirLocal { id: local(1), name: Some("p".into()), ty: MirType::Ptr, is_param: false },
                MirLocal { id: local(2), name: Some("n".into()), ty: MirType::U64, is_param: false },
            ],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(0)),
                        func: FunctionRef::internal("build".into()),
                        args: vec![],
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(1)),
                        // Lowering marks a call answering a raw pointer; the
                        // name says nothing (mem.unsafe/PT1).
                        func: FunctionRef { points_into_args: true, ..FunctionRef::internal("string_as_ptr".into()) },
                        args: vec![MirOperand::Local(local(0))],
                    }),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: Some(local(2)),
                        func: FunctionRef::extern_c("strlen".into()),
                        args: vec![MirOperand::Local(local(1))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());

        let stmts = &f.blocks[0].statements;
        let dec = stmts
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::RcDec { local: l } if *l == local(0)))
            .expect("string is released somewhere");
        let read = stmts
            .iter()
            .position(|s| matches!(&s.kind, MirStmtKind::Call { func, .. } if func.name == "strlen"))
            .unwrap();
        assert!(dec > read, "release must follow the read through the pointer: {stmts:?}");
    }

    /// A string the aborting branch builds is not the surviving branch's to
    /// release.
    ///
    /// The edge release exists for a value whose only remaining reader is a
    /// branch that panics — releasing it on the branch that carries on is the
    /// point. It is wrong when that branch is also where the value is *built*:
    /// on the surviving side the slot was never written, and
    /// `rask_string_free` read an uninitialised header. `combined("42", 2)!`
    /// succeeded and still did it, because the panic branch's
    /// `"parse error: …"` was released on the success branch (#1121).
    #[test]
    fn a_string_built_only_where_it_panics_is_not_released_where_it_does_not() {
        let mut f = make_fn(
            vec![
                MirLocal { id: local(0), name: Some("c".into()), ty: MirType::Bool, is_param: false },
                string_local(1, "msg"),
            ],
            vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(0),
                        rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Bool(true))),
                    })],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(local(0)),
                        then_block: BlockId(1),
                        else_block: BlockId(2),
                    }),
                },
                // Carries on. Never sees `msg`.
                MirBlock {
                    id: BlockId(1),
                    statements: vec![],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
                },
                // Builds the message and dies.
                MirBlock {
                    id: BlockId(2),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(local(1)),
                            func: FunctionRef::internal("build_message".into()),
                            args: vec![],
                        }),
                        MirStmt::dummy(MirStmtKind::Call {
                            dst: None,
                            func: FunctionRef::reading("panic_forced_error"),
                            args: vec![MirOperand::Local(local(1))],
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Unreachable),
                },
            ],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());

        let surviving = &f.blocks[1].statements;
        assert!(
            !has_rc_dec(surviving, local(1)),
            "the branch that carries on never had the string: {surviving:?}",
        );
    }

    /// A half-built aggregate is not a dead one.
    ///
    /// `insert_aggregate_release` puts the release after the group's last use
    /// in a block, and a `Store` into the aggregate was counted as one — so a
    /// `try` that wraps its error released the outer Result after its tag had
    /// been written and before its payload had. `release_either` read the tag,
    /// took the err branch, and freed a string header made of stack garbage
    /// (#1122).
    #[test]
    fn a_store_into_an_aggregate_is_not_a_place_to_release_it() {
        let mut f = make_fn(
            vec![
                MirLocal {
                    id: local(0),
                    name: Some("r".into()),
                    ty: MirType::Result {
                        ok: Box::new(MirType::Buffer),
                        err: Box::new(MirType::Buffer),
                    },
                    is_param: false,
                },
                string_local(1, "payload"),
            ],
            vec![
                MirBlock {
                    id: BlockId(0),
                    statements: vec![
                        MirStmt::dummy(MirStmtKind::Call {
                            dst: Some(local(1)),
                            func: FunctionRef::internal("build".into()),
                            args: vec![],
                        }),
                        // The tag, and nothing else — the payload lands in the
                        // next block.
                        MirStmt::dummy(MirStmtKind::Store {
                            addr: local(0),
                            offset: 0,
                            value: MirOperand::Constant(MirConst::Int(1)),
                            store_size: 1,
                            field: None,
                        }),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Goto {
                        target: BlockId(1),
                    }),
                },
                MirBlock {
                    id: BlockId(1),
                    statements: vec![MirStmt::dummy(MirStmtKind::Store {
                        addr: local(0),
                        offset: 24,
                        value: MirOperand::Local(local(1)),
                        store_size: 8,
                        field: None,
                    })],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                        value: Some(MirOperand::Local(local(0))),
                    }),
                },
            ],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());

        let building = &f.blocks[0].statements;
        assert!(
            !building
                .iter()
                .any(|s| matches!(&s.kind, MirStmtKind::RcDecContents { local: l } if *l == local(0))),
            "nothing may release a Result whose payload isn't written yet: {building:?}",
        );
    }

    #[test]
    fn copy_inserts_rc_inc() {
        // dst = src (both strings) → RcInc on dst
        let mut f = make_fn(
            vec![string_local(0, "src"), string_local(1, "dst")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    // Use dst so it's live somewhere
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("print_string".into()),
                        args: vec![MirOperand::Local(local(1))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());
        assert!(has_rc_inc(&f.blocks[0].statements, local(1)));
    }

    #[test]
    fn call_result_no_rc_inc() {
        // dst = call(...) returning string → no RcInc (new allocation)
        let mut f = make_fn(
            vec![string_local(0, "s")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(local(0)),
                    func: FunctionRef::reading("string_new"),
                    args: vec![],
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());
        assert_eq!(count_rc_inc(&f.blocks[0].statements), 0);
    }

    #[test]
    fn last_use_inserts_rc_dec() {
        // src used, then dead → RcDec
        let mut f = make_fn(
            vec![string_local(0, "src"), string_local(1, "dst")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());
        // Both src and dst should get RcDec (src after copy, dst after block)
        assert!(has_rc_dec(&f.blocks[0].statements, local(0)));
        assert!(has_rc_dec(&f.blocks[0].statements, local(1)));
    }

    /// At `dst = src` the increment on the copy has to happen before the
    /// release of the original. Placed the other way round, a refcount of one
    /// hits zero, the buffer is freed, and the increment meant to keep it alive
    /// lands on freed memory.
    #[test]
    fn copy_increments_before_it_releases() {
        let mut f = make_fn(
            vec![string_local(0, "src"), string_local(1, "dst")],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());
        let stmts = &f.blocks[0].statements;
        let (src, dst) = (local(0), local(1));
        let inc = stmts.iter().position(|s|
            matches!(&s.kind, MirStmtKind::RcInc { local } if *local == dst));
        let dec = stmts.iter().position(|s|
            matches!(&s.kind, MirStmtKind::RcDec { local } if *local == src));
        assert!(inc.is_some() && dec.is_some(), "expected both ops: {stmts:?}");
        assert!(inc < dec, "inc on the copy must precede the release: {stmts:?}");
    }

    /// A phi reads its argument on the incoming edge, not in the phi's own
    /// block. Treating it as a use there put the release at the top of a loop
    /// header, where it runs on the first iteration — before anything has
    /// written the local it releases.
    #[test]
    fn phi_argument_is_not_a_use_in_the_header() {
        let header = MirBlock {
            id: BlockId(1),
            statements: vec![MirStmt::dummy(MirStmtKind::Phi {
                dst: local(0),
                args: vec![
                    (BlockId(0), MirOperand::Constant(MirConst::String("".into()))),
                    (BlockId(2), MirOperand::Local(local(1))),
                ],
            })],
            terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(2) }),
        };
        let body = MirBlock {
            id: BlockId(2),
            statements: vec![MirStmt::dummy(MirStmtKind::Call {
                dst: Some(local(1)),
                func: FunctionRef::reading("string_new"),
                args: vec![],
            })],
            terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) }),
        };
        let entry = MirBlock {
            id: BlockId(0),
            statements: vec![],
            terminator: MirTerminator::dummy(MirTerminatorKind::Goto { target: BlockId(1) }),
        };
        let mut f = make_fn(
            vec![string_local(0, "carried"), string_local(1, "fresh")],
            vec![entry, header, body],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());
        let header = f.blocks.iter().find(|b| b.id == BlockId(1)).unwrap();
        assert!(
            !has_rc_dec(&header.statements, local(1)),
            "no release for a phi argument in the header: {:?}", header.statements
        );
    }

    #[test]
    fn no_ops_for_non_string_locals() {
        let mut f = make_fn(
            vec![MirLocal { id: local(0), name: Some("x".into()), ty: MirType::I64, is_param: false }],
            vec![MirBlock {
                id: BlockId(0),
                statements: vec![MirStmt::dummy(MirStmtKind::Assign {
                    dst: local(0),
                    rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(42))),
                })],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_rc_ops(&mut f, &HashMap::new(), &HashMap::new(), &HashSet::new(), &|_, _, _, _| false, &HashSet::new(), &crate::CopyLayouts::default(), rask_ast::Target::default());
        assert_eq!(count_rc_inc(&f.blocks[0].statements), 0);
        assert_eq!(count_rc_dec(&f.blocks[0].statements), 0);
    }
}

/// Whether `func` takes a reference of its own whenever it hands parameter
/// `i` to a keeper, leaving the caller's untouched.
///
/// True of a borrowed or `mutate` Copy aggregate that may hold strings: the
/// caller didn't give it up, so a callee that stores it stores a copy. The
/// rc pass inserts those retains, and `params_a_callee_keeps` reads the
/// same answer as "not kept", so the caller goes on releasing its own.
pub(crate) fn retains_what_it_hands_on(func: &MirFunction, i: usize, copy_layouts: &crate::CopyLayouts) -> bool {
    let Some(p) = func.params.get(i) else { return false };
    let mode = func.param_modes.get(i);
    // A borrowed parameter that isn't Copy can only reach a keeper through a
    // generic body or a sequence's yield, where the checker lets a terminal
    // hand back the item it was lent (`find`). Either way the keeper gets a
    // copy, so the references are taken.
    matches!(mode, Some(ParamMode::Borrow | ParamMode::Mutate))
        && (aggregate_may_hold_string(&p.ty) || matches!(p.ty, MirType::InterfaceObject { .. }))
        && (copy_layouts.is_copy(&p.ty) || mode == Some(&ParamMode::Borrow))
}

/// The retain lowering placed after the store at `si`, when that store copies
/// a value into a binding that owns its copy (`let w = s` on a Copy array):
/// the store copies, the retain gives the copy references of its own, and
/// the source is still its holder's to release.
fn lowering_copy_retain(stmts: &[MirStmt], si: usize, owns_copy: &HashSet<LocalId>) -> Option<usize> {
    let MirStmtKind::Store { addr, .. } = &stmts[si].kind else { return None };
    if !owns_copy.contains(addr) {
        return None;
    }
    for (j, later) in stmts.iter().enumerate().skip(si + 1) {
        match &later.kind {
            MirStmtKind::RcIncContents { local, for_hand_off: false } if local == addr => return Some(j),
            MirStmtKind::UnwindArm { .. } | MirStmtKind::UnwindDisarm { .. } => {}
            _ => return None,
        }
    }
    None
}

/// The `mutate` parameters the frame holds for the call: aggregates that
/// aren't Copy. Lowering writes a whole new value into one as a new value in
/// the slot (`ZeroAggregate`, then the store), so the release pass decides
/// what the old one was. A Copy one stays the caller's storage, read as a
/// view (`retains_what_it_hands_on`).
pub(crate) fn held_mutate_params(func: &MirFunction, copy_layouts: &crate::CopyLayouts) -> Vec<LocalId> {
    func.params
        .iter()
        .zip(func.param_modes.iter())
        .filter(|(p, mode)| {
            **mode == ParamMode::Mutate
                && matches!(
                    p.ty,
                    MirType::Struct(_)
                        | MirType::Enum(_)
                        | MirType::Tuple(_)
                        | MirType::Array { .. }
                        | MirType::Option(_)
                        | MirType::Result { .. }
                )
                && !copy_layouts.is_copy(&p.ty)
        })
        .map(|(p, _)| p.id)
        .collect()
}
