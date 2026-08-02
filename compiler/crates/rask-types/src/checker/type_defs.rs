// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type definitions used throughout the checker.

use std::collections::HashMap;

use rask_ast::NodeId;
use rask_resolve::SymbolId;

use super::type_table::TypeTable;

use crate::types::{Type, TypeId};

/// The function a call expression resolves to (CALL6). Recorded once during
/// type checking so lowering and the hidden-param pass never re-derive it
/// from a reconstructed name.
///
/// A structured id, never a name string:
/// - `Free` for `f(...)` — the callee's resolved symbol.
/// - `Method` for `recv.m(...)` / `T.m(...)` — the *resolved* receiver type
///   plus the method name selected by dispatch. Methods have no single symbol
///   id yet, so `(receiver type, name)` stands in as the structured id.
///
/// The receiver is stored fully applied — substitutions run, aliases resolved —
/// which is the part consumers can't reconstruct. `node_types` holds whatever
/// the receiver *expression* was assigned, and that is routinely still a type
/// variable (or missing entirely, for nodes synthesized after checking).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Callee {
    Free(SymbolId),
    Method { recv: Type, method: String },
}

impl Callee {
    /// The receiver's TypeId, for user-defined types. `None` for stdlib and
    /// primitive receivers, which have no entry in the type table.
    pub fn recv_type_id(&self) -> Option<TypeId> {
        match self {
            Callee::Method { recv: Type::Named(id), .. } => Some(*id),
            Callee::Method { recv: Type::Generic { base, .. }, .. } => Some(*base),
            _ => None,
        }
    }
}

/// Canonical name for a method receiver, matching how monomorphization mangles
/// `{Type}_{method}`. Returns `None` for receivers that don't qualify a method
/// name on their own (bare type variables, tuples, unit).
pub fn receiver_name(ty: &Type, types: &TypeTable) -> Option<String> {
    match ty {
        Type::Named(id) | Type::Generic { base: id, .. } => {
            let name = types.type_name(*id);
            (!name.starts_with('<')).then_some(name)
        }
        Type::UnresolvedNamed(name) => Some(name.clone()),
        Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
        Type::String => Some("string".to_string()),
        // `T?` is `T or none`; it dispatches as Option, everything else as Result.
        Type::Result { err, .. } if **err == Type::None => Some("Option".to_string()),
        Type::Result { .. } => Some("Result".to_string()),
        Type::RawPtr(_) => Some("Ptr".to_string()),
        Type::Slice(_) => Some("Slice".to_string()),
        Type::Bool => Some("bool".to_string()),
        Type::Char => Some("char".to_string()),
        Type::I8 => Some("i8".to_string()),
        Type::I16 => Some("i16".to_string()),
        Type::I32 => Some("i32".to_string()),
        Type::I64 => Some("i64".to_string()),
        Type::I128 => Some("i128".to_string()),
        Type::U8 => Some("u8".to_string()),
        Type::U16 => Some("u16".to_string()),
        Type::U32 => Some("u32".to_string()),
        Type::U64 => Some("u64".to_string()),
        Type::U128 => Some("u128".to_string()),
        Type::F32 => Some("f32".to_string()),
        Type::F64 => Some("f64".to_string()),
        Type::TraitObject { trait_name } => Some(trait_name.clone()),
        _ => None,
    }
}

/// Information about a user-defined type.
#[derive(Debug, Clone)]
pub enum TypeDef {
    Struct {
        name: String,
        type_params: Vec<String>,
        fields: Vec<(String, Type)>,
        methods: Vec<MethodSig>,
        is_resource: bool,
        /// U1–U4: marked @unique — no implicit copy even if small enough
        is_unique: bool,
        /// B1–G4: @binary struct for wire-format parsing/building
        is_binary: bool,
        /// V5: fields marked `private` — accessible only inside extend blocks
        private_fields: Vec<String>,
        /// ER42/L1 transitive linearity: true if `is_resource` is true OR any
        /// field type is itself transitively linear. Computed by a fixed-point
        /// pass after declaration collection.
        is_transitive_resource: bool,
    },
    Enum {
        name: String,
        type_params: Vec<String>,
        variants: Vec<(String, Vec<Type>)>,
        methods: Vec<MethodSig>,
        /// ER42/L1 transitive linearity: true if any variant payload contains
        /// a transitively-linear type. Computed by a fixed-point pass after
        /// declaration collection.
        is_transitive_resource: bool,
    },
    Trait {
        name: String,
        super_traits: Vec<String>,
        methods: Vec<MethodSig>,
        /// TR3: names of methods that declare their own type parameters.
        /// These can't be dispatched through `any` — no vtable slot.
        generic_methods: Vec<String>,
        is_unsafe: bool,
        /// G1: `duck trait` — satisfied by shape, no declaration needed.
        is_duck: bool,
    },
    Union {
        name: String,
        fields: Vec<(String, Type)>,
    },
    /// Nominal type alias: same layout as underlying, distinct identity.
    NominalAlias {
        name: String,
        underlying: Type,
        with_traits: Vec<String>,
        /// Methods from `extend` blocks. A nominal newtype has its own identity,
        /// so it carries its own methods like structs and enums.
        methods: Vec<MethodSig>,
    },
}

/// Method name without its type-parameter suffix: `convert<T>` → `convert`.
fn method_base(name: &str) -> &str {
    name.split('<').next().unwrap_or(name)
}

impl TypeDef {
    /// TR3: true if `method` is a generic method of this trait (can't dispatch through `any`).
    /// Trait method names carry their type params (`convert<T>`); the call site does
    /// not, so compare on the base name.
    pub fn is_generic_trait_method(&self, method: &str) -> bool {
        matches!(self, TypeDef::Trait { generic_methods, .. }
            if generic_methods.iter().any(|m| method_base(m) == method_base(method)))
    }

    /// TR1–TR3: names of trait methods callable through `any`, in declaration order.
    /// Skips Self-returning (TR2) and generic (TR3) methods — these have no vtable slot,
    /// so the vtable layout and the MIR dispatch offset both index this list.
    pub fn object_compatible_method_names(&self) -> Vec<String> {
        match self {
            TypeDef::Trait { methods, generic_methods, .. } => methods
                .iter()
                .filter(|m| {
                    let returns_self = matches!(&m.ret, Type::UnresolvedNamed(n) if n == "Self");
                    let is_generic = generic_methods.iter().any(|g| g == &m.name);
                    !returns_self && !is_generic
                })
                .map(|m| m.name.clone())
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// Method signature.
#[derive(Debug, Clone)]
pub struct MethodSig {
    pub name: String,
    pub self_param: SelfParam,
    pub params: Vec<(Type, ParamMode)>,
    pub ret: Type,
    /// Type parameters the method declares for itself, as (name, bounds) —
    /// e.g. the `E` in `func tag<E>(self, e: E) -> E`, or `T: Named`. Separate
    /// from the receiver type's own parameters: these get a fresh variable per
    /// *call*, not per receiver.
    pub type_params: Vec<(String, Vec<String>)>,
}

/// How self is passed to a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfParam {
    None,   // Static method
    Value,  // self (read-only, default)
    Mutate, // mutate self (mutable)
    Take,   // take self (consumed)
}

/// How a parameter is passed to a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    Default, // Normal pass (read-only, default)
    Mutate,  // mutate param (mutable borrow)
    Take,    // take param (consumed)
}

/// Builtin module method signature.
#[derive(Debug, Clone)]
pub struct ModuleMethodSig {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

/// Endianness for multi-byte binary fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    Big,
    Little,
}

/// A single field's binary layout specifier.
#[derive(Debug, Clone)]
pub struct BinaryFieldSpec {
    pub name: String,
    pub bits: u32,
    pub endian: Option<Endian>,
    pub runtime_type: Type,
    /// Byte offset within the struct where this field's bits start
    pub bit_offset: u32,
    /// Whether this is a fixed byte array ([N]u8)
    pub is_byte_array: bool,
    pub byte_array_len: usize,
}

/// Metadata for a @binary struct.
#[derive(Debug, Clone)]
pub struct BinaryStructInfo {
    pub name: String,
    pub fields: Vec<BinaryFieldSpec>,
    pub total_bits: u32,
    /// SIZE in bytes (rounded up)
    pub size_bytes: u32,
}

/// Result of type checking.
#[derive(Debug)]
pub struct TypedProgram {
    /// Resolved symbols from name resolution.
    pub symbols: rask_resolve::SymbolTable,
    /// Symbol resolutions from name resolution.
    pub resolutions: HashMap<NodeId, SymbolId>,
    /// Type table with all type definitions.
    pub types: TypeTable,
    /// Computed type for each expression node.
    pub node_types: HashMap<NodeId, Type>,
    /// Resolved type arguments for each generic call site.
    /// Key is the Call/MethodCall expression's NodeId.
    pub call_type_args: HashMap<NodeId, Vec<Type>>,
    /// CALL6: the function each call resolves to, keyed by the Call/MethodCall
    /// expression's NodeId. The single source of truth for dispatch — lowering
    /// and the hidden-param pass read this instead of mangling type names.
    pub call_targets: HashMap<NodeId, Callee>,
    /// TR5: implicit trait coercion sites. NodeId of expression → trait name.
    pub trait_coercions: HashMap<NodeId, String>,
    /// Unsafe operations recorded during type checking (span + category).
    pub unsafe_ops: Vec<(rask_ast::Span, super::UnsafeCategory)>,
    /// Types for binding names and parameters, keyed by (span.start, span.end, file_id).
    /// Used by the LSP for hover on identifiers that aren't expression nodes.
    pub span_types: HashMap<(usize, usize, u16), Type>,
    /// T1: method-call spans that resolved to a channel `Sender.send`. Read by
    /// the ownership checker to transfer ownership of the sent value even when
    /// inference leaves the receiver as a type variable in `node_types`.
    pub channel_send_sites: std::collections::HashSet<rask_ast::Span>,
}
