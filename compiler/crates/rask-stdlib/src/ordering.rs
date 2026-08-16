// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! `Ordering` — the one definition.
//!
//! `Ordering` is a compiler-registered enum, not a declared one, so no decl
//! reaches the layout pass for it. Each stage used to carry its own copy of the
//! variant list, and MIR carried none at all: every `Ordering.X` lowered to tag
//! 0, so `a.compare(b) == Ordering.Greater` was true whenever the values were
//! equal (#496). The list below is now the only copy.
//!
//! ## It still gets a layout
//!
//! `rask_mono::ordering_layout` synthesizes one from that list, so `Ordering`
//! lays out like any other fieldless enum: a `u8` tag at offset 0.
//! `compare` stores its result into a real slot rather than handing back the
//! bare tag, which is what makes `extend Ordering with Displayable` work on
//! native — `{a.compare(b)}` used to print `0` for Less while the interpreter
//! printed `less` (#729).
//!
//! Two boundaries still want the tag as a number, and both convert explicitly:
//!
//! - Comparator closures. `rask_vec_sort_by`'s C adapter reads the return as
//!   an integer and tests it against zero, so returning the aggregate handed it
//!   a stack address and `sort_by` sorted by address. `terminate_return`
//!   converts when the function's declared return is `i64` — that one funnel
//!   catches both a tail expression and an explicit `return` in a block body.
//! - The assert-failure helpers, which take `i64`.
//!
//! One trap worth knowing: store the tag at the value's natural width, not one
//! byte. Structural `==` compares the whole slot, so a narrow store leaves the
//! rest undefined and equality turns on whatever the stack held — three
//! identical asserts passed in `main` and the third failed inside a `test`
//! block. `t55_ordering`, `t61_nominal_traits` and `t_sort_by_closure` are the
//! suites that catch all of this; run `tests/differential.sh`.

/// Comparison results (ORD1) and atomic memory orderings, in tag order.
/// The two share one enum, matching the resolver's and interpreter's
/// registrations.
pub const ORDERING_VARIANTS: &[&str] = &[
    // Comparison (ORD1)
    "Less", "Equal", "Greater",
    // Atomic memory ordering
    "Relaxed", "Acquire", "Release", "AcqRel", "SeqCst",
];

/// The tag a variant name carries, or `None` if `Ordering` has no such variant.
pub fn ordering_tag(variant: &str) -> Option<i64> {
    ORDERING_VARIANTS.iter().position(|v| *v == variant).map(|i| i as i64)
}

/// The three comparison results, by tag. `compare` produces one of these.
pub const ORDERING_LESS: i64 = 0;
pub const ORDERING_EQUAL: i64 = 1;
pub const ORDERING_GREATER: i64 = 2;
