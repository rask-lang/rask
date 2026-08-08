// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Which file a `Span` came from.
//!
//! `Span` has carried a `file_id` for a long time and the parser fills it in per
//! file, but nothing on the rendering side could read it — the formatter took a
//! single source string, so every span in a package was resolved against
//! whichever file the caller happened to pass. Real errors landed on the wrong
//! file at impossible columns, and fixes aimed at span *propagation* never
//! changed the output because propagation was already correct.
//!
//! This is the missing half: the lookup a `file_id` resolves through.
//!
//! Ids must be assigned the same way the parser assigns them — see
//! `SourceMap::push`.

use rask_ast::LineMap;

/// One source file: its display name, its text, and the line index for it.
pub struct SourceFile {
    pub name: String,
    pub text: String,
    pub line_map: LineMap,
}

/// `file_id` → source, indexed positionally.
#[derive(Default)]
pub struct SourceMap {
    files: Vec<SourceFile>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a file and return the id it was given.
    ///
    /// Ids are positional, so files must be pushed in exactly the order the
    /// parser numbered them. In particular the parser advances its counter only
    /// for files that lex *and* parse, so a file that failed either step must be
    /// skipped here too — pushing it anyway shifts every later id by one and
    /// silently mis-attributes the rest of the package.
    pub fn push(&mut self, name: impl Into<String>, text: impl Into<String>) -> u16 {
        let text = text.into();
        let line_map = LineMap::new(&text);
        self.files.push(SourceFile { name: name.into(), text, line_map });
        (self.files.len() - 1) as u16
    }

    pub fn get(&self, file_id: u16) -> Option<&SourceFile> {
        self.files.get(file_id as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }
}
