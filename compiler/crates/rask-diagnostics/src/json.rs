// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! JSON diagnostic output for machine consumption.
//!
//! Produces structured JSON that IDEs and AI agents can parse to understand
//! and fix errors. Each diagnostic includes source context, exact locations
//! (line/col), and actionable repair suggestions.
//!
//! Use `--format json` with any rask command to get this output.

use serde::Serialize;

use rask_ast::Span;

use crate::{codes::ErrorCodeRegistry, source_map::{SourceFile, SourceMap}, Diagnostic, LabelStyle};

/// A complete JSON diagnostic report for a compilation run.
#[derive(Debug, Serialize)]
pub struct DiagnosticReport {
    /// Schema version for forward compatibility (v2: added fix/why fields).
    pub version: u32,
    /// The file that was compiled.
    pub file: String,
    /// Whether compilation succeeded (no errors).
    pub success: bool,
    /// The compilation phase that produced these diagnostics.
    pub phase: String,
    /// All diagnostics from this compilation.
    pub diagnostics: Vec<JsonDiagnostic>,
    /// Total error count.
    pub error_count: usize,
    /// Total warning count.
    pub warning_count: usize,
}

/// A single diagnostic in JSON form, enriched with source context.
#[derive(Debug, Serialize)]
pub struct JsonDiagnostic {
    /// Severity: "error", "warning", or "note".
    pub severity: String,
    /// Error code (e.g., "E0308").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    /// Error category (e.g., "Type", "Ownership").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    /// Human-readable error message.
    pub message: String,
    /// Primary source location.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<SourceLocation>,
    /// All labeled source spans.
    pub labels: Vec<JsonLabel>,
    /// Additional notes.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
    /// Actionable help message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// Concrete code fix suggestion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<JsonSuggestion>,
    /// Concrete fix instruction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
    /// One-sentence rule explanation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

/// A source location with line/column (1-based).
#[derive(Debug, Serialize)]
pub struct SourceLocation {
    /// The file the line and column count in. In a package that's often not
    /// the report's `file`.
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub byte_offset: usize,
    /// The source line text for context.
    pub source_line: String,
}

/// A labeled span in JSON form.
#[derive(Debug, Serialize)]
pub struct JsonLabel {
    /// "primary" or "secondary".
    pub role: String,
    /// The file this label's span is in.
    pub file: String,
    /// Label message.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Start location.
    pub start: LineCol,
    /// End location.
    pub end: LineCol,
    /// The source line containing this label.
    pub source_line: String,
}

/// Line/column pair (1-based).
#[derive(Debug, Serialize)]
pub struct LineCol {
    pub line: usize,
    pub column: usize,
    pub byte_offset: usize,
}

/// A concrete code replacement suggestion.
#[derive(Debug, Serialize)]
pub struct JsonSuggestion {
    /// What to replace.
    pub span: JsonSpan,
    /// The replacement text.
    pub replacement: String,
    /// The full line after applying the fix.
    pub result_line: String,
}

/// Byte span in JSON form.
#[derive(Debug, Serialize)]
pub struct JsonSpan {
    pub start: usize,
    pub end: usize,
}

/// Convert diagnostics to a structured JSON report.
///
/// Every span is placed through `sources` by its `file_id`. Counting a span
/// from another file of the package against `file`'s text gave a line and
/// column that pointed at nothing, with a source line from the wrong file.
/// A span whose file isn't in the map gets no location.
pub fn to_json_report(
    diagnostics: &[Diagnostic],
    sources: &SourceMap,
    file: &str,
    phase: &str,
) -> DiagnosticReport {
    let registry = ErrorCodeRegistry::default();
    let mut error_count = 0;
    let mut warning_count = 0;

    let json_diags: Vec<JsonDiagnostic> = diagnostics
        .iter()
        .map(|d| {
            match d.severity {
                crate::Severity::Error => error_count += 1,
                crate::Severity::Warning => warning_count += 1,
                crate::Severity::Note => {}
            }
            to_json_diagnostic(d, sources, &registry)
        })
        .collect();

    DiagnosticReport {
        version: 2,
        file: file.to_string(),
        success: error_count == 0,
        phase: phase.to_string(),
        diagnostics: json_diags,
        error_count,
        warning_count,
    }
}

/// Line and column of `offset` in `file`, 1-based.
fn line_col(file: &SourceFile, offset: usize) -> (usize, usize) {
    let (line, col) = file.line_map.offset_to_line_col(offset);
    (line as usize, col as usize)
}

fn line_text(file: &SourceFile, line: usize) -> String {
    file.line_map.line_text(&file.text, line as u32).unwrap_or("").to_string()
}

fn to_json_diagnostic(
    diag: &Diagnostic,
    sources: &SourceMap,
    registry: &ErrorCodeRegistry,
) -> JsonDiagnostic {
    let severity = match diag.severity {
        crate::Severity::Error => "error",
        crate::Severity::Warning => "warning",
        crate::Severity::Note => "note",
    };

    let code = diag.code.as_ref().map(|c| c.0.clone());
    let category = code
        .as_ref()
        .and_then(|c| registry.get(c))
        .map(|info| info.category.to_string());
    let file_of = |span: &Span| sources.get(span.file_id);

    let location = diag
        .labels
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .or(diag.labels.first())
        .and_then(|l| {
            let file = file_of(&l.span)?;
            let (line, column) = line_col(file, l.span.start);
            Some(SourceLocation {
                file: file.name.clone(),
                line,
                column,
                byte_offset: l.span.start,
                source_line: line_text(file, line),
            })
        });

    let labels = diag
        .labels
        .iter()
        .filter_map(|l| {
            let file = file_of(&l.span)?;
            let (start_line, start_col) = line_col(file, l.span.start);
            let (end_line, end_col) = line_col(file, l.span.end);
            Some(JsonLabel {
                role: match l.style {
                    LabelStyle::Primary => "primary".to_string(),
                    LabelStyle::Secondary => "secondary".to_string(),
                },
                file: file.name.clone(),
                message: l.message.clone(),
                start: LineCol { line: start_line, column: start_col, byte_offset: l.span.start },
                end: LineCol { line: end_line, column: end_col, byte_offset: l.span.end },
                source_line: line_text(file, start_line),
            })
        })
        .collect();

    let suggestion = diag.help.as_ref().and_then(|h| {
        let s = h.suggestion.as_ref()?;
        let file = file_of(&s.span)?;
        let (line, col) = line_col(file, s.span.start);
        let original_line = line_text(file, line);
        let span_len = s.span.end.saturating_sub(s.span.start);
        let prefix = &original_line[..col.saturating_sub(1).min(original_line.len())];
        let suffix_start = (col - 1 + span_len).min(original_line.len());
        let suffix = &original_line[suffix_start..];
        Some(JsonSuggestion {
            span: JsonSpan { start: s.span.start, end: s.span.end },
            replacement: s.replacement.clone(),
            result_line: format!("{}{}{}", prefix, s.replacement, suffix),
        })
    });

    JsonDiagnostic {
        severity: severity.to_string(),
        code,
        category,
        message: diag.message.clone(),
        location,
        labels,
        notes: diag.notes.clone(),
        help: diag.help.as_ref().map(|h| h.message.clone()),
        suggestion,
        fix: diag.fix.clone(),
        why: diag.why.clone(),
    }
}

/// Serialize a diagnostic report to pretty JSON.
pub fn to_json_string(report: &DiagnosticReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
}
