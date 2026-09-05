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
    /// A `Link<T>` whose node was deleted. Not a move at all: the pointer stayed
    /// put and the thing it pointed at was freed, so every name for it is dead.
    /// The move checker is only the mechanism that proves it (analysis.fourth-option).
    LinkDeleted,
    /// A `Link<T>` moved into another name the ordinary way. Nothing was deleted —
    /// links are affine among locals so that two names for one node can't drift
    /// apart, and this is that rule firing, not a use after free.
    LinkMoved,
    /// The binding holds an `Owned` box — `own` allocated it and something has
    /// already consumed it, so a second use is a second free (mem.linear/L3).
    Owned,
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

    /// mem.linear/L1–L6 with mem.parameters/PM1: a parameter the caller only
    /// lent out can't be given away.
    ///
    /// A parameter without `take` is a borrow — the caller keeps the value and
    /// goes on using it. The body used to treat it as owned, so it could be fed
    /// straight to a `take` parameter or a `take self` method with nothing said.
    /// For a `@resource` that's a double-close the caller can't see; for a plain
    /// value it's a use of something already given away.
    #[error("cannot give away `{name}` — it's borrowed, not owned")]
    ConsumeBorrowedParam {
        name: String,
        /// The parameter's declaration, to point at and to suggest `take` on.
        declared_at: Span,
        /// `mutate` reads differently from a plain borrow: it's exclusive access,
        /// which is still not ownership.
        is_mutate: bool,
        /// What the value was being handed to, when it has a name.
        sink: Option<String>,
    },

    /// mem.parameters/PM2 with PM6: a `mutate` parameter consumed and not
    /// replaced.
    ///
    /// `mutate` is exclusive access, so taking the value out and writing a
    /// replacement back is the point of the mode — `out.push(b.build()); b =
    /// StringBuilder.new()`. What it isn't is a way to give the value away: PM2
    /// promises the caller their value is still there when the call returns, and
    /// nothing checked that anything was put back.
    #[error("gave `{name}` away and didn't put anything back")]
    MutateParamLeftEmpty {
        name: String,
        /// Where it was consumed.
        consumed_at: Span,
        /// The parameter's declaration.
        declared_at: Span,
        /// True when only *some* paths consumed it — the message differs, and so
        /// does the fix.
        maybe: bool,
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

    /// #804: a `mutate` parameter was consumed and nothing put back, so the caller
    /// is left holding a value that moved out from under it.
    #[error("`{name}` was consumed and not replaced before returning")]
    MutateParamNotReplaced {
        name: String,
        ty: String,
        consumed_at: Span,
    },

    /// #804: a plain (borrowed) parameter was consumed — passed to a `take`
    /// parameter, or had a `take self` method called on it. The caller still owns
    /// it, so this is a second consumption of one value.
    #[error("`{name}` is borrowed from the caller and can't be given away")]
    ConsumedBorrowedParam {
        name: String,
        ty: String,
    },

    /// analysis.fourth-option: a link that would outlive the rack it points into.
    ///
    /// A link is a pointer to a node, and the nodes live in the rack. When the
    /// rack dies the node goes with it, so a link that escapes the rack's scope
    /// is dangling — with nothing deleted, so the use-after-delete rule never
    /// looks at it. A link is Copy and escapes freely, which is the point of a
    /// link and also exactly what block-scoped borrowing exists to stop.
    #[error("`{link}` would outlive the rack it points into")]
    LinkOutlivesRack {
        link: String,
        /// The rack, when this body declared it.
        rack: String,
        /// How it escapes — a return, or an assignment into a longer-lived name.
        via: LinkEscape,
        /// The escaping name is a *container* holding links rather than a link:
        /// `v.push(n)` then `return v`. Same dangle, different sentence.
        carried: bool,
    },

    /// analysis.fourth-option: a node written through a link whose rack this
    /// body may only read.
    ///
    /// A link is an access path into a rack, not a permission of its own, so the
    /// write is checked against the rack — the same rule `Handle` has, where
    /// `scene.nodes[h].f = x` needs `mutate scene`. Exempting links let
    /// `func combat_round(world: Rack<Entity>) { t.health -= e.damage }` mutate
    /// every node behind a signature promising the rack was only read.
    #[error("cannot write through `{link}` — nothing here grants writing this rack's nodes")]
    NodeWriteNeedsWritableRack {
        link: String,
        /// The rack, when this body can name it. `None` when the link came in as
        /// a parameter and no writable rack parameter came with it.
        rack: Option<String>,
    },

    /// analysis.fourth-option: an unnamed delete through a parameter that didn't
    /// declare `deleting`. The caller was never told its links could die here.
    #[error("`{operation}` through `{param}` deletes nodes the caller never named")]
    UndeclaredDelete {
        param: String,
        operation: String,
    },

    /// A binding that holds a resource the field walk could not name — inside a
    /// `Vec`, a `Map`, an optional, a tuple, an enum payload. The obligation falls
    /// back to the whole binding, and `where_` says which shape forced that so the
    /// root-named error doesn't read like a bug.
    #[error("`{name}` holds a resource in {where_} and must be used before the end of this block")]
    ResourceNotConsumedOpaque {
        name: String,
        where_: String,
    },

    /// Resource type not consumed before scope exit.
    #[error("`{name}` must be used before the end of this block")]
    ResourceNotConsumed {
        name: String,
    },

    /// H1: a resource-typed value produced by an expression statement is
    /// never bound to anything, so it's dropped unconsumed the instant it's
    /// produced (e.g. `spawn(|| { ... })` with no `let`).
    #[error("value of resource type `{type_name}` is dropped without being consumed")]
    ResourceDiscardedAsStatement {
        type_name: String,
    },

    /// mem.linear/L1 for an `Owned<T>` local: `own` allocated and nothing
    /// consumed it. Kept apart from `ResourceNotConsumed` because the fix is
    /// `drop(name)`, not `.close()`.
    #[error("`{name}` must be dropped before the end of this block")]
    OwnedNotConsumed {
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

/// How a link escapes its rack's scope. Drives the E0379 copy.
#[derive(Debug, Clone)]
pub enum LinkEscape {
    /// `return n` where the rack is a local of this function.
    Return,
    /// Assigned into a name declared in an outer scope.
    Assignment { target: String },
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
