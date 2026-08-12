// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Which step resolved each method call's receiver type.
//!
//! MIR mangles a method call to `{Type}_{method}`, and the type comes from an
//! ordered chain: the checker's recorded dispatch target first, then a series of
//! narrower guesses. The guesses are what #425 set out to delete, and the last
//! sweep left nine calls still using them — "a fallback that fires nine times is
//! still load bearing nine times."
//!
//! This records which step answered, so a step can be removed when the tally
//! says nothing reaches it. `RASK_TRACE_DISPATCH=1` prints the tally at the end
//! of a compile, with the method names for anything that didn't come from the
//! checker.

use std::cell::RefCell;
use std::collections::BTreeMap;

thread_local! {
    static TALLY: RefCell<BTreeMap<&'static str, Vec<String>>> = RefCell::new(BTreeMap::new());
}

/// True when tracing is on. Checked before building the method-name string.
pub fn enabled() -> bool {
    std::env::var_os("RASK_TRACE_DISPATCH").is_some()
}

/// Record that `step` resolved the receiver for a call to `method`.
pub fn record(step: &'static str, method: &str) {
    if !enabled() {
        return;
    }
    TALLY.with(|t| {
        t.borrow_mut()
            .entry(step)
            .or_default()
            .push(method.to_string())
    });
}

/// Print the tally: one line per step, with method names for the guessing steps.
pub fn report() {
    if !enabled() {
        return;
    }
    TALLY.with(|t| {
        let t = t.borrow();
        let total: usize = t.values().map(|v| v.len()).sum();
        eprintln!("[dispatch] {total} method calls reached the chain");
        for (step, methods) in t.iter() {
            if step.starts_with('0') {
                eprintln!("[dispatch]   {step}: {}", methods.len());
                continue;
            }
            let mut names: Vec<&str> = methods.iter().map(String::as_str).collect();
            names.sort_unstable();
            names.dedup();
            eprintln!(
                "[dispatch]   {step}: {} ({})",
                methods.len(),
                names.join(", ")
            );
        }
    });
}
