// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Layout constants for runtime data structures.
//!
//! The values the C runtime also reads come from `rask_types::abi` and
//! `rask_mono::runtime_abi`; the runtime gets the latter as rask_abi.h.

// ── Fat pointer (interface object) ───────────────────────────────────
pub const FAT_PTR_DATA_OFFSET: i32 = 0;
pub const FAT_PTR_VTABLE_OFFSET: i32 = 8;

// ── Result / Option / union ──────────────────────────────────────
// `rask_types::abi` is the one definition; these are i32 for Cranelift's
// load/store offsets. A wrapper's payload offset depends on the payload, so it
// comes from `MirType::payload_offset`, never a constant.
pub const TAG_OFFSET: i32 = rask_types::abi::TAG_OFFSET as i32;
pub const TAG_BYTES: u32 = rask_types::abi::TAG_BYTES;
pub const ORIGIN_FILE_OFFSET: i32 = rask_types::abi::RESULT_ORIGIN_FILE_OFFSET as i32;
pub const ORIGIN_LINE_OFFSET: i32 = rask_types::abi::RESULT_ORIGIN_LINE_OFFSET as i32;
/// The tag of `Some`/`Ok` and of `None`/`Err`.
pub const TAG_PRESENT: i64 = rask_mono::runtime_abi::RASK_TAG_PRESENT;
pub const TAG_ABSENT: i64 = rask_mono::runtime_abi::RASK_TAG_ABSENT;

// ── String SSO (string.c) ────────────────────────────────────────
/// A bit of a `RaskStr`'s second word: set means the heap form, clear means SSO.
pub const STRING_HEAP_FLAG: u64 = 1u64 << rask_mono::runtime_abi::RASK_STRING_HEAP_BIT;

// Empty string: 16 zero bytes except the last, which is the inline capacity
// left: all of it.
pub const EMPTY_STRING_LO: i64 = 0;
pub const EMPTY_STRING_HI: i64 = rask_mono::runtime_abi::RASK_SSO_MAX << 56;
pub const STRING_SIZE: i32 = 16;
