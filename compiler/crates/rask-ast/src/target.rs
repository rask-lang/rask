// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The machine a compilation is for.
//!
//! Two facts about a target change what a program means: how wide a pointer
//! is (so how wide `usize` and every pointer-holding value is), and whether
//! C's `char` is signed (so what a `c_char` is). Both are stated here, once,
//! and handed down from the driver as a value, because the stages that need
//! them (the checker's C scalar names, layout, MIR, codegen, the interpreters,
//! the comptime evaluator) can't each guess.
//!
//! They used to be a process-wide global the driver set before the front end
//! ran. A global lets two compilations in one process, such as the LSP or a
//! test binary, disagree about what `usize` is, and it hid every place that
//! needed the width behind a call that took no argument.

/// What a compilation is for. Copy it freely; it is two words of facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Target {
    pointer_bits: u32,
    c_char_unsigned: bool,
}

/// What an empty placeholder (`TypeTable::default()`, a unit test's table)
/// works in. A compilation never gets this by omission: the driver builds its
/// `Target` from the configuration it was handed.
impl Default for Target {
    fn default() -> Self {
        Target::LP64
    }
}

impl Target {
    pub(crate) fn from_parts(pointer_bits: u32, c_char_unsigned: bool) -> Target {
        Target { pointer_bits, c_char_unsigned }
    }

    /// 64-bit pointers, C `char` signed. The x86-64 machines the compiler
    /// links for today, and what a tool that emits no code (the playground's
    /// interpreter, a unit test) works in.
    pub const LP64: Target = Target { pointer_bits: 64, c_char_unsigned: false };

    /// The target an architecture name stands for, spelled as a target triple
    /// and `cfg.arch` spell it.
    ///
    /// Anything unrecognised is 64-bit: every architecture the compiler can
    /// reach today is, and guessing 32 would silently narrow `usize` rather
    /// than fail loudly.
    pub fn for_arch(arch: &str) -> Target {
        let pointer_bits = match arch {
            "x86" | "i686" | "arm" | "armv7" | "wasm32" | "riscv32" | "mips" => 32,
            _ => 64,
        };
        // C's `char` is unsigned on ARM, AArch64, RISC-V and PowerPC, signed
        // on x86 and wasm.
        let c_char_unsigned = arch.starts_with("arm")
            || arch.starts_with("aarch64")
            || arch.starts_with("riscv")
            || arch.starts_with("powerpc");
        Target { pointer_bits, c_char_unsigned }
    }

    /// P2: how wide a pointer is, in bits. `isize` and `usize` are this wide.
    pub fn pointer_bits(&self) -> u32 {
        self.pointer_bits
    }

    /// Bytes in a pointer, which is also its alignment.
    pub fn word(&self) -> u32 {
        self.pointer_bits / 8
    }

    /// Whether C's `char` is unsigned here.
    pub fn c_char_unsigned(&self) -> bool {
        self.c_char_unsigned
    }

    /// The fixed-width spelling `usize` stands for.
    pub fn usize_spelling(&self) -> &'static str {
        if self.pointer_bits == 32 { "u32" } else { "u64" }
    }

    /// The fixed-width spelling `isize` stands for.
    pub fn isize_spelling(&self) -> &'static str {
        if self.pointer_bits == 32 { "i32" } else { "i64" }
    }

    /// What one of the C scalar names (`c_int`, `c_size`, …) is on this
    /// target, as a primitive spelling. `None` for any other name.
    ///
    /// `import c` translates a header's `int` to `c_int`, `size_t` to `c_size`
    /// and so on (struct.c-interop/TM1), and until these were types none of
    /// those names was one at all. So `*i64` passed for a `const int *` with no
    /// complaint, and the C side read the 64-bit buffer as 32-bit ints: the
    /// airtight case is one element of 2^32, which sums to 0 (#947).
    ///
    /// The widths follow the target's data model. LP64 everywhere the compiler
    /// can currently link, so `long` is 64-bit; the Windows LLP64 case is a
    /// target that doesn't reach codegen yet.
    pub fn c_type_spelling(&self, name: &str) -> Option<&'static str> {
        let long = if self.pointer_bits == 32 { "i32" } else { "i64" };
        let ulong = if self.pointer_bits == 32 { "u32" } else { "u64" };
        Some(match name {
            "c_char" => {
                if self.c_char_unsigned { "u8" } else { "i8" }
            }
            "c_short" => "i16",
            "c_ushort" => "u16",
            "c_int" => "i32",
            "c_uint" => "u32",
            "c_long" => long,
            "c_ulong" => ulong,
            "c_longlong" => "i64",
            "c_ulonglong" => "u64",
            "c_size" => self.usize_spelling(),
            "c_ssize" => self.isize_spelling(),
            _ => return None,
        })
    }
}

/// Every `c_*` name, for the tables that enumerate primitives.
pub const C_SCALARS: &[&str] = &[
    "c_char", "c_short", "c_ushort", "c_int", "c_uint",
    "c_long", "c_ulong", "c_longlong", "c_ulonglong", "c_size", "c_ssize",
];

/// Whether `name` is one of the C scalar names. Target-independent: what it
/// stands for is `Target::c_type_spelling`'s to say.
pub fn is_c_scalar(name: &str) -> bool {
    C_SCALARS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P2: `usize`/`isize` are pointer-sized, so their width comes from the
    /// target rather than from a literal 64 written into a dozen match arms
    /// (#651).
    #[test]
    fn usize_follows_the_pointer_width() {
        let narrow = Target::for_arch("arm");
        assert_eq!((narrow.pointer_bits(), narrow.word()), (32, 4));
        assert_eq!((narrow.usize_spelling(), narrow.isize_spelling()), ("u32", "i32"));
        let wide = Target::for_arch("x86_64");
        assert_eq!((wide.pointer_bits(), wide.word()), (64, 8));
        assert_eq!((wide.usize_spelling(), wide.isize_spelling()), ("u64", "i64"));
        assert_eq!(Target::for_arch("something-new"), Target::LP64);
    }

    #[test]
    fn c_scalars_follow_the_target() {
        let t32 = Target::for_arch("arm");
        assert_eq!(t32.c_type_spelling("c_long"), Some("i32"));
        assert_eq!(t32.c_type_spelling("c_size"), Some("u32"));
        assert_eq!(t32.c_type_spelling("c_char"), Some("u8"));
        let x86 = Target::for_arch("x86_64");
        assert_eq!(x86.c_type_spelling("c_long"), Some("i64"));
        assert_eq!(x86.c_type_spelling("c_char"), Some("i8"));
        assert_eq!(x86.c_type_spelling("i64"), None);
        for name in C_SCALARS {
            assert!(x86.c_type_spelling(name).is_some(), "{name} has no spelling");
        }
    }
}
