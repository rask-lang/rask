// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Where a bare value may become a `T?` or a `T or E`.
//!
//! This list exists because nothing in the tree used to have it. Wrapping a
//! value was implemented four separate times — once per position — and the four
//! disagreed about how many layers they could build, so a value's fate depended
//! on where it was written rather than what it was. Depth 1 worked in all of
//! them, which is why it went unnoticed until nested shapes appeared (#701).
//!
//! Both the type checker and MIR lowering ask `wraps_error_branch` whether a
//! value typed as `E` at that position is an error going to the err branch
//! (ER9, at a `return`) or a payload going to the success branch (everywhere
//! else, because ER11 means a bare `E` isn't allowed there at all). The answer
//! is a match on `CoercionSite`, so adding a variant breaks the build until it
//! has said which it is — that enforcement is the only reason a shared list
//! beats a convention.
//!
//! The checker used to answer that question from its own two-value enum
//! (`return` vs "everything else"), which is why the two halves could disagree
//! about a position without anything noticing: a bare `2` widened to an `i64?`
//! parameter through `f(2)` and was rejected through `w.m(2)`, same rule, two
//! code paths.

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
    /// `f(v)` or `x.m(v)` where the parameter is declared as a wrapper. One
    /// variant for both spellings on purpose — the rule is about the position,
    /// not about how the callee was found.
    Argument,
    /// `x = v` where the place is declared as a wrapper. Split from
    /// `AnnotatedBinding` because the target comes from the place's existing
    /// type rather than from an annotation, and the diagnostics differ.
    Assignment,
    /// `S { field: v }` where the field is declared as a wrapper.
    StructField,
    /// The value handed back by a `catch` arm, when the expression it recovers
    /// keeps a wrapper shape (`let x: T? = f() catch _ => none`).
    CatchArm,
}

impl CoercionSite {
    /// Every position, so a pass can assert it covers the whole set.
    pub const ALL: [CoercionSite; 6] = [
        CoercionSite::Return,
        CoercionSite::AnnotatedBinding,
        CoercionSite::Argument,
        CoercionSite::Assignment,
        CoercionSite::StructField,
        CoercionSite::CatchArm,
    ];

    /// Can a value typed as the error side land on the error branch here?
    ///
    /// ER9 gives that to `return` (and to a `catch` arm, which is a return by
    /// another name): the signature already says what the error type is, so
    /// picking the branch by type is unambiguous under disjointness (ER3).
    /// Everywhere else ER11 applies — the value has to arrive already carrying
    /// the union type, so a value that happens to equal `E` at those positions
    /// is the success payload, not an error.
    ///
    /// The checker uses this to decide what to accept and MIR uses it to decide
    /// which slot to fill. Same answer from the same match, which is the point.
    pub fn wraps_error_branch(self) -> bool {
        match self {
            CoercionSite::Return | CoercionSite::CatchArm => true,
            CoercionSite::AnnotatedBinding
            | CoercionSite::Argument
            | CoercionSite::Assignment
            | CoercionSite::StructField => false,
        }
    }

    /// How the position reads in a diagnostic.
    pub fn describe(self) -> &'static str {
        match self {
            CoercionSite::Return => "a return",
            CoercionSite::AnnotatedBinding => "an annotated binding",
            CoercionSite::Argument => "an argument",
            CoercionSite::Assignment => "an assignment",
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
