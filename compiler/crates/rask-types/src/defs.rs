// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! What a type declaration says, once its fields and signatures are `Type`s.

use rask_ast::ty::ParamMode;

use crate::Type;

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
        /// `@reads_as(field)`: methods the type doesn't declare are the
        /// field's read-only ones, called on the field (type.reads-as).
        reads_as: Option<String>,
        /// `@temporary(keep: [...])`: a value lives for the expression that
        /// makes it, and these methods are the ways to keep one
        /// (type.temporary). `None` for an ordinary type.
        temporary: Option<Vec<String>>,
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
        /// `type Wrapping<T: Integer> = T`: the `T`. The underlying type is
        /// written in them.
        type_params: Vec<String>,
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
const CONFORMANCE_SEP: char = '~';

pub fn conformance_symbol(base: &str, package: &str) -> String {
    format!("{}{}{}", base, CONFORMANCE_SEP, package)
}

impl TypeDef {
    /// The type's declared parameters: a generic struct's, enum's or
    /// newtype's. Empty for anything else.
    pub fn type_params(&self) -> &[String] {
        match self {
            TypeDef::Struct { type_params, .. }
            | TypeDef::Enum { type_params, .. }
            | TypeDef::NominalAlias { type_params, .. } => type_params,
            _ => &[],
        }
    }

    /// TR3: true if `method` is a generic method of this interface (can't dispatch through `any`).
    /// Interface method names carry their type params (`convert<T>`); the call site does
    /// not, so compare on the base name.
    pub fn is_generic_interface_method(&self, method: &str) -> bool {
        matches!(self, TypeDef::Interface { generic_methods, .. }
            if generic_methods.iter().any(|m| m == method))
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
    /// Who wrote it, and whether it has a body.
    pub origin: MethodOrigin,
    /// What it does to its receiver's size, as declared (`@grows`, `@shrinks`).
    pub effect: rask_ast::decl::DeclaredEffects,
    /// Declared `public`. A checker-supplied signature has no declaration and
    /// is callable from anywhere, so it counts as public.
    pub is_public: bool,
    /// Each parameter's declared default, positionally matching `params`.
    /// Empty for a signature with no declaration behind it.
    pub defaults: Vec<Option<rask_ast::expr::Expr>>,
}

/// Where a method came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MethodOrigin {
    /// Written in the program or the stdlib.
    Declared,
    /// The checker supplied it (EQ1, HA1, ORD1 and the rest) and the backends
    /// answer it structurally: there is no body behind it.
    Structural,
    /// The checker supplied it and wrote its body (`derive.rs`). Runs no code
    /// the program wrote beyond what its fields' own methods run.
    Derived,
}

impl MethodOrigin {
    /// A call to it is a call: there is a body to run.
    pub fn has_body(self) -> bool {
        self != MethodOrigin::Structural
    }
}

/// How self is passed to a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfParam {
    None,   // Static method
    Value,  // self (read-only, default)
    Mutate, // mutate self (mutable)
    Take,   // take self (consumed)
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
