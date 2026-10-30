// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR lowering - transform AST to MIR CFG.

mod closures;
mod collections;
mod concurrency;
mod errors;
mod expr;
mod iterators;
mod json_decode;
mod match_lower;
mod stmt;

use crate::FieldAccess;
use crate::{
    BlockBuilder, MirFunction, MirOperand, MirRValue, MirStmt, MirStmtKind, MirTerminator,
    MirTerminatorKind, MirType, BlockId, LocalId, operand::{MirConst, FunctionRef},
};
use crate::types::{StructLayoutId, EnumLayoutId};
use rask_ast::{
    decl::{ConstDecl, Decl, DeclKind, FnDecl},
    expr::{BinOp, Expr, ExprKind, UnaryOp},
    LineMap, NodeId, Span,
};
use rask_check::layout::{StructLayout, EnumLayout};
use rask_types::Type;
use rask_ast::ty::{ParamMode, TypeExpr};
use std::collections::HashMap;

/// Typed expression result from lowering
type TypedOperand = (MirOperand, MirType);

/// `none` for `Link<T>?` — the null address. See `rask_types::abi`.
pub(crate) const LINK_NONE_SENTINEL: i64 = rask_types::abi::LINK_NONE_SENTINEL;

/// The `none` word for a lowered type that is a niche *option*.
///
/// `Link<T>?` collapses to `Link`, so for a link the value and the option are
/// one type and the word itself carries the answer.
pub(crate) fn mir_niche_none(ty: &MirType) -> Option<i64> {
    match ty {
        MirType::Option(inner) if inner.is_niche_payload() => inner.niche_none(),
        MirType::Link(_) => ty.niche_none(),
        _ => None,
    }
}

/// `T?`, collapsing the link niche. `Link<T>?` *is* one word — the address,
/// with null for `none` — so wrapping it in an `Option` would give the local a
/// tagged-union shape codegen then reads a tag byte out of. That read goes
/// through the sentinel itself, which is not an address you may load from.
///
pub(crate) fn option_of(inner: MirType) -> MirType {
    MirType::Option(Box::new(inner))
}

/// `dst = ` a fresh heap block for a value of type `ty`, `size` bytes long.
///
/// `malloc` only promises 16-byte alignment, so an `@align(64)` value goes
/// through `rask_alloc_aligned`. Both blocks go back through `rask_free`.
pub(crate) fn alloc_block_for(dst: LocalId, size: u32, ty: &MirType) -> MirStmt {
    let size = MirOperand::Constant(MirConst::Int(size.max(1) as i64));
    let (name, args) = match ty.align() {
        a if a > crate::MALLOC_ALIGN => (
            "rask_alloc_aligned",
            vec![size, MirOperand::Constant(MirConst::Int(a as i64))],
        ),
        _ => ("rask_alloc", vec![size]),
    };
    MirStmt::dummy(MirStmtKind::Call {
        dst: Some(dst),
        func: FunctionRef::internal(name.to_string()),
        args,
    })
}


/// Check if a raw Type is a niche-optimized option — `Link<T>?`.
///
/// One word where the value *is* the option, so it carries no tag. Ask for the
/// sentinel, not just the fact.
///
/// By-name only. A generic the checker resolved names its base by `TypeId`, and
/// a free function can't see the table — prefer
/// `MirContext::niche_option_sentinel` wherever a context is in reach.
pub(crate) fn is_niche_option_handle(ty: &Type) -> bool {
    niche_option_sentinel_named(ty).is_some()
}

/// The word that means `none` for a niche option spelled by name.
pub(crate) fn niche_option_sentinel_named(ty: &Type) -> Option<i64> {
    let inner = ty.as_option()?;
    let Type::UnresolvedGeneric { name, .. } = inner else { return None };
    niche_sentinel_for_head(name)
}

/// The `none` word for a niche whose head name is `head`, with or without its
/// type argument attached — a resolved generic names its base by the
/// declaration's own spelling, `"Handle<T>"` and all.
pub(crate) fn niche_sentinel_for_head(head: &str) -> Option<i64> {
    match head.trim() {
        "Link" => Some(LINK_NONE_SENTINEL),
        _ => None,
    }
}

/// An iterator chain recognized from AST method call nesting.
///
/// vec.iter().filter(|x| pred(x)).map(|x| transform(x))
///  ↓source    ↓adapters[0]        ↓adapters[1]
///
/// Fused into a single index-based loop at MIR level.
pub(crate) struct IterChain<'a> {
    /// Source collection (the `.iter()` receiver)
    pub source: &'a Expr,
    /// Adapter operations in application order
    pub adapters: Vec<IterAdapter<'a>>,
}

/// A single iterator adapter operation.
pub(crate) enum IterAdapter<'a> {
    Filter { closure: &'a Expr },
    Map { closure: &'a Expr },
    Take { count: &'a Expr },
    Skip { count: &'a Expr },
    Enumerate,
}

/// #270: classify a function's params for `mutate` write-back. Returns one entry
/// per param: `Some(ty)` for a `mutate` param that needs a pointer of its own,
/// `None` otherwise. A real aggregate's local already is an address, so it stays
/// `None`; a scalar and a container handle both need one, for the same reason —
/// the local holds a value rather than a place (#270, #1197).
///
/// A receiver only gets a pointer from a Rask body. A `@native` method's
/// receiver is whatever its C signature takes, which is the value:
/// `Random.u64(mutate self)` hands `rask_rng_u64` the handle.
fn scalar_mutate_params(f: &rask_ast::decl::FnDecl, ctx: &MirContext) -> Vec<Option<MirType>> {
    let native = f.body_lives_elsewhere();
    f.params
        .iter()
        .map(|p| {
            if native && p.name == "self" {
                return None;
            }
            let written = p.ty.as_ref().filter(|_| p.is_mutate)?;
            mutate_param_passing(ParamMode::Mutate, &p.name, &ctx.resolve_type_expr(written)).0
        })
        .collect()
}

/// Which params are `mutate` on an aggregate — the ones the caller passes by
/// address, so a callee's write is supposed to reach the caller's storage.
fn aggregate_mutate_params(params: &[rask_ast::decl::Param], ctx: &MirContext) -> Vec<bool> {
    params
        .iter()
        .map(|p| {
            let Some(written) = p.ty.as_ref().filter(|_| p.is_mutate) else {
                return false;
            };
            mutate_param_passing(ParamMode::Mutate, &p.name, &ctx.resolve_type_expr(written)).1
        })
        .collect()
}

/// How a parameter of type `ty` is passed: `(Some(ty), _)` for a `mutate`
/// parameter that gets a pointer of its own (a scalar, or a container handle
/// that isn't `self`), `(_, true)` for a `mutate` aggregate whose local
/// already is the caller's address. A declared function, a closure literal
/// and a call through a function value all answer from here, so the two
/// ends of a call can't disagree.
pub(crate) fn mutate_param_passing(mode: ParamMode, name: &str, ty: &MirType) -> (Option<MirType>, bool) {
    if mode != ParamMode::Mutate {
        return (None, false);
    }
    if crate::lower::stmt::mutate_param_needs_own_pointer(name, ty) {
        (Some(ty.clone()), false)
    } else {
        (None, crate::lower::stmt::mutate_param_by_pointer(ty))
    }
}

/// Function signature for type inference
#[derive(Clone)]
struct FuncSig {
    ret_ty: MirType,
    /// #270: per-parameter (is_mutate, scalar type) — populated for user
    /// functions/methods. A `Some(ty)` entry marks a scalar `mutate` param that
    /// is passed by pointer (the caller passes an address, the callee loads/stores
    /// through it); `None` means the param is not a by-pointer scalar mutate.
    /// Empty for extern/stdlib (no scalar mutate write-back).
    scalar_mutate_params: Vec<Option<MirType>>,
    /// Per-parameter: a `mutate` param of an aggregate type, which the caller
    /// passes as an address. A collection element passed here has to be written
    /// back after the call — the caller handed over a copy.
    aggregate_mutate_params: Vec<bool>,
    /// Element type when the function returns `Vec<T>`. `ret_ty` collapses a Vec
    /// to an opaque pointer, so `for x in f()` has nothing to type its binding
    /// from once the checker's node types are out of reach (a closure body, an
    /// instantiated copy) — the declared return type is the remaining record.
    ret_vec_elem: Option<MirType>,
    /// Declared parameter types, positionally. Used to type an
    /// unannotated closure argument's parameters: `|req| { … }` passed to a
    /// `func(Request) -> Response` parameter has nothing else to go on, and
    /// defaulting them to i64 made field access and method dispatch inside the
    /// closure body operate on the wrong type.
    param_tys: Vec<Option<TypeExpr>>,
}

/// Loop context for break/continue
struct LoopContext {
    label: Option<String>,
    /// Block to jump to on `continue`
    continue_block: BlockId,
    /// Block to jump to on `break`
    exit_block: BlockId,
    /// For `break value` - local to assign the value to
    result_local: Option<LocalId>,
    /// ensure_stack depth when loop started — loop-scoped ensures
    /// are stack[ensure_depth..] and must run on break/continue/iteration-end.
    ensure_depth: usize,
    /// `pending_write_backs` depth when the loop's body started. The ones above
    /// it were opened inside the body and are owed on `break`/`continue`.
    writeback_depth: usize,
}

/// One wrapper layer of a type, as seen by `coerce_into_wrapper`.
#[derive(Debug, Clone, PartialEq)]
enum WrapLayer {
    Option,
    Result { err: MirType },
}

/// Every value the program computes at compile time, handed down from the
/// fold (ctrl.comptime/CT2, CT56). A `comptime` position never reaches MIR as
/// code to run: it reaches it as the value, or the program didn't compile.
#[derive(Debug, Clone, Default)]
pub struct Folded {
    /// Each folded node — a `comptime` block, a call to a `comptime func` —
    /// and where its value lives.
    pub nodes: HashMap<NodeId, FoldedNode>,
    /// Module consts whose value is folded, and the node holding it.
    pub consts: HashMap<String, NodeId>,
    /// The data the values live in, by symbol.
    pub data: HashMap<String, ComptimeGlobalMeta>,
    /// Each `comptime` statement the fold settled, by the statement.
    pub statements: HashMap<NodeId, FoldedStmt>,
}

/// What a `comptime` statement came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FoldedStmt {
    /// A `comptime if`: which branch the program keeps — `true` the first.
    /// Only that branch is code; the other was never there.
    Branch(bool),
    /// A `comptime { … }` of statements: it ran while compiling, and nothing
    /// of it is left to run.
    Ran,
}

/// One folded value: its data symbol, and the MIR type its bytes were written
/// as. The bytes and the type come from the same place, so a read can't pick
/// a width the write didn't use.
#[derive(Debug, Clone)]
pub struct FoldedNode {
    pub symbol: String,
    pub ty: MirType,
}

/// What a folded value's data holds.
#[derive(Debug, Clone, PartialEq)]
pub enum FoldedForm {
    /// The value exactly as it sits in memory; a use copies it out.
    Value,
    /// A string's characters. A use is a string literal, which is already
    /// const data with a sentinel refcount (#1074).
    Text,
    /// A list's elements. A use builds a `Vec` the user owns from them.
    List,
    /// A map's entries, key then value. A use inserts them into a new map: a
    /// hash table seeds its layout per process, so there's no table to emit
    /// (#1075).
    Map,
}

/// The data behind a folded value.
#[derive(Debug, Clone)]
pub struct ComptimeGlobalMeta {
    pub form: FoldedForm,
    pub bytes: Vec<u8>,
    /// Element count (for a list or a map)
    pub elem_count: usize,
    /// What a list's elements are, from its checked type. Without it, indexing
    /// one had no element type to resolve, and an empty `Vec<string>` came out
    /// with word-sized slots for sixteen-byte strings.
    pub elem: Option<MirType>,
    /// What a folded `Map`'s keys and values are. The blob is its entries as
    /// `(K, V)` tuples, and lowering builds the map they go into.
    pub map_types: Option<(MirType, MirType)>,
    /// `(byte offset, text)` for each string too long to sit inline.
    /// Codegen emits a static header for the text and writes its address at the
    /// offset — a pointer isn't a byte, so it can't travel in `bytes`.
    pub string_relocs: Vec<(usize, String)>,
}

/// The data symbol for a folded node.
pub fn folded_symbol(node: NodeId) -> String {
    format!("__comptime_{}", node.0)
}

/// Prefix for the writable data slot holding a module-level const's value.
/// Codegen finds the slots by scanning `GlobalRef` names for this prefix, so
/// the two sides can't drift apart.
pub const CONST_SLOT_PREFIX: &str = "__rask_const_slot__";

/// Data-slot name for a module-level const.
pub fn const_slot_name(const_name: &str) -> String {
    format!("{}{}", CONST_SLOT_PREFIX, const_name)
}

/// Name of the thunk that fills a module-level const's slot. One per const,
/// called from the top of main in declaration order.
pub fn const_init_fn_name(const_name: &str) -> String {
    format!("__rask_const_init__{}", const_name)
}

/// Local in an init thunk holding the initializer's value. Named so the
/// measuring pass can read the const's real MIR type back off the thunk.
const CONST_SLOT_VALUE_LOCAL: &str = "__const_slot_value";

/// A literal initializer folds to a constant, so a copy per function is
/// indistinguishable from a shared one. Must agree with `try_eval_const_init`.
fn const_init_is_literal(init: &Expr) -> bool {
    matches!(
        &init.kind,
        ExprKind::Int(..) | ExprKind::Float(..) | ExprKind::String(_) | ExprKind::Bytes(_) | ExprKind::Bool(_)
    )
}

/// Empty tables, for the fields a caller doesn't have. `'static`, so they
/// satisfy any `MirContext<'a>`.
mod empty {
    use super::*;
    use std::sync::OnceLock;

    macro_rules! empty_of {
        ($name:ident, $ty:ty) => {
            pub fn $name() -> &'static $ty {
                static V: OnceLock<$ty> = OnceLock::new();
                V.get_or_init(Default::default)
            }
        };
    }
    empty_of!(strings, std::collections::HashSet<String>);
    empty_of!(folded, Folded);
    empty_of!(node_names, HashMap<NodeId, String>);
    empty_of!(nominal_map, HashMap<String, Type>);
    empty_of!(map_key_fns, HashMap<NodeId, rask_mono::MapKeyFns>);
    empty_of!(decode_defaults, HashMap<(String, String), String>);
}

impl<'a> MirContext<'a> {
    /// A context from the checker's and monomorphizer's output.
    ///
    /// Six call sites used to build this struct field by field — 21 fields each,
    /// across two crates — and they drifted. The comptime evaluator passed an
    /// empty `call_targets` for two months, because an unrelated commit needed
    /// the struct to compile and an empty map was the quickest way there; the
    /// effect was a `comptime { }` block lowering with method dispatch blanked
    /// out while the same code lowered by the main pipeline had it (#425, #727).
    ///
    /// The tables that come straight off `TypedProgram` are read here, so
    /// they can't be forgotten or blanked. The per-node records come from
    /// `records` instead, which the real pipeline builds merged with the
    /// monomorphizer's instantiated bodies (`MonoProgram::node_records`):
    /// taking the unmerged ones off `typed` would lose every generic
    /// instantiation.
    ///
    /// Everything else defaults to empty, with a `with_*` to set it. A new field
    /// is one edit here, and no call site can miss it.
    pub fn new(
        typed: &'a rask_check::TypedProgram,
        struct_layouts: &'a [StructLayout],
        enum_layouts: &'a [EnumLayout],
        records: &'a rask_mono::NodeRecords,
        type_names: &'a HashMap<rask_types::TypeId, String>,
    ) -> Self {
        Self {
            struct_layouts,
            enum_layouts,
            node_types: &records.node_types,
            call_targets: &records.call_targets,
            operator_targets: &records.operator_targets,
            error_wraps: &records.error_wraps,
            fallback_keeps_shape: &records.fallback_keeps_shape,
            err_returns: &records.err_returns,
            escaping_closures: &records.escaping_closures,
            lent_captures: &records.lent_captures,
            field_reuses: &records.field_reuses,
            return_moves: &records.return_moves,
            task_bound_closures: &records.task_bound_closures,
            type_names,
            // Straight off the checker — never optional.
            type_defs: &typed.types,
            mutate_self_fns: Some(&typed.mutate_self_fns),
            interface_coercions: &typed.interface_coercions,
            c_fn_pointers: &typed.c_fn_pointers,
            c_contexts: &typed.c_contexts,
            try_chain_placement: &typed.try_chain_placement,
            inferred_fn_ret: &typed.inferred_fn_ret,
            // Defaults; the `with_*` below set the ones a caller has.
            folded: empty::folded(),
            extern_funcs: empty::strings(),
            package_modules: empty::strings(),
            line_map: None,
            source_file: None,
            interface_methods: HashMap::new(),
            call_rewrites: empty::node_names(),
            map_key_fns: empty::map_key_fns(),
            decode_defaults: empty::decode_defaults(),
            resource_types: empty::strings(),
            nominal_underlying: empty::nominal_map(),
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(Default::default()),
            const_slot_types: std::cell::RefCell::new(HashMap::new()),
        }
    }

    pub fn with_folded(mut self, folded: &'a Folded) -> Self {
        self.folded = folded;
        self
    }

    pub fn with_extern_funcs(mut self, funcs: &'a std::collections::HashSet<String>) -> Self {
        self.extern_funcs = funcs;
        self
    }

    pub fn with_package_modules(
        mut self,
        modules: &'a std::collections::HashSet<String>,
    ) -> Self {
        self.package_modules = modules;
        self
    }

    pub fn with_source(mut self, path: &'a str, line_map: Option<&'a LineMap>) -> Self {
        self.source_file = Some(path);
        self.line_map = line_map;
        self
    }

    pub fn with_interface_methods(mut self, methods: HashMap<String, Vec<String>>) -> Self {
        self.interface_methods = methods;
        self
    }

    /// What monomorphization decided per call: which copy it names, and the
    /// key functions a map it builds needs.
    pub fn with_mono_calls(mut self, mono: &'a rask_mono::MonoProgram) -> Self {
        self.call_rewrites = &mono.call_rewrites;
        self.map_key_fns = &mono.map_key_fns;
        self.decode_defaults = &mono.decode_defaults;
        self
    }

    pub fn with_resource_types(mut self, types: &'a std::collections::HashSet<String>) -> Self {
        self.resource_types = types;
        self
    }

    pub fn with_nominal_underlying(mut self, map: &'a HashMap<String, Type>) -> Self {
        self.nominal_underlying = map;
        self
    }

}

/// No extern takes a closure through a context pointer.
pub(crate) static EMPTY_C_CONTEXTS: std::sync::LazyLock<HashMap<String, rask_types::CContext>> =
    std::sync::LazyLock::new(HashMap::new);

/// No function value coerced to a C function pointer.
pub(crate) static EMPTY_C_FN_POINTERS: std::sync::LazyLock<std::collections::HashSet<NodeId>> =
    std::sync::LazyLock::new(std::collections::HashSet::new);

/// Layout context for MIR lowering — struct/enum metadata from monomorphization.
pub struct MirContext<'a> {
    pub struct_layouts: &'a [StructLayout],
    /// GC9: spans of methods whose `self` is mutable, from the type checker.
    /// A declared `mutate self` is visible on the Param, but an *inferred* one
    /// isn't — this carries the checker's answer so lowering doesn't re-derive
    /// the rule. Keyed by (span.start, span.end, file_id).
    ///
    /// `None` means no type checker ran, which is only legitimate for the
    /// synthetic lowering units the unit tests hand-build. A real compile that
    /// arrives without it is a plumbing bug, and `method_mutates_self` says so
    /// rather than quietly answering "doesn't mutate".
    pub mutate_self_fns: Option<&'a std::collections::HashSet<(usize, usize, u16)>>,
    pub enum_layouts: &'a [EnumLayout],
    /// Type information for each expression node from type checking
    pub node_types: &'a HashMap<NodeId, Type>,
    /// TypeId → name mapping from the type checker, for resolving Named types.
    pub type_names: &'a HashMap<rask_types::TypeId, String>,
    /// The checker's type table.
    ///
    /// A layout has dropped `@resource` and substituted its field types by the
    /// time it exists, and `std.reflect`'s `is_resource` and flatness walk are
    /// asking about exactly those (#791). This is the declarations as the checker
    /// recorded them — the same thing the interpreter reads off its AST maps.
    pub type_defs: &'a rask_check::TypeTable,
    /// Every value computed at compile time. Lowering reads these instead
    /// of lowering the code that would compute them.
    pub folded: &'a Folded,
    /// Names of extern "C" functions — calls emit FunctionRef::extern_c().
    pub extern_funcs: &'a std::collections::HashSet<String>,
    /// Names of imported external packages — used to recognize cross-package
    /// qualified calls like `pkg.func()` and `pkg.Type` field access.
    pub package_modules: &'a std::collections::HashSet<String>,
    /// Line map for converting byte offsets to line:col (None in tests)
    pub line_map: Option<&'a LineMap>,
    /// Source file path for runtime error messages (None in tests)
    pub source_file: Option<&'a str>,
    /// Cross-function Vec element types inferred from push/set calls.
    /// Key: tracking path (e.g. "v", "self.history"), Value: element MirType.
    /// Shared across function lowerings via RefCell.
    ///
    /// The key is just the path, so two functions with a local of the same name
    /// hash to the same entry. A name used for a `Vec<string>` in one function
    /// and a `Vec<i64>` in another is a conflict, and guessing either way
    /// miscompiles the other — so a conflicting name is dropped from the map
    /// and never re-added, leaving those functions on their own per-function
    /// tracking. Two `test` blocks that both call their vector `v` used to make
    /// the second one fail codegen and vanish from the run.
    pub shared_elem_types: std::cell::RefCell<HashMap<String, MirType>>,
    /// Tracking paths seen with more than one element type. Kept out of
    /// `shared_elem_types` for good.
    pub shared_elem_conflicts: std::cell::RefCell<std::collections::HashSet<String>>,
    /// Interface method lists for interface object dispatch.
    /// Key: interface name, Value: method names in declaration order.
    pub interface_methods: HashMap<String, Vec<String>>,
    /// TR5: implicit interface coercion sites. NodeId of expression → interface name.
    pub interface_coercions: &'a HashMap<NodeId, String>,
    /// CB1/CB2: function values passed where C wants a function pointer.
    pub c_fn_pointers: &'a std::collections::HashSet<NodeId>,
    /// CB3: externs whose callback takes a closure through its context.
    pub c_contexts: &'a HashMap<String, rask_types::CContext>,
    /// ER31a: `try` sites whose propagated error gets wrapped in a variant of
    /// the enclosing function's error enum, keyed by the `try` expression.
    pub error_wraps: &'a HashMap<NodeId, rask_types::ErrorWrap>,
    /// ER14a: `??` sites whose right side is still wrapped, so the present
    /// path hands back the left operand instead of its payload.
    pub fallback_keeps_shape: &'a std::collections::HashSet<NodeId>,
    /// ER9: `return` values that go to the error side.
    pub err_returns: &'a std::collections::HashSet<NodeId>,
    /// CM1: closure literals that outlive the frame that built them. Those
    /// carry their captures; the rest hold the address and write through it.
    pub escaping_closures: &'a std::collections::HashSet<NodeId>,
    /// CM3: for those closures, the captures that are lent parameters of the
    /// frame that built them (ownership). Held, never freed by the closure.
    pub lent_captures: &'a std::collections::HashMap<NodeId, Vec<String>>,
    /// Assignments whose new value takes the old one: the slot's old value is
    /// the new one's now, so nothing is released before the write (ownership).
    pub field_reuses: &'a std::collections::HashSet<NodeId>,
    /// The bindings each `return` moves, keyed by the returned expression
    /// (ownership). A `for` body over a sequence returns from inside a
    /// closure, and the frame learns from this what left on that exit.
    pub return_moves: &'a std::collections::HashMap<NodeId, Vec<String>>,
    /// Closures that captured a link or a `Local` box (#1356).
    pub task_bound_closures: &'a std::collections::HashSet<NodeId>,
    /// ER16a: `try` node → the postfix-chain step it attaches to. The branch
    /// goes there, and the rest of the chain works on the payload.
    pub try_chain_placement: &'a HashMap<NodeId, NodeId>,
    /// Call expression NodeId → mangled callee name for generic function calls.
    pub call_rewrites: &'a HashMap<NodeId, String>,
    /// A map-building call → the `hash`/`eq` its keys go through (#1391).
    pub map_key_fns: &'a HashMap<NodeId, rask_mono::MapKeyFns>,
    /// (struct, field) → the method that builds that field's decode default.
    pub decode_defaults: &'a HashMap<(String, String), String>,
    /// CALL6: the receiver type dispatch actually selected, per call node.
    ///
    /// Authoritative for method-name qualification. `node_types` holds the type
    /// assigned to the receiver *expression*, which is frequently an unresolved
    /// variable or missing outright; this is the applied type the checker
    /// dispatched on, so lowering reads it instead of guessing a prefix from
    /// the receiver's syntactic shape.
    pub call_targets: &'a HashMap<NodeId, rask_check::Callee>,
    /// OR1: operator calls the checker resolved to a conformance.
    ///
    /// `a * b` is a machine instruction on some pairs and a call on others, and
    /// on a primitive receiver it's the *right* operand that decides. Lowering
    /// can't re-derive that from the receiver's MIR type — an `f64` receiver
    /// looks like plain arithmetic either way — so it reads what the checker
    /// settled on.
    pub operator_targets: &'a HashMap<NodeId, rask_check::OperatorTarget>,
    /// Type names marked with `@resource` — used for resource tracking ops (C1/C2).
    pub resource_types: &'a std::collections::HashSet<String>,
    /// Nominal newtype name → the type it wraps, as a type string.
    ///
    /// `type Id = u64 implements …` has no layout of its own: it *is* a u64 with a
    /// distinct identity, so it's transparent in MIR. Without this the name
    /// resolved to a bare `Ptr` with nothing allocated behind it, and
    /// construction stored through an uninitialised pointer (#445).
    pub nominal_underlying: &'a HashMap<String, Type>,
    /// Module-level const name → the MIR type its global slot holds.
    ///
    /// Filled by `compute_const_slot_types` before any function is lowered.
    /// It has to come from actually lowering the initializer: the checker's
    /// type for `time.Instant.now()` is the struct `Instant`, but lowering
    /// resolves the stdlib signature to a plain `i64`, and a reference that
    /// guessed the struct would deref a timestamp as a pointer. Left empty,
    /// consts keep the old re-evaluate-per-function behaviour.
    pub const_slot_types: std::cell::RefCell<HashMap<String, MirType>>,
    /// Function name → return type the checker inferred, for the ones that don't
    /// declare it. Only consulted when there's no annotation to read.
    pub inferred_fn_ret: &'a HashMap<String, Type>,
}

/// For contexts built without checker data (tests, comptime): no function has an
/// inferred return type, so every signature comes from its annotation.
pub static EMPTY_INFERRED_RET: std::sync::LazyLock<HashMap<String, Type>> =
    std::sync::LazyLock::new(HashMap::new);

impl<'a> MirContext<'a> {
    /// A function's MIR return type: the declared one, else the type the checker
    /// inferred for it, else void.
    ///
    /// The last step is a real answer only for a function with no `return` value
    /// — `func f() { }`. A missing annotation on `func f() { return 41 }` used to
    /// land there too, so the signature said void while the body returned an i64
    /// and Cranelift rejected the function (#571).
    pub fn fn_ret_ty(&self, name: &str, declared: Option<&TypeExpr>) -> MirType {
        if let Some(s) = declared {
            return self.resolve_type_expr(s);
        }
        match self.inferred_fn_ret.get(name) {
            Some(ty) => self.type_to_mir(ty),
            None => MirType::Void,
        }
    }

    /// Empty context for tests that don't need layouts or type information.
    pub fn empty_with_map(map: &'a HashMap<NodeId, Type>) -> MirContext<'a> {
        static EMPTY_FOLDED: std::sync::LazyLock<Folded> =
            std::sync::LazyLock::new(Folded::default);
        static EMPTY_EXTERNS: std::sync::LazyLock<std::collections::HashSet<String>> =
            std::sync::LazyLock::new(std::collections::HashSet::new);
        static EMPTY_PACKAGES: std::sync::LazyLock<std::collections::HashSet<String>> =
            std::sync::LazyLock::new(std::collections::HashSet::new);
        static EMPTY_TYPE_NAMES: std::sync::LazyLock<HashMap<rask_types::TypeId, String>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_COERCIONS: std::sync::LazyLock<HashMap<NodeId, String>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_ERROR_WRAPS: std::sync::LazyLock<HashMap<NodeId, rask_types::ErrorWrap>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_COALESCE_SHAPE: std::sync::LazyLock<std::collections::HashSet<NodeId>> =
            std::sync::LazyLock::new(std::collections::HashSet::new);
        static EMPTY_RETURN_MOVES: std::sync::LazyLock<std::collections::HashMap<NodeId, Vec<String>>> =
            std::sync::LazyLock::new(std::collections::HashMap::new);
        static EMPTY_LENT: std::sync::LazyLock<std::collections::HashMap<NodeId, Vec<String>>> =
            std::sync::LazyLock::new(std::collections::HashMap::new);
        static EMPTY_ESCAPING: std::sync::LazyLock<std::collections::HashSet<NodeId>> =
            std::sync::LazyLock::new(std::collections::HashSet::new);
        static EMPTY_TRY_PLACEMENT: std::sync::LazyLock<HashMap<NodeId, NodeId>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_REWRITES: std::sync::LazyLock<HashMap<NodeId, String>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_TARGETS: std::sync::LazyLock<HashMap<NodeId, rask_check::Callee>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_OPERATOR_TARGETS:
            std::sync::LazyLock<HashMap<NodeId, rask_check::OperatorTarget>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_RESOURCE_TYPES: std::sync::LazyLock<std::collections::HashSet<String>> =
            std::sync::LazyLock::new(std::collections::HashSet::new);
        static EMPTY_NOMINAL: std::sync::LazyLock<HashMap<String, Type>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_TYPE_DEFS: std::sync::LazyLock<rask_check::TypeTable> =
            std::sync::LazyLock::new(Default::default);
        MirContext {
            struct_layouts: &[],
            mutate_self_fns: None,
            enum_layouts: &[],
            node_types: map,
            type_names: &EMPTY_TYPE_NAMES,
            type_defs: &EMPTY_TYPE_DEFS,
            folded: &EMPTY_FOLDED,
            extern_funcs: &EMPTY_EXTERNS,
            package_modules: &EMPTY_PACKAGES,
            line_map: None,
            source_file: None,
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(std::collections::HashSet::new()),
            interface_methods: HashMap::new(),
            interface_coercions: &EMPTY_COERCIONS,
            c_fn_pointers: &EMPTY_ESCAPING,
            c_contexts: &crate::lower::EMPTY_C_CONTEXTS,
            error_wraps: &EMPTY_ERROR_WRAPS,
            fallback_keeps_shape: &EMPTY_COALESCE_SHAPE,
            err_returns: &EMPTY_COALESCE_SHAPE,
            escaping_closures: &EMPTY_ESCAPING,
            lent_captures: &EMPTY_LENT,
            field_reuses: &EMPTY_ESCAPING,
            return_moves: &EMPTY_RETURN_MOVES,
            task_bound_closures: &EMPTY_ESCAPING,
            try_chain_placement: &EMPTY_TRY_PLACEMENT,
            call_rewrites: &EMPTY_REWRITES,
            map_key_fns: empty::map_key_fns(),
            decode_defaults: empty::decode_defaults(),
            call_targets: &EMPTY_TARGETS,
            operator_targets: &EMPTY_OPERATOR_TARGETS,
            resource_types: &EMPTY_RESOURCE_TYPES,
            nominal_underlying: &EMPTY_NOMINAL,
            const_slot_types: std::cell::RefCell::new(HashMap::new()),
            inferred_fn_ret: &EMPTY_INFERRED_RET,
        }
    }

    /// A field-less **stdlib** struct is a runtime handle, not an aggregate.
    ///
    /// The stdlib is full of these: `public struct File { }`, and the same
    /// shape for `TcpConnection`, `Metadata`, `Instant`, `Random`,
    /// `ThreadPool`, and the handle types in async.rk and thread.rk. The
    /// `struct` exists so `extend` has somewhere to hang methods; the value is
    /// a file descriptor or a pointer. Typed as an aggregate, a local holding
    /// one is an *address*, so reading a handle out of a `T or E` bound the
    /// address of the result slot instead of the descriptor in it — which is
    /// how the native HTTP server accepted connections and then read from a
    /// made-up fd (#673).
    ///
    /// Stdlib only, and the name set is derived from the stub sources rather
    /// than listed here. A user's field-less struct is a real value with its
    /// own identity: `struct Bad { }` used as the error side of `i64 or Bad`
    /// is told apart from the ok side by its MIR type, and collapsing it to
    /// i64 makes both branches identical.
    fn struct_or_handle(&self, name: &str, idx: u32, sl: &StructLayout) -> MirType {
        if sl.fields.is_empty()
            && rask_stdlib::mir_metadata::stdlib_type_names().contains(name)
        {
            return MirType::I64;
        }
        MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align))
    }

    /// The layout for `name`. A program type sharing a stdlib type's name has
    /// a symbol of its own by now (#1333), so a name names one layout.
    pub fn find_struct(&self, name: &str) -> Option<(u32, &StructLayout)> {
        self.struct_layouts
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
            .map(|(i, s)| (i as u32, s))
    }

    /// AN1: the declared type of one field of a user annotation.
    ///
    /// Annotations register as nominal structs in the checker's table but never
    /// get a layout — nothing constructs one (AN3), so mono has no reason to
    /// lay it out. The table is where the declaration still is.
    pub fn annotation_field_type(&self, annotation: &str, field: &str) -> Option<MirType> {
        use rask_types::TypeDef;
        let id = match self.type_defs.lookup(annotation)? {
            rask_types::Type::Named(id) => id,
            _ => return None,
        };
        let TypeDef::Struct { fields, .. } = self.type_defs.get(id)? else {
            return None;
        };
        fields
            .iter()
            .find(|(name, _)| name == field)
            .map(|(_, ty)| self.type_to_mir(ty))
    }

    /// The type of one payload field of an enum variant, read off the checker's
    /// declaration with the scrutinee's type arguments substituted in.
    ///
    /// A generic enum's shared layout gives every type parameter one word and
    /// records a placeholder type in the slot, so the layout can't say what a
    /// payload binding actually holds. `match m { Maybe.Just(v) => … }` on a
    /// `Maybe<Wrap<i64>>` bound `v` as an integer, and the arm loaded the inner
    /// struct's 8 bytes and dereferenced them as a pointer (#871).
    ///
    /// `None` for anything the layout already gets right: a concrete payload, a
    /// non-generic enum, a scrutinee whose instantiation isn't settled.
    pub fn variant_payload_mir(
        &self,
        scrutinee: Option<&Type>,
        variant: &str,
        field_index: usize,
    ) -> Option<MirType> {
        let (base, args) = match scrutinee? {
            Type::Generic { base, args } => (self.type_names.get(base)?.clone(), args),
            Type::UnresolvedGeneric { name, args } => (name.clone(), args),
            _ => return None,
        };
        let bare = base.as_str().trim();
        let id = self.type_defs.get_type_id(bare)?;
        let rask_types::TypeDef::Enum { type_params, variants, .. } = self.type_defs.get(id)?
        else {
            return None;
        };
        let bare_variant = variant.rsplit('.').next().unwrap_or(variant);
        let declared = variants
            .iter()
            .find(|(n, _)| n == bare_variant)?
            .1
            .get(field_index)?;
        // Only a payload declared *as* one of the type parameters. Anything
        // concrete is already spelled out in the layout.
        let Type::UnresolvedNamed(param) = declared else { return None };
        let pos = type_params.iter().position(|p| p == param)?;
        let rask_types::GenericArg::Type(arg) = args.get(pos)? else { return None };
        Some(self.type_to_mir(arg))
    }

    /// The layout of one *instantiation* of a generic type, when mono emitted a
    /// separate one.
    ///
    /// Generic types share a single layout with a word-sized placeholder per type
    /// parameter, which is right until a type argument is an inline aggregate
    /// wider than a word. Mono emits `One$Big` for those; `One<i64>` has no
    /// instance layout and falls through to the shared one. The name comes from
    /// `rask_check::layout::generic_instance_name`, so both sides spell it the same way by
    /// construction (#781).
    pub fn generic_instance_mir_type(
        &self,
        base: &rask_types::TypeId,
        args: &[rask_types::GenericArg],
    ) -> Option<MirType> {
        let name = self.generic_instance_layout_name(base, args)?;
        if let Some((idx, sl)) = self.find_struct(&name) {
            return Some(MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align)));
        }
        let (idx, el) = self.find_enum(&name)?;
        Some(MirType::Enum(EnumLayoutId::new(idx, el.size, el.align)))
    }

    /// `Base$Arg$Arg` for a resolved generic type, or `None` when an argument
    /// can't be named.
    fn generic_instance_layout_name(
        &self,
        base: &rask_types::TypeId,
        args: &[rask_types::GenericArg],
    ) -> Option<String> {
        let base_name = self.type_names.get(base)?;
        let bare = base_name.trim();
        let mut arg_tys = Vec::with_capacity(args.len());
        for arg in args {
            let rask_types::GenericArg::Type(t) = arg else { return None };
            arg_tys.push((**t).clone());
        }
        rask_check::layout::generic_instance_name(bare, &arg_tys, self.type_names)
    }

    /// The instance layout name for a checker type, in either spelling.
    ///
    /// An instantiated copy's types were built by rewriting type strings, so its
    /// `One<Big>` is an `UnresolvedGeneric` carrying a plain name rather than a
    /// `Generic` carrying an interned id. Both name the same layout (#814).
    fn instance_layout_name_of(&self, ty: Option<&Type>) -> Option<String> {
        match ty? {
            Type::Generic { base, args } => self.generic_instance_layout_name(base, args),
            Type::UnresolvedGeneric { name, args } => {
                let bare = name.trim();
                let mut arg_tys = Vec::with_capacity(args.len());
                for arg in args {
                    let rask_types::GenericArg::Type(t) = arg else { return None };
                    arg_tys.push((**t).clone());
                }
                rask_check::layout::generic_instance_name(bare, &arg_tys, self.type_names)
            }
            _ => None,
        }
    }

    /// The instance struct layout for a checker type, if it has one.
    pub fn generic_instance_struct(&self, ty: Option<&Type>) -> Option<(u32, &StructLayout)> {
        let name = self.instance_layout_name_of(ty)?;
        self.find_struct(&name)
    }

    /// The instance enum layout for a checker type, if it has one.
    pub fn generic_instance_enum(&self, ty: Option<&Type>) -> Option<(u32, &EnumLayout)> {
        let name = self.instance_layout_name_of(ty)?;
        self.find_enum(&name)
    }

    /// Size in bytes for a MirType. Now just delegates to `MirType::size()` since
    /// StructLayoutId/EnumLayoutId carry their real byte sizes.
    pub fn mir_type_size(&self, ty: &MirType) -> u32 {
        ty.size()
    }

    /// `find_enum`, accepting a name with written type arguments.
    ///
    /// `Holder<i64>.Full(4)` reaches lowering as the name `Holder<i64>`; the
    /// layouts are keyed by the bare name. `find_struct_written` has done this for
    /// structs since the beginning, which is why the struct form worked and the
    /// enum form didn't (#782).
    pub fn find_enum_written(&self, name: &str) -> Option<(u32, &EnumLayout)> {
        if let Some(found) = self.find_enum(name) {
            return Some(found);
        }
        // IM3, the same reason `resolve_type_name` follows aliases: a transparent
        // alias is its target, so a variant reached through it is the target's
        // variant. Without this `Shade.Red` under `type alias Shade = Colour`
        // found no enum, fell through to a bare constant, and the comparison
        // that followed read that integer as an address — a segfault, not a
        // wrong answer (#998).
        if let Some(target) = self.type_defs.alias_target_name(name) {
            if target != name {
                if let Some(found) = self.find_enum(&target) {
                    return Some(found);
                }
            }
        }
        let base = name.trim();
        if base == name {
            return None;
        }
        self.find_enum(base)
    }

    pub fn find_enum(&self, name: &str) -> Option<(u32, &EnumLayout)> {
        self.enum_layouts
            .iter()
            .enumerate()
            .find(|(_, e)| e.name == name)
            .map(|(i, e)| (i as u32, e))
    }
    /// The MIR type of a written type. A container is `Container(kind)`, as
    /// in `type_to_mir`.
    pub fn resolve_type_expr(&self, ty: &TypeExpr) -> MirType {
        let mir = self.written_layout(ty);
        if mir != MirType::Ptr {
            return mir;
        }
        match ty.name().as_deref().and_then(crate::ContainerKind::from_head) {
            Some(kind) => MirType::Container(kind),
            None => mir,
        }
    }

    fn written_layout(&self, ty: &TypeExpr) -> MirType {
        match ty {
            TypeExpr::Unit | TypeExpr::NoneType => MirType::Void,
            TypeExpr::Optional(inner) => option_of(self.resolve_type_expr(inner)),
            TypeExpr::Result { ok, err } => MirType::Result {
                ok: Box::new(self.resolve_type_expr(ok)),
                err: Box::new(self.resolve_type_expr(err)),
            },
            // Same order the checker canonicalizes to — by member name. A
            // union's member index is what says which member is present, so the
            // frame that writes one and the frame that reads it have to agree
            // (#1103).
            TypeExpr::Union(members) => {
                let mut members: Vec<&TypeExpr> = members.iter().collect();
                members.sort_by_key(|m| m.to_string());
                MirType::Union(members.into_iter().map(|m| self.resolve_type_expr(m)).collect())
            }
            TypeExpr::Tuple(elems) => {
                MirType::Tuple(elems.iter().map(|e| self.resolve_type_expr(e)).collect())
            }
            // A literal length, then a module-level `const` naming one — read
            // from the checker's table so the two can't disagree (#906). Anything
            // still symbolic keeps 0, which preserves the element type.
            TypeExpr::Array { elem, len } => MirType::Array {
                elem: Box::new(self.resolve_type_expr(elem)),
                len: len
                    .parse::<u32>()
                    .ok()
                    .or_else(|| self.type_defs.const_length(len).and_then(|n| u32::try_from(n).ok()))
                    .unwrap_or(0),
            },
            // A closure is a pointer to its block, and saying so in the type is
            // what lets a carrier holding one give it back (#1253).
            TypeExpr::Func { .. } => MirType::FuncPtr(crate::types::SignatureId(0)),
            // `any io.Writer` is the stdlib's `Writer`, whose symbol is its bare
            // name. Spelled from the written text it was an interface called
            // "io.Writer" that no vtable is filed under, and a call through it
            // couldn't be lowered (#1309). The table knows import aliases, so
            // `any i.Writer` under `import io as i` is the same one.
            TypeExpr::Any(interface) => MirType::InterfaceObject {
                interface_name: match self.type_defs.module_interface(interface) {
                    Some(id) => self.type_defs.type_name(id),
                    None => interface.to_string(),
                },
            },
            TypeExpr::RawPtr(_) | TypeExpr::FixedCount { .. } | TypeExpr::Int(_) => MirType::Ptr,
            TypeExpr::Named { path, args } if args.is_empty() => self.resolve_type_name(&path.join(".")),
            TypeExpr::Named { path, args } => self.resolve_generic_expr(&path.join("."), args),
        }
    }

    /// `Name<args>` as written.
    fn resolve_generic_expr(&self, head: &str, args: &[TypeExpr]) -> MirType {
        match (head, args) {
            ("Result", [ok, err]) => MirType::Result {
                ok: Box::new(self.resolve_type_expr(ok)),
                err: Box::new(self.resolve_type_expr(err)),
            },
            ("Option", [inner]) => option_of(self.resolve_type_expr(inner)),
            ("Link", [node]) => match self.resolve_type_expr(node) {
                MirType::Struct(sid) => MirType::Link(sid),
                _ => MirType::Ptr,
            },
            // `Heap<T>` is a block address, always — that is what makes it a
            // type rather than a fact about a binding. The payload keeps its
            // container kind, since `drop` has to free what it points at.
            ("Heap", [inner]) => MirType::Heap(Box::new(self.resolve_type_expr(inner))),
            (head, _) if lowers_to_ptr(head) => MirType::Ptr,
            _ => {
                // A generic instantiation whose type argument is an inline
                // aggregate has a layout of its own, not found under the base
                // name: `first$Big(o: One<Big>)` was given the shared 8-byte
                // layout while its caller passed 24 bytes (#781).
                let arg_tys: Vec<Type> = args.iter().map(rask_check::layout::field_type).collect();
                if self.nominal_underlying.contains_key(head) {
                    let generic: Vec<rask_types::GenericArg> =
                        arg_tys.iter().cloned().map(|t| rask_types::GenericArg::Type(Box::new(t))).collect();
                    return self.resolve_unresolved_generic(head, &generic);
                }
                if let Some(name) = rask_check::layout::generic_instance_name(head, &arg_tys, self.type_names) {
                    if let Some((idx, sl)) = self.find_struct(&name) {
                        return MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align));
                    }
                    if let Some((idx, el)) = self.find_enum(&name) {
                        return MirType::Enum(EnumLayoutId::new(idx, el.size, el.align));
                    }
                }
                self.resolve_type_name(head)
            }
        }
    }

    /// A generic newtype at these arguments is what it wraps at them.
    /// `None` when `def` isn't a newtype.
    fn newtype_at(&self, def: Option<&rask_types::TypeDef>, args: &[rask_types::GenericArg]) -> Option<MirType> {
        let Some(rask_types::TypeDef::NominalAlias { type_params, underlying, .. }) = def else {
            return None;
        };
        let map: HashMap<String, Type> = type_params
            .iter()
            .cloned()
            .zip(args.iter().filter_map(|a| match a {
                rask_types::GenericArg::Type(t) => Some((**t).clone()),
                rask_types::GenericArg::ConstUsize(_) => None,
            }))
            .collect();
        Some(self.type_to_mir(&rask_check::substitute_type(underlying, &map)))
    }

    /// A generic the checker left as a name with arguments: `One<Big>` in an
    /// instantiated copy, `Channel<i64>` before its name resolved.
    fn resolve_unresolved_generic(&self, name: &str, args: &[rask_types::GenericArg]) -> MirType {
        if self.nominal_underlying.contains_key(name) {
            let def = self.type_defs.get_type_id(name).and_then(|id| self.type_defs.get(id));
            if let Some(wrapped) = self.newtype_at(def, args) {
                return wrapped;
            }
        }
        match name {
            name if lowers_to_ptr(name) => MirType::Ptr,
            _ => {
                // An instantiation whose argument is an inline aggregate has a
                // layout of its own, not found under the base name (#781).
                let arg_tys: Option<Vec<Type>> = args
                    .iter()
                    .map(|a| match a {
                        rask_types::GenericArg::Type(t) => Some((**t).clone()),
                        rask_types::GenericArg::ConstUsize(_) => None,
                    })
                    .collect();
                if let Some(instance) = arg_tys
                    .and_then(|tys| rask_check::layout::generic_instance_name(name, &tys, self.type_names))
                {
                    if let Some((idx, sl)) = self.find_struct(&instance) {
                        return MirType::Struct(StructLayoutId::new(idx, sl.size, sl.align));
                    }
                    if let Some((idx, el)) = self.find_enum(&instance) {
                        return MirType::Enum(EnumLayoutId::new(idx, el.size, el.align));
                    }
                }
                self.resolve_type_name(name)
            }
        }
    }

    /// The MIR type a single type name stands for.
    pub fn resolve_type_name(&self, name: &str) -> MirType {
        // IM3: a transparent alias is its target, so the layout to find is the
        // target's: `let d: Span = …` under `import time.Duration as Span` was
        // matching `stdlib/builtins.rk`'s own `Span` layout by name (#923, #975).
        if let Some(target) = self.type_defs.alias_target(name) {
            return self.resolve_type_expr(target);
        }
        match name {
            "i8" => MirType::I8,
            "i16" => MirType::I16,
            "i32" => MirType::I32,
            "i64" => MirType::I64,
            "i128" => MirType::I128,
            "isize" => MirType::isize_ty(),
            "u8" => MirType::U8,
            "u16" => MirType::U16,
            "u32" => MirType::U32,
            "u64" => MirType::U64,
            "u128" => MirType::U128,
            "usize" => MirType::usize_ty(),
            "f32" => MirType::F32,
            "f64" => MirType::F64,
            "bool" => MirType::Bool,
            "char" => MirType::Char,
            // `bytes` is a string's value without the UTF-8 rule: same layout,
            // same refcount, same runtime (std.bytes/BY3).
            "string" | "bytes" => MirType::Buffer,
            // Bare `Error` is `any Error` written short (#1095). BI2 reserves the
            // name, so nothing else reaches here under it.
            "Error" => MirType::InterfaceObject { interface_name: "Error".to_string() },
            name if lowers_to_ptr(name) => MirType::Ptr,
            _ => {
                // A nominal newtype has no layout — it is whatever it wraps.
                if let Some(underlying) = self.nominal_underlying.get(name) {
                    return self.type_to_mir(underlying);
                }
                if let Some((idx, sl)) = self.find_struct(name) {
                    self.struct_or_handle(name, idx, sl)
                } else if let Some((idx, el)) = self.find_enum(name) {
                    MirType::Enum(EnumLayoutId::new(idx, el.size, el.align))
                } else {
                    self.module_qualified_mir_type(name)
                }
            }
        }
    }


    /// A type reached through its module — `http.Response`, or `h.Response`
    /// under `import http as h`.
    ///
    /// Layouts are filed under the bare name, and the checker's own
    /// `resolve_named` has stripped the prefix like this since #915. MIR's
    /// separate type-string parser never did, so an annotation written the
    /// aliased way left the local a bare pointer and every field read off it
    /// fell back to a machine word: `h.Response.status` is a `u16` and printed
    /// through `i64_to_string`. Right only because the field sat in an 8-byte
    /// slot that zero-extends it — and wrong the moment fields hold their real
    /// widths (#1097, which is what blocks #1083).
    fn module_qualified_mir_type(&self, name: &str) -> MirType {
        let Some((_, bare)) = name.rsplit_once('.') else {
            return MirType::Ptr;
        };
        if bare.is_empty() {
            return MirType::Ptr;
        }
        if let Some((idx, sl)) = self.find_struct(bare) {
            return self.struct_or_handle(bare, idx, sl);
        }
        if let Some((idx, el)) = self.find_enum(bare) {
            return MirType::Enum(EnumLayoutId::new(idx, el.size, el.align));
        }
        MirType::Ptr
    }

    /// The registration a whole container of links needs, or `None`.
    ///
    /// A container filled entry by entry has each `push` record its edge. One
    /// that arrives whole doesn't: `h.list = h.list.filter(…)` builds a fresh
    /// vector, and no push ever ran over it. Registering the container's
    /// contents is idempotent — a record is one per (container, target) — so
    /// doing it at every such assignment costs nothing where the pushes already
    /// did the work.
    pub(crate) fn container_link_registration(&self, ty: &Type) -> Option<&'static str> {
        let head = self.generic_head(ty)?;
        let args = match ty {
            Type::UnresolvedGeneric { args, .. } | Type::Generic { args, .. } => args,
            _ => return None,
        };
        let value = args.iter().rev().find_map(|a| match a {
            rask_types::GenericArg::Type(t) => Some(t),
            _ => None,
        })?;
        if !self.is_link_type(value) {
            return None;
        }
        match head.as_str() {
            "Vec" => Some("Link_register_vec"),
            "Map" => Some("Link_register_map"),
            _ => None,
        }
    }

    /// The counterpart: what to call on the container a place *held*, when that
    /// place is about to hold a different one.
    ///
    /// A container's records name it by address and live on the targets, which
    /// have no way to hear that nobody holds the container any more. So
    /// `old.children = old.children.filter(…)` left records for the vector it
    /// replaced, and a later delete of one of those targets walked a record for
    /// storage the program had finished with (#983).
    pub(crate) fn container_link_forget(&self, ty: &Type) -> Option<&'static str> {
        match self.container_link_registration(ty)? {
            "Link_register_vec" => Some("Link_forget_vec"),
            "Link_register_map" => Some("Link_forget_map"),
            _ => None,
        }
    }

    /// Does this struct have a field that can hold an edge — a `Link<T>`, or a
    /// `Vec`/`Map` of them? Same question codegen asks to build the descriptor;
    /// asked here to decide whether the call is worth emitting at all.
    pub(crate) fn struct_carries_links(&self, ty: &MirType) -> bool {
        let MirType::Struct(sid) = ty else { return false };
        let Some(layout) = self.struct_layouts.get(sid.id as usize) else { return false };
        layout.fields.iter().any(|f| Self::field_holds_links(&f.ty))
    }

    /// `Link<T>` / `Link<T>?`, or a `Vec`/`Map` whose values are links.
    pub(crate) fn field_holds_links(ty: &Type) -> bool {
        let bare = ty.as_option().unwrap_or(ty);
        let Type::UnresolvedGeneric { name, args } = bare else { return false };
        if name == "Link" {
            return true;
        }
        if name != "Vec" && name != "Map" {
            return false;
        }
        args.iter().rev().find_map(|a| match a {
            rask_types::GenericArg::Type(t) => Some(t),
            _ => None,
        })
        .is_some_and(|v| matches!(v.as_option().unwrap_or(v),
            Type::UnresolvedGeneric { name, .. } if name == "Link"))
    }

    /// Is this the `Link<T>` type, however the checker happens to spell it?
    ///
    /// A resolved `Generic` names its base by the declaration's own spelling —
    /// `"Link<T>"`, parameter and all — so the comparison is on the head, not
    /// the whole string.
    fn is_link_type(&self, ty: &Type) -> bool {
        self.generic_head(ty).as_deref() == Some("Link")
    }

    /// The head of a generic type's name: `Link<T>` and `Link` both give `Link`.
    fn generic_head(&self, ty: &Type) -> Option<String> {
        let name = match ty {
            Type::UnresolvedGeneric { name, .. } => name.clone(),
            Type::Generic { base, .. } => self.type_names.get(base)?.clone(),
            _ => return None,
        };
        Some(name.as_str().to_string())
    }

    /// `Link<T>` — the node's address, carrying `T`'s layout so a field access
    /// through it is the same base+offset projection an aggregate local gets.
    /// Falls back to a bare pointer when `T` has no layout to attach (a generic
    /// body, or a node type nothing instantiated).
    fn link_mir_type(&self, arg: Option<&rask_types::GenericArg>) -> MirType {
        let node = match arg {
            Some(rask_types::GenericArg::Type(t)) => self.type_to_mir(t),
            _ => return MirType::Ptr,
        };
        match node {
            MirType::Struct(sid) => MirType::Link(sid),
            _ => MirType::Ptr,
        }
    }

    /// The word that means `none` for a niche option, whichever way the type
    /// is spelled.
    ///
    /// `Handle<T>?` reaches lowering two ways: still by name, and as a resolved
    /// `Generic` naming its base by `TypeId`. Only the by-name form used to
    /// count, so a `Vec<Handle<T>?>` was sized as a tagged option — 16 bytes
    /// for an 8-byte element — while the push wrote the niche (#959).
    pub(crate) fn niche_option_sentinel(&self, ty: &Type) -> Option<i64> {
        let inner = ty.as_option()?;
        let head = match inner {
            Type::UnresolvedGeneric { name, .. } => name.clone(),
            Type::Generic { base, .. } => self.type_names.get(base)?.clone(),
            _ => return None,
        };
        niche_sentinel_for_head(&head)
    }

    /// The MIR type of a niche option, keeping *which* niche it is.
    ///
    /// `Handle<T>?` and `Link<T>?` are the same shape with different `none`
    /// words. Collapsing both to `Option(Handle)` — which is what a struct
    /// field's type used to do — put the handle's all-ones `none` into a link
    /// field, and the rack then tried to register an edge from address -1.
    pub(crate) fn niche_option_mir_type(&self, ty: &Type) -> Option<MirType> {
        let inner = ty.as_option()?;
        let (name, args) = match inner {
            Type::UnresolvedGeneric { name, args } => (name.clone(), args),
            Type::Generic { base, args } => (self.type_names.get(base)?.clone(), args),
            _ => return None,
        };
        let payload = match name.as_str().trim() {
            "Link" => self.link_mir_type(args.first()),
            _ => return None,
        };
        Some(MirType::Option(Box::new(payload)))
    }

    /// Every layout's field types as MIR types, for codegen's walks over a
    /// struct's or a variant's fields.
    ///
    /// An instance layout's type-parameter field carries the substituted type,
    /// so it converts like any other field. A shared layout's still names the
    /// parameter, which resolves to no layout and comes out a bare word that
    /// owns nothing.
    pub fn field_types(&self) -> crate::FieldTypes {
        let field = |f: &rask_check::layout::FieldLayout| self.type_to_mir(&f.ty);
        crate::FieldTypes {
            structs: self
                .struct_layouts
                .iter()
                .map(|l| l.fields.iter().map(field).collect())
                .collect(),
            variants: self
                .enum_layouts
                .iter()
                .map(|l| l.variants.iter().map(|v| v.fields.iter().map(field).collect()).collect())
                .collect(),
        }
    }

    /// The MIR type of a checked type.
    ///
    /// A container is `Container(kind)` wherever it sits, so what frees it is
    /// in the type itself; everything else is the layout `type_layout` finds.
    pub fn type_to_mir(&self, ty: &Type) -> MirType {
        let mir = self.type_layout(ty);
        if mir != MirType::Ptr {
            return mir;
        }
        match crate::ContainerKind::of(ty, self.type_names) {
            Some(kind) => MirType::Container(kind),
            None => mir,
        }
    }

    fn type_layout(&self, ty: &Type) -> MirType {
        match ty {
            Type::Unit | Type::None => MirType::Void,
            Type::Bool => MirType::Bool,
            Type::I8 => MirType::I8,
            Type::I16 => MirType::I16,
            Type::I32 => MirType::I32,
            Type::I64 => MirType::I64,
            Type::I128 => MirType::I128,
            Type::U8 => MirType::U8,
            Type::U16 => MirType::U16,
            Type::U32 => MirType::U32,
            Type::U64 => MirType::U64,
            Type::U128 => MirType::U128,
            Type::F32 => MirType::F32,
            Type::F64 => MirType::F64,
            Type::Char => MirType::Char,
            Type::String | Type::Bytes => MirType::Buffer,
            Type::Never => MirType::Void,
            Type::InterfaceObject { interface_name, decl } => MirType::InterfaceObject {
                interface_name: self.type_defs.interface_symbol(interface_name, *decl),
            },
            // Named types — look up in struct/enum layouts by name
            Type::UnresolvedNamed(name) => self.resolve_type_name(name),
            // Handle<T> → packed i64 handle
            // Link<T> → the node's address; Rack<T> → the rack's. The checker
            // hands these over as `Generic` once the name resolves and as
            // `UnresolvedGeneric` before that, so both spellings land here.
            Type::UnresolvedGeneric { name, args } if name == "Link" => {
                self.link_mir_type(args.first())
            }
            Type::UnresolvedGeneric { name, .. } if name == "Rack" => MirType::Ptr,
            // `Heap<T>` structurally, not through the rendered name: an inner
            // type the checker has resolved is a `Named(TypeId)`, which renders
            // as an id and leaves `resolve_type_str` nothing to look up. A
            // `Shared<Heap<Pt>, Local>` came out as `heap<ptr>` and the read
            // through it segfaulted.
            Type::UnresolvedGeneric { name, args } if name == "Heap" => {
                let payload = match args.first() {
                    Some(rask_types::GenericArg::Type(inner)) => self.type_to_mir(inner),
                    _ => MirType::Ptr,
                };
                MirType::Heap(Box::new(payload))
            }
            Type::Generic { args, .. } if self.is_link_type(ty) => {
                self.link_mir_type(args.first())
            }
            Type::Generic { .. } if self.generic_head(ty).as_deref() == Some("Rack") => {
                MirType::Ptr
            }
            // Resolved named types — look up via type_names, then struct/enum layouts
            Type::Named(id) => {
                if let Some(name) = self.type_names.get(id) {
                    self.resolve_type_name(name)
                } else {
                    MirType::Ptr
                }
            }
            Type::Generic { base, args } => {
                // A generic newtype is whatever it wraps at these arguments:
                // `Saturating<u8>` is a `u8`. Read by name, it was the bare
                // `T`, a pointer, and the arithmetic ran a word wide.
                if let Some(wrapped) = self.newtype_at(self.type_defs.get(*base), args) {
                    return wrapped;
                }
                if let Some(instance) = self.generic_instance_mir_type(base, args) {
                    return instance;
                }
                if let Some(name) = self.type_names.get(base) {
                    self.resolve_type_name(name)
                } else {
                    MirType::Ptr
                }
            }
            Type::UnresolvedGeneric { name, args } => self.resolve_unresolved_generic(name, args),
            Type::RawPtr(_) => MirType::Ptr,
            // A closure is a pointer to its block, and saying so in the type is
            // what lets a carrier holding one give it back. `MirType::Ptr` is
            // every other pointer as well, so a tuple element or an optional's
            // payload spelled that way carried no way to tell a closure from an
            // address — and nothing freed it (#1253). It is also what makes
            // `func(…) -> …?` a niche: a present closure is never null.
            Type::Fn { .. } => MirType::FuncPtr(crate::types::SignatureId(0)),
            // Tuple → struct-like layout with positional fields. A container
            // part keeps its kind, as a wrapper's payload does: the release
            // walk has no other way to tell `(Vec<i64>, i64)`'s vector from a
            // plain word (#1395).
            Type::Tuple(fields) => {
                MirType::Tuple(fields.iter().map(|t| self.type_to_mir(t)).collect())
            }
            // Array → real array with element type and length. A container
            // element keeps its kind, the same as a tuple part (#1403).
            Type::Array { elem, len } => MirType::Array {
                elem: Box::new(self.type_to_mir(elem)),
                len: *len as u32,
            },
            // Option (T or none): niche-optimized handle, or a tagged union.
            //
            // A handle keeps the collapsed spelling — `type_to_mir` gives bare
            // `Handle` where `resolve_type_str` gives `Option(Handle)`, and
            // everything downstream accepts either. A *link* does not: it used
            // to collapse the same way, and then `Vec<Link<T>?>.get()` collapsed
            // twice and lost the outer option's tag (#959). `Option(Link)` is a
            // niche by `MirType::is_niche_payload`, which is what the checks
            // ask, so the spelling costs nothing.
            Type::Result { ok: inner, err } if **err == Type::None => {
                MirType::Option(Box::new(self.type_to_mir(inner)))
            }
            // Result<T, E> → tagged union (tag + max(T, E) payload)
            Type::Result { ok, err } => MirType::Result {
                ok: Box::new(self.type_to_mir(ok)),
                err: Box::new(self.type_to_mir(err)),
            },
            // Union → tracks variant sizes
            Type::Union(variants) => {
                MirType::Union(variants.iter().map(|t| self.type_to_mir(t)).collect())
            }
            // SIMD vector → MirType::SimdVector
            Type::SimdVector { elem, lanes } => MirType::SimdVector {
                elem: Box::new(self.type_to_mir(elem)),
                lanes: *lanes as u32,
            },
            // Should not reach MIR lowering
            Type::Var(_) | Type::Error => MirType::Ptr,
            // AT6: a projection resolves during type checking; one reaching
            // MIR named a conformance that doesn't exist and was already
            // reported.
            Type::Assoc { .. } => MirType::Ptr,
        }
    }

    /// The MIR type for an expression node — `None` when the checker's answer
    /// can't be one.
    ///
    /// This is the only place a checker type becomes a layout, which makes it
    /// the only place that can invent a width. Two answers can't be layouts: a
    /// type still holding an inference variable nobody solved, and one still
    /// naming a type parameter the substitution missed. Both convert to a
    /// plausible 8-byte scalar — `Ptr` for a bare variable, `i64` through a
    /// stub's spelling of one — and nothing downstream can tell either from a
    /// real answer.
    ///
    /// That's why call sites around this crate grew
    /// `.filter(|t| !matches!(t, MirType::Ptr))`: each was reaching for this
    /// check and catching one half of it. The guard belongs here, where no call
    /// site can miss it, and `None` then means one thing — the checker has no
    /// answer. Absence every consumer already handles; a wrong width none of
    /// them can (#1020).
    ///
    /// `lookup_raw_type` deliberately hands both over: reading the *head* of a
    /// type is fine with an open argument. `Handle<?>` is still a
    /// `Handle`, which is how the ownership checker knows a handle was
    /// dropped.
    pub fn lookup_node_type(&self, node_id: NodeId) -> Option<MirType> {
        let found = self.node_types.get(&node_id);
        crate::fallback::record_lookup(found);
        let ty = found?;
        if crate::fallback::type_is_open(ty) || type_names_a_parameter(ty).is_some() {
            return None;
        }
        Some(self.type_to_mir(ty))
    }

    /// Look up the raw Type for an expression node (preserves generic info).
    pub fn lookup_raw_type(&self, node_id: NodeId) -> Option<&Type> {
        let found = self.node_types.get(&node_id);
        crate::fallback::record_lookup(found);
        found
    }

    /// Is a value of this checked type text: a `string`, or a nominal type
    /// over one? Its MIR type is a `Buffer` either way, and so is `bytes`, so
    /// this is the question to ask wherever text and bytes behave differently.
    pub fn is_text(&self, ty: &Type) -> bool {
        let mut ty = ty;
        // A nominal over a nominal is fine; a cycle was rejected by the checker,
        // but nothing here should spin on one.
        for _ in 0..8 {
            if matches!(ty, Type::String) {
                return true;
            }
            match ty.head_name(self.type_names).and_then(|n| self.nominal_underlying.get(n)) {
                Some(under) => ty = under,
                None => return false,
            }
        }
        false
    }

    /// `is_text` for an expression node. No checker type means no.
    pub fn node_is_text(&self, node_id: NodeId) -> bool {
        self.lookup_raw_type(node_id).is_some_and(|t| self.is_text(t))
    }

    /// Extract stdlib type prefix for method name qualification.
    ///
    /// Returns the type prefix (e.g. "Vec", "Map", "string") used to build
    /// qualified method names like "Vec_push", "Map_get", "string_len".
    /// Without qualification, bare names like "get" or "len" are ambiguous
    /// across Vec, Map and String.
    ///
    /// Type/module names are derived from stdlib stub files via
    /// `rask_stdlib::mir_metadata`. Structural types (Result, Option, Ptr)
    /// are matched directly since they're language-level, not stdlib.
    pub fn stdlib_type_prefix(ty: &Type) -> Option<&str> {
        let names = rask_stdlib::mir_metadata::stdlib_type_names();
        let modules = rask_stdlib::mir_metadata::stdlib_module_names();
        match ty {
            Type::String => Some("string"),
            Type::Bytes => Some("bytes"),
            Type::UnresolvedNamed(name) => {
                if names.contains(name.as_str()) || modules.contains(name.as_str()) {
                    Some(name.as_str())
                } else {
                    None
                }
            }
            Type::UnresolvedGeneric { name, .. } => {
                if names.contains(name.as_str()) || modules.contains(name.as_str()) {
                    Some(name.as_str())
                } else {
                    None
                }
            }
            Type::Result { err, .. } if **err == Type::None => Some("Option"),
            Type::Result { .. } => Some("Result"),
            Type::RawPtr(_) => Some("Ptr"),
            _ => None,
        }
    }

    /// Extract type prefix for method name qualification, including user types.
    ///
    /// Extends `stdlib_type_prefix` to also handle user-defined struct/enum
    /// types from extend blocks. Monomorphization produces qualified names
    /// like "Person_greet"; this ensures MIR calls match.
    /// OR6: the prefix a *conformance* method's symbol carries.
    ///
    /// Not `builtin_method_prefix`: that collapses widths, so every float
    /// receiver answers `f64` and an `f32 implements Mul<…>` body would be
    /// called under someone else's name. A conformance is filed on the type as
    /// written.
    pub fn conformance_prefix(
        ty: &Type,
        type_names: &HashMap<rask_types::TypeId, String>,
    ) -> Option<String> {
        Self::type_prefix(ty, type_names)
            .or_else(|| rask_check::primitive_spelling(ty).map(str::to_string))
    }

    pub fn type_prefix(ty: &Type, type_names: &HashMap<rask_types::TypeId, String>) -> Option<String> {
        if let Some(s) = Self::stdlib_type_prefix(ty) {
            return Some(s.to_string());
        }
        // A resolved name is a dispatch prefix if it names a type. Capitalization
        // was the whole test, which is right for user types — they must start
        // uppercase — and wrong for the two stdlib types spelled lowercase,
        // `string` and `cstring`. `string` never came through here because
        // `Type::String` is its own variant; `cstring` did, and lost its prefix.
        let names_a_type = |name: &std::string::String| {
            name.chars().next().map_or(false, |c| c.is_uppercase())
                || rask_stdlib::mir_metadata::stdlib_type_names().contains(name.as_str())
        };
        match ty {
            Type::Named(id) => type_names.get(id).filter(|n| names_a_type(n)).cloned(),
            Type::Generic { base, .. } => {
                type_names.get(base).filter(|n| names_a_type(n)).cloned()
            }
            Type::UnresolvedNamed(name)
                if name.chars().next().map_or(false, |c| c.is_uppercase()) =>
            {
                Some(name.clone())
            }
            Type::UnresolvedGeneric { name, .. }
                if name.chars().next().map_or(false, |c| c.is_uppercase()) =>
            {
                Some(name.clone())
            }
            _ => None,
        }
    }

    /// CALL6: the method-name prefix dispatch chose for the call at `node`.
    ///
    /// This is the receiver type the checker actually resolved, so it stays
    /// right where the syntactic guesses go wrong — a `self` receiver, a
    /// pool-element binding, a Handle deref, a nominal newtype, an if-let
    /// binding of a stdlib type. `None` means nothing was recorded (a call node
    /// synthesized after checking) or the receiver is a primitive, which
    /// qualifies through the MIR type instead.
    /// Record a tracking path's element type across function lowerings.
    ///
    /// A path already recorded with a *different* type is a name collision
    /// between two functions, not new information: the entry is dropped and the
    /// path blacklisted, so both functions fall back to their own tracking
    /// instead of one of them getting the other's element width.
    pub fn record_shared_elem(&self, key: String, ty: MirType) {
        if self.shared_elem_conflicts.borrow().contains(&key) {
            return;
        }
        let mut shared = self.shared_elem_types.borrow_mut();
        match shared.get(&key) {
            Some(existing) if *existing != ty => {
                shared.remove(&key);
                self.shared_elem_conflicts.borrow_mut().insert(key);
            }
            Some(_) => {}
            None => {
                shared.insert(key, ty);
            }
        }
    }

    /// XC5: the package whose `extend` block a call reaches, when more than one
    /// declares the method. The checker decided it; monomorphization emitted
    /// the body under the matching symbol.
    pub fn recorded_conformance_package(&self, node: NodeId) -> Option<String> {
        match self.call_targets.get(&node)? {
            rask_check::Callee::Method { package, .. } => package.clone(),
            rask_check::Callee::Free(_) => None,
        }
    }

    pub fn recorded_prefix(&self, node: NodeId) -> Option<String> {
        // OR1: an operator the pair resolved names its own receiver. Reading it
        // off the MIR type instead would collapse `f32` onto `f64` — the width
        // collapse the builtin prefixes want and a conformance symbol doesn't.
        if let Some(target) = self.operator_targets.get(&node) {
            return Self::conformance_prefix(&target.recv, self.type_names);
        }
        match self.call_targets.get(&node)? {
            rask_check::Callee::Method { recv, method, .. } => Self::type_prefix(recv, self.type_names)
                .or_else(|| Self::declared_width_prefix(recv, method))
                .or_else(|| builtin_method_prefix(recv).map(str::to_string)),
            rask_check::Callee::Free(_) => None,
        }
    }

    /// A primitive method a stdlib file declares at one width — `f32.to_bits`
    /// in float.rk — is filed under that width. The builtin prefixes collapse
    /// `f32` onto `f64`, which is right for a libm call and wrong here:
    /// `f32.to_bits()` is the f32's own 32 bits.
    fn declared_width_prefix(recv: &Type, method: &str) -> Option<String> {
        let name = rask_check::primitive_spelling(recv)?;
        rask_stdlib::lookup_method(name, method).map(|_| name.to_string())
    }

    /// Extract type prefix from a field type string (e.g. "Vec<string>" → "Vec").
    /// Used when resolving method calls on struct fields.
    /// A written type you can call.
    ///
    /// Function types are spelled `|T| -> R` or `func(T) -> R`, so a string test
    /// used to be enough. `Sequence<T>` is also callable but wears a nominal
    /// name — it has to, so adapters can attach to it (type.sequence/SEQ1) —
    /// and it names no arrow at all, so the string test missed it and a
    /// `func evens(src: Sequence<i32>)` lowered `src(…)` as a call to a
    /// function called "src".
    pub fn is_callable_ty_str(s: &str) -> bool {
        let s = s.trim();
        s.starts_with('|')
            || s.starts_with("func(")
            || s.starts_with("Sequence<")
            || s.starts_with("SequenceMut<")
    }

    /// The answer a callable type hands back, for a type the checker resolved.
    /// `None` if it isn't callable. A sequence answers nothing — it is driven
    /// for its yields, not for a return value.
    pub fn callable_ret_ty(
        &self,
        ty: &Type,
        type_names: &HashMap<rask_types::TypeId, String>,
    ) -> Option<MirType> {
        if let Type::Fn { ret, .. } = ty {
            return Some(self.type_to_mir(ret));
        }
        let head = Self::type_prefix(ty, type_names)?;
        match head.as_str() {
            "Sequence" | "SequenceMut" => Some(MirType::Void),
            _ => None,
        }
    }

}

/// Supplementary metadata for a local variable, keyed by variable name.
/// Consolidates type prefix, full type string, collection element type,
/// and channel element size into one struct so they stay in sync.
/// A name's local and metadata as they were before a scope rebound it.
pub(crate) struct SavedName {
    name: String,
    local: Option<(LocalId, MirType)>,
    meta: Option<LocalMeta>,
}

#[derive(Clone, Default)]
pub(crate) struct LocalMeta {
    /// Stdlib type prefix (e.g. "Random", "File", "Vec").
    /// Fallback when the type checker leaves types unresolved.
    pub type_prefix: Option<String>,
    /// The full written type of a generic one (e.g. `Shared<Database>`).
    /// Resolves generic inner types when type checker info is incomplete.
    pub full_type: Option<TypeExpr>,
    /// Collection element MirType (e.g. the T in Vec<T>).
    /// Propagates element types through for-in iteration after mono.
    pub elem_type: Option<MirType>,
    /// Channel/Shared element size in bytes.
    /// Used by Receiver_recv to allocate correctly-sized output buffers.
    pub channel_elem_size: Option<i64>,
    /// C1/C2: resource_id local for consumption cancellation.
    /// Set when an ensure registers this variable as its receiver.
    pub resource_id: Option<LocalId>,
    /// The name stands for a place this frame doesn't own: a `mutate`
    /// parameter (mem.borrowing/M-rules), or a `with` binding on a box whose
    /// payload is bound by address. Whole-value reassignment has to land in
    /// that place, so `p = expr` lowers to a Store(*p, ...) instead of
    /// Assign(p, ...).
    pub assigns_through: bool,
    /// This local holds an *address*, and the scalar at it has this type. Reads
    /// of the bare name load through it, writes store through it, and the size
    /// of the access comes from here. `None` for a normal local and for an
    /// aggregate, whose MIR value is already an address used through field
    /// access rather than a bare load.
    ///
    /// Two things produce one. A scalar (Copy) `mutate` parameter arrives by
    /// pointer so the callee's writes reach the caller's storage (#270). And a
    /// `mut` scalar that an `ensure` body reads and the function later writes
    /// gets a stack cell of its own, so the cleanup hook can capture the cell
    /// rather than a snapshot taken when the ensure was scheduled (#1011).
    pub scalar_through_ptr: Option<MirType>,
}

pub struct MirLowerer<'a> {
    builder: BlockBuilder,
    /// Variable name → (local id, type)
    locals: HashMap<String, (LocalId, MirType)>,
    /// Function name → signature (for call return types)
    func_sigs: HashMap<String, FuncSig>,
    /// Stack of enclosing loops (innermost last)
    loop_stack: Vec<LoopContext>,
    /// Layout context from monomorphization
    ctx: &'a MirContext<'a>,
    /// Synthesized closure functions produced during lowering
    synthesized_functions: Vec<MirFunction>,
    /// Counter for generating unique closure function names
    closure_counter: u32,
    /// Whether the task block just lowered boxes its result.
    ///
    /// Written by `lower_closure_expecting` and read by `lower_spawn`, right
    /// after. A one-shot handoff rather than a return value because the
    /// decision is made where the closure's entry point is built.
    spawn_result_boxed: bool,
    /// The closure being lowered is a C function pointer (CB2): compiled with
    /// no environment, entered the C way, and handed back as its address.
    c_callback: bool,
    /// Inside a closure body: the locals that hold a capture's address
    /// rather than a copy of it. A whole-value write to one replaces what
    /// the creating frame (or the environment) holds.
    pub(crate) addressed_captures: std::collections::HashSet<LocalId>,
    /// While a `for` body over a sequence is lowered: the names of the loop's
    /// variables a `return` in it moved (`MirContext::return_moves`).
    pub(crate) seq_moved_names: Option<std::collections::HashSet<String>>,
    /// Name of the function being lowered (for closure naming)
    parent_name: String,
    /// What the function's attributes say about raw-pointer checks
    /// (mem.unsafe, debug-mode safety).
    pointer_checks: PointerChecks,
    /// Variable name → supplementary metadata (type prefix, full type, elem type, channel size).
    /// Keys may exist here without a corresponding entry in `locals` (e.g. module imports).
    local_meta: HashMap<String, LocalMeta>,
    /// Names this function reassigns, anywhere in its body — an `=` target, or
    /// an argument passed `mutate`/`own`/`deleting`, or a `take <place>`.
    ///
    /// Only the ensure-hook capture decision reads this (see
    /// `scalars_frozen_after_here`). A name that is never reassigned holds the
    /// same value at panic time as when the ensure was scheduled, which is what
    /// makes snapshotting a scalar into the hook's env exact instead of stale.
    reassigned_names: std::collections::HashSet<String>,
    /// Every name that appears inside an `ensure` body in this function.
    ///
    /// Scanned up front, because whether a `mut` scalar needs a stack cell has
    /// to be decided at its binding — which lowering reaches before it reaches
    /// the `ensure` that reads it (#1011).
    ensure_read_names: std::collections::HashSet<String>,
    /// W2a/W2b: Active `with` pool bindings for re-resolution after pool mutators.
    /// Maps pool variable name → Vec of (handle_local, binding_local, pool_local).
    /// When set, `return expr` inside an inlined closure body assigns to the
    /// target local and jumps to the continuation block instead of emitting
    /// MirTerminator::Return.  Used by fold/reduce/etc. The third part is
    /// `pending_write_backs` depth when the body started. The fourth is the
    /// enclosing function's return type when the body is a `for` over a
    /// sequence: its `return` answers for that function, so the value is
    /// shaped for that function's return type before it is stored.
    inline_return_target: Option<(LocalId, BlockId, usize, Option<MirType>)>,
    /// The type a `return` inside the inlined body stored, when one fired.
    ///
    /// Doubles as "the body already stored its result and terminated". Without
    /// it, fold assigned the body's fall-off value over the accumulator the
    /// return had just written, resetting it every iteration (#462) — and an
    /// inlined predicate reported the wrong type for its result local.
    inline_return_taken: Option<MirType>,
    /// Stack of active ensure cleanup blocks (innermost last).
    /// At function exit points (return, try error, implicit return),
    /// this becomes the cleanup_chain on CleanupReturn terminators.
    ensure_stack: Vec<BlockId>,
    /// Elements copied out of a collection that go back in when the body that
    /// holds them ends, innermost last: a `for mutate` binding, a `with v[i]`
    /// binding.
    ///
    /// The normal end of the body writes each one back. Any other way out has
    /// to as well, or the body's changes are lost: `return item` from a `for
    /// mutate` handed back the new value and left the collection unchanged
    /// (#650), and `return`, `continue` or a labelled `break` out of a `with`
    /// block did the same. Every function exit drains all of these, the same
    /// way it drains `ensure_stack`; a `break` or `continue` drains the ones
    /// opened inside the loop it leaves.
    pending_write_backs: Vec<PendingWriteBack>,
    /// Collection elements lent to something that writes through them, waiting
    /// for the call to be emitted so the borrow can be released.
    ///
    /// Reading `v[i]` copies the element out of the buffer — that's the value
    /// semantics `let e = v[i]` needs. A callee that writes through a `mutate`
    /// parameter would write into that copy, so it gets a pointer to the real
    /// element instead, held across exactly the one call.
    elem_writebacks: Vec<ElemWriteback>,
    /// Qualified method names that have `take self` (consume the receiver).
    /// Used for consumption cancellation (C1/C2).
    take_self_methods: std::collections::HashSet<String>,
    /// Which parameter positions a callee declares `take`, keyed the same way
    /// `take_self_methods` is. Passing a linear value to one of them is a move
    /// (mem.linear/L5), so it cancels the `ensure` the caller scheduled — and a
    /// borrowed parameter must not, which is why this can't be read off the
    /// argument alone. Positions exclude `self`.
    take_param_positions: std::collections::HashMap<String, Vec<usize>>,
    /// Lowering an `ensure` body into its own thunk function, where a consuming
    /// call cancels nothing and the resource slot isn't in this frame.
    in_ensure_thunk: bool,
    /// Methods that write through `self` — declared `mutate self`/`take self`,
    /// or private ones that assign into self (GC9 infers the mode there).
    pub(crate) mutate_self_methods: std::collections::HashSet<String>,
    /// For each ensure cleanup block, the receiver variable name and its
    /// resource_id local. Used for consumption cancellation (C1/C2):
    /// if the receiver was consumed before scope exit, skip the ensure.
    ensure_receivers: HashMap<BlockId, (String, LocalId)>,
    /// Module-level consts with a non-literal initializer, by name, waiting to
    /// be materialized on first reference in this function.
    ///
    /// They used to be emitted eagerly at the top of every function. That put
    /// `const config = Shared.new(Config.from_env())` inside `Config.from_env`
    /// itself, which then called itself forever — the binary died on a stack
    /// overflow before reaching main's first line (#463). Materializing at the
    /// use site keeps them out of functions that never mention them.
    pending_module_consts: HashMap<String, (Expr, Option<TypeExpr>)>,
    /// Module-level consts that own a global slot, and the type stored there.
    /// A reference loads from the slot instead of re-running the initializer.
    const_slots: HashMap<String, MirType>,
    /// Set while lowering a const's init thunk. Inside its own thunk the const
    /// is a definition, not a reference — that one place runs the initializer.
    const_init_target: Option<String>,
    /// Declared type of the struct field currently being initialised, as written.
    /// `Map.new()` needs its key/value sizes, and the checker doesn't type the
    /// nodes of a stdlib body — `Headers(entries: Map.new())` had no type on
    /// that call, so the map was built with 8-byte slots and a `string` value lost
    /// half of its 16 bytes.
    field_type_hint: Option<Type>,
    /// Element type of a Vec built by a fused `collect()`, keyed by the local
    /// holding it. The fused loop is the only place that type exists — the
    /// checker leaves `collect()`'s element an inference variable — and a binding
    /// needs it so `for v in page` knows what it is iterating.
    collected_elem_types: HashMap<LocalId, MirType>,
    /// Stack of enclosing `try { … } catch e => …` handlers (innermost last).
    /// A `try` inside one of these blocks jumps to the handler instead of
    /// returning from the function (ER18).
    catch_frames: Vec<CatchFrame>,
    /// This body is a `test` block's, lowered as a void function by
    /// `extract_tests`. A `try` here has no caller to propagate to and ends the
    /// test instead (#932). Read off the synthetic declaration's `test_body`
    /// attribute rather than guessed from the void return type — every function
    /// without a return annotation has one of those, `main` included, and
    /// guessing panicked in them with a message about test blocks.
    in_test_body: bool,
    /// ER16a: the `try` whose branch is still owed, and the chain step it
    /// attaches to. Armed by `lower_try` and discharged by `lower_expr` when it
    /// reaches that step.
    pending_try_step: Option<(NodeId, NodeId)>,
    /// Active `comptime for` loop-binding names and the field they're currently
    /// bound to (innermost last, for nesting). `field` here isn't a runtime
    /// value — `field.name`/`value.(field.name)` splice this directly instead
    /// of going through the normal local/struct-layout lookup (CT48/CT49).
    comptime_for_bindings: Vec<(String, ReflectFieldConst)>,
    /// Bindings in this body whose initializer folded to a compile-time string.
    /// `value.(name)` reads them so a field name can be given a name of its own
    /// (`let which = comptime { "y" }`) instead of being spelled inline (#930).
    comptime_strings: HashMap<String, String>,
}

/// One field's compile-time-known metadata inside an unrolled `comptime for
/// field in reflect.fields<T>()` body (CT48–CT54). Mirrors the interpreter's
/// FieldInfo shape (rask-interp/src/stdlib/reflect.rs) so native and interp
/// agree.
#[derive(Clone)]
pub(crate) struct ReflectFieldConst {
    pub(crate) name: String,
    pub(crate) type_name: String,
    pub(crate) offset: u32,
    pub(crate) size: u32,
    pub(crate) is_public: bool,
    pub(crate) serial_name: String,
    pub(crate) is_skipped: bool,
    pub(crate) has_default: bool,
    /// Raw attribute strings (`indexed(weight:3)`) — what `field.has<A>()`
    /// answers from (type.annotations/AN6).
    pub(crate) attrs: Vec<rask_ast::attr::Attr>,
}

/// Where a `try` inside a `try { … } catch e => …` block sends its error.
#[derive(Clone)]
pub(crate) struct CatchFrame {
    /// Block that binds `e` and runs the handler body.
    pub(crate) handler_block: BlockId,
    /// Slot the failing error payload is copied into before the jump.
    pub(crate) err_val: LocalId,
    /// Type of that slot — the enclosing function's error type.
    pub(crate) err_ty: MirType,
    /// Slot carrying the failing Result's origin line (ER15).
    pub(crate) origin_line: LocalId,
}

impl<'a> MirLowerer<'a> {
    /// Lower a monomorphized function declaration to MIR.
    ///
    /// Get the metadata entry for a variable, creating a default if absent.
    pub(crate) fn meta_mut(&mut self, name: &str) -> &mut LocalMeta {
        self.local_meta.entry(name.to_string()).or_default()
    }

    /// What `names` stand for now, to put back once a scope that rebinds them
    /// ends. Lowering keeps one flat name table, so without this a pattern's
    /// binding outlived its branch: CF12's `if d is Del { … }` rebinds `d` to
    /// the payload, and after the `if`, `d` still named the payload.
    pub(crate) fn save_names(&self, names: Vec<&str>) -> Vec<SavedName> {
        names
            .into_iter()
            .map(|n| SavedName {
                name: n.to_string(),
                local: self.locals.get(n).cloned(),
                meta: self.local_meta.get(n).cloned(),
            })
            .collect()
    }

    pub(crate) fn restore_names(&mut self, saved: Vec<SavedName>) {
        for s in saved {
            match s.local {
                Some(l) => {
                    self.locals.insert(s.name.clone(), l);
                }
                None => {
                    self.locals.remove(&s.name);
                }
            }
            match s.meta {
                Some(m) => {
                    self.local_meta.insert(s.name, m);
                }
                None => {
                    self.local_meta.remove(&s.name);
                }
            }
        }
    }

    /// Get the metadata entry for a variable (read-only).
    pub(crate) fn meta(&self, name: &str) -> Option<&LocalMeta> {
        self.local_meta.get(name)
    }

    /// True when this name refers to a value here — a local, a module-level
    /// const (materialised or still pending), or a comptime global. Used to
    /// stop a capitalised value name from being read as a type.
    pub(crate) fn name_holds_a_value(&self, name: &str) -> bool {
        self.locals.contains_key(name)
            || self.const_slots.contains_key(name)
            || self.pending_module_consts.contains_key(name)
            || self.ctx.folded.consts.contains_key(name)
    }

    /// Record a module-level const's box type from its initializer, e.g.
    /// `Shared.new(Metrics { … })` → prefix `Shared`, full type `Shared<Metrics>`.
    ///
    /// A cross-module const reference gets left an inference var by the checker,
    /// so guard access can't read the inner type off the use site — the
    /// initializer is the only place it's concrete.
    pub(crate) fn record_module_const_meta(&mut self, name: &str, init: &Expr) {
        let ExprKind::MethodCall { object, args, .. } = &init.kind else { return };
        let Some(type_name) = object.name() else { return };
        let Some(prefix) = type_prefix_from_str(type_name) else { return };
        self.meta_mut(name).type_prefix = Some(prefix.clone());
        if let Some(inner) = args.first().and_then(|a| {
            self.ctx.lookup_raw_type(a.expr.id)
                .and_then(|t| MirContext::type_prefix(t, self.ctx.type_names))
        }) {
            self.meta_mut(name).full_type =
                Some(TypeExpr::generic(prefix, vec![TypeExpr::named(inner)]));
        }
    }

    /// Bring a module-level const into scope the first time it's named in this
    /// function. Returns the local, or `None` if `name` isn't one.
    ///
    /// Consts with a global slot load the one value that the init thunk stored
    /// before main; the rest still re-run their initializer here (see
    /// `pending_module_consts`).
    ///
    /// Idempotent: once emitted the const lives in `locals` like any other
    /// binding, so later references in the same function reuse it.
    pub(crate) fn materialize_module_const(
        &mut self,
        name: &str,
    ) -> Result<Option<(LocalId, MirType)>, LoweringError> {
        if let Some((local_id, ty)) = self.locals.get(name) {
            return Ok(Some((*local_id, ty.clone())));
        }
        if self.const_init_target.as_deref() != Some(name) {
            if let Some(ty) = self.const_slots.get(name).cloned() {
                return Ok(Some(self.load_const_slot(name, ty)));
            }
        }
        let Some((init, _decl_ty)) = self.pending_module_consts.remove(name) else {
            return Ok(None);
        };
        let (op, ty) = self.lower_expr(&init)?;
        let local_id = self.builder.alloc_local(name.to_string(), ty.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: local_id,
            rvalue: MirRValue::Use(op),
        }));
        self.locals.insert(name.to_string(), (local_id, ty.clone()));
        Ok(Some((local_id, ty)))
    }

    /// Read a module-level const out of its global slot.
    ///
    /// The slot always holds 8 bytes: the value itself for scalars, a heap
    /// pointer for aggregates. An aggregate reference copies the bytes out, so
    /// each function gets its own immutable copy — the shared thing is the box
    /// the pointer points at, which is the whole point of #470.
    fn load_const_slot(&mut self, name: &str, ty: MirType) -> (LocalId, MirType) {
        let addr = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
            dst: addr,
            name: const_slot_name(name),
        }));

        let local_id = self.builder.alloc_local(name.to_string(), ty.clone());
        if ty.passed_by_address() {
            let heap = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: heap,
                rvalue: MirRValue::Deref(MirOperand::Local(addr)),
            }));
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: local_id,
                rvalue: MirRValue::Use(MirOperand::Local(heap)),
            }));
        } else {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: local_id,
                rvalue: MirRValue::Deref(MirOperand::Local(addr)),
            }));
        }
        self.locals.insert(name.to_string(), (local_id, ty.clone()));
        (local_id, ty)
    }

    /// Fill a const's global slot at the end of its init thunk.
    fn store_const_slot(&mut self, name: &str, value: MirOperand, ty: &MirType) {
        let addr = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
            dst: addr,
            name: const_slot_name(name),
        }));

        let aggregate = ty.passed_by_address();
        let stored = if aggregate {
            // The value sits in this thunk's frame, which is gone by the time
            // anything reads the slot. Copy it to the heap and share that.
            let size = self.aggregate_size(ty);
            let heap = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(alloc_block_for(heap, size, ty));
            // One store of the whole value. Copied a word at a time through
            // `Field { field_index: 0, byte_offset }`, every word read field 0:
            // codegen picks a struct's field by index and the offset never
            // counted, so `const P: Point = Point(x: 1, y: 6)` read back `1 1`.
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: heap,
                offset: 0,
                value,
                store_size: size,
                field: None,
            }));
            MirOperand::Local(heap)
        } else {
            value
        };

        // The slot holds an aggregate's heap address, or a scalar at its own
        // width.
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr,
            offset: 0,
            value: stored,
            store_size: if aggregate { MirType::Ptr.size() } else { ty.size() },
            field: None,
        }));
    }

    /// `val` as the error side of `result_ty`, when it is that `T or E`'s `E`
    /// (or, already wrapped, its union): tag 1, no origin, the error as
    /// payload. `None` when it isn't the error side.
    ///
    /// What a function's exit does for `return e` (codegen's `exit_value`),
    /// for a `return` whose value is stored before it leaves.
    pub(super) fn wrap_err_branch(
        &mut self,
        val: &MirOperand,
        val_ty: &MirType,
        result_ty: &MirType,
    ) -> Option<MirOperand> {
        let MirType::Result { err, .. } = result_ty else { return None };
        let is_err = **err == *val_ty
            || (matches!(**err, MirType::Union(_)) && self.union_member_index(err, val_ty).is_some());
        if !is_err {
            return None;
        }
        self.build_err(val.clone(), val_ty, result_ty)
    }

    /// `val` on the error side of `result_ty`: tag 1, no origin, the error as
    /// payload — a union member wrapped into the union first. `None` when
    /// `result_ty` has no error side.
    pub(super) fn build_err(
        &mut self,
        val: MirOperand,
        val_ty: &MirType,
        result_ty: &MirType,
    ) -> Option<MirOperand> {
        let MirType::Result { err, .. } = result_ty else { return None };
        let err_ty = (**err).clone();
        let payload = if matches!(err_ty, MirType::Union(_)) && err_ty != *val_ty {
            self.wrap_operand_into_union(&val, val_ty, &err_ty)?
        } else {
            val
        };
        let slot = self.builder.alloc_temp(result_ty.clone());
        self.store_wrapper_tag(slot, result_ty, rask_mono::runtime_abi::RASK_TAG_ABSENT);
        self.store_wrapper_payload(slot, result_ty, payload, &err_ty);
        Some(MirOperand::Local(slot))
    }

    /// An aggregate's own size, from its layout.
    pub(crate) fn aggregate_size(&self, ty: &MirType) -> u32 {
        match ty {
            MirType::Struct(StructLayoutId { id, .. }) => self
                .ctx
                .struct_layouts
                .get(*id as usize)
                .map_or(ty.size(), |l| l.size),
            MirType::Enum(EnumLayoutId { id, .. }) => self
                .ctx
                .enum_layouts
                .get(*id as usize)
                .map_or(ty.size(), |l| l.size),
            other => other.size(),
        }
    }

    /// The MIR type a module-level const's slot holds, as measured by
    /// `compute_const_slot_types`. `None` means no slot — the const falls back
    /// to being re-evaluated at each reference.
    fn module_const_slot_ty(&self, name: &str) -> Option<MirType> {
        self.ctx.const_slot_types.borrow().get(name).cloned()
    }

    /// Element MIR type of a checker `Vec<T>`, in either the pre-resolve
    /// (`UnresolvedGeneric`) or resolved (`Generic`) spelling.
    /// The name and type arguments of a possibly-generic checker type.
    ///
    /// `type_names` stores the *declaration* form, so a resolved `Generic` for a
    /// Map comes back as `"Map<K, V>"`, not `"Map"`. Comparing that against a
    /// bare name never matched, which quietly disabled the resolved-`Generic`
    /// path entirely — only `UnresolvedGeneric`, which carries a bare name, ever
    /// resolved. Strip the parameters before comparing.
    pub(crate) fn generic_head<'t>(&self, ty: &'t Type) -> Option<(String, &'t Vec<rask_types::GenericArg>)> {
        let (name, args) = match ty {
            Type::UnresolvedGeneric { name, args } => (name.clone(), args),
            Type::Generic { base, args } => (self.ctx.type_names.get(base)?.clone(), args),
            _ => return None,
        };
        let bare = name.as_str().trim().to_string();
        Some((bare, args))
    }

    /// What a collection holds: the element of a `Vec`, the *value* of a
    /// `Map`. Indexing any of them yields this type.
    pub(crate) fn collection_elem_of_checker_type(&self, ty: &Type) -> Option<MirType> {
        self.collection_elem_type(ty).map(|t| self.ctx.type_to_mir(&t))
    }

    /// `collection_elem_of_checker_type`, as the checker's type. For the
    /// questions a layout can't answer — a `Buffer` element is a `string` or
    /// `bytes`, and only this says which.
    pub(crate) fn collection_elem_type(&self, ty: &Type) -> Option<Type> {
        // A fixed array holds its element type outright. Without this arm every
        // element-typed dispatch decision — `join`'s string vs integer variant,
        // `sort`'s comparator — read "not a collection" and took the integer
        // branch, so `["a", "b"].join("-")` printed the bytes as numbers (#1021).
        if let Type::Array { elem, .. } = ty {
            return Some((**elem).clone());
        }
        let (name, args) = self.generic_head(ty)?;
        // A box holding a collection is transparent here: `Shared<Map<K, V>>`
        // holds what its `Map` holds, and `self.counters.read().get(k)` asks
        // exactly that.
        if matches!(name.as_str(), "Shared" | "Heap") {
            let rask_types::GenericArg::Type(inner) = args.first()? else { return None };
            return self.collection_elem_type(inner);
        }
        let arg = match name.as_str() {
            // Iterator is here because a `for` source is often one, and it
            // carries its element type the same way. So does `Sequence<T>` —
            // it was simply never added, which is why `for (k, v) in self`
            // inside `Sequence.to_map` had no element type and fell back to a
            // word (#1046).
            "Vec" | "Sequence" | "SequenceMut" => args.first()?,
            // `m[k]` and `m.get(k)` yield V, not K.
            "Map" => args.get(1)?,
            _ => return None,
        };
        match arg {
            // An unresolved variable is not an answer — see below.
            rask_types::GenericArg::Type(inner) if matches!(**inner, Type::Var(_)) => None,
            rask_types::GenericArg::Type(inner) => Some((**inner).clone()),
            _ => None,
        }
    }

    /// The checker's element type for a collection expression: its own type,
    /// else the declared type of the field it reads.
    pub(crate) fn collection_elem_type_of_expr(&self, expr: &Expr) -> Option<Type> {
        if let Some(elem) = self.ctx.lookup_raw_type(expr.id).and_then(|t| self.collection_elem_type(t)) {
            return Some(elem);
        }
        match &expr.kind {
            ExprKind::Field { object, field } => {
                let layout = self.struct_layout_of_expr(object)?;
                let f = layout.fields.iter().find(|f| f.name == *field)?;
                self.collection_elem_type(&f.ty.clone())
            }
            _ => None,
        }
    }

    pub(crate) fn vec_elem_of_checker_type(&self, ty: &Type) -> Option<MirType> {
        let (name, args) = self.generic_head(ty)?;
        if name != "Vec" {
            return None;
        }
        match args.first()? {
            // An unresolved inference variable is not an answer. It lowers to
            // Ptr, which is indistinguishable from a genuine `Vec<SomeStruct>`,
            // so accepting it here shadowed the fallbacks that *do* know —
            // `Vec.from([1, 2, 3])` ended up with a pointer element type and
            // `|x| { return x + 1 }` compiled to pointer arithmetic (x + 8).
            rask_types::GenericArg::Type(inner) if matches!(**inner, Type::Var(_)) => None,
            rask_types::GenericArg::Type(inner) => Some(self.ctx.type_to_mir(inner)),
            _ => None,
        }
    }

    /// `Map<K, V>` → the two MIR types, when the checker settled both.
    ///
    /// An unresolved inference variable is not an answer, the same reason
    /// `vec_elem_of_checker_type` refuses one: it lowers to `Ptr`, which is
    /// indistinguishable from a real pointer-shaped type.
    pub(crate) fn map_kv_of_checker_type(&self, ty: &Type) -> Option<(MirType, MirType)> {
        let (name, args) = self.generic_head(ty)?;
        if name != "Map" || args.len() < 2 {
            return None;
        }
        let settled = |i: usize| match args.get(i) {
            Some(rask_types::GenericArg::Type(inner)) if !matches!(**inner, Type::Var(_)) => {
                Some(self.ctx.type_to_mir(inner))
            }
            _ => None,
        };
        Some((settled(0)?, settled(1)?))
    }

    /// The struct layout of whatever an expression evaluates to, when it's a
    /// struct. Walks field chains (`a.b.c`) so a nested field's declared type
    /// stays reachable.
    pub(crate) fn struct_layout_of_expr(&self, expr: &Expr) -> Option<rask_check::layout::StructLayout> {
        let from_checker = self.ctx.lookup_raw_type(expr.id).and_then(|ty| match ty {
            Type::UnresolvedNamed(n) => self.ctx.find_struct(n).map(|(_, l)| l.clone()),
            Type::Named(id) => self
                .ctx
                .type_names
                .get(id)
                .and_then(|n| self.ctx.find_struct(n).map(|(_, l)| l.clone())),
            _ => None,
        });
        if from_checker.is_some() {
            return from_checker;
        }
        match &expr.kind {
            ExprKind::Ident(name) => match self.locals.get(name).map(|(_, t)| t.clone()) {
                Some(MirType::Struct(crate::types::StructLayoutId { id, .. })) => {
                    self.ctx.struct_layouts.get(id as usize).cloned()
                }
                _ => None,
            },
            ExprKind::Field { object, field } => {
                let base = self.struct_layout_of_expr(object)?;
                let f = base.fields.iter().find(|f| f.name == *field)?;
                match &f.ty {
                    Type::UnresolvedNamed(n) => self.ctx.find_struct(n).map(|(_, l)| l.clone()),
                    Type::Named(id) => self
                        .ctx
                        .type_names
                        .get(id)
                        .and_then(|n| self.ctx.find_struct(n).map(|(_, l)| l.clone())),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Base type name of an indexed object — `"Vec"` for both `rows[i]` and
    /// `self.rows[i]`. Generics are stripped: `Vec<Row>` → `Vec`.
    ///
    /// A variable's tracked prefix first (it survives cases the checker leaves as
    /// an inference variable), then the checker's type, which is the only source
    /// for a field or any other non-`Ident` object.
    pub(crate) fn index_object_base(&self, object: &Expr) -> Option<String> {
        let prefix = if let Some(var_name) = object.name() {
            self.meta(var_name).and_then(|m| m.type_prefix.clone())
        } else {
            None
        }
        .or_else(|| {
            self.ctx
                .lookup_raw_type(object.id)
                .and_then(|ty| MirContext::type_prefix(ty, self.ctx.type_names))
        })?;
        Some(prefix.as_str().trim().to_string())
    }

    /// Write every open element binding back into its collection, innermost
    /// first. For every function exit.
    pub(crate) fn emit_all_write_backs(&mut self) {
        self.emit_write_backs_above(0);
    }

    /// Write back the element bindings opened above `depth`, innermost first.
    /// For a `break` or `continue`, which leaves the bodies opened inside the
    /// loop it targets.
    pub(crate) fn emit_write_backs_above(&mut self, depth: usize) {
        let pending = self.pending_write_backs.get(depth..).unwrap_or(&[]).to_vec();
        for wb in pending.iter().rev() {
            self.emit_write_back(wb);
        }
    }

    /// LP13: a Vec element goes back by index, a Map value by key.
    ///
    /// Not `Vec_set`/`Map_set`: those replace the slot and release what it
    /// held, and here what it held is the binding being put back.
    pub(crate) fn emit_write_back(&mut self, wb: &PendingWriteBack) {
        let func = if wb.map { "Map_write_back" } else { "Vec_write_back" };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal(func.to_string()),
            args: vec![wb.collection.clone(), wb.at.clone(), MirOperand::Local(wb.value)],
        }));
    }

    /// Hand an `Ordering` back as its tag when the function is declared to
    /// return a plain integer.
    ///
    /// That combination means a comparator closure: `rask_vec_sort_by`'s C
    /// adapter reads the return as a number and tests it against zero, so
    /// returning the aggregate handed it a stack address and `sort_by` sorted
    /// by address — a different order run to run. The tag is exactly what
    /// crossed this boundary before `Ordering` had a layout (#729).
    ///
    /// Applied at the return terminator rather than at the closure's tail
    /// expression, because `|a, b| { return a.compare(b) }` reaches the same
    /// boundary through an explicit `return`.
    fn ordering_return_as_tag(&mut self, value: Option<MirOperand>) -> Option<MirOperand> {
        if *self.builder.ret_ty() != MirType::I64 {
            return value;
        }
        let Some(MirOperand::Local(id)) = value else { return value };
        let Some(ty) = self.builder.local_type(id) else { return value };
        if !self.is_ordering_ty(&ty) {
            return value;
        }
        let tag = self.emit_ordering_tag_i64(MirOperand::Local(id));
        Some(MirOperand::Local(tag))
    }

    /// The type a `return` from here hands back: the function's, which in the
    /// body of a `for` over a sequence isn't the builder's. That body is the
    /// yield closure, and its own return type is `bool`.
    pub(crate) fn return_ty(&self) -> MirType {
        self.inline_return_target
            .as_ref()
            .and_then(|t| t.3.clone())
            .unwrap_or_else(|| self.builder.ret_ty().clone())
    }

    /// Terminate the current block as a function return.
    ///
    /// One place, because there are four of them — `return`, `try`'s error path
    /// (twice), and the implicit tail — and each one has to run the same exits:
    /// pending `for mutate` writebacks, then the ensure chain.
    ///
    /// In a `for` over a sequence the value goes where the body's `return`
    /// statements put theirs, and the yield stops with the flag raised. A plain
    /// `Return` there answered the yield instead: `try` in the loop body ended
    /// the loop and the function carried on as if it had succeeded.
    pub(crate) fn terminate_return(&mut self, value: Option<MirOperand>) {
        if let Some((dst, cont, writeback_depth, Some(_))) = self.inline_return_target.clone() {
            if let Some(val) = value {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst,
                    rvalue: MirRValue::Use(val),
                }));
            }
            self.emit_write_backs_above(writeback_depth);
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: cont }));
            return;
        }
        let value = self.ordering_return_as_tag(value);
        self.emit_all_write_backs();
        if self.ensure_stack.is_empty() {
            self.builder
                .terminate(MirTerminator::dummy(MirTerminatorKind::Return { value }));
        } else {
            self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::CleanupReturn {
                value,
                cleanup_chain: self.cleanup_chain(),
            }));
        }
    }

    /// A value the fold computed, as a MIR value of the type its bytes were
    /// written in.
    ///
    /// - A string is a string literal (`FoldedForm::Text`).
    /// - A list or a map is built from its elements into a collection the use
    ///   owns.
    /// - Anything else is its bytes: a scalar is loaded, an aggregate copied
    ///   out, so changing the copy leaves the constant alone.
    pub(crate) fn load_folded(&mut self, folded: &FoldedNode) -> Result<(MirOperand, MirType), LoweringError> {
        let Some(meta) = self.ctx.folded.data.get(&folded.symbol) else {
            return Err(LoweringError::InvalidConstruct(format!(
                "folded value `{}` has no data", folded.symbol
            )));
        };
        if meta.form == FoldedForm::Text {
            let text = String::from_utf8(meta.bytes.clone()).map_err(|_| {
                LoweringError::InvalidConstruct(format!("folded string `{}` isn't UTF-8", folded.symbol))
            })?;
            return Ok((MirOperand::Constant(MirConst::String(text)), MirType::Buffer));
        }
        // An aggregate's data is named as a value of its own type, so the
        // copy below is a copy between two of them. Named as a bare address,
        // an optional or a result read it as a payload and wrapped it:
        // `by: (func(i64) -> i64)? = none` arrived as `Some(<address>)`.
        let in_place = meta.form == FoldedForm::Value && folded.ty.passed_by_address();
        let data = self.builder.alloc_temp(if in_place { folded.ty.clone() } else { MirType::Ptr });
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::GlobalRef {
            dst: data,
            name: folded.symbol.clone(),
        }));
        match meta.form {
            FoldedForm::List => {
                // The vector is told what it holds so what it hands out is
                // retained and released like one.
                let elem_desc = meta.elem.clone().unwrap_or(MirType::I64);
                let elem_size = elem_desc.size() as i64;
                let vec_local = self.builder.alloc_temp(MirType::Ptr);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: Some(vec_local),
                    func: FunctionRef::internal("rask_vec_from_static".to_string()),
                    args: vec![
                        MirOperand::Local(data),
                        MirOperand::Constant(MirConst::Int(meta.elem_count as i64)),
                        MirOperand::Constant(MirConst::Int(elem_size)),
                        crate::elem_strs::elem(elem_desc),
                    ],
                }));
                Ok((MirOperand::Local(vec_local), MirType::Ptr))
            }
            FoldedForm::Map => {
                // An empty map has no first entry to read the key and value
                // types off, and holds nothing that could tell the
                // difference, so any pair will do for one.
                let (k, v) = meta.map_types.clone().unwrap_or((MirType::I64, MirType::I64));
                self.build_comptime_map(data, meta.elem_count, k, v)
            }
            FoldedForm::Value | FoldedForm::Text => {
                let value = self.builder.alloc_temp(folded.ty.clone());
                let rvalue = if in_place {
                    // One aggregate assigned from another copies its bytes,
                    // so the use owns a copy and the data stays as it was.
                    MirRValue::Use(MirOperand::Local(data))
                } else {
                    MirRValue::Deref(MirOperand::Local(data))
                };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign { dst: value, rvalue }));
                Ok((MirOperand::Local(value), folded.ty.clone()))
            }
        }
    }

    /// The `T` in a declared `Owned<T>`, or `None` if the type isn't one.
    ///
    /// `Owned<T>` is the only box whose slot holds a pointer to a value the
    /// program otherwise treats as a plain `T` (OW5) — the rest of the family is
    /// opaque, reached through `with`. So it's the only one where a store or a
    /// read has to cross the boundary.
    pub(crate) fn owned_payload(&self, ty: &rask_types::Type) -> Option<rask_types::Type> {
        let (name, args) = self.generic_head(ty)?;
        if name != "Heap" {
            return None;
        }
        match args.first()? {
            rask_types::GenericArg::Type(inner) => Some(inner.as_ref().clone()),
            _ => None,
        }
    }

    /// When `callee` names a local holding a function value: that local, and
    /// what calling it answers. Read off the checker's type for the callee, so
    /// every way of binding a function value is covered at once.
    pub(crate) fn callable_local(&self, callee: &Expr) -> Option<(LocalId, MirType)> {
        let ExprKind::Ident(name) = &callee.kind else { return None };
        let (local, _) = self.locals.get(name)?;
        let ty = self.ctx.lookup_raw_type(callee.id)?;
        let ret = self.ctx.callable_ret_ty(ty, self.ctx.type_names)?;
        Some((*local, ret))
    }

    /// Box a value on its way into a declared `Heap<T>` slot, unless it is a
    /// block already.
    pub(crate) fn box_into_owned_slot(
        &mut self,
        val: MirOperand,
        val_ty: &MirType,
    ) -> MirOperand {
        if matches!(val_ty, MirType::Heap(_)) {
            return val;
        }
        self.box_into_owned(val, val_ty)
    }

    /// Heap-allocate a copy of `val` and hand back the block's address — what
    /// `Heap(…)` means.
    ///
    /// Every payload gets a block. Skipping the allocation for the ones that fit
    /// the slot saved a `malloc` on `Heap(42)` and cost the compiler a second
    /// shape to carry everywhere: what `*b` reads, what `drop(b)` frees and
    /// what a field holds were each answered twice, from a flag on the binding,
    /// and a flag can't cross a carrier (#1234, #1256). The allocation is also
    /// what the source asked for — `Heap(…)` is written where the cost is.
    ///
    /// The block is the payload's own size, written and read at that width:
    /// an `f32` written eight bytes wide and read four comes back 0.
    pub(crate) fn box_into_owned(&mut self, val: MirOperand, val_ty: &MirType) -> MirOperand {
        let width = val_ty.size();
        let heap = self.builder.alloc_temp(MirType::Heap(Box::new(val_ty.clone())));
        self.builder.push_stmt(alloc_block_for(heap, width, val_ty));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: heap,
            offset: 0,
            value: val,
            store_size: width,
            field: None,
        }));
        MirOperand::Local(heap)
    }

    /// An operand as a local, spilling a constant into a temp when it isn't one.
    pub(crate) fn as_local(&mut self, op: MirOperand) -> LocalId {
        match op {
            MirOperand::Local(id) => id,
            _ => {
                let tmp = self.builder.alloc_temp(MirType::I64);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: tmp,
                    rvalue: MirRValue::Use(op),
                }));
                tmp
            }
        }
    }

    /// Element type of an expression that evaluates to a `Vec<T>`, or None when
    /// it isn't one (or the type can't be recovered).
    ///
    /// The checker's type first, then the same fallbacks a binding uses: a
    /// variable's tracked element type, and the callee's declared return type.
    /// A `Vec.new()` filled by `push` leaves the checker with an inference
    /// variable, and a call returning `Vec<T>` doesn't always carry a type on the
    /// argument node.
    /// What the collection this expression evaluates to holds — element for a
    /// `Vec`, value for a `Map`.
    ///
    /// Tries the checker's type for the expression, then the declared type of a
    /// struct field, then `Vec`-specific tracking. The field case is the one that
    /// matters most: push-tracking is per-function and the checker doesn't type
    /// every node, so a collection that arrived as a field of something built
    /// elsewhere has only its declaration to go on.
    pub(crate) fn collection_elem_of_expr(&self, expr: &Expr) -> Option<MirType> {
        self.collection_elem_of_expr_inner(expr)
    }

    /// An element a call takes *out* of this collection, typed so a container
    /// element says which container it is (`MirType::Container`).
    ///
    /// `elem` is what tracking found, which is a bare `Ptr` when lowering
    /// only saw an address. The frame owns what `remove` and friends hand
    /// over, and the free it owes a `Vec<Vec<T>>`'s element is `Vec_free` —
    /// which a bare pointer can't say.
    /// The checker's type first; inside a monomorphized stdlib body it often
    /// has none, and the receiver's written type (substituted by mono) does.
    pub(crate) fn handed_over_elem(&self, collection: &Expr, elem: MirType) -> MirType {
        if elem != MirType::Ptr {
            return elem;
        }
        if let Some(ty) = self.ctx.lookup_raw_type(collection.id) {
            if let Some((name, args)) = self.generic_head(ty) {
                if matches!(name.as_str(), "Vec" | "Sequence") {
                    if let Some(rask_types::GenericArg::Type(inner)) = args.first() {
                        if !matches!(**inner, Type::Var(_)) {
                            return self.ctx.type_to_mir(inner);
                        }
                    }
                }
            }
        }
        let written = match &collection.kind {
            ExprKind::Ident(name) => self.meta(name).and_then(|m| m.full_type.as_ref()),
            // `v.take_all()` holds what `v` holds.
            ExprKind::MethodCall { object, method, .. } if method == "take_all" => {
                return self.handed_over_elem(object, elem);
            }
            _ => None,
        };
        if let Some(t) = written {
            if t.name().as_deref() == Some("Vec") {
                if let Some(arg) = t.args().first() {
                    return self.ctx.resolve_type_expr(arg);
                }
            }
        }
        elem
    }

    fn collection_elem_of_expr_inner(&self, expr: &Expr) -> Option<MirType> {
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            if let Some(elem) = self.collection_elem_of_checker_type(ty) {
                return Some(elem);
            }
        }
        if let ExprKind::Field { object, field } = &expr.kind {
            if let Some(layout) = self.struct_layout_of_expr(object) {
                if let Some(f) = layout.fields.iter().find(|f| f.name == *field) {
                    if let Some(elem) = self.collection_elem_of_checker_type(&f.ty.clone()) {
                        return Some(elem);
                    }
                }
            }
        }
        // A comptime-built global — `const SQUARES = comptime { … }` — records
        // what it holds alongside its bytes.
        if let ExprKind::Ident(name) = &expr.kind {
            if let Some(elem) = self.ctx.folded.consts.get(name)
                .and_then(|node| self.ctx.folded.nodes.get(node))
                .and_then(|f| self.ctx.folded.data.get(&f.symbol))
                .and_then(|g| g.elem.clone())
            {
                return Some(elem);
            }
        }
        // A view over a collection holds what the collection holds, so ask the
        // receiver. `for prime in PRIMES.iter()` types as `Vec<?>` — the checker
        // leaves the element a variable — while `PRIMES` itself knows.
        // `values` views a collection; `read`/`write` unwrap a box around
        // one. Either way the receiver is the thing that knows.
        if let ExprKind::MethodCall { object, method, .. } = &expr.kind {
            if matches!(method.as_str(), "values" | "into_iter" | "read" | "write") {
                if let Some(elem) = self.collection_elem_of_expr(object) {
                    return Some(elem);
                }
            }
        }
        self.vec_elem_of_expr(expr)
    }

    pub(crate) fn vec_elem_of_expr(&self, expr: &Expr) -> Option<MirType> {
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            if let Some(elem) = self.vec_elem_of_checker_type(ty) {
                return Some(elem);
            }
        }
        match &expr.kind {
            ExprKind::Ident(name) => self.meta(name).and_then(|m| m.elem_type.clone()),
            // A `Vec<T>` field: the struct's declaration is the answer, and it's
            // the only one available when the Vec was built somewhere else —
            // push tracking is per-function and the checker doesn't type every
            // field node.
            ExprKind::Field { object, field } => {
                let layout = self.struct_layout_of_expr(object)?;
                let f = layout.fields.iter().find(|f| f.name == *field)?;
                self.vec_elem_of_checker_type(&f.ty.clone())
            }
            ExprKind::Call { func, .. } => match &func.kind {
                ExprKind::Ident(callee) => {
                    let key = self.ctx.call_rewrites.get(&expr.id).cloned()
                        .unwrap_or_else(|| callee.clone());
                    self.func_sigs.get(&key).and_then(|s| s.ret_vec_elem.clone())
                }
                _ => None,
            },
            ExprKind::MethodCall { object, method, .. } => {
                let prefix = match &object.kind {
                    ExprKind::Ident(n) => self.meta(n).and_then(|m| m.type_prefix.clone()),
                    _ => None,
                }?;
                let base = prefix.as_str().trim();
                self.func_sigs
                    .get(&format!("{}_{}", base, method))
                    .and_then(|s| s.ret_vec_elem.clone())
            }
            _ => None,
        }
    }

    /// Element type of anything that can be fed to `Vec.from` — an array
    /// literal, another Vec, a slice.
    pub(crate) fn iterable_elem_of(&self, expr: &Expr) -> Option<MirType> {
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            match ty {
                // An unresolved element is no answer — it lowers to Ptr, which
                // reads as a real aggregate element and shadows the fallbacks
                // below that do know.
                Type::Array { elem, .. }
                    if !matches!(**elem, Type::Var(_)) =>
                {
                    return Some(self.ctx.type_to_mir(elem))
                }
                _ => {}
            }
        }
        // The literal's own type can be unresolved while its entries are
        // concrete: `Vec.from([1, 2, 3])` leaves the checker with `Vec<?>` but
        // the 1 is plainly an integer.
        if let ExprKind::Array(elements) = &expr.kind {
            if let Some(first) = elements.first() {
                if let Some(ty) = self.ctx.lookup_raw_type(first.id) {
                    if !matches!(ty, Type::Var(_)) {
                        return Some(self.ctx.type_to_mir(ty));
                    }
                }
            }
        }
        self.vec_elem_of_expr(expr)
    }

    /// Value type of a `Map<K, V>` receiver, for sizing what `get` hands back.
    ///
    /// The checker's type first, then the struct layout for a `self.field`
    /// receiver, then a variable's recorded `Map<K, V>` annotation. Stdlib bodies
    /// carry synthesized node IDs the checker never typed — `Headers.get` calling
    /// `self.entries.get(…)` is exactly that — so the layout path is the one that
    /// answers there.
    pub(crate) fn map_value_mir(&self, receiver: &Expr) -> Option<MirType> {
        fn value_of(ty: &Type, lower: &MirLowerer<'_>) -> Option<MirType> {
            let (name, args) = match ty {
                Type::UnresolvedGeneric { name, args } => (name.clone(), args),
                Type::Generic { base, args } => (lower.ctx.type_names.get(base)?.clone(), args),
                _ => return None,
            };
            if name != "Map" {
                return None;
            }
            match args.get(1)? {
                rask_types::GenericArg::Type(v) => Some(lower.ctx.type_to_mir(v)),
                _ => None,
            }
        }

        if let Some(ty) = self.ctx.lookup_raw_type(receiver.id) {
            if let Some(v) = value_of(ty, self) {
                return Some(v);
            }
        }

        match &receiver.kind {
            ExprKind::Field { object, field } => {
                let base_ty = match &object.kind {
                    ExprKind::Ident(name) => self.locals.get(name).map(|(_, t)| t.clone()),
                    _ => None,
                }?;
                let MirType::Struct(crate::types::StructLayoutId { id, .. }) = base_ty else {
                    return None;
                };
                let layout = self.ctx.struct_layouts.get(id as usize)?;
                let f = layout.fields.iter().find(|f| f.name == *field)?;
                value_of(&f.ty, self)
            }
            ExprKind::Ident(name) => {
                let full = self.meta(name).and_then(|m| m.full_type.clone())?;
                match (full.name().as_deref(), full.args()) {
                    (Some("Map"), [_, value]) => Some(self.ctx.resolve_type_expr(value)),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Is `name` a nominal newtype with no layout of its own?
    pub(crate) fn is_transparent_newtype(&self, name: &str) -> bool {
        self.ctx.nominal_underlying.contains_key(name)
            && self.ctx.find_struct(name).is_none()
            && self.ctx.find_enum(name).is_none()
    }

    /// Does this expression have the type of a nominal newtype?
    pub(crate) fn expr_is_transparent_newtype(&self, expr: &Expr) -> bool {
        self.ctx
            .lookup_raw_type(expr.id)
            .and_then(|ty| MirContext::type_prefix(ty, self.ctx.type_names))
            .map(|name| self.is_transparent_newtype(&name))
            .unwrap_or(false)
    }

    /// Lower a nominal newtype construction — `Id(5)` — to
    /// the wrapped value itself. Returns `None` when `name` isn't one.
    pub(crate) fn lower_newtype_wrap(
        &mut self,
        name: &str,
        inner: Option<&Expr>,
    ) -> Result<Option<TypedOperand>, LoweringError> {
        if !self.is_transparent_newtype(name) {
            return Ok(None);
        }
        let Some(inner) = inner else { return Ok(None) };
        let (op, _) = self.lower_expr(inner)?;
        Ok(Some((op, self.ctx.resolve_type_name(name))))
    }

    /// Method-dispatch prefix for a Struct/Enum MIR type — its layout name.
    /// Lets dispatch mangle `{Type}_{method}` from the concrete MIR type when
    /// the checker left the receiver untyped.
    pub(crate) fn mir_aggregate_prefix(&self, ty: &MirType) -> Option<String> {
        match ty {
            MirType::Struct(StructLayoutId { id, .. }) => self
                .ctx
                .struct_layouts
                .get(*id as usize)
                .map(|l| l.name.clone()),
            MirType::Enum(EnumLayoutId { id, .. }) => self
                .ctx
                .enum_layouts
                .get(*id as usize)
                .map(|l| l.name.clone()),
            _ => None,
        }
    }

    /// How many wrapper layers a type carries, outermost first.
    ///
    /// `i64? or E` is `[Result{err: E}, Option]` over a core of `i64`.
    fn wrapper_layers(ty: &MirType) -> (Vec<WrapLayer>, &MirType) {
        let mut layers = Vec::new();
        let mut cur = ty;
        loop {
            match cur {
                MirType::Option(inner) => {
                    layers.push(WrapLayer::Option);
                    cur = inner;
                }
                MirType::Result { ok, err } => {
                    layers.push(WrapLayer::Result {
                        err: (**err).clone(),
                    });
                    cur = ok;
                }
                _ => return (layers, cur),
            }
        }
    }

    /// Wrap `val` into the layers `dst_ty` has and `src_ty` doesn't.
    ///
    /// Every position that can coerce a bare value into a wrapper goes through
    /// here: `return`, a `let` with an annotation, a call argument, a struct
    /// field. Each of those used to carry its own wrap, which is why the depth
    /// they supported disagreed — `return` did one layer plus a hardcoded
    /// `T? or E`, a struct field did exactly one, and codegen's widening did
    /// Option only. Anything at depth 2 landed in whichever gap it happened to
    /// hit: `f32??` truncated its payload, `i64? or E` returned a bare integer
    /// the caller dereferenced, `string??` in a field read back as `none`
    /// (#644, #637, #376, #383).
    ///
    /// Layer count is what's compared, never the payload's own type — an `i32`
    /// literal going into an `i64` payload is still a value that needs wrapping,
    /// and requiring the types to match exactly is what made `return 5` from an
    /// `i64? or E` skip the wrap entirely.
    fn coerce_into_wrapper(
        &mut self,
        site: rask_ast::coercion::CoercionSite,
        val: MirOperand,
        src_ty: &MirType,
        dst_ty: &MirType,
    ) -> MirOperand {
        // A `return` that goes to the error side never reaches here: the
        // checker recorded it (`err_returns`) and the return builds it. What
        // a catch arm hands back has no such record yet, so it still picks
        // the side by type.
        let err_branch_by_type = site == rask_ast::coercion::CoercionSite::CatchArm;

        let (dst_layers, _) = Self::wrapper_layers(dst_ty);
        let (src_layers, _) = Self::wrapper_layers(src_ty);
        if dst_layers.len() <= src_layers.len() {
            return val;
        }

        // A niche option — `Handle<T>?` or `Link<T>?` — is one word: the value
        // itself, with a reserved word for `none`. There's no tag to write, and
        // wrapping anyway built a 16-byte `Some(v)` for an 8-byte slot.
        if matches!(dst_ty, MirType::Option(inner) if inner.is_niche_payload()) {
            return val;
        }

        let n_add = dst_layers.len() - src_layers.len();

        // An error value at a Result layer is the err side, not a payload to
        // wrap as Ok. Only the outermost added layer can be the one it belongs
        // to, since the layers below it are the ok branch's own shape.
        if err_branch_by_type {
            if let Some(WrapLayer::Result { err }) = dst_layers.first() {
                if src_ty == err {
                    return val;
                }
                // A union member is the err side too, and it is never *equal*
                // to the union — `AErr` against `AErr | BErr` misses. It fell
                // through and got wrapped as Ok, so `return AErr { … }` from an
                // `i64 or (AErr | BErr)` came back to the caller as a success
                // whose value was the error's payload bytes read as an integer
                // (#1013). Not a crash, an answer, which is the worst thing
                // `T or E` can do.
                //
                // The `try` path has always done this — `wrap_union_member` in
                // lower/errors.rs — because propagation goes through a
                // different producer. One shape, two producers, one of them
                // covered.
                if matches!(err, MirType::Union(_)) {
                    if let Some(wrapped) = self.wrap_operand_into_union(&val, src_ty, err) {
                        return wrapped;
                    }
                }
            }
        }

        // Build inwards-out: the innermost added layer takes the value, each
        // layer above it takes the slot built beneath.
        let mut cur_op = val;
        let mut cur_ty = src_ty.clone();
        for depth in (0..n_add).rev() {
            // `dst_layers[depth]` is the layer being added; everything from
            // there inwards is the type of the slot it builds.
            let layer_ty = Self::peel_layers(dst_ty, depth);
            let slot = self.builder.alloc_temp(layer_ty.clone());
            // Some for an Option, Ok for a Result; a success carries no origin.
            self.store_wrapper_tag(slot, &layer_ty, rask_mono::runtime_abi::RASK_TAG_PRESENT);
            self.store_wrapper_payload(slot, &layer_ty, cur_op, &cur_ty);
            cur_op = MirOperand::Local(slot);
            cur_ty = layer_ty;
        }
        cur_op
    }

    /// `x == v` where `x` is a `T?` and `v` is a bare `T`: wrap the bare side so
    /// both are optionals, which is the comparison codegen already knows.
    ///
    /// The checker accepts the mixed form and the interpreter answers it by
    /// wrapping, the way every other position auto-wraps a bare value. MIR
    /// emitted a raw compare between a 16-byte optional slot and a scalar
    /// instead, and codegen then read the scalar as an address — so
    /// `let a: bool? = true` then `if a == true` failed Cranelift's verifier
    /// (`load.i8 v10 ; v10 = 1`, loading from the address 1), and the `i64?`
    /// spelling segfaulted at runtime while the interpreter printed the answer
    /// (#834).
    ///
    /// `x == none` is untouched: `none`'s type isn't the payload's, so neither
    /// side matches.
    pub(crate) fn align_optional_compare(
        &mut self,
        left: MirOperand,
        left_ty: &MirType,
        right: MirOperand,
        right_ty: &MirType,
    ) -> (MirOperand, MirOperand) {
        let payload_of = |ty: &MirType| match ty {
            MirType::Option(inner) => Some((**inner).clone()),
            _ => None,
        };
        // An equality operand is an argument position — `a == v` is `a.eq(v)`
        // after desugaring, and `v` is the argument.
        let site = rask_ast::coercion::CoercionSite::Argument;
        if let Some(inner) = payload_of(left_ty) {
            if &inner == right_ty {
                let wrapped = self.coerce_into_wrapper(site, right, right_ty, left_ty);
                return (left, wrapped);
            }
        }
        if let Some(inner) = payload_of(right_ty) {
            if &inner == left_ty {
                let wrapped = self.coerce_into_wrapper(site, left, left_ty, right_ty);
                return (wrapped, right);
            }
        }
        (left, right)
    }

    /// Write a `T?`'s, a `T or E`'s or a union's tag (a union's member index),
    /// one byte at the front. A `T or E`'s origin is zeroed with it: a success,
    /// and an error made here rather than propagated, come from nowhere.
    pub(crate) fn store_wrapper_tag(&mut self, slot: LocalId, wrapper_ty: &MirType, tag: i64) {
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: rask_types::abi::TAG_OFFSET,
            value: MirOperand::Constant(MirConst::Int(tag)),
            store_size: rask_types::abi::TAG_BYTES,
            field: None,
        }));
        if matches!(wrapper_ty, MirType::Result { .. }) {
            self.zero_result_origin(slot);
        }
    }

    /// A scalar operand converted to `slot`'s type when the two differ in
    /// width or register class, for a store into a slot of that type. A
    /// narrower value stored as is leaves the bytes above it holding whatever
    /// was there.
    ///
    /// Only a real change converts. A handle typed `i64` going into a `ptr`
    /// slot is the same word, and copying it through a temporary hid the move
    /// from the ownership pass, which then dropped the handle on the way out:
    /// `os.signals` handed back a receiver it had already freed.
    pub(crate) fn to_slot_width(&mut self, value: MirOperand, slot: &MirType) -> MirOperand {
        let have = match &value {
            MirOperand::Local(id) => self.builder.local_type(*id),
            _ => None,
        };
        match have {
            Some(have)
                if !have.passed_by_address()
                    && !slot.passed_by_address()
                    && have.size() > 0
                    && slot.size() > 0
                    && (have.size() != slot.size() || have.is_float() != slot.is_float()) =>
            {
                let converted = self.builder.alloc_temp(slot.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: converted,
                    rvalue: MirRValue::Cast { value, target_ty: slot.clone() },
                }));
                MirOperand::Local(converted)
            }
            _ => value,
        }
    }

    /// A scalar going into a runtime container's slot, as a local of the
    /// slot's own type: the runtime copies the slot's width out of it, so the
    /// value has to be that wide and in that register class. A constant has
    /// no type of its own until it lands in one. Anything passed by address
    /// already is its storage.
    pub(crate) fn as_slot_value(&mut self, value: MirOperand, slot: &MirType) -> MirOperand {
        if slot.passed_by_address() || slot.size() == 0 {
            return value;
        }
        match value {
            MirOperand::Constant(c) => {
                let typed = self.builder.alloc_temp(slot.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: typed,
                    rvalue: MirRValue::Use(MirOperand::Constant(c)),
                }));
                MirOperand::Local(typed)
            }
            local => self.to_slot_width(local, slot),
        }
    }

    /// The arguments `Map_entries` takes for a `Map<key, val>`: the map, then
    /// where a value sits in an entry and how big an entry is. An entry is a
    /// `(key, value)` tuple, read by the same code that reads any tuple, so
    /// its layout comes from `tuple_offsets` rather than the runtime's guess:
    /// it used to put the value at the next eight-byte boundary, and a
    /// `Map<i32, i32>` read each value four bytes early.
    pub(crate) fn map_entries_args(&self, map: MirOperand, key: &MirType, val: &MirType) -> Vec<MirOperand> {
        let fields = [key.clone(), val.clone()];
        let value_offset = crate::types::tuple_offsets(&fields)[1];
        let entry_size = MirType::Tuple(fields.to_vec()).size();
        vec![
            map,
            MirOperand::Constant(MirConst::Int(value_offset as i64)),
            MirOperand::Constant(MirConst::Int(entry_size as i64)),
        ]
    }

    /// Fill `slot`, a local holding a `T?`, with a present `value`. A niche
    /// option is its payload, so the value is the whole of it.
    pub(crate) fn fill_present(&mut self, slot: LocalId, value: MirOperand) {
        let ty = self.builder.local_type(slot).expect("an option slot has a type");
        let MirType::Option(inner) = &ty else {
            panic!("fill_present on a {ty:?}, not an option");
        };
        if ty.wrapper_layout().is_none() {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: slot,
                rvalue: MirRValue::Use(value),
            }));
            return;
        }
        let inner = (**inner).clone();
        self.store_wrapper_tag(slot, &ty, rask_mono::runtime_abi::RASK_TAG_PRESENT);
        self.store_wrapper_payload(slot, &ty, value, &inner);
    }

    /// Fill `slot`, a local holding a `T?`, with `none`.
    pub(crate) fn fill_absent(&mut self, slot: LocalId) {
        let ty = self.builder.local_type(slot).expect("an option slot has a type");
        if ty.wrapper_layout().is_some() {
            self.store_wrapper_tag(slot, &ty, rask_mono::runtime_abi::RASK_TAG_ABSENT);
            return;
        }
        let sentinel = ty.niche_none().expect("an option with no layout is a niche");
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: slot,
            rvalue: MirRValue::Use(MirOperand::Constant(MirConst::Int(sentinel))),
        }));
    }

    /// Clear a `T or E`'s origin: no source line, no file.
    pub(crate) fn zero_result_origin(&mut self, slot: LocalId) {
        for (offset, size) in [
            (rask_types::abi::RESULT_ORIGIN_LINE_OFFSET, rask_types::abi::RESULT_ORIGIN_LINE_BYTES),
            (rask_types::abi::RESULT_ORIGIN_FILE_OFFSET, 8),
        ] {
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: slot,
                offset,
                value: MirOperand::Constant(MirConst::Int(0)),
                store_size: size,
                field: None,
            }));
        }
    }

    /// Write a wrapper's payload at its offset, as the side it fills: a scalar
    /// converted to that side's type, an aggregate copied whole. `value_ty` is
    /// the value's own type, which says which side of a `T or E` it is.
    pub(crate) fn store_wrapper_payload(
        &mut self,
        slot: LocalId,
        wrapper_ty: &MirType,
        value: MirOperand,
        value_ty: &MirType,
    ) {
        let arm = match wrapper_ty {
            MirType::Option(inner) => (**inner).clone(),
            MirType::Result { ok, err } => {
                if value_ty == &**err {
                    (**err).clone()
                } else {
                    (**ok).clone()
                }
            }
            _ => value_ty.clone(),
        };
        let size = self.aggregate_size(&arm);
        if size == 0 {
            return;
        }
        // `let opt: i64? = big` with `big: u32` (CV1a): stored at its own
        // width into an 8-byte payload, it left four bytes of whatever was
        // there above the value.
        let value = self.to_slot_width(value, &arm);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: slot,
            offset: wrapper_ty.payload_offset(),
            value,
            store_size: size,
            field: None,
        }));
    }

    /// `ty` with `n` of its outermost wrapper layers removed.
    fn peel_layers(ty: &MirType, n: usize) -> MirType {
        let mut cur = ty;
        for _ in 0..n {
            match cur {
                MirType::Option(inner) => cur = inner,
                MirType::Result { ok, .. } => cur = ok,
                _ => break,
            }
        }
        cur.clone()
    }

    /// Current cleanup chain in LIFO order (last-registered ensure runs first).
    fn cleanup_chain(&self) -> Vec<BlockId> {
        self.ensure_stack.iter().rev().copied().collect()
    }

    /// Inline loop-scoped ensure cleanup at break/continue/iteration-end.
    /// Copies statements from ensures registered after `depth` in LIFO order.
    /// For simple ensures (Unreachable terminator): copies statements inline.
    /// For branching ensures (else handler): creates block copies at the exit point.
    /// C1/C2: `expr` has just been lowered. If it is the call (or aggregate)
    /// that consumes an ensure's value, mark the value consumed now, so the
    /// ensure stands down.
    ///
    /// Now, and not when the enclosing statement ends. A consuming call owns
    /// its receiver from the moment it is made, whatever it returns, and code
    /// can leave between the call and the end of the statement: `let v =
    /// h.join() catch e => { return -1 }` returns from the catch arm with the
    /// handle already freed by `join`. The mark used to be emitted after the
    /// whole statement, so that return ran `ensure h.detach()` on a freed
    /// handle, and `!`, `try` and a `match` arm did the same. Hooking every
    /// lowered expression also retires the old statement-level walk and its
    /// list of shapes to look inside, which had been one spelling behind the
    /// AST four times (#1216, #1224, #1231).
    ///
    /// An ensure body *is* the deferred consumption, so a consuming call in it
    /// cancels nothing — and the resource's slot belongs to the function that
    /// registered it, not to the thunk.
    pub(super) fn mark_consumed_by(&mut self, expr: &rask_ast::expr::Expr) {
        if self.in_ensure_thunk {
            return;
        }
        self.emit_resource_consume(expr);
    }

    /// This whole expression is being moved somewhere else, so a bare name in
    /// it is a consumption (mem.linear/L5) and cancels the `ensure` scheduled
    /// for it.
    ///
    /// Separate from the walk above because position is what decides: `return a`
    /// hands `a` away, `a.id()` does not, and both arrive as an `Ident` under
    /// some node. Only the callers that *are* a move — `return`, `break`, a
    /// binding, an assignment — ask this one.
    pub(super) fn check_resource_moved(&mut self, expr: &rask_ast::expr::Expr) {
        use rask_ast::expr::ExprKind;
        if self.in_ensure_thunk {
            return;
        }
        let mut expr = expr;
        loop {
            expr = match &expr.kind {
                ExprKind::Try { expr: inner } | ExprKind::Unwrap { expr: inner, .. } => inner,
                _ => break,
            };
        }
        if let ExprKind::Ident(name) = &expr.kind {
            if let Some(res_id) = self.meta(name).and_then(|m| m.resource_id) {
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ResourceConsume {
                    resource_id: res_id,
                }));
            }
        }
    }

    /// One expression, no recursion: is *this* the consuming call?
    fn emit_resource_consume(&mut self, expr: &rask_ast::expr::Expr) {
        use rask_ast::expr::ExprKind;
        // Storing a linear value in an aggregate moves it in — the aggregate's
        // release frees it from then on. The `ensure` that was scheduled at
        // acquisition has to be cancelled by that move exactly as a `close()`
        // cancels it, or the block is freed twice. The ownership pass already
        // reads the move this way (`consume_owned_into_aggregate`); this is the
        // same walk on the lowering side.
        if matches!(
            expr.kind,
            ExprKind::StructLit { .. } | ExprKind::Tuple(_) | ExprKind::Array(_)
        ) {
            self.consume_resources_moved_into_aggregate(expr);
            return;
        }
        // `drop(p)` is a call, not a method call, so the arm below never saw it
        // and the `ensure drop(p)` it was meant to cancel ran anyway — a second
        // free of the same block, which glibc reports as a double free rather
        // than as anything to do with the two lines that caused it. Native only:
        // the interpreter tracks consumption by value.
        if let ExprKind::Call { func, args, .. } = &expr.kind {
            if matches!(&func.kind, ExprKind::Ident(n) if n == "drop") {
                if let Some(ExprKind::Ident(name)) = args.first().map(|a| &a.expr.kind) {
                    if let Some(res_id) = self.meta(name).and_then(|m| m.resource_id) {
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ResourceConsume {
                            resource_id: res_id,
                        }));
                    }
                }
            }
            if let Some(callee) = func.name() {
                self.consume_take_arguments(&callee, args);
            }
            return;
        }
        if let ExprKind::MethodCall { object, method, args, .. } = &expr.kind {
            if let Some(receiver_name) = object.name() {
                if let Some(prefix) = self.meta(receiver_name).and_then(|m| m.type_prefix.clone()) {
                    self.consume_take_arguments(&format!("{}_{}", prefix, method), args);
                }
            }
            if let Some(receiver_name) = object.name() {
                // Check if this receiver has a resource_id (registered by an ensure)
                let resource_id = self.meta(receiver_name)
                    .and_then(|m| m.resource_id);
                let prefix = self.meta(receiver_name)
                    .and_then(|m| m.type_prefix.clone());
                if let Some(res_id) = resource_id {
                    if let Some(ref prefix) = prefix {
                        let qualified = format!("{}_{}", prefix, method);
                        if self.take_self_methods.contains(&qualified) {
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ResourceConsume {
                                resource_id: res_id,
                            }));
                        }
                    }
                }
            }
        }
    }

    /// Cancel the `ensure` on any linear value this call takes ownership of.
    ///
    /// Only the positions the callee declares `take`: a parameter without it is
    /// the caller's value on loan (mem.parameters/PM1), and cancelling there
    /// would leave nothing to close it.
    fn consume_take_arguments(&mut self, callee: &str, args: &[rask_ast::expr::CallArg]) {
        use rask_ast::expr::ExprKind;
        let Some(positions) = self.take_param_positions.get(callee).cloned() else {
            return;
        };
        for i in positions {
            let Some(arg) = args.get(i) else { continue };
            if let ExprKind::Ident(name) = &arg.expr.kind {
                if let Some(res_id) = self.meta(name).and_then(|m| m.resource_id) {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ResourceConsume {
                        resource_id: res_id,
                    }));
                }
            }
        }
    }

    /// Emit `ResourceConsume` for every linear binding this aggregate takes in.
    fn consume_resources_moved_into_aggregate(&mut self, expr: &rask_ast::expr::Expr) {
        use rask_ast::expr::ExprKind;
        match &expr.kind {
            ExprKind::Ident(name) => {
                if let Some(res_id) = self.meta(name).and_then(|m| m.resource_id) {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::ResourceConsume {
                        resource_id: res_id,
                    }));
                }
            }
            ExprKind::Tuple(elems) | ExprKind::Array(elems) => {
                for e in elems {
                    self.consume_resources_moved_into_aggregate(e);
                }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields {
                    self.consume_resources_moved_into_aggregate(&f.value);
                }
                if let Some(sp) = spread {
                    self.consume_resources_moved_into_aggregate(sp);
                }
            }
            _ => {}
        }
    }

    /// Extract the receiver variable name from an ensure body.
    /// For `ensure X.method()` and `ensure drop(X)`, returns Some("X").
    ///
    /// `drop` is the whole cleanup vocabulary of `Heap<T>` — there is no
    /// `take self` method to name — so leaving the call form out meant a boxed
    /// value's ensure had no receiver, and nothing could cancel it.
    fn extract_ensure_receiver(body: &[rask_ast::stmt::Stmt]) -> Option<String> {
        use rask_ast::expr::ExprKind;
        use rask_ast::stmt::StmtKind;
        if let Some(first) = body.first() {
            if let StmtKind::Expr(expr) = &first.kind {
                match &expr.kind {
                    ExprKind::MethodCall { object, .. } => {
                        if let Some(name) = object.name() {
                            return Some(name.to_string());
                        }
                    }
                    ExprKind::Call { func, args, .. } => {
                        if matches!(&func.kind, ExprKind::Ident(n) if n == "drop") {
                            if let Some(ExprKind::Ident(name)) =
                                args.first().map(|a| &a.expr.kind)
                            {
                                return Some(name.clone());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// True for types whose MIR value is a single 8-byte pointer to the live
    /// data — an aggregate's stack address, or a heap pointer (Vec/Map/String).
    /// Capturing such a value by value is capture-by-reference: the ensure hook
    /// sees later mutations (U2). Scalars are excluded (a value copy would go
    /// stale), as are fat pointers (an interface object — 16 bytes, doesn't fit an
    /// 8-byte env slot).
    /// Collect every name this body reassigns. Walks closure and task bodies
    /// too: a closure writing an outer name reassigns it just the same.
    pub(crate) fn collect_reassigned(body: &[rask_ast::stmt::Stmt]) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        for stmt in body {
            Self::scan_reassigned_stmt(stmt, &mut out);
        }
        out
    }

    fn note_place(place: &Expr, out: &mut std::collections::HashSet<String>) {
        // The root of the place is what gets rebound. `s.f = 1` mutates through
        // `s` without changing what `s` points at, but counting it costs only
        // precision, and precision here only ever means "hook not reified".
        let mut cur = place;
        loop {
            match &cur.kind {
                ExprKind::Ident(name) => {
                    out.insert(name.clone());
                    return;
                }
                ExprKind::Field { object, .. }
                | ExprKind::OptionalField { object, .. }
                | ExprKind::Index { object, .. }
                | ExprKind::DynamicField { object, .. } => cur = object,
                _ => return,
            }
        }
    }

    /// The expressions and nested bodies directly under a statement.
    ///
    /// One enumeration, three questions asked of it: which names this body
    /// reassigns, which names its `ensure`s read, and where the `ensure`s are.
    /// Keeping them on one walker is what stops the three drifting apart as
    /// statement kinds are added.
    fn stmt_children(
        stmt: &rask_ast::stmt::Stmt,
    ) -> (Vec<&Expr>, Vec<&Vec<rask_ast::stmt::Stmt>>) {
        use rask_ast::stmt::StmtKind as SK;
        let mut kids: Vec<&Expr> = Vec::new();
        let mut bodies: Vec<&Vec<rask_ast::stmt::Stmt>> = Vec::new();
        match &stmt.kind {
            SK::Assign { target, value, .. } => {
                kids.push(target);
                kids.push(value);
            }
            SK::Let { init, .. } | SK::Mut { init, .. } => kids.push(init),
            SK::LetTuple { init, .. } | SK::MutTuple { init, .. } | SK::LetStruct { init, .. } => {
                kids.push(init)
            }
            SK::Expr(e) | SK::Return(Some(e)) => kids.push(e),
            SK::Break { value: Some(e), .. } => kids.push(e),
            SK::While { cond, body, .. } => {
                kids.push(cond);
                bodies.push(body);
            }
            SK::WhileLet { expr, body, .. } => {
                kids.push(expr);
                bodies.push(body);
            }
            SK::For { iter, body, .. } | SK::ComptimeFor { iter, body, .. } => {
                kids.push(iter);
                bodies.push(body);
            }
            SK::Loop { body, .. } | SK::Comptime(body) => bodies.push(body),
            SK::Ensure { body, else_handler } => {
                bodies.push(body);
                if let Some((_, handler)) = else_handler {
                    bodies.push(handler);
                }
            }
            _ => {}
        }
        (kids, bodies)
    }

    fn scan_reassigned_stmt(stmt: &rask_ast::stmt::Stmt, out: &mut std::collections::HashSet<String>) {
        use rask_ast::stmt::StmtKind as SK;
        // The one thing a statement says on its own: an assignment rebinds the
        // root of its target. Everything else is in the children.
        if let SK::Assign { target, .. } = &stmt.kind {
            Self::note_place(target, out);
        }
        let (kids, bodies) = Self::stmt_children(stmt);
        for k in kids {
            Self::scan_reassigned_expr(k, out);
        }
        for b in bodies {
            Self::scan_reassigned_body(b, out);
        }
    }

    /// Every name read inside an `ensure` in this body, nested scopes included.
    ///
    /// An over-approximation on purpose: it collects every identifier the ensure
    /// mentions, bound or not, because the cost of a false positive is one
    /// scalar getting a stack cell it didn't need, and the cost of a miss is an
    /// ensure that silently doesn't run on a panic.
    pub(crate) fn collect_ensure_reads(
        body: &[rask_ast::stmt::Stmt],
    ) -> std::collections::HashSet<String> {
        let mut out = std::collections::HashSet::new();
        Self::find_ensures_body(body, &mut out);
        out
    }

    fn find_ensures_body(
        body: &[rask_ast::stmt::Stmt],
        out: &mut std::collections::HashSet<String>,
    ) {
        use rask_ast::stmt::StmtKind as SK;
        for stmt in body {
            if let SK::Ensure { body, else_handler } = &stmt.kind {
                Self::note_idents_body(body, out);
                if let Some((_, handler)) = else_handler {
                    Self::note_idents_body(handler, out);
                }
            }
            // Nested scopes can hold their own ensures.
            let (kids, bodies) = Self::stmt_children(stmt);
            for k in kids {
                Self::find_ensures_expr(k, out);
            }
            for b in bodies {
                Self::find_ensures_body(b, out);
            }
        }
    }

    fn find_ensures_expr(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        let (kids, bodies) = Self::expr_children(expr);
        for k in kids {
            Self::find_ensures_expr(k, out);
        }
        for b in bodies {
            Self::find_ensures_body(b, out);
        }
    }

    fn note_idents_body(
        body: &[rask_ast::stmt::Stmt],
        out: &mut std::collections::HashSet<String>,
    ) {
        for stmt in body {
            let (kids, bodies) = Self::stmt_children(stmt);
            for k in kids {
                Self::note_idents_expr(k, out);
            }
            for b in bodies {
                Self::note_idents_body(b, out);
            }
        }
    }

    fn note_idents_expr(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        if let ExprKind::Ident(name) = &expr.kind {
            out.insert(name.clone());
        }
        let (kids, bodies) = Self::expr_children(expr);
        for k in kids {
            Self::note_idents_expr(k, out);
        }
        for b in bodies {
            Self::note_idents_body(b, out);
        }
    }

    fn scan_reassigned_body(body: &[rask_ast::stmt::Stmt], out: &mut std::collections::HashSet<String>) {
        for stmt in body {
            Self::scan_reassigned_stmt(stmt, out);
        }
    }

    /// The sub-expressions and nested bodies directly under an expression.
    fn expr_children(expr: &Expr) -> (Vec<&Expr>, Vec<&Vec<rask_ast::stmt::Stmt>>) {
        let mut kids: Vec<&Expr> = Vec::new();
        let mut bodies: Vec<&Vec<rask_ast::stmt::Stmt>> = Vec::new();
        match &expr.kind {
            // A callee that can write its argument rebinds the caller's local.
            ExprKind::Call { func, args } => {
                kids.push(func);
                for a in args {
                    kids.push(&a.expr);
                }
            }
            ExprKind::MethodCall { object, args, .. } => {
                kids.push(object);
                for a in args {
                    kids.push(&a.expr);
                }
            }
            // OPT32: `take <place>` leaves `none` behind — a write to the place.
            ExprKind::Take { place } => kids.push(place),
            ExprKind::Binary { left, right, .. } => {
                kids.push(left);
                kids.push(right);
            }
            ExprKind::Unary { operand, .. } => kids.push(operand),
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => kids.push(object),
            ExprKind::IsPresent { expr: inner, .. }
            | ExprKind::Unwrap { expr: inner, .. }
            | ExprKind::Cast { expr: inner, .. }
            | ExprKind::Convert { expr: inner, .. }
            | ExprKind::IsPattern { expr: inner, .. }
            | ExprKind::Try { expr: inner } => kids.push(inner),
            ExprKind::GuardPattern { expr: inner, else_branch, .. } => {
                kids.push(inner);
                kids.push(else_branch);
            }
            ExprKind::DynamicField { object, field_expr } => {
                kids.push(object);
                kids.push(field_expr);
            }
            ExprKind::Index { object, index } => {
                kids.push(object);
                kids.push(index);
            }
            ExprKind::NullCoalesce { value, default } => {
                kids.push(value);
                kids.push(default);
            }
            ExprKind::Catch { value, clause } => {
                kids.push(value);
                kids.push(&clause.body);
            }
            ExprKind::If { cond, then_branch, else_branch, .. } => {
                kids.push(cond);
                kids.push(then_branch);
                kids.extend(else_branch.iter().map(|b| b.as_ref()));
            }
            ExprKind::IfLet { expr: scrut, then_branch, else_branch, .. } => {
                kids.push(scrut);
                kids.push(then_branch);
                kids.extend(else_branch.iter().map(|b| b.as_ref()));
            }
            ExprKind::Match { scrutinee, arms } => {
                kids.push(scrutinee);
                kids.extend(arms.iter().map(|a| a.body.as_ref()));
            }
            ExprKind::Range { start, end, .. } => {
                kids.extend(start.iter().map(|b| b.as_ref()));
                kids.extend(end.iter().map(|b| b.as_ref()));
            }
            ExprKind::StructLit { fields, .. } => kids.extend(fields.iter().map(|f| &f.value)),
            ExprKind::Array(items) | ExprKind::Tuple(items) => kids.extend(items.iter()),
            ExprKind::ArrayRepeat { value, count } => {
                kids.push(value);
                kids.push(count);
            }
            ExprKind::WithAs { bindings, body } => {
                kids.extend(bindings.iter().map(|b| &b.source));
                bodies.push(body);
            }
            ExprKind::Block(body)
            | ExprKind::Loop { body, .. }
            | ExprKind::UsingBlock { body, .. }
            | ExprKind::Unsafe { body }
            | ExprKind::Comptime { body } => bodies.push(body),
            ExprKind::Closure { body, .. } | ExprKind::Spawn { body, .. } => kids.push(body),
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                kids.push(condition);
                kids.extend(message.iter().map(|m| m.as_ref()));
            }
            ExprKind::StringInterp(segments) => {
                use rask_ast::expr::StringSegment;
                for seg in segments {
                    if let StringSegment::Expr(e, _) = seg {
                        kids.push(e);
                    }
                }
            }
            _ => {}
        }
        (kids, bodies)
    }

    fn scan_reassigned_expr(expr: &Expr, out: &mut std::collections::HashSet<String>) {
        // What an expression says on its own: a call that can write an argument
        // rebinds the caller's local, and `take <place>` leaves `none` behind
        // (OPT32) — a write to the place.
        match &expr.kind {
            ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
                for a in args {
                    if a.mode != rask_ast::expr::ArgMode::Default {
                        Self::note_place(&a.expr, out);
                    }
                }
            }
            ExprKind::Take { place } => Self::note_place(place, out),
            _ => {}
        }
        let (kids, bodies) = Self::expr_children(expr);
        for k in kids {
            Self::scan_reassigned_expr(k, out);
        }
        for b in bodies {
            Self::scan_reassigned_body(b, out);
        }
    }

    /// Nothing in this function reassigns any of these names, so the value an
    /// ensure hook snapshots when the ensure is scheduled is the value the
    /// cleanup would read during unwind.
    ///
    /// Coarser than it could be — a write *before* the ensure is harmless, and
    /// this counts it — but the direction is safe: an unclear case keeps the
    /// ensure inline-only, which is what it was before hooks existed.
    fn scalars_frozen_after_here(&self, names: &[&String]) -> bool {
        names.iter().all(|n| !self.reassigned_names.contains(n.as_str()))
    }

    fn is_ref_capturable(ty: &MirType) -> bool {
        matches!(
            ty,
            MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Array { .. }
                | MirType::Tuple(_)
                | MirType::Ptr
                | MirType::Container(_)
                | MirType::Buffer
        )
    }

    /// Reify an ensure body as a runtime hook thunk so the cleanup runs if the
    /// scope unwinds on a native panic (ctrl.panic/U1). Returns
    /// `(thunk_name, captures)` on success; `None` keeps inline-only behavior.
    ///
    /// The body may be any statement list, with or without an `else |e|`
    /// handler — both are lowered into the thunk the same way the inline cleanup
    /// block lowers them, through `lower_ensure_else_handler`.
    ///
    /// Free variables are captured by reference where the MIR value already *is*
    /// an address (structs, enums, arrays, tuples, strings, pointers, handles),
    /// so cleanup sees mutations made before the panic. A scalar has no such
    /// address: its value lives in an SSA variable, so all the thunk could
    /// capture is a snapshot taken when the ensure was scheduled. Printing a
    /// stale number during unwind is worse than not printing, so a body that
    /// reads a scalar the function writes again returns `None` and stays
    /// inline-only. A scalar nothing writes after the ensure is scheduled is
    /// captured by value — the snapshot is the live value by definition.
    ///
    /// The optional `resource` id is captured by value so the thunk can skip a
    /// cleanup whose receiver was already consumed (C1).
    fn try_reify_ensure_hook(
        &mut self,
        body: &[rask_ast::stmt::Stmt],
        else_handler: &Option<(String, Vec<rask_ast::stmt::Stmt>)>,
        resource: Option<LocalId>,
    ) -> Option<(String, Vec<crate::stmt::ClosureCapture>)> {
        if body.is_empty() {
            return None;
        }

        // Free variables of the body plus, if there is one, the handler body
        // with its error parameter bound.
        let mut free = self.collect_free_vars_block(body);
        if let Some((param_name, handler_body)) = else_handler {
            let mut bound = std::collections::HashSet::new();
            bound.insert(param_name.clone());
            let mut seen: std::collections::HashSet<String> =
                free.iter().map(|(n, _, _, _)| n.clone()).collect();
            self.walk_free_vars_block(handler_body, &bound, &mut seen, &mut free);
        }

        // A scalar the body reads can only be snapshotted (see the doc comment).
        // Sound only while nothing writes it after the ensure is scheduled.
        let scalar_reads: Vec<&String> = free
            .iter()
            .filter(|(_, _, ty, _)| !Self::is_ref_capturable(ty))
            .map(|(name, _, _, _)| name)
            .collect();
        if !scalar_reads.is_empty() && !self.scalars_frozen_after_here(&scalar_reads) {
            return None;
        }

        // Ordered captures: free vars first, then the resource id when present.
        // Aggregates go by reference (their MIR value is the address); scalars
        // by value.
        struct Cap {
            outer: LocalId,
            name: String,
            ty: MirType,
            by_ref: bool,
            copy: bool,
        }
        let mut caps: Vec<Cap> = free
            .iter()
            .map(|(name, id, ty, copy)| Cap {
                outer: *id,
                name: name.clone(),
                ty: ty.clone(),
                by_ref: Self::is_ref_capturable(ty),
                copy: *copy,
            })
            .collect();
        let res_index = resource.map(|res| {
            caps.push(Cap {
                outer: res,
                name: "__ensure_res".to_string(),
                ty: MirType::I64,
                by_ref: false,
                copy: true,
            });
            caps.len() - 1
        });

        let thunk_name = format!("{}__ensure_thunk_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        let mut thunk_builder = BlockBuilder::new(thunk_name.clone(), MirType::Void);
        let env_param = thunk_builder.add_param("__env".to_string(), MirType::Ptr);

        let captures = crate::stmt::lay_out_captures(
            caps.iter().map(|c| (c.outer, c.ty.clone(), c.by_ref, c.copy)),
        );
        let mut thunk_locals: HashMap<String, (LocalId, MirType)> = HashMap::new();
        let mut thunk_res_local: Option<LocalId> = None;
        for (i, cap) in caps.iter().enumerate() {
            // A by-ref capture's local in the thunk is the same thing the outer
            // local is, so it gets the same type. A `mut` scalar's stack cell is
            // registered as `Ptr` by name — that is how the capture decision
            // reads it as an address — but the local itself is a one-word array,
            // and typing the thunk's copy `Ptr` made it look like a scalar's
            // address instead of an aggregate. `addr_taken` then turned each
            // read into a load through it, one dereference too many, and the
            // hook read the value as if it were a pointer (#1011).
            let cap_ty = self
                .builder
                .local_type(cap.outer)
                .filter(|t| cap.by_ref && (t.passed_by_address() || matches!(t, MirType::Ptr | MirType::Container(_))))
                .unwrap_or_else(|| cap.ty.clone());
            let dst = thunk_builder.alloc_local(cap.name.clone(), cap_ty.clone());
            thunk_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst,
                env_ptr: env_param,
                offset: captures[i].offset,
                access: if cap.by_ref {
                    crate::CaptureAccess::Borrowed
                } else {
                    crate::CaptureAccess::Value
                },
            }));
            if Some(i) == res_index {
                thunk_res_local = Some(dst);
            } else {
                // The body still resolves the name the way the outer function
                // did — through the pointer for a cell — so the registered type
                // stays `cap.ty`.
                thunk_locals.insert(cap.name.clone(), (dst, cap.ty.clone()));
            }
        }

        // Consumption cancellation (C1): skip the body if already consumed.
        if let Some(res_local) = thunk_res_local {
            let consumed = thunk_builder.alloc_temp(MirType::I64);
            thunk_builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(consumed),
                func: crate::FunctionRef::internal("rask_resource_is_consumed".to_string()),
                args: vec![MirOperand::Local(res_local)],
            }));
            let body_block = thunk_builder.create_block();
            let skip_block = thunk_builder.create_block();
            thunk_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                cond: MirOperand::Local(consumed),
                then_block: skip_block,
                else_block: body_block,
            }));
            thunk_builder.switch_to_block(skip_block);
            thunk_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: None }));
            thunk_builder.switch_to_block(body_block);
        }

        // Lower the body into the thunk (reuses method resolution). The pending
        // module consts are saved along with the locals: the thunk is its own MIR
        // function and materialises its own copy of any const it touches, but that
        // must not consume the outer function's entry — the cleanup path lowers
        // this same body again, and with the entry gone the reference compiled to
        // a call named after the const (#403).
        let saved_builder = std::mem::replace(&mut self.builder, thunk_builder);
        let saved_locals = std::mem::replace(&mut self.locals, thunk_locals);
        let saved_pending = self.pending_module_consts.clone();
        let saved_loop_stack = std::mem::take(&mut self.loop_stack);
        let saved_ensure_stack = std::mem::take(&mut self.ensure_stack);
        // The thunk's own bindings must not leave their metadata behind: a `let`
        // inside the body would otherwise mark an outer `mut` of the same name
        // immutable, and a later ensure would snapshot it.
        let saved_meta = self.local_meta.clone();
        let saved_in_thunk = std::mem::replace(&mut self.in_ensure_thunk, true);
        let body_result = (|| -> Result<(), LoweringError> {
            for s in body {
                self.lower_stmt(s)?;
            }
            if let Some((param_name, handler_body)) = else_handler {
                self.lower_ensure_else_handler(param_name, handler_body)?;
            }
            Ok(())
        })();
        self.in_ensure_thunk = saved_in_thunk;
        thunk_builder = std::mem::replace(&mut self.builder, saved_builder);
        self.locals = saved_locals;
        self.pending_module_consts = saved_pending;
        self.loop_stack = saved_loop_stack;
        self.ensure_stack = saved_ensure_stack;
        self.local_meta = saved_meta;

        if body_result.is_err() {
            return None;
        }
        if thunk_builder.current_block_unterminated() {
            thunk_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: None }));
        }

        let thunk_fn = thunk_builder.finish();
        self.func_sigs.insert(thunk_name.clone(), FuncSig { ret_ty: MirType::Void, scalar_mutate_params: Vec::new(), aggregate_mutate_params: Vec::new(), ret_vec_elem: None, param_tys: Vec::new() });
        self.synthesized_functions.push(thunk_fn);

        Some((thunk_name, captures))
    }

    /// End of a loop body: run the loop-scoped ensures (EN7) and hand control
    /// to the next iteration.
    ///
    /// A body that already terminated has nothing to hand on. `return` is the
    /// case that matters: it terminates the block and leaves it that way, so
    /// terminating again overwrote the `return` with this goto and the value
    /// was silently thrown away — `for x in xs { return x }` ran the loop out
    /// and fell through to whatever followed (#635). `break` and `continue`
    /// leave a fresh dead block behind instead, so they still land here and
    /// still get their unreachable goto, same as before.
    fn close_loop_body(&mut self, ensure_depth: usize, next: BlockId) {
        if !self.builder.current_block_unterminated() {
            return;
        }
        self.emit_loop_cleanup(ensure_depth);
        self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto {
            target: next,
        }));
    }

    fn emit_loop_cleanup(&mut self, depth: usize) {
        for i in (depth..self.ensure_stack.len()).rev() {
            let block_id = self.ensure_stack[i];
            let stmts: Vec<_> = self.builder.block_stmts(block_id).to_vec();
            // Check if this cleanup block has a sub-CFG (Branch terminator)
            let term_kind = self.builder.block_terminator_kind(block_id);
            if let Some(MirTerminatorKind::Branch { cond, then_block, else_block }) = term_kind {
                // Branching ensure (ER2 else handler): create block copies
                for stmt in stmts {
                    self.builder.push_stmt(stmt);
                }
                // Create local copies of the sub-blocks
                let then_copy = self.builder.create_block();
                let else_copy = self.builder.create_block();
                let merge = self.builder.create_block();

                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond,
                    then_block: then_copy,
                    else_block: else_copy,
                }));

                // Copy then-block (handler) statements
                self.builder.switch_to_block(then_copy);
                let then_stmts: Vec<_> = self.builder.block_stmts(then_block).to_vec();
                for stmt in then_stmts {
                    self.builder.push_stmt(stmt);
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge }));

                // Copy else-block (done) — typically empty, just continues
                self.builder.switch_to_block(else_copy);
                let else_stmts: Vec<_> = self.builder.block_stmts(else_block).to_vec();
                for stmt in else_stmts {
                    self.builder.push_stmt(stmt);
                }
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Goto { target: merge }));

                // Continue in merge block
                self.builder.switch_to_block(merge);
            } else {
                // Simple ensure: just copy statements inline
                for stmt in stmts {
                    self.builder.push_stmt(stmt);
                }
            }
        }
    }

    /// `all_decls` provides function signatures for resolving call return types.
    /// `ctx` provides struct/enum layout data for resolving field types and offsets.
    ///
    /// Returns the lowered function plus any synthesized closure functions.
    /// Lower a monomorphized function declaration to MIR.
    ///
    /// `qualified_name` overrides the function name from the AST. Needed because
    /// monomorphization produces qualified names like "Document_new" while the
    /// AST FnDecl still has bare "new".
    pub fn lower_function(
        decl: &Decl,
        all_decls: &[Decl],
        ctx: &MirContext,
    ) -> Result<Vec<MirFunction>, LoweringError> {
        Self::lower_function_named(decl, all_decls, ctx, None)
    }

    pub fn lower_function_named(
        decl: &Decl,
        all_decls: &[Decl],
        ctx: &MirContext,
        qualified_name: Option<&str>,
    ) -> Result<Vec<MirFunction>, LoweringError> {
        Self::lower_function_inner(decl, all_decls, ctx, qualified_name, None)
    }

    /// Lower the thunk that fills one module-level const's global slot.
    /// Reuses the normal function path so the initializer sees the same
    /// signature tables and const metadata every other function does.
    fn lower_const_init(
        c: &ConstDecl,
        all_decls: &[Decl],
        ctx: &MirContext,
    ) -> Result<Vec<MirFunction>, LoweringError> {
        let name = const_init_fn_name(&c.name);
        let decl = Decl {
            id: NodeId(0),
            span: Span::new(0, 0),
            kind: DeclKind::Fn(FnDecl {
                name: name.clone(),
                type_params: Vec::new(),
                params: Vec::new(),
                ret_ty: None,
                body: Vec::new(),
                is_pub: false,
                is_private: true,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: Vec::new(),
                doc: None,
                span: Span::new(0, 0),
                decl_start: 0,
            }),
        };
        Self::lower_function_inner(
            &decl,
            all_decls,
            ctx,
            Some(&name),
            Some((&c.name, &c.init)),
        )
    }

    /// Measure what each module-level const's global slot has to hold, by
    /// lowering its initializer once and taking the resulting MIR type.
    ///
    /// Must run before any function is lowered: references read the answer out
    /// of `ctx.const_slot_types`, and a const missing from that map keeps the
    /// old per-function behaviour. The thunks lowered here are thrown away —
    /// only their types are wanted, and the real ones are emitted alongside
    /// main. A const whose initializer doesn't lower in isolation simply
    /// doesn't get a slot.
    pub fn compute_const_slot_types(all_decls: &[Decl], ctx: &MirContext) {
        let mut measured = HashMap::new();
        for d in all_decls {
            let DeclKind::Const(c) = &d.kind else { continue };
            if const_init_is_literal(&c.init) || measured.contains_key(&c.name) {
                continue;
            }
            // `const N = comptime …` is already evaluated and folded into a
            // data section. Giving it a slot would turn the folded constant
            // back into a runtime load, and any const computed from it would
            // stop folding too.
            if ctx.folded.consts.contains_key(&c.name) {
                continue;
            }
            if let Ok(Some(ty)) = Self::measure_const_init_ty(c, all_decls, ctx) {
                measured.insert(c.name.clone(), ty);
            }
        }
        *ctx.const_slot_types.borrow_mut() = measured;
    }

    /// Emit the real init thunks for every const that got a slot.
    ///
    /// Normally these come out alongside `main`, which is where the calls to
    /// them are emitted too. The test and benchmark runners skip `main` — its
    /// only job there is making the test bodies reachable — so the thunks have
    /// to be produced separately, and their entry point calls them itself.
    /// Run `compute_const_slot_types` first; a const with no slot is a folded
    /// literal and needs no thunk.
    pub fn lower_const_init_thunks(
        all_decls: &[Decl],
        ctx: &MirContext,
    ) -> Result<Vec<MirFunction>, LoweringError> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for d in all_decls {
            let DeclKind::Const(c) = &d.kind else { continue };
            if !ctx.const_slot_types.borrow().contains_key(&c.name) || !seen.insert(c.name.clone()) {
                continue;
            }
            out.extend(Self::lower_const_init(c, all_decls, ctx)?);
        }
        Ok(out)
    }

    /// Lower one const initializer in isolation and report its MIR type.
    fn measure_const_init_ty(
        c: &ConstDecl,
        all_decls: &[Decl],
        ctx: &MirContext,
    ) -> Result<Option<MirType>, LoweringError> {
        let fns = Self::lower_const_init(c, all_decls, ctx)?;
        Ok(fns
            .first()
            .and_then(|f| {
                f.locals
                    .iter()
                    .find(|l| l.name.as_deref() == Some(CONST_SLOT_VALUE_LOCAL))
            })
            .map(|l| l.ty.clone()))
    }

    /// Lower one function, and refuse to hand back MIR that was built on a
    /// guessed type.
    ///
    /// Every place lowering can't resolve a type routes through
    /// `fallback::unknown_type`. Guessing i64 there is right only for a payload
    /// that already fits a machine word, so it used to pass silently for
    /// integers and quietly corrupt everything else. The record is drained
    /// around each function so a failure names the function that caused it.
    fn lower_function_inner(
        decl: &Decl,
        all_decls: &[Decl],
        ctx: &MirContext,
        qualified_name: Option<&str>,
        const_init: Option<(&str, &Expr)>,
    ) -> Result<Vec<MirFunction>, LoweringError> {
        crate::fallback::reset();
        let result = Self::lower_function_typed(decl, all_decls, ctx, qualified_name, const_init);
        let gave_up = crate::fallback::take_hits();
        // An empty node-type map means nobody ran the checker — the lowering unit
        // tests hand-build ASTs to assert block structure, so *every* node is
        // untyped there and "couldn't resolve a type" says nothing. A real
        // compile always arrives with node types.
        let synthetic = ctx.node_types.is_empty();
        if !gave_up.is_empty() && !synthetic && !crate::fallback::fallback_allowed() {
            return Err(LoweringError::UnknownType(gave_up));
        }
        result
    }

    fn lower_function_typed(
        decl: &Decl,
        all_decls: &[Decl],
        ctx: &MirContext,
        qualified_name: Option<&str>,
        const_init: Option<(&str, &Expr)>,
    ) -> Result<Vec<MirFunction>, LoweringError> {
        let fn_decl = match &decl.kind {
            DeclKind::Fn(f) => f,
            _ => {
                return Err(LoweringError::InvalidConstruct(
                    "Expected function declaration".to_string(),
                ))
            }
        };

        let ret_ty = ctx.fn_ret_ty(&fn_decl.name, fn_decl.ret_ty.as_ref());

        // Build function signature table from all declarations
        let mut func_sigs = HashMap::new();
        for d in all_decls {
            match &d.kind {
                DeclKind::Fn(f) => {
                    let sig_ret = ctx.fn_ret_ty(&f.name, f.ret_ty.as_ref());
                    func_sigs.insert(f.name.clone(), FuncSig {
                        ret_ty: sig_ret,
                        scalar_mutate_params: scalar_mutate_params(f, ctx),
                        aggregate_mutate_params: aggregate_mutate_params(&f.params, ctx),
                        ret_vec_elem: vec_elem_of_type(f.ret_ty.as_ref(), ctx),
                        param_tys: f.params.iter().map(|p| p.ty.clone()).collect(),
                    });
                }
                DeclKind::Extern(ext) => {
                    let sig_ret = ext
                        .ret_ty
                        .as_ref()
                        .map(|t| ctx.resolve_type_expr(t))
                        .unwrap_or(MirType::Void);
                    func_sigs.insert(ext.name.clone(), FuncSig { ret_ty: sig_ret, scalar_mutate_params: Vec::new(), aggregate_mutate_params: Vec::new(), ret_vec_elem: None, param_tys: Vec::new() });
                }
                DeclKind::Impl(impl_decl) => {
                    for m in &impl_decl.methods {
                        let qualified = format!("{}_{}", impl_decl.target_ty.name().unwrap_or_default(), m.name);
                        let sig_ret = m
                            .ret_ty
                            .as_ref()
                            .map(|t| ctx.resolve_type_expr(t))
                            .unwrap_or(MirType::Void);
                        func_sigs.insert(qualified, FuncSig {
                            ret_ty: sig_ret,
                            scalar_mutate_params: scalar_mutate_params(m, ctx),
                            aggregate_mutate_params: aggregate_mutate_params(&m.params, ctx),
                            ret_vec_elem: vec_elem_of_type(m.ret_ty.as_ref(), ctx),
                            param_tys: m.params.iter().map(|p| p.ty.clone()).collect(),
                        });
                    }
                }
                _ => {}
            }
        }

        // Inject signatures for stdlib methods so return types resolve.
        // Derived from stub files via rask_stdlib::mir_metadata.
        for meta in rask_stdlib::mir_metadata::method_metas() {
            func_sigs.entry(meta.qualified_name.clone()).or_insert(FuncSig {
                ret_ty: ctx.resolve_type_expr(&meta.ret_ty),
                scalar_mutate_params: Vec::new(),
                aggregate_mutate_params: Vec::new(),
                ret_vec_elem: None,
                param_tys: Vec::new(),
            });
        }

        // C1/C2: collect qualified method names with `take self` for
        // consumption cancellation. When such a method is called on an
        // ensure receiver, the ensure is cancelled.
        let mut take_self_methods = std::collections::HashSet::new();
        // The stdlib's own `take self` methods, which are declarations in
        // `stdlib/*.rk` rather than decls in this program. `Handle.join`
        // consumes its handle exactly the way a user method does, and not
        // knowing that let a registered `ensure h.detach()` run after it and
        // detach a handle that was already gone (#1216).
        let mut take_param_positions: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();
        {
            let reg = rask_stdlib::stubs::StubRegistry::load();
            for ty in reg.type_names() {
                for m in reg.methods(ty) {
                    if m.take_self {
                        take_self_methods.insert(format!("{}_{}", ty, m.name));
                    }
                    let takes: Vec<usize> = m
                        .param_modes
                        .iter()
                        .enumerate()
                        .filter(|(_, mode)| mode.is_take)
                        .map(|(i, _)| i)
                        .collect();
                    if !takes.is_empty() {
                        take_param_positions.insert(format!("{}_{}", ty, m.name), takes);
                    }
                }
            }
        }
        // Methods that write through `self`. A receiver reached through a field
        // has to be passed as that field's address for these, or the write lands
        // in a copy — see `place_address` in lower/expr.rs (#702). Declared
        // `mutate self` counts, and so does a private method that assigns into
        // self, because type.gradual/GC9 infers the mode from the body there.
        let mut mutate_self_methods = std::collections::HashSet::new();
        for d in all_decls {
            match &d.kind {
                DeclKind::Impl(impl_decl) => {
                    for m in &impl_decl.methods {
                        // Filed under the target's name, which is what a call
                        // site dispatches through: `extend Handle<T>` is
                        // `Handle_join` (#1216).
                        let qualified =
                            format!("{}_{}", impl_decl.target_ty.name().unwrap_or_default(), m.name);
                        if m.params.first().map_or(false, |p| p.name == "self" && p.is_take) {
                            take_self_methods.insert(qualified.clone());
                        }
                        let takes = take_positions(&m.params);
                        if !takes.is_empty() {
                            take_param_positions.insert(qualified.clone(), takes);
                        }
                        if method_mutates_self(m, ctx) {
                            mutate_self_methods.insert(qualified);
                        }
                    }
                }
                DeclKind::Fn(f) => {
                    // After monomorphization, impl methods become standalone functions
                    // named "Type_method" with a `take self` first parameter. An
                    // instance is `Group_join_all$i64`, and a call site asks with
                    // the receiver's type name — `Group_join_all` — so both go in,
                    // or `join_all` wasn't known to consume a `Group<i64>` and its
                    // `ensure g.detach()` ran after it.
                    let names: Vec<&str> = match f.name.split_once('$') {
                        Some((base, _)) => vec![f.name.as_str(), base],
                        None => vec![f.name.as_str()],
                    };
                    let take_self =
                        f.params.first().map_or(false, |p| p.name == "self" && p.is_take);
                    let takes = take_positions(&f.params);
                    let mutates = method_mutates_self(f, ctx);
                    for name in names {
                        if take_self {
                            take_self_methods.insert(name.to_string());
                        }
                        if !takes.is_empty() {
                            take_param_positions.insert(name.to_string(), takes.clone());
                        }
                        if mutates {
                            mutate_self_methods.insert(name.to_string());
                        }
                    }
                }
                _ => {}
            }
        }

        let func_name = qualified_name
            .map(|s| s.to_string())
            .unwrap_or_else(|| fn_decl.name.clone());

        // Const slots are filled from main, so only main emits the thunk calls.
        let is_entry_point = func_name == "main" && const_init.is_none();
        let mut const_init_thunks: Vec<ConstDecl> = Vec::new();

        let mut lowerer = MirLowerer {
            builder: BlockBuilder::new(func_name.clone(), ret_ty.clone()),
            locals: HashMap::new(),
            func_sigs,
            loop_stack: Vec::new(),
            ctx,
            synthesized_functions: Vec::new(),
            closure_counter: 0,
            spawn_result_boxed: false,
            c_callback: false,
            addressed_captures: std::collections::HashSet::new(),
            seq_moved_names: None,
            parent_name: func_name,
            pointer_checks: PointerChecks::of(&fn_decl.attrs),
            local_meta: HashMap::new(),
            reassigned_names: std::collections::HashSet::new(),
            ensure_read_names: std::collections::HashSet::new(),
            inline_return_target: None,
            inline_return_taken: None,
            ensure_stack: Vec::new(),
            pending_write_backs: Vec::new(),
            elem_writebacks: Vec::new(),
            take_self_methods,
            take_param_positions,
            in_ensure_thunk: false,
            mutate_self_methods,
            ensure_receivers: HashMap::new(),
            pending_module_consts: HashMap::new(),
            const_slots: HashMap::new(),
            const_init_target: const_init.map(|(n, _)| n.to_string()),
            collected_elem_types: HashMap::new(),
            field_type_hint: None,
            catch_frames: Vec::new(),
            in_test_body: fn_decl.attrs.iter().any(|a| a.is("test_body")),
            pending_try_step: None,
            comptime_for_bindings: Vec::new(),
            comptime_strings: HashMap::new(),
        };

        // Which names this body reassigns (see `reassigned_names`). Scanned once
        // here rather than tracked as lowering goes, because the question is
        // about statements the ensure hasn't reached yet.
        lowerer.reassigned_names = Self::collect_reassigned(&fn_decl.body);
        lowerer.ensure_read_names = Self::collect_ensure_reads(&fn_decl.body);

        // Resolve Self from the function name, for the methods that still
        // arrive with it: a generic owner's template keeps `Self` because the
        // per-receiver copy is what knows the layout. Splitting at the first
        // underscore is right there and only there — a type name with an
        // underscore in it reads as a different type, which is what made a
        // dependency's `Helper_liba_describe` look up a `Helper` (#1129). Mono
        // writes the type in wherever it knows it, so this sees the rest.
        let self_type: Option<TypeExpr> = fn_decl.params.iter()
            .any(|p| p.ty.as_ref().is_some_and(|t| t.is_name("Self")))
            .then(|| {
                // Extract the type name prefix from the qualified function name
                lowerer.parent_name.split('_').next().map(TypeExpr::named)
            })
            .flatten();

        // Add parameters
        for param in &fn_decl.params {
            let written = match &param.ty {
                Some(t) if t.is_name("Self") => self_type.as_ref().or(param.ty.as_ref()),
                other => other.as_ref(),
            };
            let param_ty = match written {
                Some(t) => ctx.resolve_type_expr(t),
                None => MirType::Void,
            };
            // #270: a scalar `mutate` param is passed by pointer so the callee can
            // write back through it. Register the param local as a pointer; reads
            // load and writes store through it, keyed by the recorded scalar type.
            let scalar_mutate = param.is_mutate
                && crate::lower::stmt::mutate_param_needs_own_pointer(&param.name, &param_ty);
            let local_ty = if scalar_mutate { MirType::Ptr } else { param_ty.clone() };
            // The parameter keeps saying which container it is, so a frame that
            // gets it back from a call knows which free matches (#1502).
            let declared_ty = match written {
                Some(t) if !scalar_mutate => ctx.resolve_type_expr(t),
                _ => local_ty.clone(),
            };
            let local_id = lowerer.builder.add_param(param.name.clone(), declared_ty);
            lowerer.locals.insert(param.name.clone(), (local_id, local_ty));
            // Set type prefix for parameters so method calls qualify correctly.
            // mir_type_name handles Struct/Enum/String/primitives; the written
            // type catches Ptr types like Vec<T>, Map<K,V>.
            {
                let prefix = lowerer.mir_type_name(&param_ty)
                    .or_else(|| written.and_then(type_prefix_of));
                let meta = lowerer.local_meta.entry(param.name.clone()).or_default();
                if param.is_mutate {
                    meta.assigns_through = true;
                }
                if scalar_mutate {
                    meta.scalar_through_ptr = Some(param_ty.clone());
                }
                if let Some(p) = prefix {
                    meta.type_prefix = Some(p);
                }
                // The full written type for generic ones (Shared<T>, Channel<T>, etc.)
                if let Some(written) = written.filter(|t| !t.args().is_empty()) {
                    meta.full_type = Some(written.clone());
                    // Collection element types, so for-loop iteration resolves:
                    // Vec<Inline> → Struct(Inline).
                    //
                    // A `Sequence<T>` parameter carries its element type the
                    // same way, and this is the only place it survives
                    // monomorphization: the instantiated copy has fresh node
                    // ids, so the checker's record is gone, but mono did
                    // substitute the declared type — `Sequence<(K, V)>`
                    // arrives here as `Sequence<(i32, i32)>`. Without it
                    // `for (k, v) in self` inside `Sequence.to_map` had no
                    // element type and fell back to a machine word (#1046).
                    if let (Some("Vec" | "Sequence" | "SequenceMut"), [elem]) =
                        (written.name().as_deref(), written.args())
                    {
                        meta.elem_type = Some(ctx.resolve_type_expr(elem));
                    }
                }
            }
        }

        // Inject module-level constants as locals so functions can reference them.
        // Literal consts are evaluated directly; complex initializers (constructor
        // calls, etc.) are lowered as regular expressions at function entry.
        for d in all_decls {
            if let DeclKind::Const(c) = &d.kind {
                if lowerer.locals.contains_key(&c.name) {
                    continue;
                }
                if let Some((op, ty)) = lowerer.try_eval_const_init(&c.init, c.ty.as_ref()) {
                    let local_id = lowerer.builder.alloc_local(c.name.clone(), ty.clone());
                    lowerer.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: local_id,
                        rvalue: MirRValue::Use(op),
                    }));
                    lowerer.locals.insert(c.name.clone(), (local_id, ty));
                } else if lowerer.ctx.folded.consts.contains_key(&c.name) {
                    // Folded at compile time: a reference reads the folded value
                    // (`load_folded`). Queueing the initializer as well
                    // ran the `comptime` block again at runtime, at the first
                    // reference in every function — inside a loop, once a turn —
                    // and each run built a Vec nothing freed
                    // (examples/17_comptime.rk, #1399).
                } else {
                    // Non-literal init (e.g. Shared<T>.new(...)). The type
                    // metadata is recorded here regardless — it costs nothing,
                    // emits no code, and method dispatch reads the const's box
                    // type (`Mutex<Store>`) from more places than just the
                    // reference site.
                    lowerer.record_module_const_meta(&c.name, &c.init);
                    match lowerer.module_const_slot_ty(&c.name) {
                        // One value in a global slot, filled once before main.
                        Some(ty) => {
                            lowerer.const_slots.insert(c.name.clone(), ty);
                        }
                        // Type not pinned down: fall back to re-running the
                        // initializer at the first reference in this function.
                        None => {
                            lowerer.pending_module_consts.insert(
                                c.name.clone(),
                                (c.init.clone(), c.ty.clone()),
                            );
                        }
                    }
                }
            }
        }

        // Run every const's init thunk before main's first line, in declaration
        // order — the same order the interpreter evaluates them in.
        if is_entry_point {
            let mut seen = std::collections::HashSet::new();
            for d in all_decls {
                let DeclKind::Const(c) = &d.kind else { continue };
                if !lowerer.const_slots.contains_key(&c.name) || !seen.insert(c.name.clone()) {
                    continue;
                }
                lowerer.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                    dst: None,
                    func: FunctionRef::internal(const_init_fn_name(&c.name)),
                    args: Vec::new(),
                }));
                const_init_thunks.push(c.clone());
            }
        }

        // The init thunk's whole body is the const's initializer.
        if let Some((const_name, init)) = const_init {
            let (op, actual_ty) = lowerer.lower_expr(init)?;
            // Park the value in a named local so the measuring pass can read
            // the const's real MIR type back off this function.
            let value = lowerer
                .builder
                .alloc_local(CONST_SLOT_VALUE_LOCAL.to_string(), actual_ty.clone());
            lowerer.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: value,
                rvalue: MirRValue::Use(op),
            }));
            lowerer.store_const_slot(const_name, MirOperand::Local(value), &actual_ty);
        }

        // Lower function body
        for stmt in &fn_decl.body {
            lowerer.lower_stmt(stmt)?;
        }

        // Implicit return for functions that don't explicitly return.
        // Void functions get `return`, non-void get Unreachable (caller
        // must ensure all paths return explicitly).
        // Result { ok: Void, .. } also gets an implicit return — emit a
        // wrapped Ok-tagged temp so callers (including the inliner, which
        // copies the return value into the call-site dst) see a properly
        // initialized Result instead of stale stack bytes.
        if lowerer.builder.current_block_unterminated() {
            let result_void_ok = matches!(&ret_ty,
                MirType::Result { ok, .. } if matches!(ok.as_ref(), MirType::Void));
            let implicit_ok = matches!(ret_ty, MirType::Void) || result_void_ok;
            if implicit_ok {
                // For Result<Void, E>, build a temp holding Ok(void) so the
                // caller's slot gets the right tag bytes even when the call
                // is inlined.
                let return_value = if result_void_ok {
                    let wrap_local = lowerer.builder.alloc_temp(ret_ty.clone());
                    // The tag and a zeroed origin, so the caller's copy of
                    // the whole slot carries no stale bytes.
                    lowerer.store_wrapper_tag(
                        wrap_local, &ret_ty, rask_mono::runtime_abi::RASK_TAG_PRESENT,
                    );
                    Some(MirOperand::Local(wrap_local))
                } else {
                    None
                };
                if lowerer.ensure_stack.is_empty() {
                    lowerer.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: return_value }));
                } else {
                    lowerer.builder.terminate(MirTerminator::dummy(MirTerminatorKind::CleanupReturn {
                        value: return_value,
                        cleanup_chain: lowerer.cleanup_chain(),
                    }));
                }
            } else {
                lowerer.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Unreachable));
            }
        }

        // Every element borrow is released right after the call it was taken
        // for, at one of the two call-emission points that lower `mutate` args.
        // A leftover means some other call shape reached a `mutate` aggregate
        // param without going through those — the borrow would stay open and
        // the next push into that Vec would panic, a long way from the cause.
        debug_assert!(
            lowerer.elem_writebacks.is_empty(),
            "{}: {} element borrow(s) never released — a call path reached a \
             `mutate` aggregate parameter without a matching release",
            lowerer.parent_name,
            lowerer.elem_writebacks.len(),
        );

        let mut main_fn = lowerer.builder.finish();
        main_fn.is_extern_c = fn_decl.abi.is_some();
        main_fn.inline_hint = fn_decl.attrs.iter().any(|a| a.is("inline"));
        main_fn.source_file = ctx.source_file.map(|s| s.to_string());
        let mut result = vec![main_fn];
        for f in &mut lowerer.synthesized_functions {
            f.source_file = ctx.source_file.map(|s| s.to_string());
        }
        result.extend(lowerer.synthesized_functions);
        for c in &const_init_thunks {
            result.extend(Self::lower_const_init(c, all_decls, ctx)?);
        }
        Ok(result)
    }

    /// Evaluate a module-level constant initializer to a MIR constant.
    /// Only handles simple literals; complex expressions fall through.
    fn try_eval_const_init(&self, expr: &Expr, ty_hint: Option<&TypeExpr>) -> Option<(MirOperand, MirType)> {
        match &expr.kind {
            ExprKind::Int(val, suffix) => {
                let ty = if let Some(hint) = ty_hint {
                    self.ctx.resolve_type_expr(hint)
                } else {
                    match suffix {
                        Some(rask_ast::token::IntSuffix::I8) => MirType::I8,
                        Some(rask_ast::token::IntSuffix::I16) => MirType::I16,
                        Some(rask_ast::token::IntSuffix::I32) => MirType::I32,
                        Some(rask_ast::token::IntSuffix::U8) => MirType::U8,
                        Some(rask_ast::token::IntSuffix::U16) => MirType::U16,
                        Some(rask_ast::token::IntSuffix::U32) => MirType::U32,
                        Some(rask_ast::token::IntSuffix::U64)
                        | Some(rask_ast::token::IntSuffix::U64ByMagnitude) => MirType::U64,
                        Some(rask_ast::token::IntSuffix::I128)
                        | Some(rask_ast::token::IntSuffix::I128ByMagnitude) => MirType::I128,
                        Some(rask_ast::token::IntSuffix::U128)
                        | Some(rask_ast::token::IntSuffix::U128ByMagnitude) => MirType::U128,
                        _ => MirType::I64,
                    }
                };
                let konst = match ty {
                    MirType::I128 | MirType::U128 => MirConst::Int128(*val),
                    _ => MirConst::Int(*val as i64),
                };
                Some((MirOperand::Constant(konst), ty))
            }
            ExprKind::Float(val, _) => {
                let ty = if let Some(hint) = ty_hint {
                    self.ctx.resolve_type_expr(hint)
                } else {
                    MirType::F64
                };
                Some((MirOperand::Constant(MirConst::Float(*val)), ty))
            }
            ExprKind::String(s) => Some((MirOperand::Constant(MirConst::String(s.clone())), MirType::Buffer)),
            ExprKind::Bytes(b) => Some((MirOperand::Constant(MirConst::Bytes(b.clone())), MirType::Buffer)),
            ExprKind::Bool(b) => Some((MirOperand::Constant(MirConst::Bool(*b)), MirType::Bool)),
            _ => None,
        }
    }

    /// Look up the type of an expression from the type checker.
    /// Returns None if type info is unavailable (e.g., in tests without full type checking).
    fn lookup_expr_type(&self, expr: &Expr) -> Option<MirType> {
        self.ctx.lookup_node_type(expr.id)
    }

    /// Element type of a `Vec<T>`, in either the pre-resolve
    /// (`UnresolvedGeneric`) or resolved (`Generic`) spelling.
    fn vec_elem_raw_type<'t>(&self, ty: &'t Type) -> Option<&'t Type> {
        let args = match ty {
            Type::UnresolvedGeneric { name, args } if name == "Vec" => args,
            // `type_names` stores the declaration name with its parameter list
            // attached ("Vec<T>"), not the bare "Vec" — an exact match here
            // never fired, so a `Vec<any Interface>` field (which resolves to this
            // form through the checker's `HasField` constraint) lost its
            // interface-object element type and segfaulted iterating natively.
            Type::Generic { base, args }
                if self
                    .ctx
                    .type_names
                    .get(base)
                    .is_some_and(|n| n == "Vec") => args,
            _ => return None,
        };
        match args.first()? {
            rask_types::GenericArg::Type(t) => Some(t),
            rask_types::GenericArg::ConstUsize(_) => None,
        }
    }

    /// Extract the element type from an iterator type using raw type info.
    /// For Range<i32>, returns I32. Falls back to AST heuristics after mono.
    fn extract_iterator_elem_type(&self, expr: &Expr) -> Option<MirType> {
        // Try type checker info first (works pre-mono)
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            match ty {
                Type::UnresolvedGeneric { name, args } if name == "Range" => {
                    return args.first().and_then(|arg| {
                        if let rask_types::GenericArg::Type(t) = arg {
                            Some(self.ctx.type_to_mir(t))
                        } else {
                            None
                        }
                    })
                }
                Type::Array { elem, .. } => return Some(self.ctx.type_to_mir(elem)),
                // Vec<any Interface> yields fat-pointer elements. Only the interface-object
                // case is taken from the checker here: concrete element types are
                // already covered by the tracked elem_type below, but an interface object
                // carries a vtable half that nothing downstream can recover once the
                // binding has been typed as a plain scalar.
                _ => {
                    if let Some(Type::InterfaceObject { interface_name, decl }) = self.vec_elem_raw_type(ty) {
                        return Some(MirType::InterfaceObject {
                            interface_name: self.ctx.type_defs.interface_symbol(interface_name, *decl),
                        });
                    }
                }
            }
        }

        // After mono, node IDs are fresh — use AST structure heuristics.
        // Functions known to return Vec<string>:
        if let ExprKind::MethodCall { object, method, .. } = &expr.kind {
            // String methods that produce iterators
            match method.as_str() {
                "split" | "split_whitespace" | "lines" | "graphemes" => return Some(MirType::Buffer),
                "chars" => return Some(MirType::Char),
                "bytes" => return Some(MirType::U8),
                _ => {}
            }
            if let Some(name) = object.name() {
                match (name, method.as_str()) {
                    ("fs", "read_lines") => return Some(MirType::Buffer),
                    ("fs", "read_bytes") => return Some(MirType::U8),
                    _ => {}
                }
            }
        }

        // Variable bound from a known collection — check tracked element types
        if let ExprKind::Ident(name) = &expr.kind {
            if let Some(elem_ty) = self.meta(name).and_then(|m| m.elem_type.as_ref()) {
                return Some(elem_ty.clone());
            }
        }

        // Iterating the result of a call: take the element type off the callee's
        // declared `Vec<T>`. Needed wherever the checker's node types are out of
        // reach — inside a closure body, `for spec in seed_specs()` typed its
        // binding i64 and sent the wrong bytes down a channel (#463).
        let callee = match &expr.kind {
            ExprKind::Call { func, .. } => match &func.kind {
                ExprKind::Ident(name) => Some(name.clone()),
                _ => None,
            },
            ExprKind::MethodCall { object, method, .. } => match &object.kind {
                ExprKind::Ident(recv) => Some(format!("{}_{}", recv, method)),
                _ => None,
            },
            _ => None,
        };
        if let Some(name) = callee {
            if let Some(elem) = self.func_sigs.get(&name).and_then(|s| s.ret_vec_elem.clone()) {
                return Some(elem);
            }
        }

        // Iterating a `Vec<T>` field — the struct declaration knows T even when
        // nothing else here does, because the Vec was filled somewhere else
        // (returned from a call, decoded from JSON).
        self.vec_elem_of_expr(expr)
    }

    /// The success payload of an Option/Result being bound, from the checker if
    /// it typed the node and from the value's own MIR type otherwise.
    ///
    /// The checker doesn't type every node, so `extract_payload_type` alone
    /// returns None often enough to matter — and the lowered value's type is
    /// sitting right there saying `Option(T)` or `Result { ok: T }`. Consulting
    /// it turns most "couldn't work out the payload" cases into an answer instead
    /// of a guess.
    pub(super) fn payload_type_of(&self, expr: &Expr, val_ty: &MirType) -> Option<MirType> {
        self.extract_payload_type(expr).or_else(|| match val_ty {
            MirType::Result { ok, .. } => Some(ok.as_ref().clone()),
            MirType::Option(inner) => Some(inner.as_ref().clone()),
            _ => None,
        })
    }

    /// As `payload_type_of`, for a site that knows whether the optional is
    /// niche-encoded.
    ///
    /// A niche optional carries no tag and no wrapper — a `Handle?` *is* a
    /// `Handle`, with `none` as a sentinel — so `val_ty` is already the payload
    /// type and there's no `Option` for the match above to see through.
    pub(super) fn payload_type_of_niche(
        &self,
        expr: &Expr,
        val_ty: &MirType,
        is_niche: bool,
    ) -> Option<MirType> {
        if is_niche {
            // A niche optional and its payload share a representation — `none`
            // is the null pointer, so there is no wrapper to unwrap. They do
            // not share a *type*: handing back `val_ty` bound `t` in
            // `x is Link<T> as t` at `Link<T>?`, so every later read of `t`
            // was lowered as an unwrap of an optional that isn't there. One
            // dereference too many, and `t.name` read through whatever the
            // first field held (#1179).
            return Some(
                Self::payload_of_mir(val_ty).unwrap_or_else(|| val_ty.clone()),
            );
        }
        self.payload_type_of(expr, val_ty)
    }

    /// The error payload, same two sources as `payload_type_of`.
    pub(super) fn err_type_of(&self, expr: &Expr, val_ty: &MirType) -> Option<MirType> {
        self.extract_err_type(expr).or_else(|| match val_ty {
            MirType::Result { err, .. } => Some(err.as_ref().clone()),
            _ => None,
        })
    }

    /// Extract the Ok/Some payload type from the raw type of an expression.
    /// For Option<T>, returns T. For Result<T, E>, returns T.
    /// MirType::Ptr is a legitimate result for Vec/Map/Channel/etc.,
    /// so the caller must not treat Ptr as "unresolved".
    fn extract_payload_type(&self, expr: &Expr) -> Option<MirType> {
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            match ty {
                Type::Result { ok: inner, err } if **err == Type::None => {
                    Some(self.ctx.type_to_mir(inner))
                }
                Type::Result { ok, .. } => {
                    Some(self.ctx.type_to_mir(ok))
                }
                _ => None,
            }
        } else {
            None
        }
    }

    /// Ok/Some payload of an already-lowered MIR type.
    ///
    /// The backstop for `extract_payload_type`: stdlib bodies and post-mono
    /// copies carry synthesized node IDs the checker never typed, so the only
    /// record of the payload type is the scrutinee's own MIR type, built from
    /// the callee's declared return type. Without this an if-let over a stdlib
    /// `T or E` binds its payload as a bare i64 and method dispatch on the
    /// binding has no type to work from.
    pub(crate) fn payload_of_mir(ty: &MirType) -> Option<MirType> {
        match ty {
            MirType::Result { ok, .. } => Some((**ok).clone()),
            MirType::Option(inner) => Some((**inner).clone()),
            _ => None,
        }
    }

    /// Extract the Err payload type from the raw type of an expression.
    fn extract_err_type(&self, expr: &Expr) -> Option<MirType> {
        if let Some(ty) = self.ctx.lookup_raw_type(expr.id) {
            match ty {
                Type::Result { err, .. } => {
                    Some(self.ctx.type_to_mir(err))
                }
                _ => None,
            }
        } else {
            None
        }
    }

    /// Which side of a Result/Option scrutinee a pattern's `Type`/variant name
    /// refers to: `true` = err/none side (tag 1), `false` = ok/Some side (tag 0).
    ///
    /// The single home of the ok/err routing decision. Real type identities
    /// decide — the name is matched against the scrutinee's actual ok and err
    /// type names (and union members on the err side). The lowercase-first-char
    /// guess is the ONE documented last resort in the lowerer (#259), reached
    /// only when neither side has a discoverable nominal name: generics collapse
    /// to `Ptr`, primitives and strings carry none.
    pub(crate) fn pattern_is_err_side(&self, name: &str, val_ty: &MirType) -> bool {
        let (ok_ty, err_ty) = match val_ty {
            MirType::Result { ok, err } => (Some(ok.as_ref()), Some(err.as_ref())),
            MirType::Option(inner) => (Some(inner.as_ref()), None),
            _ => (None, None),
        };
        // OPT15: `none` names the absent branch, which is always the other
        // side. Without this the lowercase-means-ok fallback below claimed it
        // for the payload, and `x is none` answered backwards on native.
        if name == "none" {
            return true;
        }
        // An optional has no *named* absent side — `none` is the only way to
        // spell it, and that's the line above. Everything else names the
        // payload, whatever the payload's MIR type looks like. Without this an
        // opaque stdlib type that lowers to a bare `i64` (Duration, Instant)
        // reached the uppercase-means-err guess at the bottom, and
        // `d is Duration` on a `Duration?` tested the absent tag — false for a
        // value that was right there.
        if matches!(val_ty, MirType::Option(_)) {
            return false;
        }
        // Identity match wins, on the base name. The pattern carries the type
        // arguments the source wrote and a layout's name doesn't, so
        // `r is Refused<i64>` on a `void or Refused<i64>` compared
        // "Refused<i64>" against "Refused", missed, and fell through to the
        // "err side is nominally named, so this must be the ok side" rule
        // below — routing the error arm to tag 0. `Vec.try_push` is declared
        // `void or GrowError<T>`, so both of its answers read backwards
        // natively while the interpreter had them right.
        //
        // Two instantiations of one generic on the two sides would both match;
        // the ok side is checked first, which is the same precedence an exact
        // match had.
        if let Some(ok) = ok_ty {
            if self.mir_type_name(ok).is_some_and(|n| same_nominal(&n, name)) {
                return false;
            }
        }
        if let Some(err) = err_ty {
            if self.mir_type_name(err).is_some_and(|n| same_nominal(&n, name)) {
                return true;
            }
            if let MirType::Union(variants) = err {
                if variants
                    .iter()
                    .any(|v| self.mir_type_name(v).is_some_and(|n| same_nominal(&n, name)))
                {
                    return true;
                }
            }
        }
        // One side named but unmatched ⇒ the pattern is the other side. Handles
        // generic ok types (`Vec<i32>` → Ptr) that lose their nominal name.
        //
        // Only a *nominal* name counts. An error type that didn't get a layout
        // lowers to `i64`, and taking that as "the err side is named, so this
        // pattern must be the ok side" routed `r is SysError` on a
        // `void or SysError` to tag 0 — the success arm.
        let is_nominal = |n: &str| n.chars().next().is_some_and(|c| c.is_uppercase());
        if err_ty
            .and_then(|t| self.mir_type_name(t))
            .is_some_and(|n| is_nominal(&n))
        {
            return false;
        }
        if ok_ty
            .and_then(|t| self.mir_type_name(t))
            .is_some_and(|n| is_nominal(&n))
        {
            return true;
        }
        // Last resort: lowercase ⇒ ok, uppercase ⇒ err.
        !name.chars().next().map_or(false, |c| c.is_lowercase())
    }

    /// Resolve a pattern to its discriminant tag with no type context. Only the
    /// nominal-variant cases are meaningful here — Ident/TypePat routing that
    /// needs the scrutinee type goes through `pattern_tag_in_type_context`.
    fn pattern_tag(&self, pattern: &rask_ast::expr::Pattern) -> i64 {
        use rask_ast::expr::Pattern;
        match pattern {
            Pattern::Constructor { name, .. } | Pattern::Struct { name, .. } => self.variant_tag(name),
            Pattern::Ident(name) if is_variant_name(name) => self.variant_tag(name),
            _ => 0,
        }
    }

    /// Resolve a pattern to its expected discriminant tag using the scrutinee's
    /// real type. `r is DivError` on `T or DivError` routes to tag 1 (err) by
    /// type identity, not by the capitalization of "DivError".
    pub(crate) fn pattern_tag_in_type_context(
        &self,
        pattern: &rask_ast::expr::Pattern,
        val_ty: &MirType,
    ) -> i64 {
        use rask_ast::expr::Pattern;
        match pattern {
            Pattern::Ident(name) if is_variant_name(name) => {
                // A nominal name on a Result/Option side → that side's tag;
                // otherwise a nested enum variant → its own tag.
                if matches!(val_ty, MirType::Result { .. } | MirType::Option(_)) {
                    let matches_side = self.mir_side_names_contain(val_ty, name);
                    // A side whose type never got a layout lowers to `i64`, so
                    // the name comparison can't find it. Falling through to the
                    // variant lookup then answered 0 for every such name, and
                    // `r is SysError` on a `void or SysError` routed to the
                    // success arm.
                    if matches_side || !self.names_a_known_variant(name) {
                        return if self.pattern_is_err_side(name, val_ty) { 1 } else { 0 };
                    }
                }
                self.variant_tag_in_scrutinee(name, val_ty)
                    .unwrap_or_else(|| self.variant_tag(name))
            }
            Pattern::Ident(_) => 0,
            Pattern::TypePat { ty, .. } => {
                if self.pattern_is_err_side(&type_pat_name(ty), val_ty) { 1 } else { 0 }
            }
            // A named variant is a variant like a positional one: `s is
            // Rect(w, h)` tested against tag 0 whatever `Rect` was, so it was
            // true of every first variant and of nothing else.
            Pattern::Constructor { name, .. } | Pattern::Struct { name, .. } => self
                .variant_tag_in_scrutinee(name, val_ty)
                .unwrap_or_else(|| self.variant_tag(name)),
            _ => self.pattern_tag(pattern),
        }
    }

    /// True when some declared enum has a variant by this name.
    fn names_a_known_variant(&self, name: &str) -> bool {
        let bare = name.rsplit('.').next().unwrap_or(name);
        self.ctx
            .enum_layouts
            .iter()
            .any(|l| l.variants.iter().any(|v| v.name == bare))
            || rask_stdlib::ordering_tag(bare).is_some()
    }

    /// True when `name` is the nominal name of the ok side, err side, or an err
    /// union member of a Result/Option scrutinee.
    fn mir_side_names_contain(&self, val_ty: &MirType, name: &str) -> bool {
        let sides: [Option<&MirType>; 2] = match val_ty {
            MirType::Result { ok, err } => [Some(ok.as_ref()), Some(err.as_ref())],
            MirType::Option(inner) => [Some(inner.as_ref()), None],
            _ => [None, None],
        };
        for side in sides.into_iter().flatten() {
            if self.mir_type_name(side).is_some_and(|n| same_nominal(&n, name)) {
                return true;
            }
            if let MirType::Union(variants) = side {
                if variants
                    .iter()
                    .any(|v| self.mir_type_name(v).is_some_and(|n| same_nominal(&n, name)))
                {
                    return true;
                }
            }
        }
        false
    }

    /// Which member of a union a type is, by position as written.
    pub(crate) fn union_member_index(&self, union_ty: &MirType, member: &MirType) -> Option<usize> {
        let MirType::Union(members) = union_ty else {
            return None;
        };
        if let Some(i) = members.iter().position(|m| m == member) {
            return Some(i);
        }
        // A member that never got a layout lowers to something anonymous, so
        // compare by nominal name when the types themselves don't match.
        let name = self.mir_type_name(member)?;
        members
            .iter()
            .position(|m| self.mir_type_name(m).as_deref() == Some(name.as_str()))
    }

    /// Put a member value into its union's `[member index][payload]` layout.
    ///
    /// The operand twin of `wrap_union_member`, which takes a local because the
    /// `try` path already has one. Returns `None` when the value isn't a member
    /// of this union, so the caller can fall through to whatever it did before.
    fn wrap_operand_into_union(
        &mut self,
        val: &MirOperand,
        src_ty: &MirType,
        union_ty: &MirType,
    ) -> Option<MirOperand> {
        let MirType::Union(members) = union_ty else {
            return None;
        };
        if matches!(src_ty, MirType::Union(_)) {
            return None;
        }
        let index = self.union_member_index(union_ty, src_ty)?;
        let member = members.get(index)?.clone();

        let slot = self.builder.alloc_temp(union_ty.clone());
        self.store_wrapper_tag(slot, union_ty, index as i64);
        self.store_wrapper_payload(slot, union_ty, val.clone(), &member);
        Some(MirOperand::Local(slot))
    }

    /// Which member of a union a *name* is, by position as written.
    /// The member type and its byte offset when `ty_name` names one member of
    /// the error union of `scrutinee_ty`. `None` for anything else — a plain
    /// error type, or a name that isn't a member.
    fn union_member_binding(&self, scrutinee_ty: &MirType, ty_name: &str) -> Option<(MirType, u32)> {
        let MirType::Result { err, .. } = scrutinee_ty else {
            return None;
        };
        let MirType::Union(members) = err.as_ref() else {
            return None;
        };
        let bare = ty_name.rsplit('.').next().unwrap_or(ty_name);
        let idx = self.union_member_index_by_name(err.as_ref(), bare)?;
        let member = members.get(idx)?.clone();
        Some((member, scrutinee_ty.payload_offset() + err.payload_offset()))
    }

    pub(crate) fn union_member_index_by_name(&self, union_ty: &MirType, name: &str) -> Option<usize> {
        let MirType::Union(members) = union_ty else {
            return None;
        };
        members
            .iter()
            .position(|m| self.mir_type_name(m).as_deref() == Some(name))
    }

    /// Look up the tag value for a variant name.
    ///
    /// Accepts both the bare and the qualified spelling. A pattern written
    /// `Kind.B` arrives here as one string, and searching the layouts for a
    /// variant literally named "Kind.B" never matched — so every `x is
    /// Enum.Variant` compared the tag against 0 and only ever answered true for
    /// the first variant (#476). `match` already stripped the qualifier.
    ///
    /// When the qualifier is there it also pins down which enum to look in,
    /// which matters as soon as two enums share a variant name.
    fn variant_tag(&self, name: &str) -> i64 {
        self.variant_tag_impl(name)
    }

    /// The tag `name` carries inside the scrutinee's own enum, when the
    /// scrutinee is an enum that has such a variant.
    ///
    /// Without a qualifier `variant_tag` scans every layout and takes the first
    /// hit, so an unqualified arm loses the name to whichever enum was
    /// registered first — the stdlib included. `enum Top { Io(Inner) }` matched
    /// with `Io(e) => …` compared against `HttpError.Io`'s tag from
    /// `stdlib/http.rk`, no arm matched, and the match trapped (SIGILL). The
    /// value being matched knows its own enum; ask it first.
    pub(crate) fn variant_tag_in_scrutinee(&self, name: &str, val_ty: &MirType) -> Option<i64> {
        let MirType::Enum(crate::types::EnumLayoutId { id, .. }) = val_ty else {
            return None;
        };
        let layout = self.ctx.enum_layouts.get(*id as usize)?;
        let bare = name.rsplit('.').next().unwrap_or(name);
        layout.variants.iter().find(|v| v.name == bare).map(|v| v.tag as i64)
    }

    fn variant_tag_impl(&self, name: &str) -> i64 {
        if let Some(tag) = rask_stdlib::wrappers::wrapper_tag(name) {
            return tag as i64;
        }
        let (enum_name, bare) = match name.rsplit_once('.') {
            Some((qualifier, variant)) => (
                Some(qualifier.rsplit('.').next().unwrap_or(qualifier)),
                variant,
            ),
            None => (None, name),
        };
        if let Some(enum_name) = enum_name {
            for layout in self.ctx.enum_layouts.iter().filter(|l| l.name == enum_name) {
                for variant in &layout.variants {
                    if variant.name == bare {
                        return variant.tag as i64;
                    }
                }
            }
            // `Ordering` is registered by the compiler, not declared,
            // so it has no layout to read tags from.
            if enum_name == "Ordering" {
                if let Some(tag) = rask_stdlib::ordering_tag(bare) {
                    return tag;
                }
            }
        }
        // Unqualified, or the qualifier named no known enum: first
        // layout that declares the variant.
        for layout in self.ctx.enum_layouts {
            for variant in &layout.variants {
                if variant.name == bare {
                    return variant.tag as i64;
                }
            }
        }
        // A declared enum wins the bare name; `Ordering` picks up what's
        // left, which is what `Ordering.Less` arrives as — the qualifier
        // is dropped before it gets here.
        rask_stdlib::ordering_tag(bare).unwrap_or(0)
    }

    /// Check if an expression's type is niche-optimized Option<Handle<T>>.
    fn is_niche_option_expr(&self, expr: &Expr) -> bool {
        self.niche_sentinel_of_expr(expr).is_some()
    }

    /// The `none` word for an expression whose checker type is a niche option.
    fn niche_sentinel_of_expr(&self, expr: &Expr) -> Option<i64> {
        self.ctx.niche_option_sentinel(self.ctx.lookup_raw_type(expr.id)?)
    }

    /// Same question, with the lowered type as a second opinion. A `Handle?`
    /// field inside a pool element often has no checker type to read — the
    /// element's fields aren't typed at the use site — and reading it as a
    /// tagged option then tested a tag that isn't there (#438).
    ///
    /// This is the answer. `is_niche_option_expr` on its own is only right
    /// where there is no lowered type to consult; three call sites used to
    /// spell this out inline instead, which is how the checker-only version
    /// kept getting used where a MIR type was sitting right there.
    pub(crate) fn option_is_niche(&self, expr: &Expr, ty: &MirType) -> bool {
        self.option_niche(expr, ty).is_some()
    }

    /// `option_is_niche`, and which word means `none` when it is one.
    ///
    /// The lowered type answers first. It knows whether this is a handle or a
    /// link, and those have different sentinels; the checker type is the
    /// fallback for the case `option_is_niche` exists for, where there is no
    /// useful MIR type at the use site.
    pub(crate) fn option_niche(&self, expr: &Expr, ty: &MirType) -> Option<i64> {
        if let Some(sentinel) = mir_niche_none(ty) {
            return Some(sentinel);
        }
        // A lowered `Option` whose payload is a settled non-niche type has
        // already answered: it carries a tag, and the checker must not talk us
        // out of reading it. The two can disagree — a stub whose MIR return
        // type is a tagged `i64?` where the checker says `Handle<T>?` — and
        // taking the checker's word there emitted a sentinel compare against a
        // slot address.
        if matches!(ty, MirType::Option(inner)
            if !inner.is_niche_payload() && !matches!(**inner, MirType::Ptr))
        {
            return None;
        }
        self.niche_sentinel_of_expr(expr)
    }

    /// `option_is_niche` for a value that's already lowered — reads the MIR
    /// type off the operand's local.
    pub(crate) fn option_operand_niche(&self, expr: &Expr, op: &MirOperand) -> Option<i64> {
        if let MirOperand::Local(id) = op {
            if let Some(s) = self.builder.local_type(*id).as_ref().and_then(mir_niche_none) {
                return Some(s);
            }
        }
        self.niche_sentinel_of_expr(expr)
    }

    /// Emit a tag-equivalent check for an option value.
    ///
    /// Returns a local that is 0 for Some, non-zero for None — matching
    /// the tag convention used by branches. Works for both niche-optimized
    /// (compare-to-sentinel) and tagged union (EnumTag load) options.
    fn emit_option_tag(&mut self, value: &MirOperand, niche: Option<i64>) -> LocalId {
        if let Some(sentinel) = niche {
            let result = self.builder.alloc_temp(MirType::U8);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Eq,
                    left: value.clone(),
                    right: MirOperand::Constant(MirConst::Int(sentinel)),
                },
            }));
            result
        } else {
            let result = self.builder.alloc_temp(MirType::U8);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result,
                rvalue: MirRValue::EnumTag { value: value.clone() },
            }));
            result
        }
    }

    /// Read a wrapper's payload as `arm`: at the offset the wrapper's layout
    /// gives, in place for an aggregate and loaded at its own width for a
    /// scalar. The offset is the base local's own when it is a wrapper, and
    /// `wrapper_ty`'s otherwise.
    ///
    /// Saying both outright is the point. When `ok` and `err` disagree about
    /// being an aggregate, codegen can't tell from the wrapper which one is
    /// being read (#383/#389).
    pub(crate) fn payload_field(
        &self,
        base: MirOperand,
        wrapper_ty: &MirType,
        arm: &MirType,
    ) -> MirRValue {
        let stored = match &base {
            MirOperand::Local(id) => self.builder.local_type(*id).filter(|t| t.wrapper_layout().is_some()),
            _ => None,
        };
        let wrapper = stored.as_ref().unwrap_or(wrapper_ty);
        MirRValue::Field {
            byte_offset: Some(wrapper.payload_offset()),
            access: FieldAccess::for_field(arm, arm.size()),
            base,
            field_index: 0,
        }
    }

    /// Extract the payload of a `T?` or one side of a `T or E` into a new
    /// local. A niche option is its payload.
    fn emit_option_payload(
        &mut self,
        value: MirOperand,
        payload_ty: MirType,
        is_niche: bool,
    ) -> LocalId {
        let result = self.builder.alloc_temp(payload_ty.clone());
        let rvalue = if is_niche {
            MirRValue::Use(value)
        } else {
            // The container has the last word on how the payload is *stored*.
            // `Map.get` returns `i64?` — a pointer to the map's own value — while
            // the checker calls the payload a `User`. Addressing the container
            // instead of loading that pointer handed back the address of the tag,
            // so `self.users.get(id) ?? …` read a User out of the option's own
            // first bytes and `/users/1` segfaulted.
            let stored = match &value {
                MirOperand::Local(id) => self.builder.local_type(*id),
                _ => None,
            };
            let arm = match &stored {
                Some(MirType::Option(inner)) => (**inner).clone(),
                _ => payload_ty.clone(),
            };
            let wrapper = stored.unwrap_or_else(|| MirType::Option(Box::new(payload_ty)));
            self.payload_field(value, &wrapper, &arm)
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign { dst: result, rvalue }));
        result
    }

    /// Look up a user enum variant's field types, keyed by variant name.
    /// Returns (mir type, absolute byte offset within the enum, field size)
    /// per field, in declaration order. `None` when `scrutinee_ty` isn't a
    /// user enum or the variant/layout can't be found (Option/Result, whose
    /// payload comes from `extract_payload_type` instead).
    fn variant_field_types(
        &self,
        scrutinee_ty: &MirType,
        variant_name: &str,
    ) -> Option<Vec<(MirType, u32, u32)>> {
        let MirType::Enum(crate::types::EnumLayoutId { id: idx, .. }) = scrutinee_ty else {
            return None;
        };
        let layout = self.ctx.enum_layouts.get(*idx as usize)?;
        let bare_name = variant_name.rsplit('.').next().unwrap_or(variant_name);
        let variant = layout.variants.iter().find(|v| v.name == bare_name)?;
        Some(variant.fields.iter().map(|f| {
            (self.ctx.type_to_mir(&f.ty), variant.payload_offset + f.offset, f.size)
        }).collect())
    }

    /// Field names of a named variant, in declaration order: of the scrutinee's
    /// own enum, or of the error enum of a `T or E`.
    fn variant_field_names(&self, scrutinee_ty: &MirType, variant_name: &str) -> Option<Vec<String>> {
        let enum_ty = match scrutinee_ty {
            MirType::Result { err, .. } => err.as_ref(),
            other => other,
        };
        let MirType::Enum(crate::types::EnumLayoutId { id: idx, .. }) = enum_ty else {
            return None;
        };
        let layout = self.ctx.enum_layouts.get(*idx as usize)?;
        let bare_name = variant_name.rsplit('.').next().unwrap_or(variant_name);
        let variant = layout.variants.iter().find(|v| v.name == bare_name)?;
        Some(variant.fields.iter().map(|f| f.name.clone()).collect())
    }

    /// `Enum.Variant` naming a variant of a Result's error enum: each field's
    /// type, its offset *within the err payload*, and its size.
    fn err_variant_fields(
        &self,
        scrutinee_ty: &MirType,
        ty_name: &str,
    ) -> Option<Vec<(MirType, u32, u32)>> {
        let MirType::Result { err, .. } = scrutinee_ty else { return None };
        let (enum_name, variant_name) = ty_name.split_once('.')?;
        let MirType::Enum(crate::types::EnumLayoutId { id: idx, .. }) = err.as_ref() else {
            return None;
        };
        let layout = self.ctx.enum_layouts.get(*idx as usize)?;
        if layout.name != enum_name {
            return None;
        }
        let variant = layout.variants.iter().find(|v| v.name == variant_name)?;
        if variant.fields.is_empty() {
            return None;
        }
        Some(
            variant
                .fields
                .iter()
                .map(|f| {
                    (
                        self.ctx.type_to_mir(&f.ty),
                        variant.payload_offset + f.offset,
                        f.size,
                    )
                })
                .collect(),
        )
    }

    /// Bind pattern payload — with niche awareness.
    /// `payload_ty` is optional because for the common case there is no such
    /// type to have: a Constructor pattern on a user enum takes each field's
    /// type from the enum layout, and the Option/Result payload is never
    /// consulted. Resolving it eagerly at the call site meant asking a question
    /// with no answer on every `if m is Msg.Text(t)`. The paths that genuinely
    /// need it demand it below, where not knowing it is a real gap.
    fn bind_pattern_payload_niche(
        &mut self,
        pattern: &rask_ast::expr::Pattern,
        scrutinee: &Expr,
        value: MirOperand,
        payload_ty: Option<MirType>,
        is_niche: bool,
        scrutinee_ty: &MirType,
    ) {
        self.bind_pattern_fields(pattern, value, payload_ty, is_niche, scrutinee_ty);
        self.retain_pattern_copies(pattern, scrutinee, scrutinee_ty);
    }

    /// Whether `pattern` binds a name anywhere inside a variant's fields.
    fn binds_any(pattern: &rask_ast::expr::Pattern) -> bool {
        use rask_ast::expr::Pattern;
        match pattern {
            Pattern::Constructor { fields, .. } => fields.iter().any(|f| {
                matches!(f, Pattern::Ident(n) if !is_variant_name(n)) || Self::binds_any(f)
            }),
            Pattern::Struct { fields, .. } => fields.iter().any(|(_, f)| {
                matches!(f, Pattern::Ident(n) if !is_variant_name(n)) || Self::binds_any(f)
            }),
            _ => false,
        }
    }

    /// Whether a variant field's pattern needs testing at runtime: a nested
    /// variant, or a literal or range somewhere inside one. A bare name or
    /// `_` takes any value.
    pub(super) fn nested_needs_test(pattern: &rask_ast::expr::Pattern) -> bool {
        use rask_ast::expr::Pattern;
        match pattern {
            Pattern::Wildcard => false,
            Pattern::Ident(n) => is_variant_name(n),
            _ => true,
        }
    }

    /// Test `pattern` against `value`, a `ty`, branching to `fail` when it
    /// doesn't match. Returns in the block where it did.
    ///
    /// For a pattern nested inside a variant's field: `Outer.Wrapped(Inner.Bad(m))`
    /// tests `Wrapped` with the switch and `Bad` here. Each level reads its tag
    /// only after the level above has passed, so a payload is never read as a
    /// variant it isn't. Nothing is bound: `bind_pattern_fields` does that once
    /// the whole test has passed. Before this the inner tag wasn't tested at all,
    /// so `Wrapped(Inner.Good(_))` took the first `Wrapped` arm, and the inner
    /// binding was never made (#1645).
    pub(super) fn emit_subpattern_test(
        &mut self,
        value: &MirOperand,
        ty: &MirType,
        pattern: &rask_ast::expr::Pattern,
        fail: BlockId,
    ) -> Result<(), LoweringError> {
        use rask_ast::expr::Pattern;
        match pattern {
            Pattern::Wildcard => Ok(()),
            Pattern::Ident(name) if !is_variant_name(name) => Ok(()),
            Pattern::Literal(_) | Pattern::Range { .. } => {
                let pass = self.builder.create_block();
                self.emit_pattern_test(value, pattern, pass, fail)?;
                self.builder.switch_to_block(pass);
                Ok(())
            }
            // A named variant tests like its positional spelling.
            Pattern::Struct { name, fields, .. } => {
                let Some(names) = self.variant_field_names(ty, name) else {
                    return Err(LoweringError::InvalidConstruct(format!(
                        "`{name} {{ … }}` nested in a variant pattern needs an enum field, \
                         found `{ty:?}`"
                    )));
                };
                let positional = names
                    .iter()
                    .map(|n| {
                        fields.iter().find(|(f, _)| f == n).map_or(Pattern::Wildcard, |(_, p)| p.clone())
                    })
                    .collect();
                let as_constructor = Pattern::Constructor { name: name.clone(), fields: positional };
                self.emit_subpattern_test(value, ty, &as_constructor, fail)
            }
            Pattern::Ident(name) | Pattern::Constructor { name, .. } => {
                let fields: &[Pattern] = match pattern {
                    Pattern::Constructor { fields, .. } => fields,
                    _ => &[],
                };
                let (Some(tag), Some(field_types)) =
                    (self.variant_tag_in_scrutinee(name, ty), self.variant_field_types(ty, name))
                else {
                    return Err(LoweringError::InvalidConstruct(format!(
                        "`{name}` nested in a variant pattern names no variant of the \
                         field's type `{ty:?}`; only an enum's own variants nest so far"
                    )));
                };
                let tag_local = self.emit_enum_tag(value.clone());
                let is_variant = self.emit_eq_const(tag_local, tag);
                let pass = self.builder.create_block();
                self.builder.terminate(MirTerminator::dummy(MirTerminatorKind::Branch {
                    cond: MirOperand::Local(is_variant),
                    then_block: pass,
                    else_block: fail,
                }));
                self.builder.switch_to_block(pass);
                for (j, field_pat) in fields.iter().enumerate() {
                    if !Self::nested_needs_test(field_pat) {
                        continue;
                    }
                    let Some((field_ty, off, sz)) = field_types.get(j).cloned() else {
                        return Err(LoweringError::InvalidConstruct(format!(
                            "`{name}` has {} fields, the pattern names {}",
                            field_types.len(),
                            fields.len(),
                        )));
                    };
                    let inner = self.builder.alloc_temp(field_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: inner,
                        rvalue: MirRValue::Field {
                            base: value.clone(),
                            field_index: j as u32,
                            byte_offset: Some(off),
                            access: FieldAccess::for_field(&field_ty, sz),
                        },
                    }));
                    self.emit_subpattern_test(&MirOperand::Local(inner), &field_ty, field_pat, fail)?;
                }
                Ok(())
            }
            other => Err(LoweringError::InvalidConstruct(format!(
                "`{other:?}` nested in a variant pattern isn't lowered natively yet"
            ))),
        }
    }

    fn bind_pattern_fields(
        &mut self,
        pattern: &rask_ast::expr::Pattern,
        value: MirOperand,
        payload_ty: Option<MirType>,
        is_niche: bool,
        scrutinee_ty: &MirType,
    ) {
        use rask_ast::expr::Pattern;
        match pattern {
            // A named variant binds like its positional spelling: each field's
            // pattern at that field's index, `_` for the ones it leaves out.
            Pattern::Struct { name, fields, .. }
                if self.variant_field_names(scrutinee_ty, name).is_some() =>
            {
                let names = self.variant_field_names(scrutinee_ty, name).unwrap_or_default();
                let positional = names
                    .iter()
                    .map(|n| {
                        fields
                            .iter()
                            .find(|(f, _)| f == n)
                            .map_or(Pattern::Wildcard, |(_, p)| p.clone())
                    })
                    .collect();
                let as_constructor = Pattern::Constructor { name: name.clone(), fields: positional };
                self.bind_pattern_fields(&as_constructor, value, payload_ty, is_niche, scrutinee_ty);
            }
            Pattern::Constructor { name, .. }
                if matches!(scrutinee_ty, MirType::Result { err, .. }
                    if matches!(err.as_ref(), MirType::Enum(_))
                        && self.variant_field_types(err, name).is_some()) =>
            {
                // `MyErr.Bad(m)` against a `T or MyErr`: the variant's fields
                // sit in the error enum, which lives in the result's payload
                // slot. Read against the result itself, `m` came out of the
                // wrong bytes and printed empty.
                let MirType::Result { err, .. } = scrutinee_ty else { unreachable!() };
                let err_ty = (**err).clone();
                let err_local = self.builder.alloc_temp(err_ty.clone());
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: err_local,
                    rvalue: self.payload_field(value, scrutinee_ty, &err_ty),
                }));
                self.bind_pattern_fields(pattern, MirOperand::Local(err_local), payload_ty, false, &err_ty);
            }
            Pattern::Constructor { name, fields } => {
                // User enums carry a distinct type per field (e.g. `Circle(f64)`
                // vs `Rectangle(f64, f64)`); `payload_ty` is only a single type
                // (from extract_payload_type, which only understands Option/Result),
                // so look up each field's real type from the enum layout when one
                // exists. Falls back to `payload_ty` for niche Option/Result values.
                let variant_fields = self.variant_field_types(scrutinee_ty, name);
                for (i, field_pat) in fields.iter().enumerate() {
                    if let Pattern::Ident(name) = field_pat {
                        let demand = || payload_ty.clone().unwrap_or_else(|| {
                            crate::fallback::unknown_type("lower/mod:constructor_payload")
                        });
                        let (field_ty, field_loc) = if let Some(ref vf) = variant_fields {
                            vf.get(i)
                                .map(|(ty, off, sz)| (ty.clone(), Some((*off, *sz))))
                                .unwrap_or_else(|| (demand(), None))
                        } else {
                            (demand(), None)
                        };
                        let local = self.builder.alloc_local(name.clone(), field_ty.clone());
                        self.locals.insert(name.clone(), (local, field_ty.clone()));
                        let rvalue = if is_niche {
                            // Niche: the value IS the payload
                            MirRValue::Use(value.clone())
                        } else {
                            MirRValue::Field {
                                base: value.clone(),
                                field_index: i as u32,
                                byte_offset: field_loc.map(|(off, _)| off),
                                access: field_loc.map_or(FieldAccess::Word, |(_, sz)| {
                                    FieldAccess::for_field(&field_ty, sz)
                                }),
                            }
                        };
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: local,
                            rvalue,
                        }));
                        // Set type prefix so method calls on this binding
                        // get correct qualification (e.g., data.lines() → string_lines)
                        if let Some(prefix) = self.mir_type_name(&field_ty) {
                            self.meta_mut(&name).type_prefix = Some(prefix);
                        }
                    } else if Self::binds_any(field_pat) && !is_niche {
                        // A nested variant pattern: its tags were tested before
                        // binding, so the field holds that variant. Read it out
                        // and bind inside it the same way (#1645).
                        let Some((field_ty, off, sz)) =
                            variant_fields.as_ref().and_then(|vf| vf.get(i).cloned())
                        else {
                            continue;
                        };
                        let inner = self.builder.alloc_temp(field_ty.clone());
                        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                            dst: inner,
                            rvalue: MirRValue::Field {
                                base: value.clone(),
                                field_index: i as u32,
                                byte_offset: Some(off),
                                access: FieldAccess::for_field(&field_ty, sz),
                            },
                        }));
                        self.bind_pattern_fields(
                            field_pat, MirOperand::Local(inner), None, false, &field_ty,
                        );
                    }
                    // Wildcard, Literal in field position — skip binding
                }
            }
            // ER23/ER27: `Type as name` — bind the payload as a fresh local. The
            // only caller (WhileLet) already routed control flow via
            // `pattern_tag_in_type_context` and passes the ok payload type, so
            // bind that directly — no case guess needed.
            Pattern::TypePat { ty, binding: Some(name) } => {
                let ty_name = &type_pat_name(ty);
                // ER23 at variant granularity: `r is MyErr.Worse as w` binds the
                // *variant's* payload, not the whole error. The test is already
                // two-layer — err tag, then the variant tag — so all that's needed
                // is reading at the variant's own offset inside the err payload.
                // Binding `payload_ty` handed over a `MyErr` where a `string` was
                // named, and `{w}` printed the enum's tag (#766).
                if let Some(fields) = self.err_variant_fields(scrutinee_ty, ty_name) {
                    let bound_ty = match fields.as_slice() {
                        [(ty, _, _)] => ty.clone(),
                        many => MirType::Tuple(many.iter().map(|(t, _, _)| t.clone()).collect()),
                    };
                    let local = self.builder.alloc_local(name.clone(), bound_ty.clone());
                    match fields.as_slice() {
                        // One field binds directly.
                        [(field_ty, offset, size)] => {
                            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                dst: local,
                                rvalue: MirRValue::Field {
                                    base: value.clone(),
                                    field_index: 0,
                                    byte_offset: Some(scrutinee_ty.payload_offset() + *offset),
                                    access: FieldAccess::for_field(field_ty, *size),
                                },
                            }));
                        }
                        // Several bind a tuple, which has to be built: the
                        // variant's fields sit at the enum's offsets, the tuple's
                        // at its own, and the two don't line up.
                        many => {
                            let mut tuple_offset = 0u32;
                            for (field_ty, src_offset, size) in many {
                                let align = field_ty.align().max(1);
                                tuple_offset = (tuple_offset + align - 1) & !(align - 1);
                                let read = self.builder.alloc_temp(field_ty.clone());
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                                    dst: read,
                                    rvalue: MirRValue::Field {
                                        base: value.clone(),
                                        field_index: 0,
                                        byte_offset: Some(
                                            scrutinee_ty.payload_offset() + *src_offset,
                                        ),
                                        access: FieldAccess::for_field(field_ty, *size),
                                    },
                                }));
                                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                                    addr: local,
                                    offset: tuple_offset,
                                    value: MirOperand::Local(read),
                                    store_size: field_ty.size(),
                                    field: None,
                                }));
                                tuple_offset += field_ty.size();
                            }
                        }
                    }
                    self.locals.insert(name.clone(), (local, bound_ty.clone()));
                    if let Some(prefix) = self.mir_type_name(&bound_ty) {
                        self.meta_mut(name).type_prefix = Some(prefix);
                    }
                    return;
                }
                // `r is AErr as e` on an `i64 or (AErr | BErr)`. The test that
                // got us here already checked the member index, so `e` is an
                // `AErr` and nothing else — binding it at the union type made
                // every call on it a switch over all members, which emitted a
                // call to `BErr_tag` that doesn't exist (#1002). The member's
                // bytes sit past the index, inside the Result's payload.
                if let Some((member_ty, member_off)) =
                    self.union_member_binding(scrutinee_ty, ty_name)
                {
                    let local = self.builder.alloc_local(name.clone(), member_ty.clone());
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: local,
                        rvalue: MirRValue::Field {
                            base: value.clone(),
                            field_index: 0,
                            byte_offset: Some(member_off),
                            access: FieldAccess::for_field(&member_ty, member_ty.size()),
                        },
                    }));
                    self.locals.insert(name.clone(), (local, member_ty.clone()));
                    if let Some(prefix) = self.mir_type_name(&member_ty) {
                        self.meta_mut(name).type_prefix = Some(prefix);
                    }
                    return;
                }

                let bound_ty = payload_ty.clone().unwrap_or_else(|| {
                    crate::fallback::unknown_type("lower/mod:typepat_payload")
                });
                let local = self.builder.alloc_local(name.clone(), bound_ty.clone());
                let rvalue = if is_niche {
                    MirRValue::Use(value.clone())
                } else {
                    self.payload_field(value.clone(), scrutinee_ty, &bound_ty)
                };
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: local,
                    rvalue,
                }));
                self.locals.insert(name.clone(), (local, bound_ty.clone()));
                if let Some(prefix) = self.mir_type_name(&bound_ty) {
                    self.meta_mut(name).type_prefix = Some(prefix);
                }
            }
            // Ident that is a variant name: no binding (pure match)
            // Ident that is a variable: this shouldn't reach here (it's a binding, not a match)
            _ => {}
        }
    }

    /// Collect free variables in a closure body — names used but not defined
    /// within the closure itself (params or local bindings).
    fn collect_free_vars(
        &self,
        body: &Expr,
        params: &[rask_ast::expr::ClosureParam],
    ) -> Vec<(String, LocalId, MirType, bool)> {
        let mut free = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let bound: std::collections::HashSet<String> =
            params.iter().map(|p| p.name.clone()).collect();
        self.walk_free_vars(body, &bound, &mut seen, &mut free);
        free
    }

    /// Recursive walk to find free variable references.
    fn walk_free_vars(
        &self,
        expr: &Expr,
        bound: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        free: &mut Vec<(String, LocalId, MirType, bool)>,
    ) {
        use rask_ast::expr::ExprKind;
        match &expr.kind {
            ExprKind::Ident(name) => {
                if !bound.contains(name) && !seen.contains(name) {
                    if let Some((local_id, ty)) = self.locals.get(name) {
                        seen.insert(name.clone());
                        // The checker's answer, for `ClosureCapture::copy`.
                        let copy = self
                            .ctx
                            .lookup_raw_type(expr.id)
                            .is_some_and(|t| self.ctx.type_defs.is_copy(t));
                        free.push((name.clone(), *local_id, ty.clone(), copy));
                    }
                }
            }
            ExprKind::Block(stmts) => {
                self.walk_free_vars_block(stmts, bound, seen, free);
            }
            ExprKind::Binary { left, right, .. } => {
                self.walk_free_vars(left, bound, seen, free);
                self.walk_free_vars(right, bound, seen, free);
            }
            ExprKind::Unary { operand, .. } => {
                self.walk_free_vars(operand, bound, seen, free);
            }
            ExprKind::Call { func, args } => {
                self.walk_free_vars(func, bound, seen, free);
                for arg in args {
                    self.walk_free_vars(&arg.expr, bound, seen, free);
                }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.walk_free_vars(object, bound, seen, free);
                for arg in args {
                    self.walk_free_vars(&arg.expr, bound, seen, free);
                }
            }
            ExprKind::If { cond, then_branch, else_branch, .. } => {
                self.walk_free_vars(cond, bound, seen, free);
                self.walk_free_vars(then_branch, bound, seen, free);
                if let Some(e) = else_branch {
                    self.walk_free_vars(e, bound, seen, free);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.walk_free_vars(scrutinee, bound, seen, free);
                for arm in arms {
                    let mut arm_bound = bound.clone();
                    collect_pattern_names(&arm.pattern, &mut arm_bound);
                    self.walk_free_vars(&arm.body, &arm_bound, seen, free);
                }
            }
            ExprKind::Field { object, .. } => {
                self.walk_free_vars(object, bound, seen, free);
            }
            ExprKind::DynamicField { object, field_expr } => {
                self.walk_free_vars(object, bound, seen, free);
                self.walk_free_vars(field_expr, bound, seen, free);
            }
            ExprKind::Index { object, index } => {
                self.walk_free_vars(object, bound, seen, free);
                self.walk_free_vars(index, bound, seen, free);
            }
            ExprKind::Array(elems) => {
                for e in elems { self.walk_free_vars(e, bound, seen, free); }
            }
            ExprKind::Tuple(elems) => {
                for e in elems { self.walk_free_vars(e, bound, seen, free); }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields { self.walk_free_vars(&f.value, bound, seen, free); }
                if let Some(s) = spread { self.walk_free_vars(s, bound, seen, free); }
            }
            ExprKind::Closure { params: inner_params, body, .. } => {
                let mut inner_bound = bound.clone();
                for p in inner_params { inner_bound.insert(p.name.clone()); }
                self.walk_free_vars(body, &inner_bound, seen, free);
            }
            ExprKind::Spawn { body, .. } => self.walk_free_vars(body, bound, seen, free),
            ExprKind::Try { expr: inner } | ExprKind::Take { place: inner } => {
                self.walk_free_vars(inner, bound, seen, free);
            }
            ExprKind::Catch { value, ref clause } => {
                self.walk_free_vars(value, bound, seen, free);
                let mut inner_bound = bound.clone();
                inner_bound.insert(clause.binder.clone());
                self.walk_free_vars(&clause.body, &inner_bound, seen, free);
            }
            ExprKind::IsPresent { expr: inner, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
            }
            ExprKind::Unwrap { expr: inner, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
            }
            ExprKind::Cast { expr: inner, .. } | ExprKind::Convert { expr: inner, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.walk_free_vars(value, bound, seen, free);
                self.walk_free_vars(default, bound, seen, free);
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start { self.walk_free_vars(s, bound, seen, free); }
                if let Some(e) = end { self.walk_free_vars(e, bound, seen, free); }
            }
            ExprKind::IfLet { expr: inner, pattern, then_branch, else_branch, else_binding: _ } => {
                self.walk_free_vars(inner, bound, seen, free);
                let mut then_bound = bound.clone();
                collect_pattern_names(pattern, &mut then_bound);
                self.walk_free_vars(then_branch, &then_bound, seen, free);
                if let Some(e) = else_branch { self.walk_free_vars(e, bound, seen, free); }
            }
            ExprKind::GuardPattern { expr: inner, else_branch, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
                self.walk_free_vars(else_branch, bound, seen, free);
            }
            ExprKind::IsPattern { expr: inner, .. } => {
                self.walk_free_vars(inner, bound, seen, free);
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                self.walk_free_vars(condition, bound, seen, free);
                if let Some(m) = message { self.walk_free_vars(m, bound, seen, free); }
            }
            ExprKind::OptionalField { object, .. } => {
                self.walk_free_vars(object, bound, seen, free);
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.walk_free_vars(value, bound, seen, free);
                self.walk_free_vars(count, bound, seen, free);
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args {
                    self.walk_free_vars(&arg.expr, bound, seen, free);
                }
                self.walk_free_vars_block(body, bound, seen, free);
            }
            ExprKind::Unsafe { body } | ExprKind::Comptime { body } => {
                self.walk_free_vars_block(body, bound, seen, free);
            }
            ExprKind::WithAs { bindings, body } => {
                for binding in bindings {
                    self.walk_free_vars(&binding.source, bound, seen, free);
                }
                self.walk_free_vars_block(body, bound, seen, free);
            }
            ExprKind::Loop { body, .. } => {
                self.walk_free_vars_block(body, bound, seen, free);
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                            self.walk_free_vars(channel, bound, seen, free);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.walk_free_vars(channel, bound, seen, free);
                            self.walk_free_vars(value, bound, seen, free);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    self.walk_free_vars(&arm.body, bound, seen, free);
                }
            }
            // Literals — no free variables
            ExprKind::Int(..) | ExprKind::Float(..) | ExprKind::String(..)
            | ExprKind::StringInterp(..) | ExprKind::Bytes(..)
            | ExprKind::Char(..) | ExprKind::Bool(..) | ExprKind::Null | ExprKind::None | ExprKind::GenericName { .. } => {}
        }
    }

    fn walk_free_vars_block(
        &self,
        stmts: &[rask_ast::stmt::Stmt],
        bound: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        free: &mut Vec<(String, LocalId, MirType, bool)>,
    ) {
        let mut local_bound = bound.clone();
        for stmt in stmts {
            self.walk_free_vars_stmt(stmt, &local_bound, seen, free);
            match &stmt.kind {
                rask_ast::stmt::StmtKind::Mut { name, .. }
                | rask_ast::stmt::StmtKind::Let { name, .. } => {
                    local_bound.insert(name.clone());
                }
                rask_ast::stmt::StmtKind::MutTuple { patterns, .. }
                | rask_ast::stmt::StmtKind::LetTuple { patterns, .. } => {
                    for n in rask_ast::stmt::tuple_pats_flat_names(patterns) { local_bound.insert(n.to_string()); }
                }
                rask_ast::stmt::StmtKind::LetStruct { pattern, .. } => {
                    for n in rask_ast::stmt::pattern_binding_names(pattern) { local_bound.insert(n); }
                }
                _ => {}
            }
        }
    }

    fn walk_free_vars_stmt(
        &self,
        stmt: &rask_ast::stmt::Stmt,
        bound: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        free: &mut Vec<(String, LocalId, MirType, bool)>,
    ) {
        use rask_ast::stmt::{ForBinding, StmtKind};
        match &stmt.kind {
            StmtKind::Expr(e) => self.walk_free_vars(e, bound, seen, free),
            StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => {
                self.walk_free_vars(init, bound, seen, free);
            }
            StmtKind::MutTuple { init, .. }
            | StmtKind::LetTuple { init, .. }
            | StmtKind::LetStruct { init, .. } => {
                self.walk_free_vars(init, bound, seen, free);
            }
            StmtKind::Return(Some(e)) => self.walk_free_vars(e, bound, seen, free),
            StmtKind::Return(None) => {}
            StmtKind::Assign { target, value, .. } => {
                self.walk_free_vars(target, bound, seen, free);
                self.walk_free_vars(value, bound, seen, free);
            }
            StmtKind::While { cond, body, .. } => {
                self.walk_free_vars(cond, bound, seen, free);
                self.walk_free_vars_block(body, bound, seen, free);
            }
            StmtKind::WhileLet { pattern, expr, body, .. } => {
                self.walk_free_vars(expr, bound, seen, free);
                let mut body_bound = bound.clone();
                collect_pattern_names(pattern, &mut body_bound);
                self.walk_free_vars_block(body, &body_bound, seen, free);
            }
            StmtKind::For { binding, iter, body, .. } => {
                self.walk_free_vars(iter, bound, seen, free);
                let mut inner_bound = bound.clone();
                match binding {
                    ForBinding::Single(name) => { inner_bound.insert(name.clone()); }
                    ForBinding::Tuple(names) => {
                        for name in names { inner_bound.insert(name.clone()); }
                    }
                }
                self.walk_free_vars_block(body, &inner_bound, seen, free);
            }
            StmtKind::Loop { body, .. } => {
                self.walk_free_vars_block(body, bound, seen, free);
            }
            StmtKind::Break { value, .. } => {
                if let Some(v) = value { self.walk_free_vars(v, bound, seen, free); }
            }
            StmtKind::Continue(_) => {}
            StmtKind::Ensure { body, else_handler } => {
                self.walk_free_vars_block(body, bound, seen, free);
                if let Some((name, handler)) = else_handler {
                    let mut inner_bound = bound.clone();
                    inner_bound.insert(name.clone());
                    self.walk_free_vars_block(handler, &inner_bound, seen, free);
                }
            }
            StmtKind::Comptime(body) => {
                self.walk_free_vars_block(body, bound, seen, free);
            }
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.walk_free_vars(iter, bound, seen, free);
                self.walk_free_vars_block(body, bound, seen, free);
            }
            StmtKind::Discard { .. } => {}
        }
    }
}

/// Collect variable names bound by a pattern into a set.
fn collect_pattern_names(
    pattern: &rask_ast::expr::Pattern,
    names: &mut std::collections::HashSet<String>,
) {
    use rask_ast::expr::Pattern;
    match pattern {
        Pattern::Ident(name) => { names.insert(name.clone()); }
        Pattern::Constructor { fields, .. } => {
            for p in fields { collect_pattern_names(p, names); }
        }
        Pattern::Struct { fields, .. } => {
            for (_, p) in fields { collect_pattern_names(p, names); }
        }
        Pattern::Tuple(elems) => {
            for p in elems { collect_pattern_names(p, names); }
        }
        Pattern::Or(alts) => {
            // All alternatives bind the same names; just collect from the first
            if let Some(first) = alts.first() { collect_pattern_names(first, names); }
        }
        Pattern::Wildcard | Pattern::Literal(_) | Pattern::Range { .. } => {}
        Pattern::TypePat { binding, .. } => {
            if let Some(name) = binding {
                names.insert(name.clone());
            }
        }
    }
}


// =================================================================
// Operator mappings
// =================================================================

/// Element type of a declared `Vec<T>` return type: `Vec<SeedSpec>` →
/// `Struct(SeedSpec)`. `None` for anything that isn't a Vec.
fn vec_elem_of_type(ret_ty: Option<&TypeExpr>, ctx: &MirContext) -> Option<MirType> {
    match ret_ty? {
        TypeExpr::Named { path, args } if path.len() == 1 && path[0] == "Vec" && args.len() == 1 => {
            Some(ctx.resolve_type_expr(&args[0]))
        }
        _ => None,
    }
}

/// Is `name` one of the integer primitives (as spelled in source)?
/// Register-width only — `string.parse<i128>` isn't supported.
pub(crate) fn is_integer_type_name(name: &str) -> bool {
    rask_ast::primitives::is_machine_integer(name)
}

/// Type arguments `string.parse<T>` accepts — the numeric primitives.
pub(crate) fn is_parse_target_type_name(name: &str) -> bool {
    is_integer_type_name(name) || matches!(name, "f32" | "f64")
}

/// The MIR operator a desugared method name stands for: `add` is `+`.
fn operator_method_to_binop(method: &str) -> Option<crate::operand::BinOp> {
    rask_ast::expr::binary_op_of_method(method).map(lower_binop)
}

/// The MIR operator a desugared unary method name stands for: `neg` is `-`.
fn operator_method_to_unaryop(method: &str) -> Option<crate::operand::UnaryOp> {
    rask_ast::expr::unary_op_of_method(method).map(lower_unaryop)
}

fn lower_binop(op: BinOp) -> crate::operand::BinOp {
    use crate::operand::BinOp as MirBinOp;
    match op {
        BinOp::Add => MirBinOp::Add,
        BinOp::Sub => MirBinOp::Sub,
        BinOp::Mul => MirBinOp::Mul,
        BinOp::Div => MirBinOp::Div,
        BinOp::Mod => MirBinOp::Mod,
        BinOp::Eq => MirBinOp::Eq,
        BinOp::Ne => MirBinOp::Ne,
        BinOp::Lt => MirBinOp::Lt,
        BinOp::Gt => MirBinOp::Gt,
        BinOp::Le => MirBinOp::Le,
        BinOp::Ge => MirBinOp::Ge,
        BinOp::And => MirBinOp::And,
        BinOp::Or => MirBinOp::Or,
        BinOp::BitAnd => MirBinOp::BitAnd,
        BinOp::BitOr => MirBinOp::BitOr,
        BinOp::BitXor => MirBinOp::BitXor,
        BinOp::Shl => MirBinOp::Shl,
        BinOp::Shr => MirBinOp::Shr,
    }
}

/// Map AST unary operator to MIR unary operator.
fn lower_unaryop(op: UnaryOp) -> crate::operand::UnaryOp {
    use crate::operand::UnaryOp as MirUnaryOp;
    match op {
        UnaryOp::Neg => MirUnaryOp::Neg,
        UnaryOp::Not => MirUnaryOp::Not,
        UnaryOp::BitNot => MirUnaryOp::BitNot,
        UnaryOp::Ref | UnaryOp::Deref | UnaryOp::Heap => unreachable!(),
    }
}

/// Check if a name is a known enum variant (not a variable binding).
pub(crate) fn is_variant_name(name: &str) -> bool {
    matches!(name, "Some" | "None" | "Ok" | "Err")
        || name.contains('.')  // Qualified variant like "Status.Active"
        || name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
}

/// Detect identifiers that name types or stdlib modules rather than values.
///
/// Uppercase-initial names are user-defined types (structs, enums, interfaces).
/// Lowercase names are checked against stdlib stub registrations.
fn is_type_constructor_name(name: &str) -> bool {
    name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
        || rask_stdlib::mir_metadata::stdlib_type_names().contains(name)
        || rask_stdlib::mir_metadata::stdlib_module_names().contains(name)
}

/// Map a qualified function name to the stdlib type prefix of its return value.
///
/// When the type checker can't resolve concrete types (leaves `Var(TypeVarId(...))`),
/// the MIR lowerer uses this to track which type a local holds, so later
/// method calls get correctly qualified (e.g. `rng.range()` → `Rng_range`).
///
/// Derived from stdlib stub files. SIMD scalar returns are handled as a
/// fallback since stubs don't distinguish vector vs scalar return methods.
fn func_return_type_prefix(func_name: &str) -> Option<&str> {
    // SIMD: non-scalar-returning methods keep their type prefix
    if is_simd_prefix(func_name) && !is_scalar_return(func_name) {
        return func_name.split('_').next();
    }

    // Look up from stub-derived metadata
    if let Some(meta) = rask_stdlib::mir_metadata::lookup(func_name) {
        return meta.ret_type_prefix.as_deref();
    }

    None
}

fn is_simd_prefix(name: &str) -> bool {
    name.starts_with("f32x4_") || name.starts_with("f32x8_")
        || name.starts_with("f64x2_") || name.starts_with("f64x4_")
        || name.starts_with("i32x4_") || name.starts_with("i32x8_")
}

/// SIMD methods that return a scalar, not a vector.
fn is_scalar_return(func_name: &str) -> bool {
    func_name.ends_with("_sum")
        || func_name.ends_with("_product")
        || func_name.ends_with("_min")
        || func_name.ends_with("_max")
        || func_name.ends_with("_get")
        || func_name.ends_with("_store")
        || func_name.ends_with("_set")
}

/// The type parameter this type still names, if any.
///
/// A monomorphized body must not contain one. Mono substitutes every record it
/// copies into an instance and drops the record when it can't, so a parameter
/// that survives to here means a substitution went missing — and it has to stay
/// tellable apart from a real type, because `T` maps to `Ptr` or to `i64`
/// depending on how it is spelled and both read back as a plausible 8-byte
/// scalar. That is how a `string` element ended up in an 8-byte slot with no
/// refcount on it (#1020).
///
/// Same rule mono binds parameters by — one uppercase letter — so the two
/// agree on what counts as one.
pub(crate) fn type_names_a_parameter(ty: &Type) -> Option<String> {
    fn is_param(name: &str) -> bool {
        let mut chars = name.chars();
        matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
    }
    fn arg_ty(arg: &rask_types::GenericArg) -> Option<&Type> {
        match arg {
            rask_types::GenericArg::Type(t) => Some(t),
            _ => None,
        }
    }
    match ty {
        Type::UnresolvedNamed(name) if is_param(name) => Some(name.clone()),
        Type::UnresolvedGeneric { name, args } => {
            if is_param(name) {
                return Some(name.clone());
            }
            args.iter().filter_map(arg_ty).find_map(type_names_a_parameter)
        }
        Type::Generic { args, .. } => {
            args.iter().filter_map(arg_ty).find_map(type_names_a_parameter)
        }
        Type::Tuple(elems) | Type::Union(elems) => elems.iter().find_map(type_names_a_parameter),
        Type::Array { elem, .. } => type_names_a_parameter(elem),
        Type::RawPtr(inner) => type_names_a_parameter(inner),
        Type::Result { ok, err } => {
            type_names_a_parameter(ok).or_else(|| type_names_a_parameter(err))
        }
        Type::Fn { params, ret } => params
            .iter()
            .find_map(|p| type_names_a_parameter(&p.ty))
            .or_else(|| type_names_a_parameter(ret)),
        _ => None,
    }
}

/// Return type for a known stdlib function, resolving a named error type
/// against the program's layouts.
///
/// A stub's `T or E` used to lose `E` outright — the metadata parser wrote I64
/// into the error slot no matter what was declared. That gave the Result an
/// 8-byte payload where an error enum needs its own size, and left the match on
/// it with no enum to switch on: `JoinError.Panicked(m)` read the payload as an
/// address (#677). With a context in hand the declared name resolves to its
/// real layout.
fn stdlib_return_mir_type_in(func_name: &str, ctx: Option<&MirContext>) -> MirType {
    stdlib_return_mir_type_known(func_name, ctx).unwrap_or(MirType::I64)
}

/// The same answer, but `None` where the old code shrugged and said i64.
///
/// Telling "the stubs declare this" apart from "nobody here knows" is what lets
/// a caller ask the checker before it guesses. `s.clone()` is the case that
/// forced it: no stub declares `string_clone`, so the chain ran off the end and
/// typed the destination i64 — half a string, no refcount, and a pointer
/// printed as a number.
fn stdlib_return_mir_type_known(func_name: &str, ctx: Option<&MirContext>) -> Option<MirType> {
    // Try stub-derived metadata first
    if let Some(meta) = rask_stdlib::mir_metadata::lookup(func_name) {
        // Unless what it says is `T`. `Vec.remove` is declared `-> T` and the
        // stub has no idea what the caller instantiated — recording that as i64
        // is where every "eight bytes of a sixteen-byte string" in this area
        // came from. Let the caller ask the checker instead (#1020).
        if meta.ret_category.names_a_type_param() {
            return None;
        }
        // The stub's own words, parsed by the one parser that knows all the
        // shapes. `RetCategory`'s coarser reading of the same string used to
        // answer here, and it collapsed `f32` into `f64` and every named stdlib
        // type into `i64` — including the ones that aren't word-sized runtime
        // handles, which is what `StringView` needed a hard-coded exception for
        // (#1025).
        let Some(ctx) = ctx else { return None };
        return Some(ctx.resolve_type_expr(&meta.ret_ty));
    }

    // SIMD float reductions return F64
    if is_scalar_return(func_name) && !func_name.ends_with("_store") && !func_name.ends_with("_set") {
        if func_name.starts_with("f32x") || func_name.starts_with("f64x") {
            return Some(MirType::F64);
        }
    }

    // No suffix guessing. There used to be a block here that answered from the
    // *end* of the mangled name — `_to_string`, `_trim`, `_replace`,
    // `_is_empty`, `_contains` and friends — for "user type methods and methods
    // not yet in stubs". It was blind to the receiver, so it answered for any
    // type whose method happened to share a name with a string's.
    //
    // That is how `Shared<i32>.replace(7)` came back as a `string`: the strategy
    // rewrite turns it into `Cell_replace`, which ends in `_replace`. The box
    // arm in lower/expr.rs existed to shadow this, which is why it looked
    // redundant and wasn't.
    //
    // Instrumented across the 362 suite files and every example, the block fired
    // zero times — a user type's methods are in `func_sigs` from their own
    // declarations, which is consulted first, and the stdlib's are in the stub
    // metadata above. Its only live effect was the wrong answer.
    //
    // main later added `char_is_*` and `char_eq` to that block for the Unicode
    // work. Those come from `stdlib/char.rk` like any other declared method, so
    // deleting the block doesn't take them with it — the char suite covers it.

    None
}

/// True if this method writes through `self`.
///
/// `mutate self` and `take self` are visible right here on the parameter. An
/// *inferred* mode is not: type.gradual/GC9 lets a private method omit it and
/// have the compiler decide from the body, and that decision happens in the type
/// checker. It records the spans it decided for, so this reads the answer rather
/// than walking the body again — one implementation of GC9, not two that drift.
/// Parameter positions declared `take`, `self` excluded and not counted.
fn take_positions(params: &[rask_ast::decl::Param]) -> Vec<usize> {
    params
        .iter()
        .filter(|p| p.name != "self")
        .enumerate()
        .filter(|(_, p)| p.is_take)
        .map(|(i, _)| i)
        .collect()
}

fn method_mutates_self(f: &rask_ast::decl::FnDecl, ctx: &MirContext) -> bool {
    let Some(p) = f.params.first() else { return false };
    if p.name != "self" {
        return false;
    }
    // Written in the signature — no need to ask anyone.
    if p.is_mutate || p.is_take {
        return true;
    }
    match ctx.mutate_self_fns {
        Some(set) => set.contains(&(f.span.start, f.span.end, f.span.file_id)),
        // Same test for a synthetic unit the rest of lowering uses: no node
        // types means nobody ran the checker, so there was no GC9 decision to
        // record and "doesn't mutate" is the honest answer.
        None if ctx.node_types.is_empty() => false,
        None => panic!(
            "lowering `{}` needs the GC9 self-mode decision but MirContext was \
             built without `mutate_self_fns`. Only the checker can answer this — \
             pass `Some(&typed.mutate_self_fns)`.",
            f.name
        ),
    }
}

/// Do these two spellings name the same nominal type?
///
/// A pattern carries the type arguments the source wrote — `Refused<i64>` — and
/// a layout's name is the bare one, plus a `$` suffix once monomorphization has
/// been through it. So the comparison is on what comes before either.
fn same_nominal(a: &str, b: &str) -> bool {
    fn base(n: &str) -> &str {
        let n = n.trim();
        n.split('$').next().unwrap_or(n).trim()
    }
    base(a) == base(b)
}

/// The method-name prefix for a receiver with no nominal name of its own.
///
/// The checker records `Callee::Method { recv }` for every non-inference
/// receiver, these included, but `type_prefix` answers `None` for them — so
/// `x.floor()` skipped the recorded answer and re-derived the same prefix from
/// its MIR type further down the chain. This is the one mapping both ends use,
/// so they can't disagree about it.
///
/// Narrow integer widths mangle to their widest sibling: codegen has `i64_*` /
/// `u64_*` entries and narrower values ride in the same slots. That has to
/// become per-width when `std.bits` lands — `(0 as i32).count_zeros()` is 32,
/// not 64, so those methods can't share one symbol.
///
/// 128-bit has its own prefix. It used to ride the 64-bit symbols, which was
/// defensible while nothing could build a value wider than 64 bits — that stops
/// being true once arithmetic is real, and `abs` on a value past the boundary
/// truncated (#762, #794). Only `abs` and `to_string` need an `i128_*` symbol
/// today; those are the two methods the checker accepts at that width.
pub fn builtin_method_prefix(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::F32 | Type::F64 => Some("f64"),
        Type::Bool => Some("bool"),
        Type::Char => Some("char"),
        Type::I8 | Type::I16 | Type::I32 | Type::I64 => Some("i64"),
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => Some("u64"),
        // 128-bit dispatches on its own prefix. It used to ride the 64-bit
        // symbols, which was defensible only while no value needed more than 64
        // bits — `abs` on an `i128` truncated the moment that stopped being
        // true (#762, #794).
        Type::I128 => Some("i128"),
        Type::U128 => Some("u128"),
        // A slice dispatches like the container it came from: `parts[2..]
        // .join(" ")` is `Vec_join`. Without this the call fell through to the
        // name-policy table, which guesses "a two-argument `join` means Vec" —
        // right here, and only by luck.
        Type::Array { .. } => Some("Vec"),
        _ => None,
    }
}

/// The dispatch prefix for a builtin type named in source, which is the spelling
/// a folded comptime const carries in `ComptimeGlobalMeta.type_prefix`.
///
/// Goes through `builtin_method_prefix` so the width collapse — narrow integers
/// dispatch on their widest sibling — is stated in one place. A folded const used
/// its own type name as the prefix directly, so `const B: u32 = comptime { … }`
/// interpolated into a string emitted `u32_to_string`, which no backend has
/// (#824).
pub fn builtin_method_prefix_for_name(name: &str) -> Option<&'static str> {
    let ty = match name {
        "i8" => Type::I8,
        "i16" => Type::I16,
        "i32" => Type::I32,
        "i64" => Type::I64,
        "i128" => Type::I128,
        "u8" => Type::U8,
        "u16" => Type::U16,
        "u32" => Type::U32,
        "u64" => Type::U64,
        "u128" => Type::U128,
        "f32" => Type::F32,
        "f64" => Type::F64,
        "bool" => Type::Bool,
        "char" => Type::Char,
        // A string receiver never reaches `builtin_method_prefix` — it comes out
        // of `stdlib_type_prefix` instead — but it is spelled the same either way.
        "string" => return Some("string"),
        "bytes" => return Some("bytes"),
        _ => return None,
    };
    builtin_method_prefix(&ty)
}


/// Extract type prefix from a type annotation string.
///
/// Handles generic types like "Vec<i64>" → "Vec", "Map<K,V>" → "Map",
/// plain named types like "Handle" → "Handle",
/// and module-qualified types like "time.Instant" → "Instant".
/// Returns None for primitives (i64, f64, bool, string, etc.).
pub fn type_prefix_from_str(s: &str) -> Option<String> {
    let s = s.trim();
    // Strip module prefix (time.Instant → Instant)
    let base = s.rsplit('.').next().unwrap_or(s);
    // Strip generic args (Vec<i64> → Vec)
    let name = base.trim();
    // Reject primitives and empty
    if name.is_empty() { return None; }
    match name {
        _ if rask_ast::primitives::is_builtin_value_type(name) || name == "()" => None,
        _ if name.chars().next().map_or(false, |c| c.is_uppercase()) => {
            Some(name.to_string())
        }
        // Module-level functions like "time" — not a type prefix
        _ => None,
    }
}

/// `type_prefix_from_str` for a written type: the name methods are filed
/// under. `time.Instant` → `Instant`, `Vec<i64>` → `Vec`; `None` for a
/// primitive.
pub fn type_prefix_of(ty: &TypeExpr) -> Option<String> {
    let name = ty.last_segment()?;
    if rask_ast::primitives::is_builtin_value_type(name) {
        return None;
    }
    name.starts_with(char::is_uppercase).then(|| name.to_string())
}

/// The name a type pattern tests against: `none`, `MyErr`, `MyErr.Worse`,
/// `Refused` for `Refused<i64>` — the layouts it is compared with carry no
/// arguments.
pub(crate) fn type_pat_name(ty: &TypeExpr) -> String {
    match ty {
        TypeExpr::NoneType => "none".to_string(),
        TypeExpr::Named { path, .. } => path.join("."),
        other => other.to_string(),
    }
}

/// Determine result type for a binary operation.
/// Comparison ops return Bool, arithmetic returns the operand type.
fn binop_result_type(op: &crate::operand::BinOp, operand_ty: &MirType) -> MirType {
    use crate::operand::BinOp as B;
    match op {
        B::Eq | B::Ne | B::Lt | B::Gt | B::Le | B::Ge | B::And | B::Or => MirType::Bool,
        _ => operand_ty.clone(),
    }
}

/// What it takes to put an element binding back where it came from.
#[derive(Debug, Clone)]
pub(crate) struct PendingWriteBack {
    /// The Vec or Map the element was read out of.
    pub(crate) collection: MirOperand,
    /// The index for a Vec, the key for a Map.
    pub(crate) at: MirOperand,
    /// The binding holding the element, or a Map entry's value. Read out with
    /// `Vec_lend`/`Map_lend`, so the frame owns it until the write-back.
    pub(crate) value: LocalId,
    /// A Map writes back by key.
    pub(crate) map: bool,
}

/// Something owed to a `mutate` argument once its call has been emitted.
pub(crate) enum ElemWriteback {
    /// An element borrowed out of a collection, waiting to be released.
    ReleaseBorrow {
        /// The collection the element was borrowed from.
        collection: MirOperand,
        /// Which release to call — Vec and Map keep separate borrow counts.
        release: &'static str,
    },
    /// A scalar variable handed over as `mutate`. Codegen keeps scalars in
    /// registers, so the address the callee wrote through is a spilled copy —
    /// read it back into the variable or the write is lost (#899).
    ScalarCopyBack {
        /// The caller's variable.
        dst: LocalId,
        /// The spill slot the callee wrote through.
        addr: LocalId,
    },
}

/// When a raw-pointer access checks for null (mem.unsafe, debug-mode safety).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PointerChecks {
    /// Checked in a debug build, not in release.
    DebugOnly,
    /// `@checked_unsafe`: checked in every build.
    Always,
    /// `@unchecked_unsafe`: never checked.
    Never,
}

impl PointerChecks {
    fn of(attrs: &[rask_ast::attr::Attr]) -> PointerChecks {
        if attrs.iter().any(|a| a.is("unchecked_unsafe")) {
            PointerChecks::Never
        } else if attrs.iter().any(|a| a.is("checked_unsafe")) {
            PointerChecks::Always
        } else {
            PointerChecks::DebugOnly
        }
    }
}

#[derive(Debug)]
pub enum LoweringError {
    UnresolvedVariable(String),
    UnresolvedGeneric(String),
    InvalidConstruct(String),
    /// Lowering couldn't work out a type and would have guessed i64. Fatal on
    /// purpose: the guess is only ever right for a payload that already fits a
    /// machine word, so silently taking it turns a missing feature into a
    /// miscompile — an f64 loses its value, a string or a struct becomes an
    /// address. Carries the sites that gave up so the report names them.
    UnknownType(Vec<(&'static str, u32)>),
}

impl std::fmt::Display for LoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoweringError::UnresolvedVariable(name) => write!(f, "unresolved variable `{name}`"),
            LoweringError::UnresolvedGeneric(name) => write!(f, "unresolved generic `{name}`"),
            LoweringError::InvalidConstruct(what) => write!(f, "{what}"),
            LoweringError::UnknownType(sites) => {
                let where_ = sites.iter().map(|(s, _)| *s).collect::<Vec<_>>().join(", ");
                write!(
                    f,
                    "couldn't work out a type here, so there's nothing safe to \
                     compile — a guess is only ever right for a value that fits a \
                     machine word, and would silently corrupt an f64, a string or \
                     a struct.\n         \
                     A type annotation on the binding usually settles it \
                     (`const xs: Vec<i64> = …`).\n         \
                     Gave up in: {where_}"
                )
            }
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{operand::MirConst, MirRValue, MirStmt};
    use rask_ast::decl::{Decl, DeclKind, FnDecl, Param};
    use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind, MatchArm, Pattern};
    use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
    use rask_ast::{NodeId, Span};

    // ── AST construction helpers ────────────────────────────────

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn int_expr(val: i128) -> Expr {
        Expr { id: NodeId(100), kind: ExprKind::Int(val, None), span: sp() }
    }

    fn float_expr(val: f64) -> Expr {
        Expr { id: NodeId(101), kind: ExprKind::Float(val, None), span: sp() }
    }

    fn string_expr(s: &str) -> Expr {
        Expr { id: NodeId(102), kind: ExprKind::String(s.to_string()), span: sp() }
    }

    fn bool_expr(val: bool) -> Expr {
        Expr { id: NodeId(103), kind: ExprKind::Bool(val), span: sp() }
    }

    fn ident_expr(name: &str) -> Expr {
        Expr { id: NodeId(105), kind: ExprKind::Ident(name.to_string()), span: sp() }
    }

    fn call_expr(func: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(106),
            kind: ExprKind::Call {
                func: Box::new(ident_expr(func)),
                args: args.into_iter().map(|expr| CallArg { name: None, mode: ArgMode::Default, expr }).collect(),
            },
            span: sp(),
        }
    }

    fn binary_expr(op: BinOp, left: Expr, right: Expr) -> Expr {
        Expr {
            id: NodeId(107),
            kind: ExprKind::Binary {
                op,
                left: Box::new(left),
                right: Box::new(right),
            },
            span: sp(),
        }
    }

    fn unary_expr(op: UnaryOp, operand: Expr) -> Expr {
        Expr {
            id: NodeId(108),
            kind: ExprKind::Unary {
                op,
                operand: Box::new(operand),
            },
            span: sp(),
        }
    }

    fn method_call_expr(obj: Expr, method: &str, args: Vec<Expr>) -> Expr {
        Expr {
            id: NodeId(109),
            kind: ExprKind::MethodCall {
                object: Box::new(obj),
                method: method.to_string(),
                type_args: None,
                args: args.into_iter().map(|expr| CallArg { name: None, mode: ArgMode::Default, expr }).collect(),
            },
            span: sp(),
        }
    }

    fn if_expr(cond: Expr, then_br: Expr, else_br: Option<Expr>) -> Expr {
        Expr {
            id: NodeId(110),
            kind: ExprKind::If {
                cond: Box::new(cond),
                then_branch: Box::new(then_br),
                else_branch: else_br.map(Box::new),
                else_binding: None,
            },
            span: sp(),
        }
    }

    fn match_expr(scrutinee: Expr, arms: Vec<MatchArm>) -> Expr {
        Expr {
            id: NodeId(111),
            kind: ExprKind::Match {
                scrutinee: Box::new(scrutinee),
                arms,
            },
            span: sp(),
        }
    }

    fn try_expr(inner: Expr) -> Expr {
        Expr {
            id: NodeId(112),
            kind: ExprKind::Try { expr: Box::new(inner) },
            span: sp(),
        }
    }

    fn return_stmt(val: Option<Expr>) -> Stmt {
        Stmt { id: NodeId(200), kind: StmtKind::Return(val), span: sp() }
    }

    fn let_stmt(name: &str, ty: Option<&str>, init: Expr) -> Stmt {
        Stmt {
            id: NodeId(201),
            kind: StmtKind::Mut {
                name: name.to_string(),
                name_span: sp(),
                ty: ty.and_then(rask_parser::parse_type),
                init,
            },
            span: sp(),
        }
    }

    fn const_stmt(name: &str, ty: Option<&str>, init: Expr) -> Stmt {
        Stmt {
            id: NodeId(202),
            kind: StmtKind::Let {
                name: name.to_string(),
                name_span: sp(),
                ty: ty.and_then(rask_parser::parse_type),
                init,
            },
            span: sp(),
        }
    }

    fn expr_stmt(e: Expr) -> Stmt {
        Stmt { id: NodeId(203), kind: StmtKind::Expr(e), span: sp() }
    }

    fn while_stmt(cond: Expr, body: Vec<Stmt>) -> Stmt {
        Stmt {
            id: NodeId(204),
            kind: StmtKind::While { label: None, cond, body },
            span: sp(),
        }
    }

    fn loop_stmt(label: Option<&str>, body: Vec<Stmt>) -> Stmt {
        Stmt {
            id: NodeId(205),
            kind: StmtKind::Loop {
                label: label.map(|s| s.to_string()),
                body,
            },
            span: sp(),
        }
    }

    fn for_stmt(binding: &str, iter: Expr, body: Vec<Stmt>) -> Stmt {
        Stmt {
            id: NodeId(206),
            kind: StmtKind::For {
                label: None,
                binding: ForBinding::Single(binding.to_string()),
                mutate: false,
                iter,
                body,
            },
            span: sp(),
        }
    }

    fn break_stmt(label: Option<&str>, value: Option<Expr>) -> Stmt {
        Stmt {
            id: NodeId(207),
            kind: StmtKind::Break {
                label: label.map(|s| s.to_string()),
                value,
            },
            span: sp(),
        }
    }

    fn continue_stmt(label: Option<&str>) -> Stmt {
        Stmt {
            id: NodeId(208),
            kind: StmtKind::Continue(label.map(|s| s.to_string())),
            span: sp(),
        }
    }

    fn ensure_stmt(body: Vec<Stmt>, handler: Option<(&str, Vec<Stmt>)>) -> Stmt {
        Stmt {
            id: NodeId(209),
            kind: StmtKind::Ensure {
                body,
                else_handler: handler.map(|(n, s)| (n.to_string(), s)),
            },
            span: sp(),
        }
    }

    fn assign_stmt(target: Expr, value: Expr) -> Stmt {
        Stmt {
            id: NodeId(210),
            kind: StmtKind::Assign { target, value, op: None },
            span: sp(),
        }
    }

    fn make_fn(name: &str, params: Vec<(&str, &str)>, ret_ty: Option<&str>, body: Vec<Stmt>) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                type_params: vec![],
                params: params
                    .into_iter()
                    .map(|(n, ty)| Param {
                        name: n.to_string(),
                        name_span: sp(),
                        ty: rask_parser::parse_type(ty),
                        is_take: false,
                        is_mutate: false, is_deleting: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.and_then(rask_parser::parse_type),
                body,
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
                decl_start: sp().start,
            }),
            span: sp(),
        }
    }

    fn lower(decl: &Decl, all_decls: &[Decl]) -> MirFunction {
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let mut fns = MirLowerer::lower_function(decl, all_decls, &ctx).expect("lowering failed");
        fns.remove(0) // Return the main function (first element)
    }

    fn lower_one(decl: &Decl) -> MirFunction {
        lower(decl, &[decl.clone()])
    }

    // ── helpers for inspecting MIR ──────────────────────────────

    fn has_return(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator.kind, MirTerminatorKind::Return { .. }))
    }

    fn has_branch(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator.kind, MirTerminatorKind::Branch { .. }))
    }

    fn has_switch(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator.kind, MirTerminatorKind::Switch { .. }))
    }

    fn has_goto(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator.kind, MirTerminatorKind::Goto { .. }))
    }

    fn count_blocks(f: &MirFunction) -> usize {
        f.blocks.len()
    }

    fn find_call(f: &MirFunction, func_name: &str) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(&s.kind, MirStmtKind::Call { func, .. } if func.name == func_name))
        })
    }

    fn find_assign_binop(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s.kind, MirStmtKind::Assign { rvalue: MirRValue::BinaryOp { .. }, .. }))
        })
    }

    fn find_assign_unaryop(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s.kind, MirStmtKind::Assign { rvalue: MirRValue::UnaryOp { .. }, .. }))
        })
    }

    fn find_ensure_push(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s.kind, MirStmtKind::EnsurePush { .. }))
        })
    }

    fn find_ensure_pop(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s.kind, MirStmtKind::EnsurePop))
        })
    }

    fn find_cleanup_return(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| matches!(b.terminator.kind, MirTerminatorKind::CleanupReturn { .. }))
    }

    fn find_enum_tag(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s.kind, MirStmtKind::Assign { rvalue: MirRValue::EnumTag { .. }, .. }))
        })
    }

    // ═══════════════════════════════════════════════════════════
    // Type parameters that shouldn't have got this far
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn spots_a_type_parameter_the_substitution_missed() {
        use rask_types::{GenericArg, Type};
        let t = || Type::UnresolvedNamed("T".to_string());

        assert_eq!(type_names_a_parameter(&t()), Some("T".to_string()));
        assert_eq!(
            type_names_a_parameter(&Type::Result {
                ok: Box::new(t()),
                err: Box::new(Type::None),
            }),
            Some("T".to_string())
        );
        assert_eq!(
            type_names_a_parameter(&Type::UnresolvedGeneric {
                name: "Vec".to_string(),
                args: vec![GenericArg::Type(Box::new(t()))],
            }),
            Some("T".to_string())
        );
        assert_eq!(
            type_names_a_parameter(&Type::Tuple(vec![Type::I64, t()])),
            Some("T".to_string())
        );
    }

    #[test]
    fn a_real_type_is_not_a_parameter() {
        use rask_types::{GenericArg, Type};

        assert_eq!(type_names_a_parameter(&Type::String), None);
        assert_eq!(type_names_a_parameter(&Type::I64), None);
        // Two letters, so it names something — `Ok` is a type, `T` is a hole.
        assert_eq!(
            type_names_a_parameter(&Type::UnresolvedNamed("Ok".to_string())),
            None
        );
        assert_eq!(
            type_names_a_parameter(&Type::UnresolvedGeneric {
                name: "Vec".to_string(),
                args: vec![GenericArg::Type(Box::new(Type::String))],
            }),
            None
        );
    }

    // ═══════════════════════════════════════════════════════════
    // Literals
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_integer_literal() {
        let decl = make_fn("f", vec![], Some("i64"), vec![return_stmt(Some(int_expr(42)))]);
        let f = lower_one(&decl);
        let ret_block = f.blocks.iter().find(|b| matches!(b.terminator.kind, MirTerminatorKind::Return { .. })).unwrap();
        if let MirTerminatorKind::Return { value: Some(MirOperand::Constant(MirConst::Int(42))) } = &ret_block.terminator.kind {
            // good
        } else {
            panic!("Expected return 42, got: {:?}", ret_block.terminator);
        }
    }

    #[test]
    fn lower_string_literal() {
        let decl = make_fn("f", vec![], Some("string"), vec![return_stmt(Some(string_expr("hello")))]);
        let f = lower_one(&decl);
        assert_eq!(f.ret_ty, MirType::Buffer);
    }

    #[test]
    fn lower_bool_literal() {
        let decl = make_fn("f", vec![], Some("bool"), vec![return_stmt(Some(bool_expr(true)))]);
        let f = lower_one(&decl);
        let ret_block = f.blocks.iter().find(|b| matches!(b.terminator.kind, MirTerminatorKind::Return { .. })).unwrap();
        if let MirTerminatorKind::Return { value: Some(MirOperand::Constant(MirConst::Bool(true))) } = &ret_block.terminator.kind {
            // good
        } else {
            panic!("Expected return true, got: {:?}", ret_block.terminator);
        }
    }

    // ═══════════════════════════════════════════════════════════
    // Variables and bindings
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_variable_reference() {
        let decl = make_fn("f", vec![], Some("i32"), vec![
            const_stmt("x", Some("i32"), int_expr(42)),
            return_stmt(Some(ident_expr("x"))),
        ]);
        let f = lower_one(&decl);
        assert!(f.locals.iter().any(|l| l.name.as_deref() == Some("x")));
    }

    #[test]
    fn lower_unresolved_variable_errors() {
        let decl = make_fn("f", vec![], None, vec![return_stmt(Some(ident_expr("no_such_var")))]);
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let result = MirLowerer::lower_function(&decl, &[decl.clone()], &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn lower_let_binding() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", Some("i32"), int_expr(10)),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        let x_local = f.locals.iter().find(|l| l.name.as_deref() == Some("x"));
        assert!(x_local.is_some());
        assert_eq!(x_local.unwrap().ty, MirType::I32);
    }

    #[test]
    fn lower_let_infers_type() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", None, int_expr(42)),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        let x_local = f.locals.iter().find(|l| l.name.as_deref() == Some("x")).unwrap();
        assert_eq!(x_local.ty, MirType::I64);
    }

    #[test]
    fn lower_assignment() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", Some("i32"), int_expr(0)),
            assign_stmt(ident_expr("x"), int_expr(42)),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        let assign_count = f.blocks.iter()
            .flat_map(|b| b.statements.iter())
            .filter(|s| matches!(s.kind, MirStmtKind::Assign { .. }))
            .count();
        assert!(assign_count >= 2);
    }

    // ═══════════════════════════════════════════════════════════
    // Operators
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_binary_op_and_or() {
        // Short-circuit: `&&`/`||` lower to a branch + per-arm assigns,
        // not a single BinaryOp. Verify the branch terminator is emitted.
        let decl = make_fn("f", vec![], Some("bool"), vec![
            return_stmt(Some(binary_expr(BinOp::And, bool_expr(true), bool_expr(false)))),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
    }

    #[test]
    fn lower_desugared_add_method() {
        let decl = make_fn("f", vec![("a", "i32"), ("b", "i32")], Some("i32"), vec![
            return_stmt(Some(method_call_expr(ident_expr("a"), "add", vec![ident_expr("b")]))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_binop(&f));
    }

    #[test]
    fn lower_desugared_neg_method() {
        let decl = make_fn("f", vec![("a", "i32")], Some("i32"), vec![
            return_stmt(Some(method_call_expr(ident_expr("a"), "neg", vec![]))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_unaryop(&f));
    }

    #[test]
    fn lower_unary_not() {
        let decl = make_fn("f", vec![], Some("bool"), vec![
            return_stmt(Some(unary_expr(UnaryOp::Not, bool_expr(true)))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_unaryop(&f));
    }

    // ═══════════════════════════════════════════════════════════
    // Function calls
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_function_call() {
        let callee = make_fn("greet", vec![], None, vec![return_stmt(None)]);
        let decl = make_fn("main", vec![], None, vec![
            expr_stmt(call_expr("greet", vec![])),
            return_stmt(None),
        ]);
        let f = lower(&decl, &[decl.clone(), callee]);
        assert!(find_call(&f, "greet"));
    }

    #[test]
    fn lower_call_with_args() {
        let add = make_fn("add", vec![("a", "i32"), ("b", "i32")], Some("i32"), vec![
            return_stmt(Some(int_expr(0))),
        ]);
        let decl = make_fn("main", vec![], Some("i32"), vec![
            return_stmt(Some(call_expr("add", vec![int_expr(1), int_expr(2)]))),
        ]);
        let f = lower(&decl, &[decl.clone(), add]);
        assert!(find_call(&f, "add"));
    }

    // ═══════════════════════════════════════════════════════════
    // Function metadata
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_function_params() {
        let decl = make_fn("add", vec![("a", "i32"), ("b", "i32")], Some("i32"), vec![
            return_stmt(Some(int_expr(0))),
        ]);
        let f = lower_one(&decl);
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name.as_deref(), Some("a"));
        assert_eq!(f.params[0].ty, MirType::I32);
        assert_eq!(f.params[1].name.as_deref(), Some("b"));
        assert!(f.params[0].is_param);
        assert!(f.params[1].is_param);
    }

    #[test]
    fn lower_function_name_and_ret_ty() {
        let decl = make_fn("compute", vec![], Some("f64"), vec![return_stmt(Some(float_expr(0.0)))]);
        let f = lower_one(&decl);
        assert_eq!(f.name, "compute");
        assert_eq!(f.ret_ty, MirType::F64);
    }

    #[test]
    fn lower_void_return() {
        let decl = make_fn("f", vec![], None, vec![return_stmt(None)]);
        let f = lower_one(&decl);
        let ret = f.blocks.iter().find(|b| matches!(b.terminator.kind, MirTerminatorKind::Return { .. })).unwrap();
        assert!(matches!(ret.terminator.kind, MirTerminatorKind::Return { value: None }));
    }

    // ═══════════════════════════════════════════════════════════
    // parse_type_str
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn resolve_type_name_coverage() {
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        assert_eq!(ctx.resolve_type_name("i8"), MirType::I8);
        assert_eq!(ctx.resolve_type_name("i16"), MirType::I16);
        assert_eq!(ctx.resolve_type_name("i32"), MirType::I32);
        assert_eq!(ctx.resolve_type_name("i64"), MirType::I64);
        assert_eq!(ctx.resolve_type_name("u8"), MirType::U8);
        assert_eq!(ctx.resolve_type_name("u16"), MirType::U16);
        assert_eq!(ctx.resolve_type_name("u32"), MirType::U32);
        assert_eq!(ctx.resolve_type_name("u64"), MirType::U64);
        assert_eq!(ctx.resolve_type_name("f32"), MirType::F32);
        assert_eq!(ctx.resolve_type_name("f64"), MirType::F64);
        assert_eq!(ctx.resolve_type_name("bool"), MirType::Bool);
        assert_eq!(ctx.resolve_type_name("char"), MirType::Char);
        assert_eq!(ctx.resolve_type_name("string"), MirType::Buffer);
        assert_eq!(ctx.resolve_type_name("SomeStruct"), MirType::Ptr);
    }

    // ═══════════════════════════════════════════════════════════
    // Control flow
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_if_creates_branch() {
        let decl = make_fn("f", vec![], Some("i64"), vec![
            return_stmt(Some(if_expr(bool_expr(true), int_expr(1), Some(int_expr(2))))),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
        assert!(count_blocks(&f) >= 4);
    }

    #[test]
    fn lower_if_without_else() {
        let decl = make_fn("f", vec![], None, vec![
            return_stmt(Some(if_expr(bool_expr(true), int_expr(1), None))),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
    }

    #[test]
    fn lower_match_creates_switch() {
        let decl = make_fn("f", vec![("x", "i32")], Some("i64"), vec![
            return_stmt(Some(match_expr(
                ident_expr("x"),
                vec![
                    MatchArm { pattern: Pattern::Literal(Box::new(int_expr(1))), guard: None, body: Box::new(int_expr(10)) },
                    MatchArm { pattern: Pattern::Literal(Box::new(int_expr(2))), guard: None, body: Box::new(int_expr(20)) },
                ],
            ))),
        ]);
        let f = lower_one(&decl);
        assert!(has_switch(&f));
    }

    #[test]
    fn lower_while_loop_cfg() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", Some("i32"), int_expr(10)),
            while_stmt(
                binary_expr(BinOp::Gt, ident_expr("x"), int_expr(0)),
                vec![assign_stmt(ident_expr("x"), int_expr(0))],
            ),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
        assert!(has_goto(&f));
        assert!(count_blocks(&f) >= 4);
    }

    #[test]
    fn lower_for_loop() {
        let range = Expr {
            id: NodeId(300),
            kind: ExprKind::Range {
                start: Some(Box::new(int_expr(0))),
                end: Some(Box::new(int_expr(10))),
                inclusive: false,
            },
            span: sp(),
        };
        let decl = make_fn("f", vec![], None, vec![
            for_stmt("i", range, vec![expr_stmt(call_expr("process", vec![ident_expr("i")]))]),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
        // Range for-loops desugar to counter-based while (no "next" call)
        assert!(find_call(&f, "process"));
    }

    #[test]
    fn lower_infinite_loop() {
        let decl = make_fn("f", vec![], None, vec![
            loop_stmt(None, vec![break_stmt(None, None)]),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(has_goto(&f));
        assert!(has_return(&f));
    }

    #[test]
    fn lower_continue() {
        let decl = make_fn("f", vec![], None, vec![
            let_stmt("x", Some("i32"), int_expr(0)),
            while_stmt(
                binary_expr(BinOp::Lt, ident_expr("x"), int_expr(10)),
                vec![continue_stmt(None)],
            ),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        let goto_count = f.blocks.iter()
            .filter(|b| matches!(b.terminator.kind, MirTerminatorKind::Goto { .. }))
            .count();
        assert!(goto_count >= 2);
    }

    #[test]
    fn lower_break_outside_loop_errors() {
        let decl = make_fn("f", vec![], None, vec![break_stmt(None, None)]);
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let result = MirLowerer::lower_function(&decl, &[decl.clone()], &ctx);
        assert!(result.is_err());
    }

    #[test]
    fn lower_continue_outside_loop_errors() {
        let decl = make_fn("f", vec![], None, vec![continue_stmt(None)]);
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let result = MirLowerer::lower_function(&decl, &[decl.clone()], &ctx);
        assert!(result.is_err());
    }

    // ═══════════════════════════════════════════════════════════
    // Error handling
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_try_creates_tag_check() {
        let callee = make_fn("fallible", vec![], Some("i32"), vec![return_stmt(Some(int_expr(0)))]);
        let decl = make_fn("f", vec![], Some("i32"), vec![
            return_stmt(Some(try_expr(call_expr("fallible", vec![])))),
        ]);
        let f = lower(&decl, &[decl.clone(), callee]);
        assert!(find_enum_tag(&f));
        assert!(has_branch(&f));
    }

    #[test]
    fn lower_ensure_push_cleanup_return() {
        let decl = make_fn("f", vec![], None, vec![
            ensure_stmt(
                vec![expr_stmt(call_expr("do_work", vec![]))],
                None,
            ),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_ensure_push(&f));
        // Body goes in cleanup block, exit uses CleanupReturn
        assert!(find_cleanup_return(&f));
    }

    #[test]
    fn lower_ensure_with_handler() {
        let decl = make_fn("f", vec![], None, vec![
            ensure_stmt(
                vec![expr_stmt(call_expr("work", vec![]))],
                Some(("err", vec![expr_stmt(call_expr("cleanup", vec![ident_expr("err")]))])),
            ),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_ensure_push(&f));
        assert!(find_cleanup_return(&f));
        assert!(f.locals.iter().any(|l| l.name.as_deref() == Some("err")));
    }

    #[test]
    fn lower_unwrap_panics_on_err() {
        let decl = make_fn("f", vec![("x", "i32")], Some("i32"), vec![
            return_stmt(Some(Expr {
                id: NodeId(400),
                kind: ExprKind::Unwrap {
                    expr: Box::new(ident_expr("x")),
                    message: None,
                    bang: sp(),
                },
                span: sp(),
            })),
        ]);
        let f = lower_one(&decl);
        assert!(find_enum_tag(&f));
        assert!(has_branch(&f));
        assert!(find_call(&f, "panic_unwrap"));
        assert!(f.blocks.iter().any(|b| matches!(b.terminator.kind, MirTerminatorKind::Unreachable)));
    }

    // ═══════════════════════════════════════════════════════════
    // End-to-end
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn e2e_hello_world() {
        let print_fn = make_fn("print", vec![("s", "string")], None, vec![return_stmt(None)]);
        let decl = make_fn("main", vec![], None, vec![
            expr_stmt(call_expr("print", vec![string_expr("Hello, world!")])),
        ]);
        let f = lower(&decl, &[decl.clone(), print_fn]);
        assert_eq!(f.name, "main");
        assert!(find_call(&f, "print"));
    }

    #[test]
    fn e2e_mir_display_roundtrip() {
        let decl = make_fn("factorial", vec![("n", "i32")], Some("i32"), vec![
            return_stmt(Some(ident_expr("n"))),
        ]);
        let f = lower_one(&decl);
        let output = format!("{}", f);
        assert!(output.contains("func factorial"));
        assert!(output.contains("n: i32"));
        assert!(output.contains("-> i32"));
        assert!(output.contains("bb0:"));
        assert!(output.contains("return"));
    }

    #[test]
    fn e2e_nested_calls() {
        let g = make_fn("g", vec![("a", "i32")], Some("i32"), vec![return_stmt(Some(ident_expr("a")))]);
        let h = make_fn("h", vec![("a", "i32")], Some("i32"), vec![return_stmt(Some(ident_expr("a")))]);
        let decl = make_fn("f", vec![("x", "i32")], Some("i32"), vec![
            return_stmt(Some(call_expr("g", vec![call_expr("h", vec![ident_expr("x")])]))),
        ]);
        let all = vec![decl.clone(), g, h];
        let f = lower(&decl, &all);
        assert!(find_call(&f, "g"));
        assert!(find_call(&f, "h"));
    }

    #[test]
    fn e2e_assert_generates_branch() {
        let decl = make_fn("f", vec![("x", "i32")], None, vec![
            expr_stmt(Expr {
                id: NodeId(500),
                kind: ExprKind::Assert {
                    condition: Box::new(binary_expr(BinOp::Gt, ident_expr("x"), int_expr(0))),
                    message: Some(Box::new(string_expr("x must be positive"))),
                },
                span: sp(),
            }),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(has_branch(&f));
        assert!(find_call(&f, "assert_fail"));
        assert!(f.blocks.iter().any(|b| matches!(b.terminator.kind, MirTerminatorKind::Unreachable)));
    }

    #[test]
    fn e2e_cast_expression() {
        let decl = make_fn("f", vec![("x", "i32")], Some("i64"), vec![
            return_stmt(Some(Expr {
                id: NodeId(600),
                kind: ExprKind::Cast {
                    expr: Box::new(ident_expr("x")),
                    ty: rask_ast::ty::TypeExpr::named("i64"),
                },
                span: sp(),
            })),
        ]);
        let f = lower_one(&decl);
        let has_cast = f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s.kind, MirStmtKind::Assign { rvalue: MirRValue::Cast { .. }, .. }))
        });
        assert!(has_cast);
    }

    // ═══════════════════════════════════════════════════════════
    // Type constructors + enum variants
    // ═══════════════════════════════════════════════════════════

    fn lower_with_ctx(decl: &Decl, all_decls: &[Decl], ctx: &MirContext) -> MirFunction {
        let mut fns = MirLowerer::lower_function(decl, all_decls, ctx).expect("lowering failed");
        fns.remove(0)
    }

    fn find_store(f: &MirFunction) -> bool {
        f.blocks.iter().any(|b| {
            b.statements.iter().any(|s| matches!(s.kind, MirStmtKind::Store { .. }))
        })
    }

    fn count_stores(f: &MirFunction) -> usize {
        f.blocks.iter()
            .flat_map(|b| b.statements.iter())
            .filter(|s| matches!(s.kind, MirStmtKind::Store { .. }))
            .count()
    }

    #[test]
    fn lower_enum_variant_construct() {
        // Shape.Circle(5.0) → store tag 0, store payload f64
        use rask_check::layout::{EnumLayout, VariantLayout, FieldLayout};

        let shape_enum = EnumLayout {
            name: "Shape".to_string(),
            size: 16,
            align: 8,
            tag_ty: rask_types::Type::U8,
            tag_offset: 0,
            variants: vec![
                VariantLayout {
                    name: "Circle".to_string(),
                    tag: 0,
                    payload_offset: 8,
                    payload_size: 8,
                    fields: vec![FieldLayout {
                        name: "f0".to_string(),
                        ty: rask_types::Type::F64,
                        offset: 0,
                        size: 8,
                        align: 8,
                        attrs: vec![],
                        has_declared_default: false,
                        is_public: true,
                        decl_index: 0,
                        is_type_param: false,
                        bits: None,
                    }],
                    attrs: vec![],
                },
                VariantLayout {
                    name: "Square".to_string(),
                    tag: 1,
                    payload_offset: 8,
                    payload_size: 8,
                    fields: vec![FieldLayout {
                        name: "f0".to_string(),
                        ty: rask_types::Type::F64,
                        offset: 0,
                        size: 8,
                        align: 8,
                        attrs: vec![],
                        has_declared_default: false,
                        is_public: true,
                        decl_index: 0,
                        is_type_param: false,
                        bits: None,
                    }],
                    attrs: vec![],
                },
            ],
            attrs: vec![],
        };

        let enum_layouts = vec![shape_enum];
        let node_types = HashMap::new();
        let folded = Folded::default();
        let extern_funcs = std::collections::HashSet::new();
        let type_names = HashMap::new();
        let empty_coercions = HashMap::new();
        let empty_error_wraps = HashMap::new();
        let empty_fallback_shape = std::collections::HashSet::new();
        let empty_escaping = std::collections::HashSet::new();
        let empty_lent = std::collections::HashMap::new();
        let empty_return_moves = std::collections::HashMap::new();
        let empty_try_placement = HashMap::new();
        let empty_rewrites = HashMap::new();
        let empty_targets = HashMap::new();
        let empty_operator_targets = HashMap::new();
        let empty_resource_types = std::collections::HashSet::new();
        let empty_nominal = HashMap::new();
        let empty_type_defs = rask_check::TypeTable::default();
        let ctx = MirContext {
            // No checker in a hand-built lowering unit, so there's no GC9
            // decision to read. Stated, not defaulted.
            mutate_self_fns: None,
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
            type_names: &type_names,
            type_defs: &empty_type_defs,
            folded: &folded,
            extern_funcs: &extern_funcs,
            package_modules: &std::collections::HashSet::new(),
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(std::collections::HashSet::new()),
            line_map: None,
            source_file: None,
            interface_methods: HashMap::new(),
            interface_coercions: &empty_coercions,
            c_fn_pointers: &crate::lower::EMPTY_C_FN_POINTERS,
            c_contexts: &crate::lower::EMPTY_C_CONTEXTS,
            error_wraps: &empty_error_wraps,
            fallback_keeps_shape: &empty_fallback_shape,
            err_returns: &empty_fallback_shape,
            escaping_closures: &empty_escaping,
            lent_captures: &empty_lent,
            field_reuses: &empty_escaping,
            return_moves: &empty_return_moves,
            task_bound_closures: &empty_escaping,
            try_chain_placement: &empty_try_placement,
            call_rewrites: &empty_rewrites,
            map_key_fns: empty::map_key_fns(),
            decode_defaults: empty::decode_defaults(),
            call_targets: &empty_targets,
            operator_targets: &empty_operator_targets,
            resource_types: &empty_resource_types,
            nominal_underlying: &empty_nominal,
            const_slot_types: std::cell::RefCell::new(HashMap::new()),
            inferred_fn_ret: &EMPTY_INFERRED_RET,
        };

        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Shape"), "Circle", vec![float_expr(5.0)])),
            return_stmt(None),
        ]);
        let f = lower_with_ctx(&decl, &[decl.clone()], &ctx);

        // Should emit stores for tag + payload, not a Call
        assert!(find_store(&f));
        assert_eq!(count_stores(&f), 2); // tag store + payload store
        assert!(!find_call(&f, "Circle"));
    }

    #[test]
    fn lower_enum_variant_no_payload() {
        // Color.Red() → store tag only
        use rask_check::layout::{EnumLayout, VariantLayout};

        let color_enum = EnumLayout {
            name: "Color".to_string(),
            size: 1,
            align: 1,
            tag_ty: rask_types::Type::U8,
            tag_offset: 0,
            variants: vec![
                VariantLayout { name: "Red".to_string(), tag: 0, payload_offset: 0, payload_size: 0, fields: vec![], attrs: vec![] },
                VariantLayout { name: "Green".to_string(), tag: 1, payload_offset: 0, payload_size: 0, fields: vec![], attrs: vec![] },
                VariantLayout { name: "Blue".to_string(), tag: 2, payload_offset: 0, payload_size: 0, fields: vec![], attrs: vec![] },
            ],
            attrs: vec![],
        };

        let enum_layouts = vec![color_enum];
        let node_types = HashMap::new();
        let folded = Folded::default();
        let extern_funcs = std::collections::HashSet::new();
        let type_names = HashMap::new();
        let empty_coercions = HashMap::new();
        let empty_error_wraps = HashMap::new();
        let empty_fallback_shape = std::collections::HashSet::new();
        let empty_escaping = std::collections::HashSet::new();
        let empty_lent = std::collections::HashMap::new();
        let empty_return_moves = std::collections::HashMap::new();
        let empty_try_placement = HashMap::new();
        let empty_rewrites = HashMap::new();
        let empty_targets = HashMap::new();
        let empty_operator_targets = HashMap::new();
        let empty_resource_types = std::collections::HashSet::new();
        let empty_nominal = HashMap::new();
        let empty_type_defs = rask_check::TypeTable::default();
        let ctx = MirContext {
            // No checker in a hand-built lowering unit, so there's no GC9
            // decision to read. Stated, not defaulted.
            mutate_self_fns: None,
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
            type_names: &type_names,
            type_defs: &empty_type_defs,
            folded: &folded,
            extern_funcs: &extern_funcs,
            package_modules: &std::collections::HashSet::new(),
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(std::collections::HashSet::new()),
            line_map: None,
            source_file: None,
            interface_methods: HashMap::new(),
            interface_coercions: &empty_coercions,
            c_fn_pointers: &crate::lower::EMPTY_C_FN_POINTERS,
            c_contexts: &crate::lower::EMPTY_C_CONTEXTS,
            error_wraps: &empty_error_wraps,
            fallback_keeps_shape: &empty_fallback_shape,
            err_returns: &empty_fallback_shape,
            escaping_closures: &empty_escaping,
            lent_captures: &empty_lent,
            field_reuses: &empty_escaping,
            return_moves: &empty_return_moves,
            task_bound_closures: &empty_escaping,
            try_chain_placement: &empty_try_placement,
            call_rewrites: &empty_rewrites,
            map_key_fns: empty::map_key_fns(),
            decode_defaults: empty::decode_defaults(),
            call_targets: &empty_targets,
            operator_targets: &empty_operator_targets,
            resource_types: &empty_resource_types,
            nominal_underlying: &empty_nominal,
            const_slot_types: std::cell::RefCell::new(HashMap::new()),
            inferred_fn_ret: &EMPTY_INFERRED_RET,
        };

        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Color"), "Red", vec![])),
            return_stmt(None),
        ]);
        let f = lower_with_ctx(&decl, &[decl.clone()], &ctx);

        assert!(find_store(&f));
        assert_eq!(count_stores(&f), 1); // tag only
    }

    #[test]
    fn lower_enum_variant_multi_field() {
        // Msg.Pair(1, 2) → store tag + 2 payload fields
        use rask_check::layout::{EnumLayout, VariantLayout, FieldLayout};

        let msg_enum = EnumLayout {
            name: "Msg".to_string(),
            size: 12,
            align: 4,
            tag_ty: rask_types::Type::U8,
            tag_offset: 0,
            variants: vec![
                VariantLayout { name: "Empty".to_string(), tag: 0, payload_offset: 4, payload_size: 0, fields: vec![], attrs: vec![] },
                VariantLayout {
                    name: "Pair".to_string(),
                    tag: 1,
                    payload_offset: 4,
                    payload_size: 8,
                    fields: vec![
                        FieldLayout { name: "f0".to_string(), ty: rask_types::Type::I32, offset: 0, size: 4, align: 4, attrs: vec![], has_declared_default: false, is_public: true, decl_index: 0, is_type_param: false, bits: None },
                        FieldLayout { name: "f1".to_string(), ty: rask_types::Type::I32, offset: 4, size: 4, align: 4, attrs: vec![], has_declared_default: false, is_public: true, decl_index: 0, is_type_param: false, bits: None },
                    ],
                    attrs: vec![],
                },
            ],
            attrs: vec![],
        };

        let enum_layouts = vec![msg_enum];
        let node_types = HashMap::new();
        let folded = Folded::default();
        let extern_funcs = std::collections::HashSet::new();
        let type_names = HashMap::new();
        let empty_coercions = HashMap::new();
        let empty_error_wraps = HashMap::new();
        let empty_fallback_shape = std::collections::HashSet::new();
        let empty_escaping = std::collections::HashSet::new();
        let empty_lent = std::collections::HashMap::new();
        let empty_return_moves = std::collections::HashMap::new();
        let empty_try_placement = HashMap::new();
        let empty_rewrites = HashMap::new();
        let empty_targets = HashMap::new();
        let empty_operator_targets = HashMap::new();
        let empty_resource_types = std::collections::HashSet::new();
        let empty_nominal = HashMap::new();
        let empty_type_defs = rask_check::TypeTable::default();
        let ctx = MirContext {
            // No checker in a hand-built lowering unit, so there's no GC9
            // decision to read. Stated, not defaulted.
            mutate_self_fns: None,
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
            type_names: &type_names,
            type_defs: &empty_type_defs,
            folded: &folded,
            extern_funcs: &extern_funcs,
            package_modules: &std::collections::HashSet::new(),
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(std::collections::HashSet::new()),
            line_map: None,
            source_file: None,
            interface_methods: HashMap::new(),
            interface_coercions: &empty_coercions,
            c_fn_pointers: &crate::lower::EMPTY_C_FN_POINTERS,
            c_contexts: &crate::lower::EMPTY_C_CONTEXTS,
            error_wraps: &empty_error_wraps,
            fallback_keeps_shape: &empty_fallback_shape,
            err_returns: &empty_fallback_shape,
            escaping_closures: &empty_escaping,
            lent_captures: &empty_lent,
            field_reuses: &empty_escaping,
            return_moves: &empty_return_moves,
            task_bound_closures: &empty_escaping,
            try_chain_placement: &empty_try_placement,
            call_rewrites: &empty_rewrites,
            map_key_fns: empty::map_key_fns(),
            decode_defaults: empty::decode_defaults(),
            call_targets: &empty_targets,
            operator_targets: &empty_operator_targets,
            resource_types: &empty_resource_types,
            nominal_underlying: &empty_nominal,
            const_slot_types: std::cell::RefCell::new(HashMap::new()),
            inferred_fn_ret: &EMPTY_INFERRED_RET,
        };

        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Msg"), "Pair", vec![int_expr(1), int_expr(2)])),
            return_stmt(None),
        ]);
        let f = lower_with_ctx(&decl, &[decl.clone()], &ctx);

        assert_eq!(count_stores(&f), 3); // tag + 2 fields
    }

    #[test]
    fn lower_static_method_call_on_type() {
        // Instant.now() → Call to Instant_now
        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Instant"), "now", vec![])),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_call(&f, "Instant_now"));
    }

    #[test]
    fn container_without_an_element_type_is_refused() {
        // The checker rejects an element type it couldn't work out, so one
        // missing here is a lowering bug, not a reason to guess a size.
        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Vec"), "new", vec![])),
            return_stmt(None),
        ]);
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        let err = MirLowerer::lower_function(&decl, &[decl.clone()], &ctx).unwrap_err();
        assert!(matches!(err, LoweringError::InvalidConstruct(ref m) if m.contains("no type")), "{err:?}");
    }

    #[test]
    fn lower_string_static_method() {
        // string.new() → Call to string_new
        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("string"), "new", vec![])),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_call(&f, "string_new"));
    }

    #[test]
    fn lower_method_on_value_still_works() {
        // a.add(b) where a is a local variable → BinaryOp (not static call)
        let decl = make_fn("f", vec![("a", "i32"), ("b", "i32")], Some("i32"), vec![
            return_stmt(Some(method_call_expr(ident_expr("a"), "add", vec![ident_expr("b")]))),
        ]);
        let f = lower_one(&decl);
        assert!(find_assign_binop(&f));
        assert!(!find_call(&f, "i32_add"));
    }

    // ═══════════════════════════════════════════════════════════
    // Concurrency: using Multitasking {} emits runtime init/shutdown
    // ═══════════════════════════════════════════════════════════

    #[test]
    fn lower_using_multitasking_emits_init_shutdown() {
        let using_block = Expr {
            id: NodeId(700),
            kind: ExprKind::UsingBlock {
                name: "Multitasking".to_string(),
                args: vec![],
                body: vec![
                    expr_stmt(call_expr("work", vec![])),
                ],
            },
            span: sp(),
        };
        let work = make_fn("work", vec![], None, vec![return_stmt(None)]);
        let decl = make_fn("main", vec![], None, vec![
            expr_stmt(using_block),
            return_stmt(None),
        ]);
        let f = lower(&decl, &[decl.clone(), work]);
        assert!(find_call(&f, "rask_runtime_init"), "missing rask_runtime_init call");
        assert!(find_call(&f, "rask_runtime_shutdown"), "missing rask_runtime_shutdown call");
        assert!(find_call(&f, "work"), "missing body work() call");
    }

    #[test]
    fn lower_using_threadpool_emits_init_shutdown() {
        let using_block = Expr {
            id: NodeId(701),
            kind: ExprKind::UsingBlock {
                name: "ThreadPool".to_string(),
                args: vec![],
                body: vec![
                    expr_stmt(call_expr("work", vec![])),
                ],
            },
            span: sp(),
        };
        let work = make_fn("work", vec![], None, vec![return_stmt(None)]);
        let decl = make_fn("main", vec![], None, vec![
            expr_stmt(using_block),
            return_stmt(None),
        ]);
        // ThreadPool installs the worker pool, not the green scheduler — the
        // two contexts are independent, and sharing one init was why
        // `workers: n` went nowhere (#686).
        let f = lower(&decl, &[decl.clone(), work]);
        assert!(find_call(&f, "rask_threadpool_init"), "ThreadPool should emit pool init");
        assert!(find_call(&f, "rask_threadpool_shutdown"), "ThreadPool should emit pool shutdown");
        assert!(!find_call(&f, "rask_runtime_init"), "ThreadPool must not start the green scheduler");
        assert!(find_call(&f, "work"));
    }

    #[test]
    fn lower_using_unknown_no_init() {
        let using_block = Expr {
            id: NodeId(702),
            kind: ExprKind::UsingBlock {
                name: "SomeOtherContext".to_string(),
                args: vec![],
                body: vec![
                    expr_stmt(call_expr("work", vec![])),
                ],
            },
            span: sp(),
        };
        let work = make_fn("work", vec![], None, vec![return_stmt(None)]);
        let decl = make_fn("main", vec![], None, vec![
            expr_stmt(using_block),
            return_stmt(None),
        ]);
        let f = lower(&decl, &[decl.clone(), work]);
        assert!(!find_call(&f, "rask_runtime_init"), "Unknown context should not emit init");
        assert!(!find_call(&f, "rask_runtime_shutdown"), "Unknown context should not emit shutdown");
        assert!(!find_call(&f, "rask_threadpool_init"), "Unknown context should not start a pool");
        assert!(find_call(&f, "work"));
    }
}

/// A runtime handle is one word, a pointer.
///
/// Workaround: `Handle` stays an `i64`. As a pointer, a `Handles` that
/// `detach(take self)` has taken is no longer released again by its caller,
/// and that stray release is all that frees the nodes `Handles` drains with
/// `rest = *next` (#1585).
fn lowers_to_ptr(name: &str) -> bool {
    name != "Handle" && rask_stdlib::is_runtime_handle(name)
}
