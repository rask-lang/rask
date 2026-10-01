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
use crate::{BlockId, LocalId, MirFunction, MirOperand, MirStmtKind};

/// A value: where it was made, and the name it was made under. A join's value
/// has `at == u32::MAX`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Value {
    block: u32,
    at: u32,
    name: LocalId,
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
    /// The name now holds something untracked.
    Other(LocalId),
    /// Whatever the name holds leaves the frame here, on this path.
    HandOver(LocalId),
    /// A whole-width store into the slot. The next field of the value already
    /// there, or a new value over one that lives on under another name; this
    /// analysis decides which.
    Fill(LocalId),
    /// Something may write into the slot in place: a call given its address.
    /// The slot is where the value is from here on; other names that copied it
    /// hold stale bytes.
    WriteThrough(LocalId),
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
}

/// A release to insert, under `name`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Release {
    /// Before statement `at` of block `block` (an index into the original
    /// statements and blocks).
    At { block: usize, at: usize, name: LocalId },
    /// On the edge between two blocks: the value is still needed on the way
    /// out of `from`, because another successor reads it, and not by `to`.
    OnEdge { from: BlockId, to: BlockId, name: LocalId },
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
            Event::Other(n) => {
                st.bind.insert(*n, BTreeSet::from([Bind::Other]));
            }
            Event::HandOver(n) => {
                for b in st.binds(*n) {
                    if let Bind::Own(v) | Bind::Part(v) = b {
                        if st.own.contains_key(&v) {
                            st.own.insert(v, false);
                        }
                    }
                }
            }
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

fn make(st: &mut State, n: LocalId, bi: usize, si: usize) {
    let v = Value { block: bi as u32, at: si as u32, name: n };
    st.bind.insert(n, BTreeSet::from([Bind::Own(v)]));
    st.own.insert(v, true);
}

/// The state on entry to `bi`, from its predecessors' exits.
fn join(
    bi: usize,
    sh: &Shape,
    exits: &[Option<State>],
    entry_live: &BTreeSet<LocalId>,
) -> Option<State> {
    let preds: Vec<usize> = sh.preds[bi].iter().copied().filter(|p| exits[*p].is_some()).collect();
    if preds.is_empty() {
        return None;
    }
    // Each predecessor's bindings as this block sees them, phi destinations
    // included: on the edge from `p`, a phi's destination holds what its
    // operand from `p` holds.
    let seen: Vec<State> = preds
        .iter()
        .map(|&p| {
            let mut st = exits[p].clone().unwrap();
            for (dst, ops) in &sh.phis[bi] {
                let b = ops
                    .iter()
                    .find(|(from, _)| *from == p)
                    .and_then(|(_, l)| *l)
                    .map(|l| st.binds(l))
                    .unwrap_or_else(|| BTreeSet::from([Bind::Other]));
                st.bind.insert(*dst, b);
            }
            st
        })
        .collect();

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
                let Some(v) = one.and_then(Bind::value) else { break };
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

fn transfer(
    func: &MirFunction,
    facts: &Facts,
    live: &Live,
    bi: usize,
    entry: &State,
    updates: &mut HashSet<(usize, usize)>,
) -> State {
    let mut st = entry.clone();
    for si in 0..func.blocks[bi].statements.len() {
        if is_phi(func, bi, si) {
            continue;
        }
        apply(&mut st, &facts.events[bi][si], bi, si, &live.at[bi][si + 1], updates);
    }
    let len = func.blocks[bi].statements.len();
    apply(&mut st, &facts.terminator_events[bi], bi, len, &live.out[bi], updates);
    st
}

fn entry_state(func: &MirFunction, facts: &Facts) -> State {
    let _ = func;
    let mut st = State::default();
    for n in &facts.foreign {
        st.bind.insert(*n, BTreeSet::from([Bind::Other]));
    }
    st
}

fn solve(
    func: &MirFunction,
    facts: &Facts,
    sh: &Shape,
    live: &Live,
    updates: &mut HashSet<(usize, usize)>,
) -> Vec<Option<State>> {
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
                Some(entry_state(func, facts))
            } else {
                join(bi, sh, &exits, &live.at[bi][0])
            };
            let Some(entry) = entry else { continue };
            let exit = transfer(func, facts, live, bi, &entry, updates);
            if exits[bi].as_ref() != Some(&exit) {
                exits[bi] = Some(exit);
                changed = true;
            }
            entries[bi] = Some(entry);
        }
    }
    entries
}

/// Where each value this frame owns is released, and under which name.
pub fn plan(func: &MirFunction, facts: &Facts) -> Vec<Release> {
    if func.blocks.is_empty() {
        return Vec::new();
    }
    let sh = shape(func, &facts.names);

    // Liveness and the `Fill` verdicts feed each other: a store that is the
    // next field of a value is a use of its slot, and one that writes a new
    // value over it ends the old one there. Settle them together.
    let mut updates: HashSet<(usize, usize)> = HashSet::new();
    let mut live = liveness(func, facts, &sh, &updates);
    let mut entries = Vec::new();
    for _ in 0..6 {
        let mut found = HashSet::new();
        entries = solve(func, facts, &sh, &live, &mut found);
        if found == updates {
            break;
        }
        updates = found;
        live = liveness(func, facts, &sh, &updates);
    }

    let exits: Vec<Option<State>> = (0..func.blocks.len())
        .map(|bi| {
            entries[bi].as_ref().map(|e| {
                let mut scratch = HashSet::new();
                transfer(func, facts, &live, bi, e, &mut scratch)
            })
        })
        .collect();

    // `RASK_OWNERSHIP_TRACE=<function>` prints what this decided for it.
    let trace = std::env::var("RASK_OWNERSHIP_TRACE").is_ok_and(|v| v == func.name);
    if trace {
        for bi in 0..func.blocks.len() {
            eprintln!("bb{} entry {:?}", func.blocks[bi].id.0, entries[bi]);
            eprintln!("    live in {:?} out {:?}", live.at[bi][0], live.out[bi]);
            eprintln!("    exit {:?}", exits[bi]);
        }
        eprintln!("fill updates {:?}", updates);
    }
    let mut out = Vec::new();
    // Each block's state on the way out, after the releases placed inside it.
    let mut finals: Vec<Option<State>> = vec![None; func.blocks.len()];
    for bi in 0..func.blocks.len() {
        let Some(entry) = &entries[bi] else { continue };
        let stmts = &func.blocks[bi].statements;
        let mut st = entry.clone();
        for si in 0..stmts.len() {
            if is_phi(func, bi, si) {
                continue;
            }
            let before = st.clone();
            let mut scratch = HashSet::new();
            apply(&mut st, &facts.events[bi][si], bi, si, &live.at[bi][si + 1], &mut scratch);
            let touched: Vec<LocalId> = facts.reads[bi][si]
                .iter()
                .chain(facts.kills[bi][si].iter())
                .copied()
                .chain(facts.events[bi][si].iter().map(|e| match e {
                    Event::Make(n)
                    | Event::Other(n)
                    | Event::HandOver(n)
                    | Event::Fill(n)
                    | Event::WriteThrough(n) => *n,
                    Event::Alias { dst, .. }
                    | Event::Part { dst, .. }
                    | Event::View { dst, .. }
                    | Event::ViewAlso { dst, .. } => *dst,
                }))
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
                if !alive_before {
                    continue;
                }
                if let Some(name) = pick(&st, v, &touched) {
                    out.push(Release::At { block: bi, at: si + 1, name });
                    st.own.insert(v, false);
                }
            }
        }
        let len = stmts.len();
        let mut scratch = HashSet::new();
        apply(&mut st, &facts.terminator_events[bi], bi, len, &live.out[bi], &mut scratch);
        finals[bi] = Some(st);
    }

    // Still ours on the way out of a block, needed there because one
    // successor reads it, and not needed by another: it dies on that edge.
    for (pi, p) in func.blocks.iter().enumerate() {
        let Some(px) = &finals[pi] else { continue };
        let mut p_out = live.out[pi].clone();
        p_out.extend(facts.terminator_reads[pi].iter().copied());
        let values: Vec<Value> = px.own.iter().filter(|(_, o)| **o).map(|(v, _)| *v).collect();
        for &bi in &sh.succs[pi] {
            // The successor's names as seen from this edge: its phis hold
            // what they take from here.
            let mut seen = px.clone();
            for (dst, ops) in &sh.phis[bi] {
                let b = ops
                    .iter()
                    .find(|(from, _)| *from == pi)
                    .and_then(|(_, l)| *l)
                    .map(|l| px.binds(l))
                    .unwrap_or_else(|| BTreeSet::from([Bind::Other]));
                seen.bind.insert(*dst, b);
            }
            let first = func.blocks[bi]
                .statements
                .iter()
                .take_while(|s| matches!(s.kind, MirStmtKind::Phi { .. }))
                .count();
            for &v in &values {
                if !px.needed(v, &p_out) || seen.needed(v, &live.at[bi][first]) {
                    continue;
                }
                if let Some(name) = pick(px, v, &[]) {
                    out.push(Release::OnEdge { from: p.id, to: func.blocks[bi].id, name });
                }
            }
        }
    }
    out.sort();
    out.dedup();
    if trace {
        eprintln!("releases {:?}", out);
    }
    out
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
