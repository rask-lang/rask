// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Trait names that have more than one spelling.
//!
//! There is currently one: the error trait. The spec's rules call it
//! `ErrorMessage` — that's the name you write in `extend E with ErrorMessage`
//! — but every code sample in the spec writes the boxed form as `any Error`,
//! and that's the spelling people reach for. ER32's own wording puts both in
//! one sentence: "`try` auto-boxes when the current function's error type is
//! `any Error` — any `E` satisfying `ErrorMessage` widens by boxing".
//!
//! Only `ErrorMessage` was ever registered, so `any Error` resolved to a trait
//! nobody had heard of and every rule that keys off the trait name quietly
//! failed: the type didn't satisfy ER4's own bound, `try` wouldn't box into it,
//! and `.message()` on the box wasn't found (#708).
//!
//! Rather than register a second trait with the same one method — two names for
//! one thing, and every check would have to remember both — the spellings are
//! folded to one here. Anywhere a trait name is read out of source text,
//! `canonical_trait_name` runs first, so the checker, MIR's vtable mangling and
//! monomorphization all agree on which trait is meant.

/// The registered name for a trait as written in source.
///
/// Returns the input unchanged for every trait with a single spelling.
pub fn canonical_trait_name(written: &str) -> &str {
    match written.trim() {
        "Error" => "ErrorMessage",
        other => other,
    }
}

/// Split `any Trait` into its canonical trait name, or `None` if `s` isn't a
/// trait-object type.
///
/// Every reader of an `any …` type string goes through this rather than its own
/// `strip_prefix`, so none of them can be looking at a different trait than the
/// others.
pub fn trait_object_name(s: &str) -> Option<&str> {
    s.trim().strip_prefix("any ").map(canonical_trait_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_folds_to_error_message() {
        assert_eq!(canonical_trait_name("Error"), "ErrorMessage");
        assert_eq!(canonical_trait_name("ErrorMessage"), "ErrorMessage");
    }

    #[test]
    fn other_traits_are_untouched() {
        for name in ["Displayable", "Hashable", "Iterator", "Reader"] {
            assert_eq!(canonical_trait_name(name), name);
        }
    }

    #[test]
    fn trait_object_prefix_is_required() {
        assert_eq!(trait_object_name("any Error"), Some("ErrorMessage"));
        assert_eq!(trait_object_name("any Displayable"), Some("Displayable"));
        assert_eq!(trait_object_name("Error"), None);
        assert_eq!(trait_object_name("anything"), None);
    }
}
