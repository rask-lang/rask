// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! LSP protocol conversion utilities.

use tower_lsp::lsp_types::*;
use rask_diagnostics::LabelStyle;

/// Convert LSP Position (line/col) to byte offset.
pub fn position_to_offset(source: &str, pos: Position) -> usize {
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, ch) in source.char_indices() {
        if line == pos.line && col == pos.character {
            return i;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    source.len()
}

/// Convert byte offset to LSP Position.
pub fn byte_offset_to_position(source: &str, offset: usize) -> Position {
    let mut line = 0u32;
    let mut col = 0u32;

    for (i, ch) in source.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }

    Position::new(line, col)
}

/// Convert a rask diagnostic to an LSP diagnostic.
pub fn to_lsp_diagnostic(
    source: &str,
    uri: &Url,
    diag: &rask_diagnostics::Diagnostic,
) -> Diagnostic {
    // Primary span determines the main range
    let primary = diag
        .labels
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .or(diag.labels.first());

    let range = if let Some(label) = primary {
        let start = byte_offset_to_position(source, label.span.start);
        let end = byte_offset_to_position(source, label.span.end);
        Range::new(start, end)
    } else {
        Range::new(Position::new(0, 0), Position::new(0, 0))
    };

    let severity = Some(match diag.severity {
        rask_diagnostics::Severity::Error => DiagnosticSeverity::ERROR,
        rask_diagnostics::Severity::Warning => DiagnosticSeverity::WARNING,
        rask_diagnostics::Severity::Note => DiagnosticSeverity::INFORMATION,
    });

    let code = diag
        .code
        .as_ref()
        .map(|c| NumberOrString::String(c.0.clone()));

    // Build message: main message + primary label + notes + help
    let mut message = diag.message.clone();

    if let Some(label) = primary {
        if let Some(ref msg) = label.message {
            message = format!("{}: {}", message, msg);
        }
    }

    for note in &diag.notes {
        message = format!("{}\n\nnote: {}", message, note);
    }

    if let Some(ref help) = diag.help {
        message = format!("{}\n\nhelp: {}", message, help.message);
    }

    // Secondary labels become related information
    let related_information: Vec<DiagnosticRelatedInformation> = diag
        .labels
        .iter()
        .filter(|l| l.style == LabelStyle::Secondary)
        .map(|l| {
            let start = byte_offset_to_position(source, l.span.start);
            let end = byte_offset_to_position(source, l.span.end);
            DiagnosticRelatedInformation {
                location: Location {
                    uri: uri.clone(),
                    range: Range::new(start, end),
                },
                message: l.message.clone().unwrap_or_default(),
            }
        })
        .collect();

    Diagnostic {
        range,
        severity,
        code,
        code_description: None,
        source: Some("rask".to_string()),
        message,
        related_information: if related_information.is_empty() {
            None
        } else {
            Some(related_information)
        },
        tags: None,
        data: None,
    }
}

/// Check if two ranges overlap.
pub fn ranges_overlap(r1: Range, r2: Range) -> bool {
    !(r1.end < r2.start || r2.end < r1.start)
}
