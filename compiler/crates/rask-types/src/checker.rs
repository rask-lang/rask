//! Type checker implementation.

use std::collections::HashMap;

use rask_ast::decl::{Decl, DeclKind, EnumDecl, FnDecl, StructDecl, TraitDecl};
use rask_ast::expr::{BinOp, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};
use rask_resolve::{ResolvedProgram, SymbolId, SymbolKind};

use crate::types::{Type, TypeId, TypeVarId};

// ============================================================================
// Type Definitions
// ============================================================================

/// Information about a user-defined type.
#[derive(Debug, Clone)]
pub enum TypeDef {
    Struct {
        name: String,
        fields: Vec<(String, Type)>,
        methods: Vec<MethodSig>,
    },
    Enum {
        name: String,
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
    pub params: Vec<Type>,
    pub ret: Type,
}

/// How self is passed to a method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfParam {
    None,  // Static method
    Value, // self (by value)
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
}

impl TypeTable {
    pub fn new() -> Self {
        let mut table = Self {
            types: Vec::new(),
            type_names: HashMap::new(),
            builtins: HashMap::new(),
        };
        table.register_builtins();
        table
    }

    fn register_builtins(&mut self) {
        // Primitive types
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
        // Integer type aliases
        self.builtins.insert("int".to_string(), Type::I64);
        self.builtins.insert("uint".to_string(), Type::U64);
        self.builtins.insert("isize".to_string(), Type::I64);
        self.builtins.insert("usize".to_string(), Type::U64);
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

    /// Check if a name is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.builtins.contains_key(name) || self.type_names.contains_key(name)
    }

    /// Get TypeId for a name (user-defined types only).
    pub fn get_type_id(&self, name: &str) -> Option<TypeId> {
        self.type_names.get(name).copied()
    }

    /// Iterate over all type definitions.
    pub fn iter(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.iter()
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
                args: args.iter().map(|t| self.apply(t)).collect(),
            },
            Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
                name: name.clone(),
                args: args.iter().map(|t| self.apply(t)).collect(),
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
                args.iter().any(|a| self.occurs_in(var, a))
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
}

// ============================================================================
// Type Errors
// ============================================================================

/// A type error.
#[derive(Debug, thiserror::Error)]
pub enum TypeError {
    #[error("type mismatch: expected {expected:?}, found {found:?}")]
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
    #[error("type {ty:?} is not callable")]
    NotCallable { ty: Type, span: Span },
    #[error("no such field '{field}' on type {ty:?}")]
    NoSuchField { ty: Type, field: String, span: Span },
    #[error("no such method '{method}' on type {ty:?}")]
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

    // Never type
    if s == "!" {
        return Ok(Type::Never);
    }

    // Check for optional suffix: T?
    if s.ends_with('?') && !s.starts_with('(') {
        let inner = parse_type_string(&s[..s.len() - 1], types)?;
        return Ok(Type::Option(Box::new(inner)));
    }

    // Check for tuple: (T1, T2, ...)
    if s.starts_with('(') && s.ends_with(')') {
        let inner = &s[1..s.len() - 1];
        if inner.is_empty() {
            return Ok(Type::Unit);
        }
        let parts = split_type_args(inner);
        if parts.len() == 1 && !inner.contains(',') {
            // Single element in parens - not a tuple, just grouping
            return parse_type_string(inner, types);
        }
        let elems: Result<Vec<_>, _> = parts.iter().map(|p| parse_type_string(p, types)).collect();
        return Ok(Type::Tuple(elems?));
    }

    // Check for slice: []T
    if s.starts_with("[]") {
        let inner = parse_type_string(&s[2..], types)?;
        return Ok(Type::Slice(Box::new(inner)));
    }

    // Check for array: [T; N]
    if s.starts_with('[') && s.ends_with(']') {
        let inner = &s[1..s.len() - 1];
        if let Some(semi_pos) = inner.find(';') {
            let elem_str = inner[..semi_pos].trim();
            let len_str = inner[semi_pos + 1..].trim();
            let elem = parse_type_string(elem_str, types)?;
            let len: usize = len_str
                .parse()
                .map_err(|_| TypeError::InvalidTypeString(s.to_string()))?;
            return Ok(Type::Array {
                elem: Box::new(elem),
                len,
            });
        }
        // Just [T] - slice
        let inner = parse_type_string(inner, types)?;
        return Ok(Type::Slice(Box::new(inner)));
    }

    // Check for function type: func(T1, T2) -> R
    if s.starts_with("func(") || s.starts_with("fn(") {
        return parse_fn_type(s, types);
    }

    // Check for generic: Name<T, U>
    if let Some(lt_pos) = s.find('<') {
        if s.ends_with('>') {
            let name = s[..lt_pos].trim();
            let args_str = &s[lt_pos + 1..s.len() - 1];
            let arg_strs = split_type_args(args_str);
            let args: Result<Vec<_>, _> =
                arg_strs.iter().map(|a| parse_type_string(a, types)).collect();
            let args = args?;

            // Special cases
            match name {
                "Option" if args.len() == 1 => {
                    return Ok(Type::Option(Box::new(args.into_iter().next().unwrap())));
                }
                "Result" if args.len() == 2 => {
                    let mut iter = args.into_iter();
                    return Ok(Type::Result {
                        ok: Box::new(iter.next().unwrap()),
                        err: Box::new(iter.next().unwrap()),
                    });
                }
                _ => {
                    // Check if base type is registered
                    if let Some(base_id) = types.get_type_id(name) {
                        return Ok(Type::Generic { base: base_id, args });
                    }
                    // Unresolved generic
                    return Ok(Type::UnresolvedGeneric {
                        name: name.to_string(),
                        args,
                    });
                }
            }
        }
    }

    // Simple type name
    if let Some(ty) = types.lookup(s) {
        return Ok(ty);
    }

    // Unresolved named type
    Ok(Type::UnresolvedNamed(s.to_string()))
}

/// Split generic arguments by comma, respecting nested angle brackets.
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

/// Parse a function type: func(T1, T2) -> R
fn parse_fn_type(s: &str, types: &TypeTable) -> Result<Type, TypeError> {
    let prefix = if s.starts_with("func(") {
        "func("
    } else {
        "fn("
    };
    let rest = &s[prefix.len()..];

    // Find matching paren
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

    // Parse params
    let params: Result<Vec<_>, _> = if params_str.is_empty() {
        Ok(Vec::new())
    } else {
        split_type_args(params_str)
            .iter()
            .map(|p| parse_type_string(p, types))
            .collect()
    };
    let params = params?;

    // Parse return type
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
        }
    }

    /// Type check a list of declarations.
    pub fn check(mut self, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
        // Pass 1: Collect type declarations
        self.collect_type_declarations(decls);

        // Pass 2: Check all declarations
        for decl in decls {
            self.check_decl(decl);
        }

        // Pass 3: Solve constraints
        self.solve_constraints();

        // Pass 4: Apply substitutions to all node types
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
            Err(self.errors)
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

        self.types.register_type(TypeDef::Struct {
            name: s.name.clone(),
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

        self.types.register_type(TypeDef::Enum {
            name: e.name.clone(),
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
        let has_self = m.params.iter().any(|p| p.name == "self");
        let self_param = if has_self {
            SelfParam::Value
        } else {
            SelfParam::None
        };

        let params: Vec<_> = m
            .params
            .iter()
            .filter(|p| p.name != "self")
            .map(|p| parse_type_string(&p.ty, &self.types).unwrap_or(Type::Error))
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
            DeclKind::Fn(f) => self.check_fn(f),
            DeclKind::Struct(s) => {
                for method in &s.methods {
                    self.check_fn(method);
                }
            }
            DeclKind::Enum(e) => {
                for method in &e.methods {
                    self.check_fn(method);
                }
            }
            DeclKind::Impl(i) => {
                for method in &i.methods {
                    self.check_fn(method);
                }
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
            _ => {}
        }
    }

    fn check_fn(&mut self, f: &FnDecl) {
        // Set up return type
        let ret_ty = f
            .ret_ty
            .as_ref()
            .map(|t| parse_type_string(t, &self.types).unwrap_or(Type::Error))
            .unwrap_or(Type::Unit);
        self.current_return_type = Some(ret_ty);

        // Register parameter types
        for param in &f.params {
            if param.name == "self" {
                continue;
            }
            if let Ok(ty) = parse_type_string(&param.ty, &self.types) {
                // Find the symbol for this parameter and record its type
                // The name resolution should have created a symbol for it
                // For now we just parse and validate the type
                let _ = ty;
            }
        }

        // Check body
        for stmt in &f.body {
            self.check_stmt(stmt);
        }

        self.current_return_type = None;
    }

    // ------------------------------------------------------------------------
    // Statement Checking
    // ------------------------------------------------------------------------

    fn check_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.infer_expr(expr);
            }
            StmtKind::Let { name: _, ty, init } => {
                let init_ty = self.infer_expr(init);
                if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        self.ctx
                            .add_constraint(TypeConstraint::Equal(declared, init_ty, stmt.span));
                    }
                }
            }
            StmtKind::Const { name: _, ty, init } => {
                let init_ty = self.infer_expr(init);
                if let Some(ty_str) = ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        self.ctx
                            .add_constraint(TypeConstraint::Equal(declared, init_ty, stmt.span));
                    }
                }
            }
            StmtKind::Assign { target, value } => {
                let target_ty = self.infer_expr(target);
                let value_ty = self.infer_expr(value);
                self.ctx.add_constraint(TypeConstraint::Equal(
                    target_ty, value_ty, stmt.span,
                ));
            }
            StmtKind::Return(value) => {
                let ret_ty = if let Some(expr) = value {
                    self.infer_expr(expr)
                } else {
                    Type::Unit
                };
                if let Some(expected) = &self.current_return_type {
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        expected.clone(),
                        ret_ty,
                        stmt.span,
                    ));
                }
            }
            StmtKind::While { cond, body, .. } => {
                let cond_ty = self.infer_expr(cond);
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, cond_ty, stmt.span));
                for s in body {
                    self.check_stmt(s);
                }
            }
            StmtKind::For { iter, body, .. } => {
                self.infer_expr(iter);
                for s in body {
                    self.check_stmt(s);
                }
            }
            StmtKind::Break(_) | StmtKind::Continue(_) | StmtKind::Deliver { .. } => {}
            StmtKind::Ensure(body) => {
                for s in body {
                    self.check_stmt(s);
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
            StmtKind::WhileLet { expr, body, .. } => {
                self.infer_expr(expr);
                for s in body {
                    self.check_stmt(s);
                }
            }
            StmtKind::Loop { body, .. } => {
                for s in body {
                    self.check_stmt(s);
                }
            }
        }
    }

    // ------------------------------------------------------------------------
    // Expression Inference
    // ------------------------------------------------------------------------

    fn infer_expr(&mut self, expr: &Expr) -> Type {
        let ty = match &expr.kind {
            // Literals
            ExprKind::Int(_) => Type::I32, // Default integer type
            ExprKind::Float(_) => Type::F64, // Default float type
            ExprKind::String(_) => Type::String,
            ExprKind::Char(_) => Type::Char,
            ExprKind::Bool(_) => Type::Bool,

            // Identifier
            ExprKind::Ident(_) => {
                if let Some(&sym_id) = self.resolved.resolutions.get(&expr.id) {
                    self.get_symbol_type(sym_id)
                } else {
                    Type::Error
                }
            }

            // Binary operation
            ExprKind::Binary { op, left, right } => {
                self.check_binary(*op, left, right, expr.span)
            }

            // Unary operation
            ExprKind::Unary { op: _, operand } => {
                self.infer_expr(operand)
            }

            // Function call
            ExprKind::Call { func, args } => self.check_call(func, args, expr.span),

            // Method call
            ExprKind::MethodCall {
                object,
                method,
                args,
                ..
            } => self.check_method_call(object, method, args, expr.span),

            // Field access
            ExprKind::Field { object, field } => self.check_field_access(object, field, expr.span),

            // Index access
            ExprKind::Index { object, index } => {
                let obj_ty = self.infer_expr(object);
                let idx_ty = self.infer_expr(index);
                // Index should be integer
                self.ctx.add_constraint(TypeConstraint::Equal(
                    Type::U64, // usize
                    idx_ty,
                    expr.span,
                ));
                // Result type depends on collection type
                match &obj_ty {
                    Type::Array { elem, .. } | Type::Slice(elem) => *elem.clone(),
                    Type::String => Type::Char,
                    _ => self.ctx.fresh_var(),
                }
            }

            // If expression
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

            // If-let expression
            ExprKind::IfLet {
                then_branch,
                else_branch,
                expr: value,
                ..
            } => {
                self.infer_expr(value);
                let then_ty = self.infer_expr(then_branch);
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

            // Match expression
            ExprKind::Match { scrutinee, arms } => {
                self.infer_expr(scrutinee);
                let result_ty = self.ctx.fresh_var();
                for arm in arms {
                    let arm_ty = self.infer_expr(&arm.body);
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        result_ty.clone(),
                        arm_ty,
                        expr.span,
                    ));
                }
                result_ty
            }

            // Block expression
            ExprKind::Block(stmts) => {
                for stmt in stmts {
                    self.check_stmt(stmt);
                }
                // Block type is unit unless last statement is an expression
                if let Some(last) = stmts.last() {
                    if let StmtKind::Expr(e) = &last.kind {
                        return self.infer_expr(e);
                    }
                }
                Type::Unit
            }

            // Struct literal
            ExprKind::StructLit { name, fields, .. } => {
                // Get struct type
                if let Some(ty) = self.types.lookup(name) {
                    // Check field types
                    if let Type::Named(type_id) = &ty {
                        if let Some(TypeDef::Struct {
                            fields: struct_fields,
                            ..
                        }) = self.types.get(*type_id)
                        {
                            let struct_fields = struct_fields.clone();
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
                        }
                    }
                    ty
                } else {
                    Type::UnresolvedNamed(name.clone())
                }
            }

            // Array literal
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

            // Tuple literal
            ExprKind::Tuple(elements) => {
                let elem_types: Vec<_> = elements.iter().map(|e| self.infer_expr(e)).collect();
                Type::Tuple(elem_types)
            }

            // Range
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.infer_expr(s);
                }
                if let Some(e) = end {
                    self.infer_expr(e);
                }
                // Range type - simplified
                Type::UnresolvedNamed("Range".to_string())
            }

            // Try (?)
            ExprKind::Try(inner) => {
                let inner_ty = self.infer_expr(inner);
                // Result/Option unwrapping
                match &inner_ty {
                    Type::Option(inner) => *inner.clone(),
                    Type::Result { ok, .. } => *ok.clone(),
                    _ => {
                        self.errors.push(TypeError::Mismatch {
                            expected: Type::Option(Box::new(self.ctx.fresh_var())),
                            found: inner_ty,
                            span: expr.span,
                        });
                        Type::Error
                    }
                }
            }

            // Closure
            ExprKind::Closure { params, body, .. } => {
                let param_types: Vec<_> = params
                    .iter()
                    .map(|p| {
                        p.ty.as_ref()
                            .and_then(|t| parse_type_string(t, &self.types).ok())
                            .unwrap_or_else(|| self.ctx.fresh_var())
                    })
                    .collect();
                let ret_ty = self.infer_expr(body);
                Type::Fn {
                    params: param_types,
                    ret: Box::new(ret_ty),
                }
            }

            // Cast
            ExprKind::Cast { expr: inner, ty } => {
                self.infer_expr(inner);
                parse_type_string(ty, &self.types).unwrap_or(Type::Error)
            }

            // Unsafe block
            ExprKind::Unsafe { body } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                // Unsafe block type is unit unless last statement is an expression
                if let Some(last) = body.last() {
                    if let StmtKind::Expr(e) = &last.kind {
                        return self.infer_expr(e);
                    }
                }
                Type::Unit
            }

            // Comptime block
            ExprKind::Comptime { body } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                // Comptime block type is unit unless last statement is an expression
                if let Some(last) = body.last() {
                    if let StmtKind::Expr(e) = &last.kind {
                        return self.infer_expr(e);
                    }
                }
                Type::Unit
            }

            // Spawn block
            ExprKind::Spawn { body } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                // Returns a handle to the spawned task
                Type::UnresolvedNamed("JoinHandle".to_string())
            }

            // With block
            ExprKind::WithBlock { args, body, .. } => {
                for arg in args {
                    self.infer_expr(arg);
                }
                for stmt in body {
                    self.check_stmt(stmt);
                }
                Type::Unit
            }

            // Block call (e.g., raw_thread { ... })
            ExprKind::BlockCall { body, .. } => {
                for stmt in body {
                    self.check_stmt(stmt);
                }
                Type::Unit
            }

            // Array repeat expression
            ExprKind::ArrayRepeat { value, count } => {
                let elem_ty = self.infer_expr(value);
                self.infer_expr(count);
                // We don't know the size at compile time necessarily
                Type::Array {
                    elem: Box::new(elem_ty),
                    len: 0, // Unknown size
                }
            }

            // Null coalesce
            ExprKind::NullCoalesce { value, default } => {
                let val_ty = self.infer_expr(value);
                let def_ty = self.infer_expr(default);
                // Result should be the inner type
                self.ctx.add_constraint(TypeConstraint::Equal(
                    val_ty,
                    Type::Option(Box::new(def_ty.clone())),
                    expr.span,
                ));
                def_ty
            }

            // Optional field access
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

            // Assert/Check expressions
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

        // Record the type for this node
        self.node_types.insert(expr.id, ty.clone());
        ty
    }

    // ------------------------------------------------------------------------
    // Specific Type Checks
    // ------------------------------------------------------------------------

    fn check_binary(&mut self, op: BinOp, left: &Expr, right: &Expr, span: Span) -> Type {
        let left_ty = self.infer_expr(left);
        let right_ty = self.infer_expr(right);

        // Add constraint that operands have compatible types
        self.ctx.add_constraint(TypeConstraint::Equal(
            left_ty.clone(),
            right_ty.clone(),
            span,
        ));

        match op {
            // Comparison operators return bool
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => Type::Bool,
            // Logical operators need bool operands, return bool
            BinOp::And | BinOp::Or => {
                self.ctx
                    .add_constraint(TypeConstraint::Equal(Type::Bool, left_ty, span));
                Type::Bool
            }
            // Arithmetic operators return the operand type
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod => left_ty,
            // Bitwise operators
            BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr => left_ty,
        }
    }

    fn check_call(&mut self, func: &Expr, args: &[Expr], span: Span) -> Type {
        // Check for builtin function calls by name
        if let ExprKind::Ident(name) = &func.kind {
            if self.is_builtin_function(name) {
                // Builtins accept any arguments and return Unit (or Never for panic)
                for arg in args {
                    self.infer_expr(arg);
                }
                return if name == "panic" {
                    Type::Never
                } else {
                    Type::Unit
                };
            }
        }

        let func_ty = self.infer_expr(func);
        let arg_types: Vec<_> = args.iter().map(|a| self.infer_expr(a)).collect();

        match func_ty {
            Type::Fn { params, ret } => {
                // If function has 0 params but was called with args,
                // it might be a builtin - be lenient
                if params.is_empty() && !arg_types.is_empty() {
                    // Probably a builtin or variadic function
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
                // Unknown function type - create fresh return type
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
                // Check if it's a builtin function or constructor
                // For now, assume it might be callable
                self.ctx.fresh_var()
            }
        }
    }

    /// Check if a name is a built-in function.
    fn is_builtin_function(&self, name: &str) -> bool {
        matches!(name, "println" | "print" | "panic" | "assert" | "debug")
    }

    fn check_method_call(
        &mut self,
        object: &Expr,
        method: &str,
        args: &[Expr],
        span: Span,
    ) -> Type {
        let obj_ty = self.infer_expr(object);
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

    fn check_field_access(&mut self, object: &Expr, field: &str, span: Span) -> Type {
        let obj_ty = self.infer_expr(object);
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
        // Check if we've already inferred a type
        if let Some(ty) = self.symbol_types.get(&sym_id) {
            return ty.clone();
        }

        // Check for annotation in symbol table
        if let Some(sym) = self.resolved.symbols.get(sym_id) {
            // Check kind-specific type info
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
                        if let Some(type_id) = self.types.get_type_id(&enum_sym.name) {
                            return Type::Named(type_id);
                        }
                    }
                }
                _ => {}
            }
        }

        // No type yet - create fresh variable
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
            // Same type - done
            (a, b) if a == b => Ok(false),

            // Type variable on left - bind it
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

            // Type variable on right - bind it
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

            // Generic types - unify base and args
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
                    if self.unify(arg1, arg2, span)? {
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

            // Tuple types
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

            // Option types
            (Type::Option(inner1), Type::Option(inner2)) => self.unify(inner1, inner2, span),

            // Result types
            (
                Type::Result { ok: o1, err: e1 },
                Type::Result { ok: o2, err: e2 },
            ) => {
                let p1 = self.unify(o1, o2, span)?;
                let p2 = self.unify(e1, e2, span)?;
                Ok(p1 || p2)
            }

            // Array types
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

            // Slice types
            (Type::Slice(e1), Type::Slice(e2)) => self.unify(e1, e2, span),

            // Error absorbs everything (error recovery)
            (Type::Error, _) | (_, Type::Error) => Ok(false),

            // Never coerces to anything
            (Type::Never, _) => Ok(false),
            (_, Type::Never) => Ok(false),

            // Unresolved types - defer
            (Type::UnresolvedNamed(_), _) | (_, Type::UnresolvedNamed(_)) => {
                // Re-add constraint for later
                self.ctx
                    .add_constraint(TypeConstraint::Equal(t1, t2, span));
                Ok(false)
            }

            // Mismatch
            _ => Err(TypeError::Mismatch {
                expected: t1,
                found: t2,
                span,
            }),
        }
    }

    fn resolve_field(
        &mut self,
        ty: Type,
        field: String,
        expected: Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let ty = self.ctx.apply(&ty);

        match &ty {
            Type::Var(_) => {
                // Type not yet known - re-add constraint
                self.ctx.add_constraint(TypeConstraint::HasField {
                    ty,
                    field,
                    expected,
                    span,
                });
                Ok(false)
            }
            Type::Named(type_id) => {
                let field_ty_opt = self.types.get(*type_id).and_then(|def| {
                    if let TypeDef::Struct { fields, .. } = def {
                        fields.iter().find(|(n, _)| n == &field).map(|(_, t)| t.clone())
                    } else {
                        None
                    }
                });

                if let Some(field_ty) = field_ty_opt {
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
                // Tuple field access: t.0, t.1, etc.
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
        let ty = self.ctx.apply(&ty);

        match &ty {
            Type::Var(_) => {
                // Type not yet known - re-add constraint
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
                    for (param, arg) in method_sig.params.iter().zip(args.iter()) {
                        if self.unify(param, arg, span)? {
                            progress = true;
                        }
                    }

                    if self.unify(&method_sig.ret, &ret, span)? {
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
            // Built-in type methods
            Type::String => self.resolve_string_method(&method, &args, &ret, span),
            Type::Array { .. } | Type::Slice(_) => {
                self.resolve_array_method(&ty, &method, &args, &ret, span)
            }
            _ => {
                // Defer unresolved method calls
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

    fn resolve_string_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // Check rask-stdlib for method definition
        if let Some(method_def) = rask_stdlib::lookup_method("string", method) {
            // Method exists in stdlib - validate arity (excluding self)
            let expected_params = method_def.params.len();
            if args.len() != expected_params {
                return Err(TypeError::ArityMismatch {
                    expected: expected_params,
                    found: args.len(),
                    span,
                });
            }
            // Map common return types
            return match method_def.ret_ty {
                "usize" => self.unify(ret, &Type::U64, span),
                "bool" => self.unify(ret, &Type::Bool, span),
                "()" => self.unify(ret, &Type::Unit, span),
                "string" => self.unify(ret, &Type::String, span),
                "char" => self.unify(ret, &Type::Char, span),
                _ => Ok(false), // Complex return type - defer
            };
        }

        // Fallback for unlisted methods
        match method {
            "len" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "is_empty" if args.is_empty() => self.unify(ret, &Type::Bool, span),
            "contains" if args.len() == 1 => {
                self.unify(&args[0], &Type::String, span)?;
                self.unify(ret, &Type::Bool, span)
            }
            "push" | "push_str" => self.unify(ret, &Type::Unit, span),
            _ => {
                // Unknown method - could be user-defined extension
                Ok(false)
            }
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
        // Check rask-stdlib for Vec method definition
        if let Some(method_def) = rask_stdlib::lookup_method("Vec", method) {
            // Method exists in stdlib - validate arity (excluding self)
            let expected_params = method_def.params.len();
            if args.len() != expected_params {
                return Err(TypeError::ArityMismatch {
                    expected: expected_params,
                    found: args.len(),
                    span,
                });
            }
            // Map common return types
            return match method_def.ret_ty {
                "usize" => self.unify(ret, &Type::U64, span),
                "bool" => self.unify(ret, &Type::Bool, span),
                "()" => self.unify(ret, &Type::Unit, span),
                _ => Ok(false), // Complex return type (Option<T>, Result, etc.) - defer
            };
        }

        // Fallback for specific methods with complex types
        match method {
            "len" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "is_empty" if args.is_empty() => self.unify(ret, &Type::Bool, span),
            "push" => self.unify(ret, &Type::Unit, span),
            "pop" => {
                // Returns Option<T>
                let elem_ty = self.ctx.fresh_var();
                self.unify(ret, &Type::Option(Box::new(elem_ty)), span)
            }
            _ => Ok(false),
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

/// Type check a resolved program.
pub fn typecheck(resolved: ResolvedProgram, decls: &[Decl]) -> Result<TypedProgram, Vec<TypeError>> {
    let checker = TypeChecker::new(resolved);
    checker.check(decls)
}
