// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The in-memory layout of `T?`, `T or E` and an error union `(A | B)`.
//!
//! The only definition. Mono's layouts, `MirType::size`, codegen's slot sizes
//! and the C runtime all take theirs from here. Every value sits at its own
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

fn align_up(n: u32, align: u32) -> u32 {
    (n + align - 1) & !(align - 1)
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
