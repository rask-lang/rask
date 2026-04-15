// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR (Mid-level Intermediate Representation) — hybrid SSA control-flow graph.
//!
//! MIR is the bridge between high-level AST and backend code generation.
//! It uses basic blocks with statements and terminators. Lowering produces
//! non-SSA form; `transform::ssa::construct` converts to pruned SSA before
//! optimization; `transform::ssa::destruct` lowers back before codegen.

pub mod analysis;
mod builder;
mod closures;
mod display;
mod function;
mod operand;
mod program;
mod stmt;
pub mod transform;
mod types;

pub mod hidden_params;
pub mod lower;

pub use builder::BlockBuilder;
pub use closures::optimize_all_closures;
pub use transform::clone_elision::elide_clones;
pub use transform::gen_coalesce::coalesce_generation_checks;
pub use transform::string_append::optimize_string_concat;
pub use transform::pass::{MirPass, PassManager, PipelineResult};
pub use function::{BlockId, MirBlock, MirFunction, MirLocal};
pub use transform::inline::InlineRegion;
pub use operand::{BinOp, FunctionRef, LocalId, MirConst, MirOperand, MirRValue, UnaryOp};
pub use stmt::{ClosureCapture, MirStmt, MirStmtKind, MirTerminator, MirTerminatorKind, Span};
pub use lower::ComptimeGlobalMeta;
pub use program::MirProgram;
pub use types::{MirType, StructLayoutId, EnumLayoutId};
