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
}
