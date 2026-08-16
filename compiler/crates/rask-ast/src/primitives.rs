// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Primitive type spellings, in one place.
//!
//! Every crate needs to ask "is this name a primitive", and each one used to
//! carry its own literal list. They disagreed: the resolver's set had no
//! `string` and no `int`/`uint`, the interpreter's had both, and nothing said
//! which was right.
//!
//! The sets differ for real reasons, so they're named instead of merged — a
//! caller picks the question it means and the spellings are written once.
//! Name→type conversions (`"i8"` → `MirType::I8` and friends) stay with the type
//! they produce; only the spelling sets live here.

/// P1/P2: signed integers that fit a machine register.
pub const SIGNED_INTS: &[&str] = &["i8", "i16", "i32", "i64", "isize"];

/// P1/P2: unsigned integers that fit a machine register.
pub const UNSIGNED_INTS: &[&str] = &["u8", "u16", "u32", "u64", "usize"];

/// The 128-bit integers, kept apart because they behave differently nearly
/// everywhere: their own runtime representation (`Int128`/`Uint128`), their own
/// method sets, and `string.parse<T>` doesn't take them.
pub const WIDE_INTS: &[&str] = &["i128", "u128"];

/// P3: IEEE 754 floats.
pub const FLOATS: &[&str] = &["f32", "f64"];

/// P4/P5: the non-numeric scalars.
pub const BOOL_AND_CHAR: &[&str] = &["bool", "char"];

/// Pre-spec spellings the interpreter still accepts: `int` is `i32`, `uint` is
/// `u64`. Not in `specs/types/primitives.md`, so anything enforcing the spec
/// leaves them out.
pub const INT_ALIASES: &[&str] = &["int", "uint"];

/// An integer that fits a register — no 128-bit, no aliases.
pub fn is_machine_integer(name: &str) -> bool {
    SIGNED_INTS.contains(&name) || UNSIGNED_INTS.contains(&name)
}

/// P2: how wide a pointer is on the target, in bits.
///
/// `isize`/`usize` are pointer-sized, so this is the one place that decides how
/// wide they are. Every crate used to answer it for itself, by putting `usize`
/// in the same match arm as `u64` — a dozen independent assertions that a
/// pointer is eight bytes, none of them connected to a target and none of them
/// stating the rule. They all route through here now.
///
/// The width is the compiler host's, and today that is exactly right: the only
/// binaries the compiler produces are for the machine it runs on. Cross-
/// compiling to another OS, to wasm32, or to bare metal is refused at link
/// time, so no reachable target's pointer width can differ from the host's.
/// When a target triple reaches the frontend — it currently stops at codegen —
/// this takes it as a parameter and every caller follows unchanged.
pub fn pointer_bits() -> u32 {
    (std::mem::size_of::<usize>() * 8) as u32
}

/// The fixed-width spelling `usize` stands for on this target.
pub fn usize_spelling() -> &'static str {
    if pointer_bits() == 32 { "u32" } else { "u64" }
}

/// The fixed-width spelling `isize` stands for on this target.
pub fn isize_spelling() -> &'static str {
    if pointer_bits() == 32 { "i32" } else { "i64" }
}

/// Any integer spelling, 128-bit and aliases included.
pub fn is_integer(name: &str) -> bool {
    is_machine_integer(name) || WIDE_INTS.contains(&name) || INT_ALIASES.contains(&name)
}

pub fn is_float(name: &str) -> bool {
    FLOATS.contains(&name)
}

/// The spec's primitive set: integers, floats, `bool`, `char`. No `string` —
/// it's a builtin type with a heap buffer, not a scalar — and no aliases.
pub fn is_scalar(name: &str) -> bool {
    is_machine_integer(name)
        || WIDE_INTS.contains(&name)
        || FLOATS.contains(&name)
        || BOOL_AND_CHAR.contains(&name)
}

/// Every spelling that names a primitive-ish builtin, including `string` and the
/// legacy aliases. For code asking "do I recognize this type name at all".
pub fn is_builtin_scalar_or_string(name: &str) -> bool {
    is_scalar(name) || INT_ALIASES.contains(&name) || name == "string"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_set_excludes_string_and_aliases() {
        assert!(is_scalar("i64"));
        assert!(is_scalar("usize"));
        assert!(is_scalar("f32"));
        assert!(is_scalar("char"));
        assert!(!is_scalar("string"));
        assert!(!is_scalar("int"));
        assert!(!is_scalar("Vec"));
    }

    #[test]
    fn the_wide_set_includes_them() {
        assert!(is_builtin_scalar_or_string("string"));
        assert!(is_builtin_scalar_or_string("int"));
        assert!(is_builtin_scalar_or_string("uint"));
        assert!(is_builtin_scalar_or_string("i8"));
        assert!(!is_builtin_scalar_or_string("Vec"));
    }

    #[test]
    fn integers_cover_both_signs_and_the_aliases() {
        for n in SIGNED_INTS.iter().chain(UNSIGNED_INTS.iter()) {
            assert!(is_integer(n), "{} should be an integer", n);
        }
        assert!(is_integer("i128"));
        assert!(is_integer("int"));
        assert!(is_integer("uint"));
        assert!(!is_integer("f64"));
        assert!(!is_integer("bool"));
    }

    #[test]
    fn machine_integers_exclude_the_wide_ones_and_the_aliases() {
        assert!(is_machine_integer("i64"));
        assert!(is_machine_integer("usize"));
        assert!(!is_machine_integer("i128"));
        assert!(!is_machine_integer("u128"));
        assert!(!is_machine_integer("int"));
    }

    /// P2: `usize`/`isize` are pointer-sized, so their width has to come from
    /// the pointer width rather than from a literal 64 written into a dozen
    /// match arms (#651). This is the rule stated once; the rest of the
    /// compiler asks `pointer_bits` instead of assuming.
    #[test]
    fn usize_follows_the_pointer_width() {
        assert_eq!(pointer_bits(), (std::mem::size_of::<*const u8>() * 8) as u32);
        assert!(matches!(pointer_bits(), 32 | 64), "unexpected pointer width");
        match pointer_bits() {
            32 => {
                assert_eq!(usize_spelling(), "u32");
                assert_eq!(isize_spelling(), "i32");
            }
            _ => {
                assert_eq!(usize_spelling(), "u64");
                assert_eq!(isize_spelling(), "i64");
            }
        }
    }

    /// The sets are meant to be disjoint; an overlap would make "which question
    /// am I asking" ambiguous again.
    #[test]
    fn the_groups_do_not_overlap() {
        let groups = [SIGNED_INTS, UNSIGNED_INTS, WIDE_INTS, FLOATS, BOOL_AND_CHAR, INT_ALIASES];
        for (i, a) in groups.iter().enumerate() {
            for b in groups.iter().skip(i + 1) {
                for name in a.iter() {
                    assert!(!b.contains(name), "{} is in two groups", name);
                }
            }
        }
    }
}
