// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Where lowering couldn't work out a type and guessed i64.
//!
//! A guess is only ever right for a payload that already fits a machine word.
//! Every other shape — an f64 needing a float register, a 16-byte string, a
//! struct reached by address — comes out silently wrong, which reads as a
//! miscompile rather than a missing feature. Routing every guess through here
//! makes them countable, and `RASK_STRICT_TYPES=1` makes them fatal so a sweep
//! can tell which ones a real program actually reaches.
//!
//! Sites that no program reaches don't need a fallback at all; they should say
//! plainly that the type is unknown. This module is how you find out which
//! those are.

use crate::MirType;
use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    static HITS: RefCell<BTreeMap<&'static str, u32>> = RefCell::new(BTreeMap::new());
    static LOOKUPS: RefCell<LookupStats> = RefCell::new(LookupStats::default());
    static OPEN_SHAPES: RefCell<BTreeMap<String, u32>> = RefCell::new(BTreeMap::new());
}

fn trace() -> bool {
    std::env::var_os("RASK_TRACE_TYPE_FALLBACK").is_some()
}

/// How a MIR question to the checker's `node_types` turned out.
///
/// The fallback sites above are the *symptom*; this is the cause. A site only
/// guesses because the lookup that fed it came back empty or came back holding
/// a type variable — so counting the three outcomes says which of the two the
/// fix has to address, per program, instead of assuming.
#[derive(Default, Clone, Copy, Debug)]
pub struct LookupStats {
    /// The checker had a concrete type for the node.
    pub resolved: u64,
    /// The checker had an entry, but it still contains an inference variable —
    /// present and useless, which reads the same as absent to every consumer.
    pub open: u64,
    /// No entry at all: a node the checker never visited, or one created after
    /// checking finished.
    pub missing: u64,
}

impl LookupStats {
    pub fn total(&self) -> u64 {
        self.resolved + self.open + self.missing
    }
}

fn type_is_open(ty: &rask_types::Type) -> bool {
    use rask_types::{GenericArg, Type};
    match ty {
        Type::Var(_) => true,
        Type::Result { ok, err } => type_is_open(ok) || type_is_open(err),
        Type::RawPtr(inner) | Type::Slice(inner) => type_is_open(inner),
        Type::Array { elem, .. } => type_is_open(elem),
        Type::Tuple(elems) | Type::Union(elems) => elems.iter().any(type_is_open),
        Type::Fn { params, ret } => params.iter().any(type_is_open) || type_is_open(ret),
        Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args
            .iter()
            .any(|a| matches!(a, GenericArg::Type(t) if type_is_open(t))),
        _ => false,
    }
}

/// Record the outcome of one `node_types` lookup.
pub fn record_lookup(found: Option<&rask_types::Type>) {
    LOOKUPS.with(|l| {
        let mut l = l.borrow_mut();
        match found {
            None => l.missing += 1,
            Some(ty) => {
                if type_is_open(ty) {
                    l.open += 1;
                    // What the unresolved type looked like is the whole lead —
                    // `Result { ok: Var(_), err: … }` on a `receive()` is what
                    // pointed at the channel-element bug (#717). Kept behind the
                    // same flag, aggregated so a big program stays readable.
                    if trace_coverage() {
                        OPEN_SHAPES.with(|s| {
                            *s.borrow_mut().entry(format!("{ty:?}")).or_insert(0) += 1
                        });
                    }
                } else {
                    l.resolved += 1;
                }
            }
        }
    });
}

/// Lookup outcomes so far.
pub fn lookup_stats() -> LookupStats {
    LOOKUPS.with(|l| *l.borrow())
}

/// True when a coverage summary was asked for.
pub fn trace_coverage() -> bool {
    std::env::var_os("RASK_TRACE_TYPE_COVERAGE").is_some()
}

/// Record that a site could not resolve a type.
///
/// This is fatal: the enclosing function's lowering fails and the compiler
/// reports which site gave up. The i64 it still returns is only so the caller
/// can finish walking the expression — nothing is emitted from it, because
/// lowering is about to be thrown away.
///
/// `RASK_ALLOW_TYPE_FALLBACK=1` restores the old guess-and-continue behaviour.
/// It exists for bisecting whether a given failure is *this* or something else,
/// not as a way to ship a build.
pub fn i64_fallback(site: &'static str) -> MirType {
    HITS.with(|h| *h.borrow_mut().entry(site).or_insert(0) += 1);
    if trace() {
        eprintln!("[type-fallback] {site} could not resolve a type");
    }
    MirType::I64
}

/// True when the guess-and-continue escape hatch is set.
pub fn fallback_allowed() -> bool {
    std::env::var_os("RASK_ALLOW_TYPE_FALLBACK").is_some()
}

/// Every site that gave up, with a count, most frequent first.
pub fn hits() -> Vec<(&'static str, u32)> {
    let mut v: Vec<_> = HITS.with(|h| h.borrow().iter().map(|(k, v)| (*k, *v)).collect());
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v
}

/// Drain the record and return it. Called around each function's lowering so a
/// failure is attributed to the function that caused it.
pub fn take_hits() -> Vec<(&'static str, u32)> {
    let v = hits();
    reset();
    v
}

/// Forget everything recorded.
pub fn reset() {
    HITS.with(|h| h.borrow_mut().clear());
}

/// Print how well the checker's types covered what lowering asked for.
///
/// Enabled by `RASK_TRACE_TYPE_COVERAGE=1`. `missing` and `open` are the two
/// distinct failures behind every guess: a node the checker never recorded, and
/// a node it recorded while the type was still a variable. They need different
/// fixes, and before this there was no way to tell which one a given program
/// was actually hitting.
pub fn report_coverage() {
    if !trace_coverage() {
        return;
    }
    let s = lookup_stats();
    let total = s.total();
    if total == 0 {
        eprintln!("[type-coverage] lowering asked for no node types");
        return;
    }
    let pct = |n: u64| (n as f64) * 100.0 / (total as f64);
    eprintln!(
        "[type-coverage] {total} lookups: {} resolved ({:.1}%), {} open ({:.1}%), {} missing ({:.1}%)",
        s.resolved, pct(s.resolved),
        s.open, pct(s.open),
        s.missing, pct(s.missing),
    );
    let mut shapes: Vec<(String, u32)> =
        OPEN_SHAPES.with(|m| m.borrow().iter().map(|(k, v)| (k.clone(), *v)).collect());
    shapes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    for (shape, n) in shapes.iter().take(10) {
        eprintln!("[type-coverage]   open ×{n}: {shape}");
    }
}
