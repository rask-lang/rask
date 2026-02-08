// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type checker implementation.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, EnumDecl, FnDecl, ImplDecl, StructDecl, TraitDecl};
use rask_ast::expr::{BinOp, Expr, ExprKind, Pattern};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};
use rask_resolve::{ResolvedProgram, SymbolId, SymbolKind};

use crate::types::{GenericArg, Type, TypeId, TypeVarId};

// ============================================================================
// Type Definitions
// ============================================================================

/// Information about a user-defined type.
#[derive(Debug, Clone)]
pub enum TypeDef {
    Struct {
        name: String,
        type_params: Vec<String>,
        fields: Vec<(String, Type)>,
        methods: Vec<MethodSig>,
    },
    Enum {
        name: String,
        type_params: Vec<String>,
        variants: Vec<(String, Vec<Type>)>,
        methods: Vec<MethodSig>,
    },
    Trait {
        name: String,
        methods: Vec<MethodSig>,
    },
}

/// Method signature.
#[derive(Debug, Clone)]
pub struct MethodSig {
    pub name: String,
    pub self_param: SelfParam,
    pub params: Vec<(Type, ParamMode)>,
    pub ret: Type,
}

/// How self is passed to a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfParam {
    None,  // Static method
    Value, // self (by value, default)
    Read,  // read self (borrowed, read-only)
    Take,  // take self (consumed)
}

/// How a parameter is passed to a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamMode {
    Default, // Normal pass-by-value (mutability inferred)
    Read,    // read param (enforced read-only)
    Take,    // take param (consumed)
}

// ============================================================================
// Builtin Modules
// ============================================================================

/// Builtin module method signature.
#[derive(Debug, Clone)]
pub struct ModuleMethodSig {
    pub name: String,
    pub params: Vec<Type>,
    pub ret: Type,
}

/// Registry of builtin modules and their methods.
#[derive(Debug, Default)]
pub struct BuiltinModules {
    modules: HashMap<String, Vec<ModuleMethodSig>>,
}

impl BuiltinModules {
    pub fn new() -> Self {
        let mut modules = HashMap::new();

        // fs module
        let mut fs_methods = Vec::new();
        let io_error_ty = Type::UnresolvedNamed("IoError".to_string());

        // fs.open(path: string) -> File or IoError
        fs_methods.push(ModuleMethodSig {
            name: "open".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("File".to_string())),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.create(path: string) -> File or IoError
        fs_methods.push(ModuleMethodSig {
            name: "create".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("File".to_string())),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.read_file(path: string) -> string or IoError
        fs_methods.push(ModuleMethodSig {
            name: "read_file".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::String),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.write_file(path: string, content: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "write_file".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.exists(path: string) -> bool
        fs_methods.push(ModuleMethodSig {
            name: "exists".to_string(),
            params: vec![Type::String],
            ret: Type::Bool,
        });
        // fs.read_lines(path: string) -> Vec<string> or IoError
        fs_methods.push(ModuleMethodSig {
            name: "read_lines".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("Vec<string>".to_string())),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.canonicalize(path: string) -> string or IoError
        fs_methods.push(ModuleMethodSig {
            name: "canonicalize".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::String),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.copy(from: string, to: string) -> u64 or IoError
        fs_methods.push(ModuleMethodSig {
            name: "copy".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Result {
                ok: Box::new(Type::U64),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.rename(from: string, to: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "rename".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.remove(path: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "remove".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.create_dir(path: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "create_dir".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.create_dir_all(path: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "create_dir_all".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.append_file(path: string, content: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "append_file".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });

        modules.insert("fs".to_string(), fs_methods);

        // net module
        let error_ty = Type::UnresolvedNamed("Error".to_string());
        let mut net_methods = Vec::new();
        // net.tcp_listen(addr: string) -> TcpListener or Error
        net_methods.push(ModuleMethodSig {
            name: "tcp_listen".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("TcpListener".to_string())),
                err: Box::new(error_ty.clone()),
            },
        });
        modules.insert("net".to_string(), net_methods);

        // json module
        let mut json_methods = Vec::new();
        // json.encode(value) -> string (accepts any type)
        json_methods.push(ModuleMethodSig {
            name: "encode".to_string(),
            params: vec![Type::UnresolvedNamed("_Any".to_string())],
            ret: Type::String,
        });
        // json.decode(str: string) -> T or Error (generic, returns fresh var)
        json_methods.push(ModuleMethodSig {
            name: "decode".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("_JsonDecodeResult".to_string())),
                err: Box::new(error_ty),
            },
        });
        modules.insert("json".to_string(), json_methods);

        Self { modules }
    }

    pub fn get_method(&self, module: &str, method: &str) -> Option<&ModuleMethodSig> {
        self.modules.get(module)?.iter().find(|m| m.name == method)
    }

    pub fn is_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }
}

// ============================================================================
// Type Table
// ============================================================================

/// Central registry of all types in the program.
#[derive(Debug, Default)]
pub struct TypeTable {
    /// User-defined types indexed by TypeId.
    types: Vec<TypeDef>,
    /// Name to TypeId mapping.
    type_names: HashMap<String, TypeId>,
    /// Built-in type names mapped to Type.
    builtins: HashMap<String, Type>,
    /// TypeId for the builtin Option<T> enum.
    option_type_id: Option<TypeId>,
    /// TypeId for the builtin Result<T, E> enum.
    result_type_id: Option<TypeId>,
    /// Builtin modules registry.
    builtin_modules: BuiltinModules,
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
            Type::Generic { base, args } => Type::UnresolvedGeneric {
                name: self.type_name(*base),
                args: args.iter().map(|a| self.resolve_generic_arg(a)).collect(),
            },
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
            other => other,
        }
    }
}

// ============================================================================
// Type Constraints
// ============================================================================

/// A constraint generated during type inference.
#[derive(Debug, Clone)]
pub enum TypeConstraint {
    /// Two types must be equal.
    Equal(Type, Type, Span),
    /// Type must have a field with given name and type.
    HasField {
        ty: Type,
        field: String,
        expected: Type,
        span: Span,
    },
    /// Type must have a method with given signature.
    HasMethod {
        ty: Type,
        method: String,
        args: Vec<Type>,
        ret: Type,
        span: Span,
    },
}

// ============================================================================
// Inference Context
// ============================================================================

/// State for type inference and unification.
#[derive(Debug, Default)]
pub struct InferenceContext {
    /// Counter for fresh type variables.
    next_var: u32,
    /// Substitutions: TypeVarId -> Type.
    substitutions: HashMap<TypeVarId, Type>,
    /// Constraints collected during inference.
    constraints: Vec<TypeConstraint>,
}

impl InferenceContext {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a fresh type variable.
    pub fn fresh_var(&mut self) -> Type {
        let id = TypeVarId(self.next_var);
        self.next_var += 1;
        Type::Var(id)
    }

    /// Add a constraint.
    pub fn add_constraint(&mut self, constraint: TypeConstraint) {
        self.constraints.push(constraint);
    }

    /// Apply all known substitutions to a type.
    pub fn apply(&self, ty: &Type) -> Type {
        match ty {
            Type::Var(id) => {
                if let Some(resolved) = self.substitutions.get(id) {
                    self.apply(resolved)
                } else {
                    ty.clone()
                }
            }
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(|a| self.apply_generic_arg(a)).collect(),
            },
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args.iter().map(|a| self.apply_generic_arg(a)).collect(),
            },
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|t| self.apply(t)).collect(),
                ret: Box::new(self.apply(ret)),
            },
            Type::Tuple(elems) => Type::Tuple(elems.iter().map(|t| self.apply(t)).collect()),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.apply(elem)),
                len: *len,
            },
            Type::Slice(inner) => Type::Slice(Box::new(self.apply(inner))),
            Type::Option(inner) => Type::Option(Box::new(self.apply(inner))),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.apply(ok)),
                err: Box::new(self.apply(err)),
            },
            _ => ty.clone(),
        }
    }

    fn apply_generic_arg(&self, arg: &GenericArg) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => GenericArg::Type(Box::new(self.apply(ty))),
            GenericArg::ConstUsize(n) => GenericArg::ConstUsize(*n),
        }
    }

    /// Check if a type variable occurs in a type (prevents infinite types).
    fn occurs_in(&self, var: TypeVarId, ty: &Type) -> bool {
        match ty {
            Type::Var(id) => {
                if *id == var {
                    return true;
                }
                if let Some(subst) = self.substitutions.get(id) {
                    return self.occurs_in(var, subst);
                }
                false
            }
            Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => {
                args.iter().any(|a| self.occurs_in_generic_arg(var, a))
            }
            Type::Fn { params, ret } => {
                params.iter().any(|p| self.occurs_in(var, p)) || self.occurs_in(var, ret)
            }
            Type::Tuple(elems) => elems.iter().any(|e| self.occurs_in(var, e)),
            Type::Array { elem, .. } => self.occurs_in(var, elem),
            Type::Slice(inner) | Type::Option(inner) => self.occurs_in(var, inner),
            Type::Result { ok, err } => self.occurs_in(var, ok) || self.occurs_in(var, err),
            _ => false,
        }
    }

    fn occurs_in_generic_arg(&self, var: TypeVarId, arg: &GenericArg) -> bool {
        match arg {
            GenericArg::Type(ty) => self.occurs_in(var, ty),
            GenericArg::ConstUsize(_) => false,
        }
    }
}

// ============================================================================
// Type Errors
// ============================================================================

/// A type error.
#[derive(Debug, thiserror::Error)]
pub enum TypeError {
    #[error("type mismatch: expected {expected}, found {found}")]
    Mismatch {
        expected: Type,
        found: Type,
        span: Span,
    },
    #[error("undefined type: {0}")]
    Undefined(String),
    #[error("arity mismatch: expected {expected} arguments, found {found}")]
    ArityMismatch {
        expected: usize,
        found: usize,
        span: Span,
    },
    #[error("type {ty} is not callable")]
    NotCallable { ty: Type, span: Span },
    #[error("no such field '{field}' on type {ty}")]
    NoSuchField { ty: Type, field: String, span: Span },
    #[error("no such method '{method}' on type {ty}")]
    NoSuchMethod {
        ty: Type,
        method: String,
        span: Span,
    },
    #[error("infinite type: type variable would create infinite type")]
    InfiniteType { var: TypeVarId, ty: Type, span: Span },
    #[error("cannot infer type")]
    CannotInfer { span: Span },
    #[error("invalid type string: {0}")]
    InvalidTypeString(String),
    #[error("try can only be used in functions returning Option or Result, found {return_ty}")]
    TryInNonPropagatingContext { return_ty: Type, span: Span },
    #[error("try can only be used within a function")]
    TryOutsideFunction { span: Span },
    #[error("missing return statement")]
    MissingReturn {
        function_name: String,
        expected_type: Type,
        span: Span,
    },
    #[error("generic argument error: {0}")]
    GenericError(String, Span),
    #[error("cannot mutate `{var}` while borrowed")]
    AliasingViolation {
        var: String,
        borrow_span: Span,
        access_span: Span,
    },
    #[error("cannot mutate read-only parameter `{name}`")]
    MutateReadParam {
        name: String,
        span: Span,
    },
}

// ============================================================================
// Type String Parser
// ============================================================================

/// Parse a type annotation string into a Type.
pub fn parse_type_string(s: &str, types: &TypeTable) -> Result<Type, TypeError> {
    let s = s.trim();

    if s.is_empty() || s == "()" {
        return Ok(Type::Unit);
    }

    if s == "!" {
        return Ok(Type::Never);
    }

    if s.ends_with('?') && !s.starts_with('(') {
        let inner = parse_type_string(&s[..s.len() - 1], types)?;
        return Ok(Type::Option(Box::new(inner)));
    }

    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        if inner.is_empty() {
            return Ok(Type::Unit);
        }
        let parts = split_type_args(inner);
        if parts.len() == 1 && !inner.contains(',') {
            return parse_type_string(inner, types);
        }
        let elems: Result<Vec<_>, _> = parts.iter().map(|p| parse_type_string(p, types)).collect();
        return Ok(Type::Tuple(elems?));
    }

    if s.starts_with("[]") {
        let inner = parse_type_string(&s[2..], types)?;
        return Ok(Type::Slice(Box::new(inner)));
    }

    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        if let Some(semi_pos) = inner.find(';') {
            let elem_str = inner[..semi_pos].trim();
            let len_str = inner[semi_pos + 1..].trim();
            let elem = parse_type_string(elem_str, types)?;
            // Numeric size or comptime param name — use placeholder 0 for symbolic sizes
            // so element type checking proceeds. Actual size resolves at comptime.
            let len: usize = len_str.parse().unwrap_or(0);
            return Ok(Type::Array {
                elem: Box::new(elem),
                len,
            });
        }
        let inner = parse_type_string(inner, types)?;
        return Ok(Type::Slice(Box::new(inner)));
    }

    if s.starts_with("func(") || s.starts_with("fn(") {
        return parse_fn_type(s, types);
    }

    if let Some(lt_pos) = s.find('<') {
        if s.ends_with('>') {
            let name = s[..lt_pos].trim();
            let args_str = &s[lt_pos + 1..s.len() - 1];
            let arg_strs = split_type_args(args_str);
            let args: Result<Vec<GenericArg>, _> =
                arg_strs.iter().map(|a| parse_generic_arg(a, types)).collect();
            let args = args?;

            match name {
                "Owned" if args.len() == 1 => {
                    // Owned<T> is transparent to the type checker — unwrap to T
                    if let GenericArg::Type(ty) = args.into_iter().next().unwrap() {
                        return Ok(*ty);
                    } else {
                        return Err(TypeError::GenericError(
                            "Owned expects a type argument, not a const".to_string(),
                            Span::new(0, 0),
                        ));
                    }
                }
                "Option" if args.len() == 1 => {
                    // Option takes a single type argument
                    if let GenericArg::Type(ty) = args.into_iter().next().unwrap() {
                        return Ok(Type::Option(ty));
                    } else {
                        return Err(TypeError::GenericError(
                            "Option expects a type argument, not a const".to_string(),
                            Span::new(0, 0),
                        ));
                    }
                }
                "Result" if args.len() == 2 => {
                    // Result takes two type arguments
                    let mut iter = args.into_iter();
                    let ok_arg = iter.next().unwrap();
                    let err_arg = iter.next().unwrap();

                    match (ok_arg, err_arg) {
                        (GenericArg::Type(ok), GenericArg::Type(err)) => {
                            return Ok(Type::Result { ok, err });
                        }
                        _ => {
                            return Err(TypeError::GenericError(
                                "Result expects two type arguments, not const".to_string(),
                                Span::new(0, 0),
                            ));
                        }
                    }
                }
                _ => {
                    if let Some(base_id) = types.get_type_id(name) {
                        return Ok(Type::Generic { base: base_id, args });
                    }
                    return Ok(Type::UnresolvedGeneric {
                        name: name.to_string(),
                        args,
                    });
                }
            }
        }
    }

    if let Some(ty) = types.lookup(s) {
        return Ok(ty);
    }

    Ok(Type::UnresolvedNamed(s.to_string()))
}

/// Parse a single generic argument, which can be either a type or a const value.
fn parse_generic_arg(s: &str, types: &TypeTable) -> Result<GenericArg, TypeError> {
    let trimmed = s.trim();

    // Try to parse as a usize literal (const generic)
    if let Ok(n) = trimmed.parse::<usize>() {
        return Ok(GenericArg::ConstUsize(n));
    }

    // Otherwise parse as a type
    let ty = parse_type_string(trimmed, types)?;
    Ok(GenericArg::Type(Box::new(ty)))
}

fn split_type_args(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut paren_depth = 0;
    let mut start = 0;

    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            ',' if depth == 0 && paren_depth == 0 => {
                result.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }

    if start < s.len() {
        result.push(s[start..].trim());
    }

    result
}

fn parse_fn_type(s: &str, types: &TypeTable) -> Result<Type, TypeError> {
    let prefix = if s.starts_with("func(") {
        "func("
    } else {
        "fn("
    };
    let rest = &s[prefix.len()..];

    let mut depth = 1;
    let mut paren_end = 0;
    for (i, c) in rest.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    paren_end = i;
                    break;
                }
            }
            _ => {}
        }
    }

    let params_str = &rest[..paren_end];
    let after_paren = &rest[paren_end + 1..].trim();

    let params: Result<Vec<_>, _> = if params_str.is_empty() {
        Ok(Vec::new())
    } else {
        split_type_args(params_str)
            .iter()
            .map(|p| parse_type_string(p, types))
            .collect()
    };
    let params = params?;

    let ret = if after_paren.starts_with("->") {
        let ret_str = after_paren[2..].trim();
        parse_type_string(ret_str, types)?
    } else {
        Type::Unit
    };

    Ok(Type::Fn {
        params,
        ret: Box::new(ret),
    })
}

// ============================================================================
// Typed Program Output
// ============================================================================

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
}

// ============================================================================
// Borrow Tracking for Aliasing Detection
// ============================================================================

/// Borrow mode for active borrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BorrowMode {
    Shared,    // Read-only borrow
    Exclusive, // Mutable borrow
}

/// An active borrow tracked during expression evaluation.
#[derive(Debug, Clone)]
struct ActiveBorrow {
    var_name: String,
    mode: BorrowMode,
    span: Span,
}

// ============================================================================
// Type Checker
// ============================================================================

/// The type checker.
pub struct TypeChecker {
    /// Symbol table from resolution.
    resolved: ResolvedProgram,
    /// Type registry.
    types: TypeTable,
    /// Inference state.
    ctx: InferenceContext,
    /// Types assigned to nodes.
    node_types: HashMap<NodeId, Type>,
    /// Types assigned to symbols (for bindings without annotations).
    symbol_types: HashMap<SymbolId, Type>,
    /// Collected errors.
    errors: Vec<TypeError>,
    /// Current function's return type (for checking return statements).
    current_return_type: Option<Type>,
    /// Current Self type (inside extend blocks).
    current_self_type: Option<Type>,
    /// Scope stack for local variable types (innermost scope last).
    /// Tuple: (type, is_read_only).
    local_types: Vec<HashMap<String, (Type, bool)>>,
    /// Active borrows for aliasing detection (ESAD Phase 1).
    borrow_stack: Vec<ActiveBorrow>,
}

impl TypeChecker {
    /// Create a new type checker.
    pub fn new(resolved: ResolvedProgram) -> Self {
        Self {
            resolved,
            types: TypeTable::new(),
            ctx: InferenceContext::new(),
            node_types: HashMap::new(),
            symbol_types: HashMap::new(),
            errors: Vec::new(),
            current_return_type: None,
            current_self_type: None,
            local_types: Vec::new(),
            borrow_stack: Vec::new(),
        }
    }

    pub fn check(mut self, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
        self.collect_type_declarations(decls);

        // Global scope for module-level bindings (imports, etc.)
        self.push_scope();
        for decl in decls {
            self.check_decl(decl);
        }
        self.pop_scope();

        self.solve_constraints();

        let node_types: HashMap<_, _> = self
            .node_types
            .iter()
            .map(|(id, ty)| (*id, self.ctx.apply(ty)))
            .collect();

        if self.errors.is_empty() {
            Ok(TypedProgram {
                symbols: self.resolved.symbols,
                resolutions: self.resolved.resolutions,
                types: self.types,
                node_types,
            })
        } else {
            let ctx = &self.ctx;
            let types = &self.types;
            let errors = self.errors.into_iter()
                .map(|e| Self::apply_error_substitutions_with_ctx(e, ctx))
                .map(|e| types.resolve_error_types(e))
                .collect();
            Err(errors)
        }
    }

    fn apply_error_substitutions_with_ctx(error: TypeError, ctx: &InferenceContext) -> TypeError {
        match error {
            TypeError::Mismatch { expected, found, span } => TypeError::Mismatch {
                expected: ctx.apply(&expected),
                found: ctx.apply(&found),
                span,
            },
            TypeError::NotCallable { ty, span } => TypeError::NotCallable {
                ty: ctx.apply(&ty),
                span,
            },
            TypeError::NoSuchField { ty, field, span } => TypeError::NoSuchField {
                ty: ctx.apply(&ty),
                field,
                span,
            },
            TypeError::NoSuchMethod { ty, method, span } => TypeError::NoSuchMethod {
                ty: ctx.apply(&ty),
                method,
                span,
            },
            TypeError::MissingReturn { function_name, expected_type, span } => TypeError::MissingReturn {
                function_name,
                expected_type: ctx.apply(&expected_type),
                span,
            },
            TypeError::TryInNonPropagatingContext { return_ty, span } => TypeError::TryInNonPropagatingContext {
                return_ty: ctx.apply(&return_ty),
                span,
            },
            TypeError::InfiniteType { var, ty, span } => TypeError::InfiniteType {
                var,
                ty: ctx.apply(&ty),
                span,
            },
            other => other,
        }
    }

    // ------------------------------------------------------------------------
    // Pass 1: Declaration Collection
    // ------------------------------------------------------------------------

    fn collect_type_declarations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Struct(s) => self.register_struct(s),
                DeclKind::Enum(e) => self.register_enum(e),
                DeclKind::Trait(t) => self.register_trait(t),
                _ => {}
            }
        }
        for decl in decls {
            if let DeclKind::Impl(i) = &decl.kind {
                self.register_impl_methods(i);
            }
        }
    }

    fn register_impl_methods(&mut self, i: &ImplDecl) {
        let type_id = match self.types.get_type_id(&i.target_ty) {
            Some(id) => id,
            None => return,
        };
        let new_methods: Vec<_> = i.methods.iter().map(|m| self.method_signature(m)).collect();
        if let Some(def) = self.types.get_mut(type_id) {
            match def {
                TypeDef::Struct { methods, .. } | TypeDef::Enum { methods, .. } => {
                    methods.extend(new_methods);
                }
                _ => {}
            }
        }
    }

    fn register_struct(&mut self, s: &StructDecl) {
        let fields: Vec<_> = s
            .fields
            .iter()
            .map(|f| {
                let ty = parse_type_string(&f.ty, &self.types).unwrap_or(Type::Error);
                (f.name.clone(), ty)
            })
            .collect();

        let methods = s.methods.iter().map(|m| self.method_signature(m)).collect();

        let type_params: Vec<String> = s.type_params.iter().map(|p| p.name.clone()).collect();
        self.types.register_type(TypeDef::Struct {
            name: s.name.clone(),
            type_params,
            fields,
            methods,
        });
    }

    fn register_enum(&mut self, e: &EnumDecl) {
        let variants: Vec<_> = e
            .variants
            .iter()
            .map(|v| {
                let field_types: Vec<_> = v
                    .fields
                    .iter()
                    .map(|f| parse_type_string(&f.ty, &self.types).unwrap_or(Type::Error))
                    .collect();
                (v.name.clone(), field_types)
            })
            .collect();

        let methods = e.methods.iter().map(|m| self.method_signature(m)).collect();

        let type_params: Vec<String> = e.type_params.iter().map(|p| p.name.clone()).collect();
        self.types.register_type(TypeDef::Enum {
            name: e.name.clone(),
            type_params,
            variants,
            methods,
        });
    }

    fn register_trait(&mut self, t: &TraitDecl) {
        let methods = t.methods.iter().map(|m| self.method_signature(m)).collect();

        self.types.register_type(TypeDef::Trait {
            name: t.name.clone(),
            methods,
        });
    }

    fn method_signature(&self, m: &FnDecl) -> MethodSig {
        let self_param_decl = m.params.iter().find(|p| p.name == "self");
        let self_param = match self_param_decl {
            Some(p) if p.is_take => SelfParam::Take,
            Some(p) if p.ty == "read Self" || p.ty == "read" => SelfParam::Read,
            Some(_) => SelfParam::Value,
            None => SelfParam::None,
        };

        let params: Vec<_> = m
            .params
            .iter()
            .filter(|p| p.name != "self")
            .map(|p| {
                let ty = parse_type_string(&p.ty, &self.types).unwrap_or(Type::Error);
                let mode = if p.is_take {
                    ParamMode::Take
                } else if p.is_read {
                    ParamMode::Read
                } else {
                    ParamMode::Default
                };
                (ty, mode)
            })
            .collect();

        let ret = m
            .ret_ty
            .as_ref()
            .map(|t| parse_type_string(t, &self.types).unwrap_or(Type::Error))
            .unwrap_or(Type::Unit);

        MethodSig {
            name: m.name.clone(),
            self_param,
            params,
            ret,
        }
    }

    // ------------------------------------------------------------------------
    // Pass 2: Check Declarations
    // ------------------------------------------------------------------------

    fn check_decl(&mut self, decl: &Decl) {
        match &decl.kind {
            DeclKind::Fn(f) => self.check_fn(f, decl.span),
            DeclKind::Struct(s) => {
                self.current_self_type = self.types.get_type_id(&s.name).map(Type::Named);
                for method in &s.methods {
                    self.check_fn(method, decl.span);
                }
                self.current_self_type = None;
            }
            DeclKind::Enum(e) => {
                self.current_self_type = self.types.get_type_id(&e.name).map(Type::Named);
                for method in &e.methods {
                    self.check_fn(method, decl.span);
                }
                self.current_self_type = None;
            }
            DeclKind::Impl(i) => {
                self.current_self_type = self.types.get_type_id(&i.target_ty).map(Type::Named);
                for method in &i.methods {
                    self.check_fn(method, decl.span);
                }
                self.current_self_type = None;
            }
            DeclKind::Const(c) => {
                let init_ty = self.infer_expr(&c.init);
                if let Some(ty_str) = &c.ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            declared,
                            init_ty,
                            decl.span,
                        ));
                    }
                }
            }
            DeclKind::Test(t) => {
                for stmt in &t.body {
                    self.check_stmt(stmt);
                }
            }
            DeclKind::Benchmark(b) => {
                for stmt in &b.body {
                    self.check_stmt(stmt);
                }
            }
            DeclKind::Import(imp) => {
                // Register module name as local for field/method resolution.
                // Modules handled by BuiltinModules (net, json, fs) route through
                // check_method_call directly. Others like 'time' need local registration
                // so field access (time.Instant) flows through resolve_field.
                if imp.path.len() == 1 {
                    let module_name = imp.alias.as_ref().unwrap_or(&imp.path[0]).clone();
                    if !self.types.builtin_modules.is_module(&module_name) {
                        self.define_local(
                            module_name.clone(),
                            Type::UnresolvedNamed(format!("__module_{}", module_name)),
                        );
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------
    // Scope Management
    // ------------------------------------------------------------------------

    fn push_scope(&mut self) {
        self.local_types.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.local_types.pop();
    }

    fn define_local(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, false));
        }
    }

    fn define_local_read_only(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, true));
        }
    }

    fn lookup_local(&self, name: &str) -> Option<Type> {
        for scope in self.local_types.iter().rev() {
            if let Some((ty, _)) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    /// Check if a local variable is read-only (from `read` parameter mode).
    fn is_local_read_only(&self, name: &str) -> bool {
        for scope in self.local_types.iter().rev() {
            if let Some((_, read_only)) = scope.get(name) {
                return *read_only;
            }
        }
        false
    }

    /// Extract the root identifier name from an assignment target expression.
    fn root_ident_name(expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object, .. } => Self::root_ident_name(object),
            ExprKind::Index { object, .. } => Self::root_ident_name(object),
            _ => None,
        }
    }

    // ------------------------------------------------------------------------
    // Borrow Stack Management (ESAD Phase 1)
    // ------------------------------------------------------------------------

    /// Push a borrow onto the stack.
    fn push_borrow(&mut self, var_name: String, mode: BorrowMode, span: Span) {
        self.borrow_stack.push(ActiveBorrow { var_name, mode, span });
    }

    /// Pop all borrows from the current expression (called at statement end).
    fn clear_expression_borrows(&mut self) {
        self.borrow_stack.clear();
    }

    /// Check if accessing a variable would conflict with active borrows.
    /// Returns the conflicting borrow if found.
    fn check_borrow_conflict(&self, var_name: &str, access_mode: BorrowMode) -> Option<&ActiveBorrow> {
        for borrow in self.borrow_stack.iter().rev() {
            if borrow.var_name == var_name {
                // Check conflict rules from ESAD spec
                match (borrow.mode, access_mode) {
                    (BorrowMode::Shared, BorrowMode::Shared) => {
                        // Shared + Shared = OK
                        continue;
                    }
                    (BorrowMode::Shared, BorrowMode::Exclusive) |
                    (BorrowMode::Exclusive, BorrowMode::Shared) |
                    (BorrowMode::Exclusive, BorrowMode::Exclusive) => {
                        // Any combination with Exclusive = ERROR
                        return Some(borrow);
                    }
                }
            }
        }
        None
    }

    /// Scan a closure body for variable accesses and check for conflicts.
    /// This implements ESAD Phase 2.
    fn check_closure_aliasing(&mut self, params: &[rask_ast::expr::ClosureParam], body: &Expr) {
        let param_names: std::collections::HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
        self.collect_closure_accesses(body, &param_names);
    }

    /// Recursively collect variable accesses in a closure body.
    /// Skip closure params — they're fresh bindings, not captures.
    fn collect_closure_accesses(&mut self, expr: &Expr, skip: &std::collections::HashSet<&str>) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                if skip.contains(name.as_str()) { return; }
                if let Some(borrow) = self.check_borrow_conflict(name, BorrowMode::Shared) {
                    self.errors.push(TypeError::AliasingViolation {
                        var: name.clone(),
                        borrow_span: borrow.span,
                        access_span: expr.span,
                    });
                }
            }
            ExprKind::MethodCall { object, method: _, args, .. } => {
                if let ExprKind::Ident(name) = &object.kind {
                    if !skip.contains(name.as_str()) {
                        if let Some(borrow) = self.check_borrow_conflict(name, BorrowMode::Exclusive) {
                            self.errors.push(TypeError::AliasingViolation {
                                var: name.clone(),
                                borrow_span: borrow.span,
                                access_span: object.span,
                            });
                        }
                    }
                }
                for arg in args {
                    self.collect_closure_accesses(arg, skip);
                }
            }
            ExprKind::Call { func, args } => {
                self.collect_closure_accesses(func, skip);
                for arg in args {
                    self.collect_closure_accesses(arg, skip);
                }
            }
            ExprKind::Block(stmts) => {
                for stmt in stmts {
                    if let StmtKind::Expr(e) = &stmt.kind {
                        self.collect_closure_accesses(e, skip);
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------------
    // Pattern Checking
    // ------------------------------------------------------------------------

    fn check_pattern(&mut self, pattern: &Pattern, scrutinee_ty: &Type, span: Span) -> Vec<(String, Type)> {
        match pattern {
            Pattern::Wildcard => vec![],

            Pattern::Ident(name) => {
                vec![(name.clone(), scrutinee_ty.clone())]
            }

            Pattern::Literal(expr) => {
                let lit_ty = self.infer_expr(expr);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    scrutinee_ty.clone(),
                    lit_ty,
                    span,
                ));
                vec![]
            }

            Pattern::Constructor { name, fields } => {
                self.check_constructor_pattern(name, fields, scrutinee_ty, span)
            }

            Pattern::Struct { name, fields, .. } => {
                // Look up the struct type
                if let Some(type_id) = self.types.get_type_id(name) {
                    // Constrain scrutinee to be this struct type
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        scrutinee_ty.clone(),
                        Type::Named(type_id),
                        span,
                    ));
                    // Check each field pattern
                    let struct_fields = self.types.get(type_id).and_then(|def| {
                        if let TypeDef::Struct { fields, .. } = def {
                            Some(fields.clone())
                        } else {
                            None
                        }
                    });
                    let mut bindings = vec![];
                    if let Some(struct_fields) = struct_fields {
                        for (field_name, field_pattern) in fields {
                            let field_ty = struct_fields
                                .iter()
                                .find(|(n, _)| n == field_name)
                                .map(|(_, t)| t.clone())
                                .unwrap_or_else(|| {
                                    self.errors.push(TypeError::NoSuchField {
                                        ty: Type::Named(type_id),
                                        field: field_name.clone(),
                                        span,
                                    });
                                    Type::Error
                                });
                            bindings.extend(self.check_pattern(field_pattern, &field_ty, span));
                        }
                    }
                    bindings
                } else {
                    let mut bindings = vec![];
                    for (_, field_pattern) in fields {
                        let fresh = self.ctx.fresh_var();
                        bindings.extend(self.check_pattern(field_pattern, &fresh, span));
                    }
                    bindings
                }
            }

            Pattern::Tuple(patterns) => {
                let elem_types: Vec<_> = patterns.iter().map(|_| self.ctx.fresh_var()).collect();
                self.ctx.add_constraint(TypeConstraint::Equal(
                    scrutinee_ty.clone(),
                    Type::Tuple(elem_types.clone()),
                    span,
                ));
                let mut bindings = vec![];
                for (pat, elem_ty) in patterns.iter().zip(elem_types.iter()) {
                    bindings.extend(self.check_pattern(pat, elem_ty, span));
                }
                bindings
            }

            Pattern::Or(alternatives) => {
                if let Some(first) = alternatives.first() {
                    let bindings = self.check_pattern(first, scrutinee_ty, span);
                    for alt in &alternatives[1..] {
                        let _alt_bindings = self.check_pattern(alt, scrutinee_ty, span);
                        // TODO: verify same names and compatible types
                    }
                    bindings
                } else {
                    vec![]
                }
            }
        }
    }

    fn check_constructor_pattern(
        &mut self,
        name: &str,
        fields: &[Pattern],
        scrutinee_ty: &Type,
        span: Span,
    ) -> Vec<(String, Type)> {
        let resolved_scrutinee = self.ctx.apply(scrutinee_ty);

        match name {
            "Ok" => {
                match &resolved_scrutinee {
                    Type::Result { ok, .. } => {
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], ok, span);
                        }
                    }
                    Type::Var(_) => {
                        let ok_ty = self.ctx.fresh_var();
                        let err_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Result {
                                ok: Box::new(ok_ty.clone()),
                                err: Box::new(err_ty),
                            },
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &ok_ty, span);
                        }
                    }
                    _ => {}
                }
            }
            "Err" => {
                match &resolved_scrutinee {
                    Type::Result { err, .. } => {
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], err, span);
                        }
                    }
                    Type::Var(_) => {
                        let ok_ty = self.ctx.fresh_var();
                        let err_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Result {
                                ok: Box::new(ok_ty),
                                err: Box::new(err_ty.clone()),
                            },
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &err_ty, span);
                        }
                    }
                    _ => {}
                }
            }
            "Some" => {
                match &resolved_scrutinee {
                    Type::Option(inner) => {
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], inner, span);
                        }
                    }
                    Type::Var(_) => {
                        let inner_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Option(Box::new(inner_ty.clone())),
                            span,
                        ));
                        if fields.len() == 1 {
                            return self.check_pattern(&fields[0], &inner_ty, span);
                        }
                    }
                    _ => {}
                }
            }
            "None" => {
                if fields.is_empty() {
                    if !matches!(&resolved_scrutinee, Type::Option(_) | Type::Var(_)) {
                        let inner_ty = self.ctx.fresh_var();
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            scrutinee_ty.clone(),
                            Type::Option(Box::new(inner_ty)),
                            span,
                        ));
                    }
                    return vec![];
                }
            }
            _ => {}
        }

        match &resolved_scrutinee {
            Type::Named(type_id) => {
                let variant_fields = self.types.get(*type_id).and_then(|def| {
                    if let TypeDef::Enum { variants, .. } = def {
                        variants.iter()
                            .find(|(n, _)| n == name)
                            .map(|(_, f)| f.clone())
                    } else {
                        None
                    }
                });

                if let Some(variant_field_types) = variant_fields {
                    if fields.len() != variant_field_types.len() {
                        self.errors.push(TypeError::ArityMismatch {
                            expected: variant_field_types.len(),
                            found: fields.len(),
                            span,
                        });
                        return vec![];
                    }
                    let mut bindings = vec![];
                    for (pat, field_ty) in fields.iter().zip(variant_field_types.iter()) {
                        bindings.extend(self.check_pattern(pat, field_ty, span));
                    }
                    return bindings;
                }
            }
            _ => {}
        }

        let mut bindings = vec![];
        for pat in fields {
            let fresh = self.ctx.fresh_var();
            bindings.extend(self.check_pattern(pat, &fresh, span));
        }
        bindings
    }

    fn check_fn(&mut self, f: &FnDecl, fn_span: Span) {
        let ret_ty = f
            .ret_ty
            .as_ref()
            .map(|t| parse_type_string(t, &self.types).unwrap_or(Type::Error))
            .unwrap_or(Type::Unit);
        self.current_return_type = Some(ret_ty);

        self.push_scope();
        for param in &f.params {
            if param.name == "self" {
                if let Some(self_ty) = &self.current_self_type {
                    self.define_local("self".to_string(), self_ty.clone());
                }
                continue;
            }
            if let Ok(ty) = parse_type_string(&param.ty, &self.types) {
                if param.is_read {
                    self.define_local_read_only(param.name.clone(), ty);
                } else {
                    self.define_local(param.name.clone(), ty);
                }
            }
        }

        for stmt in &f.body {
            self.check_stmt(stmt);
        }

        let ret_ty = self.current_return_type.as_ref().unwrap();
        let resolved_ret_ty = self.ctx.apply(ret_ty);

        match &resolved_ret_ty {
            Type::Unit | Type::Never => {
                // No return needed
            }
            Type::Result { ok, err: _ } => {
                let resolved_ok = self.ctx.apply(ok);
                if matches!(resolved_ok, Type::Unit) {
                    // Function is () or E - implicit Ok(()) is valid
                } else {
                    // Function is T or E where T != () - require explicit return
                    if !self.has_explicit_return(&f.body) {
                        let end_span = Span::new(fn_span.end - 1, fn_span.end);
                        self.errors.push(TypeError::MissingReturn {
                            function_name: f.name.clone(),
                            expected_type: ret_ty.clone(),
                            span: end_span,
                        });
                    }
                }
            }
            _ => {
                // Non-Result, non-Unit - require explicit return
                if !self.has_explicit_return(&f.body) {
                    let end_span = Span::new(fn_span.end - 1, fn_span.end);
                    self.errors.push(TypeError::MissingReturn {
                        function_name: f.name.clone(),
                        expected_type: ret_ty.clone(),
                        span: end_span,
                    });
                }
            }
        }

        self.pop_scope();
        self.current_return_type = None;
    }

    fn has_explicit_return(&self, body: &[Stmt]) -> bool {
        // Any statement in the body that always returns means the function returns
        body.iter().any(|stmt| self.stmt_always_returns(stmt))
    }

    fn stmt_always_returns(&self, stmt: &Stmt) -> bool {
        use rask_ast::stmt::StmtKind;

        match &stmt.kind {
            StmtKind::Return(_) => true,
            StmtKind::Expr(expr) => self.expr_always_returns(expr),
            _ => false,
        }
    }

    fn expr_always_returns(&self, expr: &rask_ast::expr::Expr) -> bool {
        use rask_ast::expr::ExprKind;

        match &expr.kind {
            ExprKind::Block(stmts) | ExprKind::Unsafe { body: stmts } => {
                stmts.iter().any(|s| self.stmt_always_returns(s))
            }
            ExprKind::Match { arms, .. } => {
                !arms.is_empty() && arms.iter().all(|arm| self.expr_always_returns(&arm.body))
            }
            ExprKind::If { then_branch, else_branch, .. } => {
                else_branch.as_ref().map_or(false, |else_br| {
                    self.expr_always_returns(then_branch) && self.expr_always_returns(else_br)
                })
            }
            ExprKind::IfLet { then_branch, else_branch, .. } => {
                else_branch.as_ref().map_or(false, |else_br| {
                    self.expr_always_returns(then_branch) && self.expr_always_returns(else_br)
                })
            }
            _ => false,
        }
    }

    /// Auto-wrap return value in Ok() if function returns Result.
    /// Implements auto-Ok wrapping from spec: when function returns T or E,
    /// returning just T auto-wraps to Ok(T).
    fn wrap_in_ok_if_needed(&self, ret_ty: Type, expected: &Type) -> Type {
        let resolved_expected = self.ctx.apply(expected);

        // Check if expected type is Result
        if let Type::Result { ok: _, err } = &resolved_expected {
            let resolved_ret = self.ctx.apply(&ret_ty);

            // Don't wrap if already Result (handles explicit Ok/Err returns)
            match &resolved_ret {
                Type::Result { .. } => ret_ty,
                // Don't wrap type variables - preserve inference flexibility
                Type::Var(_) => ret_ty,
                // Wrap concrete non-Result values
                _ => Type::Result {
                    ok: Box::new(ret_ty),
                    err: err.clone(),
                },
            }
        } else {
            // Not a Result return type - no wrapping
            ret_ty
        }
    }

    // ------------------------------------------------------------------------
    // Statement Checking
    // ------------------------------------------------------------------------

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.infer_expr(expr);
                // ESAD Phase 1: Clear borrows at statement end (semicolon)
                self.clear_expression_borrows();
            }
            StmtKind::Let { name, ty, init } => {
                let init_ty = self.infer_expr(init);
                if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        self.ctx
                            .add_constraint(TypeConstraint::Equal(declared.clone(), init_ty, stmt.span));
                        self.define_local(name.clone(), declared);
                    } else {
                        self.define_local(name.clone(), init_ty);
                    }
                } else {
                    self.define_local(name.clone(), init_ty);
                }
                self.clear_expression_borrows();
            }
            StmtKind::Const { name, ty, init } => {
                let init_ty = self.infer_expr(init);
                if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        self.ctx
                            .add_constraint(TypeConstraint::Equal(declared.clone(), init_ty, stmt.span));
                        self.define_local(name.clone(), declared);
                    } else {
                        self.define_local(name.clone(), init_ty);
                    }
                } else {
                    self.define_local(name.clone(), init_ty);
                }
                self.clear_expression_borrows();
            }
            StmtKind::Assign { target, value } => {
                // Reject mutation of read-only parameters
                if let Some(root) = Self::root_ident_name(target) {
                    if self.is_local_read_only(&root) {
                        self.errors.push(TypeError::MutateReadParam {
                            name: root,
                            span: stmt.span,
                        });
                    }
                }
                let target_ty = self.infer_expr(target);
                let value_ty = self.infer_expr(value);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    target_ty, value_ty, stmt.span,
                ));
                self.clear_expression_borrows();
            }
            StmtKind::Return(value) => {
                let ret_ty = if let Some(expr) = value {
                    self.infer_expr(expr)
                } else {
                    Type::Unit
                };
                if let Some(expected) = &self.current_return_type {
                    // Auto-wrap in Ok() if returning T where function expects T or E
                    let wrapped_ty = self.wrap_in_ok_if_needed(ret_ty, expected);

                    self.ctx.add_constraint(TypeConstraint::Equal(
                        expected.clone(),
                        wrapped_ty,
                        stmt.span,
                    ));
                }
                self.clear_expression_borrows();
            }
            StmtKind::While { cond, body, .. } => {
                let cond_ty = self.infer_expr(cond);
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, cond_ty, stmt.span));
                self.push_scope();
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
            StmtKind::For { binding, iter, body, .. } => {
                let iter_ty = self.infer_expr(iter);
                self.push_scope();
                let elem_ty = match &iter_ty {
                    Type::Array { elem, .. } | Type::Slice(elem) => *elem.clone(),
                    _ => self.ctx.fresh_var(),
                };
                self.define_local(binding.clone(), elem_ty);
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
            StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Deliver { .. } => {}
            StmtKind::Ensure { body, catch } => {
                for s in body {
                    self.check_stmt(s);
                }
                if let Some((_name, handler)) = catch {
                    for s in handler {
                        self.check_stmt(s);
                    }
                }
            }
            StmtKind::Comptime(body) => {
                for s in body {
                    self.check_stmt(s);
                }
            }
            StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
                self.infer_expr(init);
            }
            StmtKind::WhileLet { pattern, expr, body } => {
                let value_ty = self.infer_expr(expr);
                self.push_scope();
                let bindings = self.check_pattern(pattern, &value_ty, stmt.span);
                for (name, ty) in bindings {
                    self.define_local(name, ty);
                }
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
            StmtKind::Loop { body, .. } => {
                self.push_scope();
                for s in body {
                    self.check_stmt(s);
                }
                self.pop_scope();
            }
        }
    }

    // ------------------------------------------------------------------------
    // Expression Inference
    // ------------------------------------------------------------------------

    fn infer_expr(&mut self, expr: &Expr) -> Type {
        let ty = match &expr.kind {
            // Literals
            ExprKind::Int(_, suffix) => {
                use rask_ast::token::IntSuffix;
                match suffix {
                    Some(IntSuffix::I8) => Type::I8,
                    Some(IntSuffix::I16) => Type::I16,
                    Some(IntSuffix::I32) => Type::I32,
                    Some(IntSuffix::I64) => Type::I64,
                    Some(IntSuffix::I128) => Type::I128,
                    Some(IntSuffix::Isize) => Type::I64,
                    Some(IntSuffix::U8) => Type::U8,
                    Some(IntSuffix::U16) => Type::U16,
                    Some(IntSuffix::U32) => Type::U32,
                    Some(IntSuffix::U64) => Type::U64,
                    Some(IntSuffix::U128) => Type::U128,
                    Some(IntSuffix::Usize) => Type::U64,
                    None => self.ctx.fresh_var(), // Infer from context, defaults to i32
                }
            }
            ExprKind::Float(_, suffix) => {
                use rask_ast::token::FloatSuffix;
                match suffix {
                    Some(FloatSuffix::F32) => Type::F32,
                    Some(FloatSuffix::F64) => Type::F64,
                    None => self.ctx.fresh_var(), // Infer from context, defaults to f64
                }
            }
            ExprKind::String(_) => Type::String,
            ExprKind::Char(_) => Type::Char,
            ExprKind::Bool(_) => Type::Bool,

            ExprKind::Ident(name) => {
                if let Some(ty) = self.lookup_local(name) {
                    ty
                } else if let Some(&sym_id) = self.resolved.resolutions.get(&expr.id) {
                    self.get_symbol_type(sym_id)
                } else {
                    Type::Error
                }
            }

            ExprKind::Binary { op, left, right } => {
                self.check_binary(*op, left, right, expr.span)
            }

            ExprKind::Unary { op: _, operand } => {
                self.infer_expr(operand)
            }

            ExprKind::Call { func, args } => self.check_call(func, args, expr.span),

            ExprKind::MethodCall {
                object,
                method,
                args,
                ..
            } => self.check_method_call(object, method, args, expr.span),

            ExprKind::Field { object, field } => self.check_field_access(object, field, expr.span),

            ExprKind::Index { object, index } => {
                let obj_ty = self.infer_expr(object);
                let _idx_ty = self.infer_expr(index);
                match &obj_ty {
                    Type::Array { elem, .. } | Type::Slice(elem) => *elem.clone(),
                    Type::String => Type::Char,
                    _ => self.ctx.fresh_var(),
                }
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_ty = self.infer_expr(cond);
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, cond_ty, expr.span));

                let then_ty = self.infer_expr(then_branch);

                if let Some(else_branch) = else_branch {
                    let else_ty = self.infer_expr(else_branch);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        then_ty.clone(),
                        else_ty,
                        expr.span,
                    ));
                    then_ty
                } else {
                    Type::Unit
                }
            }

            ExprKind::IfLet {
                pattern,
                then_branch,
                else_branch,
                expr: value,
            } => {
                let value_ty = self.infer_expr(value);
                self.push_scope();
                let bindings = self.check_pattern(pattern, &value_ty, expr.span);
                for (name, ty) in bindings {
                    self.define_local(name, ty);
                }
                let then_ty = self.infer_expr(then_branch);
                self.pop_scope();
                if let Some(else_branch) = else_branch {
                    let else_ty = self.infer_expr(else_branch);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        then_ty.clone(),
                        else_ty,
                        expr.span,
                    ));
                }
                then_ty
            }

            ExprKind::Match { scrutinee, arms } => {
                let scrutinee_ty = self.infer_expr(scrutinee);
                let result_ty = self.ctx.fresh_var();
                for arm in arms {
                    self.push_scope();
                    let bindings = self.check_pattern(&arm.pattern, &scrutinee_ty, expr.span);
                    for (name, ty) in bindings {
                        self.define_local(name, ty);
                    }
                    if let Some(guard) = &arm.guard {
                        let guard_ty = self.infer_expr(guard);
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            Type::Bool,
                            guard_ty,
                            expr.span,
                        ));
                    }
                    let arm_ty = self.infer_expr(&arm.body);
                    self.pop_scope();
                    let resolved_arm_ty = self.ctx.apply(&arm_ty);
                    if !matches!(resolved_arm_ty, Type::Never) {
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            result_ty.clone(),
                            arm_ty,
                            expr.span,
                        ));
                    }
                }
                result_ty
            }

            ExprKind::Block(stmts) => {
                for stmt in stmts {
                    self.check_stmt(stmt);
                }
                if let Some(last) = stmts.last() {
                    match &last.kind {
                        StmtKind::Expr(e) => return self.infer_expr(e),
                        StmtKind::Return(_) | StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Deliver { .. } => {
                            return Type::Never
                        }
                        _ => {}
                    }
                }
                Type::Unit
            }

            ExprKind::StructLit { name, fields, .. } => {
                if let Some(ty) = self.types.lookup(name) {
                    if let Type::Named(type_id) = &ty {
                        let (struct_fields, type_params) = match self.types.get(*type_id) {
                            Some(TypeDef::Struct { fields: sf, type_params: tp, .. }) => {
                                (sf.clone(), tp.clone())
                            }
                            _ => (vec![], vec![]),
                        };

                        if type_params.is_empty() {
                            // Non-generic struct: constrain directly
                            for field_init in fields {
                                let field_ty = self.infer_expr(&field_init.value);
                                if let Some((_, expected)) =
                                    struct_fields.iter().find(|(n, _)| n == &field_init.name)
                                {
                                    self.ctx.add_constraint(TypeConstraint::Equal(
                                        expected.clone(),
                                        field_ty,
                                        expr.span,
                                    ));
                                }
                            }
                            ty
                        } else {
                            // Generic struct: create fresh vars, substitute into fields
                            let fresh_args: Vec<GenericArg> = type_params.iter()
                                .map(|_| GenericArg::Type(Box::new(self.ctx.fresh_var())))
                                .collect();
                            let subst = Self::build_type_param_subst(&type_params, &fresh_args);

                            for field_init in fields {
                                let field_ty = self.infer_expr(&field_init.value);
                                if let Some((_, expected)) =
                                    struct_fields.iter().find(|(n, _)| n == &field_init.name)
                                {
                                    let substituted = Self::substitute_type_params(expected, &subst);
                                    self.ctx.add_constraint(TypeConstraint::Equal(
                                        substituted,
                                        field_ty,
                                        expr.span,
                                    ));
                                }
                            }

                            Type::Generic { base: *type_id, args: fresh_args }
                        }
                    } else {
                        ty
                    }
                } else {
                    Type::UnresolvedNamed(name.clone())
                }
            }

            ExprKind::Array(elements) => {
                if elements.is_empty() {
                    let elem_ty = self.ctx.fresh_var();
                    Type::Array {
                        elem: Box::new(elem_ty),
                        len: 0,
                    }
                } else {
                    let first_ty = self.infer_expr(&elements[0]);
                    for elem in &elements[1..] {
                        let elem_ty = self.infer_expr(elem);
                        self.ctx.add_constraint(TypeConstraint::Equal(
                            first_ty.clone(),
                            elem_ty,
                            expr.span,
                        ));
                    }
                    Type::Array {
                        elem: Box::new(first_ty),
                        len: elements.len(),
                    }
                }
            }

            ExprKind::Tuple(elements) => {
                let elem_types: Vec<_> = elements.iter().map(|e| self.infer_expr(e)).collect();
                // Empty tuple () is Unit type
                if elem_types.is_empty() {
                    Type::Unit
                } else {
                    Type::Tuple(elem_types)
                }
            }

            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.infer_expr(s);
                }
                if let Some(e) = end {
                    self.infer_expr(e);
                }
                Type::UnresolvedNamed("Range".to_string())
            }

            ExprKind::Try(inner) => {
                let inner_ty = self.infer_expr(inner);
                let resolved = self.ctx.apply(&inner_ty);
                match &resolved {
                    Type::Option(inner) => {
                        // For Option, just return the inner type
                        // The function return type should also be Option (checked elsewhere)
                        *inner.clone()
                    }
                    Type::Result { ok, err } => {
                        // For Result, extract the ok type and ensure error types match
                        if let Some(return_ty) = &self.current_return_type {
                            let resolved_ret = self.ctx.apply(return_ty);
                            if let Type::Result { err: ret_err, .. } = &resolved_ret {
                                // Unify the Result's error type with the function's error type
                                let _ = self.unify(err, ret_err, expr.span);
                            }
                        }
                        *ok.clone()
                    }
                    Type::Var(_) => {
                        if let Some(return_ty) = &self.current_return_type {
                            let resolved_ret = self.ctx.apply(return_ty);
                            match &resolved_ret {
                                Type::Option(_) => {
                                    let inner_opt_ty = self.ctx.fresh_var();
                                    let option_ty = Type::Option(Box::new(inner_opt_ty.clone()));
                                    let _ = self.unify(&inner_ty, &option_ty, expr.span);
                                    inner_opt_ty
                                }
                                Type::Result { .. } => {
                                    let ok_ty = self.ctx.fresh_var();
                                    let err_ty = self.ctx.fresh_var();
                                    let result_ty = Type::Result {
                                        ok: Box::new(ok_ty.clone()),
                                        err: Box::new(err_ty),
                                    };
                                    let _ = self.unify(&inner_ty, &result_ty, expr.span);
                                    ok_ty
                                }
                                Type::Var(_) => {
                                    self.ctx.fresh_var()
                                }
                                _ => {
                                    self.errors.push(TypeError::TryInNonPropagatingContext {
                                        return_ty: resolved_ret.clone(),
                                        span: expr.span,
                                    });
                                    Type::Error
                                }
                            }
                        } else {
                            self.errors.push(TypeError::TryOutsideFunction { span: expr.span });
                            Type::Error
                        }
                    }
                    _ => {
                        self.errors.push(TypeError::Mismatch {
                            expected: Type::Result {
                                ok: Box::new(self.ctx.fresh_var()),
                                err: Box::new(self.ctx.fresh_var()),
                            },
                            found: resolved,
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            ExprKind::Closure { params, body, .. } => {
                let param_types: Vec<_> = params
                    .iter()
                    .map(|p| {
                        p.ty.as_ref()
                            .and_then(|t| parse_type_string(t, &self.types).ok())
                            .unwrap_or_else(|| self.ctx.fresh_var())
                    })
                    .collect();

                // ESAD Phase 2: Check for aliasing violations in closure body
                self.check_closure_aliasing(params, body);

                let ret_ty = self.infer_expr(body);
                Type::Fn {
                    params: param_types,
                    ret: Box::new(ret_ty),
                }
            }

            ExprKind::Cast { expr: inner, ty } => {
                self.infer_expr(inner);
                parse_type_string(ty, &self.types).unwrap_or(Type::Error)
            }

            ExprKind::Unsafe { body } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                if let Some(last) = body.last() {
                    if let StmtKind::Expr(e) = &last.kind {
                        return self.infer_expr(e);
                    }
                }
                Type::Unit
            }

            ExprKind::Comptime { body } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                if let Some(last) = body.last() {
                    if let StmtKind::Expr(e) = &last.kind {
                        return self.infer_expr(e);
                    }
                }
                Type::Unit
            }

            ExprKind::Spawn { body } => {
                // Spawn blocks are like anonymous functions - they have their own return type
                let outer_return_type = self.current_return_type.take();
                let spawn_return_type = self.ctx.fresh_var();
                self.current_return_type = Some(spawn_return_type.clone());

                // Check all statements except the last (which we infer separately)
                let last_idx = body.len().saturating_sub(1);
                for (i, stmt) in body.iter().enumerate() {
                    if i < last_idx {
                        self.check_stmt(stmt);
                    }
                }

                // Infer the return type from the last statement (only process once)
                let inner_type = if let Some(last) = body.last() {
                    match &last.kind {
                        StmtKind::Expr(e) => self.infer_expr(e),
                        StmtKind::Return(_) => {
                            self.check_stmt(last);
                            Type::Never
                        }
                        _ => {
                            self.check_stmt(last);
                            Type::Unit
                        }
                    }
                } else {
                    Type::Unit
                };

                self.ctx.add_constraint(TypeConstraint::Equal(
                    spawn_return_type.clone(),
                    inner_type,
                    expr.span,
                ));

                self.current_return_type = outer_return_type;

                Type::UnresolvedGeneric {
                    name: "ThreadHandle".to_string(),
                    args: vec![GenericArg::Type(Box::new(spawn_return_type))],
                }
            }

            ExprKind::WithBlock { args, body, .. } => {
                for arg in args {
                    self.infer_expr(arg);
                }
                for stmt in body {
                    self.check_stmt(stmt);
                }
                Type::Unit
            }

            ExprKind::WithAs { bindings, body } => {
                self.push_scope();
                for (source_expr, binding_name) in bindings {
                    let elem_ty = self.infer_expr(source_expr);
                    self.define_local(binding_name.clone(), elem_ty);
                }
                for stmt in body {
                    self.check_stmt(stmt);
                }
                self.pop_scope();
                Type::Unit
            }

            ExprKind::BlockCall { body, .. } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                Type::Unit
            }

            ExprKind::ArrayRepeat { value, count } => {
                let elem_ty = self.infer_expr(value);
                self.infer_expr(count);
                Type::Array {
                    elem: Box::new(elem_ty),
                    len: 0,
                }
            }

            ExprKind::NullCoalesce { value, default } => {
                let val_ty = self.infer_expr(value);
                let def_ty = self.infer_expr(default);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    val_ty,
                    Type::Option(Box::new(def_ty.clone())),
                    expr.span,
                ));
                def_ty
            }

            ExprKind::OptionalField { object, field } => {
                let obj_ty = self.infer_expr(object);
                let field_ty = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::HasField {
                    ty: obj_ty,
                    field: field.clone(),
                    expected: field_ty.clone(),
                    span: expr.span,
                });
                Type::Option(Box::new(field_ty))
            }

            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                let cond_ty = self.infer_expr(condition);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    cond_ty,
                    Type::Bool,
                    condition.span,
                ));
                if let Some(msg) = message {
                    let msg_ty = self.infer_expr(msg);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        msg_ty,
                        Type::String,
                        msg.span,
                    ));
                }
                Type::Unit
            }
        };

        self.node_types.insert(expr.id, ty.clone());
        ty
    }

    // ------------------------------------------------------------------------
    // Specific Type Checks
    // ------------------------------------------------------------------------

    fn check_binary(&mut self, op: BinOp, left: &Expr, right: &Expr, span: Span) -> Type {
        let left_ty = self.infer_expr(left);
        let right_ty = self.infer_expr(right);

        self.ctx.add_constraint(TypeConstraint::Equal(
            left_ty.clone(),
            right_ty.clone(),
            span,
        ));

        match op {
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => Type::Bool,
            BinOp::And | BinOp::Or => {
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, left_ty, span));
                Type::Bool
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => left_ty,
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => left_ty,
        }
    }

    fn check_call(&mut self, func: &Expr, args: &[Expr], span: Span) -> Type {
        if let ExprKind::Ident(name) = &func.kind {
            if self.is_builtin_function(name) {
                for arg in args {
                    self.infer_expr(arg);
                }
                return match name.as_str() {
                    "panic" => Type::Never,
                    "format" => Type::String,
                    _ => Type::Unit,
                };
            }
        }

        let func_ty = self.infer_expr(func);
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(a)).collect();

        match func_ty {
            Type::Fn { params, ret } => {
                if params.is_empty() && !arg_types.is_empty() {
                    return *ret;
                }

                if params.len() != arg_types.len() {
                    self.errors.push(TypeError::ArityMismatch {
                        expected: params.len(),
                        found: arg_types.len(),
                        span,
                    });
                    return Type::Error;
                }

                for (param, arg) in params.iter().zip(arg_types.iter()) {
                    self.ctx
                        .add_constraint(TypeConstraint::Equal(param.clone(), arg.clone(), span));
                }

                *ret
            }
            Type::Var(_) => {
                let ret = self.ctx.fresh_var();
                self.ctx.add_constraint(TypeConstraint::Equal(
                    func_ty,
                    Type::Fn {
                        params: arg_types,
                        ret: Box::new(ret.clone()),
                    },
                    span,
                ));
                ret
            }
            Type::Error => Type::Error,
            _ => {
                self.ctx.fresh_var()
            }
        }
    }

    fn is_builtin_function(&self, name: &str) -> bool {
        matches!(name, "println" | "print" | "panic" | "assert" | "debug" | "format")
    }

    fn check_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Type {
        // Check if this is a builtin module method call (e.g., fs.open)
        if let ExprKind::Ident(name) = &object.kind {
            if self.types.builtin_modules.is_module(name) {
                return self.check_module_method(name, method, args, span);
            }
        }

        // ESAD Phase 1: Push borrow for the object being called
        // For now, conservatively assume all methods on collections create exclusive borrows
        // TODO: Refine this by checking method signatures for `read self` vs `self`
        if let ExprKind::Ident(var_name) = &object.kind {
            // Determine borrow mode based on method name (conservative heuristic)
            let mode = if method.starts_with("get") || method == "read" || method == "len" {
                BorrowMode::Shared
            } else {
                BorrowMode::Exclusive
            };
            self.push_borrow(var_name.clone(), mode, object.span);
        }

        let obj_ty_raw = self.infer_expr(object);
        let obj_ty = self.resolve_named(&obj_ty_raw);
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(a)).collect();

        let ret_ty = self.ctx.fresh_var();

        self.ctx.add_constraint(TypeConstraint::HasMethod {
            ty: obj_ty,
            method: method.to_string(),
            args: arg_types,
            ret: ret_ty.clone(),
            span,
        });

        ret_ty
    }

    fn check_module_method(
        &mut self,
        module: &str,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Type {
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(a)).collect();

        if let Some(sig) = self.types.builtin_modules.get_method(module, method) {
            // Check parameter count — skip for wildcard params (_Any accepts anything)
            let has_wildcard = sig.params.iter().any(|p| {
                matches!(p, Type::UnresolvedNamed(n) if n == "_Any")
            });
            if !has_wildcard && sig.params.len() != arg_types.len() {
                self.errors.push(TypeError::ArityMismatch {
                    expected: sig.params.len(),
                    found: arg_types.len(),
                    span,
                });
                return Type::Error;
            }

            // Check parameter types (skip _Any wildcards)
            if !has_wildcard {
                for (param_ty, arg_ty) in sig.params.iter().zip(arg_types.iter()) {
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        param_ty.clone(),
                        arg_ty.clone(),
                        span,
                    ));
                }
            }

            // Replace placeholder types with fresh vars for generic module methods
            self.freshen_module_return_type(&sig.ret.clone())
        } else {
            self.errors.push(TypeError::NoSuchMethod {
                ty: Type::UnresolvedNamed(module.to_string()),
                method: method.to_string(),
                span,
            });
            Type::Error
        }
    }

    /// Replace internal placeholder types (_JsonDecodeResult, _Any) with fresh type vars.
    fn freshen_module_return_type(&mut self, ty: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(n) if n.starts_with('_') => self.ctx.fresh_var(),
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.freshen_module_return_type(ok)),
                err: Box::new(self.freshen_module_return_type(err)),
            },
            Type::Option(inner) => Type::Option(Box::new(self.freshen_module_return_type(inner))),
            _ => ty.clone(),
        }
    }

    fn check_field_access(&mut self, object: &Expr, field: &str, span: Span) -> Type {
        let obj_ty_raw = self.infer_expr(object);
        let obj_ty = self.resolve_named(&obj_ty_raw);
        let field_ty = self.ctx.fresh_var();

        self.ctx.add_constraint(TypeConstraint::HasField {
            ty: obj_ty,
            field: field.to_string(),
            expected: field_ty.clone(),
            span,
        });

        field_ty
    }

    fn get_symbol_type(&mut self, sym_id: SymbolId) -> Type {
        if let Some(ty) = self.symbol_types.get(&sym_id) {
            return ty.clone();
        }

        if let Some(sym) = self.resolved.symbols.get(sym_id) {
            match &sym.kind {
                SymbolKind::Function { ret_ty, params, .. } => {
                    let param_types: Vec<_> = params
                        .iter()
                        .filter_map(|pid| {
                            self.resolved.symbols.get(*pid).and_then(|p| {
                                p.ty.as_ref()
                                    .and_then(|t| parse_type_string(t, &self.types).ok())
                            })
                        })
                        .collect();
                    let ret = ret_ty
                        .as_ref()
                        .and_then(|t| parse_type_string(t, &self.types).ok())
                        .unwrap_or(Type::Unit);
                    return Type::Fn {
                        params: param_types,
                        ret: Box::new(ret),
                    };
                }
                SymbolKind::Variable { .. } | SymbolKind::Parameter { .. } => {
                    if let Some(ty_str) = &sym.ty {
                        if let Ok(ty) = parse_type_string(ty_str, &self.types) {
                            return ty;
                        }
                    }
                }
                SymbolKind::Struct { .. } => {
                    if let Some(type_id) = self.types.get_type_id(&sym.name) {
                        return Type::Named(type_id);
                    }
                }
                SymbolKind::Enum { .. } => {
                    if let Some(type_id) = self.types.get_type_id(&sym.name) {
                        return Type::Named(type_id);
                    }
                }
                SymbolKind::EnumVariant { enum_id } => {
                    if let Some(enum_sym) = self.resolved.symbols.get(*enum_id) {
                        let type_id = if enum_sym.span == Span::new(0, 0) {
                            match enum_sym.name.as_str() {
                                "Result" => self.types.get_result_type_id(),
                                "Option" => self.types.get_option_type_id(),
                                _ => None,
                            }
                        } else {
                            self.types.get_type_id(&enum_sym.name)
                        };

                        if let Some(id) = type_id {
                            let variant_fields = self.types.get(id).and_then(|def| {
                                if let TypeDef::Enum { variants, .. } = def {
                                    variants.iter()
                                        .find(|(n, _)| n == &sym.name)
                                        .map(|(_, fields)| fields.clone())
                                } else {
                                    None
                                }
                            });

                            if let Some(fields) = variant_fields {
                                if fields.is_empty() {
                                    return Type::Named(id);
                                } else {
                                    let (param_types, ret_type) = if Some(id) == self.types.get_result_type_id() {
                                        let t_var = self.ctx.fresh_var();
                                        let e_var = self.ctx.fresh_var();
                                        let params = match sym.name.as_str() {
                                            "Ok" => vec![t_var.clone()],
                                            "Err" => vec![e_var.clone()],
                                            _ => fields.clone(),
                                        };
                                        let ret = Type::Result {
                                            ok: Box::new(t_var),
                                            err: Box::new(e_var),
                                        };
                                        (params, ret)
                                    } else if Some(id) == self.types.get_option_type_id() {
                                        let t_var = self.ctx.fresh_var();
                                        let params = if sym.name == "Some" {
                                            vec![t_var.clone()]
                                        } else {
                                            vec![]
                                        };
                                        let ret = Type::Option(Box::new(t_var));
                                        (params, ret)
                                    } else {
                                        let instantiated = self.instantiate_type_vars(&fields);
                                        (instantiated, Type::Named(id))
                                    };

                                    return Type::Fn {
                                        params: param_types,
                                        ret: Box::new(ret_type),
                                    };
                                }
                            } else {
                                return Type::Named(id);
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let var = self.ctx.fresh_var();
        self.symbol_types.insert(sym_id, var.clone());
        var
    }

    // ------------------------------------------------------------------------
    // Constraint Solving
    // ------------------------------------------------------------------------

    fn solve_constraints(&mut self) {
        let mut changed = true;
        let mut iterations = 0;
        const MAX_ITERATIONS: usize = 100;

        while changed && iterations < MAX_ITERATIONS {
            changed = false;
            iterations += 1;

            let constraints = std::mem::take(&mut self.ctx.constraints);
            for constraint in constraints {
                match self.solve_constraint(constraint) {
                    Ok(true) => changed = true,
                    Ok(false) => {}
                    Err(e) => self.errors.push(e),
                }
            }
        }
    }

    fn solve_constraint(&mut self, constraint: TypeConstraint) -> Result<bool, TypeError> {
        match constraint {
            TypeConstraint::Equal(t1, t2, span) => self.unify(&t1, &t2, span),
            TypeConstraint::HasField {
                ty,
                field,
                expected,
                span,
            } => self.resolve_field(ty, field, expected, span),
            TypeConstraint::HasMethod {
                ty,
                method,
                args,
                ret,
                span,
            } => self.resolve_method(ty, method, args, ret, span),
        }
    }

    fn unify(&mut self, t1: &Type, t2: &Type, span: Span) -> Result<bool, TypeError> {
        let t1 = self.ctx.apply(t1);
        let t2 = self.ctx.apply(t2);

        match (&t1, &t2) {
            (a, b) if a == b => Ok(false),

            // Empty tuple and Unit are equivalent
            (Type::Tuple(elems), Type::Unit) | (Type::Unit, Type::Tuple(elems))
                if elems.is_empty() =>
            {
                Ok(false)
            }

            (Type::Var(id), other) => {
                if self.ctx.occurs_in(*id, other) {
                    return Err(TypeError::InfiniteType {
                        var: *id,
                        ty: other.clone(),
                        span,
                    });
                }
                self.ctx.substitutions.insert(*id, other.clone());
                Ok(true)
            }

            (other, Type::Var(id)) => {
                if self.ctx.occurs_in(*id, other) {
                    return Err(TypeError::InfiniteType {
                        var: *id,
                        ty: other.clone(),
                        span,
                    });
                }
                self.ctx.substitutions.insert(*id, other.clone());
                Ok(true)
            }

            (Type::Generic { base: b1, args: a1 }, Type::Generic { base: b2, args: a2 }) => {
                if b1 != b2 || a1.len() != a2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (arg1, arg2) in a1.iter().zip(a2.iter()) {
                    if self.unify_generic_arg(arg1, arg2, span)? {
                        progress = true;
                    }
                }
                Ok(progress)
            }

            // Function types
            (
                Type::Fn {
                    params: p1,
                    ret: r1,
                },
                Type::Fn {
                    params: p2,
                    ret: r2,
                },
            ) => {
                if p1.len() != p2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (param1, param2) in p1.iter().zip(p2.iter()) {
                    if self.unify(param1, param2, span)? {
                        progress = true;
                    }
                }
                if self.unify(r1, r2, span)? {
                    progress = true;
                }
                Ok(progress)
            }

            (Type::Tuple(e1), Type::Tuple(e2)) => {
                if e1.len() != e2.len() {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                let mut progress = false;
                for (elem1, elem2) in e1.iter().zip(e2.iter()) {
                    if self.unify(elem1, elem2, span)? {
                        progress = true;
                    }
                }
                Ok(progress)
            }

            (Type::Option(inner1), Type::Option(inner2)) => self.unify(inner1, inner2, span),

            (
                Type::Result { ok: o1, err: e1 },
                Type::Result { ok: o2, err: e2 },
            ) => {
                let p1 = self.unify(o1, o2, span)?;
                let p2 = self.unify(e1, e2, span)?;
                Ok(p1 || p2)
            }

            (
                Type::Array {
                    elem: e1,
                    len: l1,
                },
                Type::Array {
                    elem: e2,
                    len: l2,
                },
            ) => {
                if l1 != l2 {
                    return Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    });
                }
                self.unify(e1, e2, span)
            }

            (Type::Slice(e1), Type::Slice(e2)) => self.unify(e1, e2, span),

            (Type::Error, _) | (_, Type::Error) => Ok(false),

            (Type::Never, _) => Ok(false),
            (_, Type::Never) => Ok(false),

            (Type::Result { ok: _, err: _ }, Type::Named(id)) | (Type::Named(id), Type::Result { ok: _, err: _ }) => {
                if Some(*id) == self.types.get_result_type_id() {
                    Ok(false)
                } else {
                    Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    })
                }
            }

            (Type::Option(_inner), Type::Named(id)) | (Type::Named(id), Type::Option(_inner)) => {
                if Some(*id) == self.types.get_option_type_id() {
                    Ok(false)
                } else {
                    Err(TypeError::Mismatch {
                        expected: t1,
                        found: t2,
                        span,
                    })
                }
            }

            (Type::UnresolvedNamed(_), _) | (_, Type::UnresolvedNamed(_)) => {
                self.ctx
                    .add_constraint(TypeConstraint::Equal(t1, t2, span));
                Ok(false)
            }

            _ => Err(TypeError::Mismatch {
                expected: t1,
                found: t2,
                span,
            }),
        }
    }

    fn unify_generic_arg(&mut self, arg1: &GenericArg, arg2: &GenericArg, span: Span) -> Result<bool, TypeError> {
        match (arg1, arg2) {
            (GenericArg::Type(t1), GenericArg::Type(t2)) => self.unify(t1, t2, span),
            (GenericArg::ConstUsize(n1), GenericArg::ConstUsize(n2)) => {
                if n1 == n2 {
                    Ok(false)
                } else {
                    Err(TypeError::GenericError(
                        format!("const generic mismatch: {} vs {}", n1, n2),
                        span,
                    ))
                }
            }
            _ => Err(TypeError::Mismatch {
                expected: Type::Error,  // TODO: Better error representation
                found: Type::Error,
                span,
            }),
        }
    }

    fn resolve_named(&self, ty: &Type) -> Type {
        match ty {
            Type::UnresolvedNamed(name) => {
                if name == "Self" {
                    if let Some(self_ty) = &self.current_self_type {
                        return self_ty.clone();
                    }
                }
                if let Some(type_id) = self.types.get_type_id(name) {
                    return Type::Named(type_id);
                }
                ty.clone()
            }
            _ => ty.clone(),
        }
    }

    /// Replace type parameter names (UnresolvedNamed) with concrete types.
    fn substitute_type_params(ty: &Type, subst: &HashMap<&str, Type>) -> Type {
        match ty {
            Type::UnresolvedNamed(name) => {
                if let Some(replacement) = subst.get(name.as_str()) {
                    return replacement.clone();
                }
                ty.clone()
            }
            Type::Option(inner) => {
                Type::Option(Box::new(Self::substitute_type_params(inner, subst)))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(Self::substitute_type_params(ok, subst)),
                err: Box::new(Self::substitute_type_params(err, subst)),
            },
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(Self::substitute_type_params(elem, subst)),
                len: *len,
            },
            Type::Slice(elem) => {
                Type::Slice(Box::new(Self::substitute_type_params(elem, subst)))
            }
            Type::Tuple(elems) => {
                Type::Tuple(elems.iter().map(|e| Self::substitute_type_params(e, subst)).collect())
            }
            Type::Fn { params, ret } => Type::Fn {
                params: params.iter().map(|p| Self::substitute_type_params(p, subst)).collect(),
                ret: Box::new(Self::substitute_type_params(ret, subst)),
            },
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args.iter().map(|a| match a {
                    GenericArg::Type(t) => GenericArg::Type(Box::new(Self::substitute_type_params(t, subst))),
                    other => other.clone(),
                }).collect(),
            },
            Type::UnresolvedGeneric { name, args } => {
                // Check if whole name is a type param
                if let Some(replacement) = subst.get(name.as_str()) {
                    return replacement.clone();
                }
                Type::UnresolvedGeneric {
                    name: name.clone(),
                    args: args.iter().map(|a| match a {
                        GenericArg::Type(t) => GenericArg::Type(Box::new(Self::substitute_type_params(t, subst))),
                        other => other.clone(),
                    }).collect(),
                }
            }
            _ => ty.clone(),
        }
    }

    /// Build a substitution map from type param names to concrete types from generic args.
    fn build_type_param_subst<'a>(
        type_params: &'a [String],
        args: &[GenericArg],
    ) -> HashMap<&'a str, Type> {
        let mut subst = HashMap::new();
        for (param, arg) in type_params.iter().zip(args.iter()) {
            if let GenericArg::Type(ty) = arg {
                subst.insert(param.as_str(), *ty.clone());
            }
        }
        subst
    }

    fn resolve_field(
        &mut self,
        ty: Type,
        field: String,
        expected: Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let ty = self.resolve_named(&self.ctx.apply(&ty));

        match &ty {
            Type::Var(_) => {
                self.ctx.add_constraint(TypeConstraint::HasField {
                    ty,
                    field,
                    expected,
                    span,
                });
                Ok(false)
            }
            Type::Named(type_id) => {
                let result = self.types.get(*type_id).and_then(|def| {
                    match def {
                        TypeDef::Struct { fields, .. } => {
                            fields.iter().find(|(n, _)| n == &field).map(|(_, t)| t.clone())
                        }
                        TypeDef::Enum { variants, .. } => {
                            variants.iter().find(|(n, _)| n == &field).map(|(_, fields)| {
                                if fields.is_empty() {
                                    ty.clone()
                                } else {
                                    Type::Fn {
                                        params: fields.clone(),
                                        ret: Box::new(ty.clone()),
                                    }
                                }
                            })
                        }
                        _ => None,
                    }
                });

                if let Some(field_ty) = result {
                    self.unify(&expected, &field_ty, span)
                } else {
                    Err(TypeError::NoSuchField {
                        ty,
                        field,
                        span,
                    })
                }
            }
            Type::Tuple(elems) => {
                if let Ok(idx) = field.parse::<usize>() {
                    if idx < elems.len() {
                        self.unify(&expected, &elems[idx], span)
                    } else {
                        Err(TypeError::NoSuchField {
                            ty,
                            field,
                            span,
                        })
                    }
                } else {
                    Err(TypeError::NoSuchField {
                        ty,
                        field,
                        span,
                    })
                }
            }
            Type::Generic { base, args } => {
                let result = self.types.get(*base).and_then(|def| {
                    match def {
                        TypeDef::Struct { type_params, fields, .. } => {
                            let subst = Self::build_type_param_subst(type_params, args);
                            fields.iter().find(|(n, _)| n == &field).map(|(_, t)| {
                                Self::substitute_type_params(t, &subst)
                            })
                        }
                        TypeDef::Enum { type_params, variants, .. } => {
                            let subst = Self::build_type_param_subst(type_params, args);
                            variants.iter().find(|(n, _)| n == &field).map(|(_, fields)| {
                                if fields.is_empty() {
                                    ty.clone()
                                } else {
                                    Type::Fn {
                                        params: fields.iter()
                                            .map(|t| Self::substitute_type_params(t, &subst))
                                            .collect(),
                                        ret: Box::new(ty.clone()),
                                    }
                                }
                            })
                        }
                        _ => None,
                    }
                });

                if let Some(field_ty) = result {
                    self.unify(&expected, &field_ty, span)
                } else {
                    Err(TypeError::NoSuchField {
                        ty,
                        field,
                        span,
                    })
                }
            }
            // Builtin struct field resolution for runtime/stdlib types
            Type::UnresolvedNamed(name) => {
                let field_ty = match (name.as_str(), field.as_str()) {
                    // time module namespace
                    ("__module_time", "Instant") => Some(Type::UnresolvedNamed("Instant".to_string())),
                    ("__module_time", "Duration") => Some(Type::UnresolvedNamed("Duration".to_string())),
                    // HttpResponse struct fields
                    ("HttpResponse", "status") => Some(Type::I32),
                    ("HttpResponse", "headers") => Some(Type::UnresolvedGeneric {
                        name: "Map".to_string(),
                        args: vec![
                            GenericArg::Type(Box::new(Type::String)),
                            GenericArg::Type(Box::new(Type::String)),
                        ],
                    }),
                    ("HttpResponse", "body") => Some(Type::String),
                    // HttpRequest struct fields
                    ("HttpRequest", "method") => Some(Type::String),
                    ("HttpRequest", "path") => Some(Type::String),
                    ("HttpRequest", "body") => Some(Type::String),
                    ("HttpRequest", "headers") => Some(Type::UnresolvedGeneric {
                        name: "Map".to_string(),
                        args: vec![
                            GenericArg::Type(Box::new(Type::String)),
                            GenericArg::Type(Box::new(Type::String)),
                        ],
                    }),
                    _ => None,
                };
                if let Some(ft) = field_ty {
                    self.unify(&expected, &ft, span)
                } else {
                    Err(TypeError::NoSuchField { ty, field, span })
                }
            }
            _ => Err(TypeError::NoSuchField {
                ty,
                field,
                span,
            }),
        }
    }

    fn resolve_method(
        &mut self,
        ty: Type,
        method: String,
        args: Vec<Type>,
        ret: Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let ty = self.resolve_named(&self.ctx.apply(&ty));

        if method == "clone" && args.is_empty() {
            return self.unify(&ty, &ret, span);
        }

        match &ty {
            Type::Var(_) => {
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty,
                    method,
                    args,
                    ret,
                    span,
                });
                Ok(false)
            }
            Type::Named(type_id) => {
                let methods = match self.types.get(*type_id) {
                    Some(TypeDef::Struct { methods, .. }) => methods.clone(),
                    Some(TypeDef::Enum { methods, .. }) => methods.clone(),
                    _ => {
                        return Err(TypeError::NoSuchMethod {
                            ty,
                            method,
                            span,
                        })
                    }
                };

                if let Some(method_sig) = methods.iter().find(|m| m.name == method) {
                    if method_sig.params.len() != args.len() {
                        return Err(TypeError::ArityMismatch {
                            expected: method_sig.params.len(),
                            found: args.len(),
                            span,
                        });
                    }

                    let mut progress = false;
                    for ((param_ty, _mode), arg) in method_sig.params.iter().zip(args.iter()) {
                        if self.unify(param_ty, arg, span)? {
                            progress = true;
                        }
                    }

                    if self.unify(&method_sig.ret, &ret, span)? {
                        progress = true;
                    }

                    Ok(progress)
                } else {
                    let variant = self.types.get(*type_id).and_then(|def| {
                        if let TypeDef::Enum { variants, .. } = def {
                            variants.iter().find(|(n, _)| n == &method).map(|(_, fields)| fields.clone())
                        } else {
                            None
                        }
                    });

                    if let Some(mut fields) = variant {
                        // Instantiate generic type parameters
                        if Some(*type_id) == self.types.get_result_type_id()
                            || Some(*type_id) == self.types.get_option_type_id()
                        {
                            fields = self.instantiate_builtin_enum_variant(*type_id, &method, &fields);
                        } else {
                            // User-defined enum: instantiate any TypeVars with fresh vars
                            fields = self.instantiate_type_vars(&fields);
                        }

                        if fields.len() != args.len() {
                            return Err(TypeError::ArityMismatch {
                                expected: fields.len(),
                                found: args.len(),
                                span,
                            });
                        }
                        let mut progress = false;
                        for (field_ty, arg) in fields.iter().zip(args.iter()) {
                            if self.unify(field_ty, arg, span)? {
                                progress = true;
                            }
                        }
                        if self.unify(&ty, &ret, span)? {
                            progress = true;
                        }
                        Ok(progress)
                    } else {
                        Err(TypeError::NoSuchMethod {
                            ty,
                            method,
                            span,
                        })
                    }
                }
            }
            Type::String => self.resolve_string_method(&method, &args, &ret, span),
            Type::Array { .. } | Type::Slice(_) => {
                self.resolve_array_method(&ty, &method, &args, &ret, span)
            }
            Type::UnresolvedNamed(name) if name == "File" => {
                self.resolve_file_method(&method, &args, &ret, span)
            }
            Type::UnresolvedGeneric { name, args: type_args } if name == "ThreadHandle" => {
                self.resolve_thread_handle_method(&type_args, &method, &args, &ret, span)
            }
            // Shared<T>, Sender<T>, Receiver<T>, Channel<T>
            Type::UnresolvedGeneric { name, args: type_args } if matches!(name.as_str(), "Shared" | "Sender" | "Receiver" | "Channel") => {
                self.resolve_concurrency_generic_method(name, &type_args, &method, &args, &ret, span)
            }
            // Builtin runtime types: Instant, Duration, TcpListener, TcpConnection, Shared (bare)
            Type::UnresolvedNamed(name) if matches!(name.as_str(), "Instant" | "Duration" | "TcpListener" | "TcpConnection" | "HttpResponse" | "Shared") => {
                self.resolve_runtime_method(name, &method, &args, &ret, span)
            }
            Type::Generic { base, args: generic_args } => {
                let (methods, type_params) = match self.types.get(*base) {
                    Some(TypeDef::Struct { methods, type_params, .. }) => {
                        (methods.clone(), type_params.clone())
                    }
                    Some(TypeDef::Enum { methods, type_params, .. }) => {
                        (methods.clone(), type_params.clone())
                    }
                    _ => {
                        return Err(TypeError::NoSuchMethod {
                            ty,
                            method,
                            span,
                        });
                    }
                };

                let subst = Self::build_type_param_subst(&type_params, generic_args);

                if let Some(method_sig) = methods.iter().find(|m| m.name == method) {
                    if method_sig.params.len() != args.len() {
                        return Err(TypeError::ArityMismatch {
                            expected: method_sig.params.len(),
                            found: args.len(),
                            span,
                        });
                    }

                    let mut progress = false;
                    for ((param_ty, _mode), arg) in method_sig.params.iter().zip(args.iter()) {
                        let substituted = Self::substitute_type_params(param_ty, &subst);
                        if self.unify(&substituted, arg, span)? {
                            progress = true;
                        }
                    }

                    let ret_substituted = Self::substitute_type_params(&method_sig.ret, &subst);
                    if self.unify(&ret_substituted, &ret, span)? {
                        progress = true;
                    }

                    Ok(progress)
                } else {
                    // Check enum variants as constructors
                    let variant = self.types.get(*base).and_then(|def| {
                        if let TypeDef::Enum { type_params: tp, variants, .. } = def {
                            variants.iter().find(|(n, _)| n == &method).map(|(_, fields)| {
                                let subst = Self::build_type_param_subst(tp, generic_args);
                                fields.iter()
                                    .map(|t| Self::substitute_type_params(t, &subst))
                                    .collect::<Vec<_>>()
                            })
                        } else {
                            None
                        }
                    });

                    if let Some(fields) = variant {
                        if fields.len() != args.len() {
                            return Err(TypeError::ArityMismatch {
                                expected: fields.len(),
                                found: args.len(),
                                span,
                            });
                        }
                        let mut progress = false;
                        for (field_ty, arg) in fields.iter().zip(args.iter()) {
                            if self.unify(field_ty, arg, span)? {
                                progress = true;
                            }
                        }
                        if self.unify(&ty, &ret, span)? {
                            progress = true;
                        }
                        Ok(progress)
                    } else {
                        Err(TypeError::NoSuchMethod {
                            ty,
                            method,
                            span,
                        })
                    }
                }
            }
            _ => {
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty,
                    method,
                    args,
                    ret,
                    span,
                });
                Ok(false)
            }
        }
    }

    fn instantiate_builtin_enum_variant(
        &self,
        type_id: TypeId,
        _variant_name: &str,
        variant_fields: &[Type],
    ) -> Vec<Type> {
        let substitution = if Some(type_id) == self.types.get_result_type_id() {
            if let Some(Type::Result { ok, err }) = &self.current_return_type {
                let mut subst = HashMap::new();
                subst.insert(TypeVarId(0), *ok.clone());
                subst.insert(TypeVarId(1), *err.clone());
                subst
            } else {
                HashMap::new()
            }
        } else if Some(type_id) == self.types.get_option_type_id() {
            if let Some(Type::Option(inner)) = &self.current_return_type {
                let mut subst = HashMap::new();
                subst.insert(TypeVarId(0), *inner.clone());
                subst
            } else {
                HashMap::new()
            }
        } else {
            HashMap::new()
        };

        variant_fields
            .iter()
            .map(|ty| self.apply_type_var_substitution(ty, &substitution))
            .collect()
    }

    fn instantiate_type_vars(&mut self, types: &[Type]) -> Vec<Type> {
        let mut subst: HashMap<TypeVarId, Type> = HashMap::new();
        for ty in types {
            self.collect_type_vars(ty, &mut subst);
        }
        types
            .iter()
            .map(|ty| self.apply_type_var_substitution(ty, &subst))
            .collect()
    }

    fn collect_type_vars(&mut self, ty: &Type, subst: &mut HashMap<TypeVarId, Type>) {
        match ty {
            Type::Var(id) => {
                subst.entry(*id).or_insert_with(|| self.ctx.fresh_var());
            }
            Type::Tuple(elems) => {
                for e in elems {
                    self.collect_type_vars(e, subst);
                }
            }
            Type::Array { elem, .. } | Type::Slice(elem) | Type::Option(elem) => {
                self.collect_type_vars(elem, subst);
            }
            Type::Result { ok, err } => {
                self.collect_type_vars(ok, subst);
                self.collect_type_vars(err, subst);
            }
            Type::Generic { args, .. } => {
                for a in args {
                    self.collect_type_vars_generic_arg(a, subst);
                }
            }
            Type::Fn { params, ret } => {
                for p in params {
                    self.collect_type_vars(p, subst);
                }
                self.collect_type_vars(ret, subst);
            }
            _ => {}
        }
    }

    fn collect_type_vars_generic_arg(&mut self, arg: &GenericArg, subst: &mut HashMap<TypeVarId, Type>) {
        match arg {
            GenericArg::Type(ty) => self.collect_type_vars(ty, subst),
            GenericArg::ConstUsize(_) => {}
        }
    }

    fn apply_type_var_substitution(
        &self,
        ty: &Type,
        substitution: &HashMap<TypeVarId, Type>,
    ) -> Type {
        match ty {
            Type::Var(id) => substitution.get(id).cloned().unwrap_or_else(|| ty.clone()),
            Type::Tuple(elems) => Type::Tuple(
                elems
                    .iter()
                    .map(|e| self.apply_type_var_substitution(e, substitution))
                    .collect(),
            ),
            Type::Array { elem, len } => Type::Array {
                elem: Box::new(self.apply_type_var_substitution(elem, substitution)),
                len: *len,
            },
            Type::Slice(elem) => {
                Type::Slice(Box::new(self.apply_type_var_substitution(elem, substitution)))
            }
            Type::Option(inner) => {
                Type::Option(Box::new(self.apply_type_var_substitution(inner, substitution)))
            }
            Type::Result { ok, err } => Type::Result {
                ok: Box::new(self.apply_type_var_substitution(ok, substitution)),
                err: Box::new(self.apply_type_var_substitution(err, substitution)),
            },
            Type::Generic { base, args } => Type::Generic {
                base: *base,
                args: args
                    .iter()
                    .map(|a| self.apply_type_var_substitution_generic_arg(a, substitution))
                    .collect(),
            },
            Type::Fn { params, ret } => Type::Fn {
                params: params
                    .iter()
                    .map(|p| self.apply_type_var_substitution(p, substitution))
                    .collect(),
                ret: Box::new(self.apply_type_var_substitution(ret, substitution)),
            },
            _ => ty.clone(),
        }
    }

    fn apply_type_var_substitution_generic_arg(
        &self,
        arg: &GenericArg,
        substitution: &HashMap<TypeVarId, Type>,
    ) -> GenericArg {
        match arg {
            GenericArg::Type(ty) => {
                GenericArg::Type(Box::new(self.apply_type_var_substitution(ty, substitution)))
            }
            GenericArg::ConstUsize(n) => GenericArg::ConstUsize(*n),
        }
    }

    fn resolve_string_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        if let Some(method_def) = rask_stdlib::lookup_method("string", method) {
            let expected_params = method_def.params.len();
            if args.len() != expected_params {
                return Err(TypeError::ArityMismatch {
                    expected: expected_params,
                    found: args.len(),
                    span,
                });
            }
            return match method_def.ret_ty {
                "usize" => self.unify(ret, &Type::U64, span),
                "bool" => self.unify(ret, &Type::Bool, span),
                "()" => self.unify(ret, &Type::Unit, span),
                "string" => self.unify(ret, &Type::String, span),
                "char" => self.unify(ret, &Type::Char, span),
                _ => Ok(false),
            };
        }

        match method {
            "len" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "is_empty" if args.is_empty() => self.unify(ret, &Type::Bool, span),
            "contains" if args.len() == 1 => {
                self.unify(&args[0], &Type::String, span)?;
                self.unify(ret, &Type::Bool, span)
            }
            "push" | "push_str" => self.unify(ret, &Type::Unit, span),
            _ => Ok(false),
        }
    }

    fn resolve_array_method(
        &mut self,
        _array_ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        if let Some(method_def) = rask_stdlib::lookup_method("Vec", method) {
            let expected_params = method_def.params.len();
            if args.len() != expected_params {
                return Err(TypeError::ArityMismatch {
                    expected: expected_params,
                    found: args.len(),
                    span,
                });
            }
            return match method_def.ret_ty {
                "usize" => self.unify(ret, &Type::U64, span),
                "bool" => self.unify(ret, &Type::Bool, span),
                "()" => self.unify(ret, &Type::Unit, span),
                _ => Ok(false),
            };
        }

        match method {
            "len" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "is_empty" if args.is_empty() => self.unify(ret, &Type::Bool, span),
            "push" => self.unify(ret, &Type::Unit, span),
            "pop" => {
                let elem_ty = self.ctx.fresh_var();
                self.unify(ret, &Type::Option(Box::new(elem_ty)), span)
            }
            _ => Ok(false),
        }
    }

    fn resolve_file_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // File methods return Result types (T or IoError)
        let io_error_ty = Type::UnresolvedNamed("IoError".to_string());

        match method {
            "read_text" | "read_all" if args.is_empty() => {
                // Returns string or IoError
                let result_type = Type::Result {
                    ok: Box::new(Type::String),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "close" if args.is_empty() => {
                // Returns () or IoError (takes self)
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "write_all" if args.len() == 1 => {
                // write_all(data: string) -> () or IoError
                self.unify(&args[0], &Type::String, span)?;
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "write" if args.len() == 1 => {
                // write(data: string) -> usize or IoError
                self.unify(&args[0], &Type::String, span)?;
                let result_type = Type::Result {
                    ok: Box::new(Type::U64),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "write_line" if args.len() == 1 => {
                // write_line(data: string) -> () or IoError
                self.unify(&args[0], &Type::String, span)?;
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            "lines" if args.is_empty() => {
                // Returns Vec<string> or IoError
                let vec_string = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(Type::String))],
                };
                let result_type = Type::Result {
                    ok: Box::new(vec_string),
                    err: Box::new(io_error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: Type::UnresolvedNamed("File".to_string()),
                method: method.to_string(),
                span,
            }),
        }
    }

    fn resolve_thread_handle_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // ThreadHandle<T> has two methods:
        // - join(self) -> T or string
        // - detach(self) -> ()

        match method {
            "join" if args.is_empty() => {
                // Extract the T type parameter
                let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
                    *t.clone()
                } else {
                    self.ctx.fresh_var()
                };

                // join returns Result<T, string>
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::String),
                };

                self.unify(ret, &result_type, span)
            }
            "detach" if args.is_empty() => {
                // detach returns ()
                self.unify(ret, &Type::Unit, span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: Type::UnresolvedGeneric {
                    name: "ThreadHandle".to_string(),
                    args: type_args.to_vec(),
                },
                method: method.to_string(),
                span,
            }),
        }
    }

    fn resolve_runtime_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let error_ty = Type::UnresolvedNamed("Error".to_string());
        match (type_name, method) {
            // Instant static constructor and instance methods
            ("Instant", "now") if args.is_empty() => {
                self.unify(ret, &Type::UnresolvedNamed("Instant".to_string()), span)
            }
            ("Instant", "elapsed") if args.is_empty() => {
                self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
            }
            ("Instant", "duration_since") if args.len() == 1 => {
                self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
            }
            // Duration methods
            ("Duration", "as_secs_f64") if args.is_empty() => {
                self.unify(ret, &Type::F64, span)
            }
            ("Duration", "as_nanos") if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            ("Duration", "as_secs") if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            ("Duration", "from_nanos") if args.len() == 1 => {
                self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
            }
            // TcpListener
            ("TcpListener", "accept") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(Type::UnresolvedNamed("TcpConnection".to_string())),
                    err: Box::new(error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            // TcpConnection
            ("TcpConnection", "read_http_request") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(Type::UnresolvedNamed("HttpRequest".to_string())),
                    err: Box::new(error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            ("TcpConnection", "write_http_response") if args.len() == 1 => {
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(error_ty),
                };
                self.unify(ret, &result_type, span)
            }
            // HttpResponse — allow method-style access for chaining
            ("HttpResponse", "status") if args.is_empty() => {
                self.unify(ret, &Type::I32, span)
            }
            // Shared static constructor: Shared.new(value) -> Shared<T>
            ("Shared", "new") if args.len() == 1 => {
                let inner = args[0].clone();
                let shared_ty = Type::UnresolvedGeneric {
                    name: "Shared".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner))],
                };
                self.unify(ret, &shared_ty, span)
            }
            _ => {
                // Fall through to constraint system for unknown methods
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: Type::UnresolvedNamed(type_name.to_string()),
                    method: method.to_string(),
                    args: args.to_vec(),
                    ret: ret.clone(),
                    span,
                });
                Ok(false)
            }
        }
    }

    fn resolve_concurrency_generic_method(
        &mut self,
        type_name: &str,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // Extract inner type T from generic args
        let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };

        match (type_name, method) {
            // Shared<T>.read(|T| -> R) -> R
            ("Shared", "read") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                self.unify(ret, &result_var, span)
            }
            // Shared<T>.write(|T| -> R) -> R
            ("Shared", "write") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                self.unify(ret, &result_var, span)
            }
            // Shared<T>.clone() -> Shared<T>
            ("Shared", "clone") if args.is_empty() => {
                let shared_ty = Type::UnresolvedGeneric {
                    name: "Shared".to_string(),
                    args: type_args.to_vec(),
                };
                self.unify(ret, &shared_ty, span)
            }
            // Sender<T>.send(value: T) -> () or string
            ("Sender", "send") if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Sender<T>.clone() -> Sender<T>
            ("Sender", "clone") if args.is_empty() => {
                let sender_ty = Type::UnresolvedGeneric {
                    name: "Sender".to_string(),
                    args: type_args.to_vec(),
                };
                self.unify(ret, &sender_ty, span)
            }
            // Receiver<T>.recv() -> T or string
            ("Receiver", "recv") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Receiver<T>.try_recv() -> T or string
            ("Receiver", "try_recv") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Channel<T>.buffered(n) -> (Sender<T>, Receiver<T>)
            ("Channel", "buffered") if args.len() == 1 => {
                let sender = Type::UnresolvedGeneric {
                    name: "Sender".to_string(),
                    args: type_args.to_vec(),
                };
                let receiver = Type::UnresolvedGeneric {
                    name: "Receiver".to_string(),
                    args: type_args.to_vec(),
                };
                let tuple_ty = Type::Tuple(vec![sender, receiver]);
                self.unify(ret, &tuple_ty, span)
            }
            // Channel<T>.unbuffered() -> (Sender<T>, Receiver<T>)
            ("Channel", "unbuffered") if args.is_empty() => {
                let sender = Type::UnresolvedGeneric {
                    name: "Sender".to_string(),
                    args: type_args.to_vec(),
                };
                let receiver = Type::UnresolvedGeneric {
                    name: "Receiver".to_string(),
                    args: type_args.to_vec(),
                };
                let tuple_ty = Type::Tuple(vec![sender, receiver]);
                self.unify(ret, &tuple_ty, span)
            }
            _ => {
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: Type::UnresolvedGeneric {
                        name: type_name.to_string(),
                        args: type_args.to_vec(),
                    },
                    method: method.to_string(),
                    args: args.to_vec(),
                    ret: ret.clone(),
                    span,
                });
                Ok(false)
            }
        }
    }
}

impl Default for TypeChecker {
    fn default() -> Self {
        Self::new(ResolvedProgram::default())
    }
}

// ============================================================================
// Public API
// ============================================================================

pub fn typecheck(resolved: ResolvedProgram, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
    let checker = TypeChecker::new(resolved);
    checker.check(decls)
}
