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
}
