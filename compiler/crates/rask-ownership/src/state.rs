// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Ownership and borrowing state tracking.

use rask_ast::Span;

/// The state of a binding during ownership analysis.
#[derive(Debug, Clone)]
pub enum BindingState {
    /// The binding owns its value.
    Owned,
    /// The value has been moved; any use is an error.
    Moved { at: Span },
    /// The value is currently borrowed.
    Borrowed { mode: BorrowMode, scope: BorrowScope },
}

/// Whether a borrow is shared (read-only) or exclusive (mutable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowMode {
    /// Multiple shared borrows allowed (read-only access).
    Shared,
    /// Only one exclusive borrow allowed (mutable access).
    Exclusive,
}

/// How long a borrow lasts.
///
/// Rask has two borrow scopes based on the "Can it grow?" rule:
/// - **Persistent**: String, struct fields, arrays - valid until block end
/// - **Instant**: Vec, Map, Pool - released at semicolon
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BorrowScope {
    /// Borrow is valid until the block ends.
    /// Used for fixed-size sources (String, struct fields, arrays, parameters).
    Persistent { block_id: u32 },
    /// Borrow is valid until the statement ends (semicolon).
    /// Used for growable sources (Vec, Map, Pool).
    Instant { stmt_id: u32 },
}

/// An active borrow during analysis.
#[derive(Debug, Clone)]
pub struct ActiveBorrow {
    /// The binding name being borrowed.
    pub source: String,
    /// Whether this is a shared or exclusive borrow.
    pub mode: BorrowMode,
    /// When this borrow ends.
    pub scope: BorrowScope,
    /// Where the borrow was created.
    pub span: Span,
    /// Field projection — `None` means whole-object borrow,
    /// `Some(fields)` means only those fields are borrowed.
    pub projection: Option<Vec<String>>,
}

impl ActiveBorrow {
    pub fn new(source: String, mode: BorrowMode, scope: BorrowScope, span: Span) -> Self {
        Self { source, mode, scope, span, projection: None }
    }

    pub fn with_projection(mut self, projection: Vec<String>) -> Self {
        self.projection = Some(projection);
        self
    }

    /// Check if this borrow overlaps with a given projection.
    /// Two borrows overlap if either is whole-object or they share fields.
    pub fn overlaps(&self, other_projection: &Option<Vec<String>>) -> bool {
        match (&self.projection, other_projection) {
            (None, _) | (_, None) => true, // whole-object overlaps everything
            (Some(a), Some(b)) => a.iter().any(|f| b.contains(f)),
        }
    }
}
