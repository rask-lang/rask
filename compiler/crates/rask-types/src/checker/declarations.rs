// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Pass 1: declaration collection and checking.

use rask_ast::decl::{Decl, DeclKind, EnumDecl, FnDecl, ImplDecl, StructDecl, TraitDecl, UnionDecl, TypeAliasDecl};
use rask_resolve::SymbolKind;
use super::type_defs::{TypeDef, MethodSig, SelfParam, ParamMode, BinaryFieldSpec, BinaryStructInfo, Endian};
use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::parse_type::parse_type_string;
use super::TypeChecker;

use crate::types::Type;
use rask_ast::Span;

impl TypeChecker {
    // ------------------------------------------------------------------------
    // Pass 1: Declaration Collection
    // ------------------------------------------------------------------------

    pub(super) fn collect_type_declarations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Struct(s) => self.register_struct(s),
                DeclKind::Enum(e) => self.register_enum(e, decl.span),
                DeclKind::Trait(t) => self.register_trait(t),
                DeclKind::Union(u) => self.register_union(u),
                DeclKind::TypeAlias(a) => self.register_type_alias(a, decl.span),
                DeclKind::Fn(f) if !f.type_params.is_empty() => {
                    // Find this function's SymbolId by matching name + Function kind.
                    // Strip generic suffix: parser stores "foo<T: Trait>" but resolver
                    // registers the base name "foo".
                    let base_name = f.name.split('<').next().unwrap_or(&f.name);
                    let type_param_names: Vec<String> = f.type_params.iter()
                        .map(|p| p.name.clone())
                        .collect();
                    if let Some(sym) = self.resolved.symbols.iter()
                        .find(|s| s.name == base_name && matches!(s.kind, SymbolKind::Function { .. }))
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
        self.propagate_uniqueness();
        self.auto_derive_traits();
        self.register_binary_methods();

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
                let has_inferred_error = f.ret_ty.as_ref().is_some_and(|t| t.ends_with(", _>"));
                let has_inferred_return = (f.ret_ty.is_none() && !f.is_pub && self.has_explicit_return(&f.body))
                    || has_inferred_error;
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

                let ret_ty = if has_inferred_error {
                    // "Result<Config, _>" → Result { ok: Config, err: fresh_var }
                    let t = f.ret_ty.as_ref().unwrap();
                    let ok_str = &t["Result<".len()..t.len() - ", _>".len()];
                    let ok_ty = parse_type_string(ok_str, &self.types).unwrap_or(Type::Error);
                    Type::Result {
                        ok: Box::new(ok_ty),
                        err: Box::new(self.ctx.fresh_var()),
                    }
                } else if has_inferred_return {
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

        // V5: collect private field names for access checking
        let private_fields: Vec<String> = s
            .fields
            .iter()
            .filter(|f| f.visibility == rask_ast::decl::FieldVisibility::Private)
            .map(|f| f.name.clone())
            .collect();

        let methods = s.methods.iter().map(|m| self.method_signature(m)).collect();

        let type_params: Vec<String> = s.type_params.iter().map(|p| p.name.clone()).collect();
        let is_resource = s.attrs.iter().any(|a| a == "resource");
        let is_unique = s.attrs.iter().any(|a| a == "unique");
        let is_binary = s.attrs.iter().any(|a| a == "binary");

        // For @binary structs, convert binary field specifiers to runtime types
        let (fields, binary_info) = if is_binary {
            let result = parse_binary_struct_fields(&s.name, &s.fields);
            match result {
                Ok((typed_fields, info)) => {
                    (typed_fields, Some(info))
                }
                Err(errors) => {
                    for err in errors {
                        self.errors.push(err);
                    }
                    (fields, None)
                }
            }
        } else {
            (fields, None)
        };

        let type_id = self.types.register_type(TypeDef::Struct {
            name: s.name.clone(),
            type_params,
            fields,
            methods,
            is_resource,
            is_unique,
            is_binary,
            private_fields,
        });

        if let Some(info) = binary_info {
            self.types.register_binary_info(type_id, info);
        }
    }

    pub(super) fn register_enum(&mut self, e: &EnumDecl, span: Span) {
        // E16: If any variant has an explicit discriminant, all must
        let has_disc = e.variants.iter().any(|v| v.discriminant.is_some());
        let all_disc = e.variants.iter().all(|v| v.discriminant.is_some());
        if has_disc && !all_disc && !e.variants.is_empty() {
            self.errors.push(TypeError::MixedDiscriminants {
                enum_name: e.name.clone(),
                span,
            });
        }

        // E17: Explicit discriminants cannot have payload variants
        if has_disc {
            for v in &e.variants {
                if !v.fields.is_empty() {
                    self.errors.push(TypeError::DiscriminantWithPayload {
                        enum_name: e.name.clone(),
                        variant: v.name.clone(),
                        span,
                    });
                }
            }
        }

        // E15: Discriminant values must be unique
        if has_disc {
            let mut seen = std::collections::HashMap::new();
            for v in &e.variants {
                if let Some(val) = v.discriminant {
                    if let Some(prev) = seen.insert(val, v.name.clone()) {
                        self.errors.push(TypeError::DuplicateDiscriminant {
                            enum_name: e.name.clone(),
                            value: val,
                            first: prev,
                            second: v.name.clone(),
                            span,
                        });
                    }
                }
            }
        }

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

    pub(super) fn register_type_alias(&mut self, a: &TypeAliasDecl, span: rask_ast::Span) {
        if a.is_transparent {
            // T6: check for cycles before registering
            if let Some(path) = self.types.check_alias_cycle(&a.name, &a.target) {
                self.errors.push(TypeError::CyclicTypeAlias {
                    cycle: path.join(" → "),
                    span,
                });
                return;
            }
            self.types.register_alias(a.name.clone(), a.target.clone());
        } else {
            // `type X = Y` — nominal, gets its own TypeId
            let underlying = parse_type_string(&a.target, &self.types).unwrap_or(Type::Error);
            self.types.register_type(TypeDef::NominalAlias {
                name: a.name.clone(),
                underlying,
                with_traits: a.with_traits.clone(),
            });
        }
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
            Some(_) => {
                // GC9: Infer mutate for private methods that write self fields
                if !m.is_pub && Self::body_writes_self(&m.body) {
                    SelfParam::Mutate
                } else {
                    SelfParam::Value
                }
            }
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
    // U4: Transitive uniqueness — struct containing @unique field is itself unique.
    // Fixed-point iteration: propagate until no changes.
    // ------------------------------------------------------------------------

    fn propagate_uniqueness(&mut self) {
        use crate::types::TypeId;

        loop {
            let mut changed = false;
            let type_count = self.types.types.len();
            for idx in 0..type_count {
                let id = TypeId(idx as u32);
                let def = self.types.get(id).unwrap().clone();
                if let TypeDef::Struct { fields, is_unique, .. } = &def {
                    if *is_unique { continue; }
                    let has_unique_field = fields.iter().any(|(_, ty)| {
                        match ty {
                            Type::Named(field_id) => self.types.is_unique_type_by_id(*field_id),
                            Type::Generic { base, .. } => self.types.is_unique_type_by_id(*base),
                            _ => false,
                        }
                    });
                    if has_unique_field {
                        if let Some(TypeDef::Struct { is_unique, .. }) = self.types.get_mut(id) {
                            *is_unique = true;
                            changed = true;
                        }
                    }
                }
            }
            if !changed { break; }
        }
    }

    // ------------------------------------------------------------------------
    // Auto-Derive: inject synthetic methods for Equal, Hashable, Default, Clone, Comparable, Debug
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

                    // CO1/ORD2: auto-derive compare if all fields are Comparable
                    // Comparable is a supertrait of Equal, so eq is implied.
                    // CO4: f32/f64 excluded (NaN breaks totality).
                    if !methods.iter().any(|m| m.name == "compare")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "compare"))
                    {
                        let ordering_ty = Type::UnresolvedNamed("Ordering".to_string());
                        new_methods.push(MethodSig {
                            name: "compare".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![(Type::Named(id), ParamMode::Default)],
                            ret: ordering_ty.clone(),
                        });
                        // ORD1: lt/le/gt/ge derived from compare
                        for op in &["lt", "le", "gt", "ge"] {
                            if !methods.iter().any(|m| m.name == *op) {
                                new_methods.push(MethodSig {
                                    name: op.to_string(),
                                    self_param: SelfParam::Value,
                                    params: vec![(Type::Named(id), ParamMode::Default)],
                                    ret: Type::Bool,
                                });
                            }
                        }
                    }

                    // G2: auto-derive debug_string for all types
                    if !methods.iter().any(|m| m.name == "debug_string") {
                        new_methods.push(MethodSig {
                            name: "debug_string".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![],
                            ret: Type::String,
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

                    // CO1/ORD2: auto-derive compare for enums (variant order, then payload)
                    if !methods.iter().any(|m| m.name == "compare")
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "compare"))
                    {
                        let ordering_ty = Type::UnresolvedNamed("Ordering".to_string());
                        new_methods.push(MethodSig {
                            name: "compare".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![(Type::Named(id), ParamMode::Default)],
                            ret: ordering_ty.clone(),
                        });
                        // ORD1: lt/le/gt/ge derived from compare
                        for op in &["lt", "le", "gt", "ge"] {
                            if !methods.iter().any(|m| m.name == *op) {
                                new_methods.push(MethodSig {
                                    name: op.to_string(),
                                    self_param: SelfParam::Value,
                                    params: vec![(Type::Named(id), ParamMode::Default)],
                                    ret: Type::Bool,
                                });
                            }
                        }
                    }

                    // G2: auto-derive debug_string for all types
                    if !methods.iter().any(|m| m.name == "debug_string") {
                        new_methods.push(MethodSig {
                            name: "debug_string".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![],
                            ret: Type::String,
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
                matches!(method, "eq" | "hash" | "clone" | "default" | "compare" | "debug_string")
            }
            // CO4: f32/f64 NOT Comparable (NaN breaks totality)
            Type::F32 | Type::F64 => {
                matches!(method, "eq" | "clone" | "default" | "debug_string")
            }
            Type::Bool | Type::Char => {
                matches!(method, "eq" | "hash" | "clone" | "default" | "compare" | "debug_string")
            }
            Type::Unit => {
                matches!(method, "eq" | "hash" | "clone" | "default" | "debug_string")
            }
            Type::String => {
                matches!(method, "eq" | "hash" | "clone" | "default" | "compare" | "debug_string")
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
                let (init_ty, declared_ty) = if let Some(ty_str) = &c.ty {
                    if let Ok(declared) = parse_type_string(ty_str, &self.types) {
                        let init_ty = self.infer_expr_expecting(&c.init, &declared);
                        (init_ty, Some(declared))
                    } else {
                        (self.infer_expr(&c.init), None)
                    }
                } else {
                    (self.infer_expr(&c.init), None)
                };
                if let Some(declared) = declared_ty {
                    self.ctx.add_constraint(TypeConstraint::Equal(
                        declared.clone(),
                        init_ty,
                        decl.span,
                    ));
                    self.define_local(c.name.clone(), declared);
                } else {
                    self.define_local(c.name.clone(), init_ty);
                }
                // ESAD Phase 2: reject volatile views at module level too
                self.check_view_at_binding(&c.name, &c.init, decl.span);
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
                    let pkg_name = &imp.path[0];
                    let module_name = imp.alias.as_ref().unwrap_or(pkg_name).clone();

                    // Register public types from external packages so
                    // qualified access (pkg.Type) resolves through the type table.
                    if let Some(ext_decls) = self.resolved.external_decls.get(pkg_name).cloned() {
                        for ext_decl in &ext_decls {
                            match &ext_decl.kind {
                                DeclKind::Struct(s) => self.register_struct(s),
                                DeclKind::Enum(e) => self.register_enum(e, ext_decl.span),
                                DeclKind::Trait(t) => self.register_trait(t),
                                DeclKind::TypeAlias(a) => self.register_type_alias(a, ext_decl.span),
                                _ => {}
                            }
                        }
                    }

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

    /// G1–G4: Register parse/build/build_into methods and SIZE/SIZE_BITS for @binary structs.
    fn register_binary_methods(&mut self) {
        use crate::types::TypeId;

        let type_count = self.types.types.len();
        for idx in 0..type_count {
            let id = TypeId(idx as u32);
            if !self.types.is_binary_type_by_id(id) {
                continue;
            }

            let struct_type = Type::Named(id);

            // G1: parse(data: []u8) -> (T, []u8) or ParseError
            let parse_result = Type::Result {
                ok: Box::new(Type::Tuple(vec![
                    struct_type.clone(),
                    Type::Slice(Box::new(Type::U8)),
                ])),
                err: Box::new(Type::UnresolvedNamed("ParseError".to_string())),
            };

            // G2: build(self) -> Vec<u8>
            let vec_u8 = Type::UnresolvedGeneric {
                name: "Vec".to_string(),
                args: vec![crate::types::GenericArg::Type(Box::new(Type::U8))],
            };

            // G3: build_into(self, buffer: []u8) -> usize or BuildError
            let build_into_result = Type::Result {
                ok: Box::new(Type::U64), // usize
                err: Box::new(Type::UnresolvedNamed("BuildError".to_string())),
            };

            let mut methods = vec![
                MethodSig {
                    name: "parse".to_string(),
                    self_param: SelfParam::None,
                    params: vec![(Type::Slice(Box::new(Type::U8)), ParamMode::Default)],
                    ret: parse_result,
                },
                MethodSig {
                    name: "build".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![],
                    ret: vec_u8,
                },
                MethodSig {
                    name: "build_into".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![(Type::Slice(Box::new(Type::U8)), ParamMode::Mutate)],
                    ret: build_into_result,
                },
            ];

            if let Some(TypeDef::Struct { methods: existing, .. }) = self.types.get_mut(id) {
                existing.append(&mut methods);
            }
        }
    }
}

/// Parse a binary field type specifier and return (bits, endian, runtime_type).
fn parse_binary_field_spec(ty_str: &str) -> Result<(u32, Option<Endian>, Type, bool, usize), String> {
    let s = ty_str.trim();

    // [N]u8 — fixed byte array
    if s.starts_with('[') {
        let bracket_end = s.find(']').ok_or_else(|| format!("invalid binary type: {}", s))?;
        let count_str = &s[1..bracket_end];
        let elem_str = &s[bracket_end + 1..];
        if elem_str != "u8" {
            return Err(format!("binary byte arrays only support u8, found [{}]{}", count_str, elem_str));
        }
        let count: usize = count_str.parse()
            .map_err(|_| format!("invalid byte array count: {}", count_str))?;
        let bits = (count as u32) * 8;
        return Ok((bits, None, Type::Array { elem: Box::new(Type::U8), len: count }, true, count));
    }

    // Bare number — N bits
    if let Ok(n) = s.parse::<u32>() {
        if n == 0 || n > 64 {
            return Err(format!("bit count must be >= 1 and <= 64, found {}", n));
        }
        let runtime_type = match n {
            1..=8 => Type::U8,
            9..=16 => Type::U16,
            17..=32 => Type::U32,
            33..=64 => Type::U64,
            _ => unreachable!(),
        };
        return Ok((n, None, runtime_type, false, 0));
    }

    // Endian types: u16be, i32le, f64be, etc.
    let (base, endian) = if let Some(base) = s.strip_suffix("be") {
        (base, Endian::Big)
    } else if let Some(base) = s.strip_suffix("le") {
        (base, Endian::Little)
    } else {
        // Non-endian types: u8, i8
        return match s {
            "u8" => Ok((8, None, Type::U8, false, 0)),
            "i8" => Ok((8, None, Type::I8, false, 0)),
            _ => Err(format!("multi-byte field '{}' must specify endianness (be/le)", s)),
        };
    };

    let (bits, runtime_type) = match base {
        "u16" => (16, Type::U16),
        "i16" => (16, Type::I16),
        "u32" => (32, Type::U32),
        "i32" => (32, Type::I32),
        "u64" => (64, Type::U64),
        "i64" => (64, Type::I64),
        "f32" => (32, Type::F32),
        "f64" => (64, Type::F64),
        _ => return Err(format!("unknown binary type: {}", s)),
    };

    Ok((bits, Some(endian), runtime_type, false, 0))
}

/// B1–V4: Parse and validate all fields of a @binary struct.
fn parse_binary_struct_fields(
    struct_name: &str,
    fields: &[rask_ast::decl::Field],
) -> Result<(Vec<(String, Type)>, BinaryStructInfo), Vec<TypeError>> {
    let mut errors = Vec::new();
    let mut typed_fields = Vec::new();
    let mut binary_fields = Vec::new();
    let mut bit_offset: u32 = 0;

    for field in fields {
        match parse_binary_field_spec(&field.ty) {
            Ok((bits, endian, runtime_type, is_byte_array, byte_array_len)) => {
                // F3: multi-byte endian types must be byte-aligned
                if endian.is_some() && bits > 8 && (bit_offset % 8) != 0 {
                    errors.push(TypeError::GenericError(
                        format!(
                            "[type.binary/F3] endian type '{}' not byte-aligned: starts at bit {}, not a byte boundary",
                            field.ty, bit_offset
                        ),
                        field.name_span,
                    ));
                }

                // V1: bit count range
                if !is_byte_array && (bits == 0 || bits > 64) {
                    errors.push(TypeError::GenericError(
                        format!("[type.binary/V1] invalid bit count: {} (must be 1-64)", bits),
                        field.name_span,
                    ));
                }

                typed_fields.push((field.name.clone(), runtime_type.clone()));
                binary_fields.push(BinaryFieldSpec {
                    name: field.name.clone(),
                    bits,
                    endian,
                    runtime_type,
                    bit_offset,
                    is_byte_array,
                    byte_array_len,
                });
                bit_offset += bits;
            }
            Err(msg) => {
                errors.push(TypeError::GenericError(msg, field.name_span));
                typed_fields.push((field.name.clone(), Type::Error));
            }
        }
    }

    // V3: total size limit (65535 bits = 8KB)
    if bit_offset > 65535 {
        errors.push(TypeError::GenericError(
            format!(
                "[type.binary/V3] total size {} bits exceeds 65535-bit limit (8KB)",
                bit_offset
            ),
            Span::new(0, 0),
        ));
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    let size_bytes = (bit_offset + 7) / 8;
    let info = BinaryStructInfo {
        name: struct_name.to_string(),
        fields: binary_fields,
        total_bits: bit_offset,
        size_bytes,
    };

    Ok((typed_fields, info))
}
