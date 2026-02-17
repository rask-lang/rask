// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Abstract Syntax Tree types for the Rask language.
//!
//! This crate defines the AST nodes shared between the lexer, parser,
//! type checker, and interpreter.

pub mod span;
pub mod token;
pub mod expr;
pub mod stmt;
pub mod decl;

pub use span::{Span, LineMap};

/// Unique identifier for AST nodes.
///
/// Used by semantic analysis passes to track resolution results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct NodeId(pub u32);

impl NodeId {
    pub const DUMMY: NodeId = NodeId(u32::MAX);
}
