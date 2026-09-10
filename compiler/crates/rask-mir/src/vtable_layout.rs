// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Where things sit in a vtable.
//!
//! Layout: `[size:i64, align:i64, owned_release:i64, method_0:i64, …]`.
//!
//! Lowering picks the offset for a `TraitCall` and codegen writes the method
//! pointers, so both need this and they must agree. They didn't: the header was
//! three words in three places, and shortening it in codegen alone left every
//! `TraitCall` reading one word past its method — a segfault on the first
//! dispatch.
//!
//! `owned_release` is for a box that *owns* the value inside it, which is the
//! case whenever the value was moved in rather than borrowed. The checker draws
//! that line already: `boxes.push(h)` moves — using `h` afterwards is E0800 —
//! while `through_a_box(h)` borrows and leaves `h` usable. So a box in a
//! container, a struct field or a return value owns its contents and needs a
//! release for them; a box built for a call does not, and `TraitDrop` (which is
//! only emitted for that borrowed case) must never call this.
//!
//! Getting that backwards is what #1144 was: the slot used to be called for
//! every box and released the value's strings, so a borrowed box decremented a
//! buffer the frame still owned. Null for a concrete type whose value owns
//! nothing.

/// Byte offset of the size field.
pub const VTABLE_SIZE_OFFSET: u32 = 0;
/// Byte offset of the alignment field.
pub const VTABLE_ALIGN_OFFSET: u32 = 8;
/// Byte offset of the release for a value the box owns; null when the concrete
/// type holds nothing that needs one.
pub const VTABLE_OWNED_RELEASE_OFFSET: u32 = 16;
/// Byte offset where method pointers begin.
pub const VTABLE_METHODS_START: u32 = 24;

/// Byte offset of the method at `index` among a trait's compatible methods.
pub fn method_offset(index: usize) -> u32 {
    VTABLE_METHODS_START + (index as u32) * 8
}
