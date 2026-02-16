// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Stdlib method dispatch — maps MIR call names to C runtime functions.
//!
//! After monomorphization, stdlib method calls arrive at codegen as bare
//! names (e.g., "push", "len"). This module maps those names to C runtime
//! functions declared in compiler/runtime/runtime.c.
//!
//! ## Runtime reconciliation
//!
//! Two sets of C implementations exist for Vec, String, Map, and Pool:
//!
//! 1. **Old i64-based** (inline in `runtime.c`): all params/returns are `int64_t`,
//!    pointers cast to/from i64. These match the Cranelift signatures below.
//!    This is what the linker (`link.rs`) actually compiles and links.
//!
//! 2. **New typed** (`vec.c`, `string.c`, `map.c`, `pool.c` + `rask_runtime.h`):
//!    proper struct pointers (`RaskVec*`, `RaskString*`, etc.) with `elem_size`
//!    params for type-safe storage. These are not linked yet.
//!
//! The typed implementations are the intended target. Migrating requires:
//! - Update dispatch entries to match typed signatures (pointer params, elem_size)
//! - Update `link.rs` to compile the separate `.c` files (or unify into runtime.c)
//! - Remove the old i64-based duplicates from runtime.c
//! - Update codegen to pass elem_size when constructing collections

use cranelift::prelude::*;
use cranelift_module::{Linkage, Module};
use std::collections::HashMap;

use crate::{CodegenError, CodegenResult};

/// A stdlib function entry: MIR name → C runtime function.
pub struct StdlibEntry {
    /// Name as it appears in MIR Call statements
    pub mir_name: &'static str,
    /// C function name in runtime.c
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
        StdlibEntry {
            mir_name: "Vec_new",
            c_name: "rask_vec_new",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "push",
            c_name: "rask_vec_push",
            params: &[types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "pop",
            c_name: "rask_vec_pop",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "len",
            c_name: "rask_vec_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "get",
            c_name: "rask_vec_get",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "set",
            c_name: "rask_vec_set",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "clear",
            c_name: "rask_vec_clear",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "is_empty",
            c_name: "rask_vec_is_empty",
            params: &[types::I64],
            ret_ty: Some(types::I8),
        },
        StdlibEntry {
            mir_name: "capacity",
            c_name: "rask_vec_capacity",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },

        // ── String operations ──────────────────────────────────
        StdlibEntry {
            mir_name: "string_new",
            c_name: "rask_string_new",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "string_len",
            c_name: "rask_string_len",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "concat",
            c_name: "rask_string_concat",
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

        // ── Map operations ─────────────────────────────────────
        StdlibEntry {
            mir_name: "Map_new",
            c_name: "rask_map_new",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "insert",
            c_name: "rask_map_insert",
            params: &[types::I64, types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "contains_key",
            c_name: "rask_map_contains_key",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I8),
        },

        // ── Pool operations ────────────────────────────────────
        StdlibEntry {
            mir_name: "Pool_new",
            c_name: "rask_pool_new",
            params: &[],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "pool_alloc",
            c_name: "rask_pool_alloc",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "pool_free",
            c_name: "rask_pool_free",
            params: &[types::I64, types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "pool_get",
            c_name: "rask_pool_checked_access",
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
            mir_name: "rask_pool_checked_access",
            c_name: "rask_pool_checked_access",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I64),
        },

        // ── Stdlib module calls ─────────────────────────────────
        // cli.args() → Vec of string pointers
        StdlibEntry {
            mir_name: "cli_args",
            c_name: "rask_cli_args",
            params: &[],
            ret_ty: Some(types::I64),
        },
        // std.exit(code)
        StdlibEntry {
            mir_name: "std_exit",
            c_name: "rask_exit",
            params: &[types::I64],
            ret_ty: None,
        },
        // fs.read_lines(path) → Vec of string pointers
        StdlibEntry {
            mir_name: "fs_read_lines",
            c_name: "rask_fs_read_lines",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        // string.contains(haystack, needle) → bool
        StdlibEntry {
            mir_name: "contains",
            c_name: "rask_string_contains",
            params: &[types::I64, types::I64],
            ret_ty: Some(types::I8),
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

        // ── Concurrency: spawn/join/detach ──────────────────────────
        StdlibEntry {
            mir_name: "spawn",
            c_name: "rask_closure_spawn",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "join",
            c_name: "rask_task_join_simple",
            params: &[types::I64],
            ret_ty: Some(types::I64),
        },
        StdlibEntry {
            mir_name: "detach",
            c_name: "rask_task_detach",
            params: &[types::I64],
            ret_ty: None,
        },
        StdlibEntry {
            mir_name: "rask_task_cancelled",
            c_name: "rask_task_cancelled",
            params: &[],
            ret_ty: Some(types::I8),
        },
        StdlibEntry {
            mir_name: "rask_sleep_ns",
            c_name: "rask_sleep_ns",
            params: &[types::I64],
            ret_ty: None,
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
