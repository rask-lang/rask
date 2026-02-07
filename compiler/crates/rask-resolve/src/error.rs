// SPDX-License-Identifier: (MIT OR Apache-2.0)
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

    pub fn unknown_package(path: Vec<String>, span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::UnknownPackage { path },
            span,
        }
    }

    pub fn not_visible(name: String, span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::NotVisible { name },
            span,
        }
    }

    pub fn shadows_import(name: String, span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::ShadowsImport { name },
            span,
        }
    }

    pub fn circular_dependency(path: Vec<String>, span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::CircularDependency { path },
            span,
        }
    }

    pub fn shadows_builtin(name: String, span: Span) -> Self {
        Self {
            kind: ResolveErrorKind::ShadowsBuiltin { name },
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

    #[error("unknown package: `{}`", if path.is_empty() { "<empty>".to_string() } else { path.join(".") })]
    UnknownPackage { path: Vec<String> },

    #[error("`{name}` is not public and cannot be accessed from this package")]
    NotVisible { name: String },

    #[error("cannot define `{name}` because it shadows an imported name; consider using a different name or aliasing the import")]
    ShadowsImport { name: String },

    #[error("circular import dependency detected: {}", path.join(" -> "))]
    CircularDependency { path: Vec<String> },

    #[error("cannot define `{name}` because it shadows a built-in; built-in types and functions cannot be redefined")]
    ShadowsBuiltin { name: String },
}
