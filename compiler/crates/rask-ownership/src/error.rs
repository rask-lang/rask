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

    /// Resource captured by closure/spawn not consumed on all code paths.
    #[error("resource `{name}` captured by {context} is not consumed on all code paths")]
    ResourceNotConsumedInClosure {
        name: String,
        context: String,
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

    /// Mutation in a frozen context (CC3/PF5).
    #[error("cannot mutate in frozen context — `{context_ty}` is frozen")]
    FrozenContextMutation {
        context_ty: String,
        operation: String,
    },

    /// Structural mutation inside `with` block on non-pool collection (W2).
    #[error("cannot {operation} `{collection}` inside `with` block — {collection} can reallocate")]
    WithBlockStructuralMutation {
        collection: String,
        operation: String,
        binding_span: Span,
    },

    /// Removing the bound handle inside `with` block (W2c).
    #[error("cannot remove `{handle}` inside `with` block — it's the bound element")]
    WithBlockBoundHandleRemoved {
        handle: String,
        collection: String,
        binding_span: Span,
    },

    /// Clearing pool inside `with` block (W2d).
    #[error("cannot clear `{collection}` inside `with` block — invalidates all elements")]
    WithBlockClear {
        collection: String,
        binding_span: Span,
    },

    /// LP14: structural mutation during `for mutate`
    #[error("cannot {operation} `{collection}` during `for mutate` — invalidates iteration")]
    ForMutateStructuralMutation {
        collection: String,
        operation: String,
        loop_span: Span,
    },

    /// LP16: passing `for mutate` item to `take` parameter
    #[error("cannot pass `{item}` to `take` parameter — item is borrowed from collection")]
    ForMutateTakeItem {
        item: String,
        collection: String,
        loop_span: Span,
    },

    /// D1: use after discard
    #[error("use of discarded value `{name}`")]
    UseAfterDiscard {
        name: String,
        discarded_at: Span,
    },

    /// D3: discard on @resource type
    #[error("cannot discard resource `{name}` — use its consuming method")]
    DiscardResource {
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
