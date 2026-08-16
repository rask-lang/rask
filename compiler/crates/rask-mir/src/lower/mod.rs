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
use rask_mono::{StructLayout, EnumLayout};
use rask_types::Type;
use std::collections::HashMap;

/// Typed expression result from lowering
type TypedOperand = (MirOperand, MirType);

/// Sentinel value representing None for niche-optimized Option<Handle<T>>.
/// All bits set (index=UINT32_MAX, gen=UINT32_MAX) — impossible for a real handle.
pub(crate) const HANDLE_NONE_SENTINEL: i64 = rask_mono::abi::HANDLE_NONE_SENTINEL;

/// Check if a raw Type is Option<Handle<T>> (eligible for niche optimization).
pub(crate) fn is_niche_option_handle(ty: &Type) -> bool {
    if let Some(inner) = ty.as_option() {
        matches!(inner, Type::UnresolvedGeneric { name, .. } if name == "Handle")
    } else {
        false
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

/// #270: classify a function's params for scalar `mutate` write-back. Returns one
/// entry per param: `Some(scalar_ty)` for a `mutate` param of a Copy scalar type
/// (passed by pointer), `None` otherwise. Aggregates already pass by pointer, so
/// they stay `None` here.
fn scalar_mutate_params(params: &[rask_ast::decl::Param], ctx: &MirContext) -> Vec<Option<MirType>> {
    params
        .iter()
        .map(|p| {
            if !p.is_mutate || p.ty.is_empty() {
                return None;
            }
            let ty = ctx.resolve_type_str(p.ty.trim_start_matches('&'));
            if crate::lower::stmt::mutate_param_by_pointer(&ty) {
                None
            } else {
                Some(ty)
            }
        })
        .collect()
}

/// Which params are `mutate` on an aggregate — the ones the caller passes by
/// address, so a callee's write is supposed to reach the caller's storage.
fn aggregate_mutate_params(params: &[rask_ast::decl::Param], ctx: &MirContext) -> Vec<bool> {
    params
        .iter()
        .map(|p| {
            if !p.is_mutate || p.ty.is_empty() {
                return false;
            }
            let ty = ctx.resolve_type_str(p.ty.trim_start_matches('&'));
            crate::lower::stmt::mutate_param_by_pointer(&ty)
        })
        .collect()
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
    /// Declared parameter type strings, positionally. Used to type an
    /// unannotated closure argument's parameters: `|req| { … }` passed to a
    /// `func(Request) -> Response` parameter has nothing else to go on, and
    /// defaulting them to i64 made field access and method dispatch inside the
    /// closure body operate on the wrong type.
    param_ty_strs: Vec<Option<String>>,
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
}

/// One wrapper layer of a type, as seen by `coerce_into_wrapper`.
#[derive(Debug, Clone, PartialEq)]
enum WrapLayer {
    Option,
    Result { err: MirType },
}

/// Metadata for a comptime-evaluated global constant.
#[derive(Debug, Clone)]
pub struct ComptimeGlobalMeta {
    pub bytes: Vec<u8>,
    /// Element count (for Vec/Array globals)
    pub elem_count: usize,
    /// Type prefix for method dispatch ("Vec", "Array", etc.)
    pub type_prefix: String,
    /// What the elements are, for a Vec/Array global. Without it, indexing one
    /// had no element type to resolve and lowering fell back to a guess.
    pub elem_type: Option<String>,
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
        ExprKind::Int(..) | ExprKind::Float(..) | ExprKind::String(_) | ExprKind::Bool(_)
    )
}

/// Does a value of this type live in memory, with the local holding a pointer
/// to it? Those go on the heap so the init thunk's frame isn't captured.
fn mir_ty_is_aggregate(ty: &MirType) -> bool {
    matches!(
        ty,
        MirType::Struct(_)
            | MirType::Enum(_)
            | MirType::Tuple(_)
            | MirType::Result { .. }
            | MirType::Option(_)
            | MirType::Union(_)
            | MirType::Array { .. }
            | MirType::Slice(_)
            | MirType::SimdVector { .. }
            | MirType::String
            | MirType::TraitObject { .. }
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
    empty_of!(comptime_globals, HashMap<String, ComptimeGlobalMeta>);
    empty_of!(node_names, HashMap<NodeId, String>);
    empty_of!(str_map, HashMap<String, String>);
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
    /// The five tables that come straight off `TypedProgram` are read here, so
    /// they can't be forgotten or blanked. `node_types` and `call_targets` stay
    /// explicit: the real pipeline passes versions merged with the
    /// monomorphizer's instantiated bodies, and silently taking the unmerged
    /// ones off `typed` would lose every generic instantiation.
    ///
    /// Everything else defaults to empty, with a `with_*` to set it. A new field
    /// is one edit here, and no call site can miss it.
    pub fn new(
        typed: &'a rask_types::TypedProgram,
        struct_layouts: &'a [StructLayout],
        enum_layouts: &'a [EnumLayout],
        node_types: &'a HashMap<NodeId, Type>,
        call_targets: &'a HashMap<NodeId, rask_types::Callee>,
        type_names: &'a HashMap<rask_types::TypeId, String>,
    ) -> Self {
        Self {
            struct_layouts,
            enum_layouts,
            node_types,
            call_targets,
            type_names,
            // Straight off the checker — never optional.
            mutate_self_fns: Some(&typed.mutate_self_fns),
            trait_coercions: &typed.trait_coercions,
            error_wraps: &typed.error_wraps,
            fallback_keeps_shape: &typed.fallback_keeps_shape,
            inferred_fn_ret: &typed.inferred_fn_ret,
            // Defaults; the `with_*` below set the ones a caller has.
            comptime_globals: empty::comptime_globals(),
            extern_funcs: empty::strings(),
            package_modules: empty::strings(),
            line_map: None,
            source_file: None,
            trait_methods: HashMap::new(),
            call_rewrites: empty::node_names(),
            resource_types: empty::strings(),
            nominal_underlying: empty::str_map(),
            comptime_interp: None,
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(Default::default()),
            const_slot_types: std::cell::RefCell::new(HashMap::new()),
        }
    }

    pub fn with_comptime_globals(
        mut self,
        globals: &'a HashMap<String, ComptimeGlobalMeta>,
    ) -> Self {
        self.comptime_globals = globals;
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

    pub fn with_trait_methods(mut self, methods: HashMap<String, Vec<String>>) -> Self {
        self.trait_methods = methods;
        self
    }

    pub fn with_call_rewrites(mut self, rewrites: &'a HashMap<NodeId, String>) -> Self {
        self.call_rewrites = rewrites;
        self
    }

    pub fn with_resource_types(mut self, types: &'a std::collections::HashSet<String>) -> Self {
        self.resource_types = types;
        self
    }

    pub fn with_nominal_underlying(mut self, map: &'a HashMap<String, String>) -> Self {
        self.nominal_underlying = map;
        self
    }

    pub fn with_comptime_interp(mut self, interp: rask_comptime::ComptimeInterpreter) -> Self {
        self.comptime_interp = Some(std::cell::RefCell::new(interp));
        self
    }
}

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
    /// Comptime-evaluated global constants (name → metadata).
    /// MIR lowering emits GlobalRef for these instead of lowering the init expr.
    pub comptime_globals: &'a HashMap<String, ComptimeGlobalMeta>,
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
    /// Comptime interpreter for evaluating `comptime if` during lowering.
    /// None in tests or when cfg is unavailable.
    pub comptime_interp: Option<std::cell::RefCell<rask_comptime::ComptimeInterpreter>>,
    /// Trait method lists for trait object dispatch.
    /// Key: trait name, Value: method names in declaration order.
    pub trait_methods: HashMap<String, Vec<String>>,
    /// TR5: implicit trait coercion sites. NodeId of expression → trait name.
    pub trait_coercions: &'a HashMap<NodeId, String>,
    /// ER31a: `try` sites whose propagated error gets wrapped in a variant of
    /// the enclosing function's error enum, keyed by the `try` expression.
    pub error_wraps: &'a HashMap<NodeId, rask_types::ErrorWrap>,
    /// ER14a: `??` sites whose right side is still wrapped, so the present
    /// path hands back the left operand instead of its payload.
    pub fallback_keeps_shape: &'a std::collections::HashSet<NodeId>,
    /// Call expression NodeId → mangled callee name for generic function calls.
    pub call_rewrites: &'a HashMap<NodeId, String>,
    /// CALL6: the receiver type dispatch actually selected, per call node.
    ///
    /// Authoritative for method-name qualification. `node_types` holds the type
    /// assigned to the receiver *expression*, which is frequently an unresolved
    /// variable or missing outright; this is the applied type the checker
    /// dispatched on, so lowering reads it instead of guessing a prefix from
    /// the receiver's syntactic shape.
    pub call_targets: &'a HashMap<NodeId, rask_types::Callee>,
    /// Type names marked with `@resource` — used for resource tracking ops (C1/C2).
    pub resource_types: &'a std::collections::HashSet<String>,
    /// Nominal newtype name → the type it wraps, as a type string.
    ///
    /// `type Id = u64 with (…)` has no layout of its own: it *is* a u64 with a
    /// distinct identity, so it's transparent in MIR. Without this the name
    /// resolved to a bare `Ptr` with nothing allocated behind it, and
    /// construction stored through an uninitialised pointer (#445).
    pub nominal_underlying: &'a HashMap<String, String>,
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
    pub fn fn_ret_ty(&self, name: &str, declared: Option<&str>) -> MirType {
        if let Some(s) = declared {
            return self.resolve_type_str(s);
        }
        match self.inferred_fn_ret.get(name) {
            Some(ty) => self.type_to_mir(ty),
            None => MirType::Void,
        }
    }

    /// Empty context for tests that don't need layouts or type information.
    pub fn empty_with_map(map: &'a HashMap<NodeId, Type>) -> MirContext<'a> {
        static EMPTY_COMPTIME: std::sync::LazyLock<HashMap<String, ComptimeGlobalMeta>> =
            std::sync::LazyLock::new(HashMap::new);
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
        static EMPTY_REWRITES: std::sync::LazyLock<HashMap<NodeId, String>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_TARGETS: std::sync::LazyLock<HashMap<NodeId, rask_types::Callee>> =
            std::sync::LazyLock::new(HashMap::new);
        static EMPTY_RESOURCE_TYPES: std::sync::LazyLock<std::collections::HashSet<String>> =
            std::sync::LazyLock::new(std::collections::HashSet::new);
        static EMPTY_NOMINAL: std::sync::LazyLock<HashMap<String, String>> =
            std::sync::LazyLock::new(HashMap::new);
        MirContext {
            struct_layouts: &[],
            mutate_self_fns: None,
            enum_layouts: &[],
            node_types: map,
            type_names: &EMPTY_TYPE_NAMES,
            comptime_globals: &EMPTY_COMPTIME,
            extern_funcs: &EMPTY_EXTERNS,
            package_modules: &EMPTY_PACKAGES,
            line_map: None,
            source_file: None,
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(std::collections::HashSet::new()),
            comptime_interp: None,
            trait_methods: HashMap::new(),
            trait_coercions: &EMPTY_COERCIONS,
            error_wraps: &EMPTY_ERROR_WRAPS,
            fallback_keeps_shape: &EMPTY_COALESCE_SHAPE,
            call_rewrites: &EMPTY_REWRITES,
            call_targets: &EMPTY_TARGETS,
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

    pub fn find_struct(&self, name: &str) -> Option<(u32, &StructLayout)> {
        self.struct_layouts
            .iter()
            .enumerate()
            .find(|(_, s)| s.name == name)
            .map(|(i, s)| (i as u32, s))
    }

    /// Size in bytes for a MirType. Now just delegates to `MirType::size()` since
    /// StructLayoutId/EnumLayoutId carry their real byte sizes.
    pub fn mir_type_size(&self, ty: &MirType) -> u32 {
        ty.size()
    }

    pub fn find_enum(&self, name: &str) -> Option<(u32, &EnumLayout)> {
        self.enum_layouts
            .iter()
            .enumerate()
            .find(|(_, e)| e.name == name)
            .map(|(i, e)| (i as u32, e))
    }

    /// Resolve a type string to MirType, looking up struct/enum names in layouts.
    pub fn resolve_type_str(&self, s: &str) -> MirType {
        match s.trim() {
            "i8" => MirType::I8,
            "i16" => MirType::I16,
            "i32" => MirType::I32,
            "i64" | "isize" => MirType::I64,
            "u8" => MirType::U8,
            "u16" => MirType::U16,
            "u32" => MirType::U32,
            "u64" | "usize" => MirType::U64,
            "f32" => MirType::F32,
            "f64" => MirType::F64,
            "bool" => MirType::Bool,
            "char" => MirType::Char,
            "string" => MirType::String,
            "()" | "" => MirType::Void,
            name => {
                // "any TraitName" → TraitObject
                if let Some(trait_name) = name.strip_prefix("any ") {
                    return MirType::TraitObject { trait_name: trait_name.to_string() };
                }
                // "[T; N]" → fixed-size array, "[]T" / "[T]" → slice. Without
                // these an annotated `const a: [i32; 5]` fell through to the
                // pointer default, and the array's length was gone by the time
                // `a.len()` looked for it — the call failed dispatch outright
                // while the same code without the annotation worked.
                if let Some(inner) = name.strip_prefix("[]") {
                    return MirType::Slice(Box::new(self.resolve_type_str(inner)));
                }
                if name.starts_with('[') && name.ends_with(']') {
                    let inner = &name[1..name.len() - 1];
                    if let Some(semi) = inner.rfind(';') {
                        let elem = self.resolve_type_str(inner[..semi].trim());
                        // A symbolic length (a comptime parameter) has no value
                        // here; 0 keeps the element type intact, which is what
                        // the checker does with the same shape.
                        let len = inner[semi + 1..].trim().parse::<u32>().unwrap_or(0);
                        return MirType::Array { elem: Box::new(elem), len };
                    }
                    return MirType::Slice(Box::new(self.resolve_type_str(inner)));
                }
                // "(T1, T2, ...)" → Tuple
                if name.starts_with('(') && name.ends_with(')') {
                    let inner = &name[1..name.len() - 1];
                    if inner.is_empty() {
                        return MirType::Void;
                    }
                    let parts = split_top_level_parens(inner, ',');
                    return MirType::Tuple(
                        parts.iter().map(|p| self.resolve_type_str(p.trim())).collect()
                    );
                }
                // "T or E" → Result<T, E>
                if let Some(or_pos) = name.find(" or ") {
                    let ok_str = name[..or_pos].trim();
                    let err_str = name[or_pos + 4..].trim();
                    return MirType::Result {
                        ok: Box::new(self.resolve_type_str(ok_str)),
                        err: Box::new(self.resolve_type_str(err_str)),
                    };
                }
                // "Result<T, E>" → MirType::Result
                if let Some(inner) = name.strip_prefix("Result<").and_then(|s| s.strip_suffix('>')) {
                    // Split on top-level comma (respecting nested <...>)
                    if let Some(comma) = find_top_level_comma(inner) {
                        let ok_str = inner[..comma].trim();
                        let err_str = inner[comma + 1..].trim();
                        return MirType::Result {
                            ok: Box::new(self.resolve_type_str(ok_str)),
                            err: Box::new(self.resolve_type_str(err_str)),
                        };
                    }
                }
                // "Option<T>" → MirType::Option
                if let Some(inner) = name.strip_prefix("Option<").and_then(|s| s.strip_suffix('>')) {
                    return MirType::Option(Box::new(self.resolve_type_str(inner)));
                }
                // "T?" → MirType::Option (shorthand syntax from type annotations)
                if let Some(inner) = name.strip_suffix('?') {
                    return MirType::Option(Box::new(self.resolve_type_str(inner)));
                }
                // Generic collection types: Vec<T>, Map<K,V>, etc. are heap pointers
                if name.starts_with("Vec<") || name == "Vec" {
                    return MirType::Ptr; // Vec handle (opaque pointer)
                }
                if name.starts_with("Map<") || name == "Map" {
                    return MirType::Ptr; // Map handle (opaque pointer)
                }
                if name.starts_with("Pool<") || name == "Pool" {
                    return MirType::Ptr;
                }
                if name.starts_with("Handle<") {
                    return MirType::Handle;
                }
                if name.starts_with("Channel<") || name.starts_with("Sender<")
                    || name.starts_with("Receiver<") || name.starts_with("Shared<")
                {
                    return MirType::Ptr;
                }
                // A nominal newtype has no layout — it is whatever it wraps.
                if let Some(underlying) = self.nominal_underlying.get(name) {
                    if underlying != name {
                        return self.resolve_type_str(underlying);
                    }
                }
                if let Some((idx, sl)) = self.find_struct(name) {
                    self.struct_or_handle(name, idx, sl)
                } else if let Some((idx, el)) = self.find_enum(name) {
                    MirType::Enum(EnumLayoutId::new(idx, el.size, el.align))
                } else if let Some(base) = name.split('<').next() {
                    // Generic type like "Box<i64>" — try base name "Box"
                    if let Some((idx, sl)) = self.find_struct(base) {
                        self.struct_or_handle(base, idx, sl)
                    } else if let Some((idx, el)) = self.find_enum(base) {
                        MirType::Enum(EnumLayoutId::new(idx, el.size, el.align))
                    } else {
                        MirType::Ptr
                    }
                } else {
                    MirType::Ptr
                }
            }
        }
    }

    /// Convert a Type from the type checker to MirType.
    pub fn type_to_mir(&self, ty: &Type) -> MirType {
        match ty {
            Type::Unit | Type::None => MirType::Void,
            Type::Bool => MirType::Bool,
            Type::I8 => MirType::I8,
            Type::I16 => MirType::I16,
            Type::I32 => MirType::I32,
            Type::I64 | Type::I128 => MirType::I64,
            Type::U8 => MirType::U8,
            Type::U16 => MirType::U16,
            Type::U32 => MirType::U32,
            Type::U64 | Type::U128 => MirType::U64,
            Type::F32 => MirType::F32,
            Type::F64 => MirType::F64,
            Type::Char => MirType::Char,
            Type::String => MirType::String,
            Type::Never => MirType::Void,
            Type::TraitObject { trait_name } => MirType::TraitObject { trait_name: trait_name.clone() },
            // Named types — look up in struct/enum layouts by name
            Type::UnresolvedNamed(name) => self.resolve_type_str(name),
            // Handle<T> → packed i64 handle
            Type::UnresolvedGeneric { name, .. } if name == "Handle" => MirType::Handle,
            // Resolved named types — look up via type_names, then struct/enum layouts
            Type::Named(id) => {
                if let Some(name) = self.type_names.get(id) {
                    self.resolve_type_str(name)
                } else {
                    MirType::Ptr
                }
            }
            Type::Generic { base, .. } => {
                if let Some(name) = self.type_names.get(base) {
                    self.resolve_type_str(name)
                } else {
                    MirType::Ptr
                }
            }
            Type::UnresolvedGeneric { .. } => {
                let type_str = format!("{}", ty);
                self.resolve_type_str(&type_str)
            }
            // Raw pointers and function types are pointer-sized
            Type::RawPtr(_) | Type::Fn { .. } => MirType::Ptr,
            // Tuple → struct-like layout with positional fields
            Type::Tuple(fields) => {
                MirType::Tuple(fields.iter().map(|t| self.type_to_mir(t)).collect())
            }
            // Array → real array with element type and length
            Type::Array { elem, len } => MirType::Array {
                elem: Box::new(self.type_to_mir(elem)),
                len: *len as u32,
            },
            // Slice → fat pointer (ptr + len)
            Type::Slice(elem) => MirType::Slice(Box::new(self.type_to_mir(elem))),
            // Option (T or none): niche-optimized Handle or tagged union
            Type::Result { ok: inner, err } if **err == Type::None => {
                if matches!(inner.as_ref(), Type::UnresolvedGeneric { name, .. } if name == "Handle") {
                    MirType::Handle
                } else {
                    MirType::Option(Box::new(self.type_to_mir(inner)))
                }
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
        }
    }

    /// Look up the MIR type for an expression node.
    pub fn lookup_node_type(&self, node_id: NodeId) -> Option<MirType> {
        let found = self.node_types.get(&node_id);
        crate::fallback::record_lookup(found);
        found.map(|ty| self.type_to_mir(ty))
    }

    /// Look up the raw Type for an expression node (preserves generic info).
    pub fn lookup_raw_type(&self, node_id: NodeId) -> Option<&Type> {
        let found = self.node_types.get(&node_id);
        crate::fallback::record_lookup(found);
        found
    }

    /// Extract stdlib type prefix for method name qualification.
    ///
    /// Returns the type prefix (e.g. "Vec", "Map", "string") used to build
    /// qualified method names like "Vec_push", "Map_get", "string_len".
    /// Without qualification, bare names like "get" or "len" are ambiguous
    /// across Vec, Map, String, and Pool.
    ///
    /// Type/module names are derived from stdlib stub files via
    /// `rask_stdlib::mir_metadata`. Structural types (Result, Option, Ptr)
    /// are matched directly since they're language-level, not stdlib.
    pub fn stdlib_type_prefix(ty: &Type) -> Option<&str> {
        let names = rask_stdlib::mir_metadata::stdlib_type_names();
        let modules = rask_stdlib::mir_metadata::stdlib_module_names();
        match ty {
            Type::String => Some("string"),
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
    pub fn type_prefix(ty: &Type, type_names: &HashMap<rask_types::TypeId, String>) -> Option<String> {
        if let Some(s) = Self::stdlib_type_prefix(ty) {
            return Some(s.to_string());
        }
        match ty {
            Type::Named(id) => {
                type_names.get(id)
                    .filter(|name| name.chars().next().map_or(false, |c| c.is_uppercase()))
                    .cloned()
            }
            Type::Generic { base, .. } => {
                type_names.get(base)
                    .filter(|name| name.chars().next().map_or(false, |c| c.is_uppercase()))
                    .cloned()
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

    pub fn recorded_prefix(&self, node: NodeId) -> Option<String> {
        match self.call_targets.get(&node)? {
            rask_types::Callee::Method { recv, .. } => Self::type_prefix(recv, self.type_names)
                .or_else(|| builtin_method_prefix(recv).map(str::to_string)),
            rask_types::Callee::Free(_) => None,
        }
    }

    /// Extract type prefix from a field type string (e.g. "Vec<string>" → "Vec").
    /// Used when resolving method calls on struct fields.
    pub fn type_prefix_str(s: &str) -> Option<String> {
        let s = s.trim();
        match s {
            "string" => Some("string".to_string()),
            "bool" | "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64"
            | "f32" | "f64" | "char" => None,
            _ => {
                // "Vec<...>" → "Vec", "Map<...>" → "Map", etc.
                if let Some(pos) = s.find('<') {
                    Some(s[..pos].to_string())
                } else if s.chars().next().map_or(false, |c| c.is_uppercase()) {
                    Some(s.to_string())
                } else {
                    None
                }
            }
        }
    }
}

/// Supplementary metadata for a local variable, keyed by variable name.
/// Consolidates type prefix, full type string, collection element type,
/// and channel element size into one struct so they stay in sync.
#[derive(Clone, Default)]
pub(crate) struct LocalMeta {
    /// Stdlib type prefix (e.g. "Random", "File", "Vec").
    /// Fallback when the type checker leaves types unresolved.
    pub type_prefix: Option<String>,
    /// Full type annotation string (e.g. "Shared<Database>").
    /// Resolves generic inner types when type checker info is incomplete.
    pub full_type: Option<String>,
    /// Collection element MirType (e.g. the T in Vec<T>).
    /// Propagates element types through for-in iteration after mono.
    pub elem_type: Option<MirType>,
    /// Channel/Shared element size in bytes.
    /// Used by Receiver_recv to allocate correctly-sized output buffers.
    pub channel_elem_size: Option<i64>,
    /// C1/C2: resource_id local for consumption cancellation.
    /// Set when an ensure registers this variable as its receiver.
    pub resource_id: Option<LocalId>,
    /// Function parameter declared `mutate`. Whole-value reassignment must
    /// flow back through the param's pointer (mem.borrowing/M-rules), so
    /// `p = expr` lowers to a Store(*p, ...) instead of Assign(p, ...).
    pub is_mutate_param: bool,
    /// #270: a scalar (Copy) `mutate` param passed by pointer. The local holds an
    /// address; reads of the bare name load through it, writes store through it,
    /// using this recorded scalar type for the access size. `None` for normal
    /// locals and aggregate mutate params (which are already pointers used via
    /// field access, not bare loads).
    pub scalar_mutate_ptr: Option<MirType>,
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
    /// Name of the function being lowered (for closure naming)
    parent_name: String,
    /// Variable names known to hold closure values
    closure_locals: std::collections::HashSet<String>,
    /// Variable name → supplementary metadata (type prefix, full type, elem type, channel size).
    /// Keys may exist here without a corresponding entry in `locals` (e.g. module imports).
    local_meta: HashMap<String, LocalMeta>,
    /// W2a/W2b: Active `with` pool bindings for re-resolution after pool mutators.
    /// Maps pool variable name → Vec of (handle_local, binding_local, pool_local).
    with_pool_bindings: HashMap<String, Vec<(LocalId, LocalId, LocalId)>>,
    /// When set, `return expr` inside an inlined closure body assigns to the
    /// target local and jumps to the continuation block instead of emitting
    /// MirTerminator::Return.  Used by fold/reduce/etc.
    inline_return_target: Option<(LocalId, BlockId)>,
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
    /// `for mutate` bodies currently being lowered, innermost last.
    ///
    /// `for mutate x in v` writes the binding back into the collection at the end
    /// of each iteration, and `continue`/`break` reach that through dedicated
    /// writeback blocks. Leaving the body by returning doesn't go through any
    /// block, so the iteration's write was simply dropped — `return item` handed
    /// back the new value and left the collection unchanged (#650). Every function
    /// exit point drains this first, the same way it drains `ensure_stack`.
    mutate_writebacks: Vec<MutateWriteback>,
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
    pending_module_consts: HashMap<String, (Expr, Option<String>)>,
    /// Module-level consts that own a global slot, and the type stored there.
    /// A reference loads from the slot instead of re-running the initializer.
    const_slots: HashMap<String, MirType>,
    /// Set while lowering a const's init thunk. Inside its own thunk the const
    /// is a definition, not a reference — that one place runs the initializer.
    const_init_target: Option<String>,
    /// Declared type of the struct field currently being initialised, as written.
    /// `Map.new()` needs its key/value sizes, and the checker doesn't type the
    /// nodes of a stdlib body — `Headers { entries: Map.new() }` had no type on
    /// that call, so the map was built with 8-byte slots and a `string` value lost
    /// half of its 16 bytes.
    field_type_hint: Option<String>,
    /// Element type of a Vec built by a fused `collect()`, keyed by the local
    /// holding it. The fused loop is the only place that type exists — the
    /// checker leaves `collect()`'s element an inference variable — and a binding
    /// needs it so `for v in page` knows what it is iterating.
    collected_elem_types: HashMap<LocalId, MirType>,
    /// Stack of enclosing `try { … } catch e => …` handlers (innermost last).
    /// A `try` inside one of these blocks jumps to the handler instead of
    /// returning from the function (ER18).
    catch_frames: Vec<CatchFrame>,
    /// Active `comptime for` loop-binding names and the field they're currently
    /// bound to (innermost last, for nesting). `field` here isn't a runtime
    /// value — `field.name`/`value.(field.name)` splice this directly instead
    /// of going through the normal local/struct-layout lookup (CT48/CT49).
    comptime_for_bindings: Vec<(String, ReflectFieldConst)>,
}

/// One field's compile-time-known metadata inside an unrolled `comptime for
/// field in reflect.fields<T>()` body (CT48–CT54). Mirrors the interpreter's
/// FieldInfo shape (rask-interp/src/stdlib/reflect.rs) so native and interp
/// agree; `is_public` has no source in `FieldLayout` so it defaults to `true`.
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
            || self.ctx.comptime_globals.contains_key(name)
    }

    /// Record a module-level const's box type from its initializer, e.g.
    /// `Shared.new(Metrics { … })` → prefix `Shared`, full type `Shared<Metrics>`.
    ///
    /// A cross-module const reference gets left an inference var by the checker,
    /// so guard access can't read the inner type off the use site — the
    /// initializer is the only place it's concrete.
    pub(crate) fn record_module_const_meta(&mut self, name: &str, init: &Expr) {
        let ExprKind::MethodCall { object, args, .. } = &init.kind else { return };
        let ExprKind::Ident(type_name) = &object.kind else { return };
        let Some(prefix) = MirContext::type_prefix_str(type_name) else { return };
        if let Some(inner) = args.first().and_then(|a| {
            self.ctx.lookup_raw_type(a.expr.id)
                .and_then(|t| MirContext::type_prefix(t, self.ctx.type_names))
        }) {
            self.meta_mut(name).full_type = Some(format!("{}<{}>", prefix, inner));
        }
        self.meta_mut(name).type_prefix = Some(prefix);
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
        if mir_ty_is_aggregate(&ty) {
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

        let stored = if mir_ty_is_aggregate(ty) {
            // The value sits in this thunk's frame, which is gone by the time
            // anything reads the slot. Copy it to the heap and share that.
            let size = self.aggregate_alloc_size(ty);
            let heap = self.builder.alloc_temp(MirType::Ptr);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
                dst: Some(heap),
                func: FunctionRef::internal("rask_alloc".to_string()),
                args: vec![MirOperand::Constant(MirConst::Int(size as i64))],
            }));
            let mut off = 0u32;
            while off < size {
                let word = self.builder.alloc_temp(MirType::I64);
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                    dst: word,
                    rvalue: MirRValue::Field {
                        base: value.clone(),
                        field_index: 0,
                        byte_offset: Some(off),
                        access: FieldAccess::Sized(8),
                    },
                }));
                self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                    addr: heap,
                    offset: off,
                    value: MirOperand::Local(word),
                    store_size: Some(8),
                }));
                off += 8;
            }
            MirOperand::Local(heap)
        } else {
            value
        };

        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr,
            offset: 0,
            value: stored,
            store_size: None,
        }));
    }

    /// Byte size to copy for an aggregate const, rounded up to whole words.
    fn aggregate_alloc_size(&self, ty: &MirType) -> u32 {
        let raw = match ty {
            MirType::Struct(StructLayoutId { id, .. }) => self
                .ctx
                .struct_layouts
                .get(*id as usize)
                .map(|l| l.size)
                .unwrap_or(8),
            MirType::Enum(EnumLayoutId { id, .. }) => self
                .ctx
                .enum_layouts
                .get(*id as usize)
                .map(|l| l.size)
                .unwrap_or(8),
            other => other.size(),
        };
        std::cmp::max(raw, 8).div_ceil(8) * 8
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
        let bare = name.split('<').next().unwrap_or(&name).trim().to_string();
        Some((bare, args))
    }

    /// What a collection holds: the element of a `Vec`/`Pool`, the *value* of a
    /// `Map`. Indexing any of them yields this type.
    pub(crate) fn collection_elem_of_checker_type(&self, ty: &Type) -> Option<MirType> {
        let (name, args) = self.generic_head(ty)?;
        // A box holding a collection is transparent here: `Mutex<Map<K, V>>`
        // holds what its `Map` holds, and `self.counters.lock().get(k)` asks
        // exactly that.
        if matches!(name.as_str(), "Mutex" | "Shared" | "Cell" | "Owned") {
            let rask_types::GenericArg::Type(inner) = args.first()? else { return None };
            return self.collection_elem_of_checker_type(inner);
        }
        let arg = match name.as_str() {
            // Iterator is here because a `for` source is often one, and it
            // carries its element type the same way.
            "Vec" | "Pool" | "Iterator" => args.first()?,
            // `m[k]` and `m.get(k)` yield V, not K.
            "Map" => args.get(1)?,
            _ => return None,
        };
        match arg {
            // An unresolved variable is not an answer — see below.
            rask_types::GenericArg::Type(inner) if matches!(**inner, Type::Var(_)) => None,
            rask_types::GenericArg::Type(inner) => Some(self.ctx.type_to_mir(inner)),
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

    /// The struct layout of whatever an expression evaluates to, when it's a
    /// struct. Walks field chains (`a.b.c`) so a nested field's declared type
    /// stays reachable.
    pub(crate) fn struct_layout_of_expr(&self, expr: &Expr) -> Option<rask_mono::StructLayout> {
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

    /// Base type name of an indexed object — `"Pool"` for both `tasks[h]` and
    /// `self.tasks[h]`. Generics are stripped: `Pool<Task>` → `Pool`.
    ///
    /// A variable's tracked prefix first (it survives cases the checker leaves as
    /// an inference variable), then the checker's type, which is the only source
    /// for a field or any other non-`Ident` object.
    pub(crate) fn index_object_base(&self, object: &Expr) -> Option<String> {
        let prefix = if let ExprKind::Ident(var_name) = &object.kind {
            self.meta(var_name).and_then(|m| m.type_prefix.clone())
        } else {
            None
        }
        .or_else(|| {
            self.ctx
                .lookup_raw_type(object.id)
                .and_then(|ty| MirContext::type_prefix(ty, self.ctx.type_names))
        })?;
        Some(prefix.split('<').next().unwrap_or(&prefix).trim().to_string())
    }

    /// Write every open `for mutate` binding back into its collection, innermost
    /// first. Call this at any point that leaves the body without passing through
    /// the loop's own writeback blocks — which means every function exit.
    pub(crate) fn emit_mutate_writebacks(&mut self) {
        for wb in self.mutate_writebacks.clone().into_iter().rev() {
            self.emit_one_mutate_writeback(&wb);
        }
    }

    /// LP13: a Vec element goes back by index, a Map entry by key.
    pub(crate) fn emit_one_mutate_writeback(&mut self, wb: &MutateWriteback) {
        let (func, args) = match wb.map_value {
            Some(value) => (
                "Map_set",
                vec![
                    MirOperand::Local(wb.collection),
                    MirOperand::Local(wb.binding),
                    MirOperand::Local(value),
                ],
            ),
            None => (
                "Vec_set",
                vec![
                    MirOperand::Local(wb.collection),
                    MirOperand::Local(wb.index),
                    MirOperand::Local(wb.binding),
                ],
            ),
        };
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::internal(func.to_string()),
            args,
        }));
    }

    /// Terminate the current block as a function return.
    ///
    /// One place, because there are four of them — `return`, `try`'s error path
    /// (twice), and the implicit tail — and each one has to run the same exits:
    /// pending `for mutate` writebacks, then the ensure chain.
    pub(crate) fn terminate_return(&mut self, value: Option<MirOperand>) {
        self.emit_mutate_writebacks();
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

    /// The `T` in a declared `Owned<T>`, or `None` if the type isn't one.
    ///
    /// `Owned<T>` is the only box whose slot holds a pointer to a value the
    /// program otherwise treats as a plain `T` (OW5) — the rest of the family is
    /// opaque, reached through `with`. So it's the only one where a store or a
    /// read has to cross the boundary.
    pub(crate) fn owned_payload(&self, ty: &rask_types::Type) -> Option<rask_types::Type> {
        let (name, args) = self.generic_head(ty)?;
        if name != "Owned" {
            return None;
        }
        match args.first()? {
            rask_types::GenericArg::Type(inner) => Some(inner.as_ref().clone()),
            _ => None,
        }
    }

    /// Heap-allocate a copy of `val` and hand back the pointer — what `own` means.
    ///
    /// A scalar needs no box: it already fits the 8-byte slot, and a scalar can't
    /// make a type recursive, so `Owned<i64>` staying transparent costs nothing.
    /// An aggregate is the case that matters, and its pointer is also its
    /// representation, so nothing downstream has to know it moved.
    pub(crate) fn box_into_owned(&mut self, val: MirOperand, val_ty: &MirType) -> MirOperand {
        if !val_ty.passed_by_address() {
            return val;
        }
        let size = val_ty.size() as i64;
        let heap = self.builder.alloc_temp(MirType::Ptr);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Call {
            dst: Some(heap),
            func: FunctionRef::internal("rask_alloc".to_string()),
            args: vec![MirOperand::Constant(MirConst::Int(size))],
        }));
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
            addr: heap,
            offset: 0,
            value: val,
            store_size: Some(size as u32),
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

    /// The generation-checked address of `pool[handle]`'s data.
    ///
    /// `PoolCheckedAccess` means one thing: the destination holds the slot's
    /// address. Reading a scalar element out of it is a separate, visible load.
    /// It used to mean "address or value, work out which from the destination's
    /// declared type", and codegen picked address every time — which for a scalar
    /// read is a Cranelift panic, not a wrong answer (#719).
    ///
    /// `as_ty` is how the destination local is declared. An aggregate is declared
    /// with its own type — that representation already *is* an address, and
    /// declaring it `Ptr` instead loses the type for everything downstream, which
    /// printed a `Pool<string>` element as its address. Anything that only stores
    /// through the local, or loads a scalar out of it, uses `Ptr`.
    pub(crate) fn pool_slot_addr(
        &mut self,
        pool: LocalId,
        handle: LocalId,
        as_ty: MirType,
    ) -> LocalId {
        let slot_addr = self.builder.alloc_temp(as_ty);
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::PoolCheckedAccess {
            dst: slot_addr,
            pool,
            handle,
        }));
        slot_addr
    }

    /// True when `object[..]` indexes a `Pool`. Decides both the
    /// `PoolCheckedAccess` lowering and whether a `with` binding aliases the slot
    /// — they have to agree, so they read the same answer.
    pub(crate) fn index_object_is_pool(&self, object: &Expr) -> bool {
        self.index_object_base(object).as_deref() == Some("Pool")
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
    /// `Vec`/`Pool`, value for a `Map`.
    ///
    /// Tries the checker's type for the expression, then the declared type of a
    /// struct field, then `Vec`-specific tracking. The field case is the one that
    /// matters most: push-tracking is per-function and the checker doesn't type
    /// every node, so a collection that arrived as a field of something built
    /// elsewhere has only its declaration to go on.
    pub(crate) fn collection_elem_of_expr(&self, expr: &Expr) -> Option<MirType> {
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
            if let Some(elem) = self.ctx.comptime_globals.get(name)
                .and_then(|g| g.elem_type.as_deref())
            {
                return Some(self.ctx.resolve_type_str(elem));
            }
        }
        // A view over a collection holds what the collection holds, so ask the
        // receiver. `for prime in PRIMES.iter()` types as `Vec<?>` — the checker
        // leaves the element a variable — while `PRIMES` itself knows.
        // `iter`/`values` view a collection; `lock`/`read`/`write` unwrap a box
        // around one. Either way the receiver is the thing that knows.
        if let ExprKind::MethodCall { object, method, .. } = &expr.kind {
            if matches!(
                method.as_str(),
                "iter" | "values" | "into_iter" | "lock" | "read" | "write"
            ) {
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
                let base = prefix.split('<').next().unwrap_or(&prefix).trim();
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
                Type::Array { elem, .. } | Type::Slice(elem)
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
                let inner = full.strip_prefix("Map<")?.strip_suffix('>')?;
                let comma = find_top_level_comma(inner)?;
                Some(self.ctx.resolve_type_str(inner[comma + 1..].trim()))
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

    /// Lower a nominal newtype construction — `Id { value: 5 }` or `Id(5)` — to
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
        Ok(Some((op, self.ctx.resolve_type_str(name))))
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
        use rask_ast::coercion::CoercionSite;

        // Which positions can put a value on the *error* branch rather than
        // wrapping it as success. ER9 gives that to `return`: a value whose type
        // is `E` goes to err, picked by type, and disjointness (ER3) makes it
        // unambiguous. Elsewhere ER11 means a bare `E` never reaches here for a
        // non-optional sum — the checker rejected it — so a value that happens to
        // equal the error type at those positions is the payload, not an error.
        //
        // Exhaustive on purpose: a new position has to say which it is.
        let err_branch_by_type = match site {
            CoercionSite::Return | CoercionSite::CatchArm => true,
            CoercionSite::AnnotatedBinding
            | CoercionSite::Argument
            | CoercionSite::StructField => false,
        };

        let (dst_layers, _) = Self::wrapper_layers(dst_ty);
        let (src_layers, _) = Self::wrapper_layers(src_ty);
        if dst_layers.len() <= src_layers.len() {
            return val;
        }

        // A `Handle<T>?` is a niche: the handle is the value and `none` is the
        // all-ones sentinel, so there's no tag to write.
        if matches!(dst_ty, MirType::Option(inner) if matches!(**inner, MirType::Handle)) {
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
            let (tag_offset, payload_offset, is_result) = match &dst_layers[depth] {
                WrapLayer::Option => (
                    rask_mono::abi::OPTION_TAG_OFFSET,
                    rask_mono::abi::OPTION_PAYLOAD_OFFSET,
                    false,
                ),
                WrapLayer::Result { .. } => (
                    rask_mono::abi::RESULT_TAG_OFFSET,
                    rask_mono::abi::RESULT_PAYLOAD_OFFSET,
                    true,
                ),
            };
            // tag 0 — Some for an Option, Ok for a Result.
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: slot,
                offset: tag_offset,
                value: MirOperand::Constant(MirConst::Int(0)),
                store_size: Some(8),
            }));
            if is_result {
                // ER15 origin stays zero on the success side.
                for off in [
                    rask_mono::abi::RESULT_ORIGIN_FILE_OFFSET,
                    rask_mono::abi::RESULT_ORIGIN_LINE_OFFSET,
                ] {
                    self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: slot,
                        offset: off,
                        value: MirOperand::Constant(MirConst::Int(0)),
                        store_size: Some(8),
                    }));
                }
            }
            let (payload_op, store_size) = if cur_ty.passed_by_address() {
                // A wrapper or struct payload lives at its own address; the store
                // is a memcpy, which is why aggregates never had a depth problem.
                (cur_op, self.aggregate_alloc_size(&cur_ty))
            } else {
                // A scalar payload fills the whole 8-byte slot: floats as f64,
                // integers full-width. The read side picks the payload apart by
                // that same rule, so storing a narrow f32 here left the reader
                // taking 8 bytes of a 4-byte write (#629's rule, one layer up).
                (self.widen_scalar_payload(cur_op, &cur_ty), 8)
            };
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                addr: slot,
                offset: payload_offset,
                value: payload_op,
                store_size: Some(store_size),
            }));
            cur_op = MirOperand::Local(slot);
            cur_ty = layer_ty;
        }
        cur_op
    }

    /// Widen a scalar to what the payload slot holds it as — `rask_mono::abi`
    /// owns that rule, this just applies it. Returns the operand unchanged when
    /// it is already that wide.
    fn widen_scalar_payload(&mut self, op: MirOperand, ty: &MirType) -> MirOperand {
        let is_float = matches!(ty, MirType::F32 | MirType::F64);
        let target = match rask_mono::abi::payload_repr(is_float, ty.passed_by_address()) {
            rask_mono::abi::PayloadRepr::InPlace => return op,
            rask_mono::abi::PayloadRepr::Float64 => MirType::F64,
            rask_mono::abi::PayloadRepr::IntFullWidth => MirType::I64,
        };
        if ty == &target || ty.size() >= rask_mono::abi::PAYLOAD_SLOT_BYTES {
            return op;
        }
        let tmp = self.builder.alloc_temp(target.clone());
        self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
            dst: tmp,
            rvalue: MirRValue::Cast {
                value: op,
                target_ty: target,
            },
        }));
        MirOperand::Local(tmp)
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
    /// C1/C2: check if an expression is a consuming method call on an ensure
    /// receiver. If so, emit ResourceConsume to cancel the ensure at cleanup time.
    fn check_resource_consume(&mut self, expr: &rask_ast::expr::Expr) {
        use rask_ast::expr::ExprKind;
        if let ExprKind::MethodCall { object, method, .. } = &expr.kind {
            if let ExprKind::Ident(receiver_name) = &object.kind {
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

    /// Extract the receiver variable name from an ensure body.
    /// For `ensure X.method()`, returns Some("X").
    fn extract_ensure_receiver(body: &[rask_ast::stmt::Stmt]) -> Option<String> {
        use rask_ast::expr::ExprKind;
        use rask_ast::stmt::StmtKind;
        if let Some(first) = body.first() {
            if let StmtKind::Expr(expr) = &first.kind {
                if let ExprKind::MethodCall { object, .. } = &expr.kind {
                    if let ExprKind::Ident(name) = &object.kind {
                        return Some(name.clone());
                    }
                }
            }
        }
        None
    }

    /// True for types whose MIR value is a single 8-byte pointer to the live
    /// data — an aggregate's stack address, or a heap pointer (Vec/Map/String).
    /// Capturing such a value by value is capture-by-reference: the ensure hook
    /// sees later mutations (U2). Scalars are excluded (a value copy would go
    /// stale), as are fat pointers (Slice/TraitObject — 16 bytes, don't fit an
    /// 8-byte env slot).
    fn is_ref_capturable(ty: &MirType) -> bool {
        matches!(
            ty,
            MirType::Struct(_)
                | MirType::Enum(_)
                | MirType::Array { .. }
                | MirType::Tuple(_)
                | MirType::Ptr
                | MirType::String
                | MirType::Handle
        )
    }

    /// Reify an ensure body as a runtime hook thunk so the cleanup runs if the
    /// scope unwinds on a native panic (ctrl.panic/U1). Returns
    /// `(thunk_name, captures)` on success; `None` keeps inline-only behavior.
    ///
    /// Scoped first cut: only a single-expression body whose free variables are
    /// all aggregate locals (captured by reference — their MIR value is the
    /// address). The optional `resource` id is captured by value so the thunk
    /// can skip a cleanup whose receiver was already consumed (C1). Everything
    /// else (else-handlers, scalar captures, multi-statement bodies) returns
    /// `None` — the ensure simply won't run on a native panic, never miscompiles.
    fn try_reify_ensure_hook(
        &mut self,
        body: &[rask_ast::stmt::Stmt],
        else_handler: &Option<(String, Vec<rask_ast::stmt::Stmt>)>,
        resource: Option<LocalId>,
    ) -> Option<(String, Vec<crate::stmt::ClosureCapture>)> {
        use rask_ast::stmt::StmtKind;

        if else_handler.is_some() {
            return None;
        }
        let expr = match body {
            [only] => match &only.kind {
                StmtKind::Expr(e) => e,
                _ => return None,
            },
            _ => return None,
        };

        // Free variables must all be aggregates (captured by reference).
        let free = self.collect_free_vars(expr, &[]);
        if free.iter().any(|(_, _, ty)| !Self::is_ref_capturable(ty)) {
            return None;
        }

        // Ordered captures: aggregate free vars (by ref), then the resource id
        // (by value) when present.
        struct Cap {
            outer: LocalId,
            name: String,
            ty: MirType,
            by_ref: bool,
        }
        let mut caps: Vec<Cap> = free
            .iter()
            .map(|(name, id, ty)| Cap {
                outer: *id,
                name: name.clone(),
                ty: ty.clone(),
                by_ref: true,
            })
            .collect();
        let res_index = resource.map(|res| {
            caps.push(Cap {
                outer: res,
                name: "__ensure_res".to_string(),
                ty: MirType::I64,
                by_ref: false,
            });
            caps.len() - 1
        });

        let thunk_name = format!("{}__ensure_thunk_{}", self.parent_name, self.closure_counter);
        self.closure_counter += 1;

        let mut thunk_builder = BlockBuilder::new(thunk_name.clone(), MirType::Void);
        let env_param = thunk_builder.add_param("__env".to_string(), MirType::Ptr);

        let mut thunk_locals: HashMap<String, (LocalId, MirType)> = HashMap::new();
        let mut thunk_res_local: Option<LocalId> = None;
        for (i, cap) in caps.iter().enumerate() {
            let dst = thunk_builder.alloc_local(cap.name.clone(), cap.ty.clone());
            thunk_builder.push_stmt(MirStmt::dummy(MirStmtKind::LoadCapture {
                dst,
                env_ptr: env_param,
                offset: (i as u32) * 8,
                by_ref: cap.by_ref,
            }));
            if Some(i) == res_index {
                thunk_res_local = Some(dst);
            } else {
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

        // Lower the body expression into the thunk (reuses method resolution).
        // The pending module consts are saved along with the locals: the thunk
        // is its own MIR function and materialises its own copy of any const it
        // touches, but that must not consume the outer function's entry — the
        // cleanup path lowers this same expression again, and with the entry
        // gone the reference compiled to a call named after the const (#403).
        let saved_builder = std::mem::replace(&mut self.builder, thunk_builder);
        let saved_locals = std::mem::replace(&mut self.locals, thunk_locals);
        let saved_pending = self.pending_module_consts.clone();
        let saved_loop_stack = std::mem::take(&mut self.loop_stack);
        let body_result = self.lower_expr(expr);
        thunk_builder = std::mem::replace(&mut self.builder, saved_builder);
        self.locals = saved_locals;
        self.pending_module_consts = saved_pending;
        self.loop_stack = saved_loop_stack;

        if body_result.is_err() {
            return None;
        }
        if thunk_builder.current_block_unterminated() {
            thunk_builder.terminate(MirTerminator::dummy(MirTerminatorKind::Return { value: None }));
        }

        let thunk_fn = thunk_builder.finish();
        self.func_sigs.insert(thunk_name.clone(), FuncSig { ret_ty: MirType::Void, scalar_mutate_params: Vec::new(), aggregate_mutate_params: Vec::new(), ret_vec_elem: None, param_ty_strs: Vec::new() });
        self.synthesized_functions.push(thunk_fn);

        let captures = caps
            .iter()
            .enumerate()
            .map(|(i, c)| crate::stmt::ClosureCapture {
                local_id: c.outer,
                offset: (i as u32) * 8,
                size: 8,
            })
            .collect();
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
                context_clauses: Vec::new(),
                body: Vec::new(),
                is_pub: false,
                is_private: true,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: Vec::new(),
                doc: None,
                span: Span::new(0, 0),
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
            if matches!(c.init.kind, ExprKind::Comptime { .. })
                || ctx.comptime_globals.contains_key(&c.name)
            {
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
    /// `fallback::i64_fallback`. Guessing i64 there is right only for a payload
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

        let ret_ty = ctx.fn_ret_ty(&fn_decl.name, fn_decl.ret_ty.as_deref());

        // Build function signature table from all declarations
        let mut func_sigs = HashMap::new();
        for d in all_decls {
            match &d.kind {
                DeclKind::Fn(f) => {
                    let sig_ret = ctx.fn_ret_ty(&f.name, f.ret_ty.as_deref());
                    func_sigs.insert(f.name.clone(), FuncSig {
                        ret_ty: sig_ret,
                        scalar_mutate_params: scalar_mutate_params(&f.params, ctx),
                        aggregate_mutate_params: aggregate_mutate_params(&f.params, ctx),
                        ret_vec_elem: vec_elem_of_type_str(f.ret_ty.as_deref(), ctx),
                        param_ty_strs: f.params.iter().map(|p| Some(p.ty.clone())).collect(),
                    });
                }
                DeclKind::Extern(ext) => {
                    let sig_ret = ext
                        .ret_ty
                        .as_deref()
                        .map(|s| ctx.resolve_type_str(s))
                        .unwrap_or(MirType::Void);
                    func_sigs.insert(ext.name.clone(), FuncSig { ret_ty: sig_ret, scalar_mutate_params: Vec::new(), aggregate_mutate_params: Vec::new(), ret_vec_elem: None, param_ty_strs: Vec::new() });
                }
                DeclKind::Impl(impl_decl) => {
                    for m in &impl_decl.methods {
                        let qualified = format!("{}_{}", impl_decl.target_ty, m.name);
                        let sig_ret = m
                            .ret_ty
                            .as_deref()
                            .map(|s| ctx.resolve_type_str(s))
                            .unwrap_or(MirType::Void);
                        func_sigs.insert(qualified, FuncSig {
                            ret_ty: sig_ret,
                            scalar_mutate_params: scalar_mutate_params(&m.params, ctx),
                            aggregate_mutate_params: aggregate_mutate_params(&m.params, ctx),
                            ret_vec_elem: vec_elem_of_type_str(m.ret_ty.as_deref(), ctx),
                            param_ty_strs: m.params.iter().map(|p| Some(p.ty.clone())).collect(),
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
                ret_ty: ret_category_to_mir_type_in(&meta.ret_category, Some(ctx)),
                scalar_mutate_params: Vec::new(),
                aggregate_mutate_params: Vec::new(),
                ret_vec_elem: None,
                param_ty_strs: Vec::new(),
            });
        }

        // C1/C2: collect qualified method names with `take self` for
        // consumption cancellation. When such a method is called on an
        // ensure receiver, the ensure is cancelled.
        let mut take_self_methods = std::collections::HashSet::new();
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
                        if m.params.first().map_or(false, |p| p.name == "self" && p.is_take) {
                            let qualified = format!("{}_{}", impl_decl.target_ty, m.name);
                            take_self_methods.insert(qualified);
                        }
                        if method_mutates_self(m, ctx) {
                            mutate_self_methods
                                .insert(format!("{}_{}", impl_decl.target_ty, m.name));
                        }
                    }
                }
                DeclKind::Fn(f) => {
                    // After monomorphization, impl methods become standalone functions
                    // named "Type_method" with a `take self` first parameter.
                    if f.params.first().map_or(false, |p| p.name == "self" && p.is_take) {
                        take_self_methods.insert(f.name.clone());
                    }
                    if method_mutates_self(f, ctx) {
                        mutate_self_methods.insert(f.name.clone());
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
            parent_name: func_name,
            closure_locals: std::collections::HashSet::new(),
            local_meta: HashMap::new(),
            with_pool_bindings: HashMap::new(),
            inline_return_target: None,
            inline_return_taken: None,
            ensure_stack: Vec::new(),
            mutate_writebacks: Vec::new(),
            elem_writebacks: Vec::new(),
            take_self_methods,
            mutate_self_methods,
            ensure_receivers: HashMap::new(),
            pending_module_consts: HashMap::new(),
            const_slots: HashMap::new(),
            const_init_target: const_init.map(|(n, _)| n.to_string()),
            collected_elem_types: HashMap::new(),
            field_type_hint: None,
            catch_frames: Vec::new(),
            comptime_for_bindings: Vec::new(),
        };

        // Resolve Self type from function name: "Document_delete_line" → "Document"
        let self_type_name: Option<String> = fn_decl.params.iter()
            .any(|p| p.ty == "Self")
            .then(|| {
                // Extract the type name prefix from the qualified function name
                lowerer.parent_name.split('_').next().map(|s| s.to_string())
            })
            .flatten();

        // Add parameters
        for param in &fn_decl.params {
            let param_ty_str = if param.ty == "Self" {
                self_type_name.as_deref().unwrap_or(&param.ty)
            } else {
                &param.ty
            };
            // Hidden context params carry `&Pool<T>` (comp.hidden-params/SIG1).
            // Rask has no reference types at the backend — a pool is an opaque
            // handle passed by value — so strip the `&` and lower the pointee.
            let param_ty_str = param_ty_str.trim_start_matches('&');
            let param_ty = ctx.resolve_type_str(param_ty_str);
            // #270: a scalar `mutate` param is passed by pointer so the callee can
            // write back through it. Register the param local as a pointer; reads
            // load and writes store through it, keyed by the recorded scalar type.
            let scalar_mutate = param.is_mutate
                && !crate::lower::stmt::mutate_param_by_pointer(&param_ty);
            let local_ty = if scalar_mutate { MirType::Ptr } else { param_ty.clone() };
            let local_id = lowerer.builder.add_param(param.name.clone(), local_ty.clone());
            lowerer.locals.insert(param.name.clone(), (local_id, local_ty));
            // Set type prefix for parameters so method calls qualify correctly.
            // mir_type_name handles Struct/Enum/String/primitives; type_prefix_from_str
            // catches Ptr types like Vec<T>, Map<K,V> from the annotation string.
            {
                let prefix = lowerer.mir_type_name(&param_ty)
                    .or_else(|| type_prefix_from_str(param_ty_str));
                let meta = lowerer.local_meta.entry(param.name.clone()).or_default();
                if param.is_mutate {
                    meta.is_mutate_param = true;
                }
                if scalar_mutate {
                    meta.scalar_mutate_ptr = Some(param_ty.clone());
                }
                if let Some(p) = prefix {
                    meta.type_prefix = Some(p);
                }
                // Store full annotation for generic types (Shared<T>, Channel<T>, etc.)
                if param_ty_str.contains('<') {
                    meta.full_type = Some(param_ty_str.to_string());
                    // Track collection element types so for-loop iteration resolves correctly.
                    // e.g., Vec<Inline> → collection_elem_types["children"] = Struct(Inline)
                    if let Some(elem_str) = param_ty_str.strip_prefix("Vec<").and_then(|s| s.strip_suffix('>')) {
                        let elem_mir = ctx.resolve_type_str(elem_str);
                        meta.elem_type = Some(elem_mir);
                    }
                    if let Some(elem_str) = param_ty_str.strip_prefix("Pool<").and_then(|s| s.strip_suffix('>')) {
                        let elem_mir = ctx.resolve_type_str(elem_str);
                        meta.elem_type = Some(elem_mir);
                    }
                }
            }

            // Function-type params (|args| -> ret) are closures passed as arguments.
            // Register them so call sites emit ClosureCall instead of Call.
            // Parser normalizes |T| -> R to "func(T) -> R", so check both forms.
            if param_ty_str.starts_with('|') || param_ty_str.starts_with("func(") {
                lowerer.closure_locals.insert(param.name.clone());
                let ret_ty = if let Some(arrow_pos) = param_ty_str.rfind("-> ") {
                    let ret_str = param_ty_str[arrow_pos + 3..].trim();
                    ctx.resolve_type_str(ret_str)
                } else {
                    MirType::Void
                };
                lowerer.func_sigs.insert(param.name.clone(), FuncSig { ret_ty, scalar_mutate_params: Vec::new(), aggregate_mutate_params: Vec::new(), ret_vec_elem: None, param_ty_strs: Vec::new() });
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
                if let Some((op, ty)) = lowerer.try_eval_const_init(&c.init, c.ty.as_deref()) {
                    let local_id = lowerer.builder.alloc_local(c.name.clone(), ty.clone());
                    lowerer.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                        dst: local_id,
                        rvalue: MirRValue::Use(op),
                    }));
                    lowerer.locals.insert(c.name.clone(), (local_id, ty));
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
                    lowerer.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: wrap_local,
                        offset: 0,
                        value: MirOperand::Constant(MirConst::Int(0)),
                        store_size: Some(8),
                    }));
                    // Zero the origin-file and origin-line fields so they
                    // don't carry stale bytes when the caller copies the
                    // whole slot.
                    lowerer.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: wrap_local,
                        offset: 8,
                        value: MirOperand::Constant(MirConst::Int(0)),
                        store_size: Some(8),
                    }));
                    lowerer.builder.push_stmt(MirStmt::dummy(MirStmtKind::Store {
                        addr: wrap_local,
                        offset: 16,
                        value: MirOperand::Constant(MirConst::Int(0)),
                        store_size: Some(8),
                    }));
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
    fn try_eval_const_init(&self, expr: &Expr, ty_hint: Option<&str>) -> Option<(MirOperand, MirType)> {
        match &expr.kind {
            ExprKind::Int(val, suffix) => {
                let ty = if let Some(hint) = ty_hint {
                    self.ctx.resolve_type_str(hint)
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
                        _ => MirType::I64,
                    }
                };
                Some((MirOperand::Constant(MirConst::Int(*val)), ty))
            }
            ExprKind::Float(val, _) => {
                let ty = if let Some(hint) = ty_hint {
                    self.ctx.resolve_type_str(hint)
                } else {
                    MirType::F64
                };
                Some((MirOperand::Constant(MirConst::Float(*val)), ty))
            }
            ExprKind::String(s) => Some((MirOperand::Constant(MirConst::String(s.clone())), MirType::String)),
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
            Type::Generic { base, args }
                if self.ctx.type_names.get(base).is_some_and(|n| n == "Vec") => args,
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
                Type::Slice(elem) => return Some(self.ctx.type_to_mir(elem)),
                // Pool iteration yields handles (packed i64)
                Type::UnresolvedNamed(n) if n == "Pool" => return Some(MirType::I64),
                Type::UnresolvedGeneric { name, .. } if name == "Pool" => return Some(MirType::I64),
                // Vec<any Trait> yields fat-pointer elements. Only the trait-object
                // case is taken from the checker here: concrete element types are
                // already covered by the tracked elem_type below, but a trait object
                // carries a vtable half that nothing downstream can recover once the
                // binding has been typed as a plain scalar.
                _ => {
                    if let Some(Type::TraitObject { trait_name }) = self.vec_elem_raw_type(ty) {
                        return Some(MirType::TraitObject { trait_name: trait_name.clone() });
                    }
                }
            }
        }

        // After mono, node IDs are fresh — use AST structure heuristics.
        // Functions known to return Vec<string>:
        if let ExprKind::MethodCall { object, method, .. } = &expr.kind {
            // String methods that produce iterators
            match method.as_str() {
                "split" | "split_whitespace" | "lines" => return Some(MirType::String),
                "chars" => return Some(MirType::Char),
                _ => {}
            }
            if let ExprKind::Ident(name) = &object.kind {
                match (name.as_str(), method.as_str()) {
                    ("cli", "args") | ("fs", "read_lines") => return Some(MirType::String),
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
            return Some(val_ty.clone());
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
    /// MirType::Ptr is a legitimate result for Vec/Map/Pool/Channel/etc.,
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
        // Exact identity match wins.
        if let Some(ok) = ok_ty {
            if self.mir_type_name(ok).as_deref() == Some(name) {
                return false;
            }
        }
        if let Some(err) = err_ty {
            if self.mir_type_name(err).as_deref() == Some(name) {
                return true;
            }
            if let MirType::Union(variants) = err {
                if variants.iter().any(|v| self.mir_type_name(v).as_deref() == Some(name)) {
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
            Pattern::Constructor { name, .. } => self.variant_tag(name),
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
            Pattern::TypePat { ty_name, .. } => {
                if self.pattern_is_err_side(ty_name, val_ty) { 1 } else { 0 }
            }
            Pattern::Constructor { name, .. } => self
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
            if self.mir_type_name(side).as_deref() == Some(name) {
                return true;
            }
            if let MirType::Union(variants) = side {
                if variants.iter().any(|v| self.mir_type_name(v).as_deref() == Some(name)) {
                    return true;
                }
            }
        }
        false
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
        // Well-known built-in variant tags
        match name {
            "Some" | "Ok" => 0,
            "None" | "Err" => 1,
            _ => {
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
        }
    }

    /// Check if an expression's type is niche-optimized Option<Handle<T>>.
    fn is_niche_option_expr(&self, expr: &Expr) -> bool {
        self.ctx.lookup_raw_type(expr.id)
            .map(|ty| is_niche_option_handle(ty))
            .unwrap_or(false)
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
        self.is_niche_option_expr(expr)
            || matches!(ty, MirType::Option(inner) if **inner == MirType::Handle)
    }

    /// `option_is_niche` for a value that's already lowered — reads the MIR
    /// type off the operand's local.
    pub(crate) fn option_operand_is_niche(&self, expr: &Expr, op: &MirOperand) -> bool {
        if self.is_niche_option_expr(expr) {
            return true;
        }
        match op {
            MirOperand::Local(id) => self
                .builder
                .local_type(*id)
                .is_some_and(|t| matches!(t, MirType::Option(inner) if *inner == MirType::Handle)),
            _ => false,
        }
    }

    /// Emit a tag-equivalent check for an option value.
    ///
    /// Returns a local that is 0 for Some, non-zero for None — matching
    /// the tag convention used by branches. Works for both niche-optimized
    /// (compare-to-sentinel) and tagged union (EnumTag load) options.
    fn emit_option_tag(&mut self, value: &MirOperand, is_niche: bool) -> LocalId {
        if is_niche {
            let result = self.builder.alloc_temp(MirType::U8);
            self.builder.push_stmt(MirStmt::dummy(MirStmtKind::Assign {
                dst: result,
                rvalue: MirRValue::BinaryOp {
                    op: crate::operand::BinOp::Eq,
                    left: value.clone(),
                    right: MirOperand::Constant(MirConst::Int(HANDLE_NONE_SENTINEL)),
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

    /// Byte offset for extracting a scalar payload from a tagged Result/Option.
    /// Scalars get the explicit `RESULT_PAYLOAD_OFFSET` so codegen loads the
    /// value; aggregates get `None` so codegen returns the payload address.
    /// (The Option codegen path ignores this offset and uses its own, so a
    /// single Result-shaped offset is correct for both.)
    /// True when a tagged payload of this type lives inline and is reached by
    /// address rather than loaded as a value.
    fn mir_payload_is_aggregate(ty: &MirType) -> bool {
        matches!(
            ty,
            MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::String
        )
    }

    fn payload_byte_offset(&self, payload_ty: &MirType) -> Option<u32> {
        let is_aggregate = Self::mir_payload_is_aggregate(payload_ty);
        if is_aggregate {
            None
        } else {
            Some(crate::types::RESULT_PAYLOAD_OFFSET)
        }
    }

    /// Extract payload from an option value into a new local.
    /// Niche: the handle value IS the payload. Tagged: load field 0.
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
            // For non-aggregate payloads (scalars, Ptr to Vec/Map/etc.), pass an
            // explicit byte_offset so codegen loads the value at
            // RESULT_PAYLOAD_OFFSET instead of falling into the aggregate
            // fast-path that returns the slot address.
            // The container has the last word on how the payload is *stored*.
            // `Map.get` returns `i64?` — a pointer to the map's own value — while
            // the checker calls the payload a `User`. Addressing the container
            // instead of loading that pointer handed back the address of the tag,
            // so `self.users.get(id) ?? …` read a User out of the option's own
            // first bytes and `/users/1` segfaulted.
            let stored_scalar = match &value {
                MirOperand::Local(id) => self.builder.local_type(*id),
                _ => None,
            }
            .and_then(|t| match t {
                MirType::Option(inner) => Some((*inner).clone()),
                _ => None,
            })
            .map(|stored| !Self::mir_payload_is_aggregate(&stored))
            .unwrap_or(false);

            let is_aggregate =
                Self::mir_payload_is_aggregate(&payload_ty) && !stored_scalar;
            let byte_offset = if is_aggregate {
                None
            } else {
                Some(crate::types::RESULT_PAYLOAD_OFFSET)
            };
            MirRValue::Field { base: value, field_index: 0, byte_offset, access: FieldAccess::Word }
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

    /// Bind pattern payload variables into the current scope.
    ///
    /// After confirming a tag match, extracts payload fields from the
    /// enum value and inserts them as named locals.
    fn bind_pattern_payload(
        &mut self,
        pattern: &rask_ast::expr::Pattern,
        value: MirOperand,
        payload_ty: Option<MirType>,
        scrutinee_ty: &MirType,
    ) {
        self.bind_pattern_payload_niche(pattern, value, payload_ty, false, scrutinee_ty);
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
        value: MirOperand,
        payload_ty: Option<MirType>,
        is_niche: bool,
        scrutinee_ty: &MirType,
    ) {
        use rask_ast::expr::Pattern;
        match pattern {
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
                            crate::fallback::i64_fallback("lower/mod:constructor_payload")
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
                    }
                    // Wildcard, Literal in field position — skip binding
                }
            }
            // ER23/ER27: `Type as name` — bind the payload as a fresh local. The
            // only caller (WhileLet) already routed control flow via
            // `pattern_tag_in_type_context` and passes the ok payload type, so
            // bind that directly — no case guess needed.
            Pattern::TypePat { ty_name: _, binding: Some(name) } => {
                let bound_ty = payload_ty.clone().unwrap_or_else(|| {
                    crate::fallback::i64_fallback("lower/mod:typepat_payload")
                });
                let local = self.builder.alloc_local(name.clone(), bound_ty.clone());
                let is_aggregate = matches!(
                    bound_ty,
                    MirType::Struct(_) | MirType::Enum(_) | MirType::Tuple(_) | MirType::String
                );
                let rvalue = if is_niche {
                    MirRValue::Use(value.clone())
                } else {
                    MirRValue::Field {
                        base: value.clone(),
                        field_index: 0,
                        byte_offset: if !is_aggregate {
                            Some(crate::types::RESULT_PAYLOAD_OFFSET)
                        } else {
                            None
                        },
                        access: FieldAccess::Word,
                    }
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
    ) -> Vec<(String, LocalId, MirType)> {
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
        free: &mut Vec<(String, LocalId, MirType)>,
    ) {
        use rask_ast::expr::ExprKind;
        match &expr.kind {
            ExprKind::Ident(name) => {
                if !bound.contains(name) && !seen.contains(name) {
                    if let Some((local_id, ty)) = self.locals.get(name) {
                        seen.insert(name.clone());
                        free.push((name.clone(), *local_id, ty.clone()));
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
            ExprKind::IfLet { expr: inner, pattern, then_branch, else_branch, else_binding } => {
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
            ExprKind::Spawn { body } | ExprKind::BlockCall { body, .. }
            | ExprKind::Loop { body, .. } => {
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
            | ExprKind::StringInterp(..)
            | ExprKind::Char(..) | ExprKind::Bool(..) | ExprKind::Null | ExprKind::None => {}
        }
    }

    fn walk_free_vars_block(
        &self,
        stmts: &[rask_ast::stmt::Stmt],
        bound: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        free: &mut Vec<(String, LocalId, MirType)>,
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
                _ => {}
            }
        }
    }

    fn walk_free_vars_stmt(
        &self,
        stmt: &rask_ast::stmt::Stmt,
        bound: &std::collections::HashSet<String>,
        seen: &mut std::collections::HashSet<String>,
        free: &mut Vec<(String, LocalId, MirType)>,
    ) {
        use rask_ast::stmt::{ForBinding, StmtKind};
        match &stmt.kind {
            StmtKind::Expr(e) => self.walk_free_vars(e, bound, seen, free),
            StmtKind::Mut { init, .. } | StmtKind::Let { init, .. } => {
                self.walk_free_vars(init, bound, seen, free);
            }
            StmtKind::MutTuple { init, .. } | StmtKind::LetTuple { init, .. } => {
                self.walk_free_vars(init, bound, seen, free);
            }
            StmtKind::Return(Some(e)) => self.walk_free_vars(e, bound, seen, free),
            StmtKind::Return(None) => {}
            StmtKind::Assign { target, value } => {
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

/// Parameter type strings of a function-type annotation, e.g.
/// `"func(Request) -> Response"` → `["Request"]`. The parser normalizes
/// `|T| -> R` to the `func(...)` form, so only that spelling needs handling.
pub(crate) fn fn_type_param_strs(ty: &str) -> Option<Vec<String>> {
    let inner = ty.trim().strip_prefix("func(")?;
    // Cut at the paren that closes the parameter list, not at a nested one.
    let mut depth = 1usize;
    let mut end = None;
    for (i, c) in inner.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let params = &inner[..end?];
    if params.trim().is_empty() {
        return Some(Vec::new());
    }
    Some(
        split_top_level_parens(params, ',')
            .iter()
            .map(|p| p.trim().to_string())
            .collect(),
    )
}

/// Element type of a declared `Vec<T>` return type, e.g. `"Vec<SeedSpec>"` →
/// `Struct(SeedSpec)`. `None` for anything that isn't a Vec.
fn vec_elem_of_type_str(ret_ty: Option<&str>, ctx: &MirContext) -> Option<MirType> {
    let inner = ret_ty?
        .trim()
        .strip_prefix("Vec<")?
        .strip_suffix('>')?
        .trim();
    Some(ctx.resolve_type_str(inner))
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

/// Recognize operator method names produced by desugar (e.g. "add", "sub", "eq")
fn operator_method_to_binop(method: &str) -> Option<crate::operand::BinOp> {
    use crate::operand::BinOp as MirBinOp;
    match method {
        "add" => Some(MirBinOp::Add),
        "sub" => Some(MirBinOp::Sub),
        "mul" => Some(MirBinOp::Mul),
        "div" => Some(MirBinOp::Div),
        "rem" => Some(MirBinOp::Mod),
        "eq" => Some(MirBinOp::Eq),
        "lt" => Some(MirBinOp::Lt),
        "gt" => Some(MirBinOp::Gt),
        "le" => Some(MirBinOp::Le),
        "ge" => Some(MirBinOp::Ge),
        "bit_and" => Some(MirBinOp::BitAnd),
        "bit_or" => Some(MirBinOp::BitOr),
        "bit_xor" => Some(MirBinOp::BitXor),
        "shl" => Some(MirBinOp::Shl),
        "shr" => Some(MirBinOp::Shr),
        _ => None,
    }
}

/// Recognize unary operator method names produced by desugar
fn operator_method_to_unaryop(method: &str) -> Option<crate::operand::UnaryOp> {
    use crate::operand::UnaryOp as MirUnaryOp;
    match method {
        "neg" => Some(MirUnaryOp::Neg),
        "bit_not" => Some(MirUnaryOp::BitNot),
        _ => None,
    }
}

/// Map AST binary operator to MIR binary operator (for &&/|| that survive desugar)
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
        UnaryOp::Ref | UnaryOp::Deref => unreachable!(),
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
/// Uppercase-initial names are user-defined types (structs, enums, traits).
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

/// Return type for known stdlib functions that don't return I64.
/// Supplements func_sigs (which only has user-defined functions).
///
/// Primary source: stub-derived metadata. Suffix-based patterns serve as
/// fallbacks for user type methods and methods not yet in stubs.
fn stdlib_return_mir_type(func_name: &str) -> MirType {
    stdlib_return_mir_type_in(func_name, None)
}

/// Same, but able to resolve a named error type against the program's layouts.
///
/// A stub's `T or E` used to lose `E` outright — the metadata parser wrote I64
/// into the error slot no matter what was declared. That gave the Result an
/// 8-byte payload where an error enum needs its own size, and left the match on
/// it with no enum to switch on: `JoinError.Panicked(m)` read the payload as an
/// address (#677). With a context in hand the declared name resolves to its
/// real layout.
fn stdlib_return_mir_type_in(func_name: &str, ctx: Option<&MirContext>) -> MirType {
    // Try stub-derived metadata first
    if let Some(meta) = rask_stdlib::mir_metadata::lookup(func_name) {
        return ret_category_to_mir_type_in(&meta.ret_category, ctx);
    }

    // f64 methods aren't stub-declared — they come from FLOAT_METHODS, which
    // knows each one's shape. Without this they all fell through to i64, so
    // `x.floor().to_string()` picked i64_to_string and printed a truncated
    // integer (#687).
    if let Some(name) = func_name.strip_prefix("f64_") {
        if let Some(m) = rask_stdlib::float_methods::lookup(name) {
            use rask_stdlib::FloatSig;
            return match m.sig {
                FloatSig::Unary | FloatSig::BinaryFloat | FloatSig::BinaryInt => MirType::F64,
                FloatSig::Predicate | FloatSig::Comparison => MirType::Bool,
                FloatSig::ToString => MirType::String,
                FloatSig::ToInt => MirType::I64,
                // Ordering is an enum; leave it to the caller's own typing.
                FloatSig::Compare => MirType::I64,
            };
        }
    }

    // SIMD float reductions return F64
    if is_scalar_return(func_name) && !func_name.ends_with("_store") && !func_name.ends_with("_set") {
        if func_name.starts_with("f32x") || func_name.starts_with("f64x") {
            return MirType::F64;
        }
    }

    // Suffix-based fallbacks for methods not in stubs (user types, etc.)
    if func_name.ends_with("_to_string") || func_name.ends_with("_to_uppercase")
        || func_name.ends_with("_to_lowercase") || func_name.ends_with("_trim")
        || func_name.ends_with("_trim_start") || func_name.ends_with("_trim_end")
        || func_name.ends_with("_replace") || func_name.ends_with("_substring")
        || func_name.ends_with("_substr")
        || func_name.ends_with("_repeat") || func_name.ends_with("_reverse")
    {
        return MirType::String;
    }
    if func_name.ends_with("_is_empty") || func_name.ends_with("_contains")
        || func_name.ends_with("_starts_with") || func_name.ends_with("_ends_with")
    {
        return MirType::Bool;
    }
    if func_name.starts_with("char_is_") || func_name == "char_eq" {
        return MirType::Bool;
    }

    MirType::I64
}

/// Convert a stub-derived RetCategory to a MirType.
fn ret_category_to_mir_type(cat: &rask_stdlib::mir_metadata::RetCategory) -> MirType {
    ret_category_to_mir_type_in(cat, None)
}

fn ret_category_to_mir_type_in(
    cat: &rask_stdlib::mir_metadata::RetCategory,
    ctx: Option<&MirContext>,
) -> MirType {
    use rask_stdlib::mir_metadata::RetCategory;
    match cat {
        RetCategory::Void => MirType::Void,
        RetCategory::Bool => MirType::Bool,
        RetCategory::I64 => MirType::I64,
        RetCategory::F64 => MirType::F64,
        RetCategory::String => MirType::String,
        RetCategory::Char => MirType::Char,
        RetCategory::Ptr => MirType::Ptr,
        RetCategory::Option(inner) => {
            MirType::Option(Box::new(ret_category_to_mir_type_in(inner, ctx)))
        }
        RetCategory::Result { ok, err } => MirType::Result {
            ok: Box::new(ret_category_to_mir_type_in(ok, ctx)),
            // Only the error side resolves a name. Everywhere else a named
            // stdlib type is an opaque runtime handle (File, TcpListener,
            // Instant) that really is a word — but an error type is an enum, and
            // its identity is what the match needs.
            err: Box::new(match (err.as_ref(), ctx) {
                (RetCategory::Named(name), Some(ctx)) => ctx.resolve_type_str(name),
                _ => ret_category_to_mir_type_in(err, ctx),
            }),
        },
        RetCategory::Named(_) => MirType::I64,
        RetCategory::Tuple(elems) => MirType::Tuple(
            elems.iter().map(|e| ret_category_to_mir_type_in(e, ctx)).collect()
        ),
    }
}

/// MIR type prefix derived from a MirType (fallback when local_type_prefix is absent).
/// Find the first comma at nesting depth 0 (respecting `<...>` brackets).
/// Split a string on a separator character at nesting depth 0,
/// respecting `<>` and `()` brackets.
fn split_top_level_parens(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            c2 if c2 == sep && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Generic arguments of a type written as `Name<A, B>`, split at top level.
pub(super) fn generic_args_of_str(s: &str) -> Option<Vec<&str>> {
    let open = s.find('<')?;
    let inner = s[open + 1..].strip_suffix('>')?;
    Some(split_top_level_parens(inner, ',').into_iter().map(str::trim).collect())
}

/// True if this method writes through `self`.
///
/// `mutate self` and `take self` are visible right here on the parameter. An
/// *inferred* mode is not: type.gradual/GC9 lets a private method omit it and
/// have the compiler decide from the body, and that decision happens in the type
/// checker. It records the spans it decided for, so this reads the answer rather
/// than walking the body again — one implementation of GC9, not two that drift.
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

fn find_top_level_comma(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
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
pub fn builtin_method_prefix(ty: &Type) -> Option<&'static str> {
    match ty {
        Type::F32 | Type::F64 => Some("f64"),
        Type::Bool => Some("bool"),
        Type::Char => Some("char"),
        Type::I8 | Type::I16 | Type::I32 | Type::I64 => Some("i64"),
        Type::U8 | Type::U16 | Type::U32 | Type::U64 => Some("u64"),
        // A slice dispatches like the container it came from: `parts[2..]
        // .join(" ")` is `Vec_join`. Without this the call fell through to the
        // name-policy table, which guesses "a two-argument `join` means Vec" —
        // right here, and only by luck.
        Type::Slice(_) | Type::Array { .. } => Some("Vec"),
        _ => None,
    }
}


/// Extract type prefix from a type annotation string.
///
/// Handles generic types like "Vec<i64>" → "Vec", "Map<K,V>" → "Map",
/// plain named types like "ThreadHandle" → "ThreadHandle",
/// and module-qualified types like "time.Instant" → "Instant".
/// Returns None for primitives (i64, f64, bool, string, etc.).
pub fn type_prefix_from_str(s: &str) -> Option<String> {
    let s = s.trim();
    // Strip module prefix (time.Instant → Instant)
    let base = s.rsplit('.').next().unwrap_or(s);
    // Strip generic args (Vec<i64> → Vec)
    let name = base.split('<').next().unwrap_or(base).trim();
    // Reject primitives and empty
    if name.is_empty() { return None; }
    match name {
        _ if rask_ast::primitives::is_builtin_scalar_or_string(name) || name == "()" => None,
        _ if name.chars().next().map_or(false, |c| c.is_uppercase()) => {
            Some(name.to_string())
        }
        // Module-level functions like "time" — not a type prefix
        _ => None,
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

/// What it takes to put a `for mutate` binding back where it came from.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MutateWriteback {
    /// The Vec or Map being iterated.
    collection: LocalId,
    /// Loop index, for a Vec. Ignored for a Map, which writes back by key.
    index: LocalId,
    /// The loop binding — the element for a Vec, the key for a Map.
    binding: LocalId,
    /// A Map's value binding. `Some` means this is a Map iteration.
    map_value: Option<LocalId>,
}

/// One outstanding element borrow, waiting for its call to be emitted so the
/// borrow can be released.
pub(crate) struct ElemWriteback {
    /// The collection the element was borrowed from.
    collection: MirOperand,
    /// Which release to call — Vec and Map keep separate borrow counts.
    release: &'static str,
}

impl MutateWriteback {
    pub(crate) fn new(
        collection: LocalId,
        index: LocalId,
        binding: LocalId,
        map_value: Option<LocalId>,
    ) -> Self {
        Self { collection, index, binding, map_value }
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

    fn int_expr(val: i64) -> Expr {
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
                ty: ty.map(|s| s.to_string()),
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
                ty: ty.map(|s| s.to_string()),
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
            kind: StmtKind::Assign { target, value },
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
                        ty: ty.to_string(),
                        is_take: false,
                        is_mutate: false,
                        default: None,
                    })
                    .collect(),
                ret_ty: ret_ty.map(|s| s.to_string()),
                context_clauses: vec![],
                body,
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
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
        assert_eq!(f.ret_ty, MirType::String);
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
    fn lower_parse_type_str_coverage() {
        let node_types = HashMap::new();
        let ctx = MirContext::empty_with_map(&node_types);
        assert_eq!(ctx.resolve_type_str("i8"), MirType::I8);
        assert_eq!(ctx.resolve_type_str("i16"), MirType::I16);
        assert_eq!(ctx.resolve_type_str("i32"), MirType::I32);
        assert_eq!(ctx.resolve_type_str("i64"), MirType::I64);
        assert_eq!(ctx.resolve_type_str("u8"), MirType::U8);
        assert_eq!(ctx.resolve_type_str("u16"), MirType::U16);
        assert_eq!(ctx.resolve_type_str("u32"), MirType::U32);
        assert_eq!(ctx.resolve_type_str("u64"), MirType::U64);
        assert_eq!(ctx.resolve_type_str("f32"), MirType::F32);
        assert_eq!(ctx.resolve_type_str("f64"), MirType::F64);
        assert_eq!(ctx.resolve_type_str("bool"), MirType::Bool);
        assert_eq!(ctx.resolve_type_str("char"), MirType::Char);
        assert_eq!(ctx.resolve_type_str("string"), MirType::String);
        assert_eq!(ctx.resolve_type_str("()"), MirType::Void);
        assert_eq!(ctx.resolve_type_str(""), MirType::Void);
        assert_eq!(ctx.resolve_type_str("SomeStruct"), MirType::Ptr);
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
                    ty: "i64".to_string(),
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
        use rask_mono::{EnumLayout, VariantLayout, FieldLayout};

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
                    }],
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
                    }],
                },
            ],
        };

        let enum_layouts = vec![shape_enum];
        let node_types = HashMap::new();
        let comptime_globals = HashMap::new();
        let extern_funcs = std::collections::HashSet::new();
        let type_names = HashMap::new();
        let empty_coercions = HashMap::new();
        let empty_error_wraps = HashMap::new();
        let empty_fallback_shape = std::collections::HashSet::new();
        let empty_rewrites = HashMap::new();
        let empty_targets = HashMap::new();
        let empty_resource_types = std::collections::HashSet::new();
        let empty_nominal = HashMap::new();
        let ctx = MirContext {
            // No checker in a hand-built lowering unit, so there's no GC9
            // decision to read. Stated, not defaulted.
            mutate_self_fns: None,
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
            type_names: &type_names,
            comptime_globals: &comptime_globals,
            extern_funcs: &extern_funcs,
            package_modules: &std::collections::HashSet::new(),
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(std::collections::HashSet::new()),
            line_map: None,
            source_file: None,
            comptime_interp: None,
            trait_methods: HashMap::new(),
            trait_coercions: &empty_coercions,
            error_wraps: &empty_error_wraps,
            fallback_keeps_shape: &empty_fallback_shape,
            call_rewrites: &empty_rewrites,
            call_targets: &empty_targets,
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
        use rask_mono::{EnumLayout, VariantLayout};

        let color_enum = EnumLayout {
            name: "Color".to_string(),
            size: 1,
            align: 1,
            tag_ty: rask_types::Type::U8,
            tag_offset: 0,
            variants: vec![
                VariantLayout { name: "Red".to_string(), tag: 0, payload_offset: 0, payload_size: 0, fields: vec![] },
                VariantLayout { name: "Green".to_string(), tag: 1, payload_offset: 0, payload_size: 0, fields: vec![] },
                VariantLayout { name: "Blue".to_string(), tag: 2, payload_offset: 0, payload_size: 0, fields: vec![] },
            ],
        };

        let enum_layouts = vec![color_enum];
        let node_types = HashMap::new();
        let comptime_globals = HashMap::new();
        let extern_funcs = std::collections::HashSet::new();
        let type_names = HashMap::new();
        let empty_coercions = HashMap::new();
        let empty_error_wraps = HashMap::new();
        let empty_fallback_shape = std::collections::HashSet::new();
        let empty_rewrites = HashMap::new();
        let empty_targets = HashMap::new();
        let empty_resource_types = std::collections::HashSet::new();
        let empty_nominal = HashMap::new();
        let ctx = MirContext {
            // No checker in a hand-built lowering unit, so there's no GC9
            // decision to read. Stated, not defaulted.
            mutate_self_fns: None,
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
            type_names: &type_names,
            comptime_globals: &comptime_globals,
            extern_funcs: &extern_funcs,
            package_modules: &std::collections::HashSet::new(),
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(std::collections::HashSet::new()),
            line_map: None,
            source_file: None,
            comptime_interp: None,
            trait_methods: HashMap::new(),
            trait_coercions: &empty_coercions,
            error_wraps: &empty_error_wraps,
            fallback_keeps_shape: &empty_fallback_shape,
            call_rewrites: &empty_rewrites,
            call_targets: &empty_targets,
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
        use rask_mono::{EnumLayout, VariantLayout, FieldLayout};

        let msg_enum = EnumLayout {
            name: "Msg".to_string(),
            size: 12,
            align: 4,
            tag_ty: rask_types::Type::U8,
            tag_offset: 0,
            variants: vec![
                VariantLayout { name: "Empty".to_string(), tag: 0, payload_offset: 4, payload_size: 0, fields: vec![] },
                VariantLayout {
                    name: "Pair".to_string(),
                    tag: 1,
                    payload_offset: 4,
                    payload_size: 8,
                    fields: vec![
                        FieldLayout { name: "f0".to_string(), ty: rask_types::Type::I32, offset: 0, size: 4, align: 4, attrs: vec![], has_declared_default: false },
                        FieldLayout { name: "f1".to_string(), ty: rask_types::Type::I32, offset: 4, size: 4, align: 4, attrs: vec![], has_declared_default: false },
                    ],
                },
            ],
        };

        let enum_layouts = vec![msg_enum];
        let node_types = HashMap::new();
        let comptime_globals = HashMap::new();
        let extern_funcs = std::collections::HashSet::new();
        let type_names = HashMap::new();
        let empty_coercions = HashMap::new();
        let empty_error_wraps = HashMap::new();
        let empty_fallback_shape = std::collections::HashSet::new();
        let empty_rewrites = HashMap::new();
        let empty_targets = HashMap::new();
        let empty_resource_types = std::collections::HashSet::new();
        let empty_nominal = HashMap::new();
        let ctx = MirContext {
            // No checker in a hand-built lowering unit, so there's no GC9
            // decision to read. Stated, not defaulted.
            mutate_self_fns: None,
            struct_layouts: &[],
            enum_layouts: &enum_layouts,
            node_types: &node_types,
            type_names: &type_names,
            comptime_globals: &comptime_globals,
            extern_funcs: &extern_funcs,
            package_modules: &std::collections::HashSet::new(),
            shared_elem_types: std::cell::RefCell::new(HashMap::new()),
            shared_elem_conflicts: std::cell::RefCell::new(std::collections::HashSet::new()),
            line_map: None,
            source_file: None,
            comptime_interp: None,
            trait_methods: HashMap::new(),
            trait_coercions: &empty_coercions,
            error_wraps: &empty_error_wraps,
            fallback_keeps_shape: &empty_fallback_shape,
            call_rewrites: &empty_rewrites,
            call_targets: &empty_targets,
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
        // Vec.new() → Call to Vec_new
        let decl = make_fn("f", vec![], None, vec![
            expr_stmt(method_call_expr(ident_expr("Vec"), "new", vec![])),
            return_stmt(None),
        ]);
        let f = lower_one(&decl);
        assert!(find_call(&f, "Vec_new"));
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
