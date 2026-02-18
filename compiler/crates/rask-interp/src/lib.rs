// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Tree-walk interpreter for the Rask language.
//!
//! Executes the AST directly without compilation.

mod value;
mod env;
mod resource;
mod interp;
mod builtins;
mod stdlib;
pub mod build_context;

pub use build_context::BuildState;
pub use interp::{BenchmarkResult, Interpreter, RuntimeDiagnostic, RuntimeError, TestResult};

#[cfg(test)]
mod drift;
