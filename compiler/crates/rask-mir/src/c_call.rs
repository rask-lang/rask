// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! How a C struct argument crosses a call, per calling convention.
//!
//! A Rask aggregate travels as a pointer, and C doesn't take one: `int
//! area(Rect r)` reads its argument out of registers, or out of a copy, by
//! rules each ABI writes differently. No backend decides this for its
//! frontend (clang classifies for LLVM), so it lives here, in terms of the
//! layout, and a backend only maps the answer onto its own types.
//!
//! Only the argument side. A C function returning a struct is rejected in the
//! checker (#1101).

use rask_types::layout::StructLayout;

/// The calling conventions a struct argument is classified for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallConv {
    /// x86-64 Linux, macOS, BSD.
    SysV64,
    /// AArch64 Linux and macOS (AAPCS64; Apple's differs in ways that don't
    /// touch struct arguments).
    Aapcs64,
    /// x86-64 Windows.
    Win64,
    /// Anything else. A struct argument is refused rather than guessed.
    Unclassified,
}

impl CallConv {
    /// The convention for a target, from a triple's architecture and OS as
    /// `target_lexicon` spells them.
    pub fn for_target(arch: &str, os: &str) -> Self {
        match (arch, os) {
            ("x86_64", "windows") => CallConv::Win64,
            ("x86_64", _) => CallConv::SysV64,
            ("aarch64", _) => CallConv::Aapcs64,
            _ => CallConv::Unclassified,
        }
    }
}

/// One register's worth of a struct argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Piece {
    /// Where in the struct it is read from.
    pub offset: u32,
    pub kind: PieceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PieceKind {
    /// Eight bytes in an integer register.
    Int,
    /// Eight bytes in a vector register, as one double's worth: System V
    /// packs two floats into one.
    Vector,
    /// One `f32` in a float register of its own.
    F32,
    /// One `f64` in a float register of its own.
    F64,
}

/// How one argument is passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CArg {
    /// Not a struct: passed as its own type.
    Scalar,
    /// In registers, one per piece.
    Registers(Vec<Piece>),
    /// Copied onto the stack by the call itself (System V's large structs).
    /// The size is rounded up to whole words.
    StackCopy(u32),
    /// The caller makes a copy of this many bytes and passes its address
    /// (AArch64 and Win64 large structs). A copy, because the callee may
    /// write to its parameter.
    ByAddress(u32),
    /// A struct on a convention nothing here classifies.
    Unsupported,
}

/// One field, as classification sees it.
#[derive(Debug, Clone, Copy)]
pub struct AbiField {
    pub offset: u32,
    pub size: u32,
    pub is_float: bool,
}

/// Classify a parameter of declared type `ty`, given the program's struct
/// layouts. Anything that names no struct is a scalar.
pub fn classify(conv: CallConv, ty: &rask_ast::ty::TypeExpr, layouts: &[StructLayout]) -> CArg {
    let Some(layout) = ty.name().and_then(|n| layouts.iter().find(|l| l.name == n)) else {
        return CArg::Scalar;
    };
    let fields: Vec<AbiField> = layout
        .fields
        .iter()
        .map(|f| AbiField { offset: f.offset, size: f.size, is_float: is_float(&f.ty) })
        .collect();
    classify_aggregate(conv, layout.size, &fields)
}

/// Classify a struct of `size` bytes with these fields.
pub fn classify_aggregate(conv: CallConv, size: u32, fields: &[AbiField]) -> CArg {
    match conv {
        CallConv::SysV64 => sysv(size, fields),
        CallConv::Aapcs64 => aapcs64(size, fields),
        CallConv::Win64 => win64(size),
        CallConv::Unclassified => CArg::Unsupported,
    }
}

/// System V: sixteen bytes or less is cut into eight-byte pieces, each in a
/// vector register if everything in it is a float and an integer register
/// otherwise. Anything larger is copied onto the stack.
fn sysv(size: u32, fields: &[AbiField]) -> CArg {
    if size > 16 {
        return CArg::StackCopy(size.div_ceil(8) * 8);
    }
    let pieces = (0..size.div_ceil(8).max(1))
        .map(|i| {
            let offset = i * 8;
            let kind = if piece_is_all_float(fields, offset) { PieceKind::Vector } else { PieceKind::Int };
            Piece { offset, kind }
        })
        .collect();
    CArg::Registers(pieces)
}

/// AAPCS64: one to four floats of one type (a homogeneous floating-point
/// aggregate) go one per float register. Otherwise sixteen bytes or less go
/// in integer registers, eight bytes each, whatever is in them, and anything
/// larger is passed by the address of a copy.
fn aapcs64(size: u32, fields: &[AbiField]) -> CArg {
    let all_float = !fields.is_empty() && fields.iter().all(|f| f.is_float);
    let one_width = fields.windows(2).all(|w| w[0].size == w[1].size);
    if all_float && one_width && fields.len() <= 4 {
        let kind = if fields[0].size == 4 { PieceKind::F32 } else { PieceKind::F64 };
        return CArg::Registers(fields.iter().map(|f| Piece { offset: f.offset, kind }).collect());
    }
    if size > 16 {
        return CArg::ByAddress(size);
    }
    CArg::Registers((0..size.div_ceil(8).max(1)).map(|i| Piece { offset: i * 8, kind: PieceKind::Int }).collect())
}

/// Win64: a struct of exactly 1, 2, 4 or 8 bytes goes in one integer
/// register, floats or not. Every other size is passed by the address of a
/// copy.
fn win64(size: u32) -> CArg {
    match size {
        1 | 2 | 4 | 8 => CArg::Registers(vec![Piece { offset: 0, kind: PieceKind::Int }]),
        _ => CArg::ByAddress(size),
    }
}

/// Whether every field touching the eight bytes at `start` is a float.
///
/// That is the whole System V vector test: a piece with one integer anywhere
/// in it goes in an integer register, however many floats sit beside it.
fn piece_is_all_float(fields: &[AbiField], start: u32) -> bool {
    let end = start + 8;
    let mut touched = false;
    for f in fields {
        if f.offset >= end || f.offset + f.size <= start {
            continue;
        }
        touched = true;
        if !f.is_float {
            return false;
        }
    }
    touched
}

fn is_float(ty: &rask_types::Type) -> bool {
    use rask_types::Type as T;
    match ty {
        T::F32 | T::F64 => true,
        T::UnresolvedNamed(n) => {
            let n = rask_ast::primitives::c_type_spelling(n).unwrap_or(n.as_str());
            matches!(n, "f32" | "f64")
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(offset: u32, size: u32, is_float: bool) -> AbiField {
        AbiField { offset, size, is_float }
    }

    fn regs(pieces: &[(u32, PieceKind)]) -> CArg {
        CArg::Registers(pieces.iter().map(|&(offset, kind)| Piece { offset, kind }).collect())
    }

    use PieceKind::*;

    // The fixture shapes of c_struct.h, per convention.
    const RECT: (u32, [AbiField; 2]) = (8, [AbiField { offset: 0, size: 4, is_float: false }, AbiField { offset: 4, size: 4, is_float: false }]);

    #[test]
    fn sysv_two_ints_share_one_integer_piece() {
        assert_eq!(classify_aggregate(CallConv::SysV64, RECT.0, &RECT.1), regs(&[(0, Int)]));
    }

    #[test]
    fn sysv_packs_two_floats_into_one_vector_piece() {
        let v = [f(0, 4, true), f(4, 4, true)];
        assert_eq!(classify_aggregate(CallConv::SysV64, 8, &v), regs(&[(0, Vector)]));
    }

    #[test]
    fn sysv_an_int_beside_a_float_makes_the_piece_integer() {
        let v = [f(0, 4, false), f(4, 4, true)];
        assert_eq!(classify_aggregate(CallConv::SysV64, 8, &v), regs(&[(0, Int)]));
    }

    #[test]
    fn sysv_sixteen_bytes_are_two_pieces_classified_apart() {
        let stamp = [f(0, 8, true), f(8, 8, false)];
        assert_eq!(classify_aggregate(CallConv::SysV64, 16, &stamp), regs(&[(0, Vector), (8, Int)]));
    }

    #[test]
    fn sysv_copies_anything_larger_onto_the_stack_in_whole_words() {
        let fields = [f(0, 8, false), f(8, 8, false), f(16, 4, false)];
        assert_eq!(classify_aggregate(CallConv::SysV64, 20, &fields), CArg::StackCopy(24));
    }

    #[test]
    fn aapcs64_gives_each_float_of_a_float_struct_its_own_register() {
        let v = [f(0, 4, true), f(4, 4, true)];
        assert_eq!(classify_aggregate(CallConv::Aapcs64, 8, &v), regs(&[(0, F32), (4, F32)]));
        let four = [f(0, 8, true), f(8, 8, true), f(16, 8, true), f(24, 8, true)];
        assert_eq!(
            classify_aggregate(CallConv::Aapcs64, 32, &four),
            regs(&[(0, F64), (8, F64), (16, F64), (24, F64)])
        );
    }

    #[test]
    fn aapcs64_puts_a_mixed_struct_in_integer_registers() {
        let stamp = [f(0, 8, true), f(8, 8, false)];
        assert_eq!(classify_aggregate(CallConv::Aapcs64, 16, &stamp), regs(&[(0, Int), (8, Int)]));
        // Floats of two widths aren't homogeneous.
        let mixed = [f(0, 4, true), f(8, 8, true)];
        assert_eq!(classify_aggregate(CallConv::Aapcs64, 16, &mixed), regs(&[(0, Int), (8, Int)]));
    }

    #[test]
    fn aapcs64_passes_a_large_struct_by_the_address_of_a_copy() {
        let triple = [f(0, 8, false), f(8, 8, false), f(16, 8, false)];
        assert_eq!(classify_aggregate(CallConv::Aapcs64, 24, &triple), CArg::ByAddress(24));
    }

    #[test]
    fn win64_registers_only_power_of_two_sizes_up_to_eight() {
        assert_eq!(classify_aggregate(CallConv::Win64, 8, &[f(0, 4, true), f(4, 4, true)]), regs(&[(0, Int)]));
        assert_eq!(classify_aggregate(CallConv::Win64, 4, &[f(0, 4, false)]), regs(&[(0, Int)]));
        assert_eq!(classify_aggregate(CallConv::Win64, 16, &[f(0, 8, true), f(8, 8, true)]), CArg::ByAddress(16));
        assert_eq!(classify_aggregate(CallConv::Win64, 12, &[f(0, 4, false), f(4, 4, false), f(8, 4, false)]), CArg::ByAddress(12));
    }

    #[test]
    fn a_scalar_parameter_is_left_alone() {
        assert_eq!(classify(CallConv::SysV64, &rask_ast::ty::TypeExpr::named("i32"), &[]), CArg::Scalar);
    }

    #[test]
    fn a_pointer_to_a_struct_is_still_a_pointer() {
        let ptr = rask_ast::ty::TypeExpr::RawPtr(Box::new(rask_ast::ty::TypeExpr::named("Rect")));
        assert_eq!(classify(CallConv::Aapcs64, &ptr, &[]), CArg::Scalar);
    }

    #[test]
    fn an_unclassified_convention_refuses_a_struct() {
        assert_eq!(classify_aggregate(CallConv::Unclassified, RECT.0, &RECT.1), CArg::Unsupported);
    }
}
