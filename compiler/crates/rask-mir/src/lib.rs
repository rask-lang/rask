// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR (Mid-level Intermediate Representation) - non-SSA control-flow graph.
//!
//! MIR is the bridge between high-level AST and backend code generation.
//! It uses basic blocks with statements and terminators.

mod builder;
mod closures;
mod display;
mod function;
mod operand;
mod stmt;
pub mod transform;
mod types;

pub mod lower;

pub use builder::BlockBuilder;
pub use closures::optimize_all_closures;
pub use transform::gen_coalesce::coalesce_generation_checks;
pub use transform::string_append::optimize_string_concat;
pub use function::{BlockId, MirBlock, MirFunction, MirLocal};
pub use operand::{BinOp, FunctionRef, LocalId, MirConst, MirOperand, MirRValue, UnaryOp};
pub use stmt::{ClosureCapture, MirStmt, MirTerminator};
pub use lower::ComptimeGlobalMeta;
pub use types::{MirType, StructLayoutId, EnumLayoutId};
