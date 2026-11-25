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
pub(crate) use rask_mir::call_abi::{ArgAdapt, RetAdapt};

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
        // ── String operations ──────────────────────────────────
        StdlibEntry {
            mir_name: "string_clone", c_name: "rask_string_clone",
            params: &[types::I64], ret_ty: None, can_panic: false,
            arg_adapt: ArgAdapt::None, ret_adapt: RetAdapt::None,
        },

        StdlibEntry::simple("Link_hash", "rask_link_hash", &[types::I64], Some(types::I64), false),

        // ── Math operations ────────────────────────────────────
        // i64.abs() (std.math/N2) — libc's llabs, no wrapper needed.
        StdlibEntry::simple("i64_abs", "llabs", &[types::I64], Some(types::I64), false),
        // `llabs` takes a `long long`, so a 128-bit value would be truncated
        // before it was negated (#762).
        StdlibEntry::simple("i128_abs", "rask_i128_abs", &[types::I128], Some(types::I128), true),
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
        &[types::I64, types::I64, types::I64, types::I64, types::I64], Some(types::I64),
    ));
    entries.push(atomic(
        "Atomic_compare_exchange_weak", "rask_atomic_compare_exchange_weak",
        &[types::I64, types::I64, types::I64, types::I64, types::I64], Some(types::I64),
    ));
    for op in &["fetch_add", "fetch_sub", "fetch_and", "fetch_or", "fetch_xor", "fetch_nand", "fetch_max", "fetch_min"] {
        entries.push(atomic(
            leak_str(&format!("Atomic_{}", op)),
            leak_str(&format!("rask_atomic_{}", op)),
            &[types::I64, types::I64, types::I64], Some(types::I64),
        ));
    }
    entries.push(atomic("Atomic_take", "rask_atomic_into_inner", &[types::I64], Some(types::I64)));

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
        // A handle's declared release, called by its own symbol when the frame
        // frees one (`@handle(release: "…")`): the handle's word in, nothing
        // back.
        for release in rask_stdlib::handles::declared_releases() {
            if !entries.iter().any(|e| e.mir_name == release) {
                entries.push(StdlibEntry::simple(release, release, &[types::I64], None, false));
            }
        }
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
/// - an element description lowering passes (`MirConst::Elem`) becomes the
///   three arguments the runtime takes for it (`expand_elem_descs`).
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
            out.push(declared_entry(
                NativeSig {
                    mir_name: format!("{type_name}_{}", m.name),
                    label: format!("{type_name}.{}", m.name),
                    takes_self: m.takes_self,
                    params: &m.params,
                    ret_ty: &m.ret_ty,
                    type_params: &m.type_params,
                    runtime_args: m.runtime_args.as_deref(),
                },
                native,
            ));
        }
    }
    // A free function's MIR name is its own name (`cancelled`, `fence`).
    for f in reg.functions() {
        let Some(native) = f.native.as_ref().filter(|n| !n.symbol.is_empty()) else {
            continue;
        };
        out.push(declared_entry(
            NativeSig {
                mir_name: f.name.clone(),
                label: f.name.clone(),
                takes_self: false,
                params: &f.params,
                ret_ty: &f.ret_ty,
                type_params: &f.type_params,
                runtime_args: None,
            },
            native,
        ));
    }
    out
}

/// What a `@native` declaration's convention is derived from, whether it is
/// a method or a free function.
struct NativeSig<'a> {
    mir_name: String,
    /// How a diagnostic names it: `Vec.push` or `cancelled`.
    label: String,
    takes_self: bool,
    params: &'a [(String, rask_ast::ty::TypeExpr)],
    ret_ty: &'a rask_ast::ty::TypeExpr,
    type_params: &'a [String],
    runtime_args: Option<&'a [rask_stdlib::RuntimeArg]>,
}

fn declared_entry(m: NativeSig<'_>, native: &rask_stdlib::NativeDecl) -> Result<StdlibEntry, String> {
    use rask_ast::ty::TypeExpr;
    use rask_stdlib::NativeReturn;

    let mir_name = m.mir_name.clone();
    let at = || format!("`{}` (`@native(\"{}\")`)", m.label, native.symbol);
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
    let string_valued =
        |t: &TypeExpr| t.bare_name().is_some_and(rask_ast::primitives::is_string_valued);
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

    // Parameters of a type parameter's type, by argument position. The
    // receiver is argument zero. A declaration that lists its runtime
    // arguments puts each parameter where the list does, after any facts
    // ahead of it; a fact that expands to a varying number of arguments
    // can't come first, or the position would depend on the call.
    let first = usize::from(m.takes_self);
    let position = |i: usize| -> Result<usize, String> {
        let Some(list) = m.runtime_args else { return Ok(first + i) };
        let at_list = list
            .iter()
            .position(|r| matches!(r, rask_stdlib::RuntimeArg::Param { index, .. } if *index == i))
            .expect("every parameter is listed once");
        if list[..at_list].iter().any(|r| {
            matches!(
                r,
                rask_stdlib::RuntimeArg::Of(rask_stdlib::TypeFact::Desc | rask_stdlib::TypeFact::HashEq, _)
            )
        }) {
            return Err(format!("{}: parameter {i} follows a fact of varying width", at()));
        }
        Ok(first + at_list)
    };
    let by_address: Vec<usize> = m
        .params
        .iter()
        .enumerate()
        .filter(|(_, (_, ty))| is_type_param(ty))
        .map(|(i, _)| position(i))
        .collect::<Result<_, _>>()?;
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

    let ret = m.ret_ty;
    let (out_param, ret_adapt) = match native.returns {
        Some(NativeReturn::Pointer) if optional(ret).is_some() => (None, RetAdapt::DerefOption),
        Some(NativeReturn::Pointer) => (None, RetAdapt::None),
        Some(NativeReturn::Negative) if optional(ret).is_some() => (None, RetAdapt::NegNone),
        Some(NativeReturn::Negative) => (None, RetAdapt::NegErr),
        Some(NativeReturn::Status) => (None, RetAdapt::Status),
        Some(NativeReturn::Nothing) => (None, RetAdapt::None),
        None if string_valued(ret) => {
            (Some(ArgAdapt::StringOutParam), RetAdapt::FromArgAdapt)
        }
        None if result_ok(ret).is_some_and(|ok| string_valued(&ok)) => {
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
            Some(inner) if string_valued(&inner) => {
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
        (ArgAdapt::WrapArg2, Some(ArgAdapt::AppendOutParam)) => ArgAdapt::WrapArg2AppendOutParam,
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
/// What can panic, so the caller records its location first: the entries
/// that say so, and the runtime symbols `rask_runtime.h` marks `RASK_PANICS`.
pub fn panicking_functions() -> HashSet<String> {
    let mut out: HashSet<String> =
        stdlib_entries().into_iter().filter(|e| e.can_panic).map(|e| e.mir_name.to_string()).collect();
    out.extend(
        rask_stdlib::runtime_header::symbols()
            .filter(|s| rask_stdlib::runtime_header::prototype(s).is_some_and(|p| p.panics))
            .map(str::to_string),
    );
    out
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
        assert!(compared > 300, "only {compared} rows matched a prototype");
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

    /// A call made by its C symbol carries its own `CallAbi` and is declared
    /// from its prototype, so a row keyed by a runtime symbol is never read.
    #[test]
    fn no_internal_row_names_a_runtime_symbol() {
        let shadowed: Vec<&str> = internal_entries()
            .into_iter()
            .filter(|e| rask_stdlib::runtime_header::declares(e.mir_name))
            .map(|e| e.mir_name)
            .collect();
        assert!(shadowed.is_empty(), "rows for calls made by their C symbol: {shadowed:?}");
    }

    /// The internal table holds only names no declaration answers. A row for
    /// a declared `@native` method is a second copy of the declaration, the
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
    ("ParamInfo.is",              Ok_("t_reflect_params")),
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
    ("reflect.params",            Ok_("t_reflect_params")),
    ("reflect.is_enum",           Ok_("t_native_reach_reflect")),
    ("reflect.has_decode",        Ok_("t_format_library_codecs")),
    ("reflect.has_encode",        Ok_("t_format_library_codecs")),
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

