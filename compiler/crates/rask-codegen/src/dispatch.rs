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

/// How to adapt arguments before a stdlib call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgAdapt {
    /// Pass args as-is
    None,
    /// Inject elem_size=8 as first arg when args empty (Vec_new)
    InjectOneSize,
    /// Inject key_size=8, val_size=8 when args empty (Map_new)
    InjectTwoSizes,
    /// Wrap args[1] as pointer (skip if string)
    WrapArg1,
    /// Wrap args[2] as pointer (skip if string)
    WrapArg2,
    /// Wrap args[1] and args[2] as pointers
    WrapArg1And2,
    /// Inject 16-byte string out-param as first arg
    StringOutParam,
    /// Copy 16 bytes to dst then RC inc (string_clone/string_to_owned)
    StringClone,
    /// In-place string mutation: out-param IS the self string
    InPlaceStringMut,
    /// Append 8-byte (or 16-byte for string dst) out-param
    AppendOutParam,
    /// Append iconst(0) (Channel_unbuffered capacity)
    AppendZero,
    /// Append iconst(8) as elem_size (Shared_read/write)
    AppendElemSize,
    /// Complex case handled by hand-written code
    Custom,
}

/// How to adapt the return value after a stdlib call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RetAdapt {
    /// Use return value as-is (or no return)
    None,
    /// Load i64 from void* (or copy 16B string if dst is string type)
    DerefOrString,
    /// NULL→None(tag=1), non-NULL→Some(tag=0, deref)
    DerefOption,
    /// Determined by ArgAdapt (StringOutParam → slot addr, AppendOutParam → slot load)
    FromArgAdapt,
}

/// A stdlib function entry: MIR name → C runtime function + adaptation.
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
    /// How to adapt arguments before the call
    pub arg_adapt: ArgAdapt,
    /// How to adapt the return value after the call
    pub ret_adapt: RetAdapt,
}

impl StdlibEntry {
    /// Shorthand for entries that need no call adaptation (the common case).
    const fn simple(
        mir_name: &'static str,
        c_name: &'static str,
        params: &'static [Type],
        ret_ty: Option<Type>,
        can_panic: bool,
    ) -> Self {
        Self { mir_name, c_name, params, ret_ty, can_panic, arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None }
    }
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
        StdlibEntry {
            mir_name: "Vec_new", c_name: "rask_vec_new",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::InjectOneSize, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("rask_vec_from_static", "rask_vec_from_static", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_from", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_free", "rask_vec_free", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "Vec_push", c_name: "rask_vec_push",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Vec_pop", c_name: "rask_vec_pop",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::AppendOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("Vec_len", "rask_vec_len", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_as_ptr", "rask_vec_as_ptr", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "Vec_get", c_name: "rask_vec_get",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Vec_get_unchecked", c_name: "rask_vec_get_unchecked",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Vec_set", c_name: "rask_vec_set",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: true,
            arg_adapt: ArgAdapt::WrapArg2, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Vec_clear", "rask_vec_clear", &[types::I64], None, false),
        StdlibEntry::simple("Vec_is_empty", "rask_vec_is_empty", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_capacity", "rask_vec_capacity", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "Vec_insert", c_name: "rask_vec_insert_at",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::WrapArg2, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Vec_remove", c_name: "rask_vec_remove_at",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::AppendOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Vec_remove_at", c_name: "rask_vec_remove_at",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::AppendOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // ── Subscript (desugared from args[0] → args.index(0)) ─
        StdlibEntry {
            mir_name: "index", c_name: "rask_vec_get",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Vec_index", c_name: "rask_vec_get",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },

        StdlibEntry::simple("Vec_slice", "rask_vec_slice", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_chunks", "rask_vec_chunks", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_to_vec", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "Vec_join", c_name: "rask_vec_join",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Vec_join_i64", c_name: "rask_vec_join_i64",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("Vec_sort", "rask_vec_sort", &[types::I64], None, false),
        StdlibEntry::simple("Vec_sort_by", "rask_vec_sort_by", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("Vec_reverse", "rask_vec_reverse", &[types::I64], None, false),
        StdlibEntry::simple("Vec_contains", "rask_vec_contains", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_dedup", "rask_vec_dedup", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "Vec_first", c_name: "rask_vec_first",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Vec_last", c_name: "rask_vec_last",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },

        // ── Iterator runtime support ──────────────────────────────
        StdlibEntry::simple("Vec_skip", "rask_iter_skip", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_map", "rask_vec_map", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_collect", "rask_vec_collect", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_filter", "rask_vec_filter", &[types::I64, types::I64], Some(types::I64), false),

        // ── String operations ──────────────────────────────────
        StdlibEntry::simple("string_free", "rask_string_free", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "string_clone", c_name: "rask_string_clone",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringClone, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_to_owned", c_name: "rask_string_clone",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringClone, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // Error origin (ER15/ER16)
        StdlibEntry {
            mir_name: "rask_result_origin", c_name: "rask_result_origin",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // Constructors (out-param)
        StdlibEntry {
            mir_name: "string_new", c_name: "rask_string_new",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_from", c_name: "rask_string_from",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_from_c", c_name: "rask_string_from",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_from_raw", c_name: "rask_string_from_bytes",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // Read-only accessors
        StdlibEntry::simple("string_len", "rask_string_len", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_eq", "rask_string_eq", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_as_ptr", "rask_string_ptr", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_as_c_str", "rask_string_ptr", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_is_empty", "rask_string_is_empty", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_find", "rask_string_find", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_index_of", "rask_string_find", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_rfind", "rask_string_rfind", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_char_at", "rask_string_char_at", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_starts_with", "rask_string_starts_with", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_ends_with", "rask_string_ends_with", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_contains", "rask_string_contains", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_byte_at", "rask_string_byte_at", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_parse", "rask_string_parse_float", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("string_parse_int", "rask_string_parse_int", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_parse_float", "rask_string_parse_float", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("string_parse_i32", "rask_string_parse_int", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_parse_i64", "rask_string_parse_int", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_parse_f64", "rask_string_parse_float", &[types::I64], Some(types::F64), false),

        // String-producing operations (out-param)
        StdlibEntry {
            mir_name: "concat", c_name: "rask_string_concat",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_substr", c_name: "rask_string_substr",
            params: &[types::I64, types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_to_lowercase", c_name: "rask_string_to_lowercase",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_to_uppercase", c_name: "rask_string_to_uppercase",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_trim", c_name: "rask_string_trim",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_trim_start", c_name: "rask_string_trim_start",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_trim_end", c_name: "rask_string_trim_end",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_repeat", c_name: "rask_string_repeat",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_reverse", c_name: "rask_string_reverse",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_replace", c_name: "rask_string_replace",
            params: &[types::I64, types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_append", c_name: "rask_string_append",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_append_cstr", c_name: "rask_string_append_cstr",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // Vec-returning string operations (no out-param needed)
        StdlibEntry::simple("string_lines", "rask_string_lines", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_split", "rask_string_split", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_split_whitespace", "rask_string_split_whitespace", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_chars", "rask_string_chars", &[types::I64], Some(types::I64), false),

        // ── Conversion to string (out-param) ──────────────────
        StdlibEntry {
            mir_name: "i64_to_string", c_name: "rask_i64_to_string",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "bool_to_string", c_name: "rask_bool_to_string",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "f64_to_string", c_name: "rask_f64_to_string",
            params: &[types::I64, types::F64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "char_to_string", c_name: "rask_char_to_string",
            params: &[types::I64, types::I32], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // ── Math operations ────────────────────────────────────
        StdlibEntry::simple("sqrt", "sqrt", &[types::F64], Some(types::F64), false),
        StdlibEntry::simple("f64_sqrt", "sqrt", &[types::F64], Some(types::F64), false),
        StdlibEntry::simple("f32_sqrt", "sqrtf", &[types::F32], Some(types::F32), false),
        StdlibEntry::simple("abs", "fabs", &[types::F64], Some(types::F64), false),
        StdlibEntry::simple("f64_powf", "pow", &[types::F64, types::F64], Some(types::F64), false),
        StdlibEntry::simple("f64_powi", "pow", &[types::F64, types::F64], Some(types::F64), false),

        // String comparison
        StdlibEntry::simple("string_compare", "rask_string_compare", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_lt", "rask_string_lt", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_gt", "rask_string_gt", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_le", "rask_string_le", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_ge", "rask_string_ge", &[types::I64, types::I64], Some(types::I64), false),

        // In-place string mutation — C signature: fn(out, self, arg)
        StdlibEntry {
            mir_name: "string_push_str", c_name: "rask_string_push_str",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::InPlaceStringMut, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_push_char", c_name: "rask_string_push_char",
            params: &[types::I64, types::I64, types::I32], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::InPlaceStringMut, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_push", c_name: "rask_string_push_char",
            params: &[types::I64, types::I64, types::I32], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::InPlaceStringMut, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("fs_list_dir", "rask_fs_list_dir", &[types::I64], Some(types::I64), false),

        // ── Map operations ─────────────────────────────────────
        StdlibEntry::simple("Map_free", "rask_map_free", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "Map_new", c_name: "rask_map_new",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::InjectTwoSizes, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_new_string_keys", c_name: "rask_map_new_string_keys",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::InjectTwoSizes, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Map_from", "rask_map_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "Map_insert", c_name: "rask_map_insert",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1And2, ret_adapt: RetAdapt::None,
        },
        // LP13: for mutate writeback — insert/replace value by key (same as Map_insert)
        StdlibEntry {
            mir_name: "Map_set", c_name: "rask_map_insert",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1And2, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_contains_key", c_name: "rask_map_contains",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_get", c_name: "rask_map_get",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry {
            mir_name: "Map_get_unwrap", c_name: "rask_map_get_unwrap",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Map_remove", c_name: "rask_map_remove",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Map_len", "rask_map_len", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Map_is_empty", "rask_map_is_empty", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Map_clear", "rask_map_clear", &[types::I64], None, false),
        StdlibEntry::simple("Map_keys", "rask_map_keys", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Map_values", "rask_map_values", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Map_iter", "rask_map_entries", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Map_entries", "rask_map_entries", &[types::I64], Some(types::I64), false),

        // ── Pool operations ────────────────────────────────────
        StdlibEntry::simple("Pool_free", "rask_pool_free", &[types::I64], None, false),
        StdlibEntry::simple("Pool_new", "rask_pool_new", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_alloc", "rask_pool_alloc_packed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_remove", "rask_pool_remove_packed", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "Pool_get", c_name: "rask_pool_get_packed",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry::simple("Pool_index", "rask_pool_get_packed", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_handles", "rask_pool_handles_packed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_values", "rask_pool_values", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_len", "rask_pool_len", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_is_empty", "rask_pool_is_empty", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_cursor", "rask_pool_handles_packed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_contains", "rask_pool_is_valid_packed", &[types::I64, types::I64], Some(types::I64), false),
        // LP13: for mutate writeback — write value to existing pool slot
        StdlibEntry {
            mir_name: "Pool_set", c_name: "rask_pool_set_packed",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::WrapArg2, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Pool_insert", c_name: "rask_pool_insert_packed_sized",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Pool_drain", "rask_pool_drain", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_checked_access", "rask_pool_get_packed", &[types::I64, types::I64], Some(types::I64), false),

        // ── Rng operations ────────────────────────────────────────
        StdlibEntry::simple("Rng_new", "rask_rng_new", &[], Some(types::I64), false),
        StdlibEntry::simple("Rng_from_seed", "rask_rng_from_seed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Rng_u64", "rask_rng_u64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Rng_i64", "rask_rng_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Rng_f64", "rask_rng_f64", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("Rng_f32", "rask_rng_f32", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("Rng_bool", "rask_rng_bool", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Rng_range", "rask_rng_range", &[types::I64, types::I64, types::I64], Some(types::I64), true),

        // ── Random module convenience functions ───────────────────
        StdlibEntry::simple("random_f64", "rask_random_f64", &[], Some(types::F64), false),
        StdlibEntry::simple("random_f32", "rask_random_f32", &[], Some(types::F64), false),
        StdlibEntry::simple("random_i64", "rask_random_i64", &[], Some(types::I64), false),
        StdlibEntry::simple("random_bool", "rask_random_bool", &[], Some(types::I64), false),
        StdlibEntry::simple("random_range", "rask_random_range", &[types::I64, types::I64], Some(types::I64), true),

        // ── File instance methods ─────────────────────────────────
        StdlibEntry::simple("File_close", "rask_file_close", &[types::I64], None, false),
        StdlibEntry::simple("File_read_all", "rask_file_read_all", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("File_read_text", "rask_file_read_all", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("File_write", "rask_file_write", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("File_write_all", "rask_file_write_all", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("File_write_line", "rask_file_write_line", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("File_lines", "rask_file_lines", &[types::I64], Some(types::I64), false),

        // ── Stdlib module calls ─────────────────────────────────
        StdlibEntry::simple("cli_args", "rask_cli_args", &[], Some(types::I64), false),
        StdlibEntry::simple("cli_parse", "rask_args_parse", &[], Some(types::I64), false),
        StdlibEntry::simple("Args_flag", "rask_args_flag", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "Args_option", c_name: "rask_args_option",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry {
            mir_name: "Args_option_or", c_name: "rask_args_option_or",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("Args_positional", "rask_args_positional", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Args_program", "rask_args_program", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("std_exit", "rask_exit", &[types::I64], None, false),
        StdlibEntry::simple("fs_read_lines", "rask_fs_read_lines", &[types::I64], Some(types::I64), false),

        // ── IO module ───────────────────────────────────────────
        StdlibEntry {
            mir_name: "io_read_line", c_name: "rask_io_read_line",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // ── FS module ───────────────────────────────────────────
        // Self-hosted from stdlib/fs.rk. Remaining C runtime stubs:
        StdlibEntry::simple("fs_write_bytes", "rask_fs_write_bytes", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("fs_create_dir_all", "rask_fs_create_dir_all", &[types::I64], None, false),
        StdlibEntry::simple("fs_open", "rask_fs_open", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("fs_create", "rask_fs_create", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("fs_metadata", "rask_fs_metadata", &[types::I64], Some(types::I64), false),

        // ── Metadata methods ────────────────────────────────────────
        StdlibEntry::simple("Metadata_size", "rask_metadata_size", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Metadata_accessed", "rask_metadata_accessed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Metadata_modified", "rask_metadata_modified", &[types::I64], Some(types::I64), false),

        // ── Time module ─────────────────────────────────────────────
        StdlibEntry::simple("Instant_now", "rask_time_Instant_now", &[], Some(types::I64), false),
        StdlibEntry::simple("Instant_elapsed", "rask_time_Instant_elapsed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Instant_duration_since", "rask_time_Instant_duration_since", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_from_nanos", "rask_time_Duration_from_nanos", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_from_millis", "rask_time_Duration_from_millis", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_nanos", "rask_time_Duration_as_nanos", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_secs", "rask_time_Duration_as_secs", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_secs_f64", "rask_time_Duration_as_secs_f64", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("Duration_as_millis", "rask_time_Duration_as_millis", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_micros", "rask_time_Duration_as_micros", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_secs_f32", "rask_time_Duration_as_secs_f32", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("Duration_seconds", "rask_time_Duration_seconds", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_millis", "rask_time_Duration_millis", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_micros", "rask_time_Duration_micros", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_nanos", "rask_time_Duration_nanos", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_from_secs_f64", "rask_time_Duration_from_secs_f64", &[types::F64], Some(types::I64), false),

        // ── I/O primitives ─────────────────────────────────────────
        StdlibEntry {
            mir_name: "io_read_string", c_name: "rask_io_read_until_close",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "rask_io_read_string", c_name: "rask_io_read_string",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "rask_io_read_until_close", c_name: "rask_io_read_until_close",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("io_write_string", "rask_io_write_string", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("io_close_fd", "rask_io_close_fd", &[types::I64], None, false),

        // ── Net module ──────────────────────────────────────────────
        StdlibEntry::simple("net_tcp_listen", "rask_net_tcp_listen", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("net_tcp_connect", "rask_net_tcp_connect", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpListener_accept", "rask_net_tcp_accept", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpListener_close", "rask_net_close", &[types::I64], None, false),
        StdlibEntry::simple("TcpListener_clone", "rask_net_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "TcpConnection_read_all", c_name: "rask_net_read_all",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::AppendOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("TcpConnection_write_all", "rask_net_write_all", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "TcpConnection_remote_addr", c_name: "rask_net_remote_addr",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("TcpConnection_read_http_request", "rask_net_read_http_request", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpConnection_write_http_response", "rask_net_write_http_response", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpConnection_close", "rask_net_close", &[types::I64], None, false),
        StdlibEntry::simple("TcpConnection_clone", "rask_net_clone", &[types::I64], Some(types::I64), false),

        // ── HTTP server close (linear resource cleanup) ─────────────
        StdlibEntry::simple("HttpServer_close", "rask_http_server_close", &[types::I64], None, false),

        // ── StringBuilder ───────────────────────────────────────────
        StdlibEntry::simple("StringBuilder_new", "rask_string_builder_new", &[], Some(types::I64), false),
        StdlibEntry::simple("StringBuilder_with_capacity", "rask_string_builder_with_capacity", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("StringBuilder_append", "rask_string_builder_append", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("StringBuilder_append_char", "rask_string_builder_append_char", &[types::I64, types::I64], None, false),
        StdlibEntry {
            mir_name: "StringBuilder_build", c_name: "rask_string_builder_build",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("StringBuilder_len", "rask_string_builder_len", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("StringBuilder_is_empty", "rask_string_builder_is_empty", &[types::I64], Some(types::I64), false),

        // ── JSON module ─────────────────────────────────────────────
        StdlibEntry {
            mir_name: "json_encode", c_name: "rask_json_encode",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "json_encode_string", c_name: "rask_json_encode_string",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "json_encode_i64", c_name: "rask_json_encode_i64",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("json_buf_new", "rask_json_buf_new", &[], Some(types::I64), false),
        StdlibEntry::simple("json_buf_add_string", "rask_json_buf_add_string", &[types::I64, types::I64, types::I64], None, false),
        StdlibEntry::simple("json_buf_add_i64", "rask_json_buf_add_i64", &[types::I64, types::I64, types::I64], None, false),
        StdlibEntry::simple("json_buf_add_f64", "rask_json_buf_add_f64", &[types::I64, types::I64, types::I64], None, false),
        StdlibEntry::simple("json_buf_add_bool", "rask_json_buf_add_bool", &[types::I64, types::I64, types::I64], None, false),
        StdlibEntry::simple("json_buf_add_raw", "rask_json_buf_add_raw", &[types::I64, types::I64, types::I64], None, false),
        StdlibEntry {
            mir_name: "json_buf_finish", c_name: "rask_json_buf_finish",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("json_buf_new_array", "rask_json_buf_new_array", &[], Some(types::I64), false),
        StdlibEntry::simple("json_buf_array_add_raw", "rask_json_buf_array_add_raw", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("json_buf_array_add_string", "rask_json_buf_array_add_string", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("json_buf_array_add_i64", "rask_json_buf_array_add_i64", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("json_buf_array_add_f64", "rask_json_buf_array_add_f64", &[types::I64, types::F64], None, false),
        StdlibEntry::simple("json_buf_array_add_bool", "rask_json_buf_array_add_bool", &[types::I64, types::I64], None, false),
        StdlibEntry {
            mir_name: "json_buf_finish_array", c_name: "rask_json_buf_finish_array",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("json_parse", "rask_json_parse", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "json_get_string", c_name: "rask_json_get_string",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("json_get_i64", "rask_json_get_i64", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_get_f64", "rask_json_get_f64", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_get_bool", "rask_json_get_bool", &[types::I64, types::I64], Some(types::I8), false),
        StdlibEntry::simple("json_decode", "rask_json_decode", &[types::I64], Some(types::I64), false),

        // ── Clone ────────────────────────────────────────────────────
        StdlibEntry::simple("clone", "rask_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_clone", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Map_clone", "rask_map_clone", &[types::I64], Some(types::I64), false),

        // ── ThreadPool ─────────────────────────────────────────────
        StdlibEntry::simple("ThreadPool_spawn", "rask_threadpool_spawn", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Thread_spawn", "rask_closure_spawn", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("ThreadHandle_join", "rask_task_join_simple", &[types::I64], Some(types::I64), true),
        StdlibEntry::simple("Thread_join", "rask_task_join_simple", &[types::I64], Some(types::I64), true),
        StdlibEntry::simple("ThreadHandle_detach", "rask_task_detach", &[types::I64], None, false),
        StdlibEntry::simple("Thread_detach", "rask_task_detach", &[types::I64], None, false),
        StdlibEntry::simple("time_sleep", "rask_sleep_ns", &[types::I64], Some(types::I64), false),

        // ── Concurrency: spawn/join/detach (green scheduler) ────────
        StdlibEntry::simple("spawn", "rask_green_closure_spawn", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("join", "rask_green_join", &[types::I64], Some(types::I64), true),
        StdlibEntry::simple("detach", "rask_green_detach", &[types::I64], None, true),
        StdlibEntry::simple("cancel", "rask_green_cancel", &[types::I64], Some(types::I64), true),
        // TaskHandle qualified names (same C functions as unqualified)
        StdlibEntry::simple("TaskHandle_join", "rask_green_join", &[types::I64], Some(types::I64), true),
        StdlibEntry::simple("TaskHandle_detach", "rask_green_detach", &[types::I64], None, true),
        StdlibEntry::simple("TaskHandle_cancel", "rask_green_cancel", &[types::I64], Some(types::I64), true),
        StdlibEntry::simple("rask_task_cancelled", "rask_green_task_is_cancelled", &[], Some(types::I32), false),
        StdlibEntry::simple("rask_sleep_ns", "rask_green_sleep_ns", &[types::I64], None, false),

        // ── Concurrency: runtime init/shutdown ───────────────────────
        StdlibEntry::simple("rask_runtime_init", "rask_runtime_init", &[types::I64], None, false),
        StdlibEntry::simple("rask_runtime_shutdown", "rask_runtime_shutdown", &[], None, false),
        StdlibEntry::simple("rask_green_spawn", "rask_green_spawn", &[types::I64, types::I64, types::I64], Some(types::I64), true),

        // ── Concurrency: yield helpers ───────────────────────────────
        StdlibEntry::simple("rask_yield", "rask_yield", &[], None, false),
        StdlibEntry::simple("rask_yield_timeout", "rask_yield_timeout", &[types::I64], None, false),
        StdlibEntry::simple("rask_yield_read", "rask_yield_read", &[types::I32, types::I64, types::I64], None, false),
        StdlibEntry::simple("rask_yield_write", "rask_yield_write", &[types::I32, types::I64, types::I64], None, false),
        StdlibEntry::simple("rask_yield_accept", "rask_yield_accept", &[types::I32], None, false),

        // ── Async I/O ─────────────────────────────────────────────────
        StdlibEntry::simple("rask_async_read", "rask_async_read", &[types::I32, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("rask_async_write", "rask_async_write", &[types::I32, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("rask_async_accept", "rask_async_accept", &[types::I32], Some(types::I64), false),

        // ── Async channels ─────────────────────────────────────────
        StdlibEntry::simple("rask_channel_send_async", "rask_channel_send_async", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("rask_channel_recv_async", "rask_channel_recv_async", &[types::I64], Some(types::I64), true),

        // ── Ensure hooks ──────────────────────────────────────────
        StdlibEntry::simple("rask_ensure_push", "rask_ensure_push", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("rask_ensure_pop", "rask_ensure_pop", &[], None, false),

        // ── Resource tracking (C1/C2 consumption cancellation) ───
        StdlibEntry::simple("rask_resource_register", "rask_resource_register", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("rask_resource_consume", "rask_resource_consume", &[types::I64], None, false),
        StdlibEntry::simple("rask_resource_is_consumed", "rask_resource_is_consumed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("rask_resource_scope_check", "rask_resource_scope_check", &[types::I64], None, false),

        // ── Memory allocation ─────────────────────────────────────
        StdlibEntry::simple("rask_alloc", "rask_alloc", &[types::I64], Some(types::I64), false),

        // ── Concurrency: channels ──────────────────────────────────
        StdlibEntry::simple("Channel_buffered", "rask_channel_new_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "Channel_unbuffered", c_name: "rask_channel_new_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::AppendZero, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Channel_new", "rask_channel_new_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("channel_tx", "rask_channel_get_tx", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("channel_rx", "rask_channel_get_rx", &[types::I64], Some(types::I64), false),

        // Sender methods
        StdlibEntry {
            mir_name: "Sender_send", c_name: "rask_channel_send_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Sender_try_send", "rask_channel_try_send_i64", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Sender_close", "rask_sender_close_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Sender_clone", "rask_sender_clone_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Sender_drop", "rask_sender_drop_i64", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "send", c_name: "rask_channel_send_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("sender_clone", "rask_sender_clone_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("sender_drop", "rask_sender_drop_i64", &[types::I64], None, false),

        // Receiver methods
        StdlibEntry::simple("Receiver_recv", "rask_channel_recv_i64", &[types::I64], Some(types::I64), true),
        StdlibEntry {
            mir_name: "Receiver_recv_struct", c_name: "rask_channel_recv_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Receiver_try_recv", "rask_channel_try_recv_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Receiver_close", "rask_recver_close_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Receiver_drop", "rask_recver_drop_i64", &[types::I64], None, false),
        StdlibEntry::simple("recv", "rask_channel_recv_i64", &[types::I64], Some(types::I64), true),
        StdlibEntry::simple("recver_drop", "rask_recver_drop_i64", &[types::I64], None, false),

        // ── Concurrency: Shared<T> ──────────────────────────────────
        StdlibEntry {
            mir_name: "Shared_new", c_name: "rask_shared_new_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Shared_read", "rask_shared_read_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_write", "rask_shared_write_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_try_read", "rask_shared_try_read_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_try_write", "rask_shared_try_write_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_clone", "rask_shared_clone_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_drop", "rask_shared_drop_i64", &[types::I64], None, false),

        // ── Concurrency: Mutex<T> ──────────────────────────────────
        StdlibEntry {
            mir_name: "Mutex_new", c_name: "rask_mutex_new_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Mutex_lock", "rask_mutex_lock_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Mutex_try_lock", "rask_mutex_try_lock_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Mutex_clone", "rask_mutex_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Mutex_drop", "rask_mutex_drop", &[types::I64], None, false),

        // ── Char predicates ───────────────────────────────────
        StdlibEntry::simple("char_is_digit", "rask_char_is_digit", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_ascii", "rask_char_is_ascii", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_alphabetic", "rask_char_is_alphabetic", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_numeric", "rask_char_is_numeric", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_alphanumeric", "rask_char_is_alphanumeric", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_whitespace", "rask_char_is_whitespace", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_uppercase", "rask_char_is_uppercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_lowercase", "rask_char_is_lowercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_to_int", "rask_char_to_int", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_to_uppercase", "rask_char_to_uppercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_to_lowercase", "rask_char_to_lowercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_len_utf8", "rask_char_len_utf8", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_eq", "rask_char_eq", &[types::I32, types::I32], Some(types::I64), false),

        // ── Path operations ──────────────────────────────────
        // Path = RaskStr. Constructors/conversions use StringOutParam.
        // Option-returning methods return NULL (None) or &thread_local (Some).
        StdlibEntry {
            mir_name: "Path_new", c_name: "rask_path_new",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Path_from", c_name: "rask_path_new",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Path_to_string", c_name: "rask_path_to_string",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Path_join", c_name: "rask_path_join",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Path_with_extension", c_name: "rask_path_with_extension",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Path_with_file_name", c_name: "rask_path_with_file_name",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Path_parent", c_name: "rask_path_parent",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry {
            mir_name: "Path_file_name", c_name: "rask_path_file_name",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry {
            mir_name: "Path_extension", c_name: "rask_path_extension",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry {
            mir_name: "Path_stem", c_name: "rask_path_stem",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry::simple("Path_is_absolute", "rask_path_is_absolute", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Path_is_relative", "rask_path_is_relative", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Path_has_extension", "rask_path_has_extension", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Path_components", "rask_path_components", &[types::I64], Some(types::I64), false),

        // ── Raw pointer operations ────────────────────────────
        StdlibEntry::simple("RawPtr_add", "rask_ptr_add", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("RawPtr_sub", "rask_ptr_sub", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("RawPtr_offset", "rask_ptr_offset", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("RawPtr_read", "rask_ptr_read", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("RawPtr_write", "rask_ptr_write", &[types::I64, types::I64, types::I64], None, false),
        StdlibEntry::simple("RawPtr_is_null", "rask_ptr_is_null", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("RawPtr_is_aligned", "rask_ptr_is_aligned", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("RawPtr_is_aligned_to", "rask_ptr_is_aligned_to", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("RawPtr_align_offset", "rask_ptr_align_offset", &[types::I64, types::I64], Some(types::I64), false),
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
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_new", ty)), "rask_atomic_int_new", &[types::I64], Some(types::I64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_default", ty)), "rask_atomic_int_default", &[], Some(types::I64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_load", ty)), "rask_atomic_int_load", &[types::I64, types::I64], Some(types::I64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_store", ty)), "rask_atomic_int_store", &[types::I64, types::I64, types::I64], None, false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_swap", ty)), "rask_atomic_int_swap", &[types::I64, types::I64, types::I64], Some(types::I64), false));
        // CAS — Custom adaptation (appends out_ok pointer)
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_compare_exchange", ty)),
            c_name: "rask_atomic_int_compare_exchange",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        });
        entries.push(StdlibEntry {
            mir_name: leak_str(&format!("{}_compare_exchange_weak", ty)),
            c_name: "rask_atomic_int_compare_exchange_weak",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        });
        for op in &["fetch_add", "fetch_sub", "fetch_and", "fetch_or", "fetch_xor", "fetch_nand", "fetch_max", "fetch_min"] {
            entries.push(StdlibEntry::simple(leak_str(&format!("{}_{}", ty, op)), leak_str(&format!("rask_atomic_int_{}", op)), &[types::I64, types::I64, types::I64], Some(types::I64), false));
        }
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_into_inner", ty)), "rask_atomic_int_into_inner", &[types::I64], Some(types::I64), false));
    }

    // AtomicBool — separate C functions
    entries.push(StdlibEntry::simple("AtomicBool_new", "rask_atomic_bool_new", &[types::I64], Some(types::I64), false));
    entries.push(StdlibEntry::simple("AtomicBool_default", "rask_atomic_bool_default", &[], Some(types::I64), false));
    entries.push(StdlibEntry::simple("AtomicBool_load", "rask_atomic_bool_load", &[types::I64, types::I64], Some(types::I64), false));
    entries.push(StdlibEntry::simple("AtomicBool_store", "rask_atomic_bool_store", &[types::I64, types::I64, types::I64], None, false));
    entries.push(StdlibEntry::simple("AtomicBool_swap", "rask_atomic_bool_swap", &[types::I64, types::I64, types::I64], Some(types::I64), false));
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_compare_exchange", c_name: "rask_atomic_bool_compare_exchange",
        params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
        ret_ty: Some(types::I64), can_panic: false,
        arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
    });
    entries.push(StdlibEntry {
        mir_name: "AtomicBool_compare_exchange_weak", c_name: "rask_atomic_bool_compare_exchange_weak",
        params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
        ret_ty: Some(types::I64), can_panic: false,
        arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
    });
    for op in &["fetch_and", "fetch_or", "fetch_xor", "fetch_nand"] {
        entries.push(StdlibEntry::simple(leak_str(&format!("AtomicBool_{}", op)), leak_str(&format!("rask_atomic_bool_{}", op)), &[types::I64, types::I64, types::I64], Some(types::I64), false));
    }
    entries.push(StdlibEntry::simple("AtomicBool_into_inner", "rask_atomic_bool_into_inner", &[types::I64], Some(types::I64), false));

    // Fences
    entries.push(StdlibEntry::simple("fence", "rask_fence", &[types::I64], None, false));
    entries.push(StdlibEntry::simple("compiler_fence", "rask_compiler_fence", &[types::I64], None, false));

    // ── SIMD vector operations ──────────────────────────────
    // Float vector types: f32x4, f32x8, f64x2, f64x4
    // Scalar args/returns are F64 (ABI), vec args/returns are I64 (pointer).
    for simd_type in &["f32x4", "f32x8", "f64x2", "f64x4"] {
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_splat", simd_type)), leak_str(&format!("rask_simd_{}_splat", simd_type)), &[types::F64], Some(types::I64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_load", simd_type)), leak_str(&format!("rask_simd_{}_load", simd_type)), &[types::I64], Some(types::I64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_store", simd_type)), leak_str(&format!("rask_simd_{}_store", simd_type)), &[types::I64, types::I64], None, false));
        for op in &["add", "sub", "mul", "div"] {
            entries.push(StdlibEntry::simple(leak_str(&format!("{}_{}", simd_type, op)), leak_str(&format!("rask_simd_{}_{}", simd_type, op)), &[types::I64, types::I64], Some(types::I64), false));
        }
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_scale", simd_type)), leak_str(&format!("rask_simd_{}_scale", simd_type)), &[types::I64, types::F64], Some(types::I64), false));
        for op in &["sum", "product", "min", "max"] {
            entries.push(StdlibEntry::simple(leak_str(&format!("{}_{}", simd_type, op)), leak_str(&format!("rask_simd_{}_{}", simd_type, op)), &[types::I64], Some(types::F64), false));
        }
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_get", simd_type)), leak_str(&format!("rask_simd_{}_get", simd_type)), &[types::I64, types::I64], Some(types::F64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_set", simd_type)), leak_str(&format!("rask_simd_{}_set", simd_type)), &[types::I64, types::I64, types::F64], None, false));
    }

    for simd_type in &["i32x4", "i32x8"] {
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_splat", simd_type)), leak_str(&format!("rask_simd_{}_splat", simd_type)), &[types::I64], Some(types::I64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_load", simd_type)), leak_str(&format!("rask_simd_{}_load", simd_type)), &[types::I64], Some(types::I64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_store", simd_type)), leak_str(&format!("rask_simd_{}_store", simd_type)), &[types::I64, types::I64], None, false));
        for op in &["add", "sub", "mul", "div"] {
            entries.push(StdlibEntry::simple(leak_str(&format!("{}_{}", simd_type, op)), leak_str(&format!("rask_simd_{}_{}", simd_type, op)), &[types::I64, types::I64], Some(types::I64), false));
        }
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_scale", simd_type)), leak_str(&format!("rask_simd_{}_scale", simd_type)), &[types::I64, types::I64], Some(types::I64), false));
        for op in &["sum", "product", "min", "max"] {
            entries.push(StdlibEntry::simple(leak_str(&format!("{}_{}", simd_type, op)), leak_str(&format!("rask_simd_{}_{}", simd_type, op)), &[types::I64], Some(types::I64), false));
        }
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_get", simd_type)), leak_str(&format!("rask_simd_{}_get", simd_type)), &[types::I64, types::I64], Some(types::I64), false));
        entries.push(StdlibEntry::simple(leak_str(&format!("{}_set", simd_type)), leak_str(&format!("rask_simd_{}_set", simd_type)), &[types::I64, types::I64, types::I64], None, false));
    }

    // Catch duplicate mir_names — second entry silently overwrites the first
    // in the dispatch HashMap, causing wrong calling conventions.
    if cfg!(debug_assertions) {
        let mut seen = HashSet::new();
        for entry in &entries {
            if !seen.insert(entry.mir_name) {
                eprintln!("warning: dispatch table has duplicate mir_name: {}", entry.mir_name);
            }
        }
    }

    entries
}

/// Build a lookup table from MIR function name to (ArgAdapt, RetAdapt).
/// Called once per codegen session, used by adapt_stdlib_call.
pub fn build_adapt_table() -> HashMap<String, (ArgAdapt, RetAdapt)> {
    stdlib_entries()
        .into_iter()
        .map(|e| (e.mir_name.to_string(), (e.arg_adapt, e.ret_adapt)))
        .collect()
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
        // Also register under c_name so declare_extern_functions can
        // detect that this function was already declared by the stdlib.
        if entry.c_name != entry.mir_name {
            func_ids.insert(entry.c_name.to_string(), id);
        }
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
