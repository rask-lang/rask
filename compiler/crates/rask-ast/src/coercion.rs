// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Where a bare value may become a `T?` or a `T or E`.
//!
//! This list exists because nothing in the tree used to have it. Wrapping a
//! value was implemented four separate times — once per position — and the four
//! disagreed about how many layers they could build, so a value's fate depended
//! on where it was written rather than what it was. Depth 1 worked in all of
//! them, which is why it went unnoticed until nested shapes appeared (#701).
//!
//! MIR lowering matches on `CoercionSite` exhaustively to decide whether a value
//! typed as `E` at that position is an error going to the err branch (ER9, at a
//! `return`) or a payload going to the success branch (everywhere else, because
//! ER11 means the checker already rejected a bare `E` there). Adding a variant
//! breaks that match until it has said which it is — that enforcement is the
//! only reason a shared list beats a convention.

/// A position where the language coerces a bare `T` into a wrapper shape.
///
/// "Coerces" means the source says `T` and the target says `T?` / `T or E` /
/// any nesting of those, and the value acquires the missing layers. Positions
/// where both sides already agree aren't coercions and aren't listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoercionSite {
    /// `return v` in a function whose declared return type is a wrapper.
    Return,
    /// `let x: T? = v` — the annotation is the target, not the initializer.
    AnnotatedBinding,
    /// `f(v)` where the parameter is declared as a wrapper.
    Argument,
    /// `S { field: v }` where the field is declared as a wrapper.
    StructField,
    /// The value handed back by a `catch` arm, when the expression it recovers
    /// keeps a wrapper shape (`let x: T? = f() catch _ => none`).
    CatchArm,
}

impl CoercionSite {
    /// Every position, so a pass can assert it covers the whole set.
    pub const ALL: [CoercionSite; 5] = [
        CoercionSite::Return,
        CoercionSite::AnnotatedBinding,
        CoercionSite::Argument,
        CoercionSite::StructField,
        CoercionSite::CatchArm,
    ];

    /// How the position reads in a diagnostic.
    pub fn describe(self) -> &'static str {
        match self {
            CoercionSite::Return => "a return",
            CoercionSite::AnnotatedBinding => "an annotated binding",
            CoercionSite::Argument => "an argument",
            CoercionSite::StructField => "a struct field",
            CoercionSite::CatchArm => "a catch arm",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_lists_every_variant() {
        // A variant added without extending ALL would slip past the passes that
        // iterate it to check their own coverage.
        for site in CoercionSite::ALL {
            assert!(!site.describe().is_empty());
        }
        let mut seen = std::collections::HashSet::new();
        for site in CoercionSite::ALL {
            assert!(seen.insert(site), "duplicate in ALL: {site:?}");
        }
    }
}
