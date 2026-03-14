// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR analysis utilities — shared graph algorithms and data queries for
//! optimization passes and codegen.

pub mod call_graph;
pub mod cfg;
pub mod dataflow;
pub mod dominators;
pub mod escape;
pub mod liveness;
pub mod loops;
pub mod uses;
pub mod local_index;
pub mod pool_ops;
pub mod typestate;
