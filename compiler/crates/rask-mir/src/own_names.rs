// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! What the program declares for itself.
//!
//! A call's shape says what it does with its arguments and result. A function
//! the program defines answers for itself, from its body: its name may sit in
//! a stdlib family (`string_shoutify`, #1217) and that proves nothing. So the
//! two questions whose answer leaks when guessed are asked here, where the
//! program's own function names are in hand, and the call's shape is read only
//! for a name the program does not define.

use std::collections::HashSet;

use crate::FunctionRef;

/// Does this call hand back a view into storage its receiver keeps owning?
pub fn returns_a_view(call: &FunctionRef, own: &HashSet<String>) -> bool {
    !own.contains(&call.name) && call.returns_a_view()
}

/// Does this call keep the argument at `index`?
pub fn keeps_argument(call: &FunctionRef, index: usize, own: &HashSet<String>) -> bool {
    !own.contains(&call.name) && call.gives(index)
}

/// Report every call lowering made up without saying what it does: no shape,
/// not a function in this program, not a runtime, extern or macro call. Once
/// per name per process.
///
/// `tests/spellings_gate.sh` sweeps the corpus for these, so a new spelling
/// nobody stated is a red gate rather than a silent leak or double free. It is
/// a sweep over the calls rather than a side effect of asking a question, so
/// it can't quietly narrow when somebody rewrites a pass.
pub fn report_unshaped_calls(fns: &[crate::MirFunction], own: &HashSet<String>) {
    use std::sync::Mutex;
    static SEEN: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    for stmt in fns.iter().flat_map(|f| f.blocks.iter()).flat_map(|b| b.statements.iter()) {
        let crate::MirStmtKind::Call { func: fref, .. } = &stmt.kind else { continue };
        if fref.shape.is_some() || fref.runtime || fref.is_extern || fref.c_macro.is_some() || own.contains(&fref.name) {
            continue;
        }
        let head = fref.name.rsplit("::").next().unwrap_or(&fref.name);
        let base = head.split('$').next().unwrap_or(head);
        let mut seen = SEEN.lock().unwrap();
        if seen.get_or_insert_with(HashSet::new).insert(base.to_string()) {
            eprintln!(
                "[unshaped-call] {base}: lowering emitted this call without saying what it does \
                 with its arguments and result, so the ownership passes have to guess. State its shape where it is emitted \
                 (`FunctionRef::borrowing`, `releasing`, `reading`, `like`)."
            );
        }
    }
}
