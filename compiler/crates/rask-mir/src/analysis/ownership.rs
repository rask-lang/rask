// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Who owns a value, edge by edge, and where it is released.
//!
//! A release pass has to answer three questions about every value the frame
//! makes: is it still ours here, is anything still going to read it, and which
//! name holds it. The answers differ by path. A value returned on one arm of a
//! `match` is the caller's on that arm and ours on the other; a vector copied
//! into a fused adapter's local after a loop is held by that local after the
//! loop and by the original name on a `return` inside it.
//!
//! So this tracks it forwards, per edge:
//!
//! - each name is bound to the values it might hold (`Bind`), or to something
//!   that isn't tracked;
//! - each value is owned or not on the path that got here;
//! - where paths meet and one name holds a different value on each, the name
//!   holds a new value from there on, and each incoming one is taken over by it
//!   on its own edge. That is a phi of two values, or `x = a` on one arm and
//!   `x = b` on the other, and a loop rebuilding a value from itself
//!   (`left = Expr.Add(left: left, …)`) is the same shape at its header.
//!
//! A value is released where it is owned and none of the names that might hold
//! it is live any more, under a name that certainly holds it there. Where no
//! name does, or the value is ours on some paths into a point and not others,
//! nothing is released: that is a leak, never a double free.
//!
//! The pass supplies the facts — which statement makes a value, copies a name,
//! reads into one, hands one over — because what counts as each is particular
//! to what the pass releases. Liveness is computed here from the pass's reads
//! and writes, because one of the questions only this analysis can answer
//! feeds back into it: whether a store into a slot that already holds a value
//! is the next field of that value or a new value written over it (`Fill`).

use std::collections::{BTreeMap, BTreeSet, HashSet};

use crate::analysis::cfg;
use crate::analysis::dominators::DominatorTree;
use crate::{
    BlockId, LocalId, MirBlock, MirFunction, MirOperand, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind,
};

/// A value: where it was made, and the name it was made under. A join's value
/// has `at == u32::MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Value {
    block: u32,
    at: u32,
    name: LocalId,
}

impl Value {
    /// The name it was made under; a join's value wasn't made by one statement.
    fn made(self) -> Option<LocalId> {
        (self.at != u32::MAX).then_some(self.name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Bind {
    /// The name holds the value itself, and releasing under it releases it.
    Own(Value),
    /// The name holds part of the value: a payload read out of it, or a
    /// container handle out of one of its fields. Handing a part on hands the
    /// value on; releasing has to name the whole.
    Part(Value),
    /// The name reaches into storage the value owns, like an element read out
    /// of a vector it holds. It keeps the value needed, and handing it on
    /// doesn't give the value away.
    View(Value),
    /// Something this analysis doesn't track.
    Other,
}

impl Bind {
    fn value(self) -> Option<Value> {
        match self {
            Bind::Own(v) | Bind::Part(v) | Bind::View(v) => Some(v),
            Bind::Other => None,
        }
    }
}

/// What one statement does to the names the pass tracks, in order.
#[derive(Clone, Debug)]
pub enum Event {
    /// The name now holds a value made here, which is ours.
    Make(LocalId),
    /// `dst = src`: both name the same value.
    Alias { dst: LocalId, src: LocalId },
    /// `dst` is part of whatever `base` holds.
    Part { dst: LocalId, base: LocalId },
    /// `dst` reaches into whatever `base` holds. Several in one statement add
    /// up: a closure capturing two values reaches into both.
    View { dst: LocalId, base: LocalId },
    /// `dst` also reaches into whatever `base` holds, on top of what it
    /// reached already: a scratch slot filled field by field.
    ViewAlso { dst: LocalId, base: LocalId },
    /// `dst` holds a new value, ours exactly when what every one of `srcs`
    /// held was ours here, and each is handed over either way. A call that
    /// gives back one of the containers it took, or one it built in their
    /// place: the caller owns what comes back if it owned everything that
    /// might. It may not be the same container as any of them, so it can't be
    /// an `Alias`.
    Remake { dst: LocalId, srcs: Vec<LocalId> },
    /// The name now holds something untracked.
    Other(LocalId),
    /// Whatever the name holds leaves the frame here, on this path.
    HandOver(LocalId),
    /// A hand-over into a closure environment that releases what it is given.
    /// The same as `HandOver` for this frame; `plan_carrying` also reports
    /// where the environment really was handed the value, so the pass can
    /// tell the environment to release exactly those.
    Carry(LocalId),
    /// A whole-width store into the slot. The next field of the value already
    /// there, or a new value over one that lives on under another name; this
    /// analysis decides which.
    Fill(LocalId),
    /// Something may write into the slot in place: a call given its address.
    /// The slot is where the value is from here on; other names that copied it
    /// hold stale bytes.
    WriteThrough(LocalId),
}

impl Event {
    /// The name the event is about.
    fn name(&self) -> LocalId {
        match self {
            Event::Make(n)
            | Event::Other(n)
            | Event::HandOver(n)
            | Event::Carry(n)
            | Event::Fill(n)
            | Event::WriteThrough(n) => *n,
            Event::Alias { dst, .. }
            | Event::Remake { dst, .. }
            | Event::Part { dst, .. }
            | Event::View { dst, .. }
            | Event::ViewAlso { dst, .. } => *dst,
        }
    }
}

/// The pass's facts about one function, indexed `[block][statement]` in
/// `func.blocks` order.
pub struct Facts {
    /// Names liveness is computed for. Every name an event mentions.
    pub names: BTreeSet<LocalId>,
    pub events: Vec<Vec<Vec<Event>>>,
    pub terminator_events: Vec<Vec<Event>>,
    /// Names each statement reads. Not a phi's operands (those are read on
    /// their edge) and not the slot of a `Fill`.
    pub reads: Vec<Vec<Vec<LocalId>>>,
    /// Names each statement writes over. Not the slot of a `Fill`.
    pub kills: Vec<Vec<Vec<LocalId>>>,
    pub terminator_reads: Vec<Vec<LocalId>>,
    /// Names holding something that isn't ours on entry: the parameters.
    pub foreign: Vec<LocalId>,
    /// Parameters that are ours on entry, because the caller gave them up.
    pub owned: Vec<LocalId>,
}

/// A release to insert, under `name`. `made` is the name the value was made
/// under, so a pass that has to act on what the value was built from (a
/// closure's captures) finds the statement that built it. `None` for a value
/// that is one of several, after a join.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Release {
    /// Before statement `at` of block `block` (an index into the original
    /// statements and blocks).
    At { block: usize, at: usize, name: LocalId, made: Option<LocalId> },
    /// On the edge between two blocks: the value is still needed on the way
    /// out of `from`, because another successor reads it, and not by `to`.
    OnEdge { from: BlockId, to: BlockId, name: LocalId, made: Option<LocalId> },
}

#[derive(Clone, Debug, Default, PartialEq)]
struct State {
    bind: BTreeMap<LocalId, BTreeSet<Bind>>,
    own: BTreeMap<Value, bool>,
}

impl State {
    fn binds(&self, name: LocalId) -> BTreeSet<Bind> {
        self.bind.get(&name).cloned().unwrap_or_else(|| BTreeSet::from([Bind::Other]))
    }

    fn owned(&self, v: Value) -> bool {
        self.own.get(&v).copied().unwrap_or(false)
    }

    /// Some name in `live` might hold `v`.
    fn needed(&self, v: Value, live: &BTreeSet<LocalId>) -> bool {
        live.iter().any(|n| self.mentions(*n, v))
    }

    fn mentions(&self, name: LocalId, v: Value) -> bool {
        self.bind.get(&name).is_some_and(|s| s.iter().any(|b| b.value() == Some(v)))
    }

    /// Names that certainly hold `v` itself.
    fn holders(&self, v: Value) -> Vec<LocalId> {
        self.bind
            .iter()
            .filter(|(_, s)| s.len() == 1 && s.contains(&Bind::Own(v)))
            .map(|(n, _)| *n)
            .collect()
    }
}

/// Per block, per name: live before each statement, then before the
/// terminator, then on the way out. `[block][position]`.
struct Live {
    at: Vec<Vec<BTreeSet<LocalId>>>,
    out: Vec<BTreeSet<LocalId>>,
}

struct Shape {
    preds: Vec<Vec<usize>>,
    succs: Vec<Vec<usize>>,
    rpo: Vec<usize>,
    /// `(dst, [(pred index, operand local)])` per block.
    phis: Vec<Vec<(LocalId, Vec<(usize, Option<LocalId>)>)>>,
    /// Phi operands each block hands its successors on the way out.
    phi_out: Vec<BTreeSet<LocalId>>,
}

fn shape(func: &MirFunction, names: &BTreeSet<LocalId>) -> Shape {
    let index_of: std::collections::HashMap<BlockId, usize> =
        func.blocks.iter().enumerate().map(|(i, b)| (b.id, i)).collect();
    let n = func.blocks.len();
    let mut succs = vec![Vec::new(); n];
    let mut preds = vec![Vec::new(); n];
    for (bi, b) in func.blocks.iter().enumerate() {
        for s in cfg::successors(&b.terminator) {
            if let Some(&si) = index_of.get(&s) {
                if !succs[bi].contains(&si) {
                    succs[bi].push(si);
                    preds[si].push(bi);
                }
            }
        }
    }
    let dom = DominatorTree::build(func);
    let mut rpo: Vec<usize> =
        dom.rpo_order().iter().filter_map(|b| index_of.get(b).copied()).collect();
    for bi in 0..n {
        if !rpo.contains(&bi) {
            rpo.push(bi);
        }
    }
    let mut phis = vec![Vec::new(); n];
    let mut phi_out = vec![BTreeSet::new(); n];
    for (bi, b) in func.blocks.iter().enumerate() {
        for st in &b.statements {
            let MirStmtKind::Phi { dst, args } = &st.kind else { continue };
            let mut ops = Vec::new();
            for (from, op) in args {
                let Some(&pi) = index_of.get(from) else { continue };
                let local = match op {
                    MirOperand::Local(l) => Some(*l),
                    _ => None,
                };
                if let Some(l) = local.filter(|l| names.contains(l)) {
                    phi_out[pi].insert(l);
                }
                ops.push((pi, local));
            }
            phis[bi].push((*dst, ops));
        }
    }
    Shape { preds, succs, rpo, phis, phi_out }
}

fn is_phi(func: &MirFunction, bi: usize, si: usize) -> bool {
    matches!(func.blocks[bi].statements[si].kind, MirStmtKind::Phi { .. })
}

fn liveness(func: &MirFunction, facts: &Facts, sh: &Shape, updates: &HashSet<(usize, usize)>) -> Live {
    let n = func.blocks.len();
    let phi_dsts: Vec<BTreeSet<LocalId>> =
        sh.phis.iter().map(|ps| ps.iter().map(|(d, _)| *d).collect()).collect();

    // Walk one block backwards from what is live on the way out.
    let walk = |bi: usize, out: &BTreeSet<LocalId>| -> Vec<BTreeSet<LocalId>> {
        let len = func.blocks[bi].statements.len();
        let mut at = vec![BTreeSet::new(); len + 2];
        let mut live = out.clone();
        at[len + 1] = live.clone();
        for r in &facts.terminator_reads[bi] {
            live.insert(*r);
        }
        at[len] = live.clone();
        for si in (0..len).rev() {
            if !is_phi(func, bi, si) {
                for k in &facts.kills[bi][si] {
                    live.remove(k);
                }
                for ev in &facts.events[bi][si] {
                    if let Event::Fill(slot) = ev {
                        if !updates.contains(&(bi, si)) {
                            live.remove(slot);
                        }
                    }
                }
                for r in &facts.reads[bi][si] {
                    live.insert(*r);
                }
                for ev in &facts.events[bi][si] {
                    if let Event::Fill(slot) = ev {
                        if updates.contains(&(bi, si)) {
                            live.insert(*slot);
                        }
                    }
                }
            }
            at[si] = live.clone();
        }
        at
    };

    let mut live_in: Vec<BTreeSet<LocalId>> = vec![BTreeSet::new(); n];
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 200 {
        changed = false;
        rounds += 1;
        for &bi in sh.rpo.iter().rev() {
            let mut out = sh.phi_out[bi].clone();
            for &s in &sh.succs[bi] {
                out.extend(live_in[s].iter().copied());
            }
            let at = walk(bi, &out);
            let mut inn = at[0].clone();
            for d in &phi_dsts[bi] {
                inn.remove(d);
            }
            if inn != live_in[bi] {
                live_in[bi] = inn;
                changed = true;
            }
        }
    }
    let mut at = Vec::with_capacity(n);
    let mut outs = Vec::with_capacity(n);
    for bi in 0..n {
        let mut out = sh.phi_out[bi].clone();
        for &s in &sh.succs[bi] {
            out.extend(live_in[s].iter().copied());
        }
        at.push(walk(bi, &out));
        outs.push(out);
    }
    Live { at, out: outs }
}

/// Apply one statement's events. `updates` collects the `Fill`s that turned
/// out to be the next field of the value already in the slot.
fn apply(
    st: &mut State,
    events: &[Event],
    bi: usize,
    si: usize,
    live_after: &BTreeSet<LocalId>,
    updates: &mut HashSet<(usize, usize)>,
) {
    let mut viewed: BTreeSet<LocalId> = BTreeSet::new();
    for ev in events {
        match ev {
            Event::Make(n) => make(st, *n, bi, si),
            Event::Alias { dst, src } => {
                let b = st.binds(*src);
                st.bind.insert(*dst, b);
            }
            Event::Part { dst, base } => {
                let seen: BTreeSet<Bind> = st
                    .binds(*base)
                    .into_iter()
                    .map(|b| match b {
                        Bind::Own(v) | Bind::Part(v) => Bind::Part(v),
                        Bind::View(v) => Bind::View(v),
                        Bind::Other => Bind::Other,
                    })
                    .collect();
                st.bind.insert(*dst, seen);
            }
            Event::View { dst, base } => {
                let seen: BTreeSet<Bind> = st
                    .binds(*base)
                    .into_iter()
                    .map(|b| match b {
                        Bind::Own(v) | Bind::Part(v) | Bind::View(v) => Bind::View(v),
                        Bind::Other => Bind::Other,
                    })
                    .collect();
                if viewed.insert(*dst) {
                    st.bind.insert(*dst, seen);
                } else {
                    let cur = st.bind.entry(*dst).or_default();
                    cur.extend(seen);
                    if cur.len() > 1 {
                        cur.remove(&Bind::Other);
                    }
                }
            }
            Event::ViewAlso { dst, base } => {
                let seen: Vec<Bind> = st
                    .binds(*base)
                    .into_iter()
                    .filter_map(|b| b.value())
                    .map(Bind::View)
                    .collect();
                let cur = st.bind.entry(*dst).or_default();
                cur.extend(seen);
            }
            Event::Remake { dst, srcs } => {
                let ours = !srcs.is_empty()
                    && srcs.iter().all(|src| {
                        let held = st.binds(*src);
                        !held.is_empty()
                            && held.iter().all(|b| matches!(b, Bind::Own(v) | Bind::Part(v) if st.owned(*v)))
                    });
                for src in srcs {
                    hand_over(st, *src);
                }
                if ours {
                    make(st, *dst, bi, si);
                } else {
                    st.bind.insert(*dst, BTreeSet::from([Bind::Other]));
                }
            }
            Event::Other(n) => {
                st.bind.insert(*n, BTreeSet::from([Bind::Other]));
            }
            Event::HandOver(n) | Event::Carry(n) => hand_over(st, *n),
            Event::Fill(n) => {
                let held = st.binds(*n);
                let only = (held.len() == 1).then(|| *held.iter().next().unwrap());
                match only {
                    // Writing into something that isn't ours is a field of it.
                    Some(Bind::Other) if st.bind.contains_key(n) => {}
                    Some(Bind::Own(v))
                        if st.owned(v)
                            && !live_after.iter().any(|m| {
                                m != n && st.bind.get(m).is_some_and(|s| s.contains(&Bind::Own(v)))
                            }) =>
                    {
                        updates.insert((bi, si));
                        written_through(st, *n);
                    }
                    _ => make(st, *n, bi, si),
                }
            }
            Event::WriteThrough(n) => written_through(st, *n),
        }
    }
}

/// `n`'s slot was written in place, so it alone holds the value now. A copy
/// made earlier shares the value's storage for as long as it is read, which
/// keeps the value needed, but its own bytes may be out of date and it is
/// never what the value is released under: `swap_out(mutate bag)` replaced
/// the vector in the copy it was handed, and releasing the original freed the
/// old vector a second time.
fn written_through(st: &mut State, n: LocalId) {
    let mine: Vec<Value> = st
        .binds(n)
        .into_iter()
        .filter_map(|b| match b {
            Bind::Own(v) => Some(v),
            _ => None,
        })
        .collect();
    for v in mine {
        for (m, set) in st.bind.iter_mut() {
            if *m != n && set.remove(&Bind::Own(v)) {
                set.insert(Bind::View(v));
            }
        }
    }
}

fn hand_over(st: &mut State, n: LocalId) {
    for b in st.binds(n) {
        if let Bind::Own(v) | Bind::Part(v) = b {
            if st.own.contains_key(&v) {
                st.own.insert(v, false);
            }
        }
    }
}

fn make(st: &mut State, n: LocalId, bi: usize, si: usize) {
    let v = Value { block: bi as u32, at: si as u32, name: n };
    st.bind.insert(n, BTreeSet::from([Bind::Own(v)]));
    st.own.insert(v, true);
}

/// The state on entry to `bi`, from its predecessors' exits.
/// Releases already decided, fed back so nothing is released twice: a value
/// released after a statement, or on an edge, is gone from there on.
#[derive(Default)]
struct Kills {
    at: HashSet<(usize, usize, Value)>,
    edges: HashSet<(usize, usize, Value)>,
}

fn join(
    bi: usize,
    sh: &Shape,
    exits: &[Option<State>],
    entry_live: &BTreeSet<LocalId>,
    kills: &Kills,
) -> Option<State> {
    let preds: Vec<usize> = sh.preds[bi].iter().copied().filter(|p| exits[*p].is_some()).collect();
    if preds.is_empty() {
        return None;
    }
    // Each predecessor's state as this block sees it: what was released on
    // the edge is gone, and a phi's destination holds what its operand from
    // that predecessor holds.
    let seen: Vec<State> = preds.iter().map(|&p| edge_view(bi, p, sh, exits[p].as_ref().unwrap(), kills)).collect();

    let mut out = State::default();
    let mut absorbed: BTreeSet<(usize, Value)> = BTreeSet::new();
    let mut joined: Vec<Value> = Vec::new();

    let mut names: BTreeSet<LocalId> = BTreeSet::new();
    for st in &seen {
        names.extend(st.bind.keys().copied());
    }
    for n in names {
        let sets: Vec<BTreeSet<Bind>> = seen.iter().map(|st| st.binds(n)).collect();
        if sets.iter().all(|s| *s == sets[0]) {
            // Never bound on any path stays unbound, so a later store makes a
            // value rather than writing a field of something foreign.
            if seen.iter().any(|st| st.bind.contains_key(&n)) {
                out.bind.insert(n, sets[0].clone());
            }
            continue;
        }
        // A different value on each path. Taken over by a value of its own
        // when every path brings one that's ours and nothing else needs.
        if entry_live.contains(&n) {
            let mut takes = Vec::new();
            for (k, st) in seen.iter().enumerate() {
                let one = (sets[k].len() == 1).then(|| *sets[k].iter().next().unwrap());
                // A view never holds what it reaches into, so it can't take
                // it over. `let r = if … { A{…} as any R } else { B{…} as any
                // R }` joined two boxes that only view their structs into one
                // value owned by `r`, and released the structs through the
                // box — the box freed twice and its contents leaked.
                let Some(v) = one.filter(|b| !matches!(b, Bind::View(_))).and_then(Bind::value) else {
                    break;
                };
                if !st.owned(v) || absorbed.iter().any(|(p, w)| *p == k && *w == v) {
                    break;
                }
                let elsewhere = entry_live.iter().any(|m| *m != n && st.mentions(*m, v));
                if elsewhere {
                    break;
                }
                takes.push((k, v));
            }
            if takes.len() == seen.len() {
                let j = Value { block: bi as u32, at: u32::MAX, name: n };
                absorbed.extend(takes);
                joined.push(j);
                out.bind.insert(n, BTreeSet::from([Bind::Own(j)]));
                continue;
            }
        }
        let mut u: BTreeSet<Bind> = BTreeSet::new();
        for s in sets {
            u.extend(s);
        }
        out.bind.insert(n, u);
    }

    let mut values: BTreeSet<Value> = BTreeSet::new();
    for st in &seen {
        values.extend(st.own.keys().copied());
    }
    for v in values {
        let owned = seen
            .iter()
            .enumerate()
            .all(|(k, st)| st.owned(v) && !absorbed.contains(&(k, v)));
        out.own.insert(v, owned);
    }
    for j in joined {
        out.own.insert(j, true);
    }
    Some(out)
}

/// `p`'s exit state as `bi` sees it along the edge between them.
fn edge_view(bi: usize, p: usize, sh: &Shape, exit: &State, kills: &Kills) -> State {
    let mut st = exit.clone();
    for (k_from, k_to, v) in &kills.edges {
        if *k_from == p && *k_to == bi {
            st.own.insert(*v, false);
        }
    }
    for (dst, ops) in &sh.phis[bi] {
        let b = ops
            .iter()
            .find(|(from, _)| *from == p)
            .and_then(|(_, l)| *l)
            .map(|l| exit.binds(l))
            .unwrap_or_else(|| BTreeSet::from([Bind::Other]));
        st.bind.insert(*dst, b);
    }
    st
}

fn transfer(
    func: &MirFunction,
    facts: &Facts,
    live: &Live,
    bi: usize,
    entry: &State,
    kills: &Kills,
    updates: &mut HashSet<(usize, usize)>,
) -> State {
    let mut st = entry.clone();
    for si in 0..func.blocks[bi].statements.len() {
        if is_phi(func, bi, si) {
            continue;
        }
        apply(&mut st, &facts.events[bi][si], bi, si, &live.at[bi][si + 1], updates);
        killed_at(&mut st, kills, bi, si);
    }
    let len = func.blocks[bi].statements.len();
    apply(&mut st, &facts.terminator_events[bi], bi, len, &live.out[bi], updates);
    st
}

fn killed_at(st: &mut State, kills: &Kills, bi: usize, si: usize) {
    for (b, s, v) in &kills.at {
        if *b == bi && *s == si {
            st.own.insert(*v, false);
        }
    }
}

fn entry_state(facts: &Facts) -> State {
    let mut st = State::default();
    for n in &facts.foreign {
        st.bind.insert(*n, BTreeSet::from([Bind::Other]));
    }
    // Made before the first statement. `u32::MAX` is a join's, so one short.
    for n in &facts.owned {
        let v = Value { block: 0, at: u32::MAX - 1, name: *n };
        st.bind.insert(*n, BTreeSet::from([Bind::Own(v)]));
        st.own.insert(v, true);
    }
    st
}

fn solve(
    func: &MirFunction,
    facts: &Facts,
    sh: &Shape,
    live: &Live,
    kills: &Kills,
    updates: &mut HashSet<(usize, usize)>,
) -> (Vec<Option<State>>, Vec<Option<State>>) {
    let n = func.blocks.len();
    let mut entries: Vec<Option<State>> = vec![None; n];
    let mut exits: Vec<Option<State>> = vec![None; n];
    let mut changed = true;
    let mut rounds = 0;
    while changed && rounds < 100 {
        changed = false;
        rounds += 1;
        updates.clear();
        for &bi in &sh.rpo {
            let entry = if bi == 0 {
                Some(entry_state(facts))
            } else {
                join(bi, sh, &exits, &live.at[bi][0], kills)
            };
            let Some(entry) = entry else { continue };
            let exit = transfer(func, facts, live, bi, &entry, kills, updates);
            if exits[bi].as_ref() != Some(&exit) {
                exits[bi] = Some(exit);
                changed = true;
            }
            entries[bi] = Some(entry);
        }
    }
    (entries, exits)
}

/// Joins where a value is still needed, seen from each block.
///
/// Releasing a value on a path that later meets one where it is still needed
/// leaves it the frame's on one path into the join and not the other. From
/// there on it can't be released at all: no name certainly holds it. So a
/// value dead on one path waits until it is dead on every path that meets
/// it, and goes once, after the join.
struct NeededLater {
    succs: Vec<Vec<usize>>,
    /// Per value, the join blocks that still need it on the way in.
    needing: std::collections::BTreeMap<Value, HashSet<usize>>,
}

impl NeededLater {
    fn new(func: &MirFunction, sh: &Shape, live: &Live, entries: &[Option<State>]) -> Self {
        let mut needing: std::collections::BTreeMap<Value, HashSet<usize>> = Default::default();
        for b in 0..func.blocks.len() {
            if sh.preds[b].len() < 2 {
                continue;
            }
            let Some(e) = &entries[b] else { continue };
            let first = func.blocks[b]
                .statements
                .iter()
                .take_while(|s| matches!(s.kind, MirStmtKind::Phi { .. }))
                .count();
            for v in e.own.keys() {
                if e.needed(*v, &live.at[b][first]) {
                    needing.entry(*v).or_default().insert(b);
                }
            }
        }
        NeededLater { succs: sh.succs.clone(), needing }
    }

    /// Some join reachable from `b`, `b` included, still needs `v`. Not
    /// through the block that makes `v`: past it, it is the next turn's.
    fn from(&self, b: usize, v: Value) -> bool {
        let Some(joins) = self.needing.get(&v) else { return false };
        let made = v.block as usize;
        if b == made {
            return false;
        }
        let mut seen: HashSet<usize> = HashSet::from([b]);
        let mut frontier = vec![b];
        while let Some(x) = frontier.pop() {
            if joins.contains(&x) {
                return true;
            }
            for &s in &self.succs[x] {
                if s != made && seen.insert(s) {
                    frontier.push(s);
                }
            }
        }
        false
    }
}

/// Where a release may go.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Placement {
    /// Right after the last use, inside a block or on an edge. For values
    /// whose every reader this analysis is told about.
    LastUse,
    /// Only where the scope a value was made in ends: before a return, on a
    /// loop's back edge, on an edge out of the region its making dominates,
    /// and on the way into a block several paths meet at. For values that may
    /// have readers this analysis can't see — a pointer into a vector's
    /// buffer — so freeing between a last named use and the end of the scope
    /// would be freeing under them.
    ScopeEnd,
}

/// Where each value this frame owns is released and under which name, and
/// where its unwind slot is armed and disarmed.
pub struct Plan {
    pub releases: Vec<Release>,
    pub unwind: Vec<Mark>,
}

/// Where each value this frame owns is released, and under which name.
pub fn plan(func: &MirFunction, facts: &Facts, placement: Placement) -> Plan {
    plan_carrying(func, facts, placement, &|_, _| false).0
}

/// A `Carry` that handed the environment a value it now owns outright:
/// `(block, statement, name)`, by index.
pub type Carried = BTreeSet<(usize, usize, LocalId)>;

/// `plan`, plus the `Carry` events that handed the environment something it
/// now owns outright: the name certainly held one value this frame owned, or
/// a part of one, and nothing the frame reads afterwards holds the same bytes.
/// Those are what the environment has to release.
///
/// `disjoint(carried, other)` is the pass's answer to "do these two names
/// certainly hold different parts of the value". It is what lets
/// `let (req, r) = accept()` carry `req` while `r` goes elsewhere. The value
/// is handed over whole either way, so whatever else is in it is left to
/// whoever it went to, which is a leak at worst.
///
/// Everything else a `Carry` names — a parameter, one value on some paths and
/// another on others, a Copy value the frame keeps reading — the environment
/// must leave alone, because something else may still free it or read it.
pub fn plan_carrying(
    func: &MirFunction,
    facts: &Facts,
    placement: Placement,
    disjoint: &dyn Fn(LocalId, LocalId) -> bool,
) -> (Plan, Carried) {
    if func.blocks.is_empty() {
        return (Plan { releases: Vec::new(), unwind: Vec::new() }, Carried::new());
    }
    let sh = shape(func, &facts.names);
    let ids: Vec<BlockId> = func.blocks.iter().map(|b| b.id).collect();

    // Liveness and the `Fill` verdicts feed each other: a store that is the
    // next field of a value is a use of its slot, and one that writes a new
    // value over it ends the old one there. Settle them together.
    let mut kills = Kills::default();
    let mut updates: HashSet<(usize, usize)> = HashSet::new();
    let mut live = liveness(func, facts, &sh, &updates);
    for _ in 0..6 {
        let mut found = HashSet::new();
        solve(func, facts, &sh, &live, &kills, &mut found);
        if found == updates {
            break;
        }
        updates = found;
        live = liveness(func, facts, &sh, &updates);
    }

    // Place, feed what was placed back in, solve again. A release on an edge
    // ends the value for everything after it, and only a fresh solve knows
    // that; without it a later statement that touches one of the value's old
    // names would release it a second time. Each round only adds.
    let mut out: Vec<Release> = Vec::new();
    let order: Vec<usize> = {
        let mut o = vec![usize::MAX; func.blocks.len()];
        for (k, &bi) in sh.rpo.iter().enumerate() {
            o[bi] = k;
        }
        o
    };
    let edges: usize = sh.succs.iter().map(|s| s.len()).sum();
    for _ in 0..(4 * edges + 64) {
        let (entries, exits) = solve(func, facts, &sh, &live, &kills, &mut HashSet::new());
        let later = NeededLater::new(func, &sh, &live, &entries);
        if placement == Placement::LastUse {
            let found = after_last_use(func, facts, &sh, &live, &entries, &kills, &later);
            if !found.is_empty() {
                for (block, si, name, v) in found {
                    out.push(Release::At { block, at: si + 1, name, made: v.made() });
                    kills.at.insert((block, si, v));
                }
                continue;
            }
        }
        let found = on_edges(func, facts, &sh, &live, &entries, &exits, &kills, &later, placement);
        if found.is_empty() {
            if placement == Placement::ScopeEnd {
                before_returns(func, facts, &live, &entries, &kills, &mut out);
            }
            break;
        }
        // One edge per solve, the earliest in program order: a release on it
        // changes what reaches everything after it, a loop's own back edge
        // included, so the rest wait for the next solve.
        let first = found.iter().map(|(pi, bi, _, _)| (order[*bi], order[*pi])).min().unwrap();
        for (pi, bi, name, v) in found {
            if (order[bi], order[pi]) != first {
                continue;
            }
            out.push(Release::OnEdge { from: ids[pi], to: ids[bi], name, made: v.made() });
            kills.edges.insert((pi, bi, v));
        }
    }
    out.sort();
    out.dedup();
    if std::env::var("RASK_OWNERSHIP_TRACE").is_ok_and(|v| v == func.name) {
        let (entries, _) = solve(func, facts, &sh, &live, &kills, &mut HashSet::new());
        for bi in 0..func.blocks.len() {
            eprintln!("bb{} entry {:?}", ids[bi].0, entries[bi]);
            eprintln!("    live in {:?} out {:?}", live.at[bi][0], live.out[bi]);
        }
        eprintln!("fill updates {:?}", updates);
        eprintln!("releases {:?}", out);
    }
    let (entries, exits) = solve(func, facts, &sh, &live, &kills, &mut HashSet::new());
    let carried = if facts.events.iter().flatten().flatten().any(|e| matches!(e, Event::Carry(_))) {
        carried_outright(func, facts, &live, &entries, &kills, disjoint)
    } else {
        Carried::new()
    };
    let unwind = unwind_marks(func, facts, &sh, &live, &entries, &exits, &kills);
    (Plan { releases: out, unwind }, carried)
}

/// The `Carry` events that hand over an owned value, or part of one, that
/// nothing else reads afterwards, against the settled state.
fn carried_outright(
    func: &MirFunction,
    facts: &Facts,
    live: &Live,
    entries: &[Option<State>],
    kills: &Kills,
    disjoint: &dyn Fn(LocalId, LocalId) -> bool,
) -> Carried {
    let mut out = Carried::new();
    for bi in 0..func.blocks.len() {
        let Some(entry) = &entries[bi] else { continue };
        let mut st = entry.clone();
        for si in 0..func.blocks[bi].statements.len() {
            if is_phi(func, bi, si) {
                continue;
            }
            let events = &facts.events[bi][si];
            for ev in events {
                let Event::Carry(n) = ev else { continue };
                let held = st.binds(*n);
                let Some(Bind::Own(v) | Bind::Part(v)) =
                    (held.len() == 1).then(|| *held.iter().next().unwrap())
                else {
                    continue;
                };
                if !st.owned(v) {
                    continue;
                }
                // The names this statement defines aren't the frame's: the
                // closure the value went into is the one being built here.
                let defined: Vec<LocalId> = events
                    .iter()
                    .filter_map(|e| match e {
                        Event::Other(d) | Event::Make(d) => Some(*d),
                        _ => None,
                    })
                    .collect();
                let read_later = live.at[bi][si + 1]
                    .iter()
                    .any(|m| !defined.contains(m) && st.mentions(*m, v) && !disjoint(*n, *m));
                if !read_later {
                    out.insert((bi, si, *n));
                }
            }
            apply(&mut st, events, bi, si, &live.at[bi][si + 1], &mut HashSet::new());
            killed_at(&mut st, kills, bi, si);
        }
    }
    out
}

/// Releases right after a value's last use inside a block: `(block,
/// statement, name, value)`.
fn after_last_use(
    func: &MirFunction,
    facts: &Facts,
    sh: &Shape,
    live: &Live,
    entries: &[Option<State>],
    kills: &Kills,
    later: &NeededLater,
) -> Vec<(usize, usize, LocalId, Value)> {
    let mut out = Vec::new();
    for bi in 0..func.blocks.len() {
        let Some(entry) = &entries[bi] else { continue };
        let mut st = entry.clone();
        for si in 0..func.blocks[bi].statements.len() {
            if is_phi(func, bi, si) {
                continue;
            }
            let before = st.clone();
            apply(&mut st, &facts.events[bi][si], bi, si, &live.at[bi][si + 1], &mut HashSet::new());
            killed_at(&mut st, kills, bi, si);
            let touched: Vec<LocalId> = facts.reads[bi][si]
                .iter()
                .chain(facts.kills[bi][si].iter())
                .copied()
                .chain(facts.events[bi][si].iter().map(Event::name))
                .collect();
            let values: Vec<Value> = st.own.iter().filter(|(_, o)| **o).map(|(v, _)| *v).collect();
            for v in values {
                if st.needed(v, &live.at[bi][si + 1]) {
                    continue;
                }
                let made_here = v.block as usize == bi && v.at as usize == si;
                let alive_before = (before.owned(v) && before.needed(v, &live.at[bi][si]))
                    || made_here
                    || touched.iter().any(|n| before.mentions(*n, v) || st.mentions(*n, v));
                if !alive_before || sh.succs[bi].iter().any(|&c| later.from(c, v)) {
                    continue;
                }
                if let Some(name) = pick(&st, v, &touched) {
                    out.push((bi, si, name, v));
                    st.own.insert(v, false);
                }
            }
        }
    }
    out
}

/// Releases on edges: `(from, to, name, value)` by block index.
#[allow(clippy::too_many_arguments)]
fn on_edges(
    func: &MirFunction,
    facts: &Facts,
    sh: &Shape,
    live: &Live,
    entries: &[Option<State>],
    exits: &[Option<State>],
    kills: &Kills,
    later: &NeededLater,
    placement: Placement,
) -> Vec<(usize, usize, LocalId, Value)> {
    let mut out = Vec::new();
    for pi in 0..func.blocks.len() {
        let Some(px) = &exits[pi] else { continue };
        // A cleanup return's successors are the `ensure` blocks it runs on the
        // way out; what it returns flows through them to the caller.
        if matches!(func.blocks[pi].terminator.kind, MirTerminatorKind::CleanupReturn { .. }) {
            continue;
        }
        let mut p_out = live.out[pi].clone();
        p_out.extend(facts.terminator_reads[pi].iter().copied());
        let values: Vec<Value> = px.own.iter().filter(|(_, o)| **o).map(|(v, _)| *v).collect();
        for &bi in &sh.succs[pi] {
            let seen = edge_view(bi, pi, sh, px, kills);
            let first = func.blocks[bi]
                .statements
                .iter()
                .take_while(|s| matches!(s.kind, MirStmtKind::Phi { .. }))
                .count();
            for &v in &values {
                if !seen.owned(v) || seen.needed(v, &live.at[bi][first]) {
                    continue;
                }
                let site = match placement {
                    // Needed on the way out because another successor reads
                    // it; dead on the way in here.
                    Placement::LastUse => px.needed(v, &p_out) && !later.from(bi, v),
                    // The last chance: past this edge the value isn't
                    // certainly ours any more, because another way in
                    // doesn't bring it — a back edge, the edge out of the arm
                    // that made it, a join where another path handed it on.
                    // Anywhere else it is still ours further on, and waits.
                    // A join value made at `bi` is a new one on every way
                    // in, so the one arriving here is always the last turn's.
                    Placement::ScopeEnd => {
                        let remade = v.block as usize == bi && v.at == u32::MAX;
                        remade || !entries[bi].as_ref().is_some_and(|e| e.owned(v))
                    }
                };
                if !site {
                    continue;
                }
                if let Some(name) = pick(px, v, &[]) {
                    out.push((pi, bi, name, v));
                }
            }
        }
    }
    out
}

/// Whatever is still ours before a return.
fn before_returns(
    func: &MirFunction,
    facts: &Facts,
    live: &Live,
    entries: &[Option<State>],
    kills: &Kills,
    out: &mut Vec<Release>,
) {
    for bi in 0..func.blocks.len() {
        if !matches!(
            func.blocks[bi].terminator.kind,
            MirTerminatorKind::Return { .. } | MirTerminatorKind::CleanupReturn { .. }
        ) {
            continue;
        }
        let Some(e) = &entries[bi] else { continue };
        let st = transfer(func, facts, live, bi, e, kills, &mut HashSet::new());
        let len = func.blocks[bi].statements.len();
        for (v, owned) in &st.own {
            if *owned {
                if let Some(name) = pick(&st, *v, &[]) {
                    out.push(Release::At { block: bi, at: len, name, made: v.made() });
                }
            }
        }
    }
}

/// The name to release `v` under: one that certainly holds it, preferring one
/// the statement at hand touches.
fn pick(st: &State, v: Value, touched: &[LocalId]) -> Option<LocalId> {
    let holders = st.holders(v);
    holders
        .iter()
        .copied()
        .filter(|n| touched.contains(n))
        .min()
        .or_else(|| holders.iter().copied().min())
}

// ─── Unwind marks (ctrl.panic/U6) ──────────────────────────
//
// A panic longjmps past every release the plan placed, so each value the frame
// owns also sits in a slot of the frame's unwind record while it is ours. The
// slot is armed where the value becomes ours under a name that certainly holds
// it, and disarmed where it stops being ours: handed over, released, or no
// longer held by that name. Read off the same settled state the releases were,
// so the two never disagree about whose a value is.

/// Where an unwind mark goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkAt {
    /// Before statement `at` of block `block`, by index into the blocks as
    /// planned. `late` marks belong to statement `at` itself (a hand-over in
    /// it); the others to the statement before it (a value it made), and go
    /// first.
    In { block: usize, at: usize, late: bool },
    /// On the edge between two blocks.
    Edge { from: BlockId, to: BlockId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarkKind {
    /// `slot` holds what `name` holds from here on. `made` is the name the
    /// value was made under, as `Release` carries it.
    Arm { slot: u32, name: LocalId, made: Option<LocalId> },
    Disarm { slot: u32 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    pub at: MarkAt,
    pub kind: MarkKind,
}

#[allow(clippy::too_many_arguments)]
fn unwind_marks(
    func: &MirFunction,
    facts: &Facts,
    sh: &Shape,
    live: &Live,
    entries: &[Option<State>],
    exits: &[Option<State>],
    kills: &Kills,
) -> Vec<Mark> {
    let n = func.blocks.len();
    let ids: Vec<BlockId> = func.blocks.iter().map(|b| b.id).collect();
    // Nothing is armed in an `ensure` body a cleanup return runs: the values
    // reaching it were released at the cleanup return.
    let cleanup_ids = cfg::cleanup_only_blocks(func);
    let cleanup: Vec<bool> = ids.iter().map(|id| cleanup_ids.contains(id)).collect();
    let mut slots: BTreeMap<(Value, LocalId), u32> = BTreeMap::new();
    let mut slot = |v: Value, name: LocalId| -> u32 {
        let next = slots.len() as u32;
        *slots.entry((v, name)).or_insert(next)
    };
    let mut marks: Vec<Mark> = Vec::new();
    let mut entry_arm: Vec<Option<BTreeMap<Value, LocalId>>> = vec![None; n];
    let mut exit_arm: Vec<BTreeMap<Value, LocalId>> = vec![BTreeMap::new(); n];

    for &bi in &sh.rpo {
        let Some(entry) = &entries[bi] else { continue };
        if cleanup[bi] || exits[bi].is_none() {
            entry_arm[bi] = Some(BTreeMap::new());
            continue;
        }
        // Keep the name the way in armed it under, when every way in seen so
        // far agrees and it still holds the value; otherwise the usual pick.
        let mut arm: BTreeMap<Value, LocalId> = BTreeMap::new();
        for (v, owned) in &entry.own {
            if !*owned {
                continue;
            }
            let holders = entry.holders(*v);
            if holders.is_empty() {
                continue;
            }
            let mut agreed: Option<Option<LocalId>> = None;
            for &p in &sh.preds[bi] {
                if entry_arm[p].is_none() {
                    continue;
                }
                let m = exit_arm[p].get(v).copied();
                agreed = match agreed {
                    None => Some(m),
                    Some(x) if x == m => Some(x),
                    _ => Some(None),
                };
            }
            let kept = agreed.flatten().filter(|m| holders.contains(m));
            if let Some(name) = kept.or_else(|| pick(entry, *v, &[])) {
                arm.insert(*v, name);
            }
        }
        entry_arm[bi] = Some(arm.clone());

        let mut st = entry.clone();
        let len = func.blocks[bi].statements.len();
        for si in 0..len {
            if is_phi(func, bi, si) {
                continue;
            }
            apply(&mut st, &facts.events[bi][si], bi, si, &live.at[bi][si + 1], &mut HashSet::new());
            // Handed over, or no longer held by the name it was armed under:
            // gone before the statement runs, because a callee that panics
            // with it is the one that owns it. A value made again here (a
            // loop's next turn) is a new one and is armed afresh below.
            for (v, name) in arm.clone() {
                let made_here = v.block as usize == bi && v.at as usize == si;
                if made_here || !st.owned(v) || !st.holders(v).contains(&name) {
                    marks.push(Mark {
                        at: MarkAt::In { block: bi, at: si, late: true },
                        kind: MarkKind::Disarm { slot: slot(v, name) },
                    });
                    arm.remove(&v);
                }
            }
            // Released right after it.
            killed_at(&mut st, kills, bi, si);
            for (v, name) in arm.clone() {
                if !st.owned(v) {
                    marks.push(Mark {
                        at: MarkAt::In { block: bi, at: si + 1, late: false },
                        kind: MarkKind::Disarm { slot: slot(v, name) },
                    });
                    arm.remove(&v);
                }
            }
            let owned: Vec<Value> = st.own.iter().filter(|(_, o)| **o).map(|(v, _)| *v).collect();
            for v in owned {
                if arm.contains_key(&v) {
                    continue;
                }
                if let Some(name) = pick(&st, v, &[]) {
                    marks.push(Mark {
                        at: MarkAt::In { block: bi, at: si + 1, late: false },
                        kind: MarkKind::Arm { slot: slot(v, name), name, made: v.made() },
                    });
                    arm.insert(v, name);
                }
            }
        }
        // A cleanup return released everything still ours just before it; the
        // `ensure` bodies it runs next can still panic.
        apply(&mut st, &facts.terminator_events[bi], bi, len, &live.out[bi], &mut HashSet::new());
        let cleanup_return =
            matches!(func.blocks[bi].terminator.kind, MirTerminatorKind::CleanupReturn { .. });
        for (v, name) in arm.clone() {
            if cleanup_return || !st.owned(v) || !st.holders(v).contains(&name) {
                marks.push(Mark {
                    at: MarkAt::In { block: bi, at: len, late: true },
                    kind: MarkKind::Disarm { slot: slot(v, name) },
                });
                arm.remove(&v);
            }
        }
        exit_arm[bi] = arm;
    }

    // Each edge hands the successor what it armed. A slot the successor
    // doesn't keep is disarmed on the edge; one it keeps under a name some way
    // in didn't arm is armed on entry.
    let mut need: Vec<BTreeSet<Value>> = vec![BTreeSet::new(); n];
    for p in 0..n {
        if entry_arm[p].is_none() || cleanup[p] {
            continue;
        }
        for &b in &sh.succs[p] {
            if cleanup[b] {
                continue;
            }
            let Some(earm) = &entry_arm[b] else { continue };
            for (v, name) in &exit_arm[p] {
                if earm.get(v) != Some(name) {
                    marks.push(Mark {
                        at: MarkAt::Edge { from: ids[p], to: ids[b] },
                        kind: MarkKind::Disarm { slot: slot(*v, *name) },
                    });
                }
            }
            for (v, name) in earm {
                if exit_arm[p].get(v) != Some(name) {
                    need[b].insert(*v);
                }
            }
        }
    }
    for b in 0..n {
        let Some(earm) = &entry_arm[b] else { continue };
        let first = func.blocks[b]
            .statements
            .iter()
            .take_while(|s| matches!(s.kind, MirStmtKind::Phi { .. }))
            .count();
        for (v, name) in earm {
            if b == 0 || need[b].contains(v) {
                marks.push(Mark {
                    at: MarkAt::In { block: b, at: first, late: false },
                    kind: MarkKind::Arm { slot: slot(*v, *name), name: *name, made: v.made() },
                });
            }
        }
    }
    marks
}

/// Where each statement a pass planned against now sits, once unwind marks
/// went in ahead of it.
pub struct Shifted {
    at: Vec<Vec<usize>>,
}

impl Shifted {
    /// The index that is "before statement `at`" in the planned blocks, after
    /// any marks placed there.
    pub fn at(&self, block: usize, at: usize) -> usize {
        self.at[block].get(at).copied().unwrap_or(at)
    }
}

/// The first unwind slot no statement in `func` uses yet.
pub fn next_unwind_slot(func: &MirFunction) -> u32 {
    func.blocks
        .iter()
        .flat_map(|b| b.statements.iter())
        .filter_map(|s| match &s.kind {
            MirStmtKind::UnwindArm { slot, .. } | MirStmtKind::UnwindDisarm { slot } => Some(*slot + 1),
            _ => None,
        })
        .max()
        .unwrap_or(0)
}

/// An edge and the statements that go on it.
pub type EdgeStmts = Vec<(BlockId, BlockId, Vec<MirStmt>)>;

/// Turn a plan's unwind marks into statements and put the in-block ones in.
///
/// `release(func, name, made)` is the pass's own release of what `name` holds,
/// the statements it would place at a release under that name. Run against the
/// blocks as planned, before the pass inserts anything of its own; the pass
/// then places its in-block releases through the returned `Shifted`, and hands
/// the returned edge marks to `insert_on_edges` together with its own edge
/// releases, so one edge gets one block.
pub fn place_unwind(
    func: &mut MirFunction,
    marks: Vec<Mark>,
    release: &mut dyn FnMut(&mut MirFunction, LocalId, Option<LocalId>) -> Vec<MirStmt>,
) -> (Shifted, EdgeStmts) {
    let base = next_unwind_slot(func);
    let mut templates: BTreeMap<u32, crate::UnwindRelease> = BTreeMap::new();
    let mut in_block: Vec<Vec<(usize, bool, MirStmt)>> = vec![Vec::new(); func.blocks.len()];
    let mut edges: EdgeStmts = Vec::new();
    for m in marks {
        let kind = match m.kind {
            MarkKind::Arm { slot, name, made } => {
                let slot = base + slot;
                let release = match templates.get(&slot) {
                    Some(t) => t.clone(),
                    None => {
                        let t = crate::UnwindRelease { placeholder: name, stmts: release(func, name, made) };
                        templates.insert(slot, t.clone());
                        t
                    }
                };
                MirStmtKind::UnwindArm { slot, value: name, release }
            }
            MarkKind::Disarm { slot } => MirStmtKind::UnwindDisarm { slot: base + slot },
        };
        match m.at {
            MarkAt::In { block, at, late } => in_block[block].push((at, late, MirStmt::dummy(kind))),
            MarkAt::Edge { from, to } => match edges.iter_mut().find(|(f, t, _)| *f == from && *t == to) {
                Some((_, _, all)) => all.push(MirStmt::dummy(kind)),
                None => edges.push((from, to, vec![MirStmt::dummy(kind)])),
            },
        }
    }
    let mut shifted = Vec::with_capacity(func.blocks.len());
    for (bi, mut here) in in_block.into_iter().enumerate() {
        let block = &mut func.blocks[bi];
        let len = block.statements.len();
        if here.is_empty() {
            shifted.push((0..=len).collect());
            continue;
        }
        here.sort_by_key(|(at, late, _)| (*at, *late));
        let mut here = here.into_iter().peekable();
        let mut old = std::mem::take(&mut block.statements).into_iter();
        let mut index = Vec::with_capacity(len + 1);
        let mut out: Vec<MirStmt> = Vec::with_capacity(len);
        for _ in 0..=len {
            let i = index.len();
            while let Some((_, _, st)) = here.next_if(|(at, _, _)| *at == i) {
                out.push(st);
            }
            index.push(out.len());
            if let Some(st) = old.next() {
                out.push(st);
            }
        }
        block.statements = out;
        shifted.push(index);
    }
    (Shifted { at: shifted }, edges)
}

/// Merge two lists of edge statements, one entry per edge, `first`'s
/// statements ahead.
pub fn merge_edges(first: EdgeStmts, then: EdgeStmts) -> EdgeStmts {
    let mut out = first;
    for (from, to, stmts) in then {
        match out.iter_mut().find(|(f, t, _)| *f == from && *t == to) {
            Some((_, _, all)) => all.extend(stmts),
            None => out.push((from, to, stmts)),
        }
    }
    out
}

/// Put each edge's releases on its edge: at the top of the successor when this
/// is the only way into it, and otherwise in a new block between the two, so
/// the release runs on this edge and no other.
pub fn insert_on_edges(func: &mut MirFunction, edges: Vec<(BlockId, BlockId, Vec<MirStmt>)>) {
    if edges.is_empty() {
        return;
    }
    let preds = cfg::predecessors(func);
    let mut next_id = func.blocks.iter().map(|b| b.id.0).max().unwrap_or(0) + 1;
    // One edge can carry several releases; they go in one block.
    let mut merged: Vec<(BlockId, BlockId, Vec<MirStmt>)> = Vec::new();
    for (from, to, stmts) in edges {
        match merged.iter_mut().find(|(f, t, _)| *f == from && *t == to) {
            Some((_, _, all)) => all.extend(stmts),
            None => merged.push((from, to, stmts)),
        }
    }
    for (from, to, releases) in merged {
        let only_way_in = preds
            .get(&to)
            .is_some_and(|ps| ps.iter().all(|p| *p == from));
        let span = func
            .blocks
            .iter()
            .find(|b| b.id == from)
            .map(|b| b.terminator.span)
            .unwrap_or(crate::Span::new(0, 0));
        // The only way out of `from`: its end is this edge. A `goto` reads
        // nothing, so the release can sit right before it.
        let only_way_out = func.blocks.iter().find(|b| b.id == from).is_some_and(|b| {
            matches!(b.terminator.kind, MirTerminatorKind::Goto { .. })
        });
        if only_way_out {
            let Some(block) = func.blocks.iter_mut().find(|b| b.id == from) else { continue };
            block.statements.extend(releases);
            continue;
        }
        if only_way_in {
            let Some(block) = func.blocks.iter_mut().find(|b| b.id == to) else { continue };
            let at = block.statements.iter().take_while(|s| matches!(s.kind, MirStmtKind::Phi { .. })).count();
            block.statements.splice(at..at, releases);
            continue;
        }
        let between = BlockId(next_id);
        next_id += 1;
        if let Some(block) = func.blocks.iter_mut().find(|b| b.id == from) {
            retarget(&mut block.terminator, to, between);
        }
        if let Some(block) = func.blocks.iter_mut().find(|b| b.id == to) {
            for stmt in &mut block.statements {
                if let MirStmtKind::Phi { args, .. } = &mut stmt.kind {
                    for (pred, _) in args.iter_mut() {
                        if *pred == from {
                            *pred = between;
                        }
                    }
                }
            }
        }
        func.blocks.push(MirBlock {
            id: between,
            statements: releases,
            terminator: MirTerminator::new(MirTerminatorKind::Goto { target: to }, span),
        });
    }
}

/// Point every arm of `term` that goes to `old` at `new`.
fn retarget(term: &mut MirTerminator, old: BlockId, new: BlockId) {
    let swap = |b: &mut BlockId| {
        if *b == old {
            *b = new;
        }
    };
    match &mut term.kind {
        MirTerminatorKind::Goto { target } => swap(target),
        MirTerminatorKind::Branch { then_block, else_block, .. } => {
            swap(then_block);
            swap(else_block);
        }
        MirTerminatorKind::Switch { cases, default, .. } => {
            cases.iter_mut().for_each(|(_, b)| swap(b));
            swap(default);
        }
        MirTerminatorKind::CleanupReturn { cleanup_chain, .. } => cleanup_chain.iter_mut().for_each(swap),
        MirTerminatorKind::Return { .. } | MirTerminatorKind::Unreachable => {}
    }
}

