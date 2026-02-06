//! JSON diagnostic output for machine consumption.
//!
//! Produces structured JSON that IDEs and AI agents can parse to understand
//! and fix errors. Each diagnostic includes source context, exact locations
//! (line/col), and actionable repair suggestions.
//!
//! Use `--format json` with any rask command to get this output.

use serde::Serialize;

use crate::{codes::ErrorCodeRegistry, Diagnostic, LabelStyle};

/// A complete JSON diagnostic report for a compilation run.
#[derive(Debug, Serialize)]
pub struct DiagnosticReport {
    /// Schema version for forward compatibility.
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
}

/// A source location with line/column (1-based).
#[derive(Debug, Serialize)]
pub struct SourceLocation {
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
pub fn to_json_report(
    diagnostics: &[Diagnostic],
    source: &str,
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
            to_json_diagnostic(d, source, &registry)
        })
        .collect();

    DiagnosticReport {
        version: 1,
        file: file.to_string(),
        success: error_count == 0,
        phase: phase.to_string(),
        diagnostics: json_diags,
        error_count,
        warning_count,
    }
}

fn to_json_diagnostic(
    diag: &Diagnostic,
    source: &str,
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

    // Primary location
    let location = diag
        .labels
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .or(diag.labels.first())
        .map(|l| {
            let (line, col) = offset_to_line_col(source, l.span.start);
            SourceLocation {
                line,
                column: col,
                byte_offset: l.span.start,
                source_line: get_line(source, line).unwrap_or("").to_string(),
            }
        });

    // Labels
    let labels = diag
        .labels
        .iter()
        .map(|l| {
            let (start_line, start_col) = offset_to_line_col(source, l.span.start);
            let (end_line, end_col) = offset_to_line_col(source, l.span.end);
            JsonLabel {
                role: match l.style {
                    LabelStyle::Primary => "primary".to_string(),
                    LabelStyle::Secondary => "secondary".to_string(),
                },
                message: l.message.clone(),
                start: LineCol {
                    line: start_line,
                    column: start_col,
                    byte_offset: l.span.start,
                },
                end: LineCol {
                    line: end_line,
                    column: end_col,
                    byte_offset: l.span.end,
                },
                source_line: get_line(source, start_line).unwrap_or("").to_string(),
            }
        })
        .collect();

    // Suggestion
    let suggestion = diag.help.as_ref().and_then(|h| {
        h.suggestion.as_ref().map(|s| {
            let (line, col) = offset_to_line_col(source, s.span.start);
            let original_line = get_line(source, line).unwrap_or("");
            let span_len = s.span.end.saturating_sub(s.span.start);
            let prefix = &original_line[..col.saturating_sub(1).min(original_line.len())];
            let suffix_start = (col - 1 + span_len).min(original_line.len());
            let suffix = &original_line[suffix_start..];
            let result_line = format!("{}{}{}", prefix, s.replacement, suffix);

            JsonSuggestion {
                span: JsonSpan {
                    start: s.span.start,
                    end: s.span.end,
                },
                replacement: s.replacement.clone(),
                result_line,
            }
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
    }
}

/// Convert byte offset to (line, col), both 1-based.
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Get source line text by 1-based line number.
fn get_line(source: &str, line_num: usize) -> Option<&str> {
    source.lines().nth(line_num - 1)
}

/// Serialize a diagnostic report to pretty JSON.
pub fn to_json_string(report: &DiagnosticReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e))
}
