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

/// Why a value was moved instead of copied.
#[derive(Debug, Clone)]
pub enum MoveReason {
    /// Type exceeds the 16-byte copy threshold.
    SizeExceedsThreshold { type_name: String, size: usize },
    /// Type owns heap memory (String, Vec, Map, Pool).
    OwnsHeapMemory { type_name: String },
    /// Type is marked @unique.
    Unique { type_name: String },
    /// Type is marked @resource.
    Resource { type_name: String },
    /// Unknown or generic type.
    Unknown,
}

/// The kind of ownership error.
#[derive(Debug, Clone, Error)]
pub enum OwnershipErrorKind {
    /// Value was moved and can no longer be used.
    #[error("value `{name}` was already moved")]
    UseAfterMove {
        name: String,
        moved_at: Span,
        reason: MoveReason,
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

    /// Trying to move a value out of a borrowed parameter.
    #[error("cannot move `{name}` — parameter is borrowed, not owned")]
    MoveFromBorrowedParam {
        name: String,
    },

    /// Resource consumed more than once.
    #[error("resource `{name}` already consumed")]
    ResourceAlreadyConsumed {
        name: String,
        consumed_at: Span,
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
