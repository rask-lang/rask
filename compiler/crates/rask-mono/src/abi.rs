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

/// `none` for a niche-optimized `Handle<T>?`. That option carries no tag — the
/// handle itself is the value — so `none` is an all-bits-set handle
/// (index=UINT32_MAX, gen=UINT32_MAX), which no live slot can ever produce.
pub const HANDLE_NONE_SENTINEL: i64 = -1;
