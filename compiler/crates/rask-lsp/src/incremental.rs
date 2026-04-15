// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Apply `didChange` content changes to an in-memory buffer.
//!
//! LSP 3.17 ships two flavors of content change:
//!  - full sync: `range: None`, text is the whole document
//!  - incremental: `range: Some(Range)`, text replaces that range
//!
//! Incremental is the default we advertise — the client sends only the edit
//! that happened, not the entire buffer, which matters for large files.

use tower_lsp::lsp_types::TextDocumentContentChangeEvent;

use crate::convert::LineIndex;

pub fn apply_change(text: &mut String, change: TextDocumentContentChangeEvent) {
    match change.range {
        None => {
            *text = change.text;
        }
        Some(range) => {
            // Rebuild a LineIndex on every change — cheap enough for typical
            // edits and avoids stale byte-offset math. Calling code is already
            // on the blocking write path.
            let idx = LineIndex::new(text);
            let start = idx.position_to_offset(text, range.start);
            let end = idx.position_to_offset(text, range.end);
            // position_to_offset clamps to char boundaries, so the slice is
            // always safe.
            let (start, end) = if start > end { (end, start) } else { (start, end) };
            text.replace_range(start..end, &change.text);
        }
    }
}

/// Convenience for tests: apply a range replacement by (line, col) coordinates.
#[cfg(test)]
pub fn apply_range(text: &mut String, start: (u32, u32), end: (u32, u32), new_text: &str) {
    use tower_lsp::lsp_types::{Position, Range};
    apply_change(
        text,
        TextDocumentContentChangeEvent {
            range: Some(Range::new(
                Position::new(start.0, start.1),
                Position::new(end.0, end.1),
            )),
            range_length: None,
            text: new_text.to_string(),
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_sync_replaces_entire_buffer() {
        let mut text = "old".to_string();
        apply_change(
            &mut text,
            TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "new".to_string(),
            },
        );
        assert_eq!(text, "new");
    }

    #[test]
    fn incremental_insert_at_start() {
        let mut text = "world".to_string();
        apply_range(&mut text, (0, 0), (0, 0), "hello ");
        assert_eq!(text, "hello world");
    }

    #[test]
    fn incremental_replace_middle() {
        let mut text = "hello world".to_string();
        apply_range(&mut text, (0, 6), (0, 11), "there");
        assert_eq!(text, "hello there");
    }

    #[test]
    fn incremental_across_lines() {
        let mut text = "line1\nline2\nline3".to_string();
        apply_range(&mut text, (1, 0), (2, 0), "REPLACED\n");
        assert_eq!(text, "line1\nREPLACED\nline3");
    }

    #[test]
    fn emoji_edit_preserves_bytes() {
        let mut text = "a🌍b".to_string();
        // Replace "🌍" (col 1..3 in UTF-16) with "c"
        apply_range(&mut text, (0, 1), (0, 3), "c");
        assert_eq!(text, "acb");
    }
}
