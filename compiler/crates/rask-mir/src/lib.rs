// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR (Mid-level Intermediate Representation) - non-SSA control-flow graph.
//!
//! MIR is the bridge between high-level AST and backend code generation.
//! It uses basic blocks with statements and terminators.

mod builder;
mod display;
mod function;
mod operand;
mod stmt;
mod types;

pub mod lower;

pub use builder::BlockBuilder;
pub use function::{BlockId, MirBlock, MirFunction, MirLocal};
pub use operand::{FunctionRef, LocalId, MirOperand, MirRValue};
pub use stmt::{MirStmt, MirTerminator};
pub use types::MirType;
