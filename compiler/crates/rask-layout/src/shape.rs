// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! How a type's pieces compose into a size, for any type that is already a
//! tree of pieces.
//!
//! `Type` is sized in `lib.rs`, against the declarations. MIR's own type
//! (`MirType`) is a second tree that has to be sized too, and it used to carry
//! its own copy of every rule: a tuple's offsets, an array's stride, a
//! wrapper's payload, a SIMD vector's alignment. The two agreed by luck. A type
//! says what shape it is (`Shaped`), and the rules live here, once. A scalar's
//! width stays with `Type::scalar_bytes` and a wrapper's placement with
//! `rask_types::abi`; this file only composes them.

use rask_ast::Target;
use rask_types::abi::{self, WrapperLayout};
use rask_types::Type;

/// The runtime's `RaskStr` (a `string` or `bytes`): 16 bytes on every target,
/// since its heap form holds a 64-bit length beside the pointer.
pub const BUFFER_SIZE_ALIGN: (u32, u32) = (16, 8);

/// A fat pointer: the data address and the vtable address.
pub fn fat_pointer(word: u32) -> (u32, u32) {
    (2 * word, word)
}

/// One level of a type, with its parts as the same kind of type.
pub enum Shape<'a, T> {
    /// No bytes: `()`, a function that returns nothing.
    Void,
    /// A number or flag of this many bytes, aligned to its size.
    Scalar(u32),
    /// One pointer.
    Word,
    /// The runtime's refcounted string/bytes buffer.
    Buffer,
    /// Data address plus vtable address.
    FatPointer,
    /// A size and alignment something else already settled: a declared
    /// struct's or enum's layout.
    Fixed { size: u32, align: u32 },
    Array(&'a T, u32),
    Tuple(&'a [T]),
    /// `T?` with a tag. A niche option (the payload's own address with null
    /// for `none`) is `Word`.
    Option(&'a T),
    Result(&'a T, &'a T),
    /// An error union.
    Union(&'a [T]),
    Simd(&'a T, u32),
}

impl<T> Shape<'_, T> {
    /// A scalar, by the checker's name for it; its width is
    /// `Type::scalar_bytes`'s.
    pub fn scalar(ty: Type) -> Self {
        Shape::Scalar(ty.scalar_bytes().expect("a scalar has a width"))
    }
}

/// A type that can say what shape it is.
pub trait Shaped: Sized {
    fn shape(&self) -> Shape<'_, Self>;
}

/// Size and alignment of a tuple's parts in order, each at its own alignment,
/// and where each part sits.
pub fn tuple_layout(parts: impl IntoIterator<Item = (u32, u32)>) -> (Vec<u32>, u32, u32) {
    let mut offset = 0u32;
    let mut max_align = 1u32;
    let mut offsets = Vec::new();
    for (size, align) in parts {
        let align = align.max(1);
        max_align = max_align.max(align);
        offset = align_up(offset, align);
        offsets.push(offset);
        offset = offset.saturating_add(size);
    }
    (offsets, align_up(offset, max_align), max_align)
}

/// `[T; len]`: the elements back to back, so the stride is the element's size.
///
/// Sizes are `u32`, so a value tops out at 4 GiB. An array over that comes
/// back as `u32::MAX`, as does a struct, tuple or enum whose parts add up past
/// it: the checker reports it (E0980), and the passes that
/// still run on a program with errors, to find more of them, ask its layout
/// too. `[0; 3000000000]` panicked the compiler in the ownership check's copy
/// verdict when this panicked instead.
pub fn array_layout(elem: (u32, u32), len: usize) -> (u32, u32) {
    let size = u32::try_from(len).ok().and_then(|len| elem.0.checked_mul(len)).unwrap_or(u32::MAX);
    (size, elem.1)
}

/// A SIMD vector: its lanes' bytes, at natural alignment up to 32. Lane
/// counts are small and fixed by the type's name, so overflow is a bug.
pub fn simd_layout(elem_size: u32, lanes: usize) -> (u32, u32) {
    let total = u32::try_from(lanes)
        .ok()
        .and_then(|lanes| elem_size.checked_mul(lanes))
        .unwrap_or_else(|| panic!("a {lanes}-lane SIMD vector of {elem_size}-byte lanes overflows its size"));
    (total, total.min(32))
}

/// The largest size and strictest alignment among a wrapper's payloads.
pub fn widest(parts: impl IntoIterator<Item = (u32, u32)>) -> (u32, u32) {
    parts.into_iter().fold((0, 1), |(size, align), (s, a)| (size.max(s), align.max(a)))
}

/// Rounds `val` up to a multiple of `align`. A size past `u32::MAX` stays
/// there, the mark the checker reports as too large (E0980): wrapped, two
/// 3 GB arrays in a struct came out 1.7 GB and the compiler hung (#2061).
pub(crate) fn align_up(val: u32, align: u32) -> u32 {
    match val.checked_add(align - 1) {
        Some(v) => v & !(align - 1),
        None => u32::MAX,
    }
}

/// Size and alignment of `ty` for this target.
pub fn size_align_of<T: Shaped>(ty: &T, target: Target) -> (u32, u32) {
    let word = target.word();
    match ty.shape() {
        Shape::Void => (0, 1),
        Shape::Scalar(n) => (n, n),
        Shape::Word => (word, word),
        Shape::Buffer => BUFFER_SIZE_ALIGN,
        Shape::FatPointer => fat_pointer(word),
        Shape::Fixed { size, align } => (size, align),
        Shape::Array(elem, len) => array_layout(size_align_of(elem, target), len as usize),
        Shape::Tuple(parts) => {
            let (_, size, align) = tuple_layout(parts.iter().map(|p| size_align_of(p, target)));
            (size, align)
        }
        Shape::Option(_) | Shape::Result(..) | Shape::Union(_) => {
            let w = wrapper_of(ty, target).expect("a wrapper has a layout");
            (w.size, w.align)
        }
        Shape::Simd(elem, lanes) => simd_layout(size_of(elem, target), lanes as usize),
    }
}

pub fn size_of<T: Shaped>(ty: &T, target: Target) -> u32 {
    size_align_of(ty, target).0
}

pub fn align_of<T: Shaped>(ty: &T, target: Target) -> u32 {
    size_align_of(ty, target).1
}

/// Where a `T?`, `T or E` or error union keeps its payload, and its size;
/// `None` for anything else.
pub fn wrapper_of<T: Shaped>(ty: &T, target: Target) -> Option<WrapperLayout> {
    match ty.shape() {
        Shape::Option(inner) => {
            let (size, align) = size_align_of(inner, target);
            Some(abi::option_layout(size, align))
        }
        Shape::Result(ok, err) => {
            let (size, align) = widest([size_align_of(ok, target), size_align_of(err, target)]);
            Some(abi::result_layout(size, align))
        }
        Shape::Union(members) => {
            let (size, align) = widest(members.iter().map(|m| size_align_of(m, target)));
            Some(abi::union_layout(size, align))
        }
        _ => None,
    }
}

/// The byte offset of a wrapper's payload; 0 for anything that isn't one (a
/// niche option is its payload).
pub fn payload_offset_of<T: Shaped>(ty: &T, target: Target) -> u32 {
    wrapper_of(ty, target).map_or(0, |w| w.payload_offset)
}

/// Where each part of a tuple sits.
pub fn tuple_offsets<T: Shaped>(parts: &[T], target: Target) -> Vec<u32> {
    tuple_layout(parts.iter().map(|p| size_align_of(p, target))).0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest type that can be sized: enough to exercise each rule.
    enum Toy {
        Byte,
        Int,
        Long,
        Ptr,
        Array(Box<Toy>, u32),
        Opt(Box<Toy>),
        Res(Box<Toy>, Box<Toy>),
        Fat,
        Text,
    }

    impl Shaped for Toy {
        fn shape(&self) -> Shape<'_, Toy> {
            match self {
                Toy::Byte => Shape::scalar(Type::U8),
                Toy::Int => Shape::scalar(Type::I32),
                Toy::Long => Shape::scalar(Type::I64),
                Toy::Ptr => Shape::Word,
                Toy::Array(elem, len) => Shape::Array(elem, *len),
                Toy::Opt(inner) => Shape::Option(inner),
                Toy::Res(ok, err) => Shape::Result(ok, err),
                Toy::Fat => Shape::FatPointer,
                Toy::Text => Shape::Buffer,
            }
        }
    }

    fn arm() -> Target {
        Target::for_arch("arm")
    }

    const X86: Target = Target::LP64;

    #[test]
    fn a_pointer_is_the_targets_word() {
        assert_eq!(size_align_of(&Toy::Ptr, arm()), (4, 4));
        assert_eq!(size_align_of(&Toy::Ptr, X86), (8, 8));
        assert_eq!(size_align_of(&Toy::Fat, arm()), (8, 4));
        assert_eq!(size_align_of(&Toy::Fat, X86), (16, 8));
        // The runtime's string is 16 bytes either way.
        assert_eq!(size_align_of(&Toy::Text, arm()), (16, 8));
        assert_eq!(size_align_of(&Toy::Text, X86), (16, 8));
    }

    #[test]
    fn a_tuple_places_each_part_at_its_own_alignment() {
        let t = [Toy::Byte, Toy::Long, Toy::Int];
        assert_eq!(tuple_offsets(&t, X86), vec![0, 8, 16]);
        assert_eq!(tuple_layout(t.iter().map(|p| size_align_of(p, X86))).1, 24);
        // A pointer in the middle moves with the width.
        let p = [Toy::Byte, Toy::Ptr, Toy::Byte];
        assert_eq!(tuple_offsets(&p, arm()), vec![0, 4, 8]);
        assert_eq!(tuple_offsets(&p, X86), vec![0, 8, 16]);
    }

    #[test]
    fn an_array_is_its_elements_back_to_back() {
        assert_eq!(size_align_of(&Toy::Array(Box::new(Toy::Int), 3), X86), (12, 4));
        assert_eq!(size_align_of(&Toy::Array(Box::new(Toy::Ptr), 3), arm()), (12, 4));
    }

    #[test]
    fn wrappers_place_the_payload_after_their_tag() {
        let opt = Toy::Opt(Box::new(Toy::Long));
        assert_eq!(wrapper_of(&opt, X86), Some(abi::option_layout(8, 8)));
        assert_eq!(payload_offset_of(&opt, X86), 8);
        let res = Toy::Res(Box::new(Toy::Int), Box::new(Toy::Byte));
        assert_eq!(wrapper_of(&res, X86), Some(abi::result_layout(4, 4)));
        assert_eq!(wrapper_of(&Toy::Int, X86), None);
        assert_eq!(payload_offset_of(&Toy::Int, X86), 0);
    }
}
