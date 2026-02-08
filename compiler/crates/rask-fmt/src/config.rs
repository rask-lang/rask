// SPDX-License-Identifier: (MIT OR Apache-2.0)

pub struct FormatConfig {
    pub indent_width: usize,
    pub max_line_width: usize,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            indent_width: 4,
            max_line_width: 100,
        }
    }
}
