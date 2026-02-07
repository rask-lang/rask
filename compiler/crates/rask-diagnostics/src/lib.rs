// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask compiler diagnostics.
//!
//! Provides a unified diagnostic type that both CLI and language server consume.
//! Each compiler phase's error types are converted to `Diagnostic` via the
//! `ToDiagnostic` trait, keeping compiler crates lightweight while enabling
//! rich error display.

pub mod codes;
pub mod convert;
pub mod formatter;
pub mod json;
pub mod suggestions;

use rask_ast::Span;
use serde::Serialize;

// ============================================================================
// Core Types
// ============================================================================

/// A compiler diagnostic with rich context for display.
#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: Option<ErrorCode>,
    pub message: String,
    pub labels: Vec<Label>,
    pub notes: Vec<String>,
    pub help: Option<Help>,
}

/// A labeled source span within a diagnostic.
#[derive(Debug, Clone, Serialize)]
pub struct Label {
    pub span: Span,
    pub style: LabelStyle,
    pub message: Option<String>,
}

/// How a label should be displayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LabelStyle {
    /// Primary error location (red underline).
    Primary,
    /// Related location (yellow/blue underline).
    Secondary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Note,
}

/// An error code like E0308.
#[derive(Debug, Clone, Serialize)]
#[serde(transparent)]
pub struct ErrorCode(pub String);

/// Actionable help attached to a diagnostic.
#[derive(Debug, Clone, Serialize)]
pub struct Help {
    pub message: String,
    pub suggestion: Option<CodeSuggestion>,
}

/// A concrete code change suggestion.
#[derive(Debug, Clone, Serialize)]
pub struct CodeSuggestion {
    pub span: Span,
    pub replacement: String,
}

// ============================================================================
// Builder API
// ============================================================================

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: None,
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            code: None,
            message: message.into(),
            labels: Vec::new(),
            notes: Vec::new(),
            help: None,
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(ErrorCode(code.into()));
        self
    }

    pub fn with_label(mut self, span: Span, style: LabelStyle, msg: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            style,
            message: Some(msg.into()),
        });
        self
    }

    pub fn with_primary(self, span: Span, msg: impl Into<String>) -> Self {
        self.with_label(span, LabelStyle::Primary, msg)
    }

    pub fn with_secondary(self, span: Span, msg: impl Into<String>) -> Self {
        self.with_label(span, LabelStyle::Secondary, msg)
    }

    pub fn with_note(mut self, note: impl Into<String>) -> Self {
        self.notes.push(note.into());
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(Help {
            message: help.into(),
            suggestion: None,
        });
        self
    }

    pub fn with_suggestion(mut self, span: Span, replacement: impl Into<String>) -> Self {
        if let Some(ref mut help) = self.help {
            help.suggestion = Some(CodeSuggestion {
                span,
                replacement: replacement.into(),
            });
        }
        self
    }

    /// Returns the primary span (first primary label, or first label).
    pub fn primary_span(&self) -> Option<Span> {
        self.labels
            .iter()
            .find(|l| l.style == LabelStyle::Primary)
            .or(self.labels.first())
            .map(|l| l.span)
    }
}

// ============================================================================
// Conversion Trait
// ============================================================================

/// Convert a compiler error into a rich diagnostic.
pub trait ToDiagnostic {
    fn to_diagnostic(&self) -> Diagnostic;
}
