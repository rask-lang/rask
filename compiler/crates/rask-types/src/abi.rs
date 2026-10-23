// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! How values are laid out in memory: structs, enums, `T?`, `T or E` and an
//! error union `(A | B)`.
//!
//! The only definition. Mono's layouts, `MirType::size`, codegen's slot sizes,
//! the C runtime and the Copy rule's 16-byte threshold all take theirs from
//! here. Every value sits at its own
//! size and alignment; nothing is padded out to a word to make it easier to
//! move.
//!
//! - `T?`: `[tag:1][payload: T]`, the payload at `T`'s alignment.
//! - `T or E`: `[tag:1][origin_line:4][origin_file:8][payload: max(T, E)]`.
//!   The origin fields carry the error's source location (ER15).
//! - `(A | B)`: `[member:1][payload: max(A, B)]`, the member's position in the
//!   union as written. The members are nominally distinct types and nothing in
//!   their bytes tells them apart, so which one is held has to be written down
//!   (#776).

/// Every wrapper's tag (or a union's member index) is its first byte.
pub const TAG_OFFSET: u32 = 0;
pub const TAG_BYTES: u32 = 1;

/// The source line a propagated error came from, as a `u32`.
pub const RESULT_ORIGIN_LINE_OFFSET: u32 = 4;
pub const RESULT_ORIGIN_LINE_BYTES: u32 = 4;
/// The source file it came from, as a pointer to its name.
pub const RESULT_ORIGIN_FILE_OFFSET: u32 = 8;
pub const RESULT_ORIGIN_FILE_BYTES: u32 = 8;
/// Where a `T or E`'s payload starts, unless it needs a stricter alignment
/// (a 32-byte SIMD vector).
const RESULT_HEADER_BYTES: u32 = 16;

/// Where a wrapper keeps its payload, and how big the whole thing is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WrapperLayout {
    pub payload_offset: u32,
    pub size: u32,
    pub align: u32,
}

pub fn align_up(n: u32, align: u32) -> u32 {
    (n + align - 1) & !(align - 1)
}

/// Where a struct's fields go, given each one's (size, align) in declaration
/// order. Returns `(field index, offset)` in memory order, and the struct's
/// size and alignment.
///
/// The default layout puts the most-aligned fields first (S1), which leaves
/// the least padding; a stable sort keeps declaration order among equals.
/// `@layout(C)` keeps declaration order (S2).
pub fn place_fields(fields: &[(u32, u32)], c_order: bool) -> (Vec<(usize, u32)>, u32, u32) {
    let mut order: Vec<usize> = (0..fields.len()).collect();
    if !c_order {
        order.sort_by(|&a, &b| fields[b].1.cmp(&fields[a].1));
    }
    let mut placed = Vec::with_capacity(fields.len());
    let mut offset = 0u32;
    let mut align = 1u32;
    for i in order {
        let (size, field_align) = fields[i];
        align = align.max(field_align);
        offset = align_up(offset, field_align);
        placed.push((i, offset));
        offset += size;
    }
    (placed, align_up(offset, align), align)
}

/// An enum: the tag first (E1), then the widest variant's payload at the
/// strictest payload alignment (E5, E6). Returns the payload offset, the
/// size and the alignment.
pub fn enum_layout(tag: (u32, u32), payloads: &[(u32, u32)]) -> (u32, u32, u32) {
    let (size, payload_align) =
        payloads.iter().fold((0u32, 1u32), |(s, a), &(ps, pa)| (s.max(ps), a.max(pa)));
    let align = tag.1.max(payload_align);
    let payload_offset = align_up(tag.0, payload_align);
    (payload_offset, align_up(payload_offset + size, align), align)
}

/// A user `union`: every field at offset 0 (UN1–UN3). Returns size and
/// alignment.
pub fn overlay(fields: &[(u32, u32)]) -> (u32, u32) {
    let (size, align) = fields.iter().fold((0u32, 1u32), |(s, a), &(fs, fa)| (s.max(fs), a.max(fa)));
    (align_up(size, align), align)
}

/// The size and alignment of a type the stdlib declares as an empty struct
/// standing for a runtime value: a handle is one word, `Duration`/`Instant`
/// are an `int64_t` of nanoseconds. The declaration has no fields, so anything
/// that sized it from them would say zero (#924, #1081).
pub fn opaque_layout(name: &str) -> Option<(u32, u32)> {
    if rask_stdlib::is_runtime_handle(name) || matches!(name, "Duration" | "Instant") {
        Some((8, 8))
    } else {
        None
    }
}

/// An enum's tag type: the backing type it declares (E14), else the
/// narrowest that numbers every variant (E2).
pub fn enum_tag(backing: Option<&str>, variant_count: usize) -> crate::Type {
    use crate::Type;
    let fits = if variant_count <= 256 { Type::U8 } else { Type::U16 };
    match backing {
        Some("u8") => Type::U8,
        Some("u16") => Type::U16,
        Some("u32") => Type::U32,
        Some("i8") => Type::I8,
        Some("i16") => Type::I16,
        Some("i32") => Type::I32,
        Some("i64") => Type::I64,
        _ => fits,
    }
}

/// Is this struct laid out in declaration order (`@layout(C)`, S2)?
pub fn has_c_layout(attrs: &[rask_ast::attr::Attr]) -> bool {
    attrs.iter().any(|a| a.is("layout") && a.positional().and_then(|v| v.ident()) == Some("C"))
}

/// `T?`, for a payload of this size and alignment.
pub fn option_layout(payload_size: u32, payload_align: u32) -> WrapperLayout {
    let align = payload_align.max(1);
    let payload_offset = align_up(TAG_BYTES, align);
    WrapperLayout { payload_offset, size: align_up(payload_offset + payload_size, align), align }
}

/// `T or E`, for the larger of the two sides and the stricter alignment.
pub fn result_layout(payload_size: u32, payload_align: u32) -> WrapperLayout {
    let align = payload_align.max(8);
    let payload_offset = align_up(RESULT_HEADER_BYTES, align);
    WrapperLayout { payload_offset, size: align_up(payload_offset + payload_size, align), align }
}

/// An error union, for its widest member and the strictest alignment.
pub fn union_layout(payload_size: u32, payload_align: u32) -> WrapperLayout {
    option_layout(payload_size, payload_align)
}

/// `none` for a niche-optimized `Link<T>?`: a link is the node's machine
/// address, and the address that can never name a node is the null one.
///
/// This used to be an all-bits-set word, which worked but hid two things. A rack
/// chunk arrives zeroed, so with null as `none` a node's links start out absent
/// with nothing written — a field codegen forgets reads as `none` instead of as
/// a live link to address 0. And a runtime check is `if (!link)`, the check C
/// already writes everywhere, instead of a comparison against a magic constant.
/// The sentinel belongs to the type, not to the niche mechanism.
pub const LINK_NONE_SENTINEL: i64 = 0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_up_rounds_to_the_next_multiple() {
        assert_eq!(align_up(8, 4), 8);
        assert_eq!(align_up(0, 4), 0);
        assert_eq!(align_up(1, 4), 4);
        assert_eq!(align_up(9, 8), 16);
    }

    #[test]
    fn fields_go_most_aligned_first_unless_c_order() {
        // u8, i64, u8: reordered to i64, u8, u8 = 10, padded to 16.
        let (placed, size, align) = place_fields(&[(1, 1), (8, 8), (1, 1)], false);
        assert_eq!(placed, vec![(1, 0), (0, 8), (2, 9)]);
        assert_eq!((size, align), (16, 8));
        // Declaration order pads after each u8.
        let (placed, size, _) = place_fields(&[(1, 1), (8, 8), (1, 1)], true);
        assert_eq!(placed, vec![(0, 0), (1, 8), (2, 16)]);
        assert_eq!(size, 24);
    }

    #[test]
    fn an_enum_is_a_tag_then_its_widest_payload() {
        assert_eq!(enum_layout((1, 1), &[]), (1, 1, 1));
        assert_eq!(enum_layout((1, 1), &[(8, 8), (4, 4)]), (8, 16, 8));
    }

    #[test]
    fn an_option_is_its_payload_and_a_tag() {
        assert_eq!(option_layout(1, 1), WrapperLayout { payload_offset: 1, size: 2, align: 1 });
        assert_eq!(option_layout(4, 4), WrapperLayout { payload_offset: 4, size: 8, align: 4 });
        assert_eq!(option_layout(8, 8), WrapperLayout { payload_offset: 8, size: 16, align: 8 });
        assert_eq!(option_layout(16, 8), WrapperLayout { payload_offset: 8, size: 24, align: 8 });
    }

    #[test]
    fn a_result_keeps_its_payload_after_the_origin() {
        assert_eq!(result_layout(1, 1), WrapperLayout { payload_offset: 16, size: 24, align: 8 });
        assert_eq!(result_layout(16, 16), WrapperLayout { payload_offset: 16, size: 32, align: 16 });
    }
}
