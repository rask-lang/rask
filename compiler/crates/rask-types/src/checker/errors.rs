// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type checker error types.

use rask_ast::Span;

use crate::types::{Type, TypeVarId};

/// A type error.
#[derive(Debug, thiserror::Error)]
pub enum TypeError {
    #[error("type mismatch: expected {expected}, found {found}")]
    Mismatch {
        expected: Type,
        found: Type,
        span: Span,
    },
    #[error("undefined type: {0}")]
    Undefined(String),
    #[error("arity mismatch: expected {expected} arguments, found {found}")]
    ArityMismatch {
        expected: usize,
        found: usize,
        span: Span,
    },
    #[error("type {ty} is not callable")]
    NotCallable { ty: Type, span: Span },
    #[error("no such field '{field}' on type {ty}")]
    NoSuchField { ty: Type, field: String, span: Span },
    #[error("no such method '{method}' on type {ty}")]
    NoSuchMethod {
        ty: Type,
        method: String,
        span: Span,
    },
    #[error("infinite type: type variable would create infinite type")]
    InfiniteType { var: TypeVarId, ty: Type, span: Span },
    #[error("cannot infer type")]
    CannotInfer { span: Span },
    #[error("invalid type string: {0}")]
    InvalidTypeString(String),
    #[error("try can only be used in functions returning Option or Result, found {return_ty}")]
    TryInNonPropagatingContext { return_ty: Type, span: Span },
    #[error("try can only be used within a function")]
    TryOutsideFunction { span: Span },
    #[error("missing return statement")]
    MissingReturn {
        function_name: String,
        expected_type: Type,
        span: Span,
    },
    #[error("generic argument error: {0}")]
    GenericError(String, Span),
    #[error("cannot mutate `{var}` while borrowed")]
    AliasingViolation {
        var: String,
        borrow_span: Span,
        access_span: Span,
    },
    #[error("cannot mutate parameter `{name}`")]
    MutateReadOnlyParam {
        name: String,
        span: Span,
    },
    #[error("string slices are temporary — cannot store `{view_var}`")]
    StringSliceStored {
        source_var: String,
        view_var: String,
        slice_span: Span,
        store_span: Span,
    },
    #[error("cannot hold view from growable source `{source_var}`")]
    VolatileViewStored {
        source_var: String,
        view_var: String,
        source_span: Span,
        store_span: Span,
    },
    #[error("cannot mutate `{source_var}` while viewed by `{view_var}`")]
    MutateBorrowedSource {
        source_var: String,
        view_var: String,
        borrow_span: Span,
        mutate_span: Span,
    },
    #[error("heap allocation in @no_alloc function: {reason}")]
    NoAllocViolation {
        reason: String,
        function_name: String,
        span: Span,
    },
    #[error("guard pattern 'else' block must diverge (return, panic, etc), found {found}")]
    GuardElseMustDiverge {
        found: Type,
        span: Span,
    },
    #[error("parameter `{param_name}` requires `mutate` annotation at call site")]
    MissingMutateAnnotation {
        param_name: String,
        param_index: usize,
        span: Span,
    },
    #[error("parameter `{param_name}` requires `own` annotation at call site")]
    MissingOwnAnnotation {
        param_name: String,
        param_index: usize,
        span: Span,
    },
    #[error("unexpected `{annotation}` annotation for parameter `{param_name}`")]
    UnexpectedAnnotation {
        annotation: String,
        param_name: String,
        param_index: usize,
        span: Span,
    },
    #[error("`try` requires a Result or Option type, found {found}")]
    TryOnNonResult {
        found: Type,
        span: Span,
    },
    #[error("{operation} requires `unsafe` block")]
    UnsafeRequired {
        operation: String,
        span: Span,
    },
    #[error("method `{method}` returns Self and cannot be called through `any {trait_name}`")]
    TraitObjectSelfReturn {
        trait_name: String,
        method: String,
        span: Span,
    },
    #[error("`{ty}` does not implement `{trait_name}`")]
    TraitNotSatisfied {
        ty: String,
        trait_name: String,
        span: Span,
    },

    #[error("the `+` operator cannot be used on strings")]
    StringAddForbidden {
        span: Span,
    },

    /// type.aliases/T9: nominal type used where underlying expected, or vice versa
    #[error("nominal type mismatch: expected `{expected}`, found `{found}`")]
    NominalMismatch {
        expected: Type,
        found: Type,
        nominal_name: String,
        span: Span,
    },

    /// ER21: public function uses `or _` (must declare error types explicitly)
    #[error("public function `{function_name}` must declare error types explicitly")]
    PublicInferredError {
        function_name: String,
        span: Span,
    },

    #[error("non-exhaustive match: missing variants {missing:?}")]
    NonExhaustiveMatch {
        missing: Vec<String>,
        span: Span,
    },

    #[error("undefined name `{name}`")]
    UndefinedName {
        name: String,
        span: Span,
    },

    #[error("unknown context `{name}` in `using` block")]
    UnknownContext {
        name: String,
        span: Span,
    },

    /// T6: cyclic type alias
    #[error("cyclic type alias: {cycle}")]
    CyclicTypeAlias {
        cycle: String,
        span: Span,
    },

    /// V5: private field accessed outside extend block
    #[error("field `{field}` on `{ty}` is private")]
    PrivateFieldAccess {
        ty: String,
        field: String,
        span: Span,
    },

    /// GC5: public function missing type annotation
    #[error("public function `{function_name}` requires explicit type annotations")]
    PublicMissingAnnotation {
        function_name: String,
        params: Vec<String>,
        missing_return: bool,
        span: Span,
    },

    /// D2: discard on Copy type (warning)
    #[error("`discard {name}` on Copy type `{ty}` has no effect")]
    DiscardCopyType {
        name: String,
        ty: Type,
        span: Span,
    },

    /// D3: discard on @resource type (error)
    #[error("cannot `discard` resource `{name}` — use its consuming method instead")]
    DiscardResourceType {
        name: String,
        ty: Type,
        span: Span,
    },

    /// D1: use after discard
    #[error("use of discarded value: `{name}`")]
    UseAfterDiscard {
        name: String,
        discarded_at: Span,
        span: Span,
    },

    /// SP3: zero step on range
    #[error("zero step")]
    ZeroStep {
        span: Span,
    },

    /// SP1/SP2: step direction doesn't match range direction (warning)
    #[error("step direction mismatch — range will be empty")]
    StepDirectionMismatch {
        range_span: Span,
        step_span: Span,
        /// "ascending" or "descending"
        range_direction: String,
        /// "positive" or "negative"
        step_direction: String,
    },

    /// ER26: @message variant missing coverage
    #[error("@message variant `{variant}` has no message template and cannot auto-delegate")]
    MessageCoverageMissing {
        variant: String,
        enum_name: String,
        span: Span,
    },

    /// E5/R5/MX3: standalone sync access without chaining
    #[error("standalone `.{method}()` on `{ty}` must be chained — use `.{method}().field` or `with` block")]
    BareSyncAccess {
        ty: String,
        method: String,
        span: Span,
    },

    /// E16: mixed explicit and auto-indexed discriminants
    #[error("enum `{enum_name}`: if any variant has `= N`, all must")]
    MixedDiscriminants {
        enum_name: String,
        span: Span,
    },

    /// E17: explicit discriminant on variant with fields
    #[error("enum `{enum_name}`: variant `{variant}` cannot have both fields and an explicit discriminant")]
    DiscriminantWithPayload {
        enum_name: String,
        variant: String,
        span: Span,
    },

    /// E15: duplicate discriminant value
    #[error("enum `{enum_name}`: duplicate discriminant value {value} on `{first}` and `{second}`")]
    DuplicateDiscriminant {
        enum_name: String,
        value: i128,
        first: String,
        second: String,
        span: Span,
    },

    /// ER3: success and error types in `T or E` must be distinct
    #[error("`T or E` requires T and E to be distinct types — both sides are `{ty}`")]
    ResultNotDisjoint {
        ty: Type,
        span: Span,
    },

    /// ER4: error type must implement `ErrorMessage` (structural: `message(self) -> string`)
    #[error("error type `{ty}` must implement `ErrorMessage` — needs `func message(self) -> string`")]
    ErrorMessageMissing {
        ty: Type,
        span: Span,
    },

    /// ER22: `else as e` requires a `T or E` condition to bind the error
    #[error("`else as {name}` requires an `if r?` condition on a Result (`T or E`)")]
    ElseBindingNotResult {
        name: String,
        span: Span,
    },

    /// ER23: `is TypeName as ...` requires the scrutinee to be a Result
    #[error("type pattern `{ty_name}` requires a Result scrutinee, found `{found}`")]
    TypePatternNotResult {
        ty_name: String,
        found: Type,
        span: Span,
    },

    /// ER23: `is TypeName` must reference a component of the union error
    #[error("type pattern `{ty_name}` is not part of the error union `{union}`")]
    TypePatternNotInUnion {
        ty_name: String,
        union: Type,
        span: Span,
    },

    /// OPT2/ER2: legacy `Some(x)`/`Ok(x)`/`Err(x)` constructor — migration error
    #[error("`{name}(...)` is no longer a valid constructor")]
    LegacyWrapperConstructor {
        name: String,
        span: Span,
    },

    /// OPT NO_MATCH: match on `T?` is rejected — migration error
    #[error("match on an Option is not supported — use the `?`-operator family")]
    MatchOnOption {
        span: Span,
    },
}
