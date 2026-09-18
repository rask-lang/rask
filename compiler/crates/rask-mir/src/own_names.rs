// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What the program declares for itself.
//!
//! MIR mints names for operations the stdlib spells differently, and
//! `mir_metadata` answers ownership questions about them. For a name it has
//! never heard of it has to guess, and the guess is made from the name: split
//! at the first `_`, and if the head is a stdlib type, treat it as an
//! unaccounted-for spelling of that family — warn, and answer every question
//! the way that leaks rather than the way that double-frees.
//!
//! `string`, `char` and `cstring` are ordinary type names, so
//!
//! ```text
//! func string_shoutify(s: string) -> string { return "{s}!!!" }
//! ```
//!
//! read as one of those. Every compile printed a warning telling the author to
//! edit a table inside the compiler, and the function was treated as owning
//! everything it touched — so what it handed back was released by nobody. A
//! leak in a user's program caused by what they named their function (#1217).
//!
//! The guess is right for what it was built for and there is no way to make it
//! right by looking harder at the name: `string_shoutify` and a spelling MIR
//! might mint tomorrow are the same string. What tells them apart is that one
//! of them is in the program. So these two questions — the only two whose
//! unmapped answer leaks — are asked here, where the program's own function
//! names are in hand, and passed through to `mir_metadata` only for a name the
//! program does not declare.

use std::collections::HashSet;

/// Does this call hand back a view into storage its receiver keeps owning?
pub fn returns_a_view(name: &str, own: &HashSet<String>) -> bool {
    if own.contains(name) {
        return false;
    }
    report_if_unmapped(name);
    rask_stdlib::mir_metadata::returns_a_view(name)
}

/// Does this call keep the argument at `index`?
pub fn keeps_argument(name: &str, index: usize, own: &HashSet<String>) -> bool {
    if own.contains(name) {
        return false;
    }
    report_if_unmapped(name);
    rask_stdlib::mir_metadata::keeps_argument(name, index)
}

/// A spelling nothing accounts for, once per name per process.
///
/// `tests/spellings_gate.sh` sweeps the corpus for these, so a new mint that
/// nobody wrote a line for is a red gate rather than a silent leak. Reported
/// from here rather than from `mir_metadata` because this is the first place
/// that knows the name isn't the program's own — which is the difference
/// between a real gap and a warning about somebody's `func string_shoutify`.
fn report_if_unmapped(name: &str) {
    if let Some((base, family)) = rask_stdlib::mir_metadata::unmapped_spelling(name) {
        rask_stdlib::mir_metadata::report_unmapped(base, family);
    }
}
