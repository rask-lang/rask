// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Panic text both backends have to print identically.
//!
//! `differential.sh` compares the two backends' output byte for byte, so a
//! message either backend raises on its own has to be one string, not two that
//! happen to match today. MIR lowering emits these into the compiled program;
//! the interpreter raises them directly.

/// std.testing/T20: a `try` whose error reached the end of a `test` block.
/// The block *is* the error branch, so the error ends that test — and the
/// error's own `message()` is appended after `": "` when the type has one.
pub const TRY_PROPAGATED_NOWHERE: &str = "try propagated an error out of a test block";
