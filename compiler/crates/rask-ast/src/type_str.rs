// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Reading the type strings the parser produces.
//!
//! Types in the AST are stored as rendered strings (`FnDecl::ret_ty` and
//! friends), and the parser renders `T or E` in a canonical form the source
//! never uses: `Result<T, E>`. Anything downstream that wants to know "is this
//! a result type?" has to ask about that canonical form, not about the surface
//! syntax — the lint's `try_*` rule asked whether the string contained `" or "`,
//! which no rendered result type ever does, so it rejected every correctly
//! written `try_*` in the language (#893).
//!
//! Four places were splitting `Result<…>` apart by hand before this existed.

/// Split canonical `Result<T, E>` into its two halves.
///
/// Splits at the top-level comma, so nested generics survive:
/// `Result<(), GrowError<T>>` gives `("()", "GrowError<T>")`.
pub fn result_parts(s: &str) -> Option<(&str, &str)> {
    let s = s.trim();
    let inner = s.strip_prefix("Result<")?.strip_suffix('>')?;
    let mut depth = 0i32;
    for (i, b) in inner.bytes().enumerate() {
        match b {
            b'<' | b'(' | b'[' => depth += 1,
            b'>' | b')' | b']' => depth -= 1,
            b',' if depth == 0 => {
                return Some((inner[..i].trim(), inner[i + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

/// Is this rendered type a `T or E` result?
pub fn is_result(s: &str) -> bool {
    result_parts(s).is_some()
}

/// Is this rendered type a `T?` optional?
pub fn is_optional(s: &str) -> bool {
    s.trim().ends_with('?')
}

/// Render a parsed type back the way it's written in source, for messages.
///
/// A diagnostic that prints `Result<(), TrySendError>` while telling you to
/// write `T or E` is naming a spelling Rask doesn't have.
pub fn to_source(s: &str) -> String {
    match result_parts(s) {
        Some((ok, err)) => format!("{} or {}", to_source(ok), to_source(err)),
        None if s.trim() == "()" => "void".to_string(),
        None => s.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_plain_result() {
        assert_eq!(result_parts("Result<i64, ParseError>"), Some(("i64", "ParseError")));
    }

    #[test]
    fn splits_past_nested_generics() {
        // The naive "split on the first comma" answer gets `GrowError<V` here.
        assert_eq!(
            result_parts("Result<Option<V>, GrowError<V>>"),
            Some(("Option<V>", "GrowError<V>"))
        );
        assert_eq!(result_parts("Result<(), GrowError<T>>"), Some(("()", "GrowError<T>")));
    }

    #[test]
    fn non_results_are_not_results() {
        for s in ["i64", "Option<V>", "Handle<T>?", "void", "()", "Vec<i64>"] {
            assert!(!is_result(s), "{} read as a result", s);
        }
    }

    #[test]
    fn renders_back_to_rask_syntax() {
        assert_eq!(to_source("Result<(), TrySendError>"), "void or TrySendError");
        assert_eq!(to_source("Result<T, TryReceiveError>"), "T or TryReceiveError");
        assert_eq!(to_source("i64"), "i64");
        assert_eq!(to_source("()"), "void");
    }

    #[test]
    fn optionals() {
        assert!(is_optional("Handle<T>?"));
        assert!(!is_optional("Handle<T>"));
    }
}
