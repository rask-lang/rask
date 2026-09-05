// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Passing a C struct by value.
//!
//! A Rask aggregate travels as a pointer everywhere else in the compiler, and
//! C doesn't work that way: `int mylib_area(Rect r)` reads its argument out of
//! registers, so handing it an address gives it whatever that address happens
//! to be. Every C API with a geometry, colour or vector parameter is that shape
//! (#948).
//!
//! The System V AMD64 rule, for the part that matters here: a struct of 16
//! bytes or less is cut into eight-byte pieces, and each piece goes in an SSE
//! register if everything in it is a float and an integer register otherwise.
//! Anything larger is copied onto the stack, which is what Cranelift's
//! `StructArgument` does.
//!
//! Only the argument side. A C function *returning* a struct is a separate
//! rule (a hidden pointer for the large case) and isn't built yet — the call
//! is rejected in the checker instead (#1101).

use cranelift_codegen::ir::{types, Type};
use rask_mono::StructLayout;

/// How one argument of a C function is passed.
#[derive(Debug, Clone, PartialEq)]
pub enum CArg {
    /// A scalar — one value, the declared type.
    Scalar(Type),
    /// An aggregate in registers: one value per eight-byte piece, `F64` for a
    /// piece that holds only floats and `I64` for anything else.
    Pieces(Vec<Type>),
    /// An aggregate too big for registers. The caller copies it onto the stack;
    /// the value handed to Cranelift is its address.
    Memory(u32),
}

impl CArg {
    /// How many Cranelift parameters this argument occupies.
    pub fn slots(&self) -> usize {
        match self {
            CArg::Scalar(_) => 1,
            CArg::Pieces(tys) => tys.len(),
            CArg::Memory(_) => 1,
        }
    }
}

/// One field, as the classification cares about it.
#[derive(Debug, Clone, Copy)]
pub struct AbiField {
    pub offset: u32,
    pub size: u32,
    pub is_float: bool,
}

/// The largest struct that still travels in registers, per System V.
const MAX_REGISTER_BYTES: u32 = 16;

/// Classify one C parameter, given the struct layouts of the program.
///
/// `scalar` is the type the parameter would have if it weren't an aggregate —
/// the caller has already mapped the type string, so this only has to decide
/// whether a struct layout by that name overrides it.
pub fn classify(type_name: &str, scalar: Type, layouts: &[StructLayout]) -> CArg {
    match find_layout(type_name, layouts) {
        Some(layout) => classify_aggregate(layout.size, &abi_fields(layout)),
        None => CArg::Scalar(scalar),
    }
}

fn find_layout<'a>(name: &str, layouts: &'a [StructLayout]) -> Option<&'a StructLayout> {
    let name = name.trim();
    if name.starts_with('*') {
        return None;
    }
    layouts.iter().find(|l| l.name == name)
}

fn abi_fields(layout: &StructLayout) -> Vec<AbiField> {
    layout
        .fields
        .iter()
        .map(|f| AbiField {
            offset: f.offset,
            size: f.size,
            is_float: is_float(&f.ty),
        })
        .collect()
}

/// The System V classification for an aggregate of `size` bytes.
pub fn classify_aggregate(size: u32, fields: &[AbiField]) -> CArg {
    if size > MAX_REGISTER_BYTES {
        // Cranelift asserts a stack argument's size is a multiple of 8, and C
        // pads a struct to its alignment anyway.
        return CArg::Memory(size.div_ceil(8) * 8);
    }
    let pieces = size.div_ceil(8).max(1);
    let tys = (0..pieces)
        .map(|i| {
            if piece_is_all_float(fields, i * 8) {
                types::F64
            } else {
                types::I64
            }
        })
        .collect();
    CArg::Pieces(tys)
}

/// Whether every field touching the eight bytes at `start` is a float.
///
/// That is the whole SSE test: a piece with one integer anywhere in it goes in
/// an integer register, however many floats sit beside it.
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

    #[test]
    fn two_ints_share_one_integer_piece() {
        let fields = [f(0, 4, false), f(4, 4, false)];
        assert_eq!(classify_aggregate(8, &fields), CArg::Pieces(vec![types::I64]));
    }

    #[test]
    fn two_floats_share_one_sse_piece() {
        let fields = [f(0, 4, true), f(4, 4, true)];
        assert_eq!(classify_aggregate(8, &fields), CArg::Pieces(vec![types::F64]));
    }

    #[test]
    fn an_int_beside_a_float_makes_the_piece_integer() {
        let fields = [f(0, 4, false), f(4, 4, true)];
        assert_eq!(classify_aggregate(8, &fields), CArg::Pieces(vec![types::I64]));
    }

    #[test]
    fn sixteen_bytes_are_two_pieces_classified_apart() {
        let fields = [f(0, 8, true), f(8, 8, false)];
        assert_eq!(
            classify_aggregate(16, &fields),
            CArg::Pieces(vec![types::F64, types::I64])
        );
    }

    #[test]
    fn anything_larger_goes_on_the_stack() {
        let fields = [f(0, 8, false), f(8, 8, false), f(16, 8, false)];
        assert_eq!(classify_aggregate(24, &fields), CArg::Memory(24));
    }

    #[test]
    fn a_stack_argument_rounds_up_to_a_whole_number_of_words() {
        let fields = [f(0, 8, false), f(8, 8, false), f(16, 4, false)];
        assert_eq!(classify_aggregate(20, &fields), CArg::Memory(24));
    }

    #[test]
    fn a_scalar_parameter_is_left_alone() {
        assert_eq!(classify("i32", types::I32, &[]), CArg::Scalar(types::I32));
    }

    #[test]
    fn a_pointer_to_a_struct_is_still_a_pointer() {
        assert_eq!(classify("*Rect", types::I64, &[]), CArg::Scalar(types::I64));
    }
}
