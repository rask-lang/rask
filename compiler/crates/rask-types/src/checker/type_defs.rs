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
    Method {
        recv: Type,
        method: String,
        /// XC4/XC5: the package whose `extend` block supplies this method, when
        /// more than one declares it. `None` is the ordinary case — one block,
        /// one body, nothing to choose between.
        package: Option<String>,
    },
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
            types.get(*id).map(|_| types.type_name(*id))
        }
        Type::UnresolvedNamed(name) => Some(name.clone()),
        Type::UnresolvedGeneric { name, .. } => Some(name.clone()),
        Type::String => Some("string".to_string()),
        // `T?` is `T or none`; it dispatches as Option, everything else as Result.
        Type::Result { err, .. } if **err == Type::None => Some("Option".to_string()),
        Type::Result { .. } => Some("Result".to_string()),
        Type::RawPtr(_) => Some("Ptr".to_string()),
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
        Type::InterfaceObject { interface_name, .. } => Some(interface_name.clone()),
        _ => None,
    }
}

/// Information about a user-defined type.
/// GT1/GT4/GT5: an interface's type parameter as declared.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceTypeParam {
    pub name: String,
    /// GT5: what a conformance's argument must satisfy.
    pub bounds: Vec<rask_ast::ty::TypeExpr>,
    /// GT4: what the bare interface name means. `None` makes the argument required.
    pub default: Option<rask_ast::ty::TypeExpr>,
}

/// AT1/AT4/AT5: an associated type a conformance supplies.
#[derive(Debug, Clone, PartialEq)]
pub struct InterfaceAssocType {
    pub name: String,
    /// AT5: what the conformance's binding must satisfy.
    pub bounds: Vec<rask_ast::ty::TypeExpr>,
    /// AT4: what a conformance that omits the binding gets.
    pub default: Option<rask_ast::ty::TypeExpr>,
}

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
        /// E19: fields marked `@skip` — left out of every serialized form, so
        /// they don't get a say in whether the type is Encode/Decode either.
        skipped_fields: Vec<String>,
        /// E13a: fields the wire form leaves out — `private` or
        /// `@no_serialize` — that have no default to fill them from on decode.
        /// A type with any of these isn't auto-`Decode`; it can still be
        /// `Encode`, since encoding never needs a value for a field it omits.
        undecodable_fields: Vec<String>,
        /// ER42/L1 transitive linearity: true if `is_resource` is true OR any
        /// field type is itself transitively linear. Computed by a fixed-point
        /// pass after declaration collection.
        is_transitive_resource: bool,
        /// E16: marked `@no_encode` / `@no_decode`. The owner saying this
        /// type's data doesn't go on a wire, whatever its fields would allow.
        no_encode: bool,
        no_decode: bool,
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
        /// E16: as on a struct. The spec writes the rule for structs, but an
        /// enum auto-derives the same way (E17) and a payload can be just as
        /// wrong to serialize, so the opt-out has to reach both.
        no_encode: bool,
        no_decode: bool,
    },
    Interface {
        name: String,
        /// GT1: `interface Scale<Rhs>` — bound by the conformance header.
        type_params: Vec<InterfaceTypeParam>,
        super_interfaces: Vec<rask_ast::ty::TypeExpr>,
        methods: Vec<MethodSig>,
        /// AT1: types a conformance supplies.
        assoc_types: Vec<InterfaceAssocType>,
        /// TR3: names of methods that declare their own type parameters.
        /// These can't be dispatched through `any` — no vtable slot.
        generic_methods: Vec<String>,
        is_unsafe: bool,
        /// G1: `duck interface` — satisfied by shape, no declaration needed.
        is_duck: bool,
    },
    Union {
        name: String,
        fields: Vec<(String, Type)>,
    },
    /// OR6: a primitive, so it has somewhere to carry conformances and the
    /// methods that come with them.
    ///
    /// `extend f64 { … }` — an inherent method on a primitive — stays illegal.
    /// What lands here is `f64 implements Mul<Meters>`: the conformance tables
    /// are keyed by `TypeId`, and without an entry a primitive had none to be
    /// keyed by, which is why the right-hand direction of every unit and vector
    /// operator was unwritable.
    ///
    /// Registered under its own name map, not `type_names` — `f64` in source
    /// still means `Type::F64`, never `Named(id)`.
    Primitive {
        name: String,
        methods: Vec<MethodSig>,
    },
    /// Nominal type alias: same layout as underlying, distinct identity.
    NominalAlias {
        name: String,
        underlying: Type,
        with_interfaces: Vec<rask_ast::ty::TypeExpr>,
        /// Methods from `extend` blocks. A nominal newtype has its own identity,
        /// so it carries its own methods like structs and enums.
        methods: Vec<MethodSig>,
    },
}

/// Method name without its type-parameter suffix: `convert<T>` → `convert`.
/// XC5: the symbol a conformance method gets where more than one package
/// declares it on the same type — `Doc_label` becomes `Doc_label~liba`.
///
/// One function so monomorphization, MIR and the checker can't drift: the name
/// the call emits has to be the name the body is emitted under.
///
/// `~` because nothing else in a generated name uses it. `_` is how a
/// dependency's declarations are qualified (`Doc` in `interfacepkg` is
/// `Doc_interfacepkg`), so `Doc_label_liba` is also what a method named `label_liba`
/// would produce; `$` is monomorphization's type-argument separator, so
/// `Doc_label$liba` reads as an instantiation; `<`, `>`, `.`, `?` and `,` all
/// appear in type spellings, and `@` means a symbol version to an ELF linker.
/// A Rask identifier can't contain `~`, so nothing a program declares collides
/// with it.
pub const CONFORMANCE_SEP: char = '~';

pub fn conformance_symbol(base: &str, package: &str) -> String {
    format!("{}{}{}", base, CONFORMANCE_SEP, package)
}

pub(crate) fn method_base(name: &str) -> &str {
    name
}

impl TypeDef {
    /// TR3: true if `method` is a generic method of this interface (can't dispatch through `any`).
    /// Interface method names carry their type params (`convert<T>`); the call site does
    /// not, so compare on the base name.
    pub fn is_generic_interface_method(&self, method: &str) -> bool {
        matches!(self, TypeDef::Interface { generic_methods, .. }
            if generic_methods.iter().any(|m| method_base(m) == method_base(method)))
    }

    /// TR1–TR3: names of interface methods callable through `any`, in declaration order.
    /// Skips Self-returning (TR2) and generic (TR3) methods — these have no vtable slot,
    /// so the vtable layout and the MIR dispatch offset both index this list.
    pub fn object_compatible_method_names(&self) -> Vec<String> {
        match self {
            TypeDef::Interface { methods, generic_methods, .. } => methods
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

/// ER31a: the variant `try` wraps a propagated error in on its way out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorWrap {
    /// The boundary enum the enclosing function returns as its error.
    pub enum_name: String,
    /// The variant that carries the source error as its only payload.
    pub variant: String,
}

/// Method signature.
#[derive(Debug, Clone)]
pub struct MethodSig {
    pub name: String,
    pub self_param: SelfParam,
    pub params: Vec<(Type, ParamMode)>,
    /// Parameter names as declared, positionally matching `params`. What a
    /// named argument's label is checked against. Empty for a signature the
    /// checker supplied: it has no declaration to name its parameters.
    pub param_names: Vec<String>,
    pub ret: Type,
    /// Type parameters the method declares for itself, as (name, bounds) —
    /// e.g. the `E` in `func tag<E>(self, e: E) -> E`, or `T: Named`. Separate
    /// from the receiver type's own parameters: these get a fresh variable per
    /// *call*, not per receiver.
    pub type_params: Vec<(String, Vec<rask_ast::ty::TypeExpr>)>,
    /// The extend header's target arguments as written, one per parameter the
    /// receiving type declares: `["(K, V)"]` for `extend Sequence<(K, V)>` on a
    /// `Sequence<T>`, `["K", "V"]` for `extend Map<K, V>`.
    ///
    /// Names inside these belong to the *receiver*, and a call binds them
    /// against the receiver's actual arguments — member-wise where one is
    /// nested. Dropping the header used to misfile them as the method's own:
    /// `to_map(self) -> Map<K, V>` in `extend Sequence<(K, V)>` looked like a
    /// method with two parameters of its own, and `probe_keys(self) -> Vec<K>`
    /// like one with a single unbindable parameter — so the return type stayed
    /// open however concrete the receiver was (#1046).
    ///
    /// Empty for a method with no generic receiver, and for the derived and
    /// interface-supplied signatures, which have no header to read.
    pub owner_patterns: Vec<rask_ast::ty::TypeExpr>,
    /// The block's `where` clause, as (receiver parameter, bounds). It covers
    /// every method in the block (type.generics/CC3), so a call on a receiver
    /// whose argument doesn't meet it is an error: `sort` lives in
    /// `extend Vec<T> where T: Comparable`, and a `Vec<i64?>` can't call it.
    pub owner_bounds: Vec<(String, Vec<rask_ast::ty::TypeExpr>)>,
    /// The checker supplied it (EQ1, HA1, ORD1 and the rest): a signature with
    /// no body behind it, which the backends answer structurally.
    pub derived: bool,
    /// Each parameter's declared default, positionally matching `params`.
    /// Empty for a signature with no declaration behind it.
    pub defaults: Vec<Option<rask_ast::expr::Expr>>,
}

/// How self is passed to a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfParam {
    None,   // Static method
    Value,  // self (read-only, default)
    Mutate, // mutate self (mutable)
    Take,   // take self (consumed)
}

pub use rask_ast::ty::ParamMode;

/// Builtin module method signature.
#[derive(Debug, Clone)]
pub struct ModuleMethodSig {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
    /// Interface bounds on the method's own type parameters, as the stub wrote them
    /// (`decode<T: Decode>` → `[("T", "Decode")]`). Checked against the written
    /// type argument at the call site.
    pub type_param_bounds: Vec<(String, rask_ast::ty::TypeExpr)>,
    /// Which bounded type parameter each parameter *is*, when its declared type
    /// is exactly one — `encode<T: Encode>(value: T)` gives `[Some("T")]`.
    ///
    /// `params` can't answer this: `stub_type` turns a single-letter type
    /// parameter into the `_Any` wildcard, which is what the return-type
    /// freshening runs on, so by then the name is gone. Without it a call that
    /// didn't write the type argument had nothing to check the bound against.
    pub param_type_params: Vec<Option<String>>,
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

/// One type parameter and what a call site settled it to.
///
/// The name is the point. Monomorphization has to know which parameter an
/// argument belongs to before it can substitute anything, and a bare positional
/// list can't say: a method's own arguments and its extend header's arrive
/// through different routes and land in one list, so reading them back means
/// guessing where the seam is. Whoever resolved the argument knew the name —
/// carrying it costs a string and removes the guess.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TypeBinding {
    pub param: String,
    pub ty: Type,
}

impl TypeBinding {
    pub fn new(param: impl Into<String>, ty: Type) -> Self {
        Self { param: param.into(), ty }
    }
}

impl TypedProgram {
    /// Each method call the checker resolved, named `Type.method` after the
    /// receiver's declared type — `Handle.join` for `t.join()` on a
    /// `Handle<i64>`. What the effects pass classifies a call by.
    pub fn method_call_names(&self) -> HashMap<NodeId, String> {
        self.call_targets
            .iter()
            .filter_map(|(node, callee)| match callee {
                Callee::Method { recv, method, .. } => {
                    let ty = receiver_name(recv, &self.types)?;
                    Some((*node, format!("{ty}.{method}")))
                }
                Callee::Free(_) => None,
            })
            .collect()
    }

    /// Program functions whose name is already some method's symbol, each
    /// against a symbol of its own: `Vec_len` → `Vec_len#fn`.
    ///
    /// A method `m` on `T` is `T_m` to every backend, and a function is its
    /// name, so `func Vec_len` and `Vec.len` were one symbol. `#` is in no
    /// identifier, so the new name can't be one either. The method keeps its
    /// symbol because the stdlib's tables, and MIR's own spellings for it, are
    /// keyed on that. `main` and a function with a foreign ABI keep theirs:
    /// something outside the program calls them by it.
    fn functions_needing_symbols(&self, decls: &[rask_ast::decl::Decl]) -> HashMap<String, String> {
        use rask_ast::decl::DeclKind;
        let method_symbols: std::collections::HashSet<String> = self
            .types
            .types
            .iter()
            .flat_map(|def| {
                let methods: &[MethodSig] = match def {
                    TypeDef::Struct { methods, .. }
                    | TypeDef::Enum { methods, .. }
                    | TypeDef::NominalAlias { methods, .. } => methods,
                    _ => &[],
                };
                let ty = super::type_table::TypeTable::def_name(def);
                methods.iter().map(move |m| format!("{ty}_{}", m.name))
            })
            .collect();
        decls
            .iter()
            .filter_map(|d| match &d.kind {
                DeclKind::Fn(f)
                    if f.name != "main"
                        && f.abi.is_none()
                        && (method_symbols.contains(&f.name)
                            || rask_stdlib::mir_metadata::is_method_symbol(&f.name)) =>
                {
                    Some((f.name.clone(), format!("{}#fn", f.name)))
                }
                _ => None,
            })
            .collect()
    }

    /// Hand the checker's own declarations to the program: the derived
    /// `eq`/`hash`/`compare` bodies and wrapper functions it wrote and
    /// checked, and every `==` on two wrappers turned into a call to the
    /// wrapper's `eq` (`checker/derive.rs`).
    ///
    /// Also writes each transparent alias's target where the alias was named.
    ///
    /// Done here, by the entry points that check a program, rather than by
    /// whoever runs next: every pass after this one reads the declarations,
    /// and none of them should have to know the checker wrote some.
    pub fn attach_derived(&mut self, decls: &mut Vec<rask_ast::decl::Decl>) {
        struct Calls<'a>(&'a HashMap<NodeId, (NodeId, String)>);
        impl rask_ast::rewrite::Rewrite for Calls<'_> {
            fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
                use rask_ast::expr::{Expr, ExprKind, CallArg, ArgMode};
                let Some((callee, name)) = self.0.get(&e.id) else { return };
                let old = std::mem::replace(&mut e.kind, ExprKind::Bool(false));
                let ExprKind::MethodCall { object, mut args, .. } = old else {
                    e.kind = old;
                    return;
                };
                let rhs = args.remove(0);
                e.kind = ExprKind::Call {
                    func: Box::new(Expr { id: *callee, kind: ExprKind::Ident(name.clone()), span: e.span }),
                    args: vec![
                        CallArg { name: None, mode: ArgMode::Default, expr: *object },
                        CallArg { name: None, mode: ArgMode::Default, expr: rhs.expr },
                    ],
                };
            }
        }
        rask_ast::rewrite::rewrite_decls(decls, &mut Calls(&self.wrapper_eq_calls));

        // A collection standing in for a `Sequence<E>` (SEQ48) gets the
        // `as_sequence()` the checker typed for it. The wrapping call is the
        // node the slot's type and the call's target were recorded on.
        // The walk goes on into the wrapped value, which still has its own
        // id, so each one is wrapped once.
        struct ChainHeads<'a>(&'a HashMap<NodeId, NodeId>, std::collections::HashSet<NodeId>);
        impl rask_ast::rewrite::Rewrite for ChainHeads<'_> {
            fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
                use rask_ast::expr::{Expr, ExprKind};
                let Some(call) = self.0.get(&e.id) else { return };
                if !self.1.insert(e.id) {
                    return;
                }
                let span = e.span;
                let value = std::mem::replace(e, Expr { id: *call, kind: ExprKind::Bool(false), span });
                e.kind = ExprKind::MethodCall {
                    object: Box::new(value),
                    method: "as_sequence".to_string(),
                    type_args: None,
                    args: Vec::new(),
                };
            }
        }
        if !self.sequence_coercions.is_empty() {
            rask_ast::rewrite::rewrite_decls(decls, &mut ChainHeads(&self.sequence_coercions, Default::default()));
        }

        // Defaults the checker filled into method calls.
        struct Defaults<'a>(&'a HashMap<NodeId, Vec<(usize, rask_ast::expr::Expr)>>);
        impl rask_ast::rewrite::Rewrite for Defaults<'_> {
            fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
                use rask_ast::expr::{ArgMode, CallArg, ExprKind};
                let Some(fills) = self.0.get(&e.id) else { return };
                let ExprKind::MethodCall { args, .. } = &mut e.kind else { return };
                for (at, expr) in fills {
                    let arg = CallArg { name: None, mode: ArgMode::Default, expr: expr.clone() };
                    args.insert((*at).min(args.len()), arg);
                }
            }
        }
        if !self.default_fills.is_empty() {
            rask_ast::rewrite::rewrite_decls(decls, &mut Defaults(&self.default_fills));
        }

        // A program type sharing a stdlib type's name has gone by its symbol
        // in the table since it was registered (`TypeTable::written_names`).
        // Its declaration and every use the program wrote say that symbol from
        // here, and the written name means the stdlib's type to every pass
        // after this one — so neither backend can mistake one for the other
        // (#1333). What the checker wrote itself is named from the table
        // already, which is why this runs before those are added.
        let renamed = self.types.release_written_names();
        rename_types(decls, &renamed, &self.type_test_patterns);

        // `is json.JsonError as e` kept its module through checking, which is
        // what said the stdlib's type was meant (#1470). The bare name means
        // that type from here, and the backends read a dotted pattern name as
        // `Enum.Variant`.
        struct ModuleTypePatterns<'a>(&'a super::type_table::TypeTable);
        impl rask_ast::rewrite::Rewrite for ModuleTypePatterns<'_> {
            fn pattern(&mut self, p: &mut rask_ast::expr::Pattern) {
                let rask_ast::expr::Pattern::TypePat {
                    ty: rask_ast::ty::TypeExpr::Named { path, .. }, ..
                } = p
                else {
                    return;
                };
                if path.len() > 1 && self.0.module_named(&path[0]).is_some() {
                    path.remove(0);
                }
            }
        }
        rask_ast::rewrite::rewrite_decls(decls, &mut ModuleTypePatterns(&self.types));

        // A program function the same way, when its name is a symbol the
        // backends already use for a method: `func Vec_len(v)` and `v.len()`
        // were one `Vec_len` to native, and the method call ran the program's
        // body (#1307).
        let renamed = self.functions_needing_symbols(decls);
        rask_ast::qualify::qualify_in_place(decls, &renamed);
        for (written, symbol) in &renamed {
            if let Some(ret) = self.inferred_fn_ret.remove(written) {
                self.inferred_fn_ret.insert(symbol.clone(), ret);
            }
            if let Some(params) = self.inferred_fn_params.remove(written) {
                self.inferred_fn_params.insert(symbol.clone(), params);
            }
        }

        let mut derived = std::mem::take(&mut self.derived_decls);
        rask_ast::rewrite::rewrite_decls(&mut derived, &mut Calls(&self.wrapper_eq_calls));
        decls.extend(derived);

        // A transparent alias is the type it names. The checker resolves one
        // through its alias table; mono, lowering and the interpreter read the
        // written types and have no table, so a field or parameter typed
        // `Names` reached them as a type nobody declared (#1316). Writing the
        // target in its place hands them what the checker already knew.
        struct Aliases<'a>(&'a super::type_table::TypeTable);
        impl rask_ast::rewrite::Rewrite for Aliases<'_> {
            fn ty(&mut self, t: &mut rask_ast::ty::TypeExpr) {
                *t = self.0.expand_aliases(t);
            }
        }
        rask_ast::rewrite::rewrite_decls(decls, &mut Aliases(&self.types));

        // The backends know an interface by its symbol, one per declaration
        // (`TypeTable::interface_symbol`). A program interface shadowing a
        // stdlib one has a symbol that isn't its name, so the program's `any
        // Writer` is written out as that symbol here — the stdlib's own
        // `any Writer` keeps the plain name and means the stdlib's (#1426).
        struct AnySymbols<'a>(&'a super::type_table::TypeTable);
        impl rask_ast::rewrite::Rewrite for AnySymbols<'_> {
            fn ty(&mut self, t: &mut rask_ast::ty::TypeExpr) {
                let rask_ast::ty::TypeExpr::Any(inner) = t else { return };
                let Some(name) = inner.name() else { return };
                let Type::InterfaceObject { decl, .. } = self.0.interface_object(&name) else { return };
                let symbol = self.0.interface_symbol(&name, decl);
                if symbol != name {
                    **inner = rask_ast::ty::TypeExpr::named(symbol);
                }
            }
        }
        rask_ast::rewrite::rewrite_decls(decls, &mut AnySymbols(&self.types));
    }
}

/// Result of type checking.
#[derive(Debug)]
pub struct TypedProgram {
    /// Resolved symbols from name resolution.
    pub symbols: rask_resolve::SymbolTable,
    /// Struct declarations synthesized from `import c` headers. Not in the
    /// source, so monomorphization has to be handed them or a C struct gets no
    /// layout (#948).
    pub c_type_decls: Vec<rask_ast::decl::Decl>,
    /// Symbol resolutions from name resolution.
    pub resolutions: HashMap<NodeId, SymbolId>,
    /// Type table with all type definitions.
    pub types: TypeTable,
    /// Computed type for each expression node.
    pub node_types: HashMap<NodeId, Type>,
    /// Resolved type arguments for each generic call site, each named by the
    /// type parameter it binds. Key is the Call/MethodCall expression's NodeId.
    pub call_type_args: HashMap<NodeId, Vec<TypeBinding>>,
    /// CALL6: the function each call resolves to, keyed by the Call/MethodCall
    /// expression's NodeId. The single source of truth for dispatch — lowering
    /// and the hidden-param pass read this instead of mangling type names.
    pub call_targets: HashMap<NodeId, Callee>,
    /// OR1: operator calls the pair resolved to a conformance, keyed by the
    /// MethodCall's NodeId.
    ///
    /// `a * b` is a machine instruction on some pairs and a call on others, and
    /// on a primitive receiver the right operand is what decides — so the
    /// backends read the answer here rather than each deciding again from the
    /// receiver alone.
    pub operator_targets: HashMap<NodeId, super::operators::OperatorTarget>,
    /// TR5: implicit interface coercion sites. NodeId of expression → the
    /// interface's symbol (`TypeTable::interface_symbol`).
    pub interface_coercions: HashMap<NodeId, String>,
    /// XC4: which package wrote each source file, by file id. A span carries
    /// its file id, so this answers "whose code is this?" for anything after
    /// the checker — the merged decl list has no packages left in it.
    pub file_packages: HashMap<u16, String>,
    /// XC5: `extend` blocks whose methods need the declaring package in their
    /// symbol, because another package declares the same method on the same
    /// type. Impl decl id → package name. Empty in every program without a
    /// collision, which is nearly all of them.
    ///
    /// Without it both blocks' `label` mangle to one `Doc_label` and whichever
    /// the pass read last wins, so `liba`'s own call ran `libb`'s body.
    pub conformance_disambiguation: HashMap<NodeId, String>,
    /// OR4: the interface each `implements` block conforms to, by its own
    /// name. Impl decl id → `Mul` for `Meters implements ops.Mul<f64>`.
    /// An operator conformance's methods are filed under the applied argument,
    /// and whether the block is one depends on which interface it names, not
    /// on how the header spelled it.
    pub conformance_interfaces: HashMap<NodeId, String>,
    /// ER31a: `try` sites whose error is wrapped in a variant of the enclosing
    /// function's error enum. NodeId of the `try` expression → the variant.
    pub error_wraps: HashMap<NodeId, ErrorWrap>,
    /// ER14a: `??` sites whose right side is still wrapped. There the present
    /// path hands back the left operand unchanged — unwrapping it would throw
    /// away the layer the chain is still carrying.
    pub fallback_keeps_shape: std::collections::HashSet<NodeId>,
    /// CM1: closure literals that outlive the frame that built them, so their
    /// captures travel with them instead of being pointed at. Worked out by the
    /// ownership pass and written back here, because lowering and the
    /// interpreter both have to agree with it.
    pub escaping_closures: std::collections::HashSet<NodeId>,
    /// Assignments whose new value is built out of the old one, so the slot's
    /// old value isn't released before the write (`rask_ownership`). Worked out
    /// by the ownership pass and written back here, like `escaping_closures`.
    pub field_reuses: std::collections::HashSet<NodeId>,
    /// Closure literals that capture a link or a `Local` box, so they may not
    /// reach another task (mem.ownership/T2, conc.sync/SH7). A task block that
    /// names one of these by itself is rejected at compile time; one that
    /// captures such a closure value is refused when the task starts, from a
    /// flag the closure carries (#1356).
    pub task_bound_closures: std::collections::HashSet<NodeId>,
    /// Closure literals in a generic body — task blocks' own included — that
    /// capture a name whose type mentions a type parameter, with each such
    /// capture's name and type. Whether one is task-bound depends on the
    /// instantiation, and both backends decide it from the substituted types
    /// through `TypeTable::generic_closure_task_bound`.
    pub generic_closure_captures: HashMap<NodeId, Vec<(String, Type)>>,
    /// ER16a: `try` node → the postfix-chain step it attaches to, when that
    /// isn't the operand itself. `try read_file(p).len()` maps the `try` to the
    /// `read_file(p)` call, so lowering branches there and hands `.len()` the
    /// payload. A `try` absent from this map wraps its whole operand.
    pub try_chain_placement: HashMap<NodeId, NodeId>,
    /// Unsafe operations recorded during type checking (span + category).
    pub unsafe_ops: Vec<(rask_ast::Span, super::UnsafeCategory)>,
    /// Types for binding names and parameters, keyed by (span.start, span.end, file_id).
    /// Used by the LSP for hover on identifiers that aren't expression nodes.
    pub span_types: HashMap<(usize, usize, u16), Type>,
    /// GC9: methods whose `self` is mutable, keyed by the method's span.
    ///
    /// `mutate self` and `take self` say so in the signature, but a private
    /// method may omit the mode and have it inferred from the body — so the
    /// signature alone doesn't answer the question. This is where the answer
    /// the checker already worked out gets recorded, so MIR lowering can read it
    /// rather than re-deriving GC9 with a second walker that could drift.
    ///
    /// Keyed by span because FnDecl has no NodeId, and spans survive the clone
    /// monomorphization makes of each generic instantiation.
    pub mutate_self_fns: std::collections::HashSet<(usize, usize, u16)>,
    /// T1: method-call spans that resolved to a channel `Sender.send`. Read by
    /// the ownership checker to transfer ownership of the sent value even when
    /// inference leaves the receiver as a type variable in `node_types`.
    pub channel_send_sites: std::collections::HashSet<rask_ast::Span>,
    /// Bare names in a pattern that the checker read as a type test rather
    /// than as a variant or a binding — `r is ParseError` — keyed by the span
    /// of the statement or expression the pattern sits in.
    ///
    /// A bare `ParseError` can be any of the three, and only the scrutinee's
    /// type says which. Renaming a type has to rename the first kind and leave
    /// `AppError`'s `ParseError` variant alone (`attach_derived`).
    pub type_test_patterns: std::collections::HashSet<(rask_ast::Span, String)>,
    /// Method calls that left parameters to their defaults: call → (position,
    /// the default's copy). The checker filled them where it knew which
    /// method the call reaches; `attach_derived` puts them in the call.
    pub default_fills: HashMap<NodeId, Vec<(usize, rask_ast::expr::Expr)>>,
    /// Function name → inferred return type, for functions that don't declare one
    /// (`func f() { return 41 }`). An absent annotation is not the same as
    /// returning nothing, and the declaration string is the only thing lowering
    /// can otherwise see — so without this the signature came out `void` while
    /// the body returned a value.
    pub inferred_fn_ret: HashMap<String, Type>,
    /// Function name → the types inferred for its untyped parameters
    /// (`func greet(name) { … }`), in declaration order and paired with the
    /// parameter name. The declaration string is empty for those, and empty
    /// reads as `void` everywhere downstream — which is how an inferred
    /// `string` parameter reached MIR as a void and printed as an address
    /// (#905). Written back into the declarations after checking.
    pub inferred_fn_params: HashMap<String, Vec<(String, Type)>>,
    /// Declarations the checker wrote and checked (`checker/derive.rs`).
    /// `attach_derived` moves them into the program.
    pub derived_decls: Vec<rask_ast::decl::Decl>,
    /// `==` calls on two wrappers that go through the wrapper's `eq`:
    /// call node → (callee node, function name). Applied by `attach_derived`.
    pub wrapper_eq_calls: HashMap<NodeId, (NodeId, String)>,
    /// A collection filling a `Sequence<E>` slot: value node → the node of
    /// the `as_sequence()` call `attach_derived` wraps it in (SEQ48).
    pub sequence_coercions: HashMap<NodeId, NodeId>,
    /// The `eq`/`hash` written for each wrapper type, for a map keyed by one.
    pub wrapper_fns: Vec<super::derive::WrapperFns>,
    /// The methods the checker wrote for generic types, as `Type_method`
    /// (`Slot_clone`). Reached only through a call pinned to the type, so
    /// mono never widens a call it couldn't pin onto one (#1434).
    pub derived_generic_methods: std::collections::HashSet<String>,
}

/// Give each type in `map` its new name, in its declaration and everywhere the
/// program names it.
///
/// `rask_ast::qualify` renames every bare name in a pattern, which is right
/// for a package's declarations and wrong here: a bare `ParseError` in a
/// pattern is just as often `AppError`'s variant of that name. Only the ones
/// the checker read as a type test are the type (`type_test_patterns`); a bare
/// constructor pattern is always a variant.
fn rename_types(
    decls: &mut [rask_ast::decl::Decl],
    map: &HashMap<String, String>,
    type_tests: &std::collections::HashSet<(rask_ast::Span, String)>,
) {
    use rask_ast::expr::{Expr, ExprKind, Pattern};
    use rask_ast::stmt::{Stmt, StmtKind};

    if map.is_empty() {
        return;
    }

    struct Types<'a> {
        map: &'a HashMap<String, String>,
        type_tests: &'a std::collections::HashSet<(rask_ast::Span, String)>,
        locals: std::collections::HashSet<String>,
    }

    impl Types<'_> {
        /// The bare type tests in one pattern, checked under `span`.
        fn type_tests_in(&self, p: &mut Pattern, span: rask_ast::Span) {
            match p {
                Pattern::Ident(name) => {
                    if self.type_tests.contains(&(span, name.clone())) {
                        if let Some(to) = self.map.get(name.as_str()) {
                            *name = to.clone();
                        }
                    }
                }
                Pattern::Constructor { fields, .. } => {
                    fields.iter_mut().for_each(|f| self.type_tests_in(f, span))
                }
                Pattern::Struct { fields, .. } => {
                    fields.iter_mut().for_each(|(_, f)| self.type_tests_in(f, span))
                }
                Pattern::Tuple(parts) | Pattern::Or(parts) => {
                    parts.iter_mut().for_each(|f| self.type_tests_in(f, span))
                }
                Pattern::Wildcard
                | Pattern::Literal(_)
                | Pattern::Range { .. }
                | Pattern::TypePat { .. } => {}
            }
        }
    }

    impl rask_ast::rewrite::Rewrite for Types<'_> {
        fn ty(&mut self, t: &mut rask_ast::ty::TypeExpr) {
            t.rename(&|name| self.map.get(name).cloned());
        }

        fn expr(&mut self, e: &mut Expr) {
            let span = e.span;
            match &mut e.kind {
                ExprKind::Ident(name) | ExprKind::GenericName { name, .. } => {
                    if !self.locals.contains(name.as_str()) {
                        if let Some(to) = self.map.get(name.as_str()) {
                            *name = to.clone();
                        }
                    }
                }
                ExprKind::StructLit { name, .. } => {
                    if let Some(to) = self.map.get(name.as_str()) {
                        *name = to.clone();
                    }
                }
                ExprKind::Match { arms, .. } => {
                    for arm in arms {
                        self.type_tests_in(&mut arm.pattern, span);
                    }
                }
                ExprKind::IfLet { pattern, .. }
                | ExprKind::GuardPattern { pattern, .. }
                | ExprKind::IsPattern { pattern, .. } => self.type_tests_in(pattern, span),
                _ => {}
            }
        }

        fn body(&mut self, b: &mut Vec<Stmt>) {
            for stmt in b {
                if let StmtKind::LetStruct { pattern, .. } = &mut stmt.kind {
                    let span = stmt.span;
                    self.type_tests_in(pattern, span);
                }
            }
        }

        fn pattern(&mut self, p: &mut Pattern) {
            // `ParseError.Plain` and `ParseError { .. }` name the type; a bare
            // name is handled with its span above.
            let name = match p {
                Pattern::Ident(name) | Pattern::Constructor { name, .. } if name.contains('.') => name,
                Pattern::Struct { name, .. } => name,
                _ => return,
            };
            match name.split_once('.') {
                Some((head, tail)) => {
                    if let Some(to) = self.map.get(head) {
                        *name = format!("{to}.{tail}");
                    }
                }
                None => {
                    if let Some(to) = self.map.get(name.as_str()) {
                        *name = to.clone();
                    }
                }
            }
        }
    }

    for decl in decls.iter_mut() {
        // A bare type test reads as a binding to a syntactic walk; it isn't one.
        let mut locals = rask_ast::qualify::names_bound_in(decl);
        locals.retain(|n| !type_tests.iter().any(|(_, t)| t == n));
        rask_ast::rewrite::rewrite_decl(decl, &mut Types { map, type_tests, locals });
        rask_ast::qualify::rename_declaration(decl, map);
    }
}
