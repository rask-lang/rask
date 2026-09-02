// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Layout constants for runtime data structures.
//!
//! These must match the C runtime definitions (pool.c, string.c, etc.).
//! Pool layout is verified by _Static_assert in pool.c.

// ── Pool (pool.c) ────────────────────────────────────────────────
pub const POOL_STRIDE_OFFSET: i32 = 16;
pub const POOL_CAP_OFFSET: i32 = 24;
pub const POOL_SLOTS_OFFSET: i32 = 40;
pub const SLOT_GEN_OFFSET: i32 = 0;
pub const SLOT_DATA_OFFSET: i32 = 8;

// ── Fat pointer (trait object) ───────────────────────────────────
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
pub const HANDLE_NONE_SENTINEL: i64 = rask_mono::abi::HANDLE_NONE_SENTINEL;

// ── String SSO (string.c) ────────────────────────────────────────
// Empty string: 16 zero bytes except byte 15 = 0x0F (remaining capacity = 15).
/// MSB of a `RaskStr`'s second word: set means the heap form, clear means SSO.
pub const STRING_HEAP_FLAG: u64 = 1u64 << 63;

pub const EMPTY_STRING_LO: i64 = 0;
pub const EMPTY_STRING_HI: i64 = 0x0F00_0000_0000_0000u64 as i64;
pub const STRING_SIZE: i32 = 16;
