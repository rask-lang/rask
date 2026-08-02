// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! `Ordering` — the one definition.
//!
//! `Ordering` is a compiler-registered enum, not a declared one, so it has no
//! layout for the backends to read variant tags out of. Each stage used to
//! carry its own copy of the variant list, and MIR carried none at all: every
//! `Ordering.X` lowered to tag 0, so `a.compare(b) == Ordering.Greater` was
//! true whenever the values were equal (#496).

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
