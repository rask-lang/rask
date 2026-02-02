//! Resolution error types.

use rask_ast::Span;
use thiserror::Error;

/// A name resolution error.
#[derive(Debug, Clone, Error)]
#[error("{kind}")]
pub struct ResolveError {
    pub kind: ResolveErrorKind,
    pub span: Span,
}

impl ResolveError {
    pub fn undefined(name: String, span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::UndefinedSymbol { name },
            span,
        }
    }

    pub fn duplicate(name: String, span: Span, previous: Span) -> Self {
        Self {
            kind: ResolveErrorKind::DuplicateDefinition { name, previous },
            span,
        }
    }

    pub fn invalid_break(label: Option<String>, span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::InvalidBreak { label },
            span,
        }
    }

    pub fn invalid_continue(label: Option<String>, span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::InvalidContinue { label },
            span,
        }
    }

    pub fn invalid_return(span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::InvalidReturn,
            span,
        }
    }
}

/// The kind of resolution error.
#[derive(Debug, Clone, Error)]
pub enum ResolveErrorKind {
    #[error("undefined symbol: {name}")]
    UndefinedSymbol { name: String },

    #[error("duplicate definition: {name} (previously defined at {previous:?})")]
    DuplicateDefinition { name: String, previous: Span },

    #[error("break outside of loop{}", label.as_ref().map(|l| format!(" (label: {})", l)).unwrap_or_default())]
    InvalidBreak { label: Option<String> },

    #[error("continue outside of loop{}", label.as_ref().map(|l| format!(" (label: {})", l)).unwrap_or_default())]
    InvalidContinue { label: Option<String> },

    #[error("return outside of function")]
    InvalidReturn,
}
