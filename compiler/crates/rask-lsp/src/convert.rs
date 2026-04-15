// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! LSP protocol conversion utilities.
//!
//! LSP v3.17 defaults to UTF-16 for positions. VS Code sends positions as
//! (line, character-in-UTF-16-code-units), but the compiler operates on UTF-8
//! byte offsets. A mismatch causes slice panics on emoji, accented letters,
//! and CJK text.
//!
//! `LineIndex` does the translation in one place. Build it once per document
//! version and reuse for all conversions in that pass — avoids repeated O(n)
//! scans of the source.

use tower_lsp::lsp_types::*;
use rask_diagnostics::LabelStyle;

use rask_ast::Span as RaskSpan;

/// Line-by-line UTF-8/UTF-16 index for a document.
///
/// For each line we remember the byte offset at which it starts and
/// whether the line is pure ASCII. Pure-ASCII lines skip the per-char
/// scan — in practice this is 95%+ of lines in most codebases.
#[derive(Debug, Clone)]
pub struct LineIndex {
    /// Byte offset at which each line starts. Always has len = line_count + 1,
    /// with the last entry pointing just past the end of the source.
    line_starts: Vec<u32>,
    /// Per-line ASCII flag. True = line is pure ASCII (byte col == UTF-16 col
    /// == char col).
    ascii_only: Vec<bool>,
    source_len: u32,
}

impl LineIndex {
    pub fn new(source: &str) -> Self {
        let bytes = source.as_bytes();
        let mut line_starts = Vec::with_capacity(source.len() / 32 + 1);
        let mut ascii_only = Vec::with_capacity(source.len() / 32 + 1);
        line_starts.push(0u32);

        let mut line_is_ascii = true;
        for (i, &b) in bytes.iter().enumerate() {
            if b >= 0x80 {
                line_is_ascii = false;
            }
            if b == b'\n' {
                ascii_only.push(line_is_ascii);
                line_starts.push((i + 1) as u32);
                line_is_ascii = true;
            }
        }
        ascii_only.push(line_is_ascii);
        // Sentinel pointing past the last byte.
        line_starts.push(bytes.len() as u32);

        Self {
            line_starts,
            ascii_only,
            source_len: bytes.len() as u32,
        }
    }

    pub fn line_count(&self) -> u32 {
        self.ascii_only.len() as u32
    }

    /// Line (0-based) that contains the given byte offset.
    pub fn line_of_offset(&self, offset: usize) -> u32 {
        let offset = (offset as u32).min(self.source_len);
        match self.line_starts.binary_search(&offset) {
            Ok(i) => (i as u32).min(self.line_count().saturating_sub(1)),
            Err(i) => (i.saturating_sub(1)) as u32,
        }
    }

    /// Byte offset → LSP Position (UTF-16).
    ///
    /// Out-of-range offsets clamp to the end of the document.
    pub fn offset_to_position(&self, source: &str, offset: usize) -> Position {
        let offset = (offset as u32).min(self.source_len);
        let line_idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => {
                // An exact hit on a line start (including sentinel); if this
                // is the sentinel row past EOF, back off to the last real line.
                if i == self.line_count() as usize {
                    i - 1
                } else if i < self.line_count() as usize
                    && self.line_starts[i] == offset
                {
                    i
                } else {
                    i.saturating_sub(1)
                }
            }
            Err(i) => i.saturating_sub(1),
        };
        let line = line_idx as u32;
        let line_start = self.line_starts[line_idx] as usize;
        let col_byte = offset as usize - line_start;

        let character = if *self.ascii_only.get(line_idx).unwrap_or(&true) {
            col_byte as u32
        } else {
            // Walk the line counting UTF-16 code units.
            let line_end = self.line_starts[line_idx + 1] as usize;
            let line_slice = &source[line_start..line_end.min(source.len())];
            utf16_units_for_bytes(line_slice, col_byte)
        };

        Position::new(line, character)
    }

    /// LSP Position (UTF-16) → byte offset.
    ///
    /// Out-of-range positions clamp to the end of the target line (or the end
    /// of the document if the line is beyond EOF). Always returns an offset on
    /// a valid UTF-8 char boundary — safe to slice the source at this point.
    pub fn position_to_offset(&self, source: &str, pos: Position) -> usize {
        if pos.line >= self.line_count() {
            return self.source_len as usize;
        }
        let line_idx = pos.line as usize;
        let line_start = self.line_starts[line_idx] as usize;
        let line_end = self.line_starts[line_idx + 1] as usize;
        // Trim trailing newline from the logical line length.
        let logical_end = if line_end > line_start
            && source.as_bytes().get(line_end - 1) == Some(&b'\n')
        {
            line_end - 1
        } else {
            line_end
        };

        let line_slice = &source[line_start..logical_end.min(source.len())];
        let col_byte = if *self.ascii_only.get(line_idx).unwrap_or(&true) {
            (pos.character as usize).min(line_slice.len())
        } else {
            bytes_for_utf16_units(line_slice, pos.character)
        };
        line_start + col_byte
    }

    /// Converts a rask Span to an LSP Range.
    pub fn span_to_range(&self, source: &str, span: RaskSpan) -> Range {
        Range::new(
            self.offset_to_position(source, span.start),
            self.offset_to_position(source, span.end),
        )
    }
}

/// Count UTF-16 code units corresponding to the first `byte_col` bytes of `line`.
fn utf16_units_for_bytes(line: &str, byte_col: usize) -> u32 {
    let clamped = byte_col.min(line.len());
    // Guard: clamped might land mid-char on an invalid offset. Round down to
    // the nearest char boundary so we never produce bogus counts.
    let safe = floor_char_boundary(line, clamped);
    let mut units = 0u32;
    for ch in line[..safe].chars() {
        units += ch.len_utf16() as u32;
    }
    units
}

/// Find the largest byte offset `<= target` that is a UTF-8 char boundary.
fn floor_char_boundary(s: &str, target: usize) -> usize {
    if target >= s.len() {
        return s.len();
    }
    let mut i = target;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Walk `line` UTF-16 code unit by code unit, stopping after `target_units`.
/// Returns the byte offset reached. Clamps to end-of-line.
fn bytes_for_utf16_units(line: &str, target_units: u32) -> usize {
    let mut seen_units = 0u32;
    for (byte_idx, ch) in line.char_indices() {
        let ch_units = ch.len_utf16() as u32;
        if seen_units + ch_units > target_units {
            return byte_idx;
        }
        seen_units += ch_units;
    }
    line.len()
}

// ─── Diagnostic conversion ────────────────────────────────────────────────

pub fn to_lsp_diagnostic(
    index: &LineIndex,
    source: &str,
    uri: &Url,
    diag: &rask_diagnostics::Diagnostic,
) -> Diagnostic {
    let primary = diag
        .labels
        .iter()
        .find(|l| l.style == LabelStyle::Primary)
        .or(diag.labels.first());

    let range = match primary {
        Some(l) => index.span_to_range(source, l.span),
        None => Range::new(Position::new(0, 0), Position::new(0, 0)),
    };

    let severity = Some(match diag.severity {
        rask_diagnostics::Severity::Error => DiagnosticSeverity::ERROR,
        rask_diagnostics::Severity::Warning => DiagnosticSeverity::WARNING,
        rask_diagnostics::Severity::Note => DiagnosticSeverity::INFORMATION,
    });

    let code = diag.code.as_ref().map(|c| NumberOrString::String(c.0.clone()));

    // Assemble the message: header + label note + notes + help.
    let mut message = diag.message.clone();
    if let Some(label) = primary {
        if let Some(ref msg) = label.message {
            message = format!("{}: {}", message, msg);
        }
    }
    for note in &diag.notes {
        message.push_str(&format!("\n\nnote: {}", note));
    }
    if let Some(ref help) = diag.help {
        message.push_str(&format!("\n\nhelp: {}", help.message));
    }

    let related_information: Vec<DiagnosticRelatedInformation> = diag
        .labels
        .iter()
        .filter(|l| l.style == LabelStyle::Secondary)
        .map(|l| DiagnosticRelatedInformation {
            location: Location {
                uri: uri.clone(),
                range: index.span_to_range(source, l.span),
            },
            message: l.message.clone().unwrap_or_default(),
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

pub fn ranges_overlap(r1: Range, r2: Range) -> bool {
    !(r1.end < r2.start || r2.end < r1.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(s: &str) -> (LineIndex, String) {
        (LineIndex::new(s), s.to_string())
    }

    #[test]
    fn ascii_positions_roundtrip() {
        let (li, s) = idx("abc\ndef\nghi");
        assert_eq!(li.offset_to_position(&s, 0), Position::new(0, 0));
        assert_eq!(li.offset_to_position(&s, 2), Position::new(0, 2));
        assert_eq!(li.offset_to_position(&s, 4), Position::new(1, 0));
        assert_eq!(li.offset_to_position(&s, 10), Position::new(2, 2));

        assert_eq!(li.position_to_offset(&s, Position::new(0, 0)), 0);
        assert_eq!(li.position_to_offset(&s, Position::new(1, 0)), 4);
        assert_eq!(li.position_to_offset(&s, Position::new(2, 2)), 10);
    }

    #[test]
    fn emoji_counts_as_two_utf16_units() {
        // "🌍" is 4 UTF-8 bytes, 2 UTF-16 code units, 1 char.
        let (li, s) = idx("a🌍b");
        // Offset 0 → col 0
        assert_eq!(li.offset_to_position(&s, 0), Position::new(0, 0));
        // Offset 1 → col 1 (after 'a')
        assert_eq!(li.offset_to_position(&s, 1), Position::new(0, 1));
        // Offset 5 → col 3 (after emoji = 2 code units)
        assert_eq!(li.offset_to_position(&s, 5), Position::new(0, 3));

        // Position col 3 → byte offset 5
        assert_eq!(li.position_to_offset(&s, Position::new(0, 3)), 5);
        // Position col 1 → byte offset 1
        assert_eq!(li.position_to_offset(&s, Position::new(0, 1)), 1);
    }

    #[test]
    fn position_beyond_end_is_clamped() {
        let (li, s) = idx("ab\ncd");
        assert_eq!(li.position_to_offset(&s, Position::new(99, 99)), 5);
        assert_eq!(li.position_to_offset(&s, Position::new(1, 99)), 5);
    }

    #[test]
    fn position_inside_emoji_never_panics() {
        // Offset 2 falls in the middle of "🌍" — should snap to 1 (before emoji).
        let (li, s) = idx("🌍");
        // LSP asks for col 1 — that's the high surrogate, our
        // implementation snaps down to 0.
        let off = li.position_to_offset(&s, Position::new(0, 1));
        assert!(off == 0 || off == 4, "got {}", off);
        assert!(s.is_char_boundary(off), "offset {} not a char boundary", off);
    }

    #[test]
    fn multiline_with_emoji() {
        let s = "héllo\n🌍world\nend";
        let li = LineIndex::new(s);
        // Line 1, col 1 (after 🌍 = 2 UTF-16 units)
        let off = li.position_to_offset(s, Position::new(1, 2));
        assert_eq!(off, 7 + 4); // "héllo\n" is 7 bytes, then 🌍 is 4 bytes
        assert!(s.is_char_boundary(off));
    }

    #[test]
    fn empty_source() {
        let s = "";
        let li = LineIndex::new(s);
        assert_eq!(li.position_to_offset(s, Position::new(0, 0)), 0);
        assert_eq!(li.offset_to_position(s, 0), Position::new(0, 0));
    }
}
