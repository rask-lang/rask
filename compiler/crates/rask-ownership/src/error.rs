// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Ownership and borrowing errors.

use rask_ast::Span;
use thiserror::Error;

/// An ownership or borrowing error.
#[derive(Debug, Clone)]
pub struct OwnershipError {
    pub kind: OwnershipErrorKind,
    pub span: Span,
}

/// The kind of ownership error.
#[derive(Debug, Clone, Error)]
pub enum OwnershipErrorKind {
    /// Value was moved and can no longer be used.
    #[error("value `{name}` was already moved")]
    UseAfterMove {
        name: String,
        moved_at: Span,
    },

    /// Conflicting access to a value (e.g., trying to write while someone is reading).
    #[error("cannot {requested} `{name}` - it's already being {existing}")]
    BorrowConflict {
        name: String,
        requested: AccessKind,
        existing: AccessKind,
        existing_span: Span,
    },

    /// Trying to change a value while it's being read elsewhere.
    #[error("`{name}` cannot be changed while it's being read")]
    MutateWhileBorrowed {
        name: String,
        borrow_span: Span,
    },

    /// Trying to store a reference from a collection (Vec, Map, Pool).
    #[error("cannot store reference from {source_type} - use inline or copy out the value")]
    InstantBorrowEscapes {
        source_type: String,
    },

    /// Trying to return or store a reference that would become invalid.
    #[error("`{name}` would become invalid after this point")]
    BorrowEscapes {
        name: String,
    },

    /// Resource type not consumed before scope exit.
    #[error("`{name}` must be used before the end of this block")]
    ResourceNotConsumed {
        name: String,
    },
}

/// User-friendly access kind for error messages.
#[derive(Debug, Clone, Copy)]
pub enum AccessKind {
    Read,
    Write,
}

impl std::fmt::Display for AccessKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccessKind::Read => write!(f, "read"),
            AccessKind::Write => write!(f, "written to"),
        }
    }
}

impl std::fmt::Display for OwnershipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.kind)
    }
}

impl std::error::Error for OwnershipError {}
