// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Layout constants for runtime data structures.
//!
//! The values the C runtime also reads come from `rask_mono::abi` and
//! `rask_mono::runtime_abi`; the runtime gets the latter as rask_abi.h.

// ── Fat pointer (interface object) ───────────────────────────────────
pub const FAT_PTR_DATA_OFFSET: i32 = 0;
pub const FAT_PTR_VTABLE_OFFSET: i32 = 8;

// ── Result / Option ──────────────────────────────────────────────
// Single source of truth in `rask_mono::abi`; re-exported here as i32 for the
// Cranelift store/load offset APIs. TAG_OFFSET and PAYLOAD_OFFSET name the Option
// slots (tag=0, payload=8); the RESULT_* names cover the ER15 origin-field layout.
pub const TAG_OFFSET: i32 = rask_mono::abi::OPTION_TAG_OFFSET as i32;
pub const PAYLOAD_OFFSET: i32 = rask_mono::abi::OPTION_PAYLOAD_OFFSET as i32;
pub const ORIGIN_FILE_OFFSET: i32 = rask_mono::abi::RESULT_ORIGIN_FILE_OFFSET as i32;
pub const ORIGIN_LINE_OFFSET: i32 = rask_mono::abi::RESULT_ORIGIN_LINE_OFFSET as i32;
pub const RESULT_PAYLOAD_OFFSET: i32 = rask_mono::abi::RESULT_PAYLOAD_OFFSET as i32;
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
