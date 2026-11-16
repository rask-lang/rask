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

/// An integer that fits a register — no 128-bit.
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
/// It is the *target's* width, and `set_pointer_bits` is how the target says
/// so. It used to read the compiler host's instead, which is the same number
/// for every binary the compiler emits — cross-compiling is refused at link
/// time — but not for every frontend. The browser playground is a wasm32 build
/// of the compiler that emits no code at all: it interprets, on 64-bit values.
/// Reading the host there made `usize` 32 bits, so `stdlib/fs.rk`'s `n as
/// usize` was a narrowing conversion and the playground rejected its own
/// standard library.
///
/// Until someone sets it, the host's width — which is what a `rask` binary
/// compiling for the machine it runs on wants, and what a unit test that never
/// builds a `CfgConfig` should see.
pub fn pointer_bits() -> u32 {
    match POINTER_BITS.load(std::sync::atomic::Ordering::Relaxed) {
        0 => (std::mem::size_of::<usize>() * 8) as u32,
        bits => bits,
    }
}

/// Declare the target's pointer width, before the frontend runs.
///
/// One compilation is one target, so this is set once per process — from the
/// `CfgConfig` the frontend was handed, so the width and the `cfg` values can't
/// describe two different machines.
pub fn set_pointer_bits(bits: u32) {
    debug_assert!(bits == 32 || bits == 64, "pointer width is 32 or 64, got {bits}");
    POINTER_BITS.store(bits, std::sync::atomic::Ordering::Relaxed);
}

/// 0 means "nobody said", which reads as the host.
static POINTER_BITS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Whether C's `char` is unsigned on the target. Set beside the pointer width,
/// for the same reason; `None` until then, which reads as the host.
pub fn set_c_char_unsigned(unsigned: bool) {
    C_CHAR.store(if unsigned { 2 } else { 1 }, std::sync::atomic::Ordering::Relaxed);
}

pub fn c_char_unsigned() -> bool {
    match C_CHAR.load(std::sync::atomic::Ordering::Relaxed) {
        0 => cfg!(any(target_arch = "aarch64", target_arch = "arm")),
        n => n == 2,
    }
}

/// 0 unset, 1 signed, 2 unsigned.
static C_CHAR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// The target the process-wide facts above describe.
pub fn process_target() -> crate::target::Target {
    crate::target::Target::from_parts(pointer_bits(), c_char_unsigned())
}

pub fn usize_spelling() -> &'static str {
    process_target().usize_spelling()
}

pub fn isize_spelling() -> &'static str {
    process_target().isize_spelling()
}

pub fn c_type_spelling(name: &str) -> Option<&'static str> {
    process_target().c_type_spelling(name)
}

/// Any integer spelling, 128-bit included.
pub fn is_integer(name: &str) -> bool {
    is_machine_integer(name) || WIDE_INTS.contains(&name)
}

pub fn is_float(name: &str) -> bool {
    FLOATS.contains(&name)
}

/// The spec's primitive set: integers, floats, `bool`, `char`. No `string` —
/// it's a builtin type with a heap buffer, not a scalar.
pub fn is_scalar(name: &str) -> bool {
    is_machine_integer(name)
        || WIDE_INTS.contains(&name)
        || FLOATS.contains(&name)
        || BOOL_AND_CHAR.contains(&name)
}

/// The spellings whose value is a string's 16 bytes: `string`, and `bytes`,
/// which is the same value without the UTF-8 rule (std.bytes/BY3). A native
/// that returns one fills an out-parameter.
pub fn is_string_valued(name: &str) -> bool {
    name == "string" || name == "bytes"
}

/// Every spelling that names a primitive-ish builtin, `string` and `bytes`
/// included. For code asking "do I recognize this type name at all".
pub fn is_builtin_value_type(name: &str) -> bool {
    is_scalar(name) || name == "string" || name == "bytes"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spec_set_excludes_string() {
        assert!(is_scalar("i64"));
        assert!(is_scalar("usize"));
        assert!(is_scalar("f32"));
        assert!(is_scalar("char"));
        assert!(!is_scalar("string"));
        assert!(!is_scalar("int"));
        assert!(!is_scalar("Vec"));
    }

    #[test]
    fn the_wide_set_includes_string() {
        assert!(is_builtin_value_type("string"));
        assert!(is_builtin_value_type("bytes"));
        assert!(!is_builtin_value_type("int"));
        assert!(is_builtin_value_type("i8"));
        assert!(!is_builtin_value_type("Vec"));
    }

    #[test]
    fn integers_cover_both_signs() {
        for n in SIGNED_INTS.iter().chain(UNSIGNED_INTS.iter()) {
            assert!(is_integer(n), "{} should be an integer", n);
        }
        assert!(is_integer("i128"));
        assert!(!is_integer("int"));
        assert!(!is_integer("f64"));
        assert!(!is_integer("bool"));
    }

    #[test]
    fn machine_integers_exclude_the_wide_ones() {
        assert!(is_machine_integer("i64"));
        assert!(is_machine_integer("usize"));
        assert!(!is_machine_integer("i128"));
        assert!(!is_machine_integer("u128"));
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
        let groups = [SIGNED_INTS, UNSIGNED_INTS, WIDE_INTS, FLOATS, BOOL_AND_CHAR];
        for (i, a) in groups.iter().enumerate() {
            for b in groups.iter().skip(i + 1) {
                for name in a.iter() {
                    assert!(!b.contains(name), "{} is in two groups", name);
                }
            }
        }
    }
}
