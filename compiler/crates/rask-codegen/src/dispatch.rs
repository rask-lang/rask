// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Stdlib method dispatch — maps MIR call names to C runtime functions.
//!
//! After monomorphization, stdlib method calls arrive at codegen as
//! type-qualified names (e.g., "Vec_push", "Map_get", "string_len").
//! Qualification happens in MIR lowering using type info from the checker.
//! This module maps those names to C runtime functions in the typed
//! implementations (vec.c, map.c, pool.c, string.c).
//!
//! ## Calling convention
//!
//! The typed C API uses `const void*` for element parameters and returns
//! `void*` for element access. Builder.rs handles the adaptation:
//! - Constructors: codegen injects hardcoded elem_size (8) args
//! - Value params (push, set, insert): codegen stores to stack slot, passes address
//! - Value returns (get, pop): codegen loads from returned/out pointer
//! - Pool handles: packed as i64 (index:32 | gen:32) via _packed functions

use cranelift::prelude::*;
use cranelift_module::{Linkage, Module};
use std::collections::HashMap;

use crate::{CodegenError, CodegenResult};

/// A stdlib function entry: MIR name → C runtime function.
pub struct StdlibEntry {
    /// Name as it appears in MIR Call statements
    pub mir_name: &'static str,
    /// C function name in the runtime
    pub c_name: &'static str,
    /// Parameter Cranelift types
    pub params: &'static [Type],
    /// Return type, or None for void
    pub ret_ty: Option<Type>,
}

/// Build the complete stdlib dispatch table.
pub fn stdlib_entries() -> Vec<StdlibEntry> {
    vec![
        // ── Vec operations ─────────────────────────────────────
        // rask_vec_new(elem_size: i64) → RaskVec*
        StdlibEntry {
            mir_name: "Vec_new",
            c_name: "rask_vec_new",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_vec_push(v: RaskVec*, elem: const void*) → i64
        StdlibEntry {
            mir_name: "Vec_push",
            c_name: "rask_vec_push",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_vec_pop(v: RaskVec*, out: void*) → i64
        StdlibEntry {
            mir_name: "Vec_pop",
            c_name: "rask_vec_pop",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_vec_len(v: const RaskVec*) → i64
        StdlibEntry {
            mir_name: "Vec_len",
            c_name: "rask_vec_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_vec_get(v: const RaskVec*, index: i64) → void*
        StdlibEntry {
            mir_name: "Vec_get",
            c_name: "rask_vec_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_vec_set(v: RaskVec*, index: i64, elem: const void*)
        StdlibEntry {
            mir_name: "Vec_set",
            c_name: "rask_vec_set",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
        },
        // rask_vec_clear(v: RaskVec*)
        StdlibEntry {
            mir_name: "Vec_clear",
            c_name: "rask_vec_clear",
            params: &[types::I64],
            ret_ty: None,
        },
        // rask_vec_is_empty(v: const RaskVec*) → i64
        StdlibEntry {
            mir_name: "Vec_is_empty",
            c_name: "rask_vec_is_empty",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_vec_capacity(v: const RaskVec*) → i64
        StdlibEntry {
            mir_name: "Vec_capacity",
            c_name: "rask_vec_capacity",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Subscript (desugared from args[0] → args.index(0)) ─
        // Bare "index" kept: desugaring doesn't have receiver type info
        StdlibEntry {
            mir_name: "index",
            c_name: "rask_vec_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Vec_index",
            c_name: "rask_vec_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Iterator stubs ─────────────────────────────────────
        // iter() on Vec returns the Vec itself (identity)
        StdlibEntry {
            mir_name: "Vec_iter",
            c_name: "rask_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_iter_skip(src: const RaskVec*, n: i64) → RaskVec*
        StdlibEntry {
            mir_name: "Vec_skip",
            c_name: "rask_iter_skip",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── String operations ──────────────────────────────────
        // rask_string_new() → RaskString*
        StdlibEntry {
            mir_name: "string_new",
            c_name: "rask_string_new",
            params: &[],
            ret_ty: Some(types::I64),
        },
        // rask_string_from(cstr: const char*) → RaskString*
        StdlibEntry {
            mir_name: "string_from",
            c_name: "rask_string_from",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_string_len(s: const RaskString*) → i64
        StdlibEntry {
            mir_name: "string_len",
            c_name: "rask_string_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_string_concat(a, b: const RaskString*) → RaskString*
        StdlibEntry {
            mir_name: "concat",
            c_name: "rask_string_concat",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── String methods ────────────────────────────────────
        StdlibEntry {
            mir_name: "string_to_lowercase",
            c_name: "rask_string_to_lowercase",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_starts_with",
            c_name: "rask_string_starts_with",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_lines",
            c_name: "rask_string_lines",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_trim",
            c_name: "rask_string_trim",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // map_err with closures is expanded inline in MIR lowering.
        // Non-closure map_err (e.g. variant constructors) still uses this stub.
        StdlibEntry {
            mir_name: "Result_map_err",
            c_name: "rask_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_split",
            c_name: "rask_string_split",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_parse_int",
            c_name: "rask_string_parse_int",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_parse_float",
            c_name: "rask_string_parse_float",
            params: &[types::I64],
            ret_ty: Some(types::F64),
        },
        StdlibEntry {
            mir_name: "string_substr",
            c_name: "rask_string_substr",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_ends_with",
            c_name: "rask_string_ends_with",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_replace",
            c_name: "rask_string_replace",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_contains",
            c_name: "rask_string_contains",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Conversion to string ──────────────────────────────
        StdlibEntry {
            mir_name: "i64_to_string",
            c_name: "rask_i64_to_string",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "bool_to_string",
            c_name: "rask_bool_to_string",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "f64_to_string",
            c_name: "rask_f64_to_string",
            params: &[types::F64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "char_to_string",
            c_name: "rask_char_to_string",
            params: &[types::I32],
            ret_ty: Some(types::I64),
        },

        // ── Map operations ─────────────────────────────────────
        // rask_map_new(key_size: i64, val_size: i64) → RaskMap*
        StdlibEntry {
            mir_name: "Map_new",
            c_name: "rask_map_new",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_map_insert(m: RaskMap*, key: const void*, val: const void*) → i64
        StdlibEntry {
            mir_name: "Map_insert",
            c_name: "rask_map_insert",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_map_contains(m: const RaskMap*, key: const void*) → i64
        StdlibEntry {
            mir_name: "Map_contains_key",
            c_name: "rask_map_contains",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_map_get(m: const RaskMap*, key: const void*) → void*
        StdlibEntry {
            mir_name: "Map_get",
            c_name: "rask_map_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_map_remove(m: RaskMap*, key: const void*) → i64
        StdlibEntry {
            mir_name: "Map_remove",
            c_name: "rask_map_remove",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Map_len",
            c_name: "rask_map_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Map_is_empty",
            c_name: "rask_map_is_empty",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Map_clear",
            c_name: "rask_map_clear",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "Map_keys",
            c_name: "rask_map_keys",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Map_values",
            c_name: "rask_map_values",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Pool operations ────────────────────────────────────
        // Packed i64 handle interface (index:32 | gen:32)
        // rask_pool_new(elem_size: i64) → RaskPool*
        StdlibEntry {
            mir_name: "Pool_new",
            c_name: "rask_pool_new",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_pool_alloc_packed(p: RaskPool*) → i64 packed handle
        StdlibEntry {
            mir_name: "Pool_alloc",
            c_name: "rask_pool_alloc_packed",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_pool_remove_packed(p: RaskPool*, packed: i64) → i64
        StdlibEntry {
            mir_name: "Pool_remove",
            c_name: "rask_pool_remove_packed",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        // rask_pool_get_packed(p: const RaskPool*, packed: i64) → void*
        StdlibEntry {
            mir_name: "Pool_get",
            c_name: "rask_pool_get_packed",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Resource tracking (runtime safety) ─────────────────
        StdlibEntry {
            mir_name: "rask_resource_register",
            c_name: "rask_resource_register",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "rask_resource_consume",
            c_name: "rask_resource_consume",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "rask_resource_scope_check",
            c_name: "rask_resource_scope_check",
            params: &[types::I64],
            ret_ty: None,
        },

        // ── Pool checked access (runtime safety) ──────────────
        StdlibEntry {
            mir_name: "Pool_checked_access",
            c_name: "rask_pool_get_packed",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Rng operations ────────────────────────────────────────
        StdlibEntry {
            mir_name: "Rng_new",
            c_name: "rask_rng_new",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Rng_from_seed",
            c_name: "rask_rng_from_seed",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Rng_u64",
            c_name: "rask_rng_u64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Rng_i64",
            c_name: "rask_rng_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Rng_f64",
            c_name: "rask_rng_f64",
            params: &[types::I64],
            ret_ty: Some(types::F64),
        },
        StdlibEntry {
            mir_name: "Rng_f32",
            c_name: "rask_rng_f32",
            params: &[types::I64],
            ret_ty: Some(types::F64),
        },
        StdlibEntry {
            mir_name: "Rng_bool",
            c_name: "rask_rng_bool",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "Rng_range",
            c_name: "rask_rng_range",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Random module convenience functions ───────────────────
        StdlibEntry {
            mir_name: "random_f64",
            c_name: "rask_random_f64",
            params: &[],
            ret_ty: Some(types::F64),
        },
        StdlibEntry {
            mir_name: "random_f32",
            c_name: "rask_random_f32",
            params: &[],
            ret_ty: Some(types::F64),
        },
        StdlibEntry {
            mir_name: "random_i64",
            c_name: "rask_random_i64",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "random_bool",
            c_name: "rask_random_bool",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "random_range",
            c_name: "rask_random_range",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── File instance methods ─────────────────────────────────
        StdlibEntry {
            mir_name: "File_close",
            c_name: "rask_file_close",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "File_read_all",
            c_name: "rask_file_read_all",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "File_read_text",
            c_name: "rask_file_read_all",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "File_write",
            c_name: "rask_file_write",
            params: &[types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "File_write_line",
            c_name: "rask_file_write_line",
            params: &[types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "File_lines",
            c_name: "rask_file_lines",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Stdlib module calls ─────────────────────────────────
        StdlibEntry {
            mir_name: "cli_args",
            c_name: "rask_cli_args",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "std_exit",
            c_name: "rask_exit",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "fs_read_lines",
            c_name: "rask_fs_read_lines",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── IO module ───────────────────────────────────────────
        StdlibEntry {
            mir_name: "io_read_line",
            c_name: "rask_io_read_line",
            params: &[],
            ret_ty: Some(types::I64),
        },

        // ── More FS module ──────────────────────────────────────
        StdlibEntry {
            mir_name: "fs_read_file",
            c_name: "rask_fs_read_file",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "fs_write_file",
            c_name: "rask_fs_write_file",
            params: &[types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "fs_exists",
            c_name: "rask_fs_exists",
            params: &[types::I64],
            ret_ty: Some(types::I8),
        },
        StdlibEntry {
            mir_name: "fs_open",
            c_name: "rask_fs_open",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "fs_create",
            c_name: "rask_fs_create",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "fs_canonicalize",
            c_name: "rask_fs_canonicalize",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "fs_copy",
            c_name: "rask_fs_copy",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "fs_rename",
            c_name: "rask_fs_rename",
            params: &[types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "fs_remove",
            c_name: "rask_fs_remove",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "fs_create_dir",
            c_name: "rask_fs_create_dir",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "fs_create_dir_all",
            c_name: "rask_fs_create_dir_all",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "fs_append_file",
            c_name: "rask_fs_append_file",
            params: &[types::I64, types::I64],
            ret_ty: None,
        },

        // ── Net module ──────────────────────────────────────────────
        StdlibEntry {
            mir_name: "net_tcp_listen",
            c_name: "rask_net_tcp_listen",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── JSON module ─────────────────────────────────────────────
        StdlibEntry {
            mir_name: "json_encode_string",
            c_name: "rask_json_encode_string",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "json_encode_i64",
            c_name: "rask_json_encode_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "json_buf_new",
            c_name: "rask_json_buf_new",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "json_buf_add_string",
            c_name: "rask_json_buf_add_string",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "json_buf_add_i64",
            c_name: "rask_json_buf_add_i64",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "json_buf_add_f64",
            c_name: "rask_json_buf_add_f64",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "json_buf_add_bool",
            c_name: "rask_json_buf_add_bool",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "json_buf_add_raw",
            c_name: "rask_json_buf_add_raw",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "json_buf_finish",
            c_name: "rask_json_buf_finish",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "json_parse",
            c_name: "rask_json_parse",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "json_get_string",
            c_name: "rask_json_get_string",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "json_get_i64",
            c_name: "rask_json_get_i64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "json_get_f64",
            c_name: "rask_json_get_f64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "json_get_bool",
            c_name: "rask_json_get_bool",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I8),
        },
        StdlibEntry {
            mir_name: "json_decode",
            c_name: "rask_json_decode",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Clone (shallow copy for i64-sized values) ───────────────
        StdlibEntry {
            mir_name: "clone",
            c_name: "rask_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Concurrency: spawn/join/detach (green scheduler) ────────
        StdlibEntry {
            mir_name: "spawn",
            c_name: "rask_green_closure_spawn",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "join",
            c_name: "rask_green_join",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "detach",
            c_name: "rask_green_detach",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "cancel",
            c_name: "rask_green_cancel",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "rask_task_cancelled",
            c_name: "rask_green_task_is_cancelled",
            params: &[],
            ret_ty: Some(types::I32),
        },
        StdlibEntry {
            mir_name: "rask_sleep_ns",
            c_name: "rask_green_sleep_ns",
            params: &[types::I64],
            ret_ty: None,
        },

        // ── Concurrency: runtime init/shutdown ───────────────────────
        StdlibEntry {
            mir_name: "rask_runtime_init",
            c_name: "rask_runtime_init",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "rask_runtime_shutdown",
            c_name: "rask_runtime_shutdown",
            params: &[],
            ret_ty: None,
        },

        // ── Concurrency: green spawn (poll-based state machine) ──────
        StdlibEntry {
            mir_name: "rask_green_spawn",
            c_name: "rask_green_spawn",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Concurrency: yield helpers ───────────────────────────────
        StdlibEntry {
            mir_name: "rask_yield",
            c_name: "rask_yield",
            params: &[],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "rask_yield_timeout",
            c_name: "rask_yield_timeout",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "rask_yield_read",
            c_name: "rask_yield_read",
            params: &[types::I32, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "rask_yield_write",
            c_name: "rask_yield_write",
            params: &[types::I32, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "rask_yield_accept",
            c_name: "rask_yield_accept",
            params: &[types::I32],
            ret_ty: None,
        },

        // ── Async I/O (dual-path: green task or blocking) ─────────────
        StdlibEntry {
            mir_name: "rask_async_read",
            c_name: "rask_async_read",
            params: &[types::I32, types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "rask_async_write",
            c_name: "rask_async_write",
            params: &[types::I32, types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "rask_async_accept",
            c_name: "rask_async_accept",
            params: &[types::I32],
            ret_ty: Some(types::I64),
        },

        // ── Async channels (yield-based) ─────────────────────────────
        StdlibEntry {
            mir_name: "rask_channel_send_async",
            c_name: "rask_channel_send_async",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "rask_channel_recv_async",
            c_name: "rask_channel_recv_async",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Green-aware sleep ────────────────────────────────────────
        StdlibEntry {
            mir_name: "rask_green_sleep_ns",
            c_name: "rask_green_sleep_ns",
            params: &[types::I64],
            ret_ty: None,
        },

        // ── Ensure hooks (LIFO cleanup) ──────────────────────────────
        StdlibEntry {
            mir_name: "rask_ensure_push",
            c_name: "rask_ensure_push",
            params: &[types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "rask_ensure_pop",
            c_name: "rask_ensure_pop",
            params: &[],
            ret_ty: None,
        },

        // ── Memory allocation ─────────────────────────────────────────
        StdlibEntry {
            mir_name: "rask_alloc",
            c_name: "rask_alloc",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Concurrency: channels ──────────────────────────────────
        StdlibEntry {
            mir_name: "Channel_new",
            c_name: "rask_channel_new_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "channel_tx",
            c_name: "rask_channel_get_tx",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "channel_rx",
            c_name: "rask_channel_get_rx",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "send",
            c_name: "rask_channel_send_i64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "recv",
            c_name: "rask_channel_recv_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "sender_clone",
            c_name: "rask_sender_clone_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "sender_drop",
            c_name: "rask_sender_drop_i64",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "recver_drop",
            c_name: "rask_recver_drop_i64",
            params: &[types::I64],
            ret_ty: None,
        },
    ]
}

/// Declare all stdlib functions in a Cranelift module.
///
/// Call after `declare_runtime_functions` and before `declare_functions`.
/// Skips names already claimed by the runtime. User-defined functions
/// declared afterwards overwrite matching entries in `func_ids`.
pub fn declare_stdlib<M: Module>(
    module: &mut M,
    func_ids: &mut HashMap<String, cranelift_module::FuncId>,
) -> CodegenResult<()> {
    for entry in stdlib_entries() {
        // Skip if already declared by runtime
        if func_ids.contains_key(entry.mir_name) {
            continue;
        }

        let mut sig = module.make_signature();
        for &param_ty in entry.params {
            sig.params.push(AbiParam::new(param_ty));
        }
        if let Some(ret) = entry.ret_ty {
            sig.returns.push(AbiParam::new(ret));
        }

        let id = module
            .declare_function(entry.c_name, Linkage::Import, &sig)
            .map_err(|e| CodegenError::CraneliftError(e.to_string()))?;
        func_ids.insert(entry.mir_name.to_string(), id);
    }
    Ok(())
}
