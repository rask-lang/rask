// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The method `x[a..b]` desugars to.
//!
//! Desugaring chooses the names operators become before the stdlib is parsed,
//! so the names live with the AST: `binary_op_method` and `unary_op_method` in
//! `expr`, and this one. Which interface each belongs to is `stdlib/ops.rk`'s
//! to say, and `rask_stdlib::operators` reads it from there.

/// The method `x[a..b]` desugars to (`Slice`'s).
pub const SLICE_METHOD: &str = "slice";
