// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Pass 1: declaration collection and checking.

use rask_ast::decl::{Decl, DeclKind, EnumDecl, FnDecl, ImplDecl, StructDecl, TraitDecl, UnionDecl, TypeAliasDecl};
use rask_resolve::SymbolKind;
use super::type_defs::{TypeDef, MethodSig, SelfParam, ParamMode};
use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::parse_type::parse_type_string;
use super::TypeChecker;

use crate::types::Type;

impl TypeChecker {
    // ------------------------------------------------------------------------
    // Pass 1: Declaration Collection
    // ------------------------------------------------------------------------

    pub(super) fn collect_type_declarations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Struct(s) => self.register_struct(s),
                DeclKind::Enum(e) => self.register_enum(e),
                DeclKind::Trait(t) => self.register_trait(t),
                DeclKind::Union(u) => self.register_union(u),
                DeclKind::TypeAlias(a) => self.register_type_alias(a),
                DeclKind::Fn(f) if !f.type_params.is_empty() => {
                    // Find this function's SymbolId by matching name + Function kind
                    let type_param_names: Vec<String> = f.type_params.iter()
                        .map(|p| p.name.clone())
                        .collect();
                    if let Some(sym) = self.resolved.symbols.iter()
                        .find(|s| s.name == f.name && matches!(s.kind, SymbolKind::Function { .. }))
                    {
                        self.fn_type_params.insert(sym.id, type_param_names);
                    }
                }
                _ => {}
            }
        }
        for decl in decls {
            if let DeclKind::Impl(i) = &decl.kind {
                self.register_impl_methods(i);
            }
        }
        self.auto_derive_traits();

        // GC1/GC2: Pre-register type vars for functions with inferred params/returns
        self.pre_register_inferred_fns(decls);
    }

    /// Create fresh type vars for functions with omitted parameter types or return types.
    /// Stores them in `symbol_types` (for callers) and `inferred_fn_types` (for check_fn).
    fn pre_register_inferred_fns(&mut self, decls: &[Decl]) {
        for decl in decls {
            let fns: Vec<&FnDecl> = match &decl.kind {
                DeclKind::Fn(f) => vec![f],
                DeclKind::Struct(s) => s.methods.iter().collect(),
                DeclKind::Enum(e) => e.methods.iter().collect(),
                _ => continue,
            };
            for f in fns {
                let has_inferred_params = f.params.iter().any(|p| p.name != "self" && p.ty.is_empty());
                let has_inferred_return = f.ret_ty.is_none() && !f.is_pub && self.has_explicit_return(&f.body);
                if !has_inferred_params && !has_inferred_return {
                    continue;
                }

                // Build param type list, creating fresh vars for empty-typed params
                let mut param_vars = Vec::new();
                let mut param_types = Vec::new();
                for p in &f.params {
                    if p.name == "self" {
                        continue;
                    }
                    let ty = if p.ty.is_empty() {
                        self.ctx.fresh_var()
                    } else {
                        parse_type_string(&p.ty, &self.types).unwrap_or(Type::Error)
                    };
                    param_vars.push((p.name.clone(), ty.clone()));
                    param_types.push(ty);
                }

                let ret_ty = if has_inferred_return {
                    self.ctx.fresh_var()
                } else if let Some(t) = &f.ret_ty {
                    parse_type_string(t, &self.types).unwrap_or(Type::Error)
                } else {
                    Type::Unit
                };

                // Register in symbol_types so callers see the right type
                if let Some(sym) = self.resolved.symbols.iter()
                    .find(|s| s.name == f.name && matches!(s.kind, SymbolKind::Function { .. }))
                {
                    self.symbol_types.insert(sym.id, Type::Fn {
                        params: param_types,
                        ret: Box::new(ret_ty.clone()),
                    });
                }

                // Store for check_fn to reuse
                self.inferred_fn_types.insert(f.name.clone(), (param_vars, ret_ty));
            }
        }
    }

    pub(super) fn register_impl_methods(&mut self, i: &ImplDecl) {
        let base_name = i.target_ty.split('<').next().unwrap_or(&i.target_ty);
        let type_id = match self.types.get_type_id(base_name) {
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

    pub(super) fn register_struct(&mut self, s: &StructDecl) {
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
        let is_resource = s.attrs.iter().any(|a| a == "resource");
        self.types.register_type(TypeDef::Struct {
            name: s.name.clone(),
            type_params,
            fields,
            methods,
            is_resource,
        });
    }

    pub(super) fn register_enum(&mut self, e: &EnumDecl) {
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

    pub(super) fn register_trait(&mut self, t: &TraitDecl) {
        let methods = t.methods.iter().map(|m| self.method_signature(m)).collect();

        self.types.register_type(TypeDef::Trait {
            name: t.name.clone(),
            super_traits: t.super_traits.clone(),
            methods,
            is_unsafe: t.is_unsafe,
        });
    }

    pub(super) fn register_type_alias(&mut self, a: &TypeAliasDecl) {
        self.types.register_alias(a.name.clone(), a.target.clone());
    }

    pub(super) fn register_union(&mut self, u: &UnionDecl) {
        let fields: Vec<_> = u
            .fields
            .iter()
            .map(|f| {
                let ty = parse_type_string(&f.ty, &self.types).unwrap_or(Type::Error);
                (f.name.clone(), ty)
            })
            .collect();

        self.types.register_type(TypeDef::Union {
            name: u.name.clone(),
            fields,
        });
    }

    pub(super) fn method_signature(&self, m: &FnDecl) -> MethodSig {
        let self_param_decl = m.params.iter().find(|p| p.name == "self");
        let self_param = match self_param_decl {
            Some(p) if p.is_take => SelfParam::Take,
            Some(p) if p.is_mutate => SelfParam::Mutate,
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
                } else if p.is_mutate {
                    ParamMode::Mutate
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
    // Auto-Derive: inject synthetic methods for Equatable, Hashable, Default, Clone
    // Runs after all types and impl methods are registered.
    // ------------------------------------------------------------------------

    fn auto_derive_traits(&mut self) {
        use crate::types::TypeId;

        let type_count = self.types.types.len();
        for idx in 0..type_count {
            let id = TypeId(idx as u32);
            let def = self.types.get(id).unwrap().clone();
            match &def {
                TypeDef::Struct { fields, methods, is_resource, .. } => {
                    if *is_resource { continue; }
                    let field_types: Vec<Type> = fields.iter().map(|(_, ty)| ty.clone()).collect();
                    let mut new_methods = Vec::new();

                    // EQ1: auto-derive eq if all fields are Equatable
                    if !methods.iter().any(|m| m.name == "eq")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "eq"))
                    {
                        new_methods.push(MethodSig {
                            name: "eq".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![(Type::Named(id), ParamMode::Default)],
                            ret: Type::Bool,
                        });
                    }

                    // HA1: auto-derive hash if all fields are Hashable (requires eq too)
                    if !methods.iter().any(|m| m.name == "hash")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "hash"))
                        && field_types.iter().all(|ty| self.type_has_method(ty, "eq"))
                    {
                        new_methods.push(MethodSig {
                            name: "hash".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![],
                            ret: Type::U64,
                        });
                    }

                    // DF1: auto-derive default if all fields are Default (structs only)
                    if !methods.iter().any(|m| m.name == "default")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "default"))
                    {
                        new_methods.push(MethodSig {
                            name: "default".to_string(),
                            self_param: SelfParam::None,
                            params: vec![],
                            ret: Type::Named(id),
                        });
                    }

                    // CL1: auto-derive clone if all fields are Clone and no raw pointers (CL2)
                    if !methods.iter().any(|m| m.name == "clone")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "clone"))
                        && !field_types.iter().any(|ty| matches!(ty, Type::RawPtr(_)))
                    {
                        new_methods.push(MethodSig {
                            name: "clone".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![],
                            ret: Type::Named(id),
                        });
                    }

                    if !new_methods.is_empty() {
                        if let Some(TypeDef::Struct { methods, .. }) = self.types.get_mut(id) {
                            methods.extend(new_methods);
                        }
                    }
                }
                TypeDef::Enum { variants, methods, .. } => {
                    let payload_types: Vec<Type> = variants.iter()
                        .flat_map(|(_, fields)| fields.iter().cloned())
                        .collect();
                    let mut new_methods = Vec::new();

                    // EQ3: auto-derive eq for enums (tag + payload equality)
                    if !methods.iter().any(|m| m.name == "eq")
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "eq"))
                    {
                        new_methods.push(MethodSig {
                            name: "eq".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![(Type::Named(id), ParamMode::Default)],
                            ret: Type::Bool,
                        });
                    }

                    // HA1: auto-derive hash for enums
                    if !methods.iter().any(|m| m.name == "hash")
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "hash"))
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "eq"))
                    {
                        new_methods.push(MethodSig {
                            name: "hash".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![],
                            ret: Type::U64,
                        });
                    }

                    // DF2: enums do NOT auto-derive Default

                    // CL1: auto-derive clone for enums
                    if !methods.iter().any(|m| m.name == "clone")
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "clone"))
                        && !payload_types.iter().any(|ty| matches!(ty, Type::RawPtr(_)))
                    {
                        new_methods.push(MethodSig {
                            name: "clone".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![],
                            ret: Type::Named(id),
                        });
                    }

                    if !new_methods.is_empty() {
                        if let Some(TypeDef::Enum { methods, .. }) = self.types.get_mut(id) {
                            methods.extend(new_methods);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// Check if a type has a given method (for auto-derive field checking).
    fn type_has_method(&self, ty: &Type, method: &str) -> bool {
        match ty {
            // Primitives
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128 |
            Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => {
                matches!(method, "eq" | "hash" | "clone" | "default")
            }
            Type::F32 | Type::F64 => {
                matches!(method, "eq" | "clone" | "default")
            }
            Type::Bool | Type::Char | Type::Unit => {
                matches!(method, "eq" | "hash" | "clone" | "default")
            }
            Type::String => {
                matches!(method, "eq" | "hash" | "clone" | "default")
            }
            // Named types: check registered methods
            Type::Named(id) => {
                if let Some(def) = self.types.get(*id) {
                    match def {
                        TypeDef::Struct { methods, .. } |
                        TypeDef::Enum { methods, .. } => {
                            methods.iter().any(|m| m.name == method)
                        }
                        _ => false,
                    }
                } else {
                    false
                }
            }
            // Option/Result: delegate to inner types
            Type::Option(inner) => self.type_has_method(inner, method),
            Type::Result { ok, err } => {
                self.type_has_method(ok, method) && self.type_has_method(err, method)
            }
            // Tuples: all elements must have the method
            Type::Tuple(elems) => elems.iter().all(|e| self.type_has_method(e, method)),
            // Arrays: element must have the method
            Type::Array { elem, .. } | Type::Slice(elem) => self.type_has_method(elem, method),
            _ => false,
        }
    }

    // ------------------------------------------------------------------------
    // Pass 2: Check Declarations
    // ------------------------------------------------------------------------

    pub(super) fn check_decl(&mut self, decl: &Decl) {
        match &decl.kind {
            DeclKind::Fn(f) => self.check_fn(f),
            DeclKind::Struct(s) => {
                self.current_self_type = self.types.get_type_id(&s.name).map(Type::Named);
                for method in &s.methods {
                    self.check_fn(method);
                }
                self.current_self_type = None;
            }
            DeclKind::Enum(e) => {
                self.current_self_type = self.types.get_type_id(&e.name).map(Type::Named);
                for method in &e.methods {
                    self.check_fn(method);
                }
                self.current_self_type = None;
            }
            DeclKind::Impl(i) => {
                // UT1: implementing an unsafe trait requires `unsafe extend`
                if let Some(trait_name) = &i.trait_name {
                    if let Some(type_id) = self.types.get_type_id(trait_name) {
                        if let Some(TypeDef::Trait { is_unsafe: true, .. }) = self.types.get(type_id) {
                            if !i.is_unsafe {
                                self.errors.push(TypeError::UnsafeRequired {
                                    operation: format!("implementing unsafe trait `{}`", trait_name),
                                    span: decl.span,
                                });
                            }
                        }
                    }
                }
                self.current_self_type = self.resolve_impl_self_type(&i.target_ty);
                for method in &i.methods {
                    self.check_fn(method);
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
            DeclKind::Union(_) => {} // No methods to check
            _ => {}
        }
    }
}
