// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Canonical in-memory layout offsets for `Result<T, E>` and `Option<T>`.
//!
//! Single source of truth. `rask-mir` and `rask-codegen` re-export these instead
//! of defining their own copies, and the C runtime mirrors them. Change a value
//! here and it changes everywhere the layout is computed.
//!
//! Result: `[tag:8][origin_file:8][origin_line:8][payload:max(ok,err)]` (ER15 —
//! the origin fields carry the error's source location for diagnostics).
//! Option: `[tag:8][payload:inner]`.

pub const RESULT_TAG_OFFSET: u32 = 0;
pub const RESULT_ORIGIN_FILE_OFFSET: u32 = 8;
pub const RESULT_ORIGIN_LINE_OFFSET: u32 = 16;
pub const RESULT_PAYLOAD_OFFSET: u32 = 24;

pub const OPTION_TAG_OFFSET: u32 = 0;
pub const OPTION_PAYLOAD_OFFSET: u32 = 8;

/// An error union `(A | B)`: `[member:8][member bytes:max(A, B)]`.
///
/// The members are nominally distinct types and nothing in their bytes tells
/// them apart, so which one is held has to be written down. Before this the
/// union collapsed to a bare pointer and `r is ParseError` on a
/// `T or (ParseError | DivError)` was answered by the Result's own tag — every
/// error read as whichever member was listed second (#776).
///
/// The index is the member's position in the union as written.
pub const UNION_MEMBER_OFFSET: u32 = 0;
pub const UNION_PAYLOAD_OFFSET: u32 = 8;

/// `none` for a niche-optimized `Handle<T>?`. That option carries no tag — the
/// handle itself is the value — so `none` is an all-bits-set handle
/// (index=UINT32_MAX, gen=UINT32_MAX), which no live slot can ever produce.
pub const HANDLE_NONE_SENTINEL: i64 = -1;

/// `none` for a niche-optimized `Link<T>?`. Same trick, different impossible
/// value: a link is the node's machine address, and the address that can never
/// name a node is the null one.
///
/// This used to borrow the handle's -1, which worked but hid two things. A rack
/// chunk arrives zeroed, so with null as `none` a node's links start out absent
/// with nothing written — a field codegen forgets reads as `none` instead of as
/// a live link to address 0. And a runtime check is `if (!link)`, the check C
/// already writes everywhere, instead of a comparison against a magic constant.
/// The sentinel belongs to the type, not to the niche mechanism.
pub const LINK_NONE_SENTINEL: i64 = 0;

/// A scalar payload occupies the whole slot, whatever its own width.
pub const PAYLOAD_SLOT_BYTES: u32 = 8;

/// How a payload sits in a wrapper's payload slot.
///
/// The float case is the one that bites. A payload read is a plain load of the
/// slot, so a 4-byte `f32` write and an 8-byte read disagree about which bytes
/// carry the value — that's #629, and the same mistake reappeared one layer up
/// when the wrap moved into MIR. Storing floats as `f64` makes the write and the
/// read agree by construction.
///
/// Integers are written full-width and may be read at their own narrower width;
/// little-endian puts the meaningful bytes first, so both agree.
///
/// Kept here beside the offsets because it is the same kind of fact: MIR widens
/// a value to this on the way in, codegen takes it apart by this on the way out,
/// and neither gets its own opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadRepr {
    /// Float scalar — occupies the slot as an `f64`.
    Float64,
    /// Integer-like scalar — written full-width, read at its own width.
    IntFullWidth,
    /// Aggregate — lives at its own address, copied in and out by bytes.
    InPlace,
}

/// Classify a payload from the two facts that decide it.
pub fn payload_repr(is_float: bool, passed_by_address: bool) -> PayloadRepr {
    if passed_by_address {
        PayloadRepr::InPlace
    } else if is_float {
        PayloadRepr::Float64
    } else {
        PayloadRepr::IntFullWidth
    }
}
