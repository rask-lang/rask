// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Rask compiler diagnostics.
//!
//! Provides a unified diagnostic type that both CLI and language server consume.
//! Each compiler phase's error types are converted to `Diagnostic` via the
//! `ToDiagnostic` interface, keeping compiler crates lightweight while enabling
//! rich error display.

pub mod codes;
pub mod convert;
pub mod formatter;
pub mod source_map;
pub mod json;
pub mod suggestions;

use rask_ast::Span;
use serde::Serialize;

/// Decide whether the formatter writes colour, instead of letting it guess.
///
/// `colored` guesses by looking at stdout. That is right for a terminal and
/// wrong everywhere the escapes are wanted but stdout isn't a tty — the
/// playground being the case in hand. It renders a diagnostic to HTML by
/// turning the escapes into spans, and on wasm there is no terminal to find,
/// so the formatter wrote none and every error arrived as flat text.
pub fn set_color(on: bool) {
    colored::control::set_override(on);
}

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
    /// Concrete fix instruction (e.g., "clone before transfer").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// One-sentence rule explanation (e.g., "`own` transfers ownership").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
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

// ============================================================================
// Prose check
// ============================================================================

/// Mid-line run of 8+ spaces, or `None`.
///
/// A long message written as a multi-line Rust literal keeps every space of
/// the source indentation unless each line ends in `\`. The terminal formatter
/// re-wraps, so the gap is invisible there — but `--format json` hands the
/// string over verbatim and the editor shows it as written. Six messages
/// shipped with runs of ~30 spaces in the middle of a sentence before anyone
/// looked at the JSON (#1298).
///
/// Runs that are deliberate don't count. A `fix` is often a code sample, and a
/// sample indents after a newline, lines its trailing `//` comments up, and
/// sometimes lays two columns out — `Float    → f32 f64`. None of that goes
/// past six spaces in the messages we have; the accidents start at eighteen.
/// So: eight, ignoring a leading indent and a gap before a comment.
fn stray_gap(text: &str) -> Option<&str> {
    for line in text.lines() {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b' ' {
                let start = i;
                while i < bytes.len() && bytes[i] == b' ' {
                    i += 1;
                }
                let aligns_a_comment = line[i..].starts_with("//");
                if start > 0 && i - start >= 8 && i < bytes.len() && !aligns_a_comment {
                    return Some(line);
                }
            } else {
                i += 1;
            }
        }
    }
    None
}

/// Panics in debug builds on a message carrying a stray gap.
#[track_caller]
fn check_prose(field: &str, text: &str) {
    if cfg!(debug_assertions) {
        if let Some(line) = stray_gap(text) {
            panic!(
                "diagnostic {field} has a run of spaces in the middle of a line, which \
                 `--format json` shows verbatim. A multi-line Rust literal needs `\\` at \
                 the end of each line to swallow the indentation.\n  {line}"
            );
        }
    }
}

impl Diagnostic {
    pub fn error(message: impl Into<String>) -> Self {
        let message = message.into();
        check_prose("message", &message);
        Self {
            severity: Severity::Error,
            code: None,
            message,
            labels: Vec::new(),
            notes: Vec::new(),
            help: None,
            fix: None,
            why: None,
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        let message = message.into();
        check_prose("message", &message);
        Self {
            severity: Severity::Warning,
            code: None,
            message,
            labels: Vec::new(),
            notes: Vec::new(),
            help: None,
            fix: None,
            why: None,
        }
    }

    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(ErrorCode(code.into()));
        self
    }

    pub fn with_label(mut self, span: Span, style: LabelStyle, msg: impl Into<String>) -> Self {
        let msg = msg.into();
        check_prose("label", &msg);
        self.labels.push(Label {
            span,
            style,
            message: Some(msg),
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
        let note = note.into();
        check_prose("note", &note);
        self.notes.push(note);
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        let help = help.into();
        check_prose("help", &help);
        self.help = Some(Help {
            message: help,
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

    pub fn with_fix(mut self, fix: impl Into<String>) -> Self {
        let fix = fix.into();
        check_prose("fix", &fix);
        self.fix = Some(fix);
        self
    }

    pub fn with_why(mut self, why: impl Into<String>) -> Self {
        let why = why.into();
        check_prose("why", &why);
        self.why = Some(why);
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
// Conversion Interface
// ============================================================================

/// Convert a compiler error into a rich diagnostic.
pub trait ToDiagnostic {
    fn to_diagnostic(&self) -> Diagnostic;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gap_in_the_middle_of_a_sentence_is_caught() {
        // What a Rust literal continued without `\` produces.
        assert!(stray_gap("take `any Shape` and use a sentinel, or wrap it in            a struct field").is_some());
        assert!(stray_gap("one\ntwo            three").is_some());
    }

    #[test]
    fn ordinary_prose_and_indented_code_samples_pass() {
        assert!(stray_gap("nothing in scope pins this down, so annotate it").is_none());
        // Two spaces after a full stop, and a trailing one, are not gaps.
        assert!(stray_gap("done.  next").is_none());
        // A two-column table in a `fix`.
        assert!(stray_gap("      Float    → f32 f64").is_none());
        // A code sample lining up its trailing comments.
        assert!(stray_gap("x.to<i8>()!   // asserts it fits").is_none());
        assert!(stray_gap("x!                // assert it's there").is_none());
        assert!(stray_gap("trailing   ").is_none());
        // A code sample indents after the newline, which is the point of it.
        assert!(stray_gap("if pool.get(0) is Some {\n      pool[0].field\n  }").is_none());
    }

    #[test]
    #[should_panic(expected = "run of spaces")]
    #[cfg(debug_assertions)]
    fn the_builders_refuse_a_gap() {
        Diagnostic::error("x").with_help("a help that got            wrapped wrong");
    }
}
