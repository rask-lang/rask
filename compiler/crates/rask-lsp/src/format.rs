// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Format a whole document via rask-fmt.

use tower_lsp::lsp_types::*;

use crate::convert::LineIndex;

pub fn format_document(source: &str) -> Option<Vec<TextEdit>> {
    let formatted = rask_fmt::format_source(source);
    if formatted == source {
        return Some(Vec::new());
    }
    let idx = LineIndex::new(source);
    let full = Range::new(
        Position::new(0, 0),
        idx.offset_to_position(source, source.len()),
    );
    Some(vec![TextEdit::new(full, formatted)])
}
