// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Interface-object drop insertion.
//!
//! `InterfaceBox` heap-allocates and copies a concrete value once to build an
//! `any Interface` fat pointer; `InterfaceCall` reads through it, and consumes
//! it only when the method is declared `take self`.
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
//! A `take self` call hands the box over like any other consumption. The method
//! releases what is inside, so the frame frees the block alone, right after the
//! call (`BoxRelease::Taken`), and `rc_insert` gives up the value a borrowing
//! box was viewing on that path.
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
//! A box passed as an argument stays the frame's when the callee's body is
//! known to keep nothing of it: the named callee, or for a vtable call every
//! body the slot can land in. Without an answer it counts as handed over,
//! which may leak but never frees something a callee kept.
//!
//! Function parameters are excluded — a `InterfaceDrop` needs the block where its
//! value was defined (for the loop back-edge check below), and a param's
//! "definition" is the call site, not a statement in this function's body.
//! A parameter that isn't returned/stored/passed onward still leaks; narrower
//! than the general case, and left for a follow-up.

use std::collections::{HashMap, HashSet};

use crate::vtable_layout::VtableBodies;
use crate::{
    BlockId, LocalId, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind,
    MirTerminatorKind, MirType,
};

/// Insert `InterfaceDrop` for every non-escaping interface-object local, across all functions.
pub fn insert_interface_drops(fns: &mut [MirFunction], vtables: &VtableBodies) {
    // Which callees keep an argument, read off their bodies — the same map the
    // closure pass uses, and for the same reason. Passing a box to a function
    // used to count as handing it over, so `io.copy(src, dst)` left both
    // buffers to a callee that only reads through them: the boxed value's
    // containers were freed by nobody. It only looked fixed while `io.copy`
    // was small enough to inline, which is why one `io.copy` in a file was
    // clean and two leaked five buffers.
    //
    // A parameter captured by *any* closure counts as escaping here. Erring
    // that way leaks; erring the other way is a double free.
    let callee_escapes =
        crate::closures::build_callee_escape_map(fns, crate::closures::CaptureRule::AnyClosure, vtables);
    // And which functions hand a fresh box *back*, which makes their caller the
    // owner. `return Boom.Bad` in a `-> i64 or Error` boxes the enum and
    // returns it, and the caller read the box out of the wrapper and dropped
    // nothing — a boxed error leaked on every failing call, which is every
    // `or Error` in the language.
    let hands_back = functions_handing_back_a_interface_box(fns, vtables);
    let targets = crate::closure_targets::ClosureTargets::build(fns);
    for func in fns.iter_mut() {
        insert_for_function(func, &callee_escapes, &hands_back, &targets, vtables);
    }
}

/// Functions whose return value is a fresh interface box the caller now owns.
///
/// A fixed point, because handing one back is transitive: a wrapper that just
/// forwards what it called joins the set on a later pass.
fn functions_handing_back_a_interface_box(fns: &[MirFunction], vtables: &VtableBodies) -> HashSet<String> {
    let targets = crate::closure_targets::ClosureTargets::build(fns);
    let by_name: HashMap<&str, &MirFunction> = fns.iter().map(|f| (f.name.as_str(), f)).collect();
    let slot_writers: HashMap<&str, (Vec<SlotWriter>, HashSet<LocalId>)> = fns
        .iter()
        .map(|f| (f.name.as_str(), (return_slot_writers(f, &by_name), returned_locals(f))))
        .filter(|(_, (w, _))| !w.is_empty())
        .collect();
    let mut names: HashSet<String> = HashSet::new();
    loop {
        let before = names.len();
        // A box that came back from a call to something already in the set
        // is this frame's, and passing it on hands it over again. Which
        // *name* holds it is the same question the caller's side asks, so
        // ask it the same way: an interface-object destination is the box, and
        // a wrapper destination holds it in its payload.
        let fresh_of: HashMap<&str, HashSet<LocalId>> = fns
            .iter()
            .map(|func| {
                let mut fresh = collect_fresh_interface_locals(func);
                fresh.extend(boxes_handed_over(func, &names, &targets, vtables));
                fresh.extend(boxes_parked_in_a_wrapper(func, &fresh));
                (func.name.as_str(), fresh)
            })
            .collect();
        for func in fns {
            if names.contains(&func.name) {
                continue;
            }
            let own = hands_one_back(func, &fresh_of[func.name.as_str()]);
            let through_closures =
                slot_writers.get(func.name.as_str()).map_or(Some(false), |(writers, returned)| {
                    let mut owned = fresh_of[func.name.as_str()].clone();
                    carry_through_moves(func, &mut owned);
                    closures_fill_the_slot(writers, returned, &owned, &by_name, &fresh_of)
                });
            match (own, through_closures) {
                (Some(false), _) | (_, None) => {}
                (Some(true), _) | (None, Some(true)) => {
                    names.insert(func.name.clone());
                }
                (None, Some(false)) => {}
            }
        }
        if names.len() == before {
            return names;
        }
    }
}

/// A closure that writes this function's return value through a borrowed
/// capture, and which of the function's variables each pointer its body reads
/// through stands for.
///
/// `return` inside the body of a `for` over a sequence can't return from the
/// function: the body is a closure. It writes the answer into a variable of
/// the function's instead, through a by-reference capture, and the function
/// returns that variable once the loop is over. The box is made inside the
/// closure, or is one the function made and the closure reads through another
/// capture, so looking only at the function's own stores found nothing, the
/// caller dropped nothing, and `for a in nums() { if a == k { return
/// Circle(…) } }` leaked a box on every hit. A nested loop hands the pointers
/// on to the inner body's closure the same way.
struct SlotWriter {
    closure: String,
    /// The closure's pointer locals, each to the function's variable it reaches.
    roots: HashMap<LocalId, LocalId>,
}

fn return_slot_writers(func: &MirFunction, by_name: &HashMap<&str, &MirFunction>) -> Vec<SlotWriter> {
    let returned = returned_locals(func);
    let identity: HashMap<LocalId, LocalId> = func.locals.iter().map(|l| (l.id, l.id)).collect();
    let mut work: Vec<(String, u32, LocalId)> = ref_captures_of(func, &identity);
    let mut by_closure: HashMap<String, HashMap<u32, LocalId>> = HashMap::new();
    let mut seen: HashSet<(String, u32)> = HashSet::new();
    while let Some((closure, offset, root)) = work.pop() {
        if !seen.insert((closure.clone(), offset)) {
            continue;
        }
        by_closure.entry(closure.clone()).or_default().insert(offset, root);
        let Some(body) = by_name.get(closure.as_str()) else { continue };
        let roots = pointer_roots(body, &HashMap::from([(offset, root)]));
        work.extend(ref_captures_of(body, &roots));
    }
    by_closure
        .into_iter()
        .filter_map(|(closure, offsets)| {
            let body = by_name.get(closure.as_str())?;
            let roots = pointer_roots(body, &offsets);
            roots.values().any(|r| returned.contains(r)).then_some(SlotWriter { closure, roots })
        })
        .collect()
}

fn returned_locals(func: &MirFunction) -> HashSet<LocalId> {
    func.blocks
        .iter()
        .filter_map(|b| match &b.terminator.kind {
            MirTerminatorKind::Return { value: Some(MirOperand::Local(id)) }
            | MirTerminatorKind::CleanupReturn { value: Some(MirOperand::Local(id)), .. } => Some(*id),
            _ => None,
        })
        .collect()
}

/// The locals a closure body reads its borrowed captures into, each to the
/// variable its env slot points at.
fn pointer_roots(body: &MirFunction, offsets: &HashMap<u32, LocalId>) -> HashMap<LocalId, LocalId> {
    body.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::LoadCapture { dst, offset, access: crate::CaptureAccess::Borrowed, .. } => {
                offsets.get(offset).map(|root| (*dst, *root))
            }
            _ => None,
        })
        .collect()
}

/// The closures `func` builds that capture one of `roots`' keys by reference:
/// the closure, the capture's offset, and the variable it stands for.
fn ref_captures_of(func: &MirFunction, roots: &HashMap<LocalId, LocalId>) -> Vec<(String, u32, LocalId)> {
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::ClosureCreate { func_name, captures, .. } => Some((func_name, captures)),
            _ => None,
        })
        .flat_map(|(name, captures)| {
            captures
                .iter()
                .filter(|c| c.by_ref)
                .filter_map(move |c| roots.get(&c.local_id).map(|root| (name.clone(), c.offset, *root)))
        })
        .collect()
}

/// Whether the closures that write the return slot put a box the function
/// owns there: `Some(true)` if one does, `Some(false)` if none stores a box,
/// `None` if one stores a box nobody here owns, which settles it the safe way.
/// A box is owned when the closure made it, or when it is read through a
/// pointer to one of the function's own boxes (`owned_by_root`).
fn closures_fill_the_slot(
    writers: &[SlotWriter],
    returned: &HashSet<LocalId>,
    owned_by_root: &HashSet<LocalId>,
    by_name: &HashMap<&str, &MirFunction>,
    fresh_of: &HashMap<&str, HashSet<LocalId>>,
) -> Option<bool> {
    let mut found = false;
    for w in writers {
        let (Some(body), Some(fresh)) = (by_name.get(w.closure.as_str()), fresh_of.get(w.closure.as_str())) else {
            continue;
        };
        let mut owned = fresh.clone();
        owned.extend(w.roots.iter().filter(|(_, r)| owned_by_root.contains(r)).map(|(p, _)| *p));
        let ty_of = local_types(body);
        for st in body.blocks.iter().flat_map(|b| b.statements.iter()) {
            let MirStmtKind::Store { addr, value: MirOperand::Local(v), .. } = &st.kind else {
                continue;
            };
            if !w.roots.get(addr).is_some_and(|r| returned.contains(r)) {
                continue;
            }
            match box_stored_in(body, *v, &owned, &ty_of) {
                Some(true) => found = true,
                Some(false) => return None,
                None => {}
            }
        }
    }
    Some(found)
}

/// Whether `value` is, or wraps, a box this frame owns: `None` when it holds
/// no box at all. A wrapper is followed back through plain moves to the
/// aggregate its payload was stored into.
fn box_stored_in(
    func: &MirFunction,
    value: LocalId,
    fresh: &HashSet<LocalId>,
    ty_of: &HashMap<LocalId, MirType>,
) -> Option<bool> {
    let mut origins: HashSet<LocalId> = HashSet::from([value]);
    loop {
        let before = origins.len();
        for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
            if let MirStmtKind::Assign { dst, rvalue: MirRValue::Use(MirOperand::Local(src)) } = &st.kind {
                if origins.contains(dst) {
                    origins.insert(*src);
                }
            }
        }
        if origins.len() == before {
            break;
        }
    }
    if is_box(ty_of, &value) {
        return Some(origins.iter().any(|o| fresh.contains(o)));
    }
    let mut found = None;
    for st in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let MirStmtKind::Store { addr, value: MirOperand::Local(b), .. } = &st.kind else {
            continue;
        };
        if origins.contains(addr) && is_box(ty_of, b) {
            if !fresh.contains(b) {
                return Some(false);
            }
            found = Some(true);
        }
    }
    found
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
fn hands_one_back(func: &MirFunction, fresh: &HashSet<LocalId>) -> Option<bool> {
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
                return Some(false);
            }
            found = true;
        }
    }
    found.then_some(true)
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
/// Not a struct or enum that releases its own fields, though
/// (`released_with_the_box`): the release walk reaches an interface-object
/// field through the vtable's `owned_release`, block and contents both.
/// Letting this rule match one of those gave the block two owners, and the
/// one that ran first was the weaker one:
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
        if released_with_the_box(&ty_of, addr) {
            continue;
        }
        if fresh.contains(v) && is_box(&ty_of, v) {
            out.extend(box_read_out_of(func, *addr, &ty_of));
        }
    }
    out
}

/// Would the box in this aggregate be freed by somebody else's release?
///
/// A struct or enum that `rc_insert` tracks gives back its fields when it
/// dies, and the release walk frees every box it reaches. Taking the box out
/// of one of those as well gave it two owners (#1161).
///
/// Not a `T?` or `T or E`. Its payload is all it holds on the path that reads
/// it, so the read takes the whole of it: `rc_insert` reads this pass's drop
/// of the payload as the payload's hand-over and releases the wrapper on the
/// other paths only. Left to the wrapper, the box was released when the
/// wrapper was, and a name still reading the box kept that release from
/// happening at all: `last = try direct(i)` in a loop that calls
/// `last.text()` first leaked every box but the last (#2060).
fn released_with_the_box(ty_of: &HashMap<LocalId, MirType>, id: &LocalId) -> bool {
    match ty_of.get(id) {
        Some(MirType::Option(_) | MirType::Result { .. }) | None => false,
        Some(ty) => crate::transform::rc_insert::aggregate_may_hold_string(ty),
    }
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
    if released_with_the_box(ty_of, &wrapper) {
        return None;
    }
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
///
/// A closure call counts when every body the closure may be hands one back:
/// `let g = |x| -> i64 or Error { … }` then `try g(n)` is the same hand-over
/// as a call to a named function, and the box leaked.
fn boxes_handed_over(
    func: &MirFunction,
    hands_back: &HashSet<String>,
    targets: &crate::closure_targets::ClosureTargets,
    vtables: &VtableBodies,
) -> HashSet<LocalId> {
    let ty_of = local_types(func);
    let mut out: HashSet<LocalId> = HashSet::new();
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        let (dst, hands) = match &stmt.kind {
            // A box taken out of a container (`ns.remove(i)`, `ns.pop()`,
            // `ns.replace(i, b)` and so `ns[i] = b`) is the caller's by the
            // declaration's `@moves_out`, as one a Rask callee hands back is.
            MirStmtKind::Call { dst: Some(dst), func: callee, .. } => (
                dst,
                hands_back.contains(&callee.name)
                    || callee.moves_out(),
            ),
            MirStmtKind::ClosureCall { dst: Some(dst), closure, .. } => (
                dst,
                targets
                    .known(&func.name, *closure)
                    .is_some_and(|bodies| bodies.iter().all(|b| hands_back.contains(b))),
            ),
            // A call through a vtable counts when every body it may land in
            // hands one back. `s.label()` on an `any Shape` whose `label`
            // returns `any Label` built a fresh box in the method, and the
            // caller dropped nothing: it leaked on every call.
            MirStmtKind::InterfaceCall { dst: Some(dst), interface_object, method_name, .. } => (
                dst,
                match ty_of.get(interface_object) {
                    Some(MirType::InterfaceObject { interface_name }) => vtables
                        .bodies(interface_name, method_name)
                        .is_some_and(|bodies| bodies.iter().all(|b| hands_back.contains(b))),
                    _ => false,
                },
            ),
            _ => continue,
        };
        if !hands {
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
    targets: &crate::closure_targets::ClosureTargets,
    vtables: &VtableBodies,
) {
    let mut interface_locals = collect_fresh_interface_locals(func);
    interface_locals.extend(boxes_handed_over(func, hands_back, targets, vtables));
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
    made.extend(boxes_handed_over(func, hands_back, targets, vtables));
    made.extend(boxes_parked_in_a_wrapper(func, &interface_locals));

    let facts = box_facts(func, &interface_locals, &made, callee_escapes, vtables);
    let plan = crate::analysis::ownership::plan(
        func,
        &facts,
        crate::analysis::ownership::Placement::ScopeEnd,
    );
    let owning = drops_that_own(func, &plan.releases, &made);
    let drop_of = |interface_object: LocalId| {
        MirStmt::dummy(MirStmtKind::InterfaceDrop {
            interface_object,
            release: if owning.contains(&interface_object) {
                crate::BoxRelease::Owns
            } else {
                crate::BoxRelease::Borrows
            },
        })
    };
    let guards = crate::analysis::ownership::Guards::new(func, &plan);
    let (_, unwind_edges) = crate::analysis::ownership::place_unwind(
        func,
        plan.unwind,
        &guards,
        &mut |_: &mut MirFunction, name: LocalId, _| vec![drop_of(name)],
    );
    let mut at_end: Vec<(usize, LocalId, Option<usize>)> = Vec::new();
    let mut on_edges: Vec<(BlockId, BlockId, Vec<MirStmt>)> = Vec::new();
    for r in plan.releases {
        match r {
            crate::analysis::ownership::Release::At { block, name, guard, .. } => at_end.push((block, name, guard)),
            crate::analysis::ownership::Release::OnEdge { from, to, name, guard, .. } => {
                on_edges.push((from, to, guards.wrap(guard, vec![drop_of(name)])))
            }
        }
    }
    at_end.sort_by_key(|(b, l, _)| (*b, l.0));
    for (block, name, guard) in at_end {
        func.blocks[block].statements.extend(guards.wrap(guard, vec![drop_of(name)]));
    }
    let flag_edges = guards.edges(func);
    crate::analysis::ownership::insert_on_edges(
        func,
        crate::analysis::ownership::merge_edges(
            crate::analysis::ownership::merge_edges(unwind_edges, on_edges),
            flag_edges,
        ),
    );
    guards.finish(func);

    // A box a `take self` call consumed still has its block. Nothing else
    // frees it, and what is inside went to the method.
    for block in &mut func.blocks {
        let mut at = 0;
        while at < block.statements.len() {
            let consumed = match &block.statements[at].kind {
                MirStmtKind::InterfaceCall { interface_object, takes_receiver: true, .. }
                    if interface_locals.contains(interface_object) =>
                {
                    Some(*interface_object)
                }
                _ => None,
            };
            at += 1;
            if let Some(interface_object) = consumed {
                block.statements.insert(
                    at,
                    MirStmt::dummy(MirStmtKind::InterfaceDrop {
                        interface_object,
                        release: crate::BoxRelease::Taken,
                    }),
                );
                at += 1;
            }
        }
    }
}

/// The names whose drop releases the boxed value too, not only the block.
///
/// A box owns what was moved into it. Two kinds of box reach a drop here
/// owning their value: one a callee handed back (it moved the value in and
/// returned the box), and one boxed in this frame from a literal written at
/// the box (`InterfaceBox.owns_value`) — `return HttpRegistry { url: path }`
/// once `make` is inlined. Releasing those through the frame's own copy of the
/// value can't work everywhere: with the box picked in a branch, the value is
/// a different struct on each path, and no one name holds it after the join.
/// The box does, so its drop releases what is in it (#1424).
///
/// A box made from a name the frame goes on holding borrows it, and its drop
/// frees the block alone. A drop reached by both kinds can't do both,
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
    let mut boxed_value: HashMap<LocalId, bool> = HashMap::new();
    let (ty_of, cells) = (local_types(func), own_cells(func));
    for stmt in func.blocks.iter().flat_map(|b| b.statements.iter()) {
        if let Some((cell, src)) = box_into_cell(&cells, &ty_of, stmt) {
            copied_from.entry(cell).or_default().push(src);
            continue;
        }
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
            MirStmtKind::InterfaceBox { dst, owns_value, .. } => {
                boxed_value.insert(*dst, *owns_value);
            }
            _ => {}
        }
    }
    let owns_its_value = |maker: LocalId| match boxed_value.get(&maker) {
        Some(owns) => *owns,
        // One a callee handed back or that came out of a wrapper: the value
        // is the box's.
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

/// What each statement does to the boxes this frame may hold.
///
/// Handed over: returned, stored, passed to a closure, or to a callee whose
/// body keeps it. A callee that demonstrably keeps nothing of the argument
/// leaves the box to this frame; no answer means it might keep it. Through a
/// vtable the callee is every body the slot can land in, and all of them must
/// answer. An interface call's receiver is a borrow.
fn box_facts(
    func: &MirFunction,
    tracked: &HashSet<LocalId>,
    made: &HashSet<LocalId>,
    callee_escapes: &HashMap<String, Vec<bool>>,
    vtables: &VtableBodies,
) -> crate::analysis::ownership::Facts {
    use crate::analysis::ownership::Event;
    use crate::analysis::uses;
    let ty_of = local_types(func);
    let cells = own_cells(func);
    let names: std::collections::BTreeSet<LocalId> = tracked.iter().copied().collect();
    let mut facts = crate::analysis::ownership::Facts {
        moved_parts: Default::default(),
        names: names.clone(),
        events: Vec::new(),
        terminator_events: Vec::new(),
        reads: Vec::new(),
        kills: Vec::new(),
        terminator_reads: Vec::new(),
        foreign: func.params.iter().map(|p| p.id).filter(|p| tracked.contains(p)).collect(),
        owned: Vec::new(),
        caller_slots: Vec::new(),
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
                MirStmtKind::ClosureCall { args, .. } => {
                    for arg in args {
                        give(&mut ev, arg);
                    }
                }
                // A call through a vtable lends its receiver, unless the
                // method is declared `take self`: then the box is the call's.
                // The method releases what is inside, and the frame frees the
                // block right after (`insert_for_function`).
                //
                // An argument is lent when no body behind the slot keeps it.
                // Each body's parameter 0 is `self`, so argument `i` is its
                // parameter `i + 1`. `a.bigger(b)` with an `any Other` gave
                // `b`'s box to the call, and nobody freed it (#2059).
                MirStmtKind::InterfaceCall { interface_object, method_name, args, takes_receiver, .. } => {
                    if *takes_receiver {
                        give(&mut ev, &MirOperand::Local(*interface_object));
                    }
                    let bodies = match ty_of.get(interface_object) {
                        Some(MirType::InterfaceObject { interface_name }) => {
                            vtables.bodies(interface_name, method_name)
                        }
                        _ => None,
                    };
                    for (i, arg) in args.iter().enumerate() {
                        let lent = bodies.is_some_and(|bodies| {
                            bodies.iter().all(|b| {
                                callee_escapes.get(b).and_then(|e| e.get(i + 1)).is_some_and(|keeps| !keeps)
                            })
                        });
                        if !lent {
                            give(&mut ev, arg);
                        }
                    }
                }
                MirStmtKind::Store { .. }
                    if box_into_cell(&cells, &ty_of, stmt)
                        .is_some_and(|(cell, src)| tracked.contains(&cell) && tracked.contains(&src)) =>
                {
                    let (cell, src) = box_into_cell(&cells, &ty_of, stmt).expect("matched above");
                    ev.push(Event::Alias { dst: cell, src });
                }
                MirStmtKind::Store { value, .. } | MirStmtKind::ArrayStore { value, .. } => {
                    give(&mut ev, value)
                }
                // A sequence loop's body returned it.
                MirStmtKind::MovedOut { local } if tracked.contains(local) => ev.push(Event::HandOver(*local)),
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
/// A box written whole into a variable that lives in memory: `*(cell+0) = box`.
///
/// A variable a closure captures by reference lives in a stack slot, so its
/// value is stored into the slot rather than assigned. That is a move into the
/// variable, the same as `dst = src`. Read as a store into somebody else's
/// storage, it handed the box away, and `let s: any Shape = Circle(…)` used by
/// a closure, or by the body of a `for` over a sequence, leaked its block.
///
/// Only a slot this frame starts (`ZeroAggregate`). Inside the closure the
/// same variable is a pointer read out of the environment, and a store through
/// it writes the other frame's variable, which that frame frees.
fn box_into_cell(cells: &HashSet<LocalId>, ty_of: &HashMap<LocalId, MirType>, stmt: &MirStmt) -> Option<(LocalId, LocalId)> {
    match &stmt.kind {
        MirStmtKind::Store { addr, offset: 0, value: MirOperand::Local(src), .. }
            if cells.contains(addr) && is_box(ty_of, addr) && is_box(ty_of, src) =>
        {
            Some((*addr, *src))
        }
        _ => None,
    }
}

/// The slots this frame starts itself.
fn own_cells(func: &MirFunction) -> HashSet<LocalId> {
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|st| match &st.kind {
            MirStmtKind::ZeroAggregate { local } => Some(*local),
            _ => None,
        })
        .collect()
}

fn carry_through_moves(func: &MirFunction, held: &mut HashSet<LocalId>) {
    let ty_of = local_types(func);
    let cells = own_cells(func);
    loop {
        let mut added = false;
        for block in &func.blocks {
            for stmt in &block.statements {
                if let Some((cell, src)) = box_into_cell(&cells, &ty_of, stmt) {
                    if held.contains(&src) && held.insert(cell) {
                        added = true;
                    }
                    continue;
                }
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

    fn interface_local(id: u32) -> MirLocal { MirLocal { id: local(id), name: None, ty: MirType::InterfaceObject { interface_name: "Speaker".into() }, is_param: false } }

    fn make_fn(locals: Vec<MirLocal>, blocks: Vec<MirBlock>) -> MirFunction {
        MirFunction {
            name: "test".to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals,
            blocks,
            entry_block: block_id(0),
            is_extern_c: false,
            inline_hint: false,
            param_modes: Vec::new(),
            item_mode_from_body: false,
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
            concrete_align: 8,
            vtable_name: ".vtable.Loud__Speaker".into(),
            owns_value: false,
        })
    }

    fn interface_call(receiver: LocalId) -> MirStmt {
        MirStmt::dummy(MirStmtKind::InterfaceCall {
            dst: None,
            interface_object: receiver,
            method_name: "speak".into(),
            vtable_offset: crate::vtable_layout::method_offset(0),
            args: vec![],
            takes_receiver: false,
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
        insert_interface_drops(std::slice::from_mut(&mut f), &VtableBodies::default());
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
        insert_interface_drops(std::slice::from_mut(&mut f), &VtableBodies::default());
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
        insert_interface_drops(std::slice::from_mut(&mut f), &VtableBodies::default());
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
                        store_size: 16,
                        field: None,
                    }),
                ],
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
        );
        insert_interface_drops(std::slice::from_mut(&mut f), &VtableBodies::default());
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
        insert_interface_drops(std::slice::from_mut(&mut f), &VtableBodies::default());
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
        insert_interface_drops(std::slice::from_mut(&mut f), &VtableBodies::default());
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
        insert_interface_drops(std::slice::from_mut(&mut f), &VtableBodies::default());
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
        insert_interface_drops(std::slice::from_mut(&mut f), &VtableBodies::default());
        assert!(!has_interface_drop(&f.blocks[0].statements, local(0)), "a field read is a borrow, not a fresh allocation");
        assert!(!has_interface_drop(&f.blocks[0].statements, local(1)), "same here — this pass must not touch the struct's own field");
    }
}
