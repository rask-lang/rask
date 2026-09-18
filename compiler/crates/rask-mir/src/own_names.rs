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
    !own.contains(name) && rask_stdlib::mir_metadata::returns_a_view(name)
}

/// Does this call keep the argument at `index`?
pub fn keeps_argument(name: &str, index: usize, own: &HashSet<String>) -> bool {
    !own.contains(name) && rask_stdlib::mir_metadata::keeps_argument(name, index)
}

/// Report every call in the program whose name reads as a spelling MIR minted
/// and that nothing accounts for. Once per name per process.
///
/// `tests/spellings_gate.sh` sweeps the corpus for these, so a new mint nobody
/// wrote a line for is a red gate rather than a silent leak. It used to come
/// out of `mir_metadata::declared()`, which every query goes through — but that
/// function is handed a bare name and cannot tell a mint from the program's own
/// `func string_shoutify`, which is the bug this module exists for.
///
/// So the sweep is its own pass over the calls rather than a side effect of
/// asking a question. The first cut hung it off the two queries this module
/// wraps, which does cover every call today — `rc_elide` asks `returns_a_view`
/// of every one — but only by accident of how that pass is written. A sweep
/// says what it means and can't quietly narrow when somebody rewrites a pass.
pub fn report_unmapped_calls(fns: &[crate::MirFunction], own: &HashSet<String>) {
    for stmt in fns.iter().flat_map(|f| f.blocks.iter()).flat_map(|b| b.statements.iter()) {
        let crate::MirStmtKind::Call { func: fref, .. } = &stmt.kind else { continue };
        if own.contains(&fref.name) {
            continue;
        }
        if let Some((base, family)) = rask_stdlib::mir_metadata::unmapped_spelling(&fref.name) {
            rask_stdlib::mir_metadata::report_unmapped(base, family);
        }
    }
}
