// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Source location tracking.

/// A span in the source code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }
}

/// Precomputed line-start offsets for O(log n) byte-offset → line:col lookup.
#[derive(Debug, Clone)]
pub struct LineMap {
    /// Byte offset of the start of each line. line_starts[0] is always 0.
    line_starts: Vec<u32>,
}

impl LineMap {
    /// Build a line map by scanning source for newlines. O(n).
    pub fn new(source: &str) -> Self {
        let mut line_starts = vec![0u32];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push((i + 1) as u32);
            }
        }
        LineMap { line_starts }
    }

    /// Convert byte offset to (line, col), both 1-based. O(log n).
    pub fn offset_to_line_col(&self, offset: usize) -> (u32, u32) {
        let offset = offset as u32;
        let line_idx = match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i.saturating_sub(1),
        };
        let line = (line_idx + 1) as u32;
        let col = offset - self.line_starts[line_idx] + 1;
        (line, col)
    }

    /// Get the source text of a 1-based line number. O(1).
    pub fn line_text<'a>(&self, source: &'a str, line: u32) -> Option<&'a str> {
        let idx = (line as usize).checked_sub(1)?;
        let start = *self.line_starts.get(idx)? as usize;
        let end = self
            .line_starts
            .get(idx + 1)
            .map(|&s| (s as usize).saturating_sub(1)) // exclude the \n
            .unwrap_or(source.len());
        source.get(start..end)
    }

    /// Number of lines in the source.
    pub fn line_count(&self) -> u32 {
        self.line_starts.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_source() {
        let lm = LineMap::new("");
        assert_eq!(lm.offset_to_line_col(0), (1, 1));
        assert_eq!(lm.line_count(), 1);
    }

    #[test]
    fn single_line() {
        let lm = LineMap::new("hello");
        assert_eq!(lm.offset_to_line_col(0), (1, 1));
        assert_eq!(lm.offset_to_line_col(4), (1, 5));
        assert_eq!(lm.line_text("hello", 1), Some("hello"));
        assert_eq!(lm.line_text("hello", 2), None);
    }

    #[test]
    fn multi_line() {
        let src = "abc\ndef\nghi";
        let lm = LineMap::new(src);
        assert_eq!(lm.line_count(), 3);
        // First line
        assert_eq!(lm.offset_to_line_col(0), (1, 1)); // 'a'
        assert_eq!(lm.offset_to_line_col(2), (1, 3)); // 'c'
        // Second line
        assert_eq!(lm.offset_to_line_col(4), (2, 1)); // 'd'
        assert_eq!(lm.offset_to_line_col(6), (2, 3)); // 'f'
        // Third line
        assert_eq!(lm.offset_to_line_col(8), (3, 1)); // 'g'

        assert_eq!(lm.line_text(src, 1), Some("abc"));
        assert_eq!(lm.line_text(src, 2), Some("def"));
        assert_eq!(lm.line_text(src, 3), Some("ghi"));
    }

    #[test]
    fn offset_at_newline() {
        let src = "ab\ncd\n";
        let lm = LineMap::new(src);
        // Offset 2 is the '\n' — belongs to line 1
        assert_eq!(lm.offset_to_line_col(2), (1, 3));
        // Offset 3 is 'c' — line 2
        assert_eq!(lm.offset_to_line_col(3), (2, 1));
        // Offset 5 is the trailing '\n' — line 2
        assert_eq!(lm.offset_to_line_col(5), (2, 3));
    }

    #[test]
    fn trailing_newline() {
        let src = "abc\n";
        let lm = LineMap::new(src);
        assert_eq!(lm.line_count(), 2);
        assert_eq!(lm.line_text(src, 1), Some("abc"));
        // Line 2 is empty (after trailing newline)
        assert_eq!(lm.line_text(src, 2), Some(""));
    }
}
