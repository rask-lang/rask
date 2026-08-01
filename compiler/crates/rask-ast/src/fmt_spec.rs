// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The format-spec grammar for string interpolation — `{expr:spec}`.
//!
//! One definition, shared by everything that reads a `{...}`: the parser
//! deciding whether the braces are an interpolation at all, and the formatters
//! deciding what to do with the spec. They used to disagree. The parser
//! accepted any run of alphanumerics as "plausibly a spec", so
//! `"{\"k\": 1}"` — a one-pair JSON body — parsed as the expression `"k"` with
//! ` 1` as its spec, printed `k`, and dropped the rest without a word.

/// The specs the formatters actually understand. Anything else means the
/// braces were never an interpolation.
pub fn is_valid_spec(spec: &str) -> bool {
    match spec {
        // Debug rendering (G2), and the integer bases.
        "debug" | "b" | "x" | "X" | "o" => true,
        // Float precision: `.N`.
        _ => spec
            .strip_prefix('.')
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())),
    }
}

/// Split `expr:spec` at the colon that separates them, ignoring colons nested
/// inside brackets or a string literal. `None` means there's no spec.
pub fn split_spec(inner: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            _ if in_string => {}
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth -= 1,
            ':' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}
