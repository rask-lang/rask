// SPDX-License-Identifier: (MIT OR Apache-2.0)

pub struct FormatConfig {
    pub indent_width: usize,
    pub max_line_width: usize,
    /// Parse the way the stdlib stub loader does. `stdlib/builtins.rk` declares
    /// `assert`, `print` and friends, whose names are keywords — the loader turns
    /// that allowance on, so the formatter has to as well or it can't read the
    /// files it's asked to format.
    pub allow_keyword_fn_names: bool,
}

impl Default for FormatConfig {
    fn default() -> Self {
        Self {
            indent_width: 4,
            max_line_width: 100,
            allow_keyword_fn_names: false,
        }
    }
}
