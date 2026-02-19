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
use std::collections::{HashMap, HashSet};

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
    /// Whether this function can panic at runtime
    pub can_panic: bool,
}

/// Leak a String to get a &'static str. Used for dynamically generated
/// dispatch entry names (atomic types). Called once at startup, small and bounded.
fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// Build the complete stdlib dispatch table.
pub fn stdlib_entries() -> Vec<StdlibEntry> {
    let mut entries = vec![
        // ── Vec operations ─────────────────────────────────────
        // rask_vec_new(elem_size: i64) → RaskVec*
        StdlibEntry {
            mir_name: "Vec_new",
            c_name: "rask_vec_new",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_from_static(data: ptr, count: i64) → RaskVec*
        StdlibEntry {
            mir_name: "rask_vec_from_static",
            c_name: "rask_vec_from_static",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // Vec.from(vec) → clone the vec (identity when source is a fresh array literal)
        StdlibEntry {
            mir_name: "Vec_from",
            c_name: "rask_vec_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_free(v: RaskVec*) → void
        StdlibEntry {
            mir_name: "Vec_free",
            c_name: "rask_vec_free",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        // rask_vec_push(v: RaskVec*, elem: const void*) → i64
        StdlibEntry {
            mir_name: "Vec_push",
            c_name: "rask_vec_push",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_pop(v: RaskVec*, out: void*) → i64
        StdlibEntry {
            mir_name: "Vec_pop",
            c_name: "rask_vec_pop",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        // rask_vec_len(v: const RaskVec*) → i64
        StdlibEntry {
            mir_name: "Vec_len",
            c_name: "rask_vec_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_as_ptr(v: const RaskVec*) → void*
        StdlibEntry {
            mir_name: "Vec_as_ptr",
            c_name: "rask_vec_as_ptr",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_get(v: const RaskVec*, index: i64) → void*
        StdlibEntry {
            mir_name: "Vec_get",
            c_name: "rask_vec_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        // rask_vec_set(v: RaskVec*, index: i64, elem: const void*)
        StdlibEntry {
            mir_name: "Vec_set",
            c_name: "rask_vec_set",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
            can_panic: true,
        },
        // rask_vec_clear(v: RaskVec*)
        StdlibEntry {
            mir_name: "Vec_clear",
            c_name: "rask_vec_clear",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        // rask_vec_is_empty(v: const RaskVec*) → i64
        StdlibEntry {
            mir_name: "Vec_is_empty",
            c_name: "rask_vec_is_empty",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_capacity(v: const RaskVec*) → i64
        StdlibEntry {
            mir_name: "Vec_capacity",
            c_name: "rask_vec_capacity",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // rask_vec_insert_at(v: RaskVec*, index: i64, elem: const void*) → i64
        StdlibEntry {
            mir_name: "Vec_insert",
            c_name: "rask_vec_insert_at",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        // rask_vec_remove_at(v: RaskVec*, index: i64, out: void*) → i64
        StdlibEntry {
            mir_name: "Vec_remove",
            c_name: "rask_vec_remove_at",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },

        // ── Subscript (desugared from args[0] → args.index(0)) ─
        // Bare "index" kept: desugaring doesn't have receiver type info
        StdlibEntry {
            mir_name: "index",
            c_name: "rask_vec_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        StdlibEntry {
            mir_name: "Vec_index",
            c_name: "rask_vec_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },

        // rask_vec_slice(v: const RaskVec*, start: i64, end: i64) → RaskVec*
        StdlibEntry {
            mir_name: "Vec_slice",
            c_name: "rask_vec_slice",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_chunks(v: const RaskVec*, chunk_size: i64) → RaskVec* (Vec of Vec ptrs)
        StdlibEntry {
            mir_name: "Vec_chunks",
            c_name: "rask_vec_chunks",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_to_vec(v: const RaskVec*) → RaskVec* (shallow clone)
        StdlibEntry {
            mir_name: "Vec_to_vec",
            c_name: "rask_vec_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_join(v: Vec<string>*, separator: string*) → string*
        StdlibEntry {
            mir_name: "Vec_join",
            c_name: "rask_vec_join",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Iterator runtime support ──────────────────────────────
        // Vec_iter is not needed — iterator chains are fused at MIR level.
        // rask_iter_skip(src: const RaskVec*, n: i64) → RaskVec*
        StdlibEntry {
            mir_name: "Vec_skip",
            c_name: "rask_iter_skip",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_map(vec: RaskVec*, fn: fn_ptr) → RaskVec*
        StdlibEntry {
            mir_name: "Vec_map",
            c_name: "rask_vec_map",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_collect(vec: RaskVec*) → RaskVec* (identity — already materialized)
        StdlibEntry {
            mir_name: "Vec_collect",
            c_name: "rask_vec_collect",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_vec_filter(vec: RaskVec*, fn: fn_ptr) → RaskVec*
        StdlibEntry {
            mir_name: "Vec_filter",
            c_name: "rask_vec_filter",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── String operations ──────────────────────────────────
        // rask_string_free(s: RaskString*) → void
        StdlibEntry {
            mir_name: "string_free",
            c_name: "rask_string_free",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        // rask_string_new() → RaskString*
        StdlibEntry {
            mir_name: "string_new",
            c_name: "rask_string_new",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_string_from(cstr: const char*) → RaskString*
        StdlibEntry {
            mir_name: "string_from",
            c_name: "rask_string_from",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_string_len(s: const RaskString*) → i64
        StdlibEntry {
            mir_name: "string_len",
            c_name: "rask_string_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_string_eq(a, b: const RaskString*) → i64 (0 or 1)
        StdlibEntry {
            mir_name: "string_eq",
            c_name: "rask_string_eq",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_string_ptr(s: const RaskString*) → const char*
        StdlibEntry {
            mir_name: "string_as_ptr",
            c_name: "rask_string_ptr",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_string_concat(a, b: const RaskString*) → RaskString*
        StdlibEntry {
            mir_name: "concat",
            c_name: "rask_string_concat",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_string_append(s: RaskString*, other: const RaskString*) → i64
        // Inserted by the self-concat → append optimization pass.
        StdlibEntry {
            mir_name: "string_append",
            c_name: "rask_string_append",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_string_append_cstr(s: RaskString*, cstr: const char*) → i64
        // Variant for string constant args — avoids RaskString allocation.
        StdlibEntry {
            mir_name: "string_append_cstr",
            c_name: "rask_string_append_cstr",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── String methods ────────────────────────────────────
        StdlibEntry {
            mir_name: "string_to_lowercase",
            c_name: "rask_string_to_lowercase",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_starts_with",
            c_name: "rask_string_starts_with",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_lines",
            c_name: "rask_string_lines",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_trim",
            c_name: "rask_string_trim",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // Result_map_err: both closure and variant constructor forms
        // are expanded inline at MIR level (lower_map_err / lower_map_err_constructor).
        StdlibEntry {
            mir_name: "string_split",
            c_name: "rask_string_split",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_split_whitespace",
            c_name: "rask_string_split_whitespace",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_parse_int",
            c_name: "rask_string_parse_int",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_parse_float",
            c_name: "rask_string_parse_float",
            params: &[types::I64],
            ret_ty: Some(types::F64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_substr",
            c_name: "rask_string_substr",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_ends_with",
            c_name: "rask_string_ends_with",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_replace",
            c_name: "rask_string_replace",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_contains",
            c_name: "rask_string_contains",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Conversion to string ──────────────────────────────
        StdlibEntry {
            mir_name: "i64_to_string",
            c_name: "rask_i64_to_string",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "bool_to_string",
            c_name: "rask_bool_to_string",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "f64_to_string",
            c_name: "rask_f64_to_string",
            params: &[types::F64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "char_to_string",
            c_name: "rask_char_to_string",
            params: &[types::I32],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Math operations ────────────────────────────────────
        StdlibEntry {
            mir_name: "sqrt",
            c_name: "sqrt",
            params: &[types::F64],
            ret_ty: Some(types::F64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "f64_sqrt",
            c_name: "sqrt",
            params: &[types::F64],
            ret_ty: Some(types::F64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "f32_sqrt",
            c_name: "sqrtf",
            params: &[types::F32],
            ret_ty: Some(types::F32),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "abs",
            c_name: "fabs",
            params: &[types::F64],
            ret_ty: Some(types::F64),
            can_panic: false,
        },

        // ── Map operations ─────────────────────────────────────
        // rask_map_free(m: RaskMap*) → void
        StdlibEntry {
            mir_name: "Map_free",
            c_name: "rask_map_free",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        // rask_map_new(key_size: i64, val_size: i64) → RaskMap*
        StdlibEntry {
            mir_name: "Map_new",
            c_name: "rask_map_new",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // Map.from(source) → clone a map
        StdlibEntry {
            mir_name: "Map_from",
            c_name: "rask_map_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_map_insert(m: RaskMap*, key: const void*, val: const void*) → i64
        StdlibEntry {
            mir_name: "Map_insert",
            c_name: "rask_map_insert",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_map_contains(m: const RaskMap*, key: const void*) → i64
        StdlibEntry {
            mir_name: "Map_contains_key",
            c_name: "rask_map_contains",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_map_get(m: const RaskMap*, key: const void*) → void*
        StdlibEntry {
            mir_name: "Map_get",
            c_name: "rask_map_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_map_remove(m: RaskMap*, key: const void*) → i64
        StdlibEntry {
            mir_name: "Map_remove",
            c_name: "rask_map_remove",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Map_len",
            c_name: "rask_map_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Map_is_empty",
            c_name: "rask_map_is_empty",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Map_clear",
            c_name: "rask_map_clear",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Map_keys",
            c_name: "rask_map_keys",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Map_values",
            c_name: "rask_map_values",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Pool operations ────────────────────────────────────
        // Packed i64 handle interface (index:32 | gen:32)
        // rask_pool_free(p: RaskPool*) → void
        StdlibEntry {
            mir_name: "Pool_free",
            c_name: "rask_pool_free",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        // rask_pool_new(elem_size: i64) → RaskPool*
        StdlibEntry {
            mir_name: "Pool_new",
            c_name: "rask_pool_new",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_pool_alloc_packed(p: RaskPool*) → i64 packed handle
        StdlibEntry {
            mir_name: "Pool_alloc",
            c_name: "rask_pool_alloc_packed",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_pool_remove_packed(p: RaskPool*, packed: i64) → i64
        StdlibEntry {
            mir_name: "Pool_remove",
            c_name: "rask_pool_remove_packed",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_pool_get_packed(p: const RaskPool*, packed: i64) → void*
        StdlibEntry {
            mir_name: "Pool_get",
            c_name: "rask_pool_get_packed",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // Pool[handle] index access → same as Pool_get
        StdlibEntry {
            mir_name: "Pool_index",
            c_name: "rask_pool_get_packed",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_pool_handles_packed(p: const RaskPool*) → RaskVec*
        StdlibEntry {
            mir_name: "Pool_handles",
            c_name: "rask_pool_handles_packed",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_pool_values(p: const RaskPool*) → RaskVec*
        StdlibEntry {
            mir_name: "Pool_values",
            c_name: "rask_pool_values",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_pool_len(p: const RaskPool*) → i64
        StdlibEntry {
            mir_name: "Pool_len",
            c_name: "rask_pool_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_pool_insert_packed_sized(p: RaskPool*, elem: void*, elem_size: i64) → packed handle
        StdlibEntry {
            mir_name: "Pool_insert",
            c_name: "rask_pool_insert_packed_sized",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // rask_pool_drain(p: RaskPool*) → RaskVec*
        StdlibEntry {
            mir_name: "Pool_drain",
            c_name: "rask_pool_drain",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },


        // ── Pool checked access (runtime safety) ──────────────
        StdlibEntry {
            mir_name: "Pool_checked_access",
            c_name: "rask_pool_get_packed",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Rng operations ────────────────────────────────────────
        StdlibEntry {
            mir_name: "Rng_new",
            c_name: "rask_rng_new",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Rng_from_seed",
            c_name: "rask_rng_from_seed",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Rng_u64",
            c_name: "rask_rng_u64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Rng_i64",
            c_name: "rask_rng_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Rng_f64",
            c_name: "rask_rng_f64",
            params: &[types::I64],
            ret_ty: Some(types::F64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Rng_f32",
            c_name: "rask_rng_f32",
            params: &[types::I64],
            ret_ty: Some(types::F64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Rng_bool",
            c_name: "rask_rng_bool",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Rng_range",
            c_name: "rask_rng_range",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },

        // ── Random module convenience functions ───────────────────
        StdlibEntry {
            mir_name: "random_f64",
            c_name: "rask_random_f64",
            params: &[],
            ret_ty: Some(types::F64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "random_f32",
            c_name: "rask_random_f32",
            params: &[],
            ret_ty: Some(types::F64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "random_i64",
            c_name: "rask_random_i64",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "random_bool",
            c_name: "rask_random_bool",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "random_range",
            c_name: "rask_random_range",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },

        // ── File instance methods ─────────────────────────────────
        StdlibEntry {
            mir_name: "File_close",
            c_name: "rask_file_close",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "File_read_all",
            c_name: "rask_file_read_all",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "File_read_text",
            c_name: "rask_file_read_all",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "File_write",
            c_name: "rask_file_write",
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "File_write_line",
            c_name: "rask_file_write_line",
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "File_lines",
            c_name: "rask_file_lines",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Stdlib module calls ─────────────────────────────────
        StdlibEntry {
            mir_name: "cli_args",
            c_name: "rask_cli_args",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "std_exit",
            c_name: "rask_exit",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_read_lines",
            c_name: "rask_fs_read_lines",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── IO module ───────────────────────────────────────────
        StdlibEntry {
            mir_name: "io_read_line",
            c_name: "rask_io_read_line",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── More FS module ──────────────────────────────────────
        StdlibEntry {
            mir_name: "fs_read_file",
            c_name: "rask_fs_read_file",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_write_file",
            c_name: "rask_fs_write_file",
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_exists",
            c_name: "rask_fs_exists",
            params: &[types::I64],
            ret_ty: Some(types::I8),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_open",
            c_name: "rask_fs_open",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_create",
            c_name: "rask_fs_create",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_canonicalize",
            c_name: "rask_fs_canonicalize",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_copy",
            c_name: "rask_fs_copy",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_rename",
            c_name: "rask_fs_rename",
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_remove",
            c_name: "rask_fs_remove",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_create_dir",
            c_name: "rask_fs_create_dir",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_create_dir_all",
            c_name: "rask_fs_create_dir_all",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "fs_append_file",
            c_name: "rask_fs_append_file",
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },

        // ── Time module ─────────────────────────────────────────────
        // Instant = i64 nanoseconds, Duration = i64 nanoseconds.
        StdlibEntry {
            mir_name: "Instant_now",
            c_name: "rask_time_Instant_now",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Instant_elapsed",
            c_name: "rask_time_Instant_elapsed",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Instant_duration_since",
            c_name: "rask_time_Instant_duration_since",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Duration_from_nanos",
            c_name: "rask_time_Duration_from_nanos",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Duration_from_millis",
            c_name: "rask_time_Duration_from_millis",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Duration_as_nanos",
            c_name: "rask_time_Duration_as_nanos",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Duration_as_secs",
            c_name: "rask_time_Duration_as_secs",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Duration_as_secs_f64",
            c_name: "rask_time_Duration_as_secs_f64",
            params: &[types::I64],
            ret_ty: Some(types::F64),
            can_panic: false,
        },

        // ── Net module ──────────────────────────────────────────────
        StdlibEntry {
            mir_name: "net_tcp_listen",
            c_name: "rask_net_tcp_listen",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        StdlibEntry {
            mir_name: "TcpListener_accept",
            c_name: "rask_net_tcp_accept",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "TcpConnection_read_http_request",
            c_name: "rask_net_read_http_request",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "TcpConnection_write_http_response",
            c_name: "rask_net_write_http_response",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // Map_from for clone lives at line ~475 (rask_map_clone).
        // rask_map_from (construct from pairs) is a stub — not wired up yet.
        // When needed, give it a distinct MIR name like "Map_from_pairs".
        StdlibEntry {
            mir_name: "json_encode",
            c_name: "rask_json_encode",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── JSON module ─────────────────────────────────────────────
        StdlibEntry {
            mir_name: "json_encode_string",
            c_name: "rask_json_encode_string",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_encode_i64",
            c_name: "rask_json_encode_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_buf_new",
            c_name: "rask_json_buf_new",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_buf_add_string",
            c_name: "rask_json_buf_add_string",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_buf_add_i64",
            c_name: "rask_json_buf_add_i64",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_buf_add_f64",
            c_name: "rask_json_buf_add_f64",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_buf_add_bool",
            c_name: "rask_json_buf_add_bool",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_buf_add_raw",
            c_name: "rask_json_buf_add_raw",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_buf_finish",
            c_name: "rask_json_buf_finish",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_parse",
            c_name: "rask_json_parse",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_get_string",
            c_name: "rask_json_get_string",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_get_i64",
            c_name: "rask_json_get_i64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_get_f64",
            c_name: "rask_json_get_f64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_get_bool",
            c_name: "rask_json_get_bool",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I8),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "json_decode",
            c_name: "rask_json_decode",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Clone ────────────────────────────────────────────────────
        // Shallow copy for i64-sized values (scalars, handles)
        StdlibEntry {
            mir_name: "clone",
            c_name: "rask_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // Deep clone for collections — copies element bytes
        StdlibEntry {
            mir_name: "Vec_clone",
            c_name: "rask_vec_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Map_clone",
            c_name: "rask_map_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_clone",
            c_name: "rask_string_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // string.to_owned() → alias for clone
        StdlibEntry {
            mir_name: "string_to_owned",
            c_name: "rask_string_clone",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // string.parse<i32>() / parse<i64>() → int parsing
        StdlibEntry {
            mir_name: "string_parse_i32",
            c_name: "rask_string_parse_int",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_parse_i64",
            c_name: "rask_string_parse_int",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "string_parse_f64",
            c_name: "rask_string_parse_float",
            params: &[types::I64],
            ret_ty: Some(types::F64),
            can_panic: false,
        },

        // ── ThreadPool ─────────────────────────────────────────────
        // ThreadPool.spawn(closure) → task handle
        StdlibEntry {
            mir_name: "ThreadPool_spawn",
            c_name: "rask_threadpool_spawn",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // Thread.spawn(closure) → task handle
        StdlibEntry {
            mir_name: "Thread_spawn",
            c_name: "rask_closure_spawn",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // ThreadHandle.join() → result
        StdlibEntry {
            mir_name: "ThreadHandle_join",
            c_name: "rask_task_join_simple",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        // Thread_join alias (type prefix from Thread.spawn → Thread, not ThreadHandle)
        StdlibEntry {
            mir_name: "Thread_join",
            c_name: "rask_task_join_simple",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        // time.sleep(duration) — Duration is nanoseconds internally
        StdlibEntry {
            mir_name: "time_sleep",
            c_name: "rask_sleep_ns",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Concurrency: spawn/join/detach (green scheduler) ────────
        StdlibEntry {
            mir_name: "spawn",
            c_name: "rask_green_closure_spawn",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "join",
            c_name: "rask_green_join",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        StdlibEntry {
            mir_name: "detach",
            c_name: "rask_green_detach",
            params: &[types::I64],
            ret_ty: None,
            can_panic: true,
        },
        StdlibEntry {
            mir_name: "cancel",
            c_name: "rask_green_cancel",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        StdlibEntry {
            mir_name: "rask_task_cancelled",
            c_name: "rask_green_task_is_cancelled",
            params: &[],
            ret_ty: Some(types::I32),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_sleep_ns",
            c_name: "rask_green_sleep_ns",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },

        // ── Concurrency: runtime init/shutdown ───────────────────────
        StdlibEntry {
            mir_name: "rask_runtime_init",
            c_name: "rask_runtime_init",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_runtime_shutdown",
            c_name: "rask_runtime_shutdown",
            params: &[],
            ret_ty: None,
            can_panic: false,
        },

        // ── Concurrency: green spawn (poll-based state machine) ──────
        StdlibEntry {
            mir_name: "rask_green_spawn",
            c_name: "rask_green_spawn",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },

        // ── Concurrency: yield helpers ───────────────────────────────
        StdlibEntry {
            mir_name: "rask_yield",
            c_name: "rask_yield",
            params: &[],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_yield_timeout",
            c_name: "rask_yield_timeout",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_yield_read",
            c_name: "rask_yield_read",
            params: &[types::I32, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_yield_write",
            c_name: "rask_yield_write",
            params: &[types::I32, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_yield_accept",
            c_name: "rask_yield_accept",
            params: &[types::I32],
            ret_ty: None,
            can_panic: false,
        },

        // ── Async I/O (dual-path: green task or blocking) ─────────────
        StdlibEntry {
            mir_name: "rask_async_read",
            c_name: "rask_async_read",
            params: &[types::I32, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_async_write",
            c_name: "rask_async_write",
            params: &[types::I32, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_async_accept",
            c_name: "rask_async_accept",
            params: &[types::I32],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Async channels (yield-based) ─────────────────────────────
        StdlibEntry {
            mir_name: "rask_channel_send_async",
            c_name: "rask_channel_send_async",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_channel_recv_async",
            c_name: "rask_channel_recv_async",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },

        // ── Ensure hooks (LIFO cleanup) ──────────────────────────────
        StdlibEntry {
            mir_name: "rask_ensure_push",
            c_name: "rask_ensure_push",
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "rask_ensure_pop",
            c_name: "rask_ensure_pop",
            params: &[],
            ret_ty: None,
            can_panic: false,
        },

        // ── Memory allocation ─────────────────────────────────────────
        StdlibEntry {
            mir_name: "rask_alloc",
            c_name: "rask_alloc",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // ── Concurrency: channels ──────────────────────────────────
        // Channel construction (MIR produces Channel_buffered / Channel_unbuffered)
        StdlibEntry {
            mir_name: "Channel_buffered",
            c_name: "rask_channel_new_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Channel_unbuffered",
            c_name: "rask_channel_new_i64",
            params: &[types::I64],  // builder injects capacity=0
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        // Legacy name kept for backward compat
        StdlibEntry {
            mir_name: "Channel_new",
            c_name: "rask_channel_new_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "channel_tx",
            c_name: "rask_channel_get_tx",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "channel_rx",
            c_name: "rask_channel_get_rx",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },

        // Sender methods (qualified: Sender_send, etc.)
        StdlibEntry {
            mir_name: "Sender_send",
            c_name: "rask_channel_send_i64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Sender_try_send",
            c_name: "rask_channel_try_send_i64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Sender_clone",
            c_name: "rask_sender_clone_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Sender_drop",
            c_name: "rask_sender_drop_i64",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        // Legacy bare names
        StdlibEntry {
            mir_name: "send",
            c_name: "rask_channel_send_i64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "sender_clone",
            c_name: "rask_sender_clone_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "sender_drop",
            c_name: "rask_sender_drop_i64",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },

        // Receiver methods (qualified: Receiver_recv, etc.)
        StdlibEntry {
            mir_name: "Receiver_recv",
            c_name: "rask_channel_recv_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        StdlibEntry {
            mir_name: "Receiver_try_recv",
            c_name: "rask_channel_try_recv_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Receiver_drop",
            c_name: "rask_recver_drop_i64",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },
        // Legacy bare names
        StdlibEntry {
            mir_name: "recv",
            c_name: "rask_channel_recv_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
        },
        StdlibEntry {
            mir_name: "recver_drop",
            c_name: "rask_recver_drop_i64",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },

        // ── Concurrency: Shared<T> ──────────────────────────────────
        StdlibEntry {
            mir_name: "Shared_new",
            c_name: "rask_shared_new_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Shared_read",
            c_name: "rask_shared_read_i64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Shared_write",
            c_name: "rask_shared_write_i64",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Shared_clone",
            c_name: "rask_shared_clone_i64",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "Shared_drop",
            c_name: "rask_shared_drop_i64",
            params: &[types::I64],
            ret_ty: None,
            can_panic: false,
        },

        // ── Raw pointer operations ────────────────────────────
        StdlibEntry {
            mir_name: "RawPtr_add",
            c_name: "rask_ptr_add",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "RawPtr_sub",
            c_name: "rask_ptr_sub",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "RawPtr_offset",
            c_name: "rask_ptr_offset",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "RawPtr_read",
            c_name: "rask_ptr_read",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "RawPtr_write",
            c_name: "rask_ptr_write",
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "RawPtr_is_null",
            c_name: "rask_ptr_is_null",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "RawPtr_is_aligned",
            c_name: "rask_ptr_is_aligned",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "RawPtr_is_aligned_to",
            c_name: "rask_ptr_is_aligned_to",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
        StdlibEntry {
            mir_name: "RawPtr_align_offset",
            c_name: "rask_ptr_align_offset",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        },
    ];

    // ── Atomic operations ──────────────────────────────────
    // All integer atomics (I8..U64, Usize, Isize) share rask_atomic_int_* C functions.
    // AtomicBool uses rask_atomic_bool_* C functions.
    // Entries generated per type name so MIR qualified names resolve correctly.

    let int_atomic_types = [
        "AtomicI8", "AtomicU8", "AtomicI16", "AtomicU16",
        "AtomicI32", "AtomicU32", "AtomicI64", "AtomicU64",
        "AtomicUsize", "AtomicIsize",
    ];

    for ty in &int_atomic_types {
        // Construction
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_new", ty)),
            c_name: "rask_atomic_int_new",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_default", ty)),
            c_name: "rask_atomic_int_default",
            params: &[],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // Load / Store / Swap
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_load", ty)),
            c_name: "rask_atomic_int_load",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_store", ty)),
            c_name: "rask_atomic_int_store",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        });
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_swap", ty)),
            c_name: "rask_atomic_int_swap",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // CAS — (ptr, expected, desired, success_ord, fail_ord, out_ok_ptr)
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_compare_exchange", ty)),
            c_name: "rask_atomic_int_compare_exchange",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_compare_exchange_weak", ty)),
            c_name: "rask_atomic_int_compare_exchange_weak",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // Fetch operations — (ptr, val, ordering)
        for op in &[
            "fetch_add", "fetch_sub", "fetch_and", "fetch_or",
            "fetch_xor", "fetch_nand", "fetch_max", "fetch_min",
        ] {
            entries.push(StdlibEntry {
                mir_name: leak_str(&format!("{}_{}", ty, op)),
                c_name: leak_str(&format!("rask_atomic_int_{}", op)),
                params: &[types::I64, types::I64, types::I64],
                ret_ty: Some(types::I64),
                can_panic: false,
            });
        }
        // into_inner
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_into_inner", ty)),
            c_name: "rask_atomic_int_into_inner",
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
    }

    // AtomicBool — separate C functions
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_new", c_name: "rask_atomic_bool_new",
        params: &[types::I64], ret_ty: Some(types::I64),
        can_panic: false,
    });
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_default", c_name: "rask_atomic_bool_default",
        params: &[], ret_ty: Some(types::I64),
        can_panic: false,
    });
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_load", c_name: "rask_atomic_bool_load",
        params: &[types::I64, types::I64], ret_ty: Some(types::I64),
        can_panic: false,
    });
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_store", c_name: "rask_atomic_bool_store",
        params: &[types::I64, types::I64, types::I64], ret_ty: None,
        can_panic: false,
    });
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_swap", c_name: "rask_atomic_bool_swap",
        params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64),
        can_panic: false,
    });
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_compare_exchange", c_name: "rask_atomic_bool_compare_exchange",
        params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
        ret_ty: Some(types::I64),
        can_panic: false,
    });
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_compare_exchange_weak", c_name: "rask_atomic_bool_compare_exchange_weak",
        params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
        ret_ty: Some(types::I64),
        can_panic: false,
    });
    for op in &["fetch_and", "fetch_or", "fetch_xor", "fetch_nand"] {
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("AtomicBool_{}", op)),
            c_name: leak_str(&format!("rask_atomic_bool_{}", op)),
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
    }
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_into_inner", c_name: "rask_atomic_bool_into_inner",
        params: &[types::I64], ret_ty: Some(types::I64),
        can_panic: false,
    });

    // Fences
    entries.push(StdlibEntry {
        mir_name: "fence", c_name: "rask_fence",
        params: &[types::I64], ret_ty: None,
        can_panic: false,
    });
    entries.push(StdlibEntry {
        mir_name: "compiler_fence", c_name: "rask_compiler_fence",
        params: &[types::I64], ret_ty: None,
        can_panic: false,
    });

    // ── SIMD vector operations ──────────────────────────────
    // Float vector types: f32x4, f32x8, f64x2, f64x4
    // Scalar args/returns are F64 (ABI), vec args/returns are I64 (pointer).
    for simd_type in &["f32x4", "f32x8", "f64x2", "f64x4"] {
        // splat(scalar) → vec
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_splat", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_splat", simd_type)),
            params: &[types::F64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // load(src_ptr) → vec
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_load", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_load", simd_type)),
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // store(vec, dst_ptr) → void
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_store", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_store", simd_type)),
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        });
        // Binary: add, sub, mul, div(vec, vec) → vec
        for op in &["add", "sub", "mul", "div"] {
            entries.push(StdlibEntry {
                mir_name: leak_str(&format!("{}_{}", simd_type, op)),
                c_name: leak_str(&format!("rask_simd_{}_{}", simd_type, op)),
                params: &[types::I64, types::I64],
                ret_ty: Some(types::I64),
                can_panic: false,
            });
        }
        // scale(vec, scalar) → vec
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_scale", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_scale", simd_type)),
            params: &[types::I64, types::F64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // Reductions: sum, product, min, max(vec) → scalar
        for op in &["sum", "product", "min", "max"] {
            entries.push(StdlibEntry {
                mir_name: leak_str(&format!("{}_{}", simd_type, op)),
                c_name: leak_str(&format!("rask_simd_{}_{}", simd_type, op)),
                params: &[types::I64],
                ret_ty: Some(types::F64),
                can_panic: false,
            });
        }
        // get(vec, index) → scalar
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_get", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_get", simd_type)),
            params: &[types::I64, types::I64],
            ret_ty: Some(types::F64),
            can_panic: false,
        });
        // set(vec, index, val) → void
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_set", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_set", simd_type)),
            params: &[types::I64, types::I64, types::F64],
            ret_ty: None,
            can_panic: false,
        });
    }

    // Integer vector types: i32x4, i32x8
    // Scalar args/returns are I64, vec args/returns are I64 (pointer).
    for simd_type in &["i32x4", "i32x8"] {
        // splat(scalar) → vec
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_splat", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_splat", simd_type)),
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // load(src_ptr) → vec
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_load", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_load", simd_type)),
            params: &[types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // store(vec, dst_ptr) → void
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_store", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_store", simd_type)),
            params: &[types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        });
        // Binary: add, sub, mul, div(vec, vec) → vec
        for op in &["add", "sub", "mul", "div"] {
            entries.push(StdlibEntry {
                mir_name: leak_str(&format!("{}_{}", simd_type, op)),
                c_name: leak_str(&format!("rask_simd_{}_{}", simd_type, op)),
                params: &[types::I64, types::I64],
                ret_ty: Some(types::I64),
                can_panic: false,
            });
        }
        // scale(vec, scalar) → vec
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_scale", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_scale", simd_type)),
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // Reductions: sum, product, min, max(vec) → scalar
        for op in &["sum", "product", "min", "max"] {
            entries.push(StdlibEntry {
                mir_name: leak_str(&format!("{}_{}", simd_type, op)),
                c_name: leak_str(&format!("rask_simd_{}_{}", simd_type, op)),
                params: &[types::I64],
                ret_ty: Some(types::I64),
                can_panic: false,
            });
        }
        // get(vec, index) → scalar
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_get", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_get", simd_type)),
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: false,
        });
        // set(vec, index, val) → void
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_set", simd_type)),
            c_name: leak_str(&format!("rask_simd_{}_set", simd_type)),
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
            can_panic: false,
        });
    }

    entries
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

/// Build the set of MIR function names that can panic at runtime.
pub fn panicking_functions() -> HashSet<String> {
    stdlib_entries()
        .into_iter()
        .filter(|e| e.can_panic)
        .map(|e| e.mir_name.to_string())
        .collect()
}
