// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Where things sit in a vtable.
//!
//! Layout: `[size:i64, align:i64, method_0:i64, method_1:i64, …]`.
//!
//! Lowering picks the offset for a `TraitCall` and codegen writes the method
//! pointers, so both need this and they must agree. They didn't: the header was
//! three words in three places, and shortening it in codegen alone left every
//! `TraitCall` reading one word past its method — a segfault on the first
//! dispatch.
//!
//! There is no drop slot. A boxed value's contents belong to the frame, not to
//! the box (mem.boxes, #1144) — `TraitBox` copies the value shallowly, so the
//! box and the frame's own local hold the same string buffer and the same
//! container handle, and a release from both sides is one decrement too many.

/// Byte offset of the size field.
pub const VTABLE_SIZE_OFFSET: u32 = 0;
/// Byte offset of the alignment field.
pub const VTABLE_ALIGN_OFFSET: u32 = 8;
/// Byte offset where method pointers begin.
pub const VTABLE_METHODS_START: u32 = 16;

/// Byte offset of the method at `index` among a trait's compatible methods.
pub fn method_offset(index: usize) -> u32 {
    VTABLE_METHODS_START + (index as u32) * 8
}
