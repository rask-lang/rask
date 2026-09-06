// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Reading a trait name out of a type.
//!
//! `any Trait` is trait-object syntax (`type.generics/G7`), so the trait's name
//! lands inside the type — and four passes read it back out: the checker's type
//! parser, MIR's vtable mangling, monomorphization's reachability, and the
//! trait-box cast. They have to agree on what the name is, which is the whole
//! reason this is one function rather than four `strip_prefix` calls.

/// Split `any Trait` into its trait name, or `None` if `s` isn't a
/// trait-object type.
pub fn trait_object_name(s: &str) -> Option<&str> {
    s.trim().strip_prefix("any ").map(str::trim)
}

/// Is this the short spelling of `any Error`?
///
/// `Error` on its own means the erased error box. It's the trait every error
/// type implements, and BI2 reserves the name — a program can't declare a type
/// called `Error` — so the bare word is never anything else. The docs and most
/// of the corpus write `T or Error` for what `T or any Error` spells out.
///
/// The checker and MIR each parse type strings on their own and ask this at
/// different points, so it lives here rather than as a `== "Error"` in each:
/// the checker asks after a type parameter of the same name has had its chance,
/// MIR asks alongside `any Trait` (nothing named `Error` survives
/// monomorphization). Only the checker knowing it left the error side of
/// `i64 or Error` a bare pointer in MIR, with a 16-byte fat pointer written
/// into it (#1095).
pub fn is_bare_error(s: &str) -> bool {
    s.trim() == "Error"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_trait_out_of_the_type() {
        assert_eq!(trait_object_name("any Error"), Some("Error"));
        assert_eq!(trait_object_name("any Displayable"), Some("Displayable"));
    }

    #[test]
    fn the_prefix_is_required() {
        assert_eq!(trait_object_name("Error"), None);
        assert_eq!(trait_object_name("anything"), None);
    }

    #[test]
    fn bare_error_is_the_short_spelling() {
        assert!(is_bare_error("Error"));
        assert!(is_bare_error(" Error "));
        assert!(!is_bare_error("any Error"));
        assert!(!is_bare_error("MyError"));
        assert!(!is_bare_error("Errors"));
    }
}
