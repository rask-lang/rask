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

    /// Value was moved on some paths but not all (e.g. one `if` branch),
    /// then used after the paths merged. Maybe-moved is treated as moved (O3).
    #[error("value `{name}` may have been moved")]
    UseAfterMaybeMove {
        name: String,
        /// One branch's move site.
        moved_at: Span,
        reason: MoveReason,
    },

    /// SM2: a `@small` type grew past the 16-byte copy threshold.
    ///
    /// The break belongs at the annotation, not at the call sites: adding a
    /// field that pushes a struct over the threshold flips every assignment
    /// from copy to move, and those errors land wherever the type is *used*,
    /// with only the `MoveReason` note connecting them back.
    #[error("`@small` type `{type_name}` outgrew the copy threshold — {size} bytes, limit is 16")]
    SmallTypeTooBig {
        type_name: String,
        size: usize,
        /// The field that took it over, with its own size, for the message.
        offending_field: Option<(String, usize)>,
    },

    /// SM3: one instantiation of a `@small` generic type doesn't fit.
    ///
    /// The fence is a promise about every instantiation, not just the ones the
    /// author had in mind. `@small struct Pair<T>` is 16 bytes at `Pair<i64>`
    /// and 32 at `Pair<string>` — the second one silently breaks the promise
    /// callers were reading off the annotation.
    #[error("`@small` type `{type_name}` outgrew the copy threshold at this instantiation — {size} bytes, limit is 16")]
    SmallInstantiationTooBig {
        /// The instantiation as written, e.g. `Pair<string>`.
        type_name: String,
        /// The bare generic name, for the fix line.
        base_name: String,
        size: usize,
        /// The field that took it over, with its own size and its type.
        offending_field: Option<(String, usize, String)>,
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

    /// C4: an ensured resource is consumed on some paths but not all, and the
    /// paths merge before scope exit. Which cleanup runs would depend on hidden
    /// runtime state, so it's a compile error (ctrl.ensure/C3–C4).
    #[error("consumption of `{name}` depends on which path ran")]
    EnsureMaybeConsumed {
        name: String,
        /// Where the ensure was scheduled.
        ensure_at: Span,
        /// One branch's consumption site.
        consumed_at: Span,
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

    /// SL2: scope-limited closure escapes its scope
    #[error("closure `{name}` captures scoped borrow and cannot escape")]
    ScopeLimitedClosureEscapes {
        name: String,
    },

    /// ER43: a wildcard pattern would silently drop a transitively-linear value.
    /// Either the whole scrutinee is linear and `_` discards it, or a pattern
    /// position inside a destructure (variant payload, struct field, tuple
    /// element) is linear and the user wrote `_` there.
    #[error("`_` here would silently drop linear value of type `{type_name}`")]
    LinearWildcardDiscard {
        /// Where the discard happens (whole scrutinee vs nested field).
        position: LinearDiscardPosition,
        /// The transitively-linear type that would be dropped.
        type_name: String,
    },
}

/// Where a linear-wildcard discard occurred. Drives the ER43 diagnostic copy.
#[derive(Debug, Clone)]
pub enum LinearDiscardPosition {
    /// Top-level wildcard arm: `match r { _ => ... }` where `r` is linear.
    Scrutinee,
    /// Field inside a destructured variant or struct pattern.
    Field {
        /// Constructor or struct name (e.g. `FileError.ReadFailed`, `Wrapper`).
        constructor: String,
        /// Field name when known (struct patterns, named variant fields).
        /// Constructor patterns use the positional index instead.
        field: Option<String>,
        /// Positional index for tuple-style payloads.
        index: Option<usize>,
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
