// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Interface-object drop insertion.
//!
//! `InterfaceBox` heap-allocates and copies a concrete value once to build an
//! `any Interface` fat pointer; `InterfaceCall` reads through it without consuming.
//! Nothing else in the pipeline ever dropped it (#366) — every interface object
//! leaked its heap allocation unconditionally.
//!
//! Mirrors `closures.rs`'s escape analysis, applied to interface-object-typed
//! locals instead of closure locals: an interface object escapes by being
//! returned, stored, or passed as a call/method argument — any of those hand
//! ownership to something else, so dropping the original name would
//! double-free. A plain move to another local (`dst = Use(src)`, or merging
//! through a `Phi`) is not an escape — it's the same value under a new name,
//! so tracking follows the new name instead (a moved-from name is excluded
//! so only the name still holding the value at its death gets dropped). What's
//! left (created, only ever read through `InterfaceCall`, never hands off
//! ownership) gets a `InterfaceDrop` before the function returns and before each
//! loop back-edge it's still alive at.
//!
//! Tracking starts from `InterfaceBox` destinations only — not every local typed
//! as an interface object. Reading one back out of existing storage (a struct
//! field, a `Vec` element, an `Option` payload) produces a local with the
//! same type but not fresh ownership: it's the same heap pointer the
//! container still holds, so a temp created by `r.inner.handle()` and
//! another by `run(r.inner)` right after are two aliases of the one box, not
//! two owners. Treating both as droppable-because-typed double-freed it
//! (`tests/suite/t62_interface_object_positions.rk`'s struct-field test). Only
//! `InterfaceBox`, and whatever a chain of moves/phis carries forward from it, is
//! a fresh allocation this pass may decide to free.
//!
//! Unlike the closure pass, this doesn't refine call-argument escapes with
//! per-callee borrow info — any appearance as a call/method argument counts
//! as escaping. That undercounts what could safely be dropped (a value
//! borrowed by a callee and not stored still "escapes" here), but never
//! drops something a callee kept, which is the risk worth avoiding.
//!
//! Function parameters are excluded — a `InterfaceDrop` needs the block where its
//! value was defined (for the loop back-edge check below), and a param's
//! "definition" is the call site, not a statement in this function's body.
//! A parameter that isn't returned/stored/passed onward still leaks; narrower
//! than the general case, and left for a follow-up.

use std::collections::{HashMap, HashSet};

use crate::{
    BlockId, LocalId, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind,
    MirTerminatorKind, MirType,
};

/// Insert `InterfaceDrop` for every non-escaping interface-object local, across all functions.
pub fn insert_interface_drops(fns: &mut [MirFunction]) {
    // Which callees keep an argument, read off their bodies — the same map the
    // closure pass uses, and for the same reason. Passing a box to a function
    // used to count as handing it over, so `io.copy(src, dst)` left both
    // buffers to a callee that only reads through them: the boxed value's
    // containers were freed by nobody. It only looked fixed while `io.copy`
    // was small enough to inline, which is why one `io.copy` in a file was
    // clean and two leaked five buffers.
    //
    // `heap_captures_only: false` — a parameter captured by *any* closure
    // counts as escaping here. Erring that way leaks; erring the other way is
    // a double free.
    let callee_escapes = crate::closures::build_callee_escape_map(fns, false);
    // And which functions hand a fresh box *back*, which makes their caller the
    // owner. `return Boom.Bad` in a `-> i64 or Error` boxes the enum and
    // returns it, and the caller read the box out of the wrapper and dropped
    // nothing — a boxed error leaked on every failing call, which is every
    // `or Error` in the language.
    let hands_back = functions_handing_back_a_interface_box(fns);
    for func in fns.iter_mut() {
        insert_for_function(func, &callee_escapes, &hands_back);
    }
}

/// Functions whose return value is a fresh interface box the caller now owns.
///
/// The same question `closures::functions_handing_back_a_closure` asks, and a
/// fixed point for the same reason: handing one back is transitive, so a
/// wrapper that just forwards what it called joins the set on a later pass.
fn functions_handing_back_a_interface_box(fns: &[MirFunction]) -> HashSet<String> {
    let mut names: HashSet<String> = HashSet::new();
    loop {
        let before = names.len();
        for func in fns {
            if names.contains(&func.name) {
                continue;
            }
            let mut fresh = collect_fresh_interface_locals(func);
            // A box that came back from a call to something already in the set
            // is this frame's, and passing it on hands it over again. Which
            // *name* holds it is the same question the caller's side asks, so
            // ask it the same way: an interface-object destination is the box, and
            // a wrapper destination holds it in its payload.
            fresh.extend(boxes_handed_over(func, &names));
            fresh.extend(boxes_parked_in_a_wrapper(func, &fresh));
            if fresh.is_empty() {
                continue;
            }
            if hands_one_back(func, &fresh) {
                names.insert(func.name.clone());
            }
        }
        if names.len() == before {
            return names;
        }
    }
}

/// Whether a returning path gives the caller a box this frame owns.
///
/// Two shapes, the same two as on the caller's side. Returning the box is one.
/// Returning a *wrapper* with the box in its payload is the other: `-> i64 or
/// Error` hands the box back as the error side, so the returned local is the
/// wrapper and never the box itself. Only the first shape was recognised, so a
/// function that forwards what it called — `let v = try classify(n)`, which
/// reads the box out and repacks it into its own wrapper — was not in the set,
/// and its caller dropped nothing (#1147).
///
/// A wrapper holding a box this frame does *not* own settles the whole function
/// the other way. `return e` for a boxed parameter hands back something whose
/// owner is the caller already; calling that fresh frees it twice. A path that
/// stores no box at all — the ok side of a `T or E` — says nothing either way.
fn hands_one_back(func: &MirFunction, fresh: &HashSet<LocalId>) -> bool {
    let ty_of = local_types(func);
    let mut found = false;
    for block in &func.blocks {
        let returned = match &block.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => *id,
            _ => continue,
        };
        if fresh.contains(&returned) {
            found = true;
            continue;
        }
        for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            let MirStmtKind::Store { addr, value: MirOperand::Local(v), .. } = &stmt.kind else {
                continue;
            };
            if *addr != returned || !is_box(&ty_of, v) {
                continue;
            }
            if !fresh.contains(v) {
                return false;
            }
            found = true;
        }
    }
    found
}

/// A box parked in one of this frame's own wrappers and read back out.
///
/// `let v = try classify(n)` reads the box out of the callee's wrapper on the
/// error side and repacks it into a wrapper of its own. Once the forwarding
/// function is inlined — and it always is, it's three statements — both
/// wrappers are locals of one frame, so the box's trip through storage happens
/// entirely inside it:
///
/// ```text
/// _38 = _33.0          // the box, out of what classify returned
/// *(_26+24) = _38      // parked in this frame's wrapper
/// _11 = _26            // the wrapper, moved
/// _18 = _11.0          // and the box, out again
/// ```
///
/// `_38` is moved-from — the store hands the box to the aggregate — so
/// dropping there would free it while `_18` still reads through it. `_18` is
/// the name holding it when it dies, which is the name to drop, and nothing
/// said so: a `Field` read is an alias of somebody else's storage by default,
/// for the good reason in the module doc.
///
/// What makes this one different is that the storage is a local the frame
/// controls. So the wrapper has to *stay* here: one that gets returned hands
/// the box to the caller instead (`hands_one_back`), and freeing it here as
/// well is a double free. And one box-typed read per wrapper only, the same
/// discipline `boxes_handed_over` keeps — two reads name one box.
///
/// A *wrapper*, though, not any aggregate. A `T?` or a `T or E` holds its
/// payload and nothing releases it, which is why the frame has to. A struct or
/// an enum is the other way round: its own release walks its fields, and a
/// interface-object field goes through the vtable's `owned_release` — block and
/// contents both, which is more than a `InterfaceDrop` does. Letting this rule
/// match a struct gave the block two owners, and the one that ran first was
/// the weaker one:
///
/// ```text
/// *(_0+0) = _10           // the box, into Shelf's field
/// _12 = _11.0             // and out again, to call through
/// interface_drop(_12)         // the frame frees the block...
/// rc_dec_contents(_11)    // ...and the struct's release reads it afterwards
/// ```
///
/// The `Vec` inside the boxed value was freed by nobody and the walk ran over
/// memory that was already gone (#1161).
fn boxes_parked_in_a_wrapper(func: &MirFunction, fresh: &HashSet<LocalId>) -> HashSet<LocalId> {
    let ty_of = local_types(func);
    let mut out: HashSet<LocalId> = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Store { addr, value: MirOperand::Local(v), .. } = &stmt.kind else {
            continue;
        };
        if releases_its_own_fields(&ty_of, addr) {
            continue;
        }
        if fresh.contains(v) && is_box(&ty_of, v) {
            out.extend(box_read_out_of(func, *addr, &ty_of));
        }
    }
    out
}

/// Does this aggregate give back what its fields hold when it dies?
///
/// A struct and an enum do — `rc_insert` puts an `RcDecContents` on one that
/// stays in the frame, and the walk reaches an interface-object field through the
/// vtable. A `T?` and a `T or E` don't: the release walk goes through them to
/// the payload, and the payload's box is the frame's to free.
fn releases_its_own_fields(ty_of: &HashMap<LocalId, MirType>, id: &LocalId) -> bool {
    matches!(ty_of.get(id), Some(MirType::Struct(_)) | Some(MirType::Enum(_)))
}

/// The one name that ends up owning the box in `wrapper`'s payload.
///
/// The wrapper is followed through plain moves first. `_11 = _24` renames the
/// aggregate and the payload read comes off the new name, which is how a plain
/// `return classify(n)` forward stayed leaking after the `try` form was fixed:
/// the call's destination was the only name anyone looked at.
///
/// One box-typed read across the whole group, and the group has to stay in this
/// frame. Two reads name one box and a drop under each frees it twice; a
/// wrapper that leaves hands the box to whoever gets it. Both answer "no
/// owner here", which leaks — the safe half.
fn box_read_out_of(
    func: &MirFunction,
    wrapper: LocalId,
    ty_of: &HashMap<LocalId, MirType>,
) -> Option<LocalId> {
    let carriers = names_the_aggregate_reaches(func, wrapper);
    if !wrapper_stays_here(func, &carriers) {
        return None;
    }
    let reads: Vec<LocalId> = func
        .blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::Assign { dst, rvalue: MirRValue::Field { base, .. } } => {
                let base = crate::analysis::uses::operand_local(base)?;
                (carriers.contains(&base) && is_box(ty_of, dst)).then_some(*dst)
            }
            _ => None,
        })
        .collect();
    match reads.as_slice() {
        [one] => Some(*one),
        _ => None,
    }
}

/// Every name an aggregate reaches by a plain move, itself included.
fn names_the_aggregate_reaches(func: &MirFunction, start: LocalId) -> HashSet<LocalId> {
    let mut reached: HashSet<LocalId> = HashSet::new();
    reached.insert(start);
    loop {
        let before = reached.len();
        for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } =
                &st.kind
            {
                if reached.contains(src) {
                    reached.insert(*dst);
                }
            }
        }
        if reached.len() == before {
            return reached;
        }
    }
}

fn local_types(func: &MirFunction) -> HashMap<LocalId, MirType> {
    func.locals.iter().map(|l| (l.id, l.ty.clone())).collect()
}

fn is_box(ty_of: &HashMap<LocalId, MirType>, id: &LocalId) -> bool {
    matches!(ty_of.get(id), Some(MirType::InterfaceObject { .. }))
}

/// Whether a wrapper, and every name it moves to, is only ever assembled,
/// read, and moved along — never handed anywhere else.
///
/// A whitelist rather than a list of ways to escape: a shape nobody thought
/// about should read as "handed away", because that answer leaks and the other
/// one double-frees.
fn wrapper_stays_here(func: &MirFunction, carriers: &HashSet<LocalId>) -> bool {
    let touches = |st: &MirStmt| {
        carriers.iter().any(|c| crate::analysis::uses::stmt_reads(st, *c))
    };
    for block in &func.blocks {
        for st in &block.statements {
            if !touches(st) {
                continue;
            }
            let allowed = match &st.kind {
                // Writing a slot of the wrapper. Writing the wrapper itself
                // into something else is a hand-off.
                MirStmtKind::Store { addr, value, .. } => {
                    carriers.contains(addr)
                        && !matches!(
                            crate::analysis::uses::operand_local(value),
                            Some(v) if carriers.contains(&v)
                        )
                }
                // Reading a slot, reading the tag, or moving the whole wrapper
                // to a name that is itself a carrier.
                MirStmtKind::Assign { rvalue: MirRValue::Field { .. }, .. }
                | MirStmtKind::Assign { rvalue: MirRValue::EnumTag { .. }, .. } => true,
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(_)) } => {
                    carriers.contains(dst)
                }
                _ => false,
            };
            if !allowed {
                return false;
            }
        }
        // Returned, or read by a terminator any other way.
        if carriers
            .iter()
            .any(|c| crate::analysis::uses::terminator_reads(&block.terminator, *c))
        {
            return false;
        }
    }
    true
}

/// The box a call handed this frame, when the callee is one of the above.
///
/// Two shapes. An interface-object-typed destination *is* the box. A wrapper
/// destination holds it in its payload — `-> i64 or Error` returns the box as
/// the error side — and the payload read is the name that owns it.
///
/// Which name owns the wrapper's payload is `box_read_out_of`'s question, and
/// the same one for a wrapper this frame assembled itself.
fn boxes_handed_over(func: &MirFunction, hands_back: &HashSet<String>) -> HashSet<LocalId> {
    let ty_of = local_types(func);
    let mut out: HashSet<LocalId> = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Call { dst: Some(dst), func: callee, .. } = &stmt.kind else { continue };
        if !hands_back.contains(&callee.name) {
            continue;
        }
        if is_box(&ty_of, dst) {
            out.insert(*dst);
        } else if matches!(
            ty_of.get(dst),
            Some(MirType::Option(_)) | Some(MirType::Result { .. })
        ) {
            out.extend(box_read_out_of(func, *dst, &ty_of));
        }
    }
    out
}

fn insert_for_function(
    func: &mut MirFunction,
    callee_escapes: &HashMap<String, Vec<bool>>,
    hands_back: &HashSet<String>,
) {
    let mut interface_locals = collect_fresh_interface_locals(func);
    interface_locals.extend(boxes_handed_over(func, hands_back));
    if interface_locals.is_empty() {
        return;
    }
    // Out of one wrapper and into the next, as many times as the frame does it.
    // Inlining puts every hop of `quadrupled → doubled → classify` in one body,
    // so the box is read out of what `classify` returned, parked in `doubled`'s
    // wrapper, read out of that, parked in `quadrupled`'s, and read out again.
    // Each step is the one `boxes_parked_in_a_wrapper` describes; running it
    // once followed the first hop and lost the second.
    //
    // And a box that arrived from a callee gets copied like any other: inlining
    // `describe(e)` writes it into the callee's parameter local, and that last
    // name is the one that owns it. Without this the original read as moved-away
    // and the copy was never a candidate, so nobody dropped it.
    loop {
        let before = interface_locals.len();
        carry_through_moves(func, &mut interface_locals);
        let parked = boxes_parked_in_a_wrapper(func, &interface_locals);
        interface_locals.extend(parked);
        if interface_locals.len() == before {
            break;
        }
    }

    // Where a box is made: boxed here, handed back by a callee, or read out
    // of a wrapper this frame parked it in. Everything else in the set is a
    // copy of one of those.
    let mut made: HashSet<LocalId> = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let MirStmtKind::InterfaceBox { dst, .. } = &stmt.kind {
            made.insert(*dst);
        }
    }
    made.extend(boxes_handed_over(func, hands_back));
    made.extend(boxes_parked_in_a_wrapper(func, &interface_locals));

    let facts = box_facts(func, &interface_locals, &made, callee_escapes);
    let plan = crate::analysis::ownership::plan(
        func,
        &facts,
        crate::analysis::ownership::Placement::ScopeEnd,
    );
    let owning = drops_that_own(func, &plan, &made);
    let drop_of = |interface_object: LocalId| {
        MirStmt::dummy(MirStmtKind::InterfaceDrop {
            interface_object,
            owns: owning.contains(&interface_object),
        })
    };
    let mut at_end: Vec<(usize, LocalId)> = Vec::new();
    let mut on_edges: Vec<(BlockId, BlockId, Vec<MirStmt>)> = Vec::new();
    for r in plan {
        match r {
            crate::analysis::ownership::Release::At { block, name, .. } => at_end.push((block, name)),
            crate::analysis::ownership::Release::OnEdge { from, to, name, .. } => {
                on_edges.push((from, to, vec![drop_of(name)]))
            }
        }
    }
    at_end.sort_by_key(|(b, l)| (*b, l.0));
    for (block, name) in at_end {
        func.blocks[block].statements.push(drop_of(name));
    }
    crate::analysis::ownership::insert_on_edges(func, on_edges);
}

/// The names whose drop releases the boxed value too, not only the block.
///
/// A box owns what was moved into it. Two kinds of box reach a drop here
/// owning their value: one a callee handed back (it moved the value in and
/// returned the box), and one boxed in this frame from a value built for the
/// box and read by nothing else — `return HttpRegistry { url: path }` once
/// `make` is inlined. Releasing those through the frame's own copy of the
/// value can't work everywhere: with the box picked in a branch, the value is
/// a different struct on each path, and no one name holds it after the join.
/// The box does, so its drop releases what is in it (#1424).
///
/// A box built for a call from a value the frame goes on using borrows it, and
/// its drop frees the block alone. A drop reached by both kinds can't do both,
/// so it borrows, and so does every other drop those boxes reach: `rc_insert`
/// hands a box's value over only when every drop it reaches owns.
fn drops_that_own(
    func: &MirFunction,
    plan: &[crate::analysis::ownership::Release],
    made: &HashSet<LocalId>,
) -> HashSet<LocalId> {
    use crate::analysis::ownership::Release;
    let names: HashSet<LocalId> = plan
        .iter()
        .map(|r| match r {
            Release::At { name, .. } | Release::OnEdge { name, .. } => *name,
        })
        .collect();
    if names.is_empty() {
        return HashSet::new();
    }

    // What each name was copied from.
    let mut copied_from: HashMap<LocalId, Vec<LocalId>> = HashMap::new();
    let mut boxed_from: HashMap<LocalId, LocalId> = HashMap::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        match &stmt.kind {
            MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } => {
                copied_from.entry(*dst).or_default().push(*src);
            }
            MirStmtKind::Phi { dst, args } => {
                for (_, op) in args {
                    if let MirOperand::Local(src) = op {
                        copied_from.entry(*dst).or_default().push(*src);
                    }
                }
            }
            MirStmtKind::InterfaceBox { dst, value, .. } => {
                if let MirOperand::Local(src) = value {
                    boxed_from.insert(*dst, *src);
                }
            }
            _ => {}
        }
    }
    let owns_its_value = |maker: LocalId| match boxed_from.get(&maker) {
        Some(src) => built_for_the_box(func, *src),
        // A box boxed from a constant, or one a callee handed back or that
        // came out of a wrapper: the value is the box's.
        None => true,
    };

    let makers_of = |name: LocalId| -> HashSet<LocalId> {
        let mut seen: HashSet<LocalId> = HashSet::new();
        let mut out: HashSet<LocalId> = HashSet::new();
        let mut frontier = vec![name];
        while let Some(n) = frontier.pop() {
            if !seen.insert(n) {
                continue;
            }
            if made.contains(&n) {
                out.insert(n);
                continue;
            }
            if let Some(srcs) = copied_from.get(&n) {
                frontier.extend(srcs.iter().copied());
            }
        }
        out
    };

    let groups: Vec<(LocalId, HashSet<LocalId>)> = names.iter().map(|n| (*n, makers_of(*n))).collect();
    let mut owning: HashSet<LocalId> = groups
        .iter()
        .filter(|(_, makers)| !makers.is_empty() && makers.iter().all(|m| owns_its_value(*m)))
        .map(|(n, _)| *n)
        .collect();
    loop {
        let lent: HashSet<LocalId> = groups
            .iter()
            .filter(|(n, _)| !owning.contains(n))
            .flat_map(|(_, makers)| makers.iter().copied())
            .collect();
        let before = owning.len();
        owning.retain(|n| groups.iter().any(|(g, makers)| g == n && makers.is_disjoint(&lent)));
        if owning.len() == before {
            return owning;
        }
    }
}

/// Is `value` a struct or enum built for one box and nothing else? Filled
/// field by field, never a copy of another name, and read by exactly one
/// statement: the `InterfaceBox`.
fn built_for_the_box(func: &MirFunction, value: LocalId) -> bool {
    use crate::analysis::uses;
    if func.params.iter().any(|p| p.id == value) {
        return false;
    }
    let ty_of = local_types(func);
    if !matches!(ty_of.get(&value), Some(MirType::Struct(_)) | Some(MirType::Enum(_))) {
        return false;
    }
    let mut boxes = 0;
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if uses::stmt_def(stmt) == Some(value) {
            return false;
        }
        if !uses::stmt_reads(stmt, value) {
            continue;
        }
        match &stmt.kind {
            MirStmtKind::Store { addr, value: v, .. }
                if *addr == value && uses::operand_local(v) != Some(value) => {}
            MirStmtKind::InterfaceBox { value: MirOperand::Local(v), .. } if *v == value => boxes += 1,
            _ => return false,
        }
    }
    boxes == 1 && !func.blocks.iter().any(|b| uses::terminator_reads(&b.terminator, value))
}

/// What each statement does to the boxes this frame may hold.
///
/// Handed over: returned, stored, passed to a closure or through a vtable, or
/// to a named callee whose body keeps it. A callee that demonstrably keeps
/// nothing of the argument leaves the box to this frame; no answer means it
/// might keep it. An interface call's receiver is a borrow.
fn box_facts(
    func: &MirFunction,
    tracked: &HashSet<LocalId>,
    made: &HashSet<LocalId>,
    callee_escapes: &HashMap<String, Vec<bool>>,
) -> crate::analysis::ownership::Facts {
    use crate::analysis::ownership::Event;
    use crate::analysis::uses;
    let names: std::collections::BTreeSet<LocalId> = tracked.iter().copied().collect();
    let mut facts = crate::analysis::ownership::Facts {
        names: names.clone(),
        events: Vec::new(),
        terminator_events: Vec::new(),
        reads: Vec::new(),
        kills: Vec::new(),
        terminator_reads: Vec::new(),
        foreign: func.params.iter().map(|p| p.id).filter(|p| tracked.contains(p)).collect(),
        owned: Vec::new(),
    };
    for block in &func.blocks {
        let (mut events, mut reads, mut kills) = (Vec::new(), Vec::new(), Vec::new());
        for stmt in &block.statements {
            let mut ev: Vec<Event> = Vec::new();
            let give = |ev: &mut Vec<Event>, op: &MirOperand| {
                if let Some(id) = uses::operand_local(op).filter(|id| tracked.contains(id)) {
                    ev.push(Event::HandOver(id));
                }
            };
            match &stmt.kind {
                MirStmtKind::Call { func: callee, args, .. } => {
                    let keeps = callee_escapes.get(&callee.name);
                    for (i, arg) in args.iter().enumerate() {
                        let borrowed = keeps.and_then(|e| e.get(i)).is_some_and(|escapes| !escapes);
                        if !borrowed {
                            give(&mut ev, arg);
                        }
                    }
                }
                MirStmtKind::ClosureCall { args, .. } | MirStmtKind::InterfaceCall { args, .. } => {
                    for arg in args {
                        give(&mut ev, arg);
                    }
                }
                MirStmtKind::Store { value, .. } | MirStmtKind::ArrayStore { value, .. } => {
                    give(&mut ev, value)
                }
                MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                    if tracked.contains(dst) && tracked.contains(src) && !made.contains(dst) =>
                {
                    ev.push(Event::Alias { dst: *dst, src: *src });
                }
                _ => {}
            }
            if let Some(d) = uses::stmt_def(stmt).filter(|d| tracked.contains(d)) {
                let bound = ev.iter().any(|e| matches!(e, Event::Alias { dst, .. } if *dst == d));
                if !bound && !matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                    ev.push(if made.contains(&d) { Event::Make(d) } else { Event::Other(d) });
                }
            }
            let (mut r, mut k) = (Vec::new(), Vec::new());
            if !matches!(stmt.kind, MirStmtKind::Phi { .. }) {
                for n in &names {
                    if uses::stmt_reads(stmt, *n) {
                        r.push(*n);
                    }
                    if uses::stmt_def(stmt) == Some(*n) {
                        k.push(*n);
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
        let mut term = Vec::new();
        if let MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
        | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } =
            &block.terminator.kind
        {
            if tracked.contains(id) {
                term.push(Event::HandOver(*id));
            }
        }
        facts.terminator_events.push(term);
        facts
            .terminator_reads
            .push(names.iter().copied().filter(|n| uses::terminator_reads(&block.terminator, *n)).collect());
    }
    facts
}

/// Locals that hold a fresh interface-object allocation: `InterfaceBox` destinations,
/// plus anything a chain of plain moves or `Phi` merges carries forward from
/// one. A local typed as an interface object but reached only through a `Field`
/// read, a container access, or a call/method return is deliberately left
/// out — see the module doc for why aliasing one of those as "droppable"
/// double-frees.
fn collect_fresh_interface_locals(func: &MirFunction) -> HashSet<LocalId> {
    // SSA renaming (loop variables especially) can leave a pre-rename local
    // declaration behind in `func.locals` even once nothing in the CFG
    // defines it anymore, so start from actual definition sites, not the
    // declared list — a `InterfaceDrop` for a local nothing ever writes reads
    // garbage (Cranelift's verifier catches it as a block-argument mismatch).
    let mut fresh: HashSet<LocalId> = HashSet::new();
    for block in &func.blocks {
        for stmt in &block.statements {
            if let MirStmtKind::InterfaceBox { dst, .. } = &stmt.kind {
                fresh.insert(*dst);
            }
        }
    }
    if fresh.is_empty() {
        return fresh;
    }

    carry_through_moves(func, &mut fresh);
    fresh
}

/// Carry every name in `held` forward through plain moves and phi-merges, to a
/// fixed point: `_4 = _3` (real lowering copies a `InterfaceBox` result into the
/// source-named local before first use) or a multi-hop chain both carry the
/// same allocation to a new name, and the last name is the one that owns it.
fn carry_through_moves(func: &MirFunction, held: &mut HashSet<LocalId>) {
    loop {
        let mut added = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                match &stmt.kind {
                    MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) }
                        if held.contains(src) && !held.contains(dst) =>
                    {
                        held.insert(*dst);
                        added = true;
                    }
                    MirStmtKind::Phi { dst, args } if !held.contains(dst) => {
                        if args.iter().any(|(_, op)| matches!(op, MirOperand::Local(id) if held.contains(id))) {
                            held.insert(*dst);
                            added = true;
                        }
                    }
                    _ => {}
                }
            }
        }
        if !added {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MirBlock, MirLocal, MirTerminator};

    fn local(id: u32) -> LocalId { LocalId(id) }
    fn block_id(id: u32) -> BlockId { BlockId(id) }

    fn interface_local(id: u32) -> MirLocal { MirLocal { id: local(id), name: None, ty: MirType::InterfaceObject { interface_name: "Speaker".into() }, is_param: false, unerased: None } }

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

    fn drops_of(stmts: &[MirStmt], names: &[LocalId]) -> usize {
        stmts
            .iter()
            .filter(|s| matches!(&s.kind, MirStmtKind::InterfaceDrop { interface_object, .. } if names.contains(interface_object)))
            .count()
    }

    fn has_interface_drop(stmts: &[MirStmt], target: LocalId) -> bool {
        stmts.iter().any(|s| matches!(&s.kind, MirStmtKind::InterfaceDrop { interface_object, .. } if *interface_object == target))
    }

    fn interface_box(dst: LocalId) -> MirStmt {
        MirStmt::dummy(MirStmtKind::InterfaceBox {
            dst,
            value: MirOperand::Local(local(99)),
            concrete_type: "Loud".into(),
            interface_name: "Speaker".into(),
            concrete_size: 32,
            vtable_name: ".vtable.Loud__Speaker".into(),
        })
    }

    fn interface_call(receiver: LocalId) -> MirStmt {
        MirStmt::dummy(MirStmtKind::InterfaceCall {
            dst: None,
            interface_object: receiver,
            method_name: "speak".into(),
            vtable_offset: crate::vtable_layout::method_offset(0),
            args: vec![],
        })
    }

    #[test]
    fn non_escaping_dropped_before_return() {
        let mut f = make_fn(
            vec![interface_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![interface_box(local(0)), interface_call(local(0))],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_interface_drops(std::slice::from_mut(&mut f));
        assert!(has_interface_drop(&f.blocks[0].statements, local(0)));
    }

    /// Reproduces the shape real lowering emits for `let s: any Speaker = ...`:
    /// the `InterfaceBox` result gets copied into a second local before use.
    #[test]
    fn moved_through_copy_drops_the_final_name_not_the_original() {
        let mut f = make_fn(
            vec![interface_local(0), interface_local(1)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    interface_box(local(0)),
                    MirStmt::dummy(MirStmtKind::Assign {
                        dst: local(1),
                        rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                    }),
                    interface_call(local(1)),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_interface_drops(std::slice::from_mut(&mut f));
        assert_eq!(drops_of(&f.blocks[0].statements, &[local(0), local(1)]), 1, "one drop, under either name");
    }

    #[test]
    fn returned_interface_object_not_dropped() {
        let mut f = make_fn(
            vec![interface_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![interface_box(local(0))],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return {
                    value: Some(MirOperand::Local(local(0))),
                }),
            }],
        );
        insert_interface_drops(std::slice::from_mut(&mut f));
        assert!(!has_interface_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn stored_interface_object_not_dropped() {
        let mut f = make_fn(
            vec![interface_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    interface_box(local(0)),
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
        insert_interface_drops(std::slice::from_mut(&mut f));
        assert!(!has_interface_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn call_argument_not_dropped() {
        let mut f = make_fn(
            vec![interface_local(0)],
            vec![MirBlock {
                id: block_id(0),
                statements: vec![
                    interface_box(local(0)),
                    MirStmt::dummy(MirStmtKind::Call {
                        dst: None,
                        func: crate::FunctionRef::internal("consume".into()),
                        args: vec![MirOperand::Local(local(0))],
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_interface_drops(std::slice::from_mut(&mut f));
        assert!(!has_interface_drop(&f.blocks[0].statements, local(0)));
    }

    #[test]
    fn loop_body_dropped_at_back_edge() {
        // block 0: entry -> goto 1
        // block 1: loop header -> branch 2/3
        // block 2: body — InterfaceBox local 0 (moved into local 1), InterfaceCall, goto 1 (back-edge)
        // block 3: exit — return
        let mut f = make_fn(
            vec![interface_local(0), interface_local(1)],
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
                        interface_box(local(0)),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(1),
                            rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                        }),
                        interface_call(local(1)),
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
        insert_interface_drops(std::slice::from_mut(&mut f));
        assert_eq!(drops_of(&f.blocks[2].statements, &[local(0), local(1)]), 1, "back-edge block should drop the loop-local interface object once");
        assert_eq!(drops_of(&f.blocks[3].statements, &[local(0), local(1)]), 0, "and the exit nothing more");
    }

    /// Reproduces the shape `assert`'s desugaring produces: the success and
    /// failure blocks are allocated (and so numbered) before the block that
    /// computes the condition and branches to them. A block-index check for
    /// "is this a back-edge" sees block 1 as jumped-to from a higher-numbered
    /// block 2 and mistakes it for a loop, inserting a second `InterfaceDrop` at
    /// block 2 on top of the one already correctly placed at block 1's
    /// return — a double free (#366 follow-up: this exact shape crashed
    /// `tests/suite/t11_interfaces.rk`'s "interface object dispatch" test in CI).
    #[test]
    fn assert_style_branch_to_lower_numbered_blocks_is_not_a_back_edge() {
        // block 0: entry — InterfaceBox local 0 (moved to local 1), goto 2
        // block 1: success — InterfaceDrop already placed here, return
        // (no block 1 predecessor other than block 2 — not a loop header)
        // block 2: InterfaceCall, branch to 1 (success) or 1 (success, for simplicity)
        let mut f = make_fn(
            vec![interface_local(0), interface_local(1)],
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
                        interface_box(local(0)),
                        MirStmt::dummy(MirStmtKind::Assign {
                            dst: local(1),
                            rvalue: MirRValue::Use(MirOperand::Local(local(0))),
                        }),
                        interface_call(local(1)),
                    ],
                    terminator: MirTerminator::dummy(MirTerminatorKind::Branch {
                        cond: MirOperand::Local(local(50)),
                        then_block: block_id(1),
                        else_block: block_id(1),
                    }),
                },
            ],
        );
        insert_interface_drops(std::slice::from_mut(&mut f));
        let all: Vec<MirStmt> = f.blocks.iter().flat_map(|b| b.statements.clone()).collect();
        assert_eq!(drops_of(&all, &[local(0), local(1)]), 1, "one drop in all, not a second at a branch mistaken for a back edge");
    }

    /// Reproduces `tests/suite/t62_interface_object_positions.rk`'s struct-field
    /// test: reading an interface object back out of a container (here, a struct
    /// field) twice produces two locals of the same type aliasing one heap
    /// box. Treating either as a fresh, droppable allocation — as a plain
    /// "is this local typed as an interface object" check would — drops the same
    /// pointer twice.
    #[test]
    fn field_read_interface_object_is_not_tracked_as_fresh() {
        let mut f = make_fn(
            vec![interface_local(0), interface_local(1)],
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
                    interface_call(local(0)),
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
        insert_interface_drops(std::slice::from_mut(&mut f));
        assert!(!has_interface_drop(&f.blocks[0].statements, local(0)), "a field read is a borrow, not a fresh allocation");
        assert!(!has_interface_drop(&f.blocks[0].statements, local(1)), "same here — this pass must not touch the struct's own field");
    }
}
