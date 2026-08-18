// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Central type registry.

use std::collections::HashMap;

use rask_ast::NodeId;

use super::builtins::BuiltinModules;
use super::type_defs::{BinaryStructInfo, TypeDef};
use super::errors::TypeError;

use crate::types::{GenericArg, Type, TypeId, TypeVarId};

/// Central registry of all types in the program.
#[derive(Debug, Default)]
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
    /// Whether registrations and lookups are on behalf of stdlib code.
    /// Mirrors the resolver's flag of the same name.
    pub(super) stdlib_mode: bool,
    /// Built-in type names mapped to Type.
    pub(super) builtins: HashMap<String, Type>,
    /// Type alias name → target type string.
    pub(super) type_aliases: HashMap<String, String>,
    /// TypeId for the builtin Option<T> enum.
    pub(super) option_type_id: Option<TypeId>,
    /// TypeId for the builtin Result<T, E> enum.
    pub(super) result_type_id: Option<TypeId>,
    /// Builtin modules registry.
    pub(super) builtin_modules: BuiltinModules,
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
    /// G1: declared/derived trait conformances (nominal). TypeId → trait base
    /// names the type conforms to, from `extend T with Trait` and auto-derive.
    pub(super) conformances: HashMap<TypeId, std::collections::HashSet<String>>,
    /// CC1/CC2: conditional-conformance conditions. (TypeId, trait base) → the
    /// `where` bounds (type-param name → required trait names) that must hold
    /// for the conformance, checked per instantiation.
    pub(super) conformance_conditions: HashMap<(TypeId, String), Vec<(String, Vec<String>)>>,
}

impl TypeTable {
    pub fn new() -> Self {
        let mut table = Self {
            types: Vec::new(),
            type_names: HashMap::new(),
            stdlib_type_names: HashMap::new(),
            stdlib_mode: false,
            builtins: HashMap::new(),
            type_aliases: HashMap::new(),
            option_type_id: None,
            result_type_id: None,
            builtin_modules: BuiltinModules::new(),
            binary_structs: HashMap::new(),
            variant_field_names: HashMap::new(),
            type_method_decls: HashMap::new(),
            conformances: HashMap::new(),
            conformance_conditions: HashMap::new(),
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

        let option_id = self.register_type(TypeDef::Enum {
            name: "Option".to_string(),
            type_params: vec!["T".to_string()],
            variants: vec![
                ("Some".to_string(), vec![Type::Var(TypeVarId(0))]),
                ("None".to_string(), vec![]),
            ],
            methods: vec![],
            is_transitive_resource: false,
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
            TypeDef::Trait { name, .. } => name.clone(),
            TypeDef::Union { name, .. } => name.clone(),
            TypeDef::NominalAlias { name, .. } => name.clone(),
        };

        // Option/Result have fixed builtin TypeIds. Redeclaration from stdlib
        // (e.g., `enum Option<T> { ... }` in option.rk) must merge methods
        // into the existing entry rather than duplicating it, so `T?` sugar
        // and `Option<T>` resolve to the same TypeId.
        //
        // Match on the base name (strip generic params) since the parser
        // stores names with their generic signature (e.g. "Option<T>").
        let base_name = name.split('<').next().unwrap_or(&name);
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
        self.types.push(def);

        let mut names = vec![name.clone()];
        // Also register the base name (without <...>) for generic type lookup
        if let Some(base_end) = name.find('<') {
            names.push(name[..base_end].to_string());
        }

        for n in names {
            if self.stdlib_mode {
                // Stdlib code always means this one.
                self.stdlib_type_names.insert(n.clone(), id);
                // Program code means it too, unless the program declares its
                // own. Registering stdlib first and not overwriting later is
                // what makes a program type shadow rather than collide.
                self.type_names.entry(n).or_insert(id);
            } else {
                self.type_names.insert(n, id);
            }
        }
        id
    }

    /// The name map to consult first, given who's asking.
    fn primary_names(&self) -> &HashMap<String, TypeId> {
        if self.stdlib_mode { &self.stdlib_type_names } else { &self.type_names }
    }

    /// The other one, for names the primary doesn't know.
    fn fallback_names(&self) -> &HashMap<String, TypeId> {
        if self.stdlib_mode { &self.type_names } else { &self.stdlib_type_names }
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

    /// Every type that declares methods, paired with the declarations carrying them.
    pub fn types_with_methods(&self) -> impl Iterator<Item = (TypeId, &[NodeId])> {
        self.type_method_decls.iter().map(|(id, decls)| (*id, decls.as_slice()))
    }

    /// Register a transparent type alias.
    pub fn register_alias(&mut self, name: String, target: String) {
        self.type_aliases.insert(name, target);
    }

    /// Resolve a type alias chain, returning the final target string.
    /// Returns None if name is not an alias.
    fn resolve_alias<'a>(&'a self, name: &'a str) -> Option<&'a str> {
        let mut current = name;
        let mut visited = Vec::new();
        loop {
            match self.type_aliases.get(current) {
                Some(target) => {
                    if visited.contains(&current) {
                        // Cycle — caller should have caught this at registration
                        return None;
                    }
                    visited.push(current);
                    current = target.as_str();
                }
                None => {
                    if current == name {
                        return None;
                    }
                    return Some(current);
                }
            }
        }
    }

    /// Check if registering `name -> target` would create a cycle.
    /// Returns the cycle path if so.
    pub fn check_alias_cycle(&self, name: &str, target: &str) -> Option<Vec<String>> {
        let mut current = target;
        let mut path = vec![name.to_string(), target.to_string()];
        loop {
            if current == name {
                return Some(path);
            }
            match self.type_aliases.get(current) {
                Some(next) => {
                    path.push(next.clone());
                    current = next.as_str();
                }
                None => return None,
            }
        }
    }

    /// Look up a type by name.
    pub fn lookup(&self, name: &str) -> Option<Type> {
        if let Some(ty) = self.builtins.get(name) {
            return Some(ty.clone());
        }
        // Check type aliases
        if let Some(target) = self.resolve_alias(name) {
            if let Some(ty) = self.builtins.get(target) {
                return Some(ty.clone());
            }
            return self.resolve_name(target).map(Type::Named);
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

    /// The key a conformance is filed under: generic args stripped.
    fn conformance_key(trait_name: &str) -> String {
        trait_name.split('<').next().unwrap_or(trait_name).trim().to_string()
    }

    /// G1: record that a type conforms to a trait (declared or auto-derived).
    /// Trait names are stored base-only (generic args stripped).
    pub fn record_conformance(&mut self, type_id: TypeId, trait_name: &str) {
        self.conformances
            .entry(type_id)
            .or_default()
            .insert(Self::conformance_key(trait_name));
    }

    /// G1: does the type declare (or auto-derive) conformance to the trait?
    pub fn declares_conformance(&self, type_id: TypeId, trait_name: &str) -> bool {
        let base = Self::conformance_key(trait_name);
        self.conformances.get(&type_id).is_some_and(|set| set.contains(&base))
    }

    /// CC1/CC2: record the `where` condition for a conditional conformance.
    pub fn record_conformance_condition(
        &mut self,
        type_id: TypeId,
        trait_name: &str,
        bounds: Vec<(String, Vec<String>)>,
    ) {
        self.conformance_conditions
            .insert((type_id, Self::conformance_key(trait_name)), bounds);
    }

    /// CC1: the `where` condition for a conformance, if it's conditional.
    pub fn conformance_condition(
        &self,
        type_id: TypeId,
        trait_name: &str,
    ) -> Option<&Vec<(String, Vec<String>)>> {
        let base = Self::conformance_key(trait_name);
        let base = base.as_str();
        self.conformance_conditions.get(&(type_id, base.to_string()))
    }

    /// Check if a name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.builtins.contains_key(name)
            || self.type_names.contains_key(name)
            || self.type_aliases.contains_key(name)
    }

    /// Get TypeId for a name (user-defined types only).
    /// Resolves through aliases.
    pub fn get_type_id(&self, name: &str) -> Option<TypeId> {
        if let Some(id) = self.resolve_name(name) {
            return Some(id);
        }
        if let Some(target) = self.resolve_alias(name) {
            return self.resolve_name(target);
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
    /// inherit the obligation. Trait objects and unresolved/error types are
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
            Type::Array { elem, .. } | Type::Slice(elem) => {
                self.type_is_transitive_resource(elem)
            }
            Type::Result { ok, err } => {
                self.type_is_transitive_resource(ok) || self.type_is_transitive_resource(err)
            }
            Type::Union(variants) => variants.iter().any(|v| self.type_is_transitive_resource(v)),
            Type::UnresolvedNamed(name) => {
                let base = name.split('<').next().unwrap_or(name);
                self.type_names
                    .get(base)
                    .map_or(false, |id| self.is_transitive_resource_by_id(*id))
            }
            Type::UnresolvedGeneric { name, args } => {
                let base_name = name.split('<').next().unwrap_or(name);
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

    /// RC1/RC3: is a value of this type *itself linear* — a thing the language
    /// requires be consumed exactly once? `@resource` structs/enums (directly or
    /// transitively), and the tuples/arrays/optionals/results built from them,
    /// qualify. Wrapper containers (`Handle`, `WeakHandle`, `Pool`, `Vec`, `Map`)
    /// do NOT: a `Handle<File>` is a copyable value, and a `Vec<File>`/`Pool<File>`
    /// is a container whose own drop story is decided separately (Pool is the
    /// sanctioned one, Vec is the violation `find_linear_container` reports).
    ///
    /// This is deliberately narrower than `type_is_transitive_resource`, which
    /// recurses into *every* generic arg and so treats `Handle<File>` as linear.
    /// For the container-element rule that's a false positive — the spec's own
    /// `Vec<Handle<Connection>>` example is legal.
    pub fn is_linear_value(&self, ty: &Type) -> bool {
        match ty {
            Type::Named(id) => self.is_transitive_resource_by_id(*id),
            Type::Generic { base, args } => {
                let full = self.type_name(*base);
                let name = full.split('<').next().unwrap_or(&full);
                if Self::is_nonlinear_wrapper(name) {
                    return false;
                }
                if self.is_transitive_resource_by_id(*base) {
                    return true;
                }
                args.iter().any(|a| matches!(a, GenericArg::Type(t) if self.is_linear_value(t)))
            }
            Type::UnresolvedGeneric { name, args } => {
                let base = name.split('<').next().unwrap_or(name);
                if Self::is_nonlinear_wrapper(base) {
                    return false;
                }
                if let Some(&id) = self.type_names.get(base) {
                    if self.is_transitive_resource_by_id(id) {
                        return true;
                    }
                }
                args.iter().any(|a| matches!(a, GenericArg::Type(t) if self.is_linear_value(t)))
            }
            Type::UnresolvedNamed(name) => {
                let base = name.split('<').next().unwrap_or(name);
                self.type_names
                    .get(base)
                    .map_or(false, |id| self.is_transitive_resource_by_id(*id))
            }
            Type::Tuple(elems) | Type::Union(elems) => {
                elems.iter().any(|t| self.is_linear_value(t))
            }
            Type::Array { elem, .. } | Type::Slice(elem) => self.is_linear_value(elem),
            // `T?` is `Result { ok: T, err: none }`; both `T or E` and `T?` carry
            // their payload linearly (RC4: an optional resource must be matched
            // and consumed), so a Vec of them is still a violation.
            Type::Result { ok, err } => self.is_linear_value(ok) || self.is_linear_value(err),
            _ => false,
        }
    }

    /// Container wrappers that hold values without becoming linear themselves.
    /// `Pool` is the sanctioned resource container (RC2); `Handle`/`WeakHandle`
    /// are copyable references; `Vec`/`Map` are handled by the outer walk.
    fn is_nonlinear_wrapper(name: &str) -> bool {
        matches!(name, "Handle" | "WeakHandle" | "Pool" | "Vec" | "Map")
    }

    /// RC1/RC3: find the first `Vec<T>` or `Map<K, V>` anywhere in `ty` whose
    /// element (or key) is a linear value. `Vec` and `Map` can't consume their
    /// elements on drop, so linear elements are rejected at the type. Returns the
    /// container spelling ("Vec"/"Map") and the offending element type.
    ///
    /// Walks the whole type tree so nested forms (`Vec<Vec<File>>`,
    /// `Map<string, File>` inside a tuple, a `Vec<File>` return of a `func`
    /// type) are caught at their innermost violation.
    /// HA4: the float key of a `Map` nested anywhere in this type, if there is
    /// one. `f32`/`f64` are not Hashable — `NaN != NaN` breaks the contract that
    /// equal keys hash equal — so they can't key a Map.
    pub fn find_float_map_key(&self, ty: &Type) -> Option<Type> {
        let args = match ty {
            Type::Generic { base, args } => {
                let full = self.type_name(*base);
                if full.split('<').next() == Some("Map") {
                    if let Some(GenericArg::Type(k)) = args.first() {
                        if matches!(**k, Type::F32 | Type::F64) {
                            return Some((**k).clone());
                        }
                    }
                }
                Some(args)
            }
            Type::UnresolvedGeneric { name, args } => {
                if name.split('<').next() == Some("Map") {
                    if let Some(GenericArg::Type(k)) = args.first() {
                        if matches!(**k, Type::F32 | Type::F64) {
                            return Some((**k).clone());
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
            Type::Slice(inner) | Type::RawPtr(inner) => nested.push(inner),
            Type::Array { elem, .. } => nested.push(elem),
            Type::Result { ok, err } => {
                nested.push(ok);
                nested.push(err);
            }
            _ => {}
        }
        nested.into_iter().find_map(|t| self.find_float_map_key(t))
    }

    pub fn find_linear_container(&self, ty: &Type) -> Option<(String, Type)> {
        // Check this node if it's a Vec/Map, then always recurse into children so
        // nested violations surface at their innermost container.
        match ty {
            Type::Generic { base, args } => {
                // `type_name` includes generic params ("Vec<T>"); strip them.
                let full = self.type_name(*base);
                let name = full.split('<').next().unwrap_or(&full);
                if let Some(hit) = self.container_violation(name, args) {
                    return Some(hit);
                }
                self.first_container_in_args(args)
            }
            Type::UnresolvedGeneric { name, args } => {
                let base = name.split('<').next().unwrap_or(name);
                if let Some(hit) = self.container_violation(base, args) {
                    return Some(hit);
                }
                self.first_container_in_args(args)
            }
            Type::Tuple(elems) | Type::Union(elems) => {
                elems.iter().find_map(|t| self.find_linear_container(t))
            }
            Type::Array { elem, .. } | Type::Slice(elem) | Type::RawPtr(elem) => {
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

    fn first_container_in_args(&self, args: &[GenericArg]) -> Option<(String, Type)> {
        args.iter().find_map(|a| match a {
            GenericArg::Type(t) => self.find_linear_container(t),
            _ => None,
        })
    }

    /// If `name`/`args` describe a `Vec<T>` or `Map<K, V>` with a linear element
    /// or key, return the violation. The check is head-only; the caller recurses.
    fn container_violation(&self, name: &str, args: &[GenericArg]) -> Option<(String, Type)> {
        let elem = |i: usize| match args.get(i) {
            Some(GenericArg::Type(t)) => Some(t.as_ref()),
            _ => None,
        };
        match name {
            "Vec" => {
                let e = elem(0)?;
                if self.is_linear_value(e) {
                    return Some(("Vec".to_string(), e.clone()));
                }
                None
            }
            "Map" => {
                // A resource key is as unconsumable on drop as a resource value.
                if let Some(k) = elem(0) {
                    if self.is_linear_value(k) {
                        return Some(("Map".to_string(), k.clone()));
                    }
                }
                if let Some(v) = elem(1) {
                    if self.is_linear_value(v) {
                        return Some(("Map".to_string(), v.clone()));
                    }
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
                Some(name.split('<').next().unwrap_or(name).trim().to_string())
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
    pub fn type_name(&self, id: TypeId) -> String {
        let name = match self.get(id) {
            Some(TypeDef::Struct { name, .. }) => name,
            Some(TypeDef::Enum { name, .. }) => name,
            Some(TypeDef::Trait { name, .. }) => name,
            Some(TypeDef::Union { name, .. }) => name,
            Some(TypeDef::NominalAlias { name, .. }) => name,
            None => return format!("<type#{}>", id.0),
        };
        name.split('<').next().unwrap_or(name).to_string()
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
        match ty {
            Type::Named(id) => Type::UnresolvedNamed(self.type_name(*id)),
            Type::Result { ok, err } if **err == Type::None => {
                Type::option(self.resolve_type_names(ok))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.resolve_type_names(ok)),
                err: Box::new(self.resolve_type_names(err)),
            },
            Type::Generic { base, args } => {
                // Canonicalize Result<T, E> and Option<T> to their first-class variants
                if Some(*base) == self.result_type_id && args.len() == 2 {
                    if let (GenericArg::Type(ok), GenericArg::Type(err)) = (&args[0], &args[1]) {
                        return Type::Result {
                            ok: Box::new(self.resolve_type_names(ok)),
                            err: Box::new(self.resolve_type_names(err)),
                        };
                    }
                }
                if Some(*base) == self.option_type_id && args.len() == 1 {
                    if let GenericArg::Type(inner) = &args[0] {
                        return Type::option(self.resolve_type_names(inner));
                    }
                }
                Type::UnresolvedGeneric {
                    name: self.type_name(*base),
                    args: args.iter().map(|a| self.resolve_generic_arg(a)).collect(),
                }
            }
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| self.resolve_type_names(p)).collect(),
                ret: Box::new(self.resolve_type_names(ret)),
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| self.resolve_type_names(e)).collect()),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.resolve_type_names(elem)),
                len: *len,
            },
            Type::Slice(elem) => Type::Slice(Box::new(self.resolve_type_names(elem))),
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args.iter().map(|a| self.resolve_generic_arg(a)).collect(),
            },
            Type::Union(types) => Type::Union(types.iter().map(|t| self.resolve_type_names(t)).collect()),
            other => other.clone(),
        }
    }

    fn resolve_generic_arg(&self, arg: &GenericArg) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(Box::new(self.resolve_type_names(ty))),
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
        error.map_types(&|ty| self.resolve_type_names(ty));
        error
    }
}
