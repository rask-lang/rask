// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Central type registry.

use std::collections::HashMap;

use super::builtins::BuiltinModules;
use super::type_defs::TypeDef;
use super::errors::TypeError;

use crate::types::{GenericArg, Type, TypeId, TypeVarId};

/// Central registry of all types in the program.
#[derive(Debug, Default)]
pub struct TypeTable {
    /// User-defined types indexed by TypeId.
    pub(super) types: Vec<TypeDef>,
    /// Name to TypeId mapping.
    pub(super) type_names: HashMap<String, TypeId>,
    /// Built-in type names mapped to Type.
    pub(super) builtins: HashMap<String, Type>,
    /// TypeId for the builtin Option<T> enum.
    pub(super) option_type_id: Option<TypeId>,
    /// TypeId for the builtin Result<T, E> enum.
    pub(super) result_type_id: Option<TypeId>,
    /// Builtin modules registry.
    pub(super) builtin_modules: BuiltinModules,
}

impl TypeTable {
    pub fn new() -> Self {
        let mut table = Self {
            types: Vec::new(),
            type_names: HashMap::new(),
            builtins: HashMap::new(),
            option_type_id: None,
            result_type_id: None,
            builtin_modules: BuiltinModules::new(),
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
        self.builtins.insert("int".to_string(), Type::I64);
        self.builtins.insert("uint".to_string(), Type::U64);
        self.builtins.insert("isize".to_string(), Type::I64);
        self.builtins.insert("usize".to_string(), Type::U64);

        let option_id = self.register_type(TypeDef::Enum {
            name: "Option".to_string(),
            type_params: vec!["T".to_string()],
            variants: vec![
                ("Some".to_string(), vec![Type::Var(TypeVarId(0))]),
                ("None".to_string(), vec![]),
            ],
            methods: vec![],
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
        });
        self.result_type_id = Some(result_id);
    }

    /// Register a user-defined type.
    pub fn register_type(&mut self, def: TypeDef) -> TypeId {
        let id = TypeId(self.types.len() as u32);
        let name = match &def {
            TypeDef::Struct { name, .. } => name.clone(),
            TypeDef::Enum { name, .. } => name.clone(),
            TypeDef::Trait { name, .. } => name.clone(),
            TypeDef::Union { name, .. } => name.clone(),
        };
        self.types.push(def);
        // Also register the base name (without <...>) for generic type lookup
        if let Some(base_end) = name.find('<') {
            let base_name = name[..base_end].to_string();
            self.type_names.insert(base_name, id);
        }
        self.type_names.insert(name, id);
        id
    }

    /// Look up a type by name.
    pub fn lookup(&self, name: &str) -> Option<Type> {
        if let Some(ty) = self.builtins.get(name) {
            return Some(ty.clone());
        }
        self.type_names.get(name).map(|&id| Type::Named(id))
    }

    /// Get a type definition by ID.
    pub fn get(&self, id: TypeId) -> Option<&TypeDef> {
        self.types.get(id.0 as usize)
    }

    /// Get a mutable type definition by ID.
    pub fn get_mut(&mut self, id: TypeId) -> Option<&mut TypeDef> {
        self.types.get_mut(id.0 as usize)
    }

    /// Check if a name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.builtins.contains_key(name) || self.type_names.contains_key(name)
    }

    /// Get TypeId for a name (user-defined types only).
    pub fn get_type_id(&self, name: &str) -> Option<TypeId> {
        self.type_names.get(name).copied()
    }

    /// Check if a type name refers to a `@resource` struct.
    pub fn is_resource_type(&self, name: &str) -> bool {
        if let Some(&id) = self.type_names.get(name) {
            if let Some(TypeDef::Struct { is_resource, .. }) = self.types.get(id.0 as usize) {
                return *is_resource;
            }
        }
        false
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
    pub fn type_name(&self, id: TypeId) -> String {
        match self.get(id) {
            Some(TypeDef::Struct { name, .. }) => name.clone(),
            Some(TypeDef::Enum { name, .. }) => name.clone(),
            Some(TypeDef::Trait { name, .. }) => name.clone(),
            Some(TypeDef::Union { name, .. }) => name.clone(),
            None => format!("<type#{}>", id.0),
        }
    }

    fn resolve_type_names(&self, ty: &Type) -> Type {
        match ty {
            Type::Named(id) => Type::UnresolvedNamed(self.type_name(*id)),
            Type::Option(inner) => Type::Option(Box::new(self.resolve_type_names(inner))),
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
                        return Type::Option(Box::new(self.resolve_type_names(inner)));
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

    pub fn resolve_error_types(&self, error: TypeError) -> TypeError {
        match error {
            TypeError::Mismatch { expected, found, span } => TypeError::Mismatch {
                expected: self.resolve_type_names(&expected),
                found: self.resolve_type_names(&found),
                span,
            },
            TypeError::NotCallable { ty, span } => TypeError::NotCallable {
                ty: self.resolve_type_names(&ty),
                span,
            },
            TypeError::NoSuchField { ty, field, span } => TypeError::NoSuchField {
                ty: self.resolve_type_names(&ty),
                field,
                span,
            },
            TypeError::NoSuchMethod { ty, method, span } => TypeError::NoSuchMethod {
                ty: self.resolve_type_names(&ty),
                method,
                span,
            },
            TypeError::MissingReturn { function_name, expected_type, span } => TypeError::MissingReturn {
                function_name,
                expected_type: self.resolve_type_names(&expected_type),
                span,
            },
            TypeError::TryInNonPropagatingContext { return_ty, span } => TypeError::TryInNonPropagatingContext {
                return_ty: self.resolve_type_names(&return_ty),
                span,
            },
            TypeError::InfiniteType { var, ty, span } => TypeError::InfiniteType {
                var,
                ty: self.resolve_type_names(&ty),
                span,
            },
            TypeError::TryOnNonResult { found, span } => TypeError::TryOnNonResult {
                found: self.resolve_type_names(&found),
                span,
            },
            other => other,
        }
    }
}
