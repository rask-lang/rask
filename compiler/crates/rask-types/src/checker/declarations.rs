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
                DeclKind::Struct(s) => {
                    self.check_declared_type_name(&s.name, "struct", decl.span);
                    let id = self.register_struct(s);
                    self.types.record_method_decl(id, decl.id);
                }
                DeclKind::Enum(e) => {
                    self.check_declared_type_name(&e.name, "enum", decl.span);
                    let id = self.register_enum(e, decl.span);
                    self.types.record_method_decl(id, decl.id);
                }
                DeclKind::Trait(t) => {
                    self.check_declared_type_name(&t.name, "trait", decl.span);
                    // DT1: shape-matching stops at the package boundary
                    if t.is_pub && t.is_duck {
                        self.errors.push(TypeError::PublicDuckTrait {
                            name: t.name.clone(),
                            span: decl.span,
                        });
                    }
                    self.register_trait(t);
                }
                DeclKind::Union(u) => {
                    self.check_declared_type_name(&u.name, "union", decl.span);
                    self.register_union(u);
                }
                // AN6: annotations register as nominal struct types so
                // `field.has<validate>()` can name them as type arguments.
                // The restricted shape (AN1) is enforced in annotations.rs.
                DeclKind::Annotation(a) => {
                    self.check_declared_type_name(&a.name, "annotation", decl.span);
                    self.annotation_types.insert(a.name.clone());
                    let s = rask_ast::decl::StructDecl {
                        name: a.name.clone(),
                        type_params: vec![],
                        fields: a.fields.clone(),
                        methods: vec![],
                        is_pub: a.is_pub,
                        attrs: vec![],
                        doc: a.doc.clone(),
                    };
                    let id = self.register_struct(&s);
                    self.types.record_method_decl(id, decl.id);
                }
                DeclKind::TypeAlias(a) => {
                    self.check_declared_type_name(&a.name, "type alias", decl.span);
                    self.register_type_alias(a, decl.span);
                }
                // `const W = 4` then `[i32; W]`. The length has to be known
                // before any declared type is parsed, so it's recorded in this
                // pass rather than where the const is checked (#906).
                DeclKind::Const(c) => {
                    if let rask_ast::expr::ExprKind::Int(n, _) = &c.init.kind {
                        if let Ok(len) = usize::try_from(*n) {
                            self.types.register_const_length(c.name.clone(), len);
                        }
                    }
                }
                DeclKind::Fn(f) => {
                    // Find this function's SymbolId by matching name + Function kind.
                    // Strip generic suffix: parser stores "foo<T: Trait>" but resolver
                    // registers the base name "foo".
                    let base_name = f.name.split('<').next().unwrap_or(&f.name);
                    // PC1: explicit <T> declarations plus implicit single-letter
                    // type params from the signature.
                    let type_param_names = signature_type_param_names(f);
                    if !type_param_names.is_empty() {
                        if let Some(sym) = self.resolved.symbols.iter()
                            .find(|s| s.name == base_name && matches!(s.kind, SymbolKind::Function { .. }))
                        {
                            self.fn_type_params.insert(sym.id, type_param_names);
                            // #314: record bounds so call sites can verify the
                            // type arg satisfies the declared trait bounds.
                            let bounds: std::collections::HashMap<String, Vec<String>> = f.type_params.iter()
                                .filter(|tp| !tp.bounds.is_empty())
                                .map(|tp| (tp.name.clone(), tp.bounds.clone()))
                                .collect();
                            if !bounds.is_empty() {
                                self.fn_type_param_bounds.insert(sym.id, bounds);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        for decl in decls {
            if let DeclKind::Impl(i) = &decl.kind {
                self.register_impl_methods(i, decl.id);
            }
        }
        // ER3/ER4: validate `T or E` in declared field/payload/target types now
        // that all `extend` methods are registered — an error type's `message()`
        // may be defined in an `extend` block declared after the type that uses it.
        let pending = std::mem::take(&mut self.pending_result_validations);
        for (ty, span) in pending {
            self.validate_result_types_in(&ty, span);
        }
        self.propagate_uniqueness();
        self.propagate_resource_linearity();
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

    /// PC3: single uppercase letters are reserved for type parameters —
    /// declaring a concrete type with one would make signatures ambiguous.
    /// E19/E21: serialization annotations the compiler has to be able to act on.
    ///
    /// `@rename` takes a string literal and `@default` a comptime expression,
    /// both checked here rather than at the encode site — an annotation the
    /// compiler silently ignores is worse than one it rejects, because the wire
    /// format then differs from what the source says.
    ///
    /// `@skip` is the old spelling of `@no_serialize`. It's recognized only to
    /// say so: left alone it reads as "excluded" and serializes the field.
    fn check_field_annotations(&mut self, s: &rask_ast::decl::StructDecl) {
        use rask_ast::decl::field_attrs;
        for field in &s.fields {
            if field_attrs::uses_old_skip(&field.attrs) {
                self.errors.push(TypeError::BadFieldAnnotation {
                    attr: "skip".to_string(),
                    field: field.name.clone(),
                    problem: "skip from what? the name didn't say, so it was renamed"
                        .to_string(),
                    fix: "@no_serialize".to_string(),
                    span: field.name_span,
                });
            }
            for attr in &field.attrs {
                let attr = attr.trim();
                let Some(arg) = attr.strip_prefix("rename") else { continue };
                let inner = arg.trim_start().strip_prefix('(').and_then(|a| a.strip_suffix(')'));
                let names_a_key = inner
                    .map(str::trim)
                    .is_some_and(|a| a.starts_with('"') && a.ends_with('"') && a.len() >= 2);
                if !names_a_key {
                    self.errors.push(TypeError::BadFieldAnnotation {
                        attr: "rename".to_string(),
                        field: field.name.clone(),
                        problem: "the serialized key has to be a string literal"
                            .to_string(),
                        fix: format!("@rename(\"{}\")", field.name),
                        span: field.name_span,
                    });
                }
            }
        }
    }

    fn check_declared_type_name(&mut self, name: &str, kind: &str, span: Span) {
        if is_type_param_name(name) {
            self.errors.push(TypeError::SingleLetterTypeName {
                name: name.to_string(),
                kind: kind.to_string(),
                span,
            });
        }
    }

    pub(super) fn register_impl_methods(&mut self, i: &ImplDecl, decl_id: rask_ast::NodeId) {
        let base_name = i.target_ty.split('<').next().unwrap_or(&i.target_ty);
        let type_id = match self.types.get_type_id(base_name) {
            Some(id) => id,
            None => return,
        };
        self.types.record_method_decl(type_id, decl_id);
        // G1: record each declared conformance. `scoped` methods stay out of the
        // inherent namespace (MN4) but the conformance is still declared.
        // CC1/CC2: a `where` clause makes every listed conformance conditional
        // (CD3: one condition per block).
        let condition: Vec<(String, Vec<String>)> = i.where_bounds.iter()
            .map(|tp| (tp.name.clone(), tp.bounds.clone()))
            .collect();
        for trait_name in &i.trait_names {
            self.types.record_conformance(type_id, trait_name);
            if !condition.is_empty() {
                self.types.record_conformance_condition(type_id, trait_name, condition.clone());
            }
        }
        let new_methods: Vec<_> = i.methods.iter().map(|m| self.method_signature(m)).collect();
        if let Some(def) = self.types.get_mut(type_id) {
            match def {
                TypeDef::Struct { methods, .. }
                | TypeDef::Enum { methods, .. }
                | TypeDef::NominalAlias { methods, .. } => {
                    methods.extend(new_methods);
                }
                _ => {}
            }
        }
    }

    pub(super) fn register_struct(&mut self, s: &StructDecl) -> crate::types::TypeId {
        // The declaration's own parameters win over types of the same name for
        // as long as its field types are being parsed (#915).
        let type_params = struct_type_param_names(s);
        let outer_params = self.types.push_type_params(type_params.clone());
        let field_tys: Vec<(Span, Type)> = s
            .fields
            .iter()
            .map(|f| {
                let ty = parse_type_string(&f.ty, &self.types).unwrap_or(Type::Error);
                (f.name_span, ty)
            })
            .collect();
        self.types.pop_type_params(outer_params);
        // ER3/ER4: validate nested `T or E` in field types (deferred — see
        // pending_result_validations; extend-defined `message()` must be visible).
        for (fspan, fty) in &field_tys {
            self.pending_result_validations.push((fty.clone(), *fspan));
            // RC1/RC3: a `Vec`/`Map` field can't hold linear elements.
            self.note_linear_container_site(*fspan, fty.clone());
        }
        let fields: Vec<_> = s
            .fields
            .iter()
            .zip(field_tys.into_iter())
            .map(|(f, (_, ty))| (f.name.clone(), ty))
            .collect();

        // V5: collect private field names for access checking
        let private_fields: Vec<String> = s
            .fields
            .iter()
            .filter(|f| f.visibility == rask_ast::decl::FieldVisibility::Private)
            .map(|f| f.name.clone())
            .collect();

        self.check_field_annotations(s);

        // E19: a `@no_serialize` field never reaches the wire, so it can't
        // disqualify the type from Encode/Decode.
        let skipped_fields: Vec<String> = s
            .fields
            .iter()
            .filter(|f| rask_ast::decl::field_attrs::is_skipped(&f.attrs))
            .map(|f| f.name.clone())
            .collect();

        // E13a: a field the wire form leaves out still needs a value on decode.
        // Its declared default (`type.structs/FD1`, FD6) or an `@default(expr)`
        // override supplies one; with neither there's nothing to build the
        // field from, so the type isn't auto-`Decode`.
        let undecodable_fields: Vec<String> = s
            .fields
            .iter()
            .filter(|f| {
                f.visibility == rask_ast::decl::FieldVisibility::Private
                    || rask_ast::decl::field_attrs::is_skipped(&f.attrs)
            })
            .filter(|f| {
                f.default.is_none()
                    && rask_ast::decl::field_attrs::default_literal(&f.attrs).is_none()
            })
            .map(|f| f.name.clone())
            .collect();

        let methods = s.methods.iter().map(|m| self.method_signature(m)).collect();

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
            skipped_fields,
            undecodable_fields,
            // ER42/L1: refined by `propagate_resource_linearity` after all
            // declarations are collected. @resource is the seed; transitive
            // linearity propagates from there.
            is_transitive_resource: is_resource,
        });

        if let Some(info) = binary_info {
            self.types.register_binary_info(type_id, info);
        }
        type_id
    }

    pub(super) fn register_enum(&mut self, e: &EnumDecl, span: Span) -> crate::types::TypeId {
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

        // E24: `@tag` is only meaningful if every payload can carry the tag
        // alongside its own fields. Both failures are decidable from the
        // declaration alone — no value has to reach json.encode for the
        // shape to be wrong — so they are rejected here rather than lowered
        // into a runtime panic (ctrl.panic/S7).
        if let Some(tag) = rask_ast::decl::field_attrs::tag_field(&e.attrs) {
            for v in &e.variants {
                // An unnamed payload has no field name to flatten into: the
                // object already holds the tag, and the payload would need an
                // invented key to sit beside it.
                if v.fields.len() == 1 && v.fields[0].name == "_0" {
                    self.errors.push(TypeError::TagOnUnnamedPayload {
                        enum_name: e.name.clone(),
                        variant: v.name.clone(),
                        tag: tag.clone(),
                        span: v.name_span,
                    });
                } else if v.fields.iter().any(|f| f.name == tag) {
                    // Writing both would mean a duplicate JSON key.
                    self.errors.push(TypeError::TagCollidesWithField {
                        enum_name: e.name.clone(),
                        variant: v.name.clone(),
                        tag: tag.clone(),
                        span: v.name_span,
                    });
                }
            }
        }

        // Parse variant payload types first (immutable borrow of self.types),
        // then validate (mutable borrow of self for errors).
        let variants: Vec<(String, Vec<(Span, Type)>)> = e
            .variants
            .iter()
            .map(|v| {
                let field_types: Vec<(Span, Type)> = v
                    .fields
                    .iter()
                    .map(|f| {
                        let ty = parse_type_string(&f.ty, &self.types).unwrap_or(Type::Error);
                        (f.name_span, ty)
                    })
                    .collect();
                (v.name.clone(), field_types)
            })
            .collect();
        // ER3/ER4: validate nested `T or E` in variant payload types (deferred).
        for (_, field_types) in &variants {
            for (fspan, fty) in field_types {
                self.pending_result_validations.push((fty.clone(), *fspan));
                // RC1/RC3: a `Vec`/`Map` payload can't hold linear elements.
                self.note_linear_container_site(*fspan, fty.clone());
            }
        }
        let variants: Vec<_> = variants
            .into_iter()
            .map(|(vname, fts)| (vname, fts.into_iter().map(|(_, t)| t).collect::<Vec<_>>()))
            .collect();

        let methods = e.methods.iter().map(|m| self.method_signature(m)).collect();

        // Field names for the struct-shaped variants, so a pattern that names
        // them can be matched against the positional payload types (#809).
        let variant_names: Vec<(String, Vec<String>)> = e
            .variants
            .iter()
            .map(|v| {
                (
                    v.name.clone(),
                    v.fields.iter().map(|f| f.name.clone()).collect(),
                )
            })
            .collect();

        // PC1: explicit `<T>` plus single letters appearing in payload types.
        let type_params = enum_type_param_names(e);
        let enum_id = self.types.register_type(TypeDef::Enum {
            name: e.name.clone(),
            type_params,
            variants,
            methods,
            // ER42/L1: refined by `propagate_resource_linearity` once all
            // declarations are visible.
            is_transitive_resource: false,
        });
        for (variant, field_names) in variant_names {
            self.types
                .variant_field_names
                .insert((enum_id, variant), field_names);
        }
        enum_id
    }

    pub(super) fn register_trait(&mut self, t: &TraitDecl) {
        let methods = t.methods.iter().map(|m| self.method_signature(m)).collect();
        let generic_methods = t.methods.iter()
            .filter(|m| !m.type_params.is_empty())
            .map(|m| m.name.clone())
            .collect();

        self.types.register_type(TypeDef::Trait {
            name: t.name.clone(),
            super_traits: t.super_traits.clone(),
            methods,
            generic_methods,
            is_unsafe: t.is_unsafe,
            is_duck: t.is_duck,
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
            // RC1/RC3: `alias Files = Vec<File>` is itself a rejected type.
            if let Ok(target) = parse_type_string(&a.target, &self.types) {
                self.note_linear_container_site(span, target);
            }
        } else {
            // `type X = Y` — nominal, gets its own TypeId
            let underlying = parse_type_string(&a.target, &self.types).unwrap_or(Type::Error);
            // ER3/ER4: validate nested `T or E` in the alias target (deferred).
            self.pending_result_validations.push((underlying.clone(), span));
            // RC1/RC3: a nominal alias to a `Vec`/`Map` of linear values.
            self.note_linear_container_site(span, underlying.clone());
            self.types.register_type(TypeDef::NominalAlias {
                name: a.name.clone(),
                underlying,
                with_traits: a.with_traits.clone(),
                methods: Vec::new(),
            });
        }
    }

    pub(super) fn register_union(&mut self, u: &UnionDecl) {
        let field_tys: Vec<(String, Span, Type)> = u
            .fields
            .iter()
            .map(|f| {
                let ty = parse_type_string(&f.ty, &self.types).unwrap_or(Type::Error);
                (f.name.clone(), f.name_span, ty)
            })
            .collect();
        // ER3/ER4: validate nested `T or E` in union field types (deferred).
        for (_, fspan, fty) in &field_tys {
            self.pending_result_validations.push((fty.clone(), *fspan));
        }
        let fields: Vec<_> = field_tys.into_iter().map(|(n, _, t)| (n, t)).collect();

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
            // The parser folds `<E>` into the declared name for display, so the
            // stored name is `tag<E>`. Method lookup compares against what the
            // call site writes — `tag` — so strip it back off and keep the
            // parameters where they can be instantiated.
            name: m.name.split('<').next().unwrap_or(&m.name).to_string(),
            self_param,
            params,
            ret,
            type_params: m.type_params.iter()
                .map(|tp| (tp.name.clone(), tp.bounds.clone()))
                .collect(),
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
    // ER42/L1: Transitive linearity — a struct/enum that contains a linear
    // field (directly or via nested struct/enum/tuple/etc.) is itself linear.
    // Drives ownership-checker consumption obligations and the ER43 wildcard
    // ban during pattern matching.
    // ------------------------------------------------------------------------

    fn propagate_resource_linearity(&mut self) {
        use crate::types::TypeId;

        loop {
            let mut changed = false;
            let type_count = self.types.types.len();
            for idx in 0..type_count {
                let id = TypeId(idx as u32);
                let def = self.types.get(id).unwrap().clone();
                match &def {
                    TypeDef::Struct { fields, is_transitive_resource, .. } => {
                        if *is_transitive_resource { continue; }
                        let has_linear_field = fields.iter().any(|(_, ty)| {
                            self.types.type_is_transitive_resource(ty)
                        });
                        if has_linear_field {
                            if let Some(TypeDef::Struct { is_transitive_resource, .. }) = self.types.get_mut(id) {
                                *is_transitive_resource = true;
                                changed = true;
                            }
                        }
                    }
                    TypeDef::Enum { variants, is_transitive_resource, .. } => {
                        if *is_transitive_resource { continue; }
                        let has_linear_payload = variants.iter().any(|(_, fts)| {
                            fts.iter().any(|ty| self.types.type_is_transitive_resource(ty))
                        });
                        if has_linear_payload {
                            if let Some(TypeDef::Enum { is_transitive_resource, .. }) = self.types.get_mut(id) {
                                *is_transitive_resource = true;
                                changed = true;
                            }
                        }
                    }
                    _ => {}
                }
            }
            if !changed { break; }
        }
    }

    // ------------------------------------------------------------------------
    // Auto-Derive: inject synthetic methods for Equal, Hashable, Default, Clone, Comparable, Debug
    // Runs after all types and impl methods are registered.
    // ------------------------------------------------------------------------

    /// The type as its own methods see it. A generic type's derived signatures
    /// have to name the parameters (`Pair<T>`, not `Pair`), or the substitution at
    /// the call site has nothing to replace and the argument is expected at the
    /// bare type constructor. `h == kept` on two `Handle<Entity>` values then
    /// reported "expected `Handle`, found `Handle<Entity>`" — the derived `eq`
    /// took its parameter as bare `Handle` (#661).
    fn self_type_with_params(id: crate::types::TypeId, type_params: &[String]) -> Type {
        if type_params.is_empty() {
            return Type::Named(id);
        }
        Type::Generic {
            base: id,
            args: type_params
                .iter()
                .map(|p| crate::types::GenericArg::Type(Box::new(Type::UnresolvedNamed(p.clone()))))
                .collect(),
        }
    }

    fn auto_derive_traits(&mut self) {
        use crate::types::TypeId;

        let type_count = self.types.types.len();
        for idx in 0..type_count {
            let id = TypeId(idx as u32);
            let def = self.types.get(id).unwrap().clone();
            match &def {
                TypeDef::Struct { fields, methods, is_resource, type_params, .. } => {
                    if *is_resource { continue; }
                    let field_types: Vec<Type> = fields.iter().map(|(_, ty)| ty.clone()).collect();
                    let self_ty = Self::self_type_with_params(id, type_params);
                    let mut new_methods = Vec::new();

                    // EQ1: auto-derive eq if all fields are Equatable
                    if !methods.iter().any(|m| m.name == "eq")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "eq"))
                    {
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
                            name: "eq".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![(self_ty.clone(), ParamMode::Default)],
                            ret: Type::Bool,
                        });
                    }

                    // HA1: auto-derive hash if all fields are Hashable (requires eq too)
                    if !methods.iter().any(|m| m.name == "hash")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "hash"))
                        && field_types.iter().all(|ty| self.type_has_method(ty, "eq"))
                    {
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
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
                            type_params: Vec::new(),
                            name: "default".to_string(),
                            self_param: SelfParam::None,
                            params: vec![],
                            ret: self_ty.clone(),
                        });
                    }

                    // CL1: auto-derive clone if all fields are Clone and no raw pointers (CL2)
                    if !methods.iter().any(|m| m.name == "clone")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "clone"))
                        && !field_types.iter().any(|ty| matches!(ty, Type::RawPtr(_)))
                    {
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
                            name: "clone".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![],
                            ret: self_ty.clone(),
                        });
                    }

                    // CO1/ORD2: auto-derive compare if all fields are Comparable
                    // Comparable is a supertrait of Equal, so eq is implied.
                    // CO4: f32/f64 excluded (NaN breaks totality).
                    if !methods.iter().any(|m| m.name == "compare")
                        && field_types.iter().all(|ty| self.type_has_method(ty, "compare"))
                    {
                        let ordering_ty = self.ordering_type();
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
                            name: "compare".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![(self_ty.clone(), ParamMode::Default)],
                            ret: ordering_ty.clone(),
                        });
                        // ORD1: lt/le/gt/ge derived from compare
                        for op in &["lt", "le", "gt", "ge"] {
                            if !methods.iter().any(|m| m.name == *op) {
                                new_methods.push(MethodSig {
                                    type_params: Vec::new(),
                                    name: op.to_string(),
                                    self_param: SelfParam::Value,
                                    params: vec![(self_ty.clone(), ParamMode::Default)],
                                    ret: Type::Bool,
                                });
                            }
                        }
                    }

                    // G2: auto-derive debug_string for all types
                    if !methods.iter().any(|m| m.name == "debug_string") {
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
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

                    // G1: mark auto-derived conformances so the nominal check
                    // accepts eligible types without an explicit `extend ... with`.
                    let eq_ok = field_types.iter().all(|ty| self.type_has_method(ty, "eq"));
                    let hash_ok = eq_ok && field_types.iter().all(|ty| self.type_has_method(ty, "hash"));
                    let clone_ok = field_types.iter().all(|ty| self.type_has_method(ty, "clone"))
                        && !field_types.iter().any(|ty| matches!(ty, Type::RawPtr(_)));
                    let cmp_ok = field_types.iter().all(|ty| self.type_has_method(ty, "compare"));
                    if eq_ok { self.types.record_conformance(id, "Equal"); }
                    if hash_ok { self.types.record_conformance(id, "Hashable"); }
                    if clone_ok { self.types.record_conformance(id, "Cloneable"); }
                    if cmp_ok { self.types.record_conformance(id, "Comparable"); }
                    self.types.record_conformance(id, "Debug");
                }
                TypeDef::Enum { variants, methods, type_params, .. } => {
                    let payload_types: Vec<Type> = variants.iter()
                        .flat_map(|(_, fields)| fields.iter().cloned())
                        .collect();
                    let self_ty = Self::self_type_with_params(id, type_params);
                    let mut new_methods = Vec::new();

                    // EQ3: auto-derive eq for enums (tag + payload equality)
                    if !methods.iter().any(|m| m.name == "eq")
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "eq"))
                    {
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
                            name: "eq".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![(self_ty.clone(), ParamMode::Default)],
                            ret: Type::Bool,
                        });
                    }

                    // HA1: auto-derive hash for enums
                    if !methods.iter().any(|m| m.name == "hash")
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "hash"))
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "eq"))
                    {
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
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
                            type_params: Vec::new(),
                            name: "clone".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![],
                            ret: self_ty.clone(),
                        });
                    }

                    // CO1/ORD2: auto-derive compare for enums (variant order, then payload)
                    if !methods.iter().any(|m| m.name == "compare")
                        && payload_types.iter().all(|ty| self.type_has_method(ty, "compare"))
                    {
                        let ordering_ty = self.ordering_type();
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
                            name: "compare".to_string(),
                            self_param: SelfParam::Value,
                            params: vec![(self_ty.clone(), ParamMode::Default)],
                            ret: ordering_ty.clone(),
                        });
                        // ORD1: lt/le/gt/ge derived from compare
                        for op in &["lt", "le", "gt", "ge"] {
                            if !methods.iter().any(|m| m.name == *op) {
                                new_methods.push(MethodSig {
                                    type_params: Vec::new(),
                                    name: op.to_string(),
                                    self_param: SelfParam::Value,
                                    params: vec![(self_ty.clone(), ParamMode::Default)],
                                    ret: Type::Bool,
                                });
                            }
                        }
                    }

                    // G2: auto-derive debug_string for all types
                    if !methods.iter().any(|m| m.name == "debug_string") {
                        new_methods.push(MethodSig {
                            type_params: Vec::new(),
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

                    // G1: mark auto-derived conformances (enum eligibility).
                    let eq_ok = payload_types.iter().all(|ty| self.type_has_method(ty, "eq"));
                    let hash_ok = eq_ok && payload_types.iter().all(|ty| self.type_has_method(ty, "hash"));
                    let clone_ok = payload_types.iter().all(|ty| self.type_has_method(ty, "clone"))
                        && !payload_types.iter().any(|ty| matches!(ty, Type::RawPtr(_)));
                    let cmp_ok = payload_types.iter().all(|ty| self.type_has_method(ty, "compare"));
                    if eq_ok { self.types.record_conformance(id, "Equal"); }
                    if hash_ok { self.types.record_conformance(id, "Hashable"); }
                    if clone_ok { self.types.record_conformance(id, "Cloneable"); }
                    if cmp_ok { self.types.record_conformance(id, "Comparable"); }
                    self.types.record_conformance(id, "Debug");
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
            t if t.is_option() => {
                if let Some(inner) = t.as_option() {
                    self.type_has_method(inner, method)
                } else {
                    false
                }
            }
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

    /// A required edge — bare `Link<T>` rather than `Link<T>?` — is rejected in
    /// this prototype, because neither half of its lifecycle is implemented.
    ///
    /// It can't be *built*: a required cycle needs one side written before its
    /// target exists, which the design answers with batches (a staged region
    /// gives the cycle a legal transient state, constraints checked at apply).
    /// Batches aren't built here.
    ///
    /// It can't be *destroyed*: delete's whole job is to set incoming edges to
    /// `none`, and a required field has no `none` to be set to. The design's
    /// answer is a declared delete policy — cascade or restrict — and neither is
    /// built here.
    ///
    /// So this is a prototype limit, not a language rule. The adversarial pass
    /// did once kill required edges outright (A4), but batches reversed that;
    /// `Link<T>` and `Link<T>?` are both meant to exist.
    ///
    /// A bare link inside a container is a different thing and stays allowed:
    /// `Vec<Link<T>>` and `Map<K, Link<T>>` lose the *entry* at delete rather
    /// than nulling it, which is what a list of live things means.
    fn reject_non_optional_link(&mut self, ty: &Type, span: Span) {
        if self.link_node_type(ty).is_some() {
            self.errors.push(TypeError::NonOptionalLink { span });
        }
    }

    pub(super) fn check_decl(&mut self, decl: &Decl) {
        match &decl.kind {
            DeclKind::Fn(f) => self.check_fn(f),
            DeclKind::Struct(s) => {
                // PC2: field types must name declared types (single letters
                // stay auto-generic, matching function signatures)
                let allowed: Vec<String> = s.type_params.iter().map(|p| p.name.clone()).collect();
                for field in &s.fields {
                    if let Ok(ty) = parse_type_string(&field.ty, &self.types) {
                        self.validate_signature_names(&ty, &allowed, field.name_span);
                        self.reject_non_optional_link(&ty, field.name_span);
                    }
                }
                self.current_self_type = self.types.get_type_id(&s.name).map(Type::Named);
                for method in &s.methods {
                    self.check_fn(method);
                }
                self.current_self_type = None;
            }
            DeclKind::Enum(e) => {
                // PC2: variant payload types must name declared types
                let allowed: Vec<String> = e.type_params.iter().map(|p| p.name.clone()).collect();
                for variant in &e.variants {
                    for field in &variant.fields {
                        if let Ok(ty) = parse_type_string(&field.ty, &self.types) {
                            self.validate_signature_names(&ty, &allowed, field.name_span);
                            self.reject_non_optional_link(&ty, field.name_span);
                        }
                    }
                }
                self.current_self_type = self.types.get_type_id(&e.name).map(Type::Named);
                for method in &e.methods {
                    self.check_fn(method);
                }
                self.current_self_type = None;
            }
            DeclKind::Impl(i) => {
                // UT1: implementing an unsafe trait requires `unsafe extend`
                for trait_name in &i.trait_names {
                    let base = trait_name.split('<').next().unwrap_or(trait_name);
                    if let Some(type_id) = self.types.get_type_id(base) {
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

                // G1: verify the declared conformance at the extend site — the
                // type must have each trait method with a matching signature.
                // Generic targets (`extend Ring<T> with ...`) are checked per
                // instantiation (CC1), so skip them here.
                if !i.trait_names.is_empty() && !i.target_ty.contains('<') {
                    if let Some(target_ty) = self.current_self_type.clone() {
                        let mut trait_errors = Vec::new();
                        {
                            let mut checker = crate::traits::TraitChecker::new(&self.types);
                            for trait_name in &i.trait_names {
                                if let Err(e) = checker.check_satisfies(&target_ty, trait_name, decl.span) {
                                    trait_errors.push((trait_name.clone(), e));
                                }
                            }
                        }
                        for (trait_name, e) in trait_errors {
                            // A header naming a trait that doesn't exist is a
                            // name problem, not a missing method — the block
                            // may well define everything the author meant.
                            if matches!(e, crate::traits::TraitError::UnknownTrait(_)) {
                                self.errors.push(TypeError::NoSuchTrait {
                                    trait_name,
                                    known: self.declared_trait_names(),
                                    span: decl.span,
                                });
                                continue;
                            }
                            self.errors.push(TypeError::TraitNotSatisfied {
                                ty: i.target_ty.clone(),
                                trait_name,
                                context: super::TraitBoundContext::ConformanceHeader,
                                span: decl.span,
                            });
                        }
                    }
                }
                // CC2: `extend Foo<T> where T: Trait { }` bounds cover every
                // method in the block, so a call like `t.wrapping_add(u)`
                // inside one of these methods needs to see them the same way
                // a method's own `where` clause would (#838).
                self.current_impl_type_param_bounds = i.where_bounds.iter()
                    .map(|tp| (tp.name.clone(), tp.bounds.clone()))
                    .collect();
                for method in &i.methods {
                    self.check_fn(method);
                }
                self.current_impl_type_param_bounds = std::collections::HashMap::new();
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
                    // Solve as we go, same as check_fn — a later statement
                    // (e.g. `m.insert(...)`) needs an earlier local's generic
                    // args (e.g. `Map.new()`'s value type) already resolved,
                    // or it stays an unbound type var forever (#390).
                    self.solve_constraints();
                }
            }
            DeclKind::Benchmark(b) => {
                for stmt in &b.body {
                    self.check_stmt(stmt);
                    self.solve_constraints();
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
                                DeclKind::Struct(s) => {
                                    let id = self.register_struct(s);
                                    self.types.record_method_decl(id, ext_decl.id);
                                }
                                DeclKind::Enum(e) => {
                                    let id = self.register_enum(e, ext_decl.span);
                                    self.types.record_method_decl(id, ext_decl.id);
                                }
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
                    type_params: Vec::new(),
                    name: "parse".to_string(),
                    self_param: SelfParam::None,
                    params: vec![(Type::Slice(Box::new(Type::U8)), ParamMode::Default)],
                    ret: parse_result,
                },
                MethodSig {
                    type_params: Vec::new(),
                    name: "build".to_string(),
                    self_param: SelfParam::Value,
                    params: vec![],
                    ret: vec_u8,
                },
                MethodSig {
                    type_params: Vec::new(),
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

/// PC1: single uppercase ASCII letter — always a type parameter in signatures.
pub(super) fn is_type_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
}

/// PC1: a function's type params — explicit `<T>` declarations plus every
/// single uppercase letter in its signature types, in signature order
/// (params left to right, then return type).
///
/// Shared by the type checker and the monomorphizer so both derive the same
/// ordered list. Uses a fresh TypeTable: results match the populated table
/// because single-letter names can never be registered types (PC3), and only
/// single letters are collected.
pub fn signature_type_param_names(f: &FnDecl) -> Vec<String> {
    use std::sync::OnceLock;
    static EMPTY_TABLE: OnceLock<super::type_table::TypeTable> = OnceLock::new();
    let table = EMPTY_TABLE.get_or_init(super::type_table::TypeTable::new);

    let mut names: Vec<String> = f.type_params.iter().map(|p| p.name.clone()).collect();
    let mut add = |n: &str| {
        if is_type_param_name(n) && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    for p in &f.params {
        if p.name != "self" && !p.ty.is_empty() {
            if let Ok(ty) = parse_type_string(&p.ty, table) {
                for_each_unresolved_name(&ty, &mut add);
            }
        }
    }
    if let Some(rt) = &f.ret_ty {
        if let Ok(ty) = parse_type_string(rt, table) {
            for_each_unresolved_name(&ty, &mut add);
        }
    }
    names
}

/// PC1 for a type declaration: explicit `<T>` declarations plus every single
/// uppercase letter appearing in its field or payload types, in declaration
/// order.
///
/// `gradual-constraints` lists struct fields and enum payloads as signature
/// positions alongside function parameters, but only the function side was
/// wired up — so `struct Pair { first: T  second: U }` registered with no type
/// parameters at all, and `Pair<i64, string>` had nothing to match against.
/// SYNTAX.md's own `Pair` example didn't compile (#913).
pub fn declared_type_param_names<'a>(
    explicit: &[rask_ast::decl::TypeParam],
    member_types: impl Iterator<Item = &'a str>,
) -> Vec<String> {
    use std::sync::OnceLock;
    static EMPTY_TABLE: OnceLock<super::type_table::TypeTable> = OnceLock::new();
    let table = EMPTY_TABLE.get_or_init(super::type_table::TypeTable::new);

    let mut names: Vec<String> = explicit.iter().map(|p| p.name.clone()).collect();
    let mut add = |n: &str| {
        if is_type_param_name(n) && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    for ty_str in member_types {
        if ty_str.is_empty() {
            continue;
        }
        if let Ok(ty) = parse_type_string(ty_str, table) {
            for_each_unresolved_name(&ty, &mut add);
        }
    }
    names
}

/// PC1 for a struct: explicit `<T>` plus single letters in its field types.
pub fn struct_type_param_names(s: &StructDecl) -> Vec<String> {
    declared_type_param_names(&s.type_params, s.fields.iter().map(|f| f.ty.as_str()))
}

/// PC1 for an enum: explicit `<T>` plus single letters in its payload types.
pub fn enum_type_param_names(e: &EnumDecl) -> Vec<String> {
    declared_type_param_names(
        &e.type_params,
        e.variants.iter().flat_map(|v| v.fields.iter().map(|f| f.ty.as_str())),
    )
}

/// Walk a parsed type tree, calling `f` on every unresolved base name.
pub(super) fn for_each_unresolved_name(ty: &Type, f: &mut impl FnMut(&str)) {
    use crate::types::GenericArg;
    match ty {
        Type::UnresolvedNamed(name) => f(name),
        Type::UnresolvedGeneric { name, args } => {
            f(name);
            for a in args {
                if let GenericArg::Type(t) = a {
                    for_each_unresolved_name(t, f);
                }
            }
        }
        Type::Generic { args, .. } => {
            for a in args {
                if let GenericArg::Type(t) = a {
                    for_each_unresolved_name(t, f);
                }
            }
        }
        Type::Result { ok, err } => {
            for_each_unresolved_name(ok, f);
            for_each_unresolved_name(err, f);
        }
        Type::Tuple(elems) | Type::Union(elems) => {
            for e in elems {
                for_each_unresolved_name(e, f);
            }
        }
        Type::Array { elem, .. }
        | Type::Slice(elem)
        | Type::RawPtr(elem)
        | Type::SimdVector { elem, .. } => for_each_unresolved_name(elem, f),
        Type::Fn { params, ret } => {
            for p in params {
                for_each_unresolved_name(p, f);
            }
            for_each_unresolved_name(ret, f);
        }
        _ => {}
    }
}
