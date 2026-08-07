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
}

fn trace() -> bool {
    std::env::var_os("RASK_TRACE_TYPE_FALLBACK").is_some()
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
