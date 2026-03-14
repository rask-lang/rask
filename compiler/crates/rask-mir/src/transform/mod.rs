// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR transform passes.

pub mod clone_elision;
pub mod dce;
pub mod gen_coalesce;
pub mod inline;
pub mod pass;
pub mod rc_elide;
pub mod rc_insert;
pub mod ssa;
pub mod state_machine;
pub mod string_append;
