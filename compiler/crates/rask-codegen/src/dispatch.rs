// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Stdlib method dispatch — maps MIR call names to C runtime functions.
//!
//! After monomorphization, stdlib method calls arrive at codegen as
//! type-qualified names (e.g., "Vec_push", "Map_get", "string_len").
//! Qualification happens in MIR lowering using type info from the checker.
//! This module maps those names to C runtime functions in the typed
//! implementations (vec.c, map.c, rack.c, string.c).
//!
//! A stdlib method's entry is built from its `@native("symbol")` declaration
//! in `stdlib/*.rk` and the symbol's prototype in `rask_runtime.h`
//! (`declared_entries`). The rest — names MIR mints for itself, and the
//! compiler-provided types with no stdlib file — are `internal_entries`.
//!
//! ## Calling convention
//!
//! The typed C API uses `const void*` for element parameters and returns
//! `void*` for element access. Builder.rs handles the adaptation:
//! - Constructors: lowering passes each element's size; codegen expands the
//!   element tags into owned-offset tables
//! - Value params (push, set, insert): codegen stores the value at its own
//!   width in a stack slot and passes the address
//! - Value returns (get, pop): codegen loads from the returned or out pointer
//!   at the destination's width

use cranelift::prelude::*;
use cranelift_module::{Linkage, Module};
use std::collections::{HashMap, HashSet};

use crate::{CodegenError, CodegenResult};

/// How to adapt arguments before a stdlib call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArgAdapt {
    /// Pass args as-is
    None,
    /// A container constructor: `leading` size arguments, then one element tag
    /// per `tags`, each expanded into (offsets pointer, count).
    ///
    /// Lowering says *what* the elements are — nothing, a string, or the struct
    /// with a given layout (`rask_mir::elem_strs`) — and this turns that into
    /// where the strings actually sit inside one element, which is a question
    /// only codegen has the layouts to answer. The runtime keeps the answer on
    /// the container, so `free` needs no argument and nothing downstream works
    /// it out again. A missing or unreadable tag means "owns nothing", which is
    /// what a container built by a path that doesn't know its element type gets.
    ContainerCtor { leading: u8, tags: u8 },
    /// Wrap args[1] as pointer (skip if string)
    WrapArg1,
    /// Wrap args[2] as pointer (skip if string)
    WrapArg2,
    /// Wrap args[1] and args[2] as pointers
    WrapArg1And2,
    /// Inject 16-byte string out-param as first arg
    StringOutParam,
    /// Two i64s written into the destination's own 16-byte slot — a
    /// `(usize, usize)` tuple, start at +0 and end at +8.
    PairOutParam,
    /// Same, but the call also returns a 0/1 status that becomes the
    /// `string or E` tag. The string goes to a scratch slot rather than to
    /// dst, because dst is the Result and the string is only its payload.
    StringResultOutParam,
    /// In-place string mutation: out-param IS the self string
    InPlaceStringMut,
    /// Append 8-byte (or 16-byte for string dst) out-param
    AppendOutParam,
    /// Append the destination `T?`'s payload address as an out-param. The call
    /// returns 1 (wrote a value) or 0 (nothing there), which becomes the tag.
    /// For anything that hands an element back out of a container it's about to
    /// free — the runtime copies while the element is still live, instead of
    /// returning a pointer into freed storage.
    OptionOutParam,
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
    /// C FFI convention: a negative scalar return means Err, otherwise Ok.
    /// Codegen wraps the return into the destination `T or E` Result slot.
    NegErr,
    /// Same convention for an Option: a negative scalar return means `none`,
    /// otherwise `some(value)`. Distinct from NegErr because Option and Result
    /// put their payloads at different offsets.
    NegNone,
    /// The return is a pointer to a sync box's payload (or, for staged access,
    /// to the working copy standing in for it). A payload that lives in its own
    /// storage binds the destination straight to that pointer, so the block's
    /// field writes land in the box rather than in a copied stack slot; anything
    /// word-sized takes one load. Which of the two is the destination type's
    /// business, so the entry only has to say "this is a payload pointer".
    ///
    /// Was a hardcoded list of four acquire names in `builder.rs`. Adding the
    /// staged pair to it was the obvious move and the wrong one — the fact
    /// belongs to the entry, and a name missing from a list four deep is how
    /// this went wrong the first time.
    BoxPayloadPtr,
    /// The return is a status: 0 for Ok, or a `RASK_CHAN_*` failure the
    /// destination's error enum names a variant for (`Closed`, `Cancelled`).
    Status,
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

    /// For `join` / `cancel` on a task handle: the C side hands back
    /// (outcome, value, message), and `builder::adapt_by_hand` assembles the
    /// `T or JoinError` from all three.
    const fn join_outcome(mir_name: &'static str, c_name: &'static str) -> Self {
        Self::simple(mir_name, c_name, &[types::I64, types::I64, types::I64], Some(types::I64), true)
    }

    /// For a `T?`-returning C function that signals "absent" with a negative
    /// index (`find`, `rfind`).
    const fn neg_none(
        mir_name: &'static str,
        c_name: &'static str,
        params: &'static [Type],
        ret_ty: Option<Type>,
        can_panic: bool,
    ) -> Self {
        Self { mir_name, c_name, params, ret_ty, can_panic, arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::NegNone }
    }
}

/// Leak a String to get a &'static str. Used for dynamically generated
/// dispatch entry names (atomic types). Called once at startup, small and bounded.
fn leak_str(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// The internal table: what a MIR name with no `@native` declaration behind
/// it calls.
///
/// These are spellings MIR mints for itself — frees, element lends and
/// write-backs, the pieces string interpolation and `json` encoding are built
/// from, task and runtime hooks (`INTERNAL_SPELLINGS` in
/// `rask_stdlib::mir_metadata` says what each stands for) — and the
/// operations of compiler-provided types that have no stdlib file to declare
/// them: the `Shared` strategies, `Atomic<T>`, SIMD vectors, raw pointers.
///
/// A method a program calls by name doesn't belong here. It belongs in
/// `stdlib/*.rk` as `@native("symbol")`, which gives its entry
/// (`declared_entries`); `internal_rows_are_not_declared` fails on a row that
/// repeats one.
pub(crate) fn internal_entries() -> Vec<StdlibEntry> {
    let mut entries = vec![
        // ── Vec operations ─────────────────────────────────────
        StdlibEntry {
            mir_name: "rask_vec_from_static", c_name: "rask_vec_from_static",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 3, tags: 1 }, ret_adapt: RetAdapt::None,
        },
        // A fixed array's receiver seen as a Vec, and the copy back after a
        // `mutate self` method (#1405).
        StdlibEntry {
            mir_name: "rask_vec_view", c_name: "rask_vec_view",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 3, tags: 1 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Vec_free_view", "rask_vec_free_view", &[types::I64], None, false),
        StdlibEntry::simple("rask_vec_copy_back", "rask_vec_copy_back", &[types::I64, types::I64], None, false),
        // `Map.from(pairs)` on an array held somewhere (map, pairs, count,
        // value offset, pair size).
        StdlibEntry::simple("rask_map_insert_pairs", "rask_map_insert_pairs", &[types::I64, types::I64, types::I64, types::I64, types::I64], None, false),
        StdlibEntry::simple("Vec_from", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        // Giving back the reference a captured variable's slot held, on the way
        // to the slot holding another one. Spelled apart from the refcount
        // ops because it is not this name's reference being released — see
        // `release_replaced_captures`.
        StdlibEntry::simple("string_free_replaced", "rask_string_free", &[types::I64], None, false),
        StdlibEntry::simple("Vec_free", "rask_vec_free", &[types::I64], None, false),
        // try_push shares push's C entry: the runtime has no reachable failure
        // yet (Vec carries no capacity bound, and OOM panics in the allocator),
        // so the destination `void or GrowError<T>` always gets its ok branch.
        // A bound makes the status meaningful, and this entry then has to build
        // GrowError.Full with the rejected element instead.
        StdlibEntry {
            mir_name: "Vec_try_push", c_name: "rask_vec_push",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Vec_get_unchecked", c_name: "rask_vec_get_unchecked",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        // An element taken out for `with`, `for mutate` or a field write, and
        // put back afterwards. The read is `v[i]`'s; only who owns the copy
        // differs (see `Internal::LendsElement`).
        StdlibEntry {
            mir_name: "Vec_lend", c_name: "rask_vec_get",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Vec_write_back", c_name: "rask_vec_write_back",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: true,
            arg_adapt: ArgAdapt::WrapArg2, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Vec_remove_at", c_name: "rask_vec_remove_at",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::AppendOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // `for x in v.take_all()`: the binding takes the element over, and the
        // slot is zeroed so the vector's free skips it.
        // `let old = self.f` before a refill: the field's bytes, handed over.
        StdlibEntry {
            mir_name: "Field_take", c_name: "rask_field_take",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::AppendOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "Vec_move_out", c_name: "rask_vec_move_out",
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

        // `mutate v[i]`: the callee has to write the real element, so it gets a
        // pointer into the buffer rather than a copy of it. `RetAdapt::None`
        // is the point — `DerefOrString` would copy the bytes into the
        // destination's own slot, which is exactly the copy being avoided.
        StdlibEntry {
            mir_name: "Vec_borrow_elem", c_name: "rask_vec_borrow_elem",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Vec_release_elem", "rask_vec_release_elem", &[types::I64], None, false),

        StdlibEntry::simple("Vec_to_vec", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        // `join` on anything but strings — the last argument is a
        // RASK_ELEM_KIND_* code, for the same reason `Vec_debug`'s is.
        StdlibEntry {
            mir_name: "Vec_join_scalar", c_name: "rask_vec_join_scalar",
            params: &[types::I64, types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // `{v:debug}` — the second argument is a RASK_ELEM_KIND_* code saying
        // how to read one element, since the header only carries its width.
        StdlibEntry {
            mir_name: "Vec_debug", c_name: "rask_vec_debug",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("Vec_sort_str", "rask_vec_sort_str", &[types::I64], None, false),
        // Sort by the scalar at offset 0 of each element: `sort()` on a Vec
        // of scalars, and `{m:debug}` ordering a map's (key, value) pairs.
        // Args: (vec, RASK_ELEM_KIND_* kind, size in bytes).
        StdlibEntry::simple(
            "Vec_sort_scalar", "rask_vec_sort_scalar",
            &[types::I64, types::I64, types::I64], None, false,
        ),
        StdlibEntry::simple("f64_compare", "rask_f64_compare_total", &[types::F64, types::F64], Some(types::I64), false),
        // `sort_by_key`: the elements, the parallel keys, and a comparator over
        // two keys. Panics only on a keys/elements length mismatch, which is
        // lowering's own bug rather than the program's.
        StdlibEntry::simple(
            "Vec_sort_by_keys", "rask_vec_sort_by_keys",
            &[types::I64, types::I64, types::I64, types::I64], None, true,
        ),
        // String elements need a real string compare: a heap RaskStr holds a
        // pointer, so equal strings don't match byte-for-byte.
        StdlibEntry::simple("Vec_contains_str", "rask_vec_contains_str", &[types::I64, types::I64], Some(types::I64), false),

        // ── Iterator runtime support ──────────────────────────────
        StdlibEntry::simple("Vec_skip", "rask_iter_skip", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_take", "rask_iter_take", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_collect", "rask_vec_collect", &[types::I64], Some(types::I64), false),

        // ── Wide<T> data-parallel (conc.data-parallel) ─────────
        // Closure-free ops only. `.wide()`/`.read()` reuse Vec clone (Wide is a
        // RaskVec* at runtime); `sum` folds int64 lanes. map/zip_with need the
        // closure-callback path, which currently segfaults natively (#441) —
        // see docs/working/native-wide.md — so they run under the interpreter only.
        StdlibEntry::simple("Vec_wide", "rask_vec_clone", &[types::I64], Some(types::I64), false),

        // ── String operations ──────────────────────────────────
        StdlibEntry::simple("string_free", "rask_string_free", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "string_clone", c_name: "rask_string_clone",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_to_owned", c_name: "rask_string_clone",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
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

        StdlibEntry::simple("string_eq", "rask_string_eq", &[types::I64, types::I64], Some(types::I64), false),
        // `x.hash()` on an integer, a bool or a char (HA1). (lo, hi, width) —
        // width bytes taken little-endian from lo and then hi, so one entry point
        // covers a 1-byte bool through a 16-byte u128 (#813).
        StdlibEntry::simple("int_hash", "rask_int_hash", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Link_hash", "rask_link_hash", &[types::I64], Some(types::I64), false),
        // find/rfind return `usize?` and the runtime signals "not found" with -1.
        // Wrapped as a plain value it came back as `some(-1)`, so `?? ...` never
        // fired and a slice taken at that index was empty (#463).
        StdlibEntry::neg_none("string_find", "rask_string_find", &[types::I64, types::I64], Some(types::I64), false),
        // `s[i]` — one byte. Indexing panics out of range, so it needs its own
        // entry point rather than `byte_at`'s none-on-miss.
        StdlibEntry::simple("string_index", "rask_string_index", &[types::I64, types::I64], Some(types::I64), true),
        StdlibEntry::neg_none("string_rfind", "rask_string_rfind", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "json_pretty", c_name: "rask_json_pretty",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::None,
        },
        // `parse<T>` reaches MIR with the target welded on (`string_parse_u8`),
        // and every width has its own entry point: sharing the 64-bit one made
        // `"70000".parse<u8>()` succeed (#837). The value comes back through an
        // out-param and the return is a 0/1 status (#472), which
        // `builder::adapt_by_hand` turns into the `T or ParseError`.
        StdlibEntry {
            mir_name: "string_parse_i8", c_name: "rask_string_parse_i8_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_i16", c_name: "rask_string_parse_i16_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_i32", c_name: "rask_string_parse_i32_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_i64", c_name: "rask_string_parse_int_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_isize", c_name: "rask_string_parse_int_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_u8", c_name: "rask_string_parse_u8_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_u16", c_name: "rask_string_parse_u16_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_u32", c_name: "rask_string_parse_u32_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_u64", c_name: "rask_string_parse_uint_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_usize", c_name: "rask_string_parse_uint_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_f32", c_name: "rask_string_parse_float_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_f64", c_name: "rask_string_parse_float_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },

        // String-producing operations (out-param)
        StdlibEntry {
            mir_name: "concat", c_name: "rask_string_concat",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // Interpolation lowers `a.concat(b)` unqualified, but a call on a
        // receiver the lowerer typed as a string mangles to `string_concat` —
        // same function, and nothing answered to the qualified name.
        StdlibEntry {
            mir_name: "string_concat", c_name: "rask_string_concat",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_substr", c_name: "rask_string_substr",
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

        // ── Conversion to string (out-param) ──────────────────
        StdlibEntry {
            mir_name: "i64_to_string", c_name: "rask_i64_to_string",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "u64_to_string", c_name: "rask_u64_to_string",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // 128-bit renderers. The value goes in at its own width; the 64-bit
        // helpers would print the low half as a different number (#762).
        StdlibEntry {
            mir_name: "i128_to_string", c_name: "rask_i128_to_string",
            params: &[types::I64, types::I128], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "u128_to_string", c_name: "rask_u128_to_string",
            params: &[types::I64, types::I128], ret_ty: None, can_panic: false,
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
        // f32 round-trips against its own width — formatting it as a double
        // spells out the exact binary value (0.1f → 0.10000000149011612).
        StdlibEntry {
            mir_name: "f32_to_string", c_name: "rask_f32_to_string",
            params: &[types::I64, types::F32], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "char_to_string", c_name: "rask_char_to_string",
            params: &[types::I64, types::I32], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // ── Format specs (std.fmt/S1) ─────────────────────────
        // The spec is parsed at compile time, so each of these gets one piece
        // of it: a base conversion first, then `string_pad` for width/align.
        StdlibEntry {
            mir_name: "i64_to_base", c_name: "rask_i64_to_base",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "u64_to_base", c_name: "rask_u64_to_base",
            params: &[types::I64, types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "f64_to_precision", c_name: "rask_f64_to_precision",
            params: &[types::I64, types::F64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "f64_to_exp", c_name: "rask_f64_to_exp",
            params: &[types::I64, types::F64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_truncate_chars", c_name: "rask_string_truncate_chars",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_pad", c_name: "rask_string_pad",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I32], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_debug", c_name: "rask_string_debug",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "char_debug", c_name: "rask_char_debug",
            params: &[types::I64, types::I32], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // ── Math operations ────────────────────────────────────
        // i64.abs() (std.math/N2) — libc's llabs, no wrapper needed.
        StdlibEntry::simple("i64_abs", "llabs", &[types::I64], Some(types::I64), false),
        // `llabs` takes a `long long`, so a 128-bit value would be truncated
        // before it was negated (#762).
        StdlibEntry::simple("i128_abs", "rask_i128_abs", &[types::I128], Some(types::I128), true),

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

        // ── Map operations ─────────────────────────────────────
        StdlibEntry::simple("Map_free", "rask_map_free", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "Map_new_string_keys", c_name: "rask_map_new_string_keys",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 2, tags: 2 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_new_link_keys", c_name: "rask_map_new_link_keys",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 2, tags: 2 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_with_capacity_link_keys", c_name: "rask_map_new_link_keys_cap",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 3, tags: 2 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_with_capacity_string_keys", c_name: "rask_map_new_string_keys_cap",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 3, tags: 2 }, ret_adapt: RetAdapt::None,
        },
        // A key compared through its own `eq`/`hash`: the two function
        // addresses ride with the sizes (and the capacity) ahead of the tags.
        StdlibEntry {
            mir_name: "Map_new_keyed", c_name: "rask_map_new_keyed",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 4, tags: 2 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_with_capacity_keyed", c_name: "rask_map_new_keyed_cap",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 5, tags: 2 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Map_from", "rask_map_clone", &[types::I64], Some(types::I64), false),
        // `m[k] = v`: insert or replace, releasing the value it replaces.
        StdlibEntry {
            mir_name: "Map_set", c_name: "rask_map_insert",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1And2, ret_adapt: RetAdapt::None,
        },
        // The Map twins of `Vec_lend` / `Vec_write_back`, by key.
        StdlibEntry {
            mir_name: "Map_lend", c_name: "rask_map_get_unwrap",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Map_write_back", c_name: "rask_map_write_back",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: true,
            arg_adapt: ArgAdapt::WrapArg1And2, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_get_unwrap", c_name: "rask_map_get_unwrap",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::DerefOrString,
        },
        // `mutate m[k]` — the Map twin of Vec_borrow_elem. `RetAdapt::None`
        // keeps the pointer into the table instead of copying the value out.
        StdlibEntry {
            mir_name: "Map_borrow_elem", c_name: "rask_map_borrow_elem",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Map_release_elem", "rask_map_release_elem", &[types::I64], None, false),
        // (map, value offset, entry size): an entry is a `(key, value)` tuple
        // and lowering lays it out.
        StdlibEntry::simple("Map_iter", "rask_map_entries", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Map_entries", "rask_map_entries", &[types::I64, types::I64, types::I64], Some(types::I64), false),

        // ── Rack + Link operations (mem.racks) ─────────────────
        StdlibEntry::simple("Rack_free", "rask_rack_free", &[types::I64], None, false),
        // Edge maintenance, emitted by lowering rather than written by anyone.
        StdlibEntry::simple("Link_set", "rask_link_set", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("Link_set_node", "rask_link_set_node",
                            &[types::I64, types::I64, types::I64], None, false),
        StdlibEntry::simple("Link_forget", "rask_link_forget", &[types::I64], None, false),
        StdlibEntry::simple("Link_register_element", "rask_link_register_element", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("Link_register_entry", "rask_link_register_entry", &[types::I64, types::I64], None, false),
        StdlibEntry {
            mir_name: "Link_register_struct", c_name: "rask_link_register_struct",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Link_register_vec", "rask_link_register_vec", &[types::I64], None, false),
        StdlibEntry::simple("Link_register_map", "rask_link_register_map", &[types::I64], None, false),
        StdlibEntry::simple("Link_forget_vec", "rask_link_forget_vec", &[types::I64], None, false),
        StdlibEntry::simple("Link_forget_map", "rask_link_forget_map", &[types::I64], None, false),

        // ── Rng operations ────────────────────────────────────────
        StdlibEntry::simple("Random_free", "rask_rng_free", &[types::I64], None, false),

        // ── Releases the drop pass emits ────────────────────────────
        StdlibEntry::simple("cstring_free", "rask_cstring_free", &[types::I64], None, false),
        // Not a method anyone writes: `build` consumes the builder, and this is
        // the release the drop pass emits for a path that never builds.
        StdlibEntry::simple("StringBuilder_free", "rask_string_builder_free", &[types::I64], None, false),

        // ── JSON module ─────────────────────────────────────────────
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
        // The value param is a `double` in the runtime. Declaring it I64 put the
        // f64 in an integer register, so every float in an encoded object came
        // out as a denormal (#478). The array variant below had it right.
        StdlibEntry::simple("json_buf_add_f64", "rask_json_buf_add_f64", &[types::I64, types::I64, types::F64], None, false),
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

        // Typed decode: the call site builds a shape describing the target
        // type, then hands it to the decoder (json.c).
        StdlibEntry::simple("json_shape_prim", "rask_json_shape_prim", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_shape_struct", "rask_json_shape_struct", &[types::I64], Some(types::I64), false),
        // A list or map shape carries what its elements own, as a container
        // constructor would: the container the decoder builds frees it.
        StdlibEntry {
            mir_name: "json_shape_vec", c_name: "rask_json_shape_vec",
            params: &[types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 2, tags: 1 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "json_shape_map", c_name: "rask_json_shape_map",
            params: &[types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 2, tags: 1 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("json_shape_opt", "rask_json_shape_opt", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_shape_field", "rask_json_shape_field",
            &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], None, false),
        StdlibEntry::simple("json_shape_free", "rask_json_shape_free", &[types::I64], None, false),
        StdlibEntry::simple("json_decode_into", "rask_json_decode_into",
            &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_decode_zero", "rask_json_decode_zero", &[types::I64, types::I64], None, false),
        StdlibEntry {
            mir_name: "json_error_message", c_name: "rask_json_error_message",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "json_encode_shaped", c_name: "rask_json_encode_shaped",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // ── Clone ────────────────────────────────────────────────────
        StdlibEntry::simple("clone", "rask_clone", &[types::I64], Some(types::I64), false),

        // ── ThreadPool ─────────────────────────────────────────────
        // `ThreadPool.spawn { … }` and `Thread.spawn { … }`: see the green
        // task's entry below for the arguments.
        StdlibEntry::simple("rask_threadpool_spawn", "rask_threadpool_spawn", &[types::I64, types::I64], Some(types::I64), true),
        StdlibEntry::simple("rask_thread_spawn", "rask_thread_spawn", &[types::I64, types::I64], Some(types::I64), true),

        // ── Concurrency: spawn/join/detach ──────────────────────────
        // join/cancel report how the task ended alongside its value — a
        // panicked task doesn't re-panic in the
        // joiner, it comes back as Err(JoinError.Panicked(msg)) (ctrl.panic/O1).
        // Two args: the closure, then whether its result is a heap box the task
        // owns and must free if no join ever comes for it (#963).
        // Panics when the task block is bound to its task (#1356), so the call
        // records where it is; without that the report named the last line
        // that happened to record one. `lower_spawn` calls the three task
        // entries by their C names: they're the runtime's, not stdlib methods.
        StdlibEntry::simple("rask_green_closure_spawn", "rask_green_closure_spawn", &[types::I64, types::I64], Some(types::I64), true),
        // A closure value a task block captured: refused if it captured a link
        // or a `Local` box itself.
        StdlibEntry::simple("rask_closure_refuse_crossing", "rask_closure_refuse_crossing", &[types::I64], None, true),
        // One handle for every spawn form (conc.async/H5); the runtime reads
        // which kind it is.
        StdlibEntry::join_outcome("join", "rask_handle_join"),
        StdlibEntry::simple("detach", "rask_handle_detach", &[types::I64], None, true),
        StdlibEntry::join_outcome("cancel", "rask_handle_cancel"),
        StdlibEntry::simple("cancelled", "rask_handle_cancelled", &[], Some(types::I8), false),
        StdlibEntry::simple("rask_sleep_ns", "rask_sleep_ns", &[types::I64], Some(types::I64), false),

        // ── Concurrency: runtime init/shutdown ───────────────────────
        StdlibEntry::simple("rask_runtime_init", "rask_runtime_init", &[types::I64], None, false),
        StdlibEntry::simple("rask_runtime_shutdown", "rask_runtime_shutdown", &[], None, false),
        StdlibEntry::simple("rask_threadpool_init", "rask_threadpool_init", &[types::I64], None, false),
        StdlibEntry::simple("rask_threadpool_shutdown", "rask_threadpool_shutdown", &[], None, false),

        // ── Ensure hooks ──────────────────────────────────────────
        StdlibEntry::simple("rask_ensure_push", "rask_ensure_push", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("rask_ensure_pop", "rask_ensure_pop", &[], None, false),
        StdlibEntry::simple("rask_unwind_push", "rask_unwind_push", &[types::I64], None, false),
        StdlibEntry::simple("rask_unwind_pop", "rask_unwind_pop", &[types::I64], None, false),

        // ── Resource tracking (C1/C2 consumption cancellation) ───
        StdlibEntry::simple("rask_resource_is_consumed", "rask_resource_is_consumed", &[types::I64], Some(types::I64), false),

        // ── Memory allocation ─────────────────────────────────────
        StdlibEntry::simple("rask_alloc", "rask_alloc", &[types::I64], Some(types::I64), false),

        // ── Concurrency: channels ──────────────────────────────────
        StdlibEntry::simple("channel_tx", "rask_channel_get_tx", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("channel_rx", "rask_channel_get_rx", &[types::I64], Some(types::I64), false),

        StdlibEntry::simple("Sender_clone", "rask_sender_clone_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Sender_drop", "rask_sender_drop_i64", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "send", c_name: "rask_channel_send_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("sender_clone", "rask_sender_clone_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("sender_drop", "rask_sender_drop_i64", &[types::I64], None, false),

        // Rotating start offset for a plain `select`'s probe order (conc.select/P1).
        StdlibEntry::simple("rask_select_rotate", "rask_select_rotate", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("rask_select_epoch", "rask_select_epoch", &[], Some(types::I64), false),
        StdlibEntry::simple("rask_select_wait", "rask_select_wait", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Receiver_drop", "rask_recver_drop_i64", &[types::I64], None, false),
        StdlibEntry::simple("recver_drop", "rask_recver_drop_i64", &[types::I64], None, false),

        // ── Concurrency: Shared<T> ──────────────────────────────────
        StdlibEntry {
            mir_name: "Shared_new", c_name: "rask_shared_new_ptr",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        // Cell — the internal spelling of `Shared<T, Local>`, the strategy that
        // takes no lock (`conc.sync/SH1`). `new` takes
        // the value by pointer plus its size, the same way Shared does; `get`
        // hands back the slot address for codegen to load or copy from.
        StdlibEntry {
            mir_name: "Cell_new", c_name: "rask_cell_new",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Cell_get", c_name: "rask_cell_get",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Cell_set", c_name: "rask_cell_set",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Cell_replace", c_name: "rask_cell_replace",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        // The same three under each lock. `get` hands back the slot's address
        // like the Cell version; `set`/`replace` take the lock around the copy.
        StdlibEntry {
            mir_name: "Shared_get", c_name: "rask_shared_get",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Shared_set", c_name: "rask_shared_set",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Shared_replace", c_name: "rask_shared_replace",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Mutex_get", c_name: "rask_mutex_get",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Mutex_set", c_name: "rask_mutex_set",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Mutex_replace", c_name: "rask_mutex_replace",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        // `take` consumes the box and yields what it held. The value goes to
        // the caller's own destination and the box is freed with it.
        StdlibEntry {
            mir_name: "Cell_take", c_name: "rask_cell_take",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Shared_take", c_name: "rask_shared_take",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Mutex_take", c_name: "rask_mutex_take",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        // `with cell as v { ... }` — same slot address as `Cell_get`, but the
        // block decides for itself whether to load through it or alias it, so no
        // ret_adapt here. Separate MIR names keep the two uses from drifting; a
        // Cell has no lock, so there's no release counterpart.
        StdlibEntry {
            mir_name: "Cell_acquire", c_name: "rask_cell_get",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::BoxPayloadPtr,
        },
        StdlibEntry::simple("Cell_data", "rask_cell_get", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "Shared_read_acquire", c_name: "rask_shared_read_acquire",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::BoxPayloadPtr,
        },
        StdlibEntry {
            mir_name: "Shared_write_acquire", c_name: "rask_shared_write_acquire",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::BoxPayloadPtr,
        },
        StdlibEntry::simple("Shared_data", "rask_shared_data", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_release", "rask_shared_release", &[types::I64], None, false),
        // Staged access (conc.sync/ST1-ST3). The "release" of the triple is the
        // commit; the discard is registered by the acquire and reached only from
        // the runtime's unwind drain, so codegen never names it.
        StdlibEntry {
            mir_name: "Shared_staged_acquire", c_name: "rask_shared_staged_acquire",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::BoxPayloadPtr,
        },
        StdlibEntry::simple("Shared_staged_data", "rask_shared_staged_data", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_staged_commit", "rask_shared_staged_commit", &[types::I64], None, false),
        StdlibEntry::simple("Shared_try_read", "rask_shared_try_read_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_try_write", "rask_shared_try_write_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_clone", "rask_shared_clone_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_drop", "rask_shared_drop_i64", &[types::I64], None, false),

        // ── Concurrency: Mutex<T> ──────────────────────────────────
        StdlibEntry {
            mir_name: "Mutex_new", c_name: "rask_mutex_new_ptr",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Mutex_acquire", c_name: "rask_mutex_acquire",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::BoxPayloadPtr,
        },
        StdlibEntry::simple("Mutex_release", "rask_mutex_release", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "Mutex_staged_acquire", c_name: "rask_mutex_staged_acquire",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::BoxPayloadPtr,
        },
        StdlibEntry::simple("Mutex_staged_data", "rask_mutex_staged_data", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Mutex_staged_commit", "rask_mutex_staged_commit", &[types::I64], None, false),
        StdlibEntry::simple("Mutex_data", "rask_mutex_data", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Mutex_try_lock", "rask_mutex_try_lock_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Mutex_clone", "rask_mutex_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Mutex_drop", "rask_mutex_drop", &[types::I64], None, false),
        // The `Local` strategy's release. `rask_cell_free` has been in the
        // runtime all along with nothing calling it — the same shape
        // `rask_shared_free` was in before #1099, and the reason
        // `Shared.local(0)` leaked its cell and its payload.
        StdlibEntry::simple("Cell_drop", "rask_cell_free", &[types::I64], None, false),
        // Another handle on the same cell. Without it a `Local` clone took the
        // `Readers` row and bumped a refcount the cell doesn't have.
        StdlibEntry::simple("Cell_clone", "rask_cell_clone", &[types::I64], Some(types::I64), false),

        // ── Char predicates ───────────────────────────────────
        StdlibEntry::simple("char_to_int", "rask_char_to_int", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_eq", "rask_char_eq", &[types::I32, types::I32], Some(types::I64), false),
    ];

    // ── Atomic operations ──────────────────────────────────
    // mem.atomics/GA1: `Atomic<T>` is the only spelling, so there is one set of
    // rows. The cell holds the payload at its own width and knows its kind
    // (lowering passes both to `new`), and values cross the call as the
    // payload's bits in an int64_t. Adapted by hand throughout
    // (`builder::hand_adaptation`): codegen turns a float or a struct into
    // those bits and back, which a numeric conversion would get wrong.
    let atomic = |mir_name: &'static str, c_name: &'static str, params: &'static [types::Type], ret: Option<types::Type>| StdlibEntry {
        mir_name, c_name, params, ret_ty: ret, can_panic: false,
        arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
    };
    entries.push(atomic("Atomic_new", "rask_atomic_new", &[types::I64, types::I64, types::I64], Some(types::I64)));
    entries.push(StdlibEntry::simple("Atomic_default", "rask_atomic_default", &[types::I64, types::I64], Some(types::I64), false));
    entries.push(atomic("Atomic_load", "rask_atomic_load", &[types::I64, types::I64], Some(types::I64)));
    entries.push(atomic("Atomic_store", "rask_atomic_store", &[types::I64, types::I64, types::I64], None));
    entries.push(atomic("Atomic_swap", "rask_atomic_swap", &[types::I64, types::I64, types::I64], Some(types::I64)));
    entries.push(atomic(
        "Atomic_compare_exchange", "rask_atomic_compare_exchange",
        &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], Some(types::I64),
    ));
    entries.push(atomic(
        "Atomic_compare_exchange_weak", "rask_atomic_compare_exchange_weak",
        &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], Some(types::I64),
    ));
    for op in &["fetch_add", "fetch_sub", "fetch_and", "fetch_or", "fetch_xor", "fetch_nand", "fetch_max", "fetch_min"] {
        entries.push(atomic(
            leak_str(&format!("Atomic_{}", op)),
            leak_str(&format!("rask_atomic_{}", op)),
            &[types::I64, types::I64, types::I64], Some(types::I64),
        ));
    }
    entries.push(atomic("Atomic_take", "rask_atomic_into_inner", &[types::I64], Some(types::I64)));
    // The ordinary drop. `into_inner` frees too and hands the value out with
    // it; this is for an atomic that just goes out of scope.
    entries.push(StdlibEntry::simple("Atomic_free", "rask_atomic_free", &[types::I64], None, false));

    // Fences
    entries.push(StdlibEntry::simple("fence", "rask_fence", &[types::I64], None, false));
    entries.push(StdlibEntry::simple("compiler_fence", "rask_compiler_fence", &[types::I64], None, false));

    // ── Raw pointer methods ─────────────────────────────────
    // Generated from rask_stdlib::PTR_METHODS so the set codegen can call is
    // exactly the set the checker accepts. A pointer is an i64 address here;
    // read/write/add/sub/offset take the pointee size as a trailing argument.
    for m in rask_stdlib::PTR_METHODS {
        use rask_stdlib::PtrSig;
        let Some(c_symbol) = m.c_symbol else { continue };
        let mir_name = leak_str(&rask_stdlib::ptr_methods::mir_name(m.name));
        let entry = match m.sig {
            PtrSig::Read => StdlibEntry::simple(
                mir_name, c_symbol, &[types::I64, types::I64], Some(types::I64), false,
            ),
            PtrSig::Write => StdlibEntry::simple(
                mir_name, c_symbol, &[types::I64, types::I64, types::I64], None, false,
            ),
            PtrSig::Arith => StdlibEntry::simple(
                mir_name, c_symbol, &[types::I64, types::I64, types::I64], Some(types::I64), false,
            ),
            PtrSig::Predicate => StdlibEntry::simple(
                mir_name, c_symbol, &[types::I64], Some(types::I64), false,
            ),
            PtrSig::PredicateInt | PtrSig::Comparison | PtrSig::ToInt => StdlibEntry::simple(
                mir_name, c_symbol, &[types::I64, types::I64], Some(types::I64), false,
            ),
            // No C symbol, so unreachable — the `else` above skipped it.
            PtrSig::Cast => continue,
        };
        entries.push(entry);
    }

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

    entries
}

/// Every call native codegen dispatches to the C runtime: one entry per
/// `@native("symbol")` stdlib declaration, then the internal table.
pub fn stdlib_entries() -> &'static [StdlibEntry] {
    static ENTRIES: std::sync::OnceLock<Vec<StdlibEntry>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| {
        let mut entries: Vec<StdlibEntry> = declared_entries()
            .into_iter()
            .map(|r| r.unwrap_or_else(|e| panic!("{e}")))
            .collect();
        entries.extend(internal_entries());
        entries
    })
}

/// The entries the stdlib's declarations give, one per `@native("symbol")`
/// method, or why one can't be built.
///
/// The symbol comes from the attribute and the registers from the symbol's
/// prototype in `rask_runtime.h`. How arguments and the result cross is read
/// off the declared types:
///
/// - a parameter of a type parameter's type (`push(take value: T)`) is a
///   value only the call site knows the width of, so it goes by address;
/// - `string` comes back through an out-parameter, and `string or E` through
///   two, the second for the failure's message;
/// - `T?` is a pointer to the element, null for `none`; `string?` writes the
///   string into the option's payload; `Link<T>?` is the link's own word,
///   the null one for `none`; any other `X?` is a negative return for `none`;
/// - a bare `T` comes back through an out-parameter as wide as the
///   destination;
/// - a constructor in `rask_mir::elem_strs::CTORS` takes the sizes lowering
///   passes and the element descriptions codegen expands.
///
/// What the types can't say, the attribute does (`rask_stdlib::NativeDecl`),
/// and a call whose adaptation is written by hand (`builder::hand_adapted`)
/// takes nothing from here but the symbol and the registers.
pub fn declared_entries() -> Vec<Result<StdlibEntry, String>> {
    let reg = rask_stdlib::StubRegistry::load();
    let mut names: Vec<&str> = reg.type_names().collect();
    names.sort_unstable();
    let mut out = Vec::new();
    for type_name in names {
        let Some(t) = reg.get_type(type_name) else { continue };
        for m in &t.methods {
            let Some(native) = m.native.as_ref().filter(|n| !n.symbol.is_empty()) else {
                continue;
            };
            out.push(declared_entry(type_name, m, native));
        }
    }
    out
}

fn declared_entry(
    type_name: &str,
    m: &rask_stdlib::types::MethodStub,
    native: &rask_stdlib::NativeDecl,
) -> Result<StdlibEntry, String> {
    use rask_ast::ty::TypeExpr;
    use rask_stdlib::NativeReturn;

    let mir_name = format!("{type_name}_{}", m.name);
    let at = || format!("`{type_name}.{}` (`@native(\"{}\")`)", m.name, native.symbol);
    let proto = crate::c_header::prototype(&native.symbol).ok_or_else(|| {
        format!("{}: `rask_runtime.h` has no prototype for it that codegen can read", at())
    })?;

    let is_type_param =
        |t: &TypeExpr| t.bare_name().is_some_and(|n| m.type_params.iter().any(|p| p == n));
    // `T?` and `Option<T>` are one type, as are `T or E` and `Result<T, E>`.
    let optional = |t: &TypeExpr| -> Option<TypeExpr> {
        match t {
            TypeExpr::Optional(inner) => Some((**inner).clone()),
            TypeExpr::Named { path, args }
                if path.len() == 1 && path[0] == "Option" && args.len() == 1 =>
            {
                Some(args[0].clone())
            }
            _ => None,
        }
    };
    let result_ok = |t: &TypeExpr| -> Option<TypeExpr> {
        match t {
            TypeExpr::Result { ok, .. } => Some((**ok).clone()),
            TypeExpr::Named { path, args }
                if path.len() == 1 && path[0] == "Result" && args.len() == 2 =>
            {
                Some(args[0].clone())
            }
            _ => None,
        }
    };

    let entry = |arg_adapt, ret_adapt| StdlibEntry {
        mir_name: leak_str(&mir_name),
        c_name: leak_str(&native.symbol),
        params: Box::leak(proto.params.clone().into_boxed_slice()),
        ret_ty: proto.ret,
        can_panic: native.panics,
        arg_adapt,
        ret_adapt,
    };

    if crate::builder::hand_adapted(&mir_name) {
        return Ok(entry(ArgAdapt::None, RetAdapt::None));
    }

    if let Some(&(_, leading, tags, _)) = rask_mir::elem_strs::CTORS
        .iter()
        .find(|(name, _, tags, _)| *name == mir_name && *tags > 0)
    {
        return Ok(entry(ArgAdapt::ContainerCtor { leading, tags }, RetAdapt::None));
    }

    // Parameters of a type parameter's type, by argument position. The
    // receiver is argument zero.
    let first = usize::from(m.takes_self);
    let by_address: Vec<usize> = m
        .params
        .iter()
        .enumerate()
        .filter(|(_, (_, ty))| is_type_param(ty))
        .map(|(i, _)| i + first)
        .collect();
    let wrap = match by_address.as_slice() {
        [] => ArgAdapt::None,
        [1] => ArgAdapt::WrapArg1,
        [2] => ArgAdapt::WrapArg2,
        [1, 2] => ArgAdapt::WrapArg1And2,
        other => {
            return Err(format!(
                "{}: arguments {other:?} go by address, and only 1 and 2 can",
                at()
            ))
        }
    };

    let ret = &m.ret_ty;
    let (out_param, ret_adapt) = match native.returns {
        Some(NativeReturn::Pointer) if optional(ret).is_some() => (None, RetAdapt::DerefOption),
        Some(NativeReturn::Pointer) => (None, RetAdapt::None),
        Some(NativeReturn::Negative) if optional(ret).is_some() => (None, RetAdapt::NegNone),
        Some(NativeReturn::Negative) => (None, RetAdapt::NegErr),
        Some(NativeReturn::Status) => (None, RetAdapt::Status),
        Some(NativeReturn::Nothing) => (None, RetAdapt::None),
        None if ret.is_name("string") => (Some(ArgAdapt::StringOutParam), RetAdapt::FromArgAdapt),
        None if result_ok(ret).is_some_and(|ok| ok.is_name("string")) => {
            (Some(ArgAdapt::StringResultOutParam), RetAdapt::FromArgAdapt)
        }
        None if result_ok(ret).is_some() => {
            return Err(format!(
                "{}: returns `{ret}`, and nothing says how the C side reports the \
                 failure. Add `returns: negative`, `status` or `nothing` to the \
                 attribute, or adapt the call by hand",
                at()
            ))
        }
        None => match optional(ret) {
            Some(inner) if is_type_param(&inner) => (None, RetAdapt::DerefOption),
            Some(inner) if inner.is_name("string") => {
                (Some(ArgAdapt::OptionOutParam), RetAdapt::FromArgAdapt)
            }
            Some(inner) if inner.name().as_deref() == Some("Link") => (None, RetAdapt::None),
            Some(_) => (None, RetAdapt::NegNone),
            None if is_type_param(ret) => {
                (Some(ArgAdapt::AppendOutParam), RetAdapt::FromArgAdapt)
            }
            None => (None, RetAdapt::None),
        },
    };
    let arg_adapt = match (wrap, out_param) {
        (wrap, None) => wrap,
        (ArgAdapt::None, Some(out)) => out,
        (_, Some(_)) => {
            return Err(format!(
                "{}: takes a value by address and returns through an out-parameter, \
                 which no one adapter does",
                at()
            ))
        }
    };
    Ok(entry(arg_adapt, ret_adapt))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Two entries with the same MIR name means the second silently wins, and
    /// the calling convention of the first is lost.
    #[test]
    fn dispatch_names_are_unique() {
        let mut seen = HashSet::new();
        for entry in stdlib_entries() {
            assert!(
                seen.insert(entry.mir_name),
                "duplicate dispatch entry for `{}`",
                entry.mir_name
            );
        }
    }

    /// Every internal row's signature is the prototype `rask_runtime.h` has for
    /// its symbol. A declared method's entry is read from the header, so it
    /// agrees by construction; an internal row is written by hand and has to
    /// be checked. The two were written separately once and nothing compared
    /// them: `rask_net_is_invalid` returns an `int8_t`, and its rows took an
    /// `i64` back, so the answer's upper bytes were whatever the register held.
    #[test]
    fn dispatch_rows_match_the_header() {
        let mut wrong = Vec::new();
        let mut compared = 0;
        // A symbol is declared to Cranelift once, so two rows sharing one
        // have to agree as well.
        let mut by_symbol: HashMap<&str, &StdlibEntry> = HashMap::new();
        for e in stdlib_entries() {
            if let Some(first) = by_symbol.insert(e.c_name, e) {
                if first.params != e.params || first.ret_ty != e.ret_ty {
                    wrong.push(format!(
                        "{} and {} both call {} with different signatures",
                        first.mir_name, e.mir_name, e.c_name
                    ));
                }
            }
            let Some(proto) = crate::c_header::prototype(e.c_name) else { continue };
            compared += 1;
            if proto.ret != e.ret_ty || proto.params.as_slice() != e.params {
                wrong.push(format!(
                    "{} → {}: row says ({:?}) -> {:?}, header says ({:?}) -> {:?}",
                    e.mir_name, e.c_name, e.params, e.ret_ty, proto.params, proto.ret
                ));
            }
        }
        // The prototype reader has to be reading the header, or this passes
        // by comparing nothing.
        assert!(compared > 400, "only {compared} rows matched a prototype");
        assert!(wrong.is_empty(), "{} rows disagree with rask_runtime.h:\n  {}", wrong.len(), wrong.join("\n  "));
    }

    /// Every `@native("symbol")` declaration gives native codegen its entry.
    ///
    /// `@native` is an assertion, and for a long time nothing checked it:
    /// `os.exit` was declared `@native` with no row anywhere, so it
    /// type-checked, ran on the interpreter, and died at codegen with
    /// "Function not found: os_exit" (#855, #1007). The entry is built from the
    /// declaration now, so the ways left to break it are a symbol the header
    /// doesn't declare, or a return type whose C convention nothing states.
    #[test]
    fn every_native_declaration_gives_an_entry() {
        let broken: Vec<String> = declared_entries().into_iter().filter_map(Result::err).collect();
        assert!(
            broken.is_empty(),
            "{} `@native` declarations don't give a dispatch entry:\n  {}",
            broken.len(),
            broken.join("\n  ")
        );
    }

    /// The internal table holds only names no declaration answers. A row for
    /// a declared `@native` method is a second copy of the declaration — the
    /// one that drifted before (`Map.insert` named `rask_map_insert` while the
    /// row called `rask_map_insert_displaced`).
    #[test]
    fn internal_rows_are_not_declared() {
        let declared: HashSet<&str> = declared_entries()
            .into_iter()
            .filter_map(Result::ok)
            .map(|e| e.mir_name)
            .collect();
        let mut doubled: Vec<&str> = internal_entries()
            .iter()
            .map(|e| e.mir_name)
            .filter(|n| declared.contains(n))
            .collect();
        doubled.sort_unstable();
        assert!(
            doubled.is_empty(),
            "{} internal rows repeat a `@native` declaration — delete the row, the \
             declaration gives the entry:\n  {}",
            doubled.len(),
            doubled.join("\n  ")
        );
    }

    /// Why a `@native` declaration has no dispatch row.
    ///
    /// `@native` with no symbol derives its name, and those are the ones that
    /// go missing quietly. Each either has a dispatch row or is classified
    /// here. The classification used to be a bare list of names, with a
    /// comment saying it mixed two kinds "and only trying one tells them
    /// apart". Trying all 65 said: 46 are fine and 19 are gaps, which is not a
    /// distinction a reader should have to rediscover. So the list records
    /// which, and the evidence.
    enum NoRow {
        /// Lowered somewhere other than a call — fused into an iterator chain,
        /// resolved at comptime, sent to libm. Correct, and it stays. The
        /// string names the `tests/suite/t_native_reach_*.rk` file that runs
        /// it on both backends, because "it's lowered elsewhere" is a claim
        /// and a claim needs a program behind it.
        LoweredElsewhere(&'static str),
        /// No entry point at all: the call type-checks, runs on the
        /// interpreter, and dies at codegen with "Function not found". The
        /// string is the tracking issue.
        NoEntryPoint(&'static str),
    }
    use NoRow::{LoweredElsewhere as Ok_, NoEntryPoint as Gap};

    /// The list may only shrink. The test fails both ways: an unlisted name
    /// with no row is a new gap, and a listed name that has since gained a row
    /// is stale and must come off — that is how `Command.*` and `Map.
    /// with_capacity` left it.
    const NATIVE_WITHOUT_A_DISPATCH_ROW: &[(&str, NoRow)] = &[
    ("FieldInfo.get",             Ok_("t_native_reach_reflect")),
    ("FieldInfo.has",             Ok_("t_native_reach_reflect")),
    ("Map.modify",                Ok_("t_native_reach_map_math_json")),
    ("Map.modify_with_default",   Ok_("t_native_reach_map_math_json")),
    ("Map.read",                  Ok_("t_native_reach_map_math_json")),
    ("Vec.all",                   Ok_("t_native_reach_vec")),
    ("Vec.any",                   Ok_("t_native_reach_vec")),
    ("Vec.find",                  Ok_("t_native_reach_vec")),
    ("Vec.fold",                  Ok_("t_native_reach_vec")),
    ("Vec.modify",                Ok_("t_native_reach_vec")),
    ("Vec.position",              Ok_("t_native_reach_vec")),
    ("Vec.read",                  Ok_("t_native_reach_vec")),
    ("Vec.sort",                  Ok_("t_container_real_width")),
    ("Vec.reduce",                Ok_("t_native_reach_vec")),
    ("Vec.sort_by_key",           Ok_("t_native_reach_vec")),
    ("Vec.sum",                   Ok_("t_native_reach_vec")),
    ("Vec.zip",                   Ok_("t_native_reach_vec")),
    ("json.decode",               Ok_("t_json_decode_qualified_value")),
    ("json.encode",               Ok_("t_native_reach_map_math_json")),
    ("json.encode_pretty",        Ok_("t_native_reach_map_math_json")),
    ("math.acos",                 Ok_("t_native_reach_map_math_json")),
    ("math.asin",                 Ok_("t_native_reach_map_math_json")),
    ("math.atan",                 Ok_("t_native_reach_map_math_json")),
    ("math.atan2",                Ok_("t_native_reach_map_math_json")),
    ("math.cos",                  Ok_("t_native_reach_map_math_json")),
    ("math.exp",                  Ok_("t_native_reach_map_math_json")),
    ("math.hypot",                Ok_("t_native_reach_map_math_json")),
    ("math.ln",                   Ok_("t_native_reach_map_math_json")),
    ("math.log10",                Ok_("t_native_reach_map_math_json")),
    ("math.log2",                 Ok_("t_native_reach_map_math_json")),
    ("math.sin",                  Ok_("t_native_reach_map_math_json")),
    ("math.tan",                  Ok_("t_native_reach_map_math_json")),
    ("math.to_degrees",           Ok_("t_native_reach_map_math_json")),
    ("math.to_radians",           Ok_("t_native_reach_map_math_json")),
    ("reflect.align_of",          Ok_("t_reflect_size_of")),
    ("reflect.is_copy",           Ok_("t_reflect_size_of")),
    ("reflect.size_of",           Ok_("t_reflect_size_of")),
    ("reflect.fields",            Ok_("t_native_reach_reflect")),
    ("reflect.has_field",         Ok_("t_native_reach_reflect")),
    ("reflect.has_method",        Ok_("t_native_reach_reflect")),
    ("reflect.implements",        Ok_("t_native_reach_reflect")),
    ("reflect.interfaces",        Ok_("t_native_reach_reflect")),
    ("reflect.methods",           Ok_("t_native_reach_reflect")),
    ("reflect.variants",          Ok_("t_native_reach_reflect")),
    ("reflect.is_enum",           Ok_("t_native_reach_reflect")),
    ("reflect.is_flat",           Ok_("t_native_reach_reflect")),
    ("reflect.is_float",          Ok_("t_native_reach_reflect")),
    ("reflect.is_integer",        Ok_("t_native_reach_reflect")),
    ("reflect.is_map",            Ok_("t_native_reach_reflect")),
    ("reflect.is_optional",       Ok_("t_native_reach_reflect")),
    ("reflect.is_resource",       Ok_("t_native_reach_reflect")),
    ("reflect.is_struct",         Ok_("t_native_reach_reflect")),
    ("reflect.is_vec",            Ok_("t_native_reach_reflect")),
    ("reflect.name_of",           Ok_("t_native_reach_reflect")),
    ];

    #[test]
    fn derived_native_names_are_listed_or_dispatched() {
        let dispatched: HashSet<&str> =
            stdlib_entries().iter().map(|e| e.mir_name).collect();
        let listed: HashSet<&str> =
            NATIVE_WITHOUT_A_DISPATCH_ROW.iter().map(|(k, _)| *k).collect();

        let reg = rask_stdlib::StubRegistry::load();
        let mut unlisted: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for type_name in reg.type_names() {
            let Some(t) = reg.get_type(&type_name) else { continue };
            for m in &t.methods {
                if !matches!(m.native.as_ref().map(|n| n.symbol.as_str()), Some("")) {
                    continue;
                }
                let key = format!("{}.{}", type_name, m.name);
                if dispatched.contains(format!("{}_{}", type_name, m.name).as_str()) {
                    continue;
                }
                seen.insert(key.clone());
                if !listed.contains(key.as_str()) {
                    unlisted.push(key);
                }
            }
        }
        unlisted.sort();
        assert!(
            unlisted.is_empty(),
            "{} `@native` declarations have no dispatch row and aren't listed. Try \
             one — write a program that calls it and run it on both backends. \
             If it answers, it is lowered somewhere bespoke: add it as \
             `LoweredElsewhere` naming the suite file that proves it. If it \
             dies with \"Function not found\", it is a gap: open an issue, add \
             it as `NoEntryPoint`, and register the probe in \
             tests/known_divergences.txt:\n  {}",
            unlisted.len(),
            unlisted.join("\n  ")
        );

        let mut stale: Vec<&str> = NATIVE_WITHOUT_A_DISPATCH_ROW
            .iter()
            .map(|(k, _)| *k)
            .filter(|k| !seen.contains(*k))
            .collect();
        stale.sort();
        assert!(
            stale.is_empty(),
            "{} entries in NATIVE_WITHOUT_A_DISPATCH_ROW no longer describe anything — \
             the declaration gained a row, changed its marker, or went away. Delete \
             them; the list only shrinks:\n  {}",
            stale.len(),
            stale.join("\n  ")
        );
    }

    /// Every classification has to cash out: a `LoweredElsewhere` names a
    /// suite file that exists and runs the call on both backends, and a
    /// `NoEntryPoint` names an issue. Without this, "lowered elsewhere" is a
    /// guess anyone can write down, which is what the old flat list was.
    #[test]
    fn every_classification_carries_its_evidence() {
        let suite = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/suite");
        let mut bad: Vec<String> = Vec::new();
        for (name, why) in NATIVE_WITHOUT_A_DISPATCH_ROW {
            match why {
                NoRow::LoweredElsewhere(probe) => {
                    let f = suite.join(format!("{probe}.rk"));
                    if !f.exists() {
                        bad.push(format!("{name}: no probe at tests/suite/{probe}.rk"));
                        continue;
                    }
                    // The probe has to actually call it. A file that names the
                    // family in a comment and never runs the method proves
                    // nothing, and that is the easy mistake to make here.
                    let method = name.rsplit('.').next().unwrap_or(name);
                    let body = std::fs::read_to_string(&f).unwrap_or_default();
                    let calls: Vec<&str> = body
                        .lines()
                        .filter(|l| !l.trim_start().starts_with("//"))
                        .collect();
                    // `.m(` or `.m<` — a generic call writes its type
                    // arguments first, as `reflect.is_vec<Row>()` does.
                    let body = calls.join("\n");
                    if !body.contains(&format!(".{method}("))
                        && !body.contains(&format!(".{method}<"))
                    {
                        bad.push(format!(
                            "{name}: tests/suite/{probe}.rk never calls `.{method}`"
                        ));
                    }
                }
                NoRow::NoEntryPoint(issue) => {
                    if !issue.starts_with('#') || issue[1..].parse::<u32>().is_err() {
                        bad.push(format!("{name}: `{issue}` is not an issue number"));
                    }
                }
            }
        }
        bad.sort();
        assert!(
            bad.is_empty(),
            "{} entries don't carry their evidence:\n  {}",
            bad.len(),
            bad.join("\n  ")
        );
    }
}

