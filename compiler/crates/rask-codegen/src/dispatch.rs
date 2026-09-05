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
    /// Atomic compare-exchange: append an out_ok pointer (result written there).
    AtomicCas,
    /// parse: append an out-param for the value; the call returns 0/1 status,
    /// which becomes the `T or ParseError` tag.
    ParseOutParam,
    /// Append the destination `T?`'s payload address as an out-param. The call
    /// returns 1 (wrote a value) or 0 (nothing there), which becomes the tag.
    /// For anything that hands an element back out of a container it's about to
    /// free — the runtime copies while the element is still live, instead of
    /// returning a pointer into freed storage.
    OptionOutParam,
    /// join/cancel: append two out-params — the task's value and a 16-byte
    /// message string. The call returns how the task ended (ok/panicked/
    /// cancelled), which becomes the `T or JoinError` tag and, when it failed,
    /// the JoinError variant.
    JoinOutcomeOutParams,
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

    /// Shorthand for C FFI functions using the negative-return=Err convention.
    const fn neg_err(
        mir_name: &'static str,
        c_name: &'static str,
        params: &'static [Type],
        ret_ty: Option<Type>,
        can_panic: bool,
    ) -> Self {
        Self { mir_name, c_name, params, ret_ty, can_panic, arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::NegErr }
    }

    /// For `TaskHandle.join` / `.cancel` and their OS-thread twins: the C side
    /// hands back (outcome, value, message) and codegen assembles the
    /// `T or JoinError` from all three.
    const fn join_outcome(mir_name: &'static str, c_name: &'static str) -> Self {
        Self {
            mir_name, c_name,
            params: &[types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64),
            can_panic: true,
            arg_adapt: ArgAdapt::JoinOutcomeOutParams,
            ret_adapt: RetAdapt::FromArgAdapt,
        }
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

/// Build the complete stdlib dispatch table.
pub fn stdlib_entries() -> Vec<StdlibEntry> {
    let mut entries = vec![
        // ── Vec operations ─────────────────────────────────────
        StdlibEntry {
            mir_name: "Vec_new", c_name: "rask_vec_new",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 1, tags: 1 }, ret_adapt: RetAdapt::None,
        },
        // Vec.with_capacity(n): (elem_size, cap) — elem_size injected at lowering.
        StdlibEntry {
            mir_name: "Vec_with_capacity", c_name: "rask_vec_with_capacity",
            params: &[types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 2, tags: 1 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "rask_vec_from_static", c_name: "rask_vec_from_static",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 3, tags: 1 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Vec_from", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_free", "rask_vec_free", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "Vec_push", c_name: "rask_vec_push",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::None,
        },
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
            mir_name: "Vec_pop", c_name: "rask_vec_pop",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry::simple("Vec_len", "rask_vec_len", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_as_ptr", "rask_vec_as_ptr", &[types::I64], Some(types::I64), false),
        // Same buffer address as as_ptr — `mutate self` is the only difference.
        StdlibEntry::simple("Vec_as_mut_ptr", "rask_vec_as_ptr", &[types::I64], Some(types::I64), false),
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
        // Safe `.get()` (V3): returns T?, none on OOB, no panic. Indexing (`v[i]`)
        // maps to Vec_get instead. DerefOption encodes NULL → None.
        StdlibEntry {
            mir_name: "Vec_get_opt", c_name: "rask_vec_get_opt",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry {
            mir_name: "Vec_set", c_name: "rask_vec_set",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: true,
            arg_adapt: ArgAdapt::WrapArg2, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Vec_clear", "rask_vec_clear", &[types::I64], None, false),
        StdlibEntry::simple("Vec_is_empty", "rask_vec_is_empty", &[types::I64], Some(types::I64), false),
        // CP1-CP3: `capacity()` is the *bound*, not the allocation — `none` when
        // the vector is unbounded, which the runtime signals with -1. The
        // allocation size isn't a Rask-visible number.
        StdlibEntry::neg_none("Vec_capacity", "rask_vec_bound", &[types::I64], Some(types::I64), false),
        StdlibEntry::neg_none("Vec_remaining", "rask_vec_remaining", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_is_bounded", "rask_vec_is_bounded", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_is_full", "rask_vec_is_full", &[types::I64], Some(types::I64), false),
        // Vec.fixed(n): (elem_size, n) — elem_size injected at lowering, same as
        // with_capacity. The difference is the bound it sets.
        StdlibEntry {
            mir_name: "Vec_fixed", c_name: "rask_vec_fixed",
            params: &[types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 2, tags: 1 }, ret_adapt: RetAdapt::None,
        },
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
        // `{v:debug}` — the second argument is a RASK_DEBUG_ELEM_* code saying
        // how to read one element, since the header only carries its width.
        StdlibEntry {
            mir_name: "vec_debug", c_name: "rask_vec_debug",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("Vec_sort", "rask_vec_sort", &[types::I64], None, false),
        // Vec<f64> needs the float total order — the default compares elements
        // as int64_t, which orders negatives backwards (type.operators/ORD3).
        StdlibEntry::simple("Vec_sort_f64", "rask_vec_sort_f64", &[types::I64], None, false),
        StdlibEntry::simple("Vec_sort_str", "rask_vec_sort_str", &[types::I64], None, false),
        // `{m:debug}` sorting a map's entries by key — the key is at offset 0
        // of each pair. Args: (vec, key kind, key size in bytes).
        StdlibEntry::simple(
            "Vec_sort_pairs", "rask_vec_sort_pairs",
            &[types::I64, types::I64, types::I64], None, false,
        ),
        StdlibEntry::simple("f64_compare", "rask_f64_compare_total", &[types::F64, types::F64], Some(types::I64), false),
        StdlibEntry::simple("Vec_sort_by", "rask_vec_sort_by", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("Vec_reverse", "rask_vec_reverse", &[types::I64], None, false),
        StdlibEntry::simple("Vec_swap", "rask_vec_swap", &[types::I64, types::I64, types::I64], None, true),
        // The runtime compares the element bytes through a pointer, so the
        // needle has to be spilled and passed by address — as a raw value it
        // was read as an address and the compare walked off into memory (#413).
        StdlibEntry {
            mir_name: "Vec_contains", c_name: "rask_vec_contains",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        // String elements need a real string compare: a heap RaskStr holds a
        // pointer, so equal strings don't match byte-for-byte.
        StdlibEntry::simple("Vec_contains_str", "rask_vec_contains_str", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_remove_adjacent_duplicates", "rask_vec_dedup", &[types::I64], None, false),
        // Both return `Option<T>` — NULL for an empty Vec — so the result is
        // wrapped, not dereferenced. `DerefOrString` read through the NULL and
        // handed back a bare value for a destination expecting an option (#412).
        StdlibEntry {
            mir_name: "Vec_first", c_name: "rask_vec_first",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry {
            mir_name: "Vec_last", c_name: "rask_vec_last",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },

        // ── Iterator runtime support ──────────────────────────────
        StdlibEntry::simple("Vec_skip", "rask_iter_skip", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_map", "rask_vec_map", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_collect", "rask_vec_collect", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Vec_filter", "rask_vec_filter", &[types::I64, types::I64], Some(types::I64), false),

        // ── Wide<T> data-parallel (conc.data-parallel) ─────────
        // Closure-free ops only. `.wide()`/`.read()` reuse Vec clone (Wide is a
        // RaskVec* at runtime); `sum` folds int64 lanes. map/zip_with need the
        // closure-callback path, which currently segfaults natively (#441) —
        // see NOTES_native_wide.md — so they run under the interpreter only.
        StdlibEntry::simple("Vec_wide", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Wide_read", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Wide_sum", "rask_wide_sum", &[types::I64], Some(types::I64), false),

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

        // ─── StringView (std.strings/V1–V6) ───────────────────────
        //
        // A view is a `RaskStr` sharing the source's buffer, so every read-only
        // operation is the string one. `view()` is the 16-byte copy plus the
        // refcount bump the StringClone adapter already emits — that is the
        // whole of V1, and it is why this needs no runtime function of its own.
        // A view of a view lands here too, re-referencing the original header
        // rather than chaining (V6).
        StdlibEntry {
            mir_name: "string_view", c_name: "rask_string_clone",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringClone, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "StringView_view", c_name: "rask_string_clone",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringClone, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // V2: copying out has to release the pin, so this allocates rather than
        // handing back another reference to the source's buffer.
        StdlibEntry {
            mir_name: "StringView_to_string", c_name: "rask_string_unshare",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("StringView_len", "rask_string_len", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("StringView_is_empty", "rask_string_is_empty", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("StringView_hash", "rask_string_hash", &[types::I64], Some(types::I64), false),

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
        StdlibEntry::simple("string_hash", "rask_string_hash", &[types::I64], Some(types::I64), false),
        // `x.hash()` on an integer, a bool or a char (HA1). (lo, hi, width) —
        // width bytes taken little-endian from lo and then hi, so one entry point
        // covers a 1-byte bool through a 16-byte u128 (#813).
        StdlibEntry::simple("int_hash", "rask_int_hash", &[types::I64, types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_as_ptr", "rask_string_ptr", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_as_c_str", "rask_string_ptr", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_is_empty", "rask_string_is_empty", &[types::I64], Some(types::I64), false),
        // find/rfind return `usize?` and the runtime signals "not found" with -1.
        // Wrapped as a plain value it came back as `some(-1)`, so `?? ...` never
        // fired and a slice taken at that index was empty (#463).
        StdlibEntry::neg_none("string_find", "rask_string_find", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::neg_none("string_index_of", "rask_string_find", &[types::I64, types::I64], Some(types::I64), false),
        // Byte offset in, scalar out; -1 for out of range or mid-character,
        // which is never a valid scalar.
        StdlibEntry::neg_none("string_char_at", "rask_string_char_at", &[types::I64, types::I64], Some(types::I64), false),
        // `s[i]` — one byte. Indexing panics out of range, so it needs its own
        // entry point rather than `byte_at`'s none-on-miss.
        StdlibEntry::simple("string_index", "rask_string_index", &[types::I64, types::I64], Some(types::I64), true),
        StdlibEntry::neg_none("string_rfind", "rask_string_rfind", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::neg_none("string_last_index_of", "rask_string_rfind", &[types::I64, types::I64], Some(types::I64), false),
        // `char_at` answers `char?`; the runtime signals out-of-range with -1,
        StdlibEntry {
            mir_name: "json_pretty", c_name: "rask_json_pretty",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("string_starts_with", "rask_string_starts_with", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_ends_with", "rask_string_ends_with", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_contains", "rask_string_contains", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_byte_at", "rask_string_byte_at", &[types::I64, types::I64], Some(types::I64), false),
        // parse<T> mangles the type argument into the name, so every width needs
        // an entry or the call has nothing to dispatch to. All integer widths
        // share the one runtime parse; the caller's Result slot narrows it.
        //
        // These use the *_into runtime entry points: the value comes back
        // through an out-param and the return value is a 0/1 status, so an
        // unparseable string becomes Err instead of Ok(0) (#472).
        //
        // Every width has its own entry point. Sharing the 64-bit signed parse
        // meant u64::MAX exactly was "value out of range", a leading `-` came
        // back as a huge positive number, and `"70000".parse<u8>()` succeeded —
        // native truncating to 112, the interpreter keeping 70000 (#837).
        StdlibEntry {
            mir_name: "string_parse", c_name: "rask_string_parse_int_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_i8", c_name: "rask_string_parse_i8_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_i16", c_name: "rask_string_parse_i16_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_i32", c_name: "rask_string_parse_i32_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_i64", c_name: "rask_string_parse_int_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_isize", c_name: "rask_string_parse_int_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_u8", c_name: "rask_string_parse_u8_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_u16", c_name: "rask_string_parse_u16_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_u32", c_name: "rask_string_parse_u32_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_u64", c_name: "rask_string_parse_uint_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_usize", c_name: "rask_string_parse_uint_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_f32", c_name: "rask_string_parse_float_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "string_parse_f64", c_name: "rask_string_parse_float_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ParseOutParam, ret_adapt: RetAdapt::None,
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
            mir_name: "string_from_char", c_name: "rask_string_from_char",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // Display columns, not scalars (std.strings/U2) — this is what fmt
        // padding counts.
        StdlibEntry::simple("string_width", "rask_string_width", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_graphemes", "rask_string_graphemes", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "string_truncate", c_name: "rask_string_truncate",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "string_normalized", c_name: "rask_string_normalized",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("string_is_ascii", "rask_string_str_is_ascii", &[types::I64], Some(types::I64), false),
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
        StdlibEntry::simple("string_char_indices", "rask_string_char_indices", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("string_bytes", "rask_string_bytes", &[types::I64], Some(types::I64), false),

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
            params: &[types::I64, types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
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
            params: &[types::I64, types::F64], ret_ty: None, can_panic: false,
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
        // The f64_* method entries are generated below from
        // rask_stdlib::FLOAT_METHODS. f32 keeps its own single-precision entry.
        StdlibEntry::simple("f32_sqrt", "sqrtf", &[types::F32], Some(types::F32), false),

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
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 2, tags: 2 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Map_new_string_keys", c_name: "rask_map_new_string_keys",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::ContainerCtor { leading: 2, tags: 2 }, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Map_from", "rask_map_clone", &[types::I64], Some(types::I64), false),
        // `insert` answers `V?` — the value it displaced. The C side hands
        // back a pointer to it (NULL for a fresh key), so DerefOption builds
        // the option the same way `Map_get` and `Map_remove` do. Passing the
        // plain `rask_map_insert` flag through untranslated made every
        // overwrite answer `1` and every fresh key answer `Some(0)` (#903).
        StdlibEntry {
            mir_name: "Map_insert", c_name: "rask_map_insert_displaced",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1And2, ret_adapt: RetAdapt::DerefOption,
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
        // `mutate m[k]` — the Map twin of Vec_borrow_elem. `RetAdapt::None`
        // keeps the pointer into the table instead of copying the value out.
        StdlibEntry {
            mir_name: "Map_borrow_elem", c_name: "rask_map_borrow_elem",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Map_release_elem", "rask_map_release_elem", &[types::I64], None, false),
        StdlibEntry {
            // Declared `-> Option<V>`: hand back the removed value, not a
            // 0/-1 status. NULL → none, otherwise some(the value).
            mir_name: "Map_remove", c_name: "rask_map_take",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::WrapArg1, ret_adapt: RetAdapt::DerefOption,
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
        // PL2: bounded pool. Args (elem_size, cap) — elem_size injected at lowering
        // like Pool_new. Enforcement (panic on full / try_insert sentinel) lives in
        // the runtime.
        StdlibEntry::simple("Pool_with_capacity", "rask_pool_with_capacity", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_alloc", "rask_pool_alloc_packed", &[types::I64], Some(types::I64), false),
        // `remove` answers `T?` — DerefOption turns the returned slot pointer
        // into some(elem), and NULL (stale handle) into none.
        StdlibEntry {
            mir_name: "Pool_remove", c_name: "rask_pool_remove_out",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::OptionOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
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
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: true,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        // try_insert on a bounded pool: the handle, or `none` when it's full
        // (PL8). The runtime signals "full" with -1, which is what NegNone
        // expects; MIR types the result as a plain tagged `i64?` rather than a
        // niche `Option<Handle>`, so the tag has to be written out.
        StdlibEntry {
            mir_name: "Pool_try_insert", c_name: "rask_pool_try_insert_packed_sized",
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::NegNone,
        },
        StdlibEntry::simple("Pool_drain", "rask_pool_drain", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Pool_checked_access", "rask_pool_get_packed", &[types::I64, types::I64], Some(types::I64), false),

        // ── Rack + Link operations (mem.racks) ─────────────────
        // `Rack.new()` has nothing to read `T` off, so the node type's size and
        // link-field offsets ride along with the first `insert` instead.
        StdlibEntry::simple("Rack_new", "rask_rack_new", &[], Some(types::I64), false),
        StdlibEntry::simple("Rack_free", "rask_rack_free", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "Rack_insert", c_name: "rask_rack_insert",
            params: &[types::I64, types::I64, types::I64, types::I64, types::I64],
            ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Rack_delete", "rask_rack_delete", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("Rack_len", "rask_rack_len", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Rack_is_empty", "rask_rack_is_empty", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Rack_contains", "rask_rack_contains", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Rack_nodes", "rask_rack_nodes", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Rack_clear", "rask_rack_clear", &[types::I64], None, false),
        StdlibEntry::simple("Rack_snapshot", "rask_rack_snapshot", &[types::I64], Some(types::I64), false),
        // `corresponding` answers `Link<T>?`, which is the niche — the sentinel
        // the runtime returns *is* the none, so nothing needs wrapping.
        StdlibEntry::simple("Rack_corresponding", "rask_rack_corresponding", &[types::I64, types::I64], Some(types::I64), false),
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
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Link_register_vec", "rask_link_register_vec", &[types::I64], None, false),
        StdlibEntry::simple("Link_register_map", "rask_link_register_map", &[types::I64], None, false),

        // ── Rng operations ────────────────────────────────────────
        StdlibEntry::simple("Random_new", "rask_rng_new", &[], Some(types::I64), false),
        StdlibEntry::simple("Random_from_seed", "rask_rng_from_seed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Random_u64", "rask_rng_u64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Random_i64", "rask_rng_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Random_f64", "rask_rng_f64", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("Random_f32", "rask_rng_f32", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("Random_bool", "rask_rng_bool", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Random_range", "rask_rng_range", &[types::I64, types::I64, types::I64], Some(types::I64), true),
        StdlibEntry::simple("Random_shuffle", "rask_random_shuffle", &[types::I64, types::I64], None, true),
        // `choice` hands back a pointer to the element, or NULL for an empty
        // Vec — the same shape `Vec_get` uses, so DerefOption builds the `T?`.
        StdlibEntry {
            mir_name: "Random_choice", c_name: "rask_random_choice",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },

        // ── Random module convenience functions ───────────────────
        StdlibEntry::simple("random_f64", "rask_random_f64", &[], Some(types::F64), false),
        StdlibEntry::simple("random_f32", "rask_random_f32", &[], Some(types::F64), false),
        StdlibEntry::simple("random_i64", "rask_random_i64", &[], Some(types::I64), false),
        StdlibEntry::simple("random_bool", "rask_random_bool", &[], Some(types::I64), false),
        StdlibEntry::simple("random_range", "rask_random_range", &[types::I64, types::I64], Some(types::I64), true),

        // ── File instance methods ─────────────────────────────────
        StdlibEntry::simple("File_close", "rask_file_close", &[types::I64], None, false),
        // `int64_t rask_file_read_all(RaskStr *out, int64_t file)` — the string
        // comes back through the out-param, the return value is the ok/err tag
        // for `string or IoError`. Declared as a 1-arg call returning i64, the
        // FILE* landed in `out` and the runtime wrote a 16-byte RaskStr over
        // it (#654).
        StdlibEntry {
            mir_name: "File_read_text", c_name: "rask_file_read_all",
            // (out, file, err_out) — the third is the failure message (#682).
            params: &[types::I64, types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::StringResultOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // read_bytes/write_bytes return/take a Vec<u8> pointer directly — a
        // plain heap pointer never looks negative, so the existing
        // negative-return-means-error convention (used elsewhere for handles
        // like TcpConnection) applies cleanly with no out-param plumbing.
        StdlibEntry::neg_err("File_read_bytes", "rask_file_read_bytes", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("File_write", "rask_file_write", &[types::I64, types::I64], None, false),
        StdlibEntry::neg_err("File_write_bytes", "rask_file_write_bytes", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("File_write_text", "rask_file_write", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("File_write_line", "rask_file_write_line", &[types::I64, types::I64], None, false),

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
            // (out, err_out) — the second is the failure message (#682).
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::StringResultOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // ── FS module ───────────────────────────────────────────
        // Self-hosted from stdlib/fs.rk. Remaining C runtime stubs:
        StdlibEntry::simple("fs_write_bytes", "rask_fs_write_bytes", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("fs_create_dir_all", "rask_fs_create_dir_all", &[types::I64], None, false),
        StdlibEntry::simple("fs_open_handle", "rask_fs_open", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("fs_create_handle", "rask_fs_create", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("File_is_null", "rask_file_is_null", &[types::I64], Some(types::I64), false),
        // `fs.metadata` and `Metadata`'s accessors used to live here. It's a
        // plain Rask struct built by Rask code now — see stdlib/fs.rk (#674).

        // ── Time module ─────────────────────────────────────────────
        StdlibEntry::simple("Instant_now", "rask_time_Instant_now", &[], Some(types::I64), false),
        StdlibEntry::simple("time_wall_clock_nanos", "rask_time_wall_clock_nanos", &[], Some(types::I64), false),
        StdlibEntry::simple("Instant_elapsed", "rask_time_Instant_elapsed", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_from_nanos", "rask_time_Duration_from_nanos", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_from_millis", "rask_time_Duration_from_millis", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_nanos", "rask_time_Duration_as_nanos", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_seconds", "rask_time_Duration_as_secs", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_seconds_f64", "rask_time_Duration_as_secs_f64", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("Duration_as_millis", "rask_time_Duration_as_millis", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_micros", "rask_time_Duration_as_micros", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_as_seconds_f32", "rask_time_Duration_as_secs_f32", &[types::I64], Some(types::F64), false),
        StdlibEntry::simple("Duration_seconds", "rask_time_Duration_seconds", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_millis", "rask_time_Duration_millis", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_micros", "rask_time_Duration_micros", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_nanos", "rask_time_Duration_nanos", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Duration_seconds_f64", "rask_time_Duration_from_secs_f64", &[types::F64], Some(types::I64), false),

        // ── Standard streams ───────────────────────────────────────
        // The handle carries the stream number, so one entry per operation
        // covers stdout, stderr and stdin (#859).
        StdlibEntry::simple("io_write_std_text", "rask_io_std_write_text", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("io_write_std_bytes", "rask_io_std_write_bytes", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("io_flush_std", "rask_io_std_flush", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("io_read_std_bytes", "rask_io_std_read_bytes", &[types::I64], Some(types::I64), false),

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
        // Plain handle returns, not `neg_err`. The adapter that turns a negative
        // return into the error side has no way to build an `IoError` — it's a
        // Rask enum — so it left the raw -1 as the payload, and matching -1 as an
        // enum tag traps. `stdlib/net.rk` checks `is_invalid()` and builds the
        // error itself (#863), the same way `fs.open` does (#858).
        StdlibEntry::simple("net_listen_handle", "rask_net_tcp_listen", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("net_connect_handle", "rask_net_tcp_connect", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpListener_accept_handle", "rask_net_tcp_accept", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpListener_is_invalid", "rask_net_is_invalid", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpConnection_is_invalid", "rask_net_is_invalid", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpListener_is_unresolved", "rask_net_is_unresolved", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpConnection_is_unresolved", "rask_net_is_unresolved", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpListener_close", "rask_net_close", &[types::I64], None, false),
        StdlibEntry::simple("TcpListener_clone", "rask_net_clone", &[types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "TcpListener_local_addr", c_name: "rask_net_local_addr",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        // read_bytes/write_bytes hand back/take a Vec<u8> pointer directly —
        // a plain heap pointer is never negative, so the same convention used
        // for handles (TcpListener.accept, etc.) applies with no out-param.
        StdlibEntry::neg_err("TcpConnection_read_bytes", "rask_net_read_bytes", &[types::I64], Some(types::I64), false),
        StdlibEntry::neg_err("TcpConnection_write_bytes", "rask_net_write_bytes", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry {
            mir_name: "TcpConnection_remote_addr", c_name: "rask_net_remote_addr",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::neg_err("TcpConnection_read_http_request", "rask_net_read_http_request", &[types::I64], Some(types::I64), false),
        StdlibEntry::neg_err("TcpConnection_write_http_response", "rask_net_write_http_response", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("TcpConnection_close", "rask_net_close", &[types::I64], None, false),
        StdlibEntry::simple("TcpConnection_clone", "rask_net_clone", &[types::I64], Some(types::I64), false),

        // ── HTTP server close (linear resource cleanup) ─────────────
        StdlibEntry::simple("HttpServer_close", "rask_http_server_close", &[types::I64], None, false),

        // ── os module: environment ──────────────────────────────────
        // env returns `string?` — the runtime hands back NULL when unset and
        // DerefOption turns that into `none`, copying the 16-byte string out of
        // the pointer for the `some` side.
        StdlibEntry {
            mir_name: "os_env", c_name: "rask_os_env",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOption,
        },
        StdlibEntry::simple("os_pid", "rask_os_pid", &[], Some(types::I64), false),
        // struct.targets/EX3 + ctrl.panic/P5: immediate exit, no unwind, no
        // ensures. Declared `@native` in stdlib/os.rk with no entry here, so
        // `os.exit(1)` reached codegen as "Function not found: os_exit" while
        // the interpreter ran it — same shape as os_set_env before #855.
        StdlibEntry::simple("os_exit", "rask_os_exit", &[types::I64], None, false),
        StdlibEntry::simple("os_env_vars", "rask_os_env_vars", &[], Some(types::I64), false),
        StdlibEntry {
            mir_name: "os_env_or", c_name: "rask_os_env_or",
            params: &[types::I64, types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry::simple("os_set_env", "rask_os_set_env", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("os_remove_env", "rask_os_remove_env", &[types::I64], None, false),
        StdlibEntry::simple("os_args", "rask_os_args", &[], Some(types::I64), false),
        StdlibEntry {
            mir_name: "os_platform", c_name: "rask_os_platform",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "os_arch", c_name: "rask_os_arch",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // Subprocess. The builder is Rask (stdlib/os.rk); only these three
        // reach the OS. `process_run` takes the pieces and returns the exit
        // status; the two readers hand back what it captured on this thread.
        StdlibEntry::simple(
            "os_process_run", "rask_process_run",
            &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64, types::I64],
            Some(types::I64), false,
        ),
        StdlibEntry {
            mir_name: "os_process_stdout", c_name: "rask_process_stdout",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },
        StdlibEntry {
            mir_name: "os_process_stderr", c_name: "rask_process_stderr",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::StringOutParam, ret_adapt: RetAdapt::FromArgAdapt,
        },

        // ── StringBuilder ───────────────────────────────────────────
        StdlibEntry::simple("StringBuilder_new", "rask_string_builder_new", &[], Some(types::I64), false),
        StdlibEntry::simple("StringBuilder_with_capacity", "rask_string_builder_with_capacity", &[types::I64], Some(types::I64), false),
        // push/push_char are the names stdlib/string.rk declares (and `Vec.push`
        // reads the same way); the C entry points kept their older spelling.
        StdlibEntry::simple("StringBuilder_push", "rask_string_builder_append", &[types::I64, types::I64], None, false),
        StdlibEntry::simple("StringBuilder_push_char", "rask_string_builder_append_char", &[types::I64, types::I64], None, false),
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

        // Typed decode: the call site builds a shape describing the target
        // type, then hands it to the decoder (json.c).
        StdlibEntry::simple("json_shape_prim", "rask_json_shape_prim", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_shape_struct", "rask_json_shape_struct", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_shape_vec", "rask_json_shape_vec", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_shape_map", "rask_json_shape_map", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_shape_opt", "rask_json_shape_opt", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("json_shape_field", "rask_json_shape_field",
            &[types::I64, types::I64, types::I64, types::I64, types::I64], None, false),
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
        StdlibEntry::simple("Vec_clone", "rask_vec_clone", &[types::I64], Some(types::I64), false),
        // I3: hands the elements over and leaves the source empty.
        StdlibEntry::simple("Vec_take_all", "rask_vec_take_all", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Map_clone", "rask_map_clone", &[types::I64], Some(types::I64), false),

        // ── ThreadPool ─────────────────────────────────────────────
        StdlibEntry::simple("ThreadPool_spawn", "rask_threadpool_spawn", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Thread_spawn", "rask_closure_spawn", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::join_outcome("ThreadHandle_join", "rask_task_join_outcome"),
        StdlibEntry::join_outcome("Thread_join", "rask_task_join_outcome"),
        StdlibEntry::simple("ThreadHandle_detach", "rask_task_detach", &[types::I64], None, false),
        StdlibEntry::simple("Thread_detach", "rask_task_detach", &[types::I64], None, false),
        StdlibEntry::simple("time_sleep", "rask_sleep_ns", &[types::I64], Some(types::I64), false),

        // ── Concurrency: spawn/join/detach (green scheduler) ────────
        // join/cancel report how the task ended alongside its value, same as
        // the OS-thread path — a panicked task no longer re-panics in the
        // joiner, it comes back as Err(JoinError.Panicked(msg)) (ctrl.panic/O1).
        // Two args: the closure, then whether its result is a heap box the task
        // owns and must free if no join ever comes for it (#963).
        StdlibEntry::simple("spawn", "rask_green_closure_spawn", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::join_outcome("join", "rask_green_join_outcome"),
        StdlibEntry::simple("detach", "rask_green_detach", &[types::I64], None, true),
        StdlibEntry::join_outcome("cancel", "rask_green_cancel_outcome"),
        // TaskHandle qualified names (same C functions as unqualified)
        StdlibEntry::join_outcome("TaskHandle_join", "rask_green_join_outcome"),
        StdlibEntry::simple("TaskHandle_detach", "rask_green_detach", &[types::I64], None, true),
        StdlibEntry::join_outcome("TaskHandle_cancel", "rask_green_cancel_outcome"),
        StdlibEntry::simple("rask_task_cancelled", "rask_green_task_is_cancelled", &[], Some(types::I32), false),
        StdlibEntry::simple("rask_sleep_ns", "rask_green_sleep_ns", &[types::I64], None, false),

        // ── Concurrency: runtime init/shutdown ───────────────────────
        StdlibEntry::simple("rask_runtime_init", "rask_runtime_init", &[types::I64], None, false),
        StdlibEntry::simple("rask_runtime_shutdown", "rask_runtime_shutdown", &[], None, false),
        StdlibEntry::simple("rask_threadpool_init", "rask_threadpool_init", &[types::I64], None, false),
        StdlibEntry::simple("rask_threadpool_shutdown", "rask_threadpool_shutdown", &[], None, false),
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
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::NegErr,
        },
        StdlibEntry::neg_err("Sender_try_send", "rask_channel_try_send_i64", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::neg_err("Sender_close", "rask_sender_close_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Sender_clone", "rask_sender_clone_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Sender_drop", "rask_sender_drop_i64", &[types::I64], None, false),
        StdlibEntry {
            mir_name: "send", c_name: "rask_channel_send_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("sender_clone", "rask_sender_clone_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("sender_drop", "rask_sender_drop_i64", &[types::I64], None, false),

        // Receiver methods. Both receives are out-param recvs returning the
        // channel status, which the Custom adapter turns into `T or E` — one
        // shape for every element size, and a closed channel is a value rather
        // than a panic (#1067).
        StdlibEntry {
            mir_name: "Receiver_receive", c_name: "rask_channel_recv_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Receiver_try_receive", c_name: "rask_channel_try_recv_into",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        // Rotating start offset for a plain `select`'s probe order (conc.select/P1).
        StdlibEntry::simple("rask_select_rotate", "rask_select_rotate", &[types::I64], Some(types::I64), false),
        StdlibEntry::neg_err("Receiver_close", "rask_recver_close_i64", &[types::I64], Some(types::I64), false),
        StdlibEntry::simple("Receiver_drop", "rask_recver_drop_i64", &[types::I64], None, false),
        StdlibEntry::simple("receive", "rask_channel_recv_i64", &[types::I64], Some(types::I64), true),
        StdlibEntry::simple("recver_drop", "rask_recver_drop_i64", &[types::I64], None, false),

        // ── Concurrency: Shared<T> ──────────────────────────────────
        StdlibEntry {
            mir_name: "Shared_new", c_name: "rask_shared_new_ptr",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry::simple("Shared_read", "rask_shared_read_ptr", &[types::I64, types::I64], Some(types::I64), false),
        StdlibEntry::simple("Shared_write", "rask_shared_write_ptr", &[types::I64, types::I64], Some(types::I64), false),
        // Cell — single-owner interior mutability (mem.cell/CE6). `new` takes
        // the value by pointer plus its size, the same way Shared does; `get`
        // hands back the slot address for codegen to load or copy from.
        StdlibEntry {
            mir_name: "Cell_new", c_name: "rask_cell_new",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Cell_get", c_name: "rask_cell_get",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Cell_set", c_name: "rask_cell_set",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Cell_replace", c_name: "rask_cell_replace",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::DerefOrString,
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
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Shared_replace", c_name: "rask_shared_replace",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Mutex_get", c_name: "rask_mutex_get",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
        },
        StdlibEntry {
            mir_name: "Mutex_set", c_name: "rask_mutex_set",
            params: &[types::I64, types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
        },
        StdlibEntry {
            mir_name: "Mutex_replace", c_name: "rask_mutex_replace",
            params: &[types::I64, types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::DerefOrString,
        },
        // `into_inner` consumes the cell and yields what it held — the same read
        // as `get`, just the last one. Freeing the cell here would dangle the
        // pointer it returns.
        StdlibEntry {
            mir_name: "Cell_into_inner", c_name: "rask_cell_get",
            params: &[types::I64], ret_ty: Some(types::I64), can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::DerefOrString,
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
        StdlibEntry::simple("Shared_staged_ptr", "rask_shared_staged_ptr", &[types::I64, types::I64], Some(types::I64), false),
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

        // ── Char predicates ───────────────────────────────────
        StdlibEntry::simple("char_is_digit", "rask_char_is_digit", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_ascii", "rask_char_is_ascii", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_alphabetic", "rask_char_is_alphabetic", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_numeric", "rask_char_is_numeric", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_alphanumeric", "rask_char_is_alphanumeric", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_whitespace", "rask_char_is_whitespace", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_uppercase", "rask_char_is_uppercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_lowercase", "rask_char_is_lowercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_control", "rask_char_is_control", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_ascii_alphabetic", "rask_char_is_ascii_alphabetic", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_ascii_digit", "rask_char_is_ascii_digit", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_ascii_hexdigit", "rask_char_is_ascii_hexdigit", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_is_ascii_punctuation", "rask_char_is_ascii_punctuation", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_to_ascii_lowercase", "rask_char_to_ascii_lowercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_to_ascii_uppercase", "rask_char_to_ascii_uppercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_to_int", "rask_char_to_int", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_to_uppercase", "rask_char_to_uppercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_to_lowercase", "rask_char_to_lowercase", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_len_utf8", "rask_char_len_utf8", &[types::I32], Some(types::I64), false),
        StdlibEntry::simple("char_eq", "rask_char_eq", &[types::I32, types::I32], Some(types::I64), false),

        // ── Path operations ──────────────────────────────────
        // Path = RaskStr. Constructors/conversions use StringOutParam.
        // Option-returning methods return NULL (None) or &thread_local (Some).

        // Raw pointer entries are generated below from
        // rask_stdlib::PTR_METHODS.
    ];

    // ── Atomic operations ──────────────────────────────────
    // mem.atomics/GA1: `Atomic<T>` is the only spelling, so there is one set of
    // rows. Every payload — every integer width, `bool`, a word-sized struct —
    // is a machine word to the runtime, which is why one C implementation
    // covers them: `_Atomic(int64_t)`. The eleven `AtomicU64`-style names the
    // table used to carry are gone with the types.

    // Custom throughout: a word-sized struct payload (GA2) arrives as an
    // address and has to travel as the word itself.
    let atomic = |mir_name: &'static str, c_name: &'static str, params: &'static [types::Type], ret: Option<types::Type>| StdlibEntry {
        mir_name, c_name, params, ret_ty: ret, can_panic: false,
        arg_adapt: ArgAdapt::Custom, ret_adapt: RetAdapt::None,
    };
    entries.push(atomic("Atomic_new", "rask_atomic_int_new", &[types::I64], Some(types::I64)));
    entries.push(StdlibEntry::simple("Atomic_default", "rask_atomic_int_default", &[], Some(types::I64), false));
    entries.push(atomic("Atomic_load", "rask_atomic_int_load", &[types::I64, types::I64], Some(types::I64)));
    entries.push(atomic("Atomic_store", "rask_atomic_int_store", &[types::I64, types::I64, types::I64], None));
    entries.push(atomic("Atomic_swap", "rask_atomic_int_swap", &[types::I64, types::I64, types::I64], Some(types::I64)));
    entries.push(atomic(
        "Atomic_compare_exchange", "rask_atomic_int_compare_exchange",
        &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], Some(types::I64),
    ));
    entries.push(atomic(
        "Atomic_compare_exchange_weak", "rask_atomic_int_compare_exchange_weak",
        &[types::I64, types::I64, types::I64, types::I64, types::I64, types::I64], Some(types::I64),
    ));
    for op in &["fetch_add", "fetch_sub", "fetch_and", "fetch_or", "fetch_xor", "fetch_nand", "fetch_max", "fetch_min"] {
        entries.push(StdlibEntry::simple(
            leak_str(&format!("Atomic_{}", op)),
            leak_str(&format!("rask_atomic_int_{}", op)),
            &[types::I64, types::I64, types::I64], Some(types::I64), false,
        ));
    }
    entries.push(atomic("Atomic_into_inner", "rask_atomic_int_into_inner", &[types::I64], Some(types::I64)));

    // Fences
    entries.push(StdlibEntry::simple("fence", "rask_fence", &[types::I64], None, false));
    entries.push(StdlibEntry::simple("compiler_fence", "rask_compiler_fence", &[types::I64], None, false));

    // ── f64 methods ─────────────────────────────────────────
    // Generated from rask_stdlib::FLOAT_METHODS so the set codegen can call
    // is exactly the set the checker accepts. Arithmetic and comparisons
    // carry no C symbol — codegen emits instructions for those.
    for m in rask_stdlib::FLOAT_METHODS {
        use rask_stdlib::FloatSig;
        let Some(c_symbol) = m.c_symbol else { continue };
        let mir_name = leak_str(&format!("f64_{}", m.name));
        let entry = match m.sig {
            FloatSig::Unary => {
                StdlibEntry::simple(mir_name, c_symbol, &[types::F64], Some(types::F64), false)
            }
            // powi's exponent reaches the call already converted to f64 —
            // there's one `pow` in libm.
            FloatSig::BinaryFloat | FloatSig::BinaryInt => StdlibEntry::simple(
                mir_name, c_symbol, &[types::F64, types::F64], Some(types::F64), false,
            ),
            // bool is i8 in the Rask ABI.
            FloatSig::Predicate => {
                StdlibEntry::simple(mir_name, c_symbol, &[types::F64], Some(types::I8), false)
            }
            // f64_to_string is declared by hand above, next to the other
            // primitives' to_string entries.
            FloatSig::ToString => continue,
            // No C symbol, so unreachable — the `else` above skipped them.
            // Reinterprets its argument, so it needs a real (memcpy) call
            // rather than an instruction — Cast would convert the value.
            FloatSig::ToBits => {
                StdlibEntry::simple(mir_name, c_symbol, &[types::F64], Some(types::I64), false)
            }
            FloatSig::Comparison | FloatSig::Compare | FloatSig::ToInt => continue,
        };
        entries.push(entry);
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every float method the checker accepts and MIR lowers to a call has to
    /// be callable natively. This is the test that would have caught #687:
    /// `x.floor()` type-checked, ran on the interpreter, and died in codegen.
    #[test]
    fn every_float_method_with_a_c_symbol_is_dispatchable() {
        let names: HashSet<&str> = stdlib_entries().iter().map(|e| e.mir_name).collect();
        for m in rask_stdlib::FLOAT_METHODS {
            if m.c_symbol.is_none() {
                continue;
            }
            let mir_name = format!("f64_{}", m.name);
            assert!(
                names.contains(mir_name.as_str()),
                "f64.{}() lowers to `{mir_name}` but codegen has no dispatch entry",
                m.name
            );
        }
    }

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

    /// Every `@native("symbol")` in the stdlib names a symbol that exists.
    ///
    /// `@native` is an assertion, and nothing checked it. `os.exit` was declared
    /// `@native` with no `os_exit` row anywhere, so it type-checked, ran on the
    /// interpreter, and died at codegen with "Function not found: os_exit". The
    /// same thing had already happened to `os.set_env` (#855) — twice is a
    /// pattern, hence #1007.
    ///
    /// The failure mode is the worst one available: the program builds on one
    /// backend and fails on the other with an internal message rather than a
    /// diagnostic, which is exactly what ctrl.panic/S7 says not to do.
    ///
    /// A named symbol has to be reachable — a row in this table, or a
    /// declaration in the runtime header. That covers 240 of the stubs and
    /// needs no list to maintain.
    #[test]
    fn every_named_native_symbol_exists() {
        let header = include_str!("../../../runtime/rask_runtime.h");
        let declared: HashSet<&str> = header
            .split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|w| w.starts_with("rask_"))
            .collect();
        let dispatched: HashSet<&str> =
            stdlib_entries().iter().map(|e| e.c_name).collect();

        let reg = rask_stdlib::StubRegistry::load();
        let mut missing: Vec<String> = Vec::new();
        for type_name in reg.type_names() {
            let Some(t) = reg.get_type(&type_name) else { continue };
            for m in &t.methods {
                let Some(symbol) = m.native.as_deref().filter(|s| !s.is_empty()) else {
                    continue;
                };
                if declared.contains(symbol) || dispatched.contains(symbol) {
                    continue;
                }
                missing.push(format!("{}.{} → {}", type_name, m.name, symbol));
            }
        }
        missing.sort();
        assert!(
            missing.is_empty(),
            "{} stdlib declarations name a native symbol that doesn't exist. Either \
             implement it (a row in `stdlib_entries()` plus the C function) or drop \
             the symbol from the `@native` marker if the call is lowered some other \
             way:\n  {}",
            missing.len(),
            missing.join("\n  ")
        );
    }

    /// `@native` with no symbol derives its name, and those are the ones that
    /// go missing quietly. Each either has a dispatch row or is on this list.
    ///
    /// The list is a snapshot of the gap as it stands, and it may only shrink.
    /// Two kinds of entry are mixed in it deliberately, because from here they
    /// look the same and only trying one tells them apart:
    ///
    ///   - lowered somewhere bespoke: `Vec.fold` and friends are fused into an
    ///     iterator chain, `reflect.*` is resolved at comptime, `math.*` goes to
    ///     libm. These are fine and will stay listed.
    ///   - no entry point at all, which is a bug: the call type-checks, runs on
    ///     the interpreter, and dies at codegen with "Function not found". The
    ///     list is where those stop being invisible. `Command.*` and `Timer.*`
    ///     were the first two families confirmed that way (#1066); `Command` is
    ///     implemented now and `Timer` says `@unimplemented`, so both are off.
    ///
    /// The test fails both ways. An unlisted name with no row is a new gap; a
    /// listed name that has since gained a row is stale and must come off.
    const NATIVE_WITHOUT_A_DISPATCH_ROW: &[&str] = &[
    "FieldInfo.get",
    "FieldInfo.has",
    "Map.capacity",
    "Map.modify",
    "Map.modify_with_default",
    "Map.read",
    "Map.with_capacity",
    "Pool.capacity",
    "Pool.clear",
    "Pool.entries",
    "Pool.get_mut_unchecked",
    "Pool.get_unchecked",
    "Pool.iter",
    "Pool.modify",
    "Pool.read",
    "Pool.remaining",
    "Pool.snapshot",
    "Pool.weak",
    "Pool.with_valid",
    "Pool.with_valid_mut",
    "TaskGroup.join_all",
    "TaskGroup.new",
    "Vec.all",
    "Vec.any",
    "Vec.enumerate",
    "Vec.find",
    "Vec.flat_map",
    "Vec.flatten",
    "Vec.fold",
    "Vec.iter",
    "Vec.max",
    "Vec.min",
    "Vec.modify",
    "Vec.position",
    "Vec.push_with",
    "Vec.read",
    "Vec.reduce",
    "Vec.sort_by_key",
    "Vec.sum",
    "Vec.take",
    "Vec.zip",
    "WeakHandle.upgrade",
    "WeakHandle.valid",
    "Wide.map",
    "Wide.max",
    "Wide.min",
    "Wide.reduce",
    "Wide.zip_with",
    "cstring.to_string",
    "json.encode_pretty",
    "math.acos",
    "math.asin",
    "math.atan",
    "math.atan2",
    "math.cos",
    "math.exp",
    "math.hypot",
    "math.ln",
    "math.log10",
    "math.log2",
    "math.sin",
    "math.tan",
    "math.to_degrees",
    "math.to_radians",
    "os.signals",
    "reflect.align_of",
    "reflect.fields",
    "reflect.is_copy",
    "reflect.is_enum",
    "reflect.is_flat",
    "reflect.is_float",
    "reflect.is_integer",
    "reflect.is_map",
    "reflect.is_optional",
    "reflect.is_resource",
    "reflect.is_struct",
    "reflect.is_vec",
    "reflect.name_of",
    "reflect.size_of",
    ];

    #[test]
    fn derived_native_names_are_listed_or_dispatched() {
        let dispatched: HashSet<&str> =
            stdlib_entries().iter().map(|e| e.mir_name).collect();
        let listed: HashSet<&str> = NATIVE_WITHOUT_A_DISPATCH_ROW.iter().copied().collect();

        let reg = rask_stdlib::StubRegistry::load();
        let mut unlisted: Vec<String> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for type_name in reg.type_names() {
            let Some(t) = reg.get_type(&type_name) else { continue };
            for m in &t.methods {
                if !matches!(m.native.as_deref(), Some("")) {
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
            "{} `@native` declarations have no dispatch row and aren't listed. If the \
             call is lowered somewhere bespoke, add it to \
             NATIVE_WITHOUT_A_DISPATCH_ROW; otherwise it needs an entry point:\n  {}",
            unlisted.len(),
            unlisted.join("\n  ")
        );

        let mut stale: Vec<&str> = NATIVE_WITHOUT_A_DISPATCH_ROW
            .iter()
            .copied()
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
}

