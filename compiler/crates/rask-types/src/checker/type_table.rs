// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Central type registry.

use std::collections::HashMap;

use rask_ast::{NodeId, Span};
use rask_ast::ty::TypeExpr;

use super::builtins::BuiltinModules;
use super::type_defs::{BinaryStructInfo, TypeDef};
use super::errors::{MapKeyFix, TypeError};

use crate::types::{GenericArg, Type, TypeId, TypeVarId};

/// What keeps a value on its task. See `TypeTable::task_bound`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskBound {
    LocalBox,
    Link,
}

/// MN3/XC3: an `T implements Interface` block, as the conformance table remembers
/// it. Auto-derive records no site at all, so having one means it was written.
///
/// `from_stdlib` is what XC3 turns on. It has to be the *first* registration's,
/// not the pass currently running: a program overriding a stdlib type's
/// `Displayable` was reported as a same-package duplicate when the guard read
/// the current pass's mode instead.
#[derive(Debug, Clone)]
pub(super) struct ConformanceSite {
    pub span: Span,
    pub decl: NodeId,
    pub from_stdlib: bool,
    /// XC4: the package whose source this block is in. `None` for the stdlib,
    /// for a single-file program, and for anything the compiler generated —
    /// all cases where there is no package to compare.
    pub package: Option<String>,
}

/// Which interface a conformance is to.
///
/// A name isn't enough: a program's `interface Writer` and the stdlib's are two
/// interfaces, and `Buffer implements Writer` is a claim about the stdlib's
/// only (#1329). An interface the compiler provides without a declaration has
/// nothing but its name.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InterfaceIdent {
    Declared(TypeId),
    Builtin(String),
}

/// GT2/GT3: what a conformance is filed under — the interface, and how its
/// parameters were applied (`Mul<f64>` and `Mul<Meters>` are two).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ConformanceKey {
    pub iface: InterfaceIdent,
    /// Defaults filled in and `Self` replaced by the conforming type's name.
    pub applied: TypeExpr,
}

/// XC1: who a type belongs to.
///
/// The rule is "only the package that declares `T` may declare these six
/// conformances for it", and a builtin has a declarer too — the standard
/// library. Treating "no package" as "nothing to check" is what let a plain
/// program give `Vec<i64>` its own `Hashable` and have every `Map` and `Set`
/// keyed on it start missing entries, which is the exact hazard the rule
/// exists to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum TypeOwner {
    /// The standard library, including every builtin.
    Stdlib,
    /// A package in this build.
    Package(String),
    /// The program itself, where there is no package to name — a single file.
    Program,
}

/// Central registry of all types in the program.
#[derive(Debug, Default, Clone)]
pub struct TypeTable {
    /// User-defined types indexed by TypeId.
    pub(super) types: Vec<TypeDef>,
    /// Name to TypeId mapping, as seen from program code.
    pub(super) type_names: HashMap<String, TypeId>,
    /// Name to TypeId mapping, as seen from stdlib code.
    ///
    /// A program may declare `struct Headers` over the stdlib's, and its own
    /// type wins — but only for *its* references. The stdlib's body still has
    /// to mean the stdlib's `Headers`. One flat map can't say that: the second
    /// registration overwrites the first, and the name then resolves to
    /// whichever declaration happened to come last (#515).
    ///
    /// Both types exist either way; this is only about which one a name means
    /// where. `type_method_decls` already binds methods by TypeId for the same
    /// reason.
    pub(super) stdlib_type_names: HashMap<String, TypeId>,
    /// A program type sharing a stdlib type's name, by its symbol: the name as
    /// the program wrote it.
    ///
    /// The checker tells the two apart by `TypeId`, but everything after it
    /// keys types, methods and layouts by name, so two types called
    /// `ParseError` were one type there and one `ParseError_message` served
    /// both (#1333). The program's gets a name of its own from the moment it is
    /// registered (`ParseError#57` — `#` is in no identifier), and this is how
    /// a message or a printed value still says what the program wrote.
    pub(super) written_names: HashMap<String, String>,
    /// Whether registrations and lookups are on behalf of stdlib code.
    /// Mirrors the resolver's flag of the same name.
    pub(super) stdlib_mode: bool,
    /// Built-in type names mapped to Type.
    pub(super) builtins: HashMap<String, Type>,
    /// Type alias name → target, as program code reads them: the program's
    /// own aliases and imports, then the stdlib's.
    type_aliases: HashMap<String, TypeExpr>,
    /// The stdlib's aliases alone, which is all stdlib code sees. A program's
    /// `import time.Duration as Span` is not the prelude's `Span` (#1479).
    stdlib_aliases: HashMap<String, TypeExpr>,
    /// Type parameter names in scope right now — the declaration or signature
    /// being checked.
    ///
    /// A declared parameter has to win over a type of the same name, or
    /// `struct Holder<Output>` silently means the stdlib's `os.Output` and
    /// every use of the field is a mismatch against a type nobody wrote (#915).
    /// Scoped rather than global: `Output` is a parameter inside that
    /// declaration and the stdlib type everywhere else.
    pub(super) type_param_scope: Vec<String>,
    /// Module-level `const` names whose initializer is an integer literal,
    /// mapped to that value.
    ///
    /// Only array lengths read this. `[i32; W]` names a length that has to be
    /// known to give the type a size, and without the value the length came out
    /// 0 — `len()` folded to zero and `for x in a` ran no iterations (#906).
    /// A computed initializer (`const W = 2 * 2`) isn't here; that wants
    /// comptime evaluation, and a symbolic length still falls back to 0.
    pub(super) const_lengths: HashMap<String, usize>,
    /// TypeId for the builtin Option<T> enum.
    pub(super) option_type_id: Option<TypeId>,
    /// TypeId for the builtin Result<T, E> enum.
    pub(super) result_type_id: Option<TypeId>,
    /// Builtin modules registry.
    pub(super) builtin_modules: BuiltinModules,
    /// `import time as tm`: `tm` → `time`.
    module_aliases: HashMap<String, String>,
    /// B1–G4: binary struct metadata indexed by TypeId
    pub binary_structs: HashMap<TypeId, BinaryStructInfo>,
    /// Field names of a struct-shaped enum variant, keyed by
    /// `(enum TypeId, variant name)` and in declaration order.
    ///
    /// `TypeDef::Enum` keeps payload types positionally, which is all a tuple
    /// variant needs. A struct variant's pattern names its fields
    /// (`Outer.Named { code, kind }`), so matching them to types needs the names
    /// too — without them the checker gave every field a fresh variable and
    /// `let x: i64 = kind` type-checked (#809).
    pub(super) variant_field_names: HashMap<(TypeId, String), Vec<String>>,
    /// AST declarations contributing methods to each type: the `struct`/`enum`
    /// itself plus every `extend` block bound to it.
    ///
    /// Two types can share a name — a program type shadows a stdlib one — and
    /// then `type_names` only remembers the winner. Monomorphization needs the
    /// loser's methods too, and a mangled `Type_method` string can't tell them
    /// apart. Binding happens here, where the TypeId is still known.
    pub(super) type_method_decls: HashMap<TypeId, Vec<NodeId>>,
    /// V1, V2, V5: methods a caller may not see from everywhere, by the name
    /// they're filed under. A public method, or one in a conformance block,
    /// has no entry.
    pub(super) method_access: HashMap<(TypeId, String), super::method_visibility::MethodAccess>,
    /// G1: declared/derived interface conformances (nominal). TypeId → the interfaces
    /// the type conforms to, from `T implements Interface` and auto-derive.
    pub(super) conformances: HashMap<TypeId, std::collections::HashSet<ConformanceKey>>,
    /// AT2/AT8: `(type, applied interface) → associated type → what it answers with`.
    pub(super) assoc_bindings: HashMap<(TypeId, ConformanceKey), HashMap<String, Type>>,
    /// MN3/XC3: where each conformance was written, so a collision between two
    /// of them is reported once, on the later one.
    pub(super) conformance_spans: HashMap<(TypeId, ConformanceKey), Vec<ConformanceSite>>,
    /// CC1/CC2: conditional-conformance conditions. (TypeId, interface) → the
    /// `where` bounds (type-param name → required interface names) that must hold
    /// for the conformance, checked per instantiation.
    pub(super) conformance_conditions: HashMap<(TypeId, InterfaceIdent), Vec<(String, Vec<TypeExpr>)>>,
    /// XC4/XC5: which package's `extend` block each method on a type came from,
    /// and which block that was. `(TypeId, method name) → [(package, impl decl)]`.
    ///
    /// One type can end up holding two `label`s, from two packages, and they are
    /// indistinguishable by signature — that is the whole point of the
    /// collision. This says which is which, so a call from `liba` reaches
    /// `liba`'s body and monomorphization emits both instead of one winning.
    pub(super) impl_method_packages:
        HashMap<(TypeId, String), Vec<(String, NodeId)>>,
    /// XC3: `(type, applied interface)` pairs with more than one written
    /// declaration. Empty in every program that doesn't have a collision, which
    /// is nearly all of them — the use-site check reads this first and does
    /// nothing when it's empty.
    pub(super) ambiguous_conformances: std::collections::HashSet<(TypeId, ConformanceKey)>,
    /// XC1: who declares each type. Anything unrecorded is a builtin, and
    /// builtins are the stdlib's.
    pub(super) declared_by: HashMap<TypeId, TypeOwner>,
    /// XC1: where each type was declared. The span's file id says which package
    /// wrote it, which is how a conformance block knows whether it owns the
    /// type it's extending.
    pub(super) declared_at: HashMap<TypeId, Span>,
    /// The bounds a generic struct or enum declares on its parameters, in
    /// declaration order: `struct Holder<T: Named>` → `[("T", [Named])]`.
    /// Every method of the type may assume them (#1364).
    pub(super) declared_param_bounds: HashMap<TypeId, Vec<(String, Vec<rask_ast::ty::TypeExpr>)>>,
    /// OR1: the conformances, read the other way round — applied interface
    /// (`Mul<Duration>`) → the types that answer it.
    ///
    /// `3 * duration` asks "which type forms this pair with `Duration`", which
    /// the by-`Self` table can only answer by walking every entry. One insert
    /// here on the way in makes it a lookup.
    pub(super) conformers_by_pair: HashMap<ConformanceKey, Vec<TypeId>>,
    /// OR12: conformance methods declared `@builtin` — the pair's types are
    /// written in the stdlib and the arithmetic is the compiler's, so there is
    /// no body to call. Keyed `(type, filed method name)`.
    pub(super) builtin_methods: std::collections::HashSet<(TypeId, String)>,
    /// OR6: the `TypeDef::Primitive` standing in for each primitive, so a
    /// conformance written against one has a `TypeId` to be filed under.
    ///
    /// Deliberately not `type_names`: a name that resolves there becomes
    /// `Type::Named(id)` in a signature, and `f64` has to stay `Type::F64`.
    pub(super) primitive_ids: HashMap<String, TypeId>,
}

impl TypeTable {
    pub fn new() -> Self {
        let mut table = Self {
            types: Vec::new(),
            type_names: HashMap::new(),
            stdlib_type_names: HashMap::new(),
            written_names: HashMap::new(),
            stdlib_mode: false,
            builtins: HashMap::new(),
            type_aliases: HashMap::new(),
            stdlib_aliases: HashMap::new(),
            type_param_scope: Vec::new(),
            const_lengths: HashMap::new(),
            option_type_id: None,
            result_type_id: None,
            builtin_modules: BuiltinModules::new(),
            module_aliases: HashMap::new(),
            binary_structs: HashMap::new(),
            variant_field_names: HashMap::new(),
            type_method_decls: HashMap::new(),
            method_access: HashMap::new(),
            conformances: HashMap::new(),
            assoc_bindings: HashMap::new(),
            conformance_spans: HashMap::new(),
            conformance_conditions: HashMap::new(),
            declared_at: HashMap::new(),
            declared_param_bounds: HashMap::new(),
            declared_by: HashMap::new(),
            ambiguous_conformances: std::collections::HashSet::new(),
            impl_method_packages: HashMap::new(),
            primitive_ids: HashMap::new(),
            builtin_methods: std::collections::HashSet::new(),
            conformers_by_pair: HashMap::new(),
        };
        table.register_builtins();
        table
    }

    fn register_builtins(&mut self) {
        self.builtins.insert("i8".to_string(), Type::I8);
        self.builtins.insert("i16".to_string(), Type::I16);
        self.builtins.insert("i32".to_string(), Type::I32);
        self.builtins.insert("i64".to_string(), Type::I64);
        self.builtins.insert("u8".to_string(), Type::U8);
        self.builtins.insert("u16".to_string(), Type::U16);
        self.builtins.insert("u32".to_string(), Type::U32);
        self.builtins.insert("u64".to_string(), Type::U64);
        self.builtins.insert("i128".to_string(), Type::I128);
        self.builtins.insert("u128".to_string(), Type::U128);
        self.builtins.insert("f32".to_string(), Type::F32);
        self.builtins.insert("f64".to_string(), Type::F64);
        self.builtins.insert("bool".to_string(), Type::Bool);
        self.builtins.insert("char".to_string(), Type::Char);
        self.builtins.insert("string".to_string(), Type::String);
        self.builtins.insert("()".to_string(), Type::Unit);
        self.builtins.insert("void".to_string(), Type::Unit);
        self.builtins.insert("none".to_string(), Type::None);
        self.builtins.insert("int".to_string(), Type::I64);
        self.builtins.insert("uint".to_string(), Type::U64);
        self.builtins.insert("isize".to_string(), Type::isize_ty());
        self.builtins.insert("usize".to_string(), Type::usize_ty());
        self.builtins.insert("Never".to_string(), Type::Never);

        // The C scalar names an `import c` header translates to. Until these
        // were here none of them was a type, so `*i64` passed for a `const int
        // *` and the C side read a 64-bit buffer as 32-bit ints (#947).
        for name in rask_ast::primitives::C_SCALARS {
            let spelling = rask_ast::primitives::c_type_spelling(name)
                .expect("C_SCALARS and c_type_spelling must agree");
            let ty = self
                .builtins
                .get(spelling)
                .cloned()
                .expect("a c_* type resolves to a primitive spelling");
            self.builtins.insert((*name).to_string(), ty);
        }

        // OR6: one entry per primitive, so a conformance written against one
        // has a `TypeId` to be filed under and the methods it brings have
        // somewhere to live.
        for name in PRIMITIVE_CONFORMANCE_TARGETS {
            // Straight into the table, deliberately skipping `register_type`:
            // the name maps are what turn a spelling into `Type::Named`, and
            // `string` going through them made `string.from_utf8(…)` resolve
            // against an entry with no methods on it.
            let id = TypeId(self.types.len() as u32);
            self.types.push(TypeDef::Primitive {
                name: (*name).to_string(),
                methods: Vec::new(),
            });
            self.primitive_ids.insert((*name).to_string(), id);
        }

        let option_id = self.register_type(TypeDef::Enum {
            name: "Option".to_string(),
            type_params: vec!["T".to_string()],
            variants: vec![
                ("Some".to_string(), vec![Type::Var(TypeVarId(0))]),
                ("None".to_string(), vec![]),
            ],
            methods: vec![],
            is_transitive_resource: false,
            no_encode: false,
            no_decode: false,
        });
        self.option_type_id = Some(option_id);

        let result_id = self.register_type(TypeDef::Enum {
            name: "Result".to_string(),
            type_params: vec!["T".to_string(), "E".to_string()],
            variants: vec![
                ("Ok".to_string(), vec![Type::Var(TypeVarId(0))]),
                ("Err".to_string(), vec![Type::Var(TypeVarId(1))]),
            ],
            methods: vec![],
            is_transitive_resource: false,
            no_encode: false,
            no_decode: false,
        });
        self.result_type_id = Some(result_id);

        // Comparison result (ORD1) plus atomic memory orderings share one enum,
        // matching the resolver and interpreter registrations. Without a real
        // TypeDef entry, a user-written `Ordering` annotation is rejected as an
        // unknown type even though the name resolves.
        self.register_type(TypeDef::Enum {
            name: "Ordering".to_string(),
            type_params: vec![],
            variants: rask_stdlib::ORDERING_VARIANTS
                .iter()
                .map(|v| (v.to_string(), vec![]))
                .collect(),
            methods: vec![],
            is_transitive_resource: false,
            no_encode: false,
            no_decode: false,
        });
    }

    /// Register a user-defined type.
    ///
    /// If the type is `Option` or `Result` (already registered as builtins),
    /// merge methods into the existing builtin entry instead of creating a
    /// duplicate. This keeps `T?` / `T or E` sugar unifying cleanly with
    /// explicit `Option<T>` / `Result<T, E>` from stdlib source.
    pub fn register_type(&mut self, def: TypeDef) -> TypeId {
        let name = match &def {
            TypeDef::Struct { name, .. } => name.clone(),
            TypeDef::Enum { name, .. } => name.clone(),
            TypeDef::Interface { name, .. } => name.clone(),
            TypeDef::Union { name, .. } => name.clone(),
            TypeDef::NominalAlias { name, .. } => name.clone(),
            TypeDef::Primitive { name, .. } => name.clone(),
        };

        // Option/Result have fixed builtin TypeIds. Redeclaration from stdlib
        // (e.g., `enum Option<T> { ... }` in option.rk) must merge methods
        // into the existing entry rather than duplicating it, so `T?` sugar
        // and `Option<T>` resolve to the same TypeId.
        //
        // Match on the base name (strip generic params) since the parser
        // stores names with their generic signature (e.g. "Option<T>").
        let base_name = name.as_str();
        let builtin_id = match base_name {
            "Option" => self.option_type_id,
            "Result" => self.result_type_id,
            _ => None,
        };
        if let Some(existing_id) = builtin_id {
            if let TypeDef::Enum { methods: new_methods, .. } = def {
                if let Some(TypeDef::Enum { methods, .. }) = self.types.get_mut(existing_id.0 as usize) {
                    methods.extend(new_methods);
                }
            }
            return existing_id;
        }

        let id = TypeId(self.types.len() as u32);
        let mut def = def;
        if self.stdlib_mode {
            // Stdlib code always means this one.
            self.stdlib_type_names.insert(name.clone(), id);
            // Program code means it too, unless the program declares its
            // own. Registering stdlib first and not overwriting later is
            // what makes a program type shadow rather than collide.
            self.type_names.entry(name).or_insert(id);
        } else {
            // Program code reaches it by what it wrote; anything minted from
            // the table names it by its symbol.
            self.type_names.insert(name.clone(), id);
            if self.shadows_stdlib_type(&name, &def) {
                let symbol = format!("{name}#{}", id.0);
                *Self::def_name_mut(&mut def) = symbol.clone();
                self.type_names.insert(symbol.clone(), id);
                self.written_names.insert(symbol, name);
            }
        }
        self.types.push(def);
        id
    }

    /// Does a program declaration of `name` share it with a stdlib type?
    ///
    /// An interface is left out: `interface_symbol` names those, and only
    /// where a type is written as `any I`.
    fn shadows_stdlib_type(&self, name: &str, def: &TypeDef) -> bool {
        !matches!(def, TypeDef::Interface { .. } | TypeDef::Primitive { .. })
            && self.stdlib_type_names.contains_key(name)
    }

    /// Hand each written name back to the stdlib once the program's own uses
    /// say the symbol (`TypedProgram::attach_derived`), and return written →
    /// symbol for that rewrite.
    ///
    /// From there on `ParseError` is the stdlib's to every pass, the same as
    /// inside the stdlib's bodies, and the program's type is `ParseError#57`.
    pub(super) fn release_written_names(&mut self) -> HashMap<String, String> {
        let mut renamed = HashMap::new();
        for (symbol, written) in &self.written_names {
            if let Some(std) = self.stdlib_type_names.get(written) {
                self.type_names.insert(written.clone(), *std);
            }
            renamed.insert(written.clone(), symbol.clone());
        }
        renamed
    }

    /// `time.Duration`: the type a stdlib module exports under `name`.
    /// `module` is the spelling the program used, so `tm.Duration` under
    /// `import time as tm` is the same type.
    ///
    /// The module says whose declaration is meant, so this never answers with
    /// the program's type. Dropping the module and looking the bare name up
    /// did, whenever the program declared a `Duration` of its own (#1470).
    pub fn module_type_id(&self, module: &str, name: &str) -> Option<TypeId> {
        let module = self.module_named(module)?;
        if !rask_stdlib::modules::exports_type(module, name) {
            return None;
        }
        self.stdlib_type_names.get(name).copied()
    }

    /// The stdlib module a spelling names: the module's own name, or what an
    /// `import m as alias` bound.
    pub fn module_named<'a>(&'a self, spelled: &'a str) -> Option<&'a str> {
        if rask_stdlib::modules::is_module(spelled) {
            return Some(spelled);
        }
        self.module_aliases.get(spelled).map(String::as_str)
    }

    /// Record `import m as alias`.
    pub(super) fn register_module_alias(&mut self, alias: String, module: String) {
        self.module_aliases.insert(alias, module);
    }

    /// The stdlib's type of this name, when the program declares its own.
    fn shadowed_stdlib_type(&self, name: &str) -> Option<TypeId> {
        let std = *self.stdlib_type_names.get(name)?;
        (self.type_names.get(name) != Some(&std)).then_some(std)
    }

    /// A type name as the program wrote it, for a message or a printed value.
    /// The name itself for every type that has no symbol of its own.
    pub fn written_name<'a>(&'a self, name: &'a str) -> &'a str {
        self.written_names.get(name).map_or(name, String::as_str)
    }

    fn def_name_mut(def: &mut TypeDef) -> &mut String {
        match def {
            TypeDef::Struct { name, .. }
            | TypeDef::Enum { name, .. }
            | TypeDef::Interface { name, .. }
            | TypeDef::Union { name, .. }
            | TypeDef::NominalAlias { name, .. }
            | TypeDef::Primitive { name, .. } => name,
        }
    }

    /// The name map to consult first, given who's asking.
    fn primary_names(&self) -> &HashMap<String, TypeId> {
        if self.stdlib_mode { &self.stdlib_type_names } else { &self.type_names }
    }

    /// The other one, for names the primary doesn't know.
    fn fallback_names(&self) -> &HashMap<String, TypeId> {
        if self.stdlib_mode { &self.type_names } else { &self.stdlib_type_names }
    }

    /// Was this type declared by the stdlib? A program type of the same name
    /// shadows it in `type_names`, never in `stdlib_type_names`.
    fn declared_in_stdlib(&self, id: TypeId) -> bool {
        self.get(id)
            .is_some_and(|def| self.stdlib_type_names.get(Self::def_name(def)) == Some(&id))
    }

    /// A name as the code that declared `owner` reads it.
    ///
    /// A stdlib interface's `: Writer` means the stdlib's `Writer` even while
    /// a program one shadows it, and the program's checks reach stdlib
    /// interfaces through their parents too (#1329).
    pub fn resolve_name_as_declared_by(&self, owner: TypeId, name: &str) -> Option<TypeId> {
        let (primary, fallback) = if self.declared_in_stdlib(owner) {
            (&self.stdlib_type_names, &self.type_names)
        } else {
            (&self.type_names, &self.stdlib_type_names)
        };
        primary.get(name).or_else(|| fallback.get(name)).copied()
    }

    /// Resolve a type name from the current scope.
    fn resolve_name(&self, name: &str) -> Option<TypeId> {
        self.primary_names()
            .get(name)
            .or_else(|| self.fallback_names().get(name))
            .copied()
    }

    /// Note that `decl` declares methods on `id`.
    pub fn record_method_decl(&mut self, id: TypeId, decl: NodeId) {
        let decls = self.type_method_decls.entry(id).or_default();
        if !decls.contains(&decl) {
            decls.push(decl);
        }
    }

    /// Note who may call a method that isn't public.
    pub(super) fn record_method_access(
        &mut self,
        id: TypeId,
        method: &str,
        access: super::method_visibility::MethodAccess,
    ) {
        self.method_access.insert((id, method.to_string()), access);
    }

    /// Who may call this method, when not everyone may.
    pub(super) fn method_access(
        &self,
        id: TypeId,
        method: &str,
    ) -> Option<&super::method_visibility::MethodAccess> {
        self.method_access.get(&(id, method.to_string()))
    }

    /// Every type that declares methods, paired with the declarations carrying them.
    pub fn types_with_methods(&self) -> impl Iterator<Item = (TypeId, &[NodeId])> {
        self.type_method_decls.iter().map(|(id, decls)| (*id, decls.as_slice()))
    }

    /// Register a transparent type alias.
    ///
    /// Scoped like a declared type: a stdlib alias is the stdlib's and, unless
    /// the program takes the name, the program's too; a program alias is the
    /// program's only.
    pub fn register_alias(&mut self, name: String, target: TypeExpr) {
        if self.stdlib_mode {
            self.stdlib_aliases.insert(name.clone(), target.clone());
            self.type_aliases.entry(name).or_insert(target);
        } else {
            self.type_aliases.insert(name, target);
        }
    }

    /// The aliases the code being checked can see.
    pub(super) fn aliases(&self) -> &HashMap<String, TypeExpr> {
        if self.stdlib_mode { &self.stdlib_aliases } else { &self.type_aliases }
    }

    /// The type `name` is an alias for, following a chain of aliases that name
    /// other aliases. `None` if it isn't an alias.
    ///
    /// Public because a name used as a *namespace* — `Span.from_millis(1)` — is
    /// matched against the stub registry by its spelling, and an alias isn't in
    /// there under its own name.
    pub fn alias_target(&self, name: &str) -> Option<&TypeExpr> {
        let aliases = self.aliases();
        let mut target = aliases.get(name)?;
        let mut seen = vec![name];
        // A cycle was rejected at registration; `seen` only keeps a bad table
        // from looping.
        while let Some(next) = target.bare_name().and_then(|n| aliases.get(n)) {
            let n = target.bare_name().unwrap_or_default();
            if seen.contains(&n) {
                return None;
            }
            seen.push(n);
            target = next;
        }
        Some(target)
    }

    /// `ty` with every transparent alias in it replaced by what it stands for,
    /// at any depth: `Names?` with `Names = Vec<string>` is `Vec<string>?`.
    pub fn expand_aliases(&self, ty: &TypeExpr) -> TypeExpr {
        if self.aliases().is_empty() {
            return ty.clone();
        }
        // `alias_target` follows bare-name chains; a target that holds another
        // alias deeper in (`Vec<Names>`) is expanded by the recursion. Cycles
        // were refused at registration, so the recursion ends.
        ty.substitute(&|name| self.alias_target(name).map(|t| self.expand_aliases(t)))
    }

    /// The name an alias stands for, when its target is a plain named type.
    pub fn alias_target_name(&self, name: &str) -> Option<String> {
        self.alias_target(name).filter(|t| t.args().is_empty()).and_then(TypeExpr::name)
    }

    /// Check if registering `name -> target` would create a cycle.
    /// Returns the cycle path if so.
    pub fn check_alias_cycle(&self, name: &str, target: &TypeExpr) -> Option<Vec<String>> {
        let mut path = vec![name.to_string(), target.to_string()];
        let mut current = target;
        loop {
            let current_name = current.bare_name()?;
            if current_name == name {
                return Some(path);
            }
            let next = self.aliases().get(current_name)?;
            path.push(next.to_string());
            current = next;
        }
    }

    /// Look up a type by name.
    pub fn lookup(&self, name: &str) -> Option<Type> {
        if let Some(ty) = self.builtins.get(name) {
            return Some(ty.clone());
        }
        if let Some(target) = self.alias_target(name) {
            return super::parse_type::resolve_type_expr(target, self).ok();
        }
        self.resolve_name(name).map(Type::Named)
    }

    /// The `(field name, type)` pairs of a struct-shaped enum variant named
    /// `Enum.Variant`, in declaration order. `None` for anything else — a plain
    /// struct, a tuple variant, an unknown name.
    pub fn struct_variant_fields(&self, qualified: &str) -> Option<Vec<(String, Type)>> {
        let (enum_name, variant) = qualified.rsplit_once('.')?;
        let enum_id = self.get_type_id(enum_name)?;
        let names = self.variant_field_names.get(&(enum_id, variant.to_string()))?;
        let TypeDef::Enum { variants, .. } = self.get(enum_id)? else { return None };
        let (_, types) = variants.iter().find(|(v, _)| v == variant)?;
        if names.len() != types.len() {
            return None;
        }
        Some(names.iter().cloned().zip(types.iter().cloned()).collect())
    }

    /// Get a type definition by ID.
    pub fn get(&self, id: TypeId) -> Option<&TypeDef> {
        self.types.get(id.0 as usize)
    }

    /// Get a mutable type definition by ID.
    pub fn get_mut(&mut self, id: TypeId) -> Option<&mut TypeDef> {
        self.types.get_mut(id.0 as usize)
    }

    /// The interface an interface reference names: `Mul<f64>` → `Mul`, and
    /// `io.Writer` → `Writer`, the name the module's interface is held under.
    /// The same unwrapping `io.Buffer` gets as a type (#1310).
    pub fn conformance_key(interface: &TypeExpr) -> String {
        Self::stdlib_module_member(interface).unwrap_or_else(|| interface.name().unwrap_or_default())
    }

    /// The interface declaration a written reference names. A module-qualified
    /// one is the module's: `io.Writer` is the stdlib's `Writer` even where the
    /// program declares its own, in a bound, a conformance header and `any`
    /// alike (#1467). Dropping the module and looking the bare name up found
    /// the program's. Anything else is the name as the code being checked
    /// means it. `None` for an interface the compiler provides by name only.
    pub fn interface_decl(&self, written: &TypeExpr) -> Option<TypeId> {
        let is_interface = |id: &TypeId| matches!(self.get(*id), Some(TypeDef::Interface { .. }));
        if let Some(id) = self.module_interface(written) {
            return Some(id);
        }
        self.get_type_id(&Self::conformance_key(written)).filter(is_interface)
    }

    /// `io.Writer`: the interface a stdlib module declares, when the reference
    /// is written through one (an `import io as i` alias counts). Never the
    /// program's interface of the same name, the way `module_type_id` never
    /// answers with the program's type.
    pub fn module_interface(&self, written: &TypeExpr) -> Option<TypeId> {
        let TypeExpr::Named { path, .. } = written else { return None };
        let [module, member] = path.as_slice() else { return None };
        let module = self.module_named(module)?;
        if !rask_stdlib::modules::exports_interface(module, member) {
            return None;
        }
        self.stdlib_type_names
            .get(member)
            .copied()
            .filter(|id| matches!(self.get(*id), Some(TypeDef::Interface { .. })))
    }

    /// A parent `interface`'s declaration lists, as that declaration's own side
    /// reads it: a stdlib interface's parents are the stdlib's.
    pub fn parent_interface(&self, interface: TypeId, parent: &TypeExpr) -> Option<TypeId> {
        self.module_interface(parent)
            .or_else(|| self.resolve_name_as_declared_by(interface, &Self::conformance_key(parent)))
    }

    /// `interface_decl`, or the compiler-provided interface of that name.
    pub fn written_interface_ident(&self, written: &TypeExpr) -> InterfaceIdent {
        self.interface_decl(written)
            .map_or_else(|| InterfaceIdent::Builtin(Self::conformance_key(written)), InterfaceIdent::Declared)
    }

    /// GT2/GT3: the key a conformance is filed under — the interface *with its
    /// arguments*, so `Mul<f64>` and `Mul<Meters>` on one type stay apart.
    ///
    /// Written-out defaults are filled in and `Self` becomes the conforming
    /// type's name, so `Meters implements Mul` and `Meters implements
    /// Mul<Meters>` land on the same key when `Rhs` defaults to `Self`.
    /// An interface with no parameters keys on its bare name.
    ///
    /// The interface is the one the code being checked means by the name.
    pub fn applied_conformance_key(&self, interface: &TypeExpr, self_name: &str) -> ConformanceKey {
        let iface = self.written_interface_ident(interface);
        self.applied_key_for(iface, interface, self_name)
    }

    /// `applied_conformance_key` for an interface already identified.
    pub fn applied_key_for(
        &self,
        iface: InterfaceIdent,
        interface: &TypeExpr,
        self_name: &str,
    ) -> ConformanceKey {
        let base = Self::conformance_key(interface);
        let bare = |iface| ConformanceKey { iface, applied: TypeExpr::named(base.clone()) };
        let type_params = match &iface {
            InterfaceIdent::Declared(id) => match self.get(*id) {
                Some(TypeDef::Interface { type_params, .. }) => type_params,
                _ => return bare(iface),
            },
            InterfaceIdent::Builtin(_) => return bare(iface),
        };
        if type_params.is_empty() {
            return bare(iface);
        }
        let written = interface.args();
        let mut args = Vec::new();
        for (i, p) in type_params.iter().enumerate() {
            let arg = match written.get(i).or(p.default.as_ref()) {
                Some(a) => a,
                // GT4: no argument and no default. The arity error is
                // reported at the header; key on what was written so the
                // conformance still exists for everything else.
                None => return bare(iface),
            };
            args.push(if arg.is_name("Self") { TypeExpr::named(self_name) } else { arg.clone() });
        }
        ConformanceKey { iface, applied: TypeExpr::generic(base, args) }
    }

    /// `any name`, as the code being checked means it: the interface it
    /// declares or imports under that name, or one the compiler provides.
    pub fn interface_object(&self, name: &str) -> Type {
        let decl = self
            .get_type_id(name)
            .filter(|id| matches!(self.get(*id), Some(TypeDef::Interface { .. })));
        Type::InterfaceObject { interface_name: name.to_string(), decl }
    }

    /// `any I` for the interface reference as written. A module-qualified
    /// spelling names the module's interface: `io.Writer` is the stdlib's
    /// `Writer` even where the program declares one of its own, the same way
    /// a stdlib signature's `any Writer` is (#1426). Otherwise the name is
    /// looked up as the table holds it — `io$Writer` when a package folded the
    /// module prefix into the key. Anything the table doesn't know keeps the
    /// spelling it was written with, so "no interface named `io.Writer`" still
    /// names what the author typed.
    pub fn interface_object_written(&self, written: &TypeExpr) -> Type {
        if let Some(id) = self.module_interface(written) {
            return Type::InterfaceObject { interface_name: self.type_name(id), decl: Some(id) };
        }
        self.interface_object(&self.interface_name_written(written))
    }

    /// `Writer` for `io.Writer`: the name a stdlib module's member is held
    /// under. `None` for anything not written through a stdlib module.
    fn stdlib_module_member(written: &TypeExpr) -> Option<String> {
        match written {
            TypeExpr::Named { path, .. } => match path.as_slice() {
                [module, rest @ ..] if !rest.is_empty() && rask_stdlib::modules::is_module(module) => {
                    Some(rest.join("."))
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn interface_name_written(&self, written: &TypeExpr) -> String {
        let path: Vec<String> = match written {
            TypeExpr::Named { path, .. } => path.clone(),
            other => vec![other.to_string()],
        };
        let joined = path.join(".");
        match path.as_slice() {
            _ if self.get_type_id(&joined).is_some() => joined,
            [head, rest @ ..] if !rest.is_empty() => {
                let tail = rest.join(".");
                let prefixed = format!("{head}${tail}");
                if self.get_type_id(&tail).is_some() {
                    tail
                } else if self.get_type_id(&prefixed).is_some() {
                    prefixed
                } else {
                    joined
                }
            }
            _ => joined,
        }
    }

    /// A stdlib signature's types as the stdlib reads them. Stub signatures are
    /// read before any declaration is registered, so their `any I` carries no
    /// declaration yet; it is the stdlib's `I`, even where a program declares
    /// its own (#1426).
    pub fn as_stdlib_reads(&self, ty: &Type) -> Type {
        let each = |args: &[GenericArg]| -> Vec<GenericArg> {
            args.iter()
                .map(|a| match a {
                    GenericArg::Type(t) => GenericArg::Type(Box::new(self.as_stdlib_reads(t))),
                    other => other.clone(),
                })
                .collect()
        };
        match ty {
            Type::InterfaceObject { interface_name, decl: None } => {
                let decl = self
                    .stdlib_type_names
                    .get(interface_name)
                    .copied()
                    .filter(|id| matches!(self.get(*id), Some(TypeDef::Interface { .. })));
                Type::InterfaceObject { interface_name: interface_name.clone(), decl }
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.as_stdlib_reads(ok)),
                err: Box::new(self.as_stdlib_reads(err)),
            },
            // A name the program has taken for a type of its own still means
            // the stdlib's here: `parse` fails with the stdlib's `ParseError`
            // whatever the program calls its enum (#1333).
            Type::UnresolvedNamed(name) if self.shadowed_stdlib_type(name).is_some() => {
                Type::Named(self.shadowed_stdlib_type(name).unwrap())
            }
            Type::Generic { base, args } => Type::Generic { base: *base, args: each(args) },
            Type::UnresolvedGeneric { name, args } => match self.shadowed_stdlib_type(name) {
                Some(base) => Type::Generic { base, args: each(args) },
                None => Type::UnresolvedGeneric { name: name.clone(), args: each(args) },
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.as_stdlib_reads(e)).collect()),
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| self.as_stdlib_reads(p)).collect(),
                ret: Box::new(self.as_stdlib_reads(ret)),
            },
            other => other.clone(),
        }
    }

    /// The name an interface goes by after checking, in the backends' tables
    /// and in vtable symbols: one per declaration.
    ///
    /// The plain name, except for a program interface that shadows one the
    /// stdlib declares. That one gets its `TypeId` attached, so `any Writer` in
    /// the program and `any Writer` in `io.copy`'s signature reach different
    /// method lists and different vtables (#1426).
    pub fn interface_symbol(&self, name: &str, decl: Option<TypeId>) -> String {
        match decl {
            Some(id) if self.shadows_stdlib_interface(name, id) => format!("{name}#{}", id.0),
            _ => name.to_string(),
        }
    }

    /// Is `id` a declaration of `name` other than the stdlib's interface of
    /// that name?
    fn shadows_stdlib_interface(&self, name: &str, id: TypeId) -> bool {
        self.stdlib_type_names
            .get(name)
            .is_some_and(|std| *std != id && matches!(self.get(*std), Some(TypeDef::Interface { .. })))
    }

    /// Every declared interface with its id.
    pub fn interfaces(&self) -> impl Iterator<Item = (TypeId, &str)> {
        self.types.iter().enumerate().filter_map(|(i, def)| match def {
            TypeDef::Interface { name, .. } => Some((TypeId(i as u32), name.as_str())),
            _ => None,
        })
    }

    /// The interface a name means to the code being checked.
    pub fn interface_ident(&self, name: &str) -> InterfaceIdent {
        self.get_type_id(name)
            .filter(|id| matches!(self.get(*id), Some(TypeDef::Interface { .. })))
            .map_or_else(|| InterfaceIdent::Builtin(name.to_string()), InterfaceIdent::Declared)
    }

    /// The stdlib's interface of this name, whatever the program declares.
    ///
    /// Operators resolve against `stdlib/ops.rk` (`type.operator-resolution`)
    /// and auto-derive provides the stdlib's `Equal` and `Debug`; a program's
    /// own `interface Sub` is neither (#1329).
    ///
    /// A check run without the stdlib loaded falls back to the program's.
    pub fn stdlib_interface_ident(&self, name: &str) -> InterfaceIdent {
        self.stdlib_type_names
            .get(name)
            .or_else(|| self.type_names.get(name))
            .copied()
            .filter(|id| matches!(self.get(*id), Some(TypeDef::Interface { .. })))
            .map_or_else(|| InterfaceIdent::Builtin(name.to_string()), InterfaceIdent::Declared)
    }

    /// G1: record that a type conforms to an interface (declared or auto-derived).
    pub fn record_conformance(&mut self, type_id: TypeId, interface: &TypeExpr) {
        let key = self.applied_conformance_key(interface, &self.type_name(type_id));
        self.record_conformance_key(type_id, key);
    }

    /// G1: record an auto-derived conformance — always to the stdlib's interface.
    pub fn record_derived_conformance(&mut self, type_id: TypeId, interface: &str) {
        let iface = self.stdlib_interface_ident(interface);
        let key = self.applied_key_for(iface, &TypeExpr::named(interface), &self.type_name(type_id));
        self.record_conformance_key(type_id, key);
    }

    fn record_conformance_key(&mut self, type_id: TypeId, key: ConformanceKey) {
        let conformers = self.conformers_by_pair.entry(key.clone()).or_default();
        if !conformers.contains(&type_id) {
            conformers.push(type_id);
        }
        self.conformances.entry(type_id).or_default().insert(key);
    }

    /// OR1: every type that conforms to this applied interface, in declaration
    /// order. `Mul<Duration>` answers with the `i64` the stdlib wrote.
    pub fn conformers_of(&self, applied: &ConformanceKey) -> &[TypeId] {
        self.conformers_by_pair.get(applied).map_or(&[], |v| v.as_slice())
    }

    /// AT2/AT8: record `type Out = Meters` for one conformance.
    pub fn record_assoc_binding(
        &mut self,
        type_id: TypeId,
        interface: &TypeExpr,
        assoc: &str,
        ty: Type,
    ) {
        let key = self.applied_conformance_key(interface, &self.type_name(type_id));
        self.assoc_bindings
            .entry((type_id, key))
            .or_default()
            .insert(assoc.to_string(), ty);
    }

    /// AT10: a binding read off a generic type's conformance names the type's
    /// parameters in the declaration's spelling (`type Out = T` on `Cell1<T>`).
    /// On the instance `Cell1<i32>` that's `i32`.
    pub fn instantiate_assoc(&self, base: &Type, binding: &Type) -> Type {
        let Type::Generic { base: id, args } = base else {
            return binding.clone();
        };
        let params = match self.get(*id) {
            Some(TypeDef::Struct { type_params, .. }) | Some(TypeDef::Enum { type_params, .. }) => type_params,
            _ => return binding.clone(),
        };
        let map: HashMap<String, Type> = params
            .iter()
            .zip(args)
            .filter_map(|(p, a)| match a {
                GenericArg::Type(t) => Some((p.clone(), (**t).clone())),
                _ => None,
            })
            .collect();
        crate::interfaces::substitute_type(binding, &map)
    }

    /// AT6/AT8: `base.assoc` projected through the applied interface `bound`:
    /// the binding `base`'s conformance to exactly that interface gives, on
    /// this instance. `None` when the conformance doesn't exist or says nothing.
    pub fn project(&self, base: &Type, bound: &TypeExpr, assoc: &str) -> Option<Type> {
        let id = self.conformance_target(base)?;
        let binding = self.assoc_binding(id, bound, assoc)?;
        Some(self.instantiate_assoc(base, binding))
    }

    /// Which of a parameter's bounds a projection `T.assoc` goes through: the
    /// one bound whose interface declares `assoc`. `None` if no bound does, or
    /// if two do — then the projection has no single meaning.
    pub fn projection_bound<'a>(&self, bounds: &'a [TypeExpr], assoc: &str) -> Option<&'a TypeExpr> {
        let mut through = bounds.iter().filter(|b| {
            matches!(
                self.interface_decl(b).and_then(|id| self.get(id)),
                Some(TypeDef::Interface { assoc_types, .. }) if assoc_types.iter().any(|a| a.name == assoc)
            )
        });
        let first = through.next()?;
        through.next().is_none().then_some(first)
    }

    /// AT6: read an associated type off a conformance. A lookup, never a search.
    pub fn assoc_binding(&self, type_id: TypeId, interface: &TypeExpr, assoc: &str) -> Option<&Type> {
        let iface = self.written_interface_ident(interface);
        self.assoc_binding_to(type_id, iface, interface, assoc)
    }

    /// `assoc_binding` for an interface already identified.
    pub fn assoc_binding_to(
        &self,
        type_id: TypeId,
        iface: InterfaceIdent,
        interface: &TypeExpr,
        assoc: &str,
    ) -> Option<&Type> {
        let key = self.applied_key_for(iface.clone(), interface, &self.type_name(type_id));
        if let Some(t) = self.assoc_bindings.get(&(type_id, key)).and_then(|m| m.get(assoc)) {
            return Some(t);
        }
        // A bare `Mul` asking about a type with exactly one `Mul<...>`
        // conformance still has one answer. Two of them is the caller's
        // problem to disambiguate, and it gets nothing here.
        if !interface.args().is_empty() {
            return None;
        }
        let mut found = None;
        for ((id, key), m) in &self.assoc_bindings {
            if *id != type_id || key.iface != iface {
                continue;
            }
            if let Some(t) = m.get(assoc) {
                if found.is_some() {
                    return None;
                }
                found = Some(t);
            }
        }
        found
    }

    /// MN3: remember where a conformance was declared.
    ///
    /// XC3: returns the earlier block's span when this pair already has one and
    /// both blocks are the program's own — the same-package clash, reported at
    /// the second declaration. A program block landing on a stdlib one is an
    /// override across a package boundary, legal for every non-core interface by
    /// XC2, and it *takes the slot* so a second program block is blamed on the
    /// program's first rather than on the stdlib's.
    ///
    /// Re-registering the same block is never a duplicate: the stdlib's
    /// declarations are collected once as stubs and again as bodies.
    pub fn record_conformance_span(
        &mut self,
        type_id: TypeId,
        interface: &TypeExpr,
        decl: NodeId,
        span: Span,
        package: Option<String>,
    ) -> Option<Span> {
        let key = self.applied_conformance_key(interface, &self.type_name(type_id));
        let from_stdlib = self.stdlib_mode;
        let mine = ConformanceSite { span, decl, from_stdlib, package: package.clone() };
        let sites = self.conformance_spans.entry((type_id, key.clone())).or_default();
        if sites.iter().any(|s| s.decl == decl) {
            return None;
        }
        if !from_stdlib {
            // The same package saying it twice. Reported here, at the second
            // block, because one author owns both. Two *packages* is XC3's
            // other half and belongs at the use site, so it just gets recorded.
            if let Some(first) = sites
                .iter()
                .find(|s| !s.from_stdlib && s.package == package)
            {
                return Some(first.span);
            }
            // A program block landing on the stdlib's is an override across a
            // package boundary, legal for every non-core interface by XC2, and it
            // takes the slot — so a second program block is blamed on the
            // program's first rather than on the stdlib's.
            sites.retain(|s| !s.from_stdlib);
        }
        let already = sites.iter().any(|s| !s.from_stdlib);
        sites.push(mine);
        if already && !from_stdlib {
            self.ambiguous_conformances.insert((type_id, key));
        }
        None
    }

    /// XC4/XC5: remember which package's block a method came from.
    pub(super) fn record_impl_method_package(
        &mut self,
        type_id: TypeId,
        method: &str,
        package: &str,
        decl: NodeId,
    ) {
        let sites = self
            .impl_method_packages
            .entry((type_id, method.to_string()))
            .or_default();
        if !sites.iter().any(|(_, d)| *d == decl) {
            sites.push((package.to_string(), decl));
        }
    }

    /// XC4/XC5: the packages whose blocks declare this method on this type.
    /// One entry is the ordinary case and needs no disambiguation.
    pub(super) fn impl_method_packages(
        &self,
        type_id: TypeId,
        method: &str,
    ) -> &[(String, NodeId)] {
        self.impl_method_packages
            .get(&(type_id, method.to_string()))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// XC3: is any conformance on this type declared more than once?
    pub(super) fn has_ambiguous_conformance(&self, type_id: TypeId) -> bool {
        self.ambiguous_conformances
            .iter()
            .any(|(id, _)| *id == type_id)
    }

    /// XC3: the applied interface keys this type has more than one declaration of.
    pub(super) fn ambiguous_conformance_keys(&self, type_id: TypeId) -> Vec<ConformanceKey> {
        self.ambiguous_conformances
            .iter()
            .filter(|(id, _)| *id == type_id)
            .map(|(_, key)| key.clone())
            .collect()
    }

    /// XC3/XC4: every written declaration of this conformance, in the order the
    /// checker read them.
    pub(super) fn conformance_sites(
        &self,
        type_id: TypeId,
        key: &ConformanceKey,
    ) -> &[ConformanceSite] {
        self.conformance_spans
            .get(&(type_id, key.clone()))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// MN3: where a conformance was declared, if it was written in source.
    /// XC1: remember where a type was declared, and who declared it.
    pub(super) fn record_declared_at(&mut self, type_id: TypeId, span: Span, owner: TypeOwner) {
        self.declared_at.entry(type_id).or_insert(span);
        self.declared_by.entry(type_id).or_insert(owner);
    }

    /// Remember the bounds a type's declaration puts on its parameters.
    pub(super) fn record_param_bounds(&mut self, type_id: TypeId, params: &[rask_ast::decl::TypeParam]) {
        if params.iter().any(|p| !p.bounds.is_empty()) {
            let bounds = params.iter().map(|p| (p.name.clone(), p.bounds.clone())).collect();
            self.declared_param_bounds.insert(type_id, bounds);
        }
    }

    /// The bounds a type's declaration puts on its parameters, in order.
    pub(super) fn param_bounds(&self, type_id: TypeId) -> &[(String, Vec<rask_ast::ty::TypeExpr>)] {
        self.declared_param_bounds.get(&type_id).map(Vec::as_slice).unwrap_or(&[])
    }

    /// XC1: who declares this type. A type with no recorded declaration is a
    /// builtin, and builtins are the stdlib's.
    pub(super) fn declared_by(&self, type_id: TypeId) -> TypeOwner {
        self.declared_by
            .get(&type_id)
            .cloned()
            .unwrap_or(TypeOwner::Stdlib)
    }

    /// The span of a type's declaration, if one was recorded.
    pub fn declared_at(&self, type_id: TypeId) -> Option<Span> {
        self.declared_at.get(&type_id).copied()
    }

    pub fn conformance_span(&self, type_id: TypeId, interface: &TypeExpr) -> Option<Span> {
        let key = self.applied_conformance_key(interface, &self.type_name(type_id));
        self.conformance_spans
            .get(&(type_id, key))
            .and_then(|v| v.first())
            .map(|s| s.span)
    }

    /// AT6: the associated type `assoc` on this type, when exactly one of its
    /// conformances declares one by that name.
    ///
    /// Used only where no bound named the projection. A call to a generic
    /// function or method reads `T.Out` through `T`'s bound instead
    /// (`TypeTable::project`), which is what decides between two conformances
    /// of one interface (#1330). What's left here is a projection on a type's
    /// own parameter, reached through the receiver. Two conformances answering to
    /// one name have no single answer, and this gives none rather than picking:
    /// disambiguating is the caller's, and `type.operator-resolution/OR1` is
    /// what does it for the operator interfaces.
    pub fn assoc_binding_any(&self, type_id: TypeId, assoc: &str) -> Option<&Type> {
        let mut found = None;
        for ((id, _), m) in &self.assoc_bindings {
            if *id != type_id {
                continue;
            }
            if let Some(t) = m.get(assoc) {
                if found.is_some_and(|f| f != t) {
                    return None;
                }
                found = Some(t);
            }
        }
        found
    }

    /// GT3: every applied form of `iface` this type conforms to.
    pub fn applied_conformances(&self, type_id: TypeId, iface: &InterfaceIdent) -> Vec<TypeExpr> {
        self.conformances
            .get(&type_id)
            .map(|set| {
                let mut v: Vec<TypeExpr> = set
                    .iter()
                    .filter(|k| k.iface == *iface)
                    .map(|k| k.applied.clone())
                    .collect();
                v.sort_by_key(|k| k.to_string());
                v
            })
            .unwrap_or_default()
    }

    /// G1: does the type declare (or auto-derive) conformance to the interface?
    ///
    /// TD3: a sub-interface requires everything its super-interfaces require, so
    /// declaring the sub-interface declares the parents too. Without that,
    /// `Horn implements Shouty` — where `interface Shouty: Speak` — left
    /// `horn as any Speak` refused for an interface the type demonstrably implements,
    /// and pushing one into a `Vec<any Speak>` was a type error (#873).
    pub fn declares_conformance(&self, type_id: TypeId, interface: &TypeExpr) -> bool {
        let iface = self.written_interface_ident(interface);
        self.declares_conformance_to(type_id, iface, interface)
    }

    /// Does the type conform to a *different* interface spelled `name`? A stdlib
    /// `Buffer` implements the stdlib's `Writer`, and a program declaring its own
    /// `Writer` hasn't changed that (#1329).
    pub fn conforms_to_namesake(&self, type_id: TypeId, name: &str) -> bool {
        let meant = self.interface_ident(name);
        self.conformances.get(&type_id).is_some_and(|set| {
            set.iter()
                .any(|k| k.iface != meant && Self::conformance_key(&k.applied) == name)
        })
    }

    /// `declares_conformance` to a declared interface named by its id.
    pub fn declares_conformance_to_decl(&self, type_id: TypeId, interface: TypeId, name: &str) -> bool {
        self.declares_conformance_to(type_id, InterfaceIdent::Declared(interface), &TypeExpr::named(name))
    }

    /// `declares_conformance` for an interface already identified.
    pub fn declares_conformance_to(
        &self,
        type_id: TypeId,
        iface: InterfaceIdent,
        interface: &TypeExpr,
    ) -> bool {
        let Some(set) = self.conformances.get(&type_id) else {
            return false;
        };
        // GT3: `Mul` asks whether any applied form is declared; `Mul<f64>` asks
        // for that one. The canonical key fills in defaults and `Self`, so a
        // bare header and its written-out equivalent agree.
        if !interface.args().is_empty() {
            let key = self.applied_key_for(iface.clone(), interface, &self.type_name(type_id));
            if set.contains(&key) {
                return true;
            }
        } else if set.iter().any(|k| k.iface == iface) {
            return true;
        }
        let InterfaceIdent::Declared(target) = iface else {
            return false;
        };
        set.iter().any(|declared| match declared.iface {
            InterfaceIdent::Declared(id) => self.interface_extends(id, target, &mut Vec::new()),
            InterfaceIdent::Builtin(_) => false,
        })
    }

    /// Is `target` somewhere in `interface`'s super-interface closure? Parents
    /// are named as the interface's own side reads them (#1329). `seen` keeps
    /// a cycle in the graph from recursing forever.
    fn interface_extends(&self, interface: TypeId, target: TypeId, seen: &mut Vec<TypeId>) -> bool {
        if seen.contains(&interface) {
            return false;
        }
        seen.push(interface);
        let Some(TypeDef::Interface { super_interfaces, .. }) = self.get(interface) else {
            return false;
        };
        super_interfaces.iter().any(|p| {
            self.parent_interface(interface, p)
                .is_some_and(|pid| pid == target || self.interface_extends(pid, target, seen))
        })
    }

    /// CC1/CC2: record the `where` condition for a conditional conformance.
    pub fn record_conformance_condition(
        &mut self,
        type_id: TypeId,
        interface: &TypeExpr,
        bounds: Vec<(String, Vec<TypeExpr>)>,
    ) {
        let iface = self.written_interface_ident(interface);
        self.conformance_conditions.insert((type_id, iface), bounds);
    }

    /// CC1: the `where` condition for a conformance, if it's conditional.
    pub fn conformance_condition(
        &self,
        type_id: TypeId,
        interface: &TypeExpr,
    ) -> Option<&Vec<(String, Vec<TypeExpr>)>> {
        self.conformance_conditions.get(&(type_id, self.written_interface_ident(interface)))
    }

    /// Check if a name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.builtins.contains_key(name)
            || self.type_names.contains_key(name)
            || self.aliases().contains_key(name)
    }

    /// Get TypeId for a name (user-defined types only).
    /// Resolves through aliases.
    pub fn get_type_id(&self, name: &str) -> Option<TypeId> {
        if let Some(id) = self.resolve_name(name) {
            return Some(id);
        }
        if let Some(target) = self.alias_target_name(name) {
            return self.resolve_name(&target);
        }
        None
    }

    /// Check if a type name refers to a `@resource` struct.
    pub fn is_resource_type(&self, name: &str) -> bool {
        if let Some(&id) = self.type_names.get(name) {
            return self.is_resource_type_by_id(id);
        }
        false
    }

    /// Check if a TypeId refers to a `@resource` struct.
    pub fn is_resource_type_by_id(&self, id: TypeId) -> bool {
        if let Some(TypeDef::Struct { is_resource, .. }) = self.types.get(id.0 as usize) {
            return *is_resource;
        }
        false
    }

    /// ER42/L1: TypeId is transitively linear (carries a `@resource` directly
    /// or through any nested field/variant). Computed by
    /// `propagate_resource_linearity` and queried during ownership checking.
    pub fn is_transitive_resource_by_id(&self, id: TypeId) -> bool {
        match self.types.get(id.0 as usize) {
            Some(TypeDef::Struct { is_transitive_resource, .. }) => *is_transitive_resource,
            Some(TypeDef::Enum { is_transitive_resource, .. }) => *is_transitive_resource,
            _ => false,
        }
    }

    /// ER42/L1: A `Type` value is transitively linear. Walks through tuples,
    /// arrays, slices, Result, and Generic args so containers of linear values
    /// inherit the obligation. Interface objects and unresolved/error types are
    /// conservatively non-linear.
    pub fn type_is_transitive_resource(&self, ty: &Type) -> bool {
        match ty {
            Type::Named(id) => self.is_transitive_resource_by_id(*id),
            Type::Generic { base, args } => {
                if self.is_transitive_resource_by_id(*base) {
                    return true;
                }
                args.iter().any(|a| match a {
                    crate::types::GenericArg::Type(t) => self.type_is_transitive_resource(t),
                    _ => false,
                })
            }
            Type::Tuple(elems) => elems.iter().any(|t| self.type_is_transitive_resource(t)),
            Type::Array { elem, .. } => {
                self.type_is_transitive_resource(elem)
            }
            Type::Result { ok, err } => {
                self.type_is_transitive_resource(ok) || self.type_is_transitive_resource(err)
            }
            Type::Union(variants) => variants.iter().any(|v| self.type_is_transitive_resource(v)),
            Type::UnresolvedNamed(name) => {
                let base = name;
                self.type_names
                    .get(base)
                    .map_or(false, |id| self.is_transitive_resource_by_id(*id))
            }
            Type::UnresolvedGeneric { name, args } => {
                let base_name = name;
                if let Some(&id) = self.type_names.get(base_name) {
                    if self.is_transitive_resource_by_id(id) {
                        return true;
                    }
                }
                args.iter().any(|a| match a {
                    crate::types::GenericArg::Type(t) => self.type_is_transitive_resource(t),
                    _ => false,
                })
            }
            _ => false,
        }
    }

    /// Is a value of this type *itself* a linear obligation — something the
    /// language requires be consumed exactly once?
    ///
    /// A `@resource` struct or enum is, directly or transitively. So is a
    /// `Heap<T>`, whatever the payload: `Heap(…)` allocates a block and exactly
    /// one consume gives it back (mem.heap/HP1, HP2).
    ///
    /// An aggregate holding a `Heap<T>` is not. Storing a block in a field, a
    /// tuple, an array or an enum payload consumes it (HP4/L5) — the aggregate
    /// owns it from then on and its release gives the block back, which is what
    /// makes `Cons(i64, Heap<List>)` work and `drop(h.inner)` an error. An
    /// aggregate holding a `@resource` still owes one: there are no
    /// destructors, so nothing but an explicit consume ever closes one, and a
    /// tuple has no name to charge but its own. Ask
    /// [`Self::holds_linear_value`] for the other question, "is there anything
    /// linear anywhere in here", which is what rules a `Vec<Heap<i64>>` out.
    ///
    /// A wrapper is not an aggregate: `T?` and `T or E` carry their payload
    /// linearly (RC4 — an optional resource must be matched and consumed),
    /// because nothing walks a wrapper and consumes what is behind its tag.
    ///
    /// The two used to be one predicate, and every caller got whichever answer
    /// the other one needed. It went unnoticed while `Heap<i64>` wasn't linear
    /// at all; once it was, a tuple of them became an obligation nothing could
    /// discharge (#1256).
    pub fn is_linear_value(&self, ty: &Type) -> bool {
        if ty.heap_payload().is_some() {
            return true;
        }
        match ty {
            Type::Named(id) => self.is_transitive_resource_by_id(*id),
            Type::Generic { base, args } => {
                let full = self.type_name(*base);
                let name = full.as_str();
                !Self::is_nonlinear_wrapper(name)
                    && (self.is_transitive_resource_by_id(*base)
                        || self.instance_slot_owes(*base, args))
            }
            Type::UnresolvedGeneric { name, .. } => {
                let base = name;
                !Self::is_nonlinear_wrapper(base)
                    && self
                        .type_names
                        .get(base)
                        .is_some_and(|&id| self.is_transitive_resource_by_id(id))
            }
            Type::UnresolvedNamed(name) => {
                let base = name;
                self.type_names
                    .get(base)
                    .map_or(false, |id| self.is_transitive_resource_by_id(*id))
            }
            // A union's value *is* one of its members, and a wrapper carries its
            // payload behind a tag with nothing to walk it — so both are linear
            // when what they hold is. Only the aggregates that own and release
            // their slots ask `slot_owes` instead.
            Type::Union(members) => members.iter().any(|t| self.is_linear_value(t)),
            Type::Result { ok, err } => self.is_linear_value(ok) || self.is_linear_value(err),
            Type::Tuple(elems) => elems.iter().any(|t| self.slot_owes(t)),
            Type::Array { elem, .. } => self.slot_owes(elem),
            _ => false,
        }
    }

    /// Does a generic struct or enum, at these type arguments, hold a field or
    /// payload that owes a consume? `Holder<Conn>` does when `Holder<T>` has
    /// an `item: T`. The declaration alone can't say — `T` is linear only at
    /// some instantiations — so `is_transitive_resource` was false and a
    /// `Holder<Conn>` was dropped with no error (#1366).
    fn instance_slot_owes(&self, base: TypeId, args: &[GenericArg]) -> bool {
        let (params, slots): (&Vec<String>, Vec<&Type>) = match self.get(base) {
            Some(TypeDef::Struct { type_params, fields, .. }) => {
                (type_params, fields.iter().map(|(_, t)| t).collect())
            }
            Some(TypeDef::Enum { type_params, variants, .. }) => {
                (type_params, variants.iter().flat_map(|(_, ts)| ts.iter()).collect())
            }
            _ => return false,
        };
        let subst: HashMap<String, Type> = params
            .iter()
            .zip(args)
            .filter_map(|(p, a)| match a {
                GenericArg::Type(t) => Some((p.clone(), (**t).clone())),
                _ => None,
            })
            .collect();
        if !subst.values().any(|t| self.holds_linear_value(t)) {
            return false;
        }
        slots
            .into_iter()
            .any(|t| self.slot_owes(&crate::interfaces::substitute_type(t, &subst)))
    }

    /// A slot inside an aggregate: does it leave the aggregate owing a consume?
    ///
    /// A `Heap<T>` doesn't — the aggregate's release gives the block back. A
    /// `@resource` does, and the aggregate is the only name left to charge.
    fn slot_owes(&self, ty: &Type) -> bool {
        ty.heap_payload().is_none() && self.is_linear_value(ty)
    }

    /// Does this type hold a linear value anywhere inside it?
    ///
    /// RC1/RC3 asks this about a `Vec`'s element and a `Map`'s key and value: a
    /// container can't consume what it drops, so a linear element is rejected at
    /// the type — `Vec<(Heap<i64>, i64)>` as much as `Vec<Heap<i64>>`.
    ///
    /// Deliberately narrower than [`Self::type_is_transitive_resource`], which
    /// recurses into *every* generic argument and so treats `Handle<File>` as
    /// linear. For the container-element rule that is a false positive — the
    /// spec's own `Vec<Handle<Connection>>` example is legal.
    pub fn holds_linear_value(&self, ty: &Type) -> bool {
        if self.is_linear_value(ty) {
            return true;
        }
        match ty {
            Type::Generic { base, args } => {
                let full = self.type_name(*base);
                let name = full.as_str();
                !Self::is_nonlinear_wrapper(name)
                    && args
                        .iter()
                        .any(|a| matches!(a, GenericArg::Type(t) if self.holds_linear_value(t)))
            }
            Type::UnresolvedGeneric { name, args } => {
                let base = name;
                !Self::is_nonlinear_wrapper(base)
                    && args
                        .iter()
                        .any(|a| matches!(a, GenericArg::Type(t) if self.holds_linear_value(t)))
            }
            Type::Tuple(elems) | Type::Union(elems) => {
                elems.iter().any(|t| self.holds_linear_value(t))
            }
            Type::Array { elem, .. } => self.holds_linear_value(elem),
            Type::Result { ok, err } => {
                self.holds_linear_value(ok) || self.holds_linear_value(err)
            }
            _ => false,
        }
    }

    /// Why a value of this type has to stay on the task that made it, if it
    /// does: it's a `Local` box, which takes no lock (conc.sync/SH7), or it
    /// carries a link (`mem.ownership/T2`). A `Readers` or `Mutex` box may
    /// cross whatever it holds.
    ///
    /// The one test for it. The checker asks at a `spawn` and for every
    /// closure literal; monomorphization asks again for a closure in a generic
    /// body, whose capture types are only concrete per instantiation.
    pub fn task_bound(&self, ty: &Type) -> Option<TaskBound> {
        if let Some(args) = self.shared_args(ty) {
            return (self.shared_strategy_name(args) == "Local").then_some(TaskBound::LocalBox);
        }
        self.holds_link(ty).then_some(TaskBound::Link)
    }

    /// Is a closure in a generic body task-bound in one instantiation? Its
    /// captures are the `generic_closure_captures` entry; `concrete` fills in
    /// that instantiation's type arguments, `None` where it can't.
    ///
    /// Decided from types, never from the captured values: an empty
    /// `Vec<Link<Node>>` holds no link yet, and still may not cross (#1382).
    /// Monomorphization and the interpreter both ask here.
    pub fn generic_closure_task_bound(
        &self,
        captures: &[(String, Type)],
        concrete: impl Fn(&Type) -> Option<Type>,
    ) -> bool {
        captures
            .iter()
            .any(|(_, ty)| concrete(ty).is_some_and(|ty| self.task_bound(&ty).is_some()))
    }

    /// The type arguments of a `Shared<T, S>`, or `None` for any other type.
    pub fn shared_args<'t>(&self, ty: &'t Type) -> Option<&'t [GenericArg]> {
        match ty {
            Type::Generic { base, args } if self.type_name(*base) == "Shared" => Some(args),
            Type::UnresolvedGeneric { name, args } if name == "Shared" => Some(args),
            _ => None,
        }
    }

    /// A `Shared` box's strategy, from its type arguments. SH3: no strategy
    /// argument means `Readers`, and so does one that can't be read, which is
    /// the safe side of every rule that asks.
    pub fn shared_strategy_name(&self, args: &[GenericArg]) -> String {
        match args.get(1) {
            Some(GenericArg::Type(s)) => match s.as_ref() {
                Type::Named(id) => self.type_name(*id),
                Type::UnresolvedNamed(n) => match self.get_type_id(n) {
                    Some(id) => self.type_name(id),
                    None => n.clone(),
                },
                _ => "Readers".to_string(),
            },
            _ => "Readers".to_string(),
        }
    }

    /// Does a value of this type carry a `Link` — itself, in an option or a
    /// container, or in a field of a struct or enum it holds?
    ///
    /// A link is a node's address, so a value carrying one can't go to another
    /// task (`mem.ownership/T2`). A `Rack` is the exception and isn't walked:
    /// moving the whole rack takes its nodes' links with the nodes they point
    /// at, which is how `snapshot()` hands a graph over.
    pub fn holds_link(&self, ty: &Type) -> bool {
        self.holds_link_inner(ty, &mut Vec::new())
    }

    fn holds_link_inner(&self, ty: &Type, seen: &mut Vec<TypeId>) -> bool {
        let generic = |base: &str, args: &[GenericArg], seen: &mut Vec<TypeId>| match base {
            "Link" => true,
            "Rack" => false,
            _ => args
                .iter()
                .any(|a| matches!(a, GenericArg::Type(t) if self.holds_link_inner(t, seen))),
        };
        match ty {
            Type::Named(id) => self.def_holds_link(*id, seen),
            Type::UnresolvedNamed(name) => {
                let base = name;
                self.type_names.get(base).is_some_and(|&id| self.def_holds_link(id, seen))
            }
            Type::Generic { base, args } => {
                let full = self.type_name(*base);
                let name = full.as_str();
                generic(name, args, seen) || self.def_holds_link(*base, seen)
            }
            Type::UnresolvedGeneric { name, args } => {
                let base = name;
                generic(base, args, seen)
                    || self.type_names.get(base).is_some_and(|&id| self.def_holds_link(id, seen))
            }
            Type::Tuple(elems) | Type::Union(elems) => {
                elems.iter().any(|t| self.holds_link_inner(t, seen))
            }
            Type::Array { elem, .. } => self.holds_link_inner(elem, seen),
            Type::Result { ok, err } => {
                self.holds_link_inner(ok, seen) || self.holds_link_inner(err, seen)
            }
            _ => false,
        }
    }

    fn def_holds_link(&self, id: TypeId, seen: &mut Vec<TypeId>) -> bool {
        if seen.contains(&id) {
            return false;
        }
        seen.push(id);
        match self.types.get(id.0 as usize) {
            Some(TypeDef::Struct { fields, .. }) => {
                fields.iter().any(|(_, t)| self.holds_link_inner(t, seen))
            }
            Some(TypeDef::Enum { variants, .. }) => variants
                .iter()
                .any(|(_, payload)| payload.iter().any(|t| self.holds_link_inner(t, seen))),
            _ => false,
        }
    }

    /// Container wrappers that hold values without becoming linear themselves.
    /// A `Link` is a copyable reference; `Vec`/`Map`/`Rack` are handled by the
    /// outer walk.
    ///
    /// The channel ends are here because `conc.async/CH1` says so outright:
    /// they can go out of scope without an explicit close. Without them,
    /// `Channel<Conn>.buffered(1)` made the two ends resources of their own —
    /// so a channel carrying a resource, which is the shape `mem.linear/L5`
    /// blesses as a consumption (`ch.send(file)`), demanded a `close()` that
    /// neither end has (#882).
    fn is_nonlinear_wrapper(name: &str) -> bool {
        matches!(
            name,
            "Vec"
                | "Map"
                | "Link"
                | "Rack"
                | "Channel"
                | "Sender"
                | "Receiver"
        )
    }

    /// RC1-RC3: find the first `Vec<T>`, `Map<K, V>` or `Rack<T>` anywhere in
    /// `ty` whose element, key or node is a linear value. None of the three can
    /// consume what it holds, so a linear one is rejected at the type. Returns the
    /// container spelling ("Vec"/"Map") and the offending element type.
    ///
    /// Walks the whole type tree so nested forms (`Vec<Vec<File>>`,
    /// `Map<string, File>` inside a tuple, a `Vec<File>` return of a `func`
    /// type) are caught at their innermost violation.
    /// HA1/HA4: a Map key has to be Hashable. `Some((ty, fix))` names the key
    /// that isn't, plus which way out to offer.
    ///
    /// This used to test `F32 | F64` by name, so the rule held for the one type
    /// that motivated it and nothing else — a struct with a float field got in,
    /// and so did a nominal newtype that declared no conformance (#812). The
    /// diagnostic already said "is not Hashable"; now the check is that.
    ///
    /// A key whose type isn't settled yet is left alone: an inference variable or
    /// an unresolved name (an open type parameter) says nothing either way, and
    /// reporting one would flag every generic container.
    fn unhashable_key(&self, key: &Type) -> Option<(Type, MapKeyFix)> {
        match key {
            Type::Var(_)
            | Type::Error
            | Type::UnresolvedNamed(_)
            | Type::UnresolvedGeneric { .. } => None,
            settled if crate::interfaces::implements_interface(self, settled, "Hashable") => None,
            settled => Some((settled.clone(), self.map_key_fix(settled))),
        }
    }

    /// Which advice fits this key type.
    fn map_key_fix(&self, key: &Type) -> MapKeyFix {
        if matches!(key, Type::F32 | Type::F64) {
            return MapKeyFix::Float;
        }
        let id = match key {
            Type::Named(id) => Some(*id),
            Type::Generic { base, .. } => Some(*base),
            _ => None,
        };
        match id.and_then(|id| self.get(id)) {
            Some(TypeDef::NominalAlias { .. }) => MapKeyFix::NominalClause,
            _ => MapKeyFix::ExtendBlock,
        }
    }

    pub fn find_unhashable_map_key(&self, ty: &Type) -> Option<(Type, MapKeyFix)> {
        let args = match ty {
            Type::Generic { base, args } => {
                let full = self.type_name(*base);
                if full == "Map" {
                    if let Some(GenericArg::Type(k)) = args.first() {
                        if let Some(bad) = self.unhashable_key(k) {
                            return Some(bad);
                        }
                    }
                }
                Some(args)
            }
            Type::UnresolvedGeneric { name, args } => {
                if name == "Map" {
                    if let Some(GenericArg::Type(k)) = args.first() {
                        if let Some(bad) = self.unhashable_key(k) {
                            return Some(bad);
                        }
                    }
                }
                Some(args)
            }
            _ => None,
        };
        // Nested: a Vec of Maps, a Map whose value is a Map, a tuple of them.
        let mut nested: Vec<&Type> = Vec::new();
        if let Some(args) = args {
            for a in args {
                if let GenericArg::Type(t) = a {
                    nested.push(t);
                }
            }
        }
        match ty {
            Type::Tuple(elems) | Type::Union(elems) => nested.extend(elems.iter()),
            Type::RawPtr(inner) => nested.push(inner),
            Type::Array { elem, .. } => nested.push(elem),
            Type::Result { ok, err } => {
                nested.push(ok);
                nested.push(err);
            }
            _ => {}
        }
        nested.into_iter().find_map(|t| self.find_unhashable_map_key(t))
    }

    pub fn find_linear_container(&self, ty: &Type) -> Option<(String, Type)> {
        // Check this node if it's a Vec/Map, then always recurse into children so
        // nested violations surface at their innermost container.
        match ty {
            Type::Generic { base, args } => {
                // `type_name` includes generic params ("Vec<T>"); strip them.
                let full = self.type_name(*base);
                let name = full.as_str();
                if let Some(hit) = self.container_violation(name, args) {
                    return Some(hit);
                }
                self.first_container_in_args(args)
            }
            Type::UnresolvedGeneric { name, args } => {
                let base = name;
                if let Some(hit) = self.container_violation(base, args) {
                    return Some(hit);
                }
                self.first_container_in_args(args)
            }
            Type::Tuple(elems) | Type::Union(elems) => {
                elems.iter().find_map(|t| self.find_linear_container(t))
            }
            Type::Array { elem, .. } | Type::RawPtr(elem) => {
                self.find_linear_container(elem)
            }
            Type::Result { ok, err } => {
                self.find_linear_container(ok).or_else(|| self.find_linear_container(err))
            }
            Type::Fn { params, ret } => params
                .iter()
                .find_map(|p| self.find_linear_container(p))
                .or_else(|| self.find_linear_container(ret)),
            _ => None,
        }
    }

    /// mem.racks/RK14: the node type of a `Rack<T>` inside `ty` whose `T` is
    /// settled and isn't a struct.
    ///
    /// A type parameter or an open variable passes: the rule is checked where
    /// the node type is concrete.
    pub fn find_rack_of_non_struct(&self, ty: &Type) -> Option<Type> {
        let in_args = |args: &[GenericArg]| {
            args.iter().find_map(|a| match a {
                GenericArg::Type(t) => self.find_rack_of_non_struct(t),
                _ => None,
            })
        };
        match ty {
            Type::Generic { base, args } => {
                if Some(*base) == self.stdlib_type_names.get("Rack").copied() {
                    if let Some(GenericArg::Type(node)) = args.first() {
                        if !self.can_be_rack_node(node) {
                            return Some((**node).clone());
                        }
                    }
                }
                in_args(args)
            }
            Type::UnresolvedGeneric { args, .. } => in_args(args),
            Type::Tuple(elems) | Type::Union(elems) => {
                elems.iter().find_map(|t| self.find_rack_of_non_struct(t))
            }
            Type::Array { elem, .. } | Type::RawPtr(elem) => self.find_rack_of_non_struct(elem),
            Type::Result { ok, err } => {
                self.find_rack_of_non_struct(ok).or_else(|| self.find_rack_of_non_struct(err))
            }
            Type::Fn { params, ret } => params
                .iter()
                .find_map(|p| self.find_rack_of_non_struct(p))
                .or_else(|| self.find_rack_of_non_struct(ret)),
            _ => None,
        }
    }

    /// A struct, or a type not settled enough to say.
    fn can_be_rack_node(&self, node: &Type) -> bool {
        let declared = |id: &TypeId| match self.get(*id) {
            Some(TypeDef::Struct { .. }) => true,
            Some(_) => false,
            None => true,
        };
        match node {
            Type::Named(id) | Type::Generic { base: id, .. } => declared(id),
            Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. } => {
                self.get_type_id(name).is_none_or(|id| declared(&id))
            }
            Type::Var(_) | Type::Error | Type::Never => true,
            _ => false,
        }
    }

    fn first_container_in_args(&self, args: &[GenericArg]) -> Option<(String, Type)> {
        args.iter().find_map(|a| match a {
            GenericArg::Type(t) => self.find_linear_container(t),
            _ => None,
        })
    }

    /// If `name`/`args` describe a `Vec<T>`, `Map<K, V>` or `Rack<T>` with a
    /// linear element, key or node, return the violation. The check is
    /// head-only; the caller recurses.
    fn container_violation(&self, name: &str, args: &[GenericArg]) -> Option<(String, Type)> {
        let elem = |i: usize| match args.get(i) {
            Some(GenericArg::Type(t)) => Some(t.as_ref()),
            _ => None,
        };
        match name {
            "Vec" => {
                let e = elem(0)?;
                if self.holds_linear_value(e) {
                    return Some(("Vec".to_string(), e.clone()));
                }
                None
            }
            "Map" => {
                // A resource key is as unconsumable on drop as a resource value.
                if let Some(k) = elem(0) {
                    if self.holds_linear_value(k) {
                        return Some(("Map".to_string(), k.clone()));
                    }
                }
                if let Some(v) = elem(1) {
                    if self.holds_linear_value(v) {
                        return Some(("Map".to_string(), v.clone()));
                    }
                }
                None
            }
            // Same reason as `Vec`: `delete` frees the node, it doesn't hand it
            // back, so there is no way to consume one. `Pool.remove` answered
            // `T?`, which is what made a pool the one container a linear value
            // could live in — and that went with the pool (rask-lang/rask#908).
            "Rack" => {
                let e = elem(0)?;
                if self.holds_linear_value(e) {
                    return Some(("Rack".to_string(), e.clone()));
                }
                None
            }
            _ => None,
        }
    }

    /// ER31a: variants of the error enum `target` that carry `source` as their
    /// only payload — the shape `try` can wrap into on the way out.
    ///
    /// Returns every match so the caller can tell "no wrap" from "more than one
    /// variant would fit, say which". Both sides must be nominal: a union or
    /// generic payload isn't a boundary-enum wrapper.
    pub fn error_wrap_variants(&self, source: &Type, target: &Type) -> Vec<String> {
        let variants = match target {
            Type::Named(id) => match self.get(*id) {
                Some(TypeDef::Enum { variants, .. }) => variants,
                _ => return Vec::new(),
            },
            _ => return Vec::new(),
        };
        let want = match self.nominal_key(source) {
            Some(k) => k,
            None => return Vec::new(),
        };
        // Wrapping a type in itself isn't a widening — plain unification covers it.
        if self.nominal_key(target).as_deref() == Some(want.as_str()) {
            return Vec::new();
        }
        variants
            .iter()
            .filter(|(_, payload)| {
                payload.len() == 1 && self.nominal_key(&payload[0]).as_deref() == Some(want.as_str())
            })
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The bare type name behind a nominal type, however the declaration order
    /// left it spelled. `None` for anything structural.
    fn nominal_key(&self, ty: &Type) -> Option<String> {
        match ty {
            Type::Named(id) => Some(self.type_name(*id)),
            Type::UnresolvedNamed(name) => {
                Some(name.trim().to_string())
            }
            _ => None,
        }
    }

    /// Check if a TypeId refers to a `@unique` struct.
    pub fn is_unique_type_by_id(&self, id: TypeId) -> bool {
        if let Some(TypeDef::Struct { is_unique, .. }) = self.types.get(id.0 as usize) {
            return *is_unique;
        }
        false
    }

    /// Check if a TypeId refers to a `@binary` struct.
    pub fn is_binary_type_by_id(&self, id: TypeId) -> bool {
        if let Some(TypeDef::Struct { is_binary, .. }) = self.types.get(id.0 as usize) {
            return *is_binary;
        }
        false
    }

    /// Store binary struct metadata.
    pub fn register_binary_info(&mut self, id: TypeId, info: BinaryStructInfo) {
        self.binary_structs.insert(id, info);
    }

    /// Get binary struct metadata.
    pub fn get_binary_info(&self, id: TypeId) -> Option<&BinaryStructInfo> {
        self.binary_structs.get(&id)
    }

    /// Get TypeId for the builtin Option<T> enum.
    pub fn get_option_type_id(&self) -> Option<TypeId> {
        self.option_type_id
    }

    /// Get TypeId for the builtin Result<T, E> enum.
    pub fn get_result_type_id(&self) -> Option<TypeId> {
        self.result_type_id
    }

    /// Iterate over all type definitions.
    pub fn iter(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.iter()
    }

    /// Get the display name for a TypeId.
    ///
    /// Generic type defs store their declaration signature as the name
    /// (`Handle<T>`, `Pool<T>`, `Foo<K, V>`). The display/base name is just the
    /// head — strip the parameter list so `Type::Generic { base, args }` renders
    /// as `Handle<Player>`, not `Handle<T><Player>`.
    /// Every registered type's `TypeId` paired with its declared name.
    ///
    /// MIR, codegen and the comptime evaluator each need this map and each had
    /// its own copy of the match that builds it; a new `TypeDef` variant made
    /// that four edits for one fact.
    pub fn type_name_map(&self) -> HashMap<TypeId, String> {
        self.types
            .iter()
            .enumerate()
            .map(|(i, def)| (TypeId(i as u32), Self::def_name(def).to_string()))
            .collect()
    }

    /// The name a `TypeDef` declares, whatever kind it is.
    pub fn def_name(def: &TypeDef) -> &str {
        match def {
            TypeDef::Struct { name, .. }
            | TypeDef::Enum { name, .. }
            | TypeDef::Interface { name, .. }
            | TypeDef::Union { name, .. }
            | TypeDef::NominalAlias { name, .. }
            | TypeDef::Primitive { name, .. } => name,
        }
    }

    /// OR12: note that a conformance method has no body — the compiler answers
    /// this pair itself.
    pub fn record_builtin_method(&mut self, type_id: TypeId, filed: &str) {
        self.builtin_methods.insert((type_id, filed.to_string()));
    }

    /// OR12: is this conformance method the compiler's rather than a body?
    pub fn is_builtin_method(&self, type_id: TypeId, filed: &str) -> bool {
        self.builtin_methods.contains(&(type_id, filed.to_string()))
    }

    /// OR6: the registered stand-in for a primitive, by its source spelling.
    pub fn primitive_id(&self, name: &str) -> Option<TypeId> {
        self.primitive_ids.get(name).copied()
    }

    /// The `TypeId` a conformance written against this type is filed under.
    ///
    /// A struct or enum answers with its own; a primitive with its stand-in.
    /// Anything else — a tuple, an array, a closure — has no conformance
    /// surface to write on.
    pub fn conformance_target(&self, ty: &Type) -> Option<TypeId> {
        match ty {
            Type::Named(id) | Type::Generic { base: id, .. } => Some(*id),
            Type::UnresolvedNamed(name) => self
                .get_type_id(name)
                .or_else(|| self.primitive_id(name)),
            Type::UnresolvedGeneric { name, .. } => self.get_type_id(name),
            _ => primitive_spelling(ty).and_then(|n| self.primitive_id(n)),
        }
    }

    pub fn type_name(&self, id: TypeId) -> String {
        let name = match self.get(id) {
            Some(TypeDef::Struct { name, .. }) => name,
            Some(TypeDef::Enum { name, .. }) => name,
            Some(TypeDef::Interface { name, .. }) => name,
            Some(TypeDef::Union { name, .. }) => name,
            Some(TypeDef::NominalAlias { name, .. }) => name,
            Some(TypeDef::Primitive { name, .. }) => name,
            None => return format!("<type#{}>", id.0),
        };
        name.to_string()
    }

    /// Get the underlying type for a nominal alias.
    pub fn get_nominal_underlying(&self, id: TypeId) -> Option<&Type> {
        match self.get(id) {
            Some(TypeDef::NominalAlias { underlying, .. }) => Some(underlying),
            _ => None,
        }
    }

    /// Get the name of a nominal alias, if this type ID is one.
    pub fn get_nominal_name(&self, id: TypeId) -> Option<String> {
        match self.get(id) {
            Some(TypeDef::NominalAlias { name, .. }) => Some(name.clone()),
            _ => None,
        }
    }

    pub fn resolve_type_names(&self, ty: &Type) -> Type {
        self.named(ty, false)
    }

    /// `resolve_type_names` with each type named as the program wrote it
    /// (`written_name`): for a message, never for a name anything looks up.
    pub fn display_type_names(&self, ty: &Type) -> Type {
        self.named(ty, true)
    }

    fn name_of(&self, id: TypeId, written: bool) -> String {
        let name = self.type_name(id);
        if written { self.written_name(&name).to_string() } else { name }
    }

    fn named(&self, ty: &Type, written: bool) -> Type {
        match ty {
            Type::Named(id) => Type::UnresolvedNamed(self.name_of(*id, written)),
            Type::Result { ok, err } if **err == Type::None => {
                Type::option(self.named(ok, written))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.named(ok, written)),
                err: Box::new(self.named(err, written)),
            },
            Type::Generic { base, args } => {
                // Canonicalize Result<T, E> and Option<T> to their first-class variants
                if Some(*base) == self.result_type_id && args.len() == 2 {
                    if let (GenericArg::Type(ok), GenericArg::Type(err)) = (&args[0], &args[1]) {
                        return Type::Result {
                            ok: Box::new(self.named(ok, written)),
                            err: Box::new(self.named(err, written)),
                        };
                    }
                }
                if Some(*base) == self.option_type_id && args.len() == 1 {
                    if let GenericArg::Type(inner) = &args[0] {
                        return Type::option(self.named(inner, written));
                    }
                }
                Type::UnresolvedGeneric {
                    name: self.name_of(*base, written),
                    args: args.iter().map(|a| self.named_arg(a, written)).collect(),
                }
            }
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| self.named(p, written)).collect(),
                ret: Box::new(self.named(ret, written)),
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.named(e, written)).collect()),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.named(elem, written)),
                len: *len,
            },
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args.iter().map(|a| self.named_arg(a, written)).collect(),
            },
            Type::Union(types) => Type::Union(types.iter().map(|t| self.named(t, written)).collect()),
            // A message naming `Cell1<i32>.Out` printed `<type#134><i32>.Out`.
            Type::Assoc { base, name } => Type::Assoc {
                base: Box::new(self.named(base, written)),
                name: name.clone(),
            },
            other => other.clone(),
        }
    }

    fn named_arg(&self, arg: &GenericArg, written: bool) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(Box::new(self.named(ty, written))),
            GenericArg::ConstUsize(n) => GenericArg::ConstUsize(*n),
        }
    }

    /// Fill in the names of every type an error carries.
    ///
    /// The walk is `TypeError::map_types`, which is exhaustive. This used to be a
    /// hand-written match ending in `other => other`: it covered 17 of the 33
    /// variants that carry a type, and every variant added after it was written
    /// silently fell through with its names unresolved. That's why the error side
    /// of a `T or E` printed as an internal id in `WrapperMethodCut`,
    /// `TryOnFlatShape` and others while `Mismatch` got it right in the same run
    /// (#646).
    pub fn resolve_error_types(&self, mut error: TypeError) -> TypeError {
        error.map_types(&|ty| self.display_type_names(ty));
        error
    }
}

impl TypeTable {
    /// Bring a declaration's type parameters into scope for name resolution.
    /// Returns the previous scope, to be handed back to `pop_type_params`.
    pub fn push_type_params(&mut self, names: Vec<String>) -> Vec<String> {
        std::mem::replace(&mut self.type_param_scope, names)
    }

    /// Restore the scope `push_type_params` replaced.
    pub fn pop_type_params(&mut self, previous: Vec<String>) {
        self.type_param_scope = previous;
    }

    /// Is this name a type parameter of whatever is being checked?
    pub fn is_type_param_in_scope(&self, name: &str) -> bool {
        self.type_param_scope.iter().any(|p| p == name)
    }

    /// The names `is_type_param_in_scope` answers yes to.
    pub fn type_param_scope(&self) -> &[String] {
        &self.type_param_scope
    }

    /// Record a module-level const's integer value, for array lengths.
    pub fn register_const_length(&mut self, name: String, value: usize) {
        self.const_lengths.insert(name, value);
    }

    /// The value of a const usable as an array length, if there is one.
    pub fn const_length(&self, name: &str) -> Option<usize> {
        self.const_lengths.get(name).copied()
    }
}

/// OR6: the primitives a conformance may be written against.
///
/// `string` is here for the operator interfaces it can carry from a package
/// (`Path`'s `/` is one), not for `+` — `type.strings` keeps concatenation a
/// method (E0397). `void`, `none` and the C aliases have no operators to
/// answer for.
pub const PRIMITIVE_CONFORMANCE_TARGETS: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128",
    "u8", "u16", "u32", "u64", "u128",
    "f32", "f64", "bool", "char", "string",
];

/// The source spelling of a primitive type, or `None` for everything else.
pub fn primitive_spelling(ty: &Type) -> Option<&'static str> {
    Some(match ty {
        Type::I8 => "i8",
        Type::I16 => "i16",
        Type::I32 => "i32",
        Type::I64 => "i64",
        Type::I128 => "i128",
        Type::U8 => "u8",
        Type::U16 => "u16",
        Type::U32 => "u32",
        Type::U64 => "u64",
        Type::U128 => "u128",
        Type::F32 => "f32",
        Type::F64 => "f64",
        Type::Bool => "bool",
        Type::Char => "char",
        Type::String => "string",
        _ => return None,
    })
}
