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
pub const TAG_OFFSET: i32 = 0;
pub const PAYLOAD_OFFSET: i32 = 8;

// Result includes error origin fields (ER15) between tag and payload.
// Layout: [tag:8][origin_file_ptr:8][origin_line:8][payload:max(ok,err)]
pub const ORIGIN_FILE_OFFSET: i32 = 8;
pub const ORIGIN_LINE_OFFSET: i32 = 16;
pub const RESULT_PAYLOAD_OFFSET: i32 = 24;

// ── String SSO (string.c) ────────────────────────────────────────
// Empty string: 16 zero bytes except byte 15 = 0x0F (remaining capacity = 15).
pub const EMPTY_STRING_LO: i64 = 0;
pub const EMPTY_STRING_HI: i64 = 0x0F00_0000_0000_0000u64 as i64;
pub const STRING_SIZE: i32 = 16;
