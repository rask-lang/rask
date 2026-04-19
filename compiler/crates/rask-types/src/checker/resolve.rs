// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Field and method resolution, including builtin type methods.

use std::collections::HashMap;

use rask_ast::Span;

use super::type_defs::TypeDef;
use super::errors::TypeError;
use super::inference::TypeConstraint;
use super::TypeChecker;

use crate::types::{GenericArg, Type, TypeId, TypeVarId};

impl TypeChecker {
    pub(super) fn resolve_field(
        &mut self,
        ty: Type,
        field: String,
        expected: Type,
        span: Span,
        self_type: Option<Type>,
    ) -> Result<bool, TypeError> {
        let ty = self.resolve_named(&self.ctx.apply(&ty));

        match &ty {
            // Source error already reported — suppress cascading field errors
            Type::Error => Ok(false),
            Type::Var(_) => {
                self.ctx.add_constraint(TypeConstraint::HasField {
                    ty,
                    field,
                    expected,
                    span,
                    self_type,
                });
                Ok(false)
            }
            Type::Named(type_id) => {
                // V5: check private field access
                if let Some(TypeDef::Struct { private_fields, name, .. }) = self.types.get(*type_id) {
                    if private_fields.contains(&field) {
                        let is_self = self_type.as_ref()
                            .is_some_and(|st| matches!(st, Type::Named(id) if *id == *type_id));
                        if !is_self {
                            return Err(TypeError::PrivateFieldAccess {
                                ty: name.clone(),
                                field,
                                span,
                            });
                        }
                    }
                }

                let result = self.types.get(*type_id).and_then(|def| {
                    match def {
                        TypeDef::Struct { fields, .. } | TypeDef::Union { fields, .. } => {
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
                        TypeDef::NominalAlias { underlying, .. } => {
                            if field == "value" {
                                Some(underlying.clone())
                            } else {
                                None
                            }
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
            // UnresolvedGeneric: resolve element field access through first
            // type arg. Handles vec[i].field where vec type wasn't fully
            // resolved during inference.
            Type::UnresolvedGeneric { args, .. } => {
                if let Some(GenericArg::Type(elem)) = args.first() {
                    let elem_ty = self.resolve_named(elem);
                    self.resolve_field(elem_ty, field, expected, span, self_type)
                } else {
                    Err(TypeError::NoSuchField { ty, field, span })
                }
            }
            // Module namespace and builtin struct field resolution
            Type::UnresolvedNamed(name) => {
                // Module namespace: __module_X.Field → look up Field in type table
                if name.starts_with("__module_") {
                    if let Some(type_id) = self.types.get_type_id(&field) {
                        return self.unify(&expected, &Type::Named(type_id), span);
                    }
                    // Fallback: treat as unresolved named type
                    let resolved_ty = Type::UnresolvedNamed(field.to_string());
                    return self.unify(&expected, &resolved_ty, span);
                }

                // Builtin struct fields for runtime/stdlib types
                let field_ty = match (name.as_str(), field.as_str()) {
                    ("Response", "status") => Some(Type::U16),
                    ("Response", "headers") => Some(Type::UnresolvedNamed("Headers".to_string())),
                    ("Response", "body") => Some(Type::String),
                    ("Request", "method") => Some(Type::UnresolvedNamed("Method".to_string())),
                    ("Request", "url") => Some(Type::String),
                    ("Request", "body") => Some(Type::String),
                    ("Request", "headers") => Some(Type::UnresolvedNamed("Headers".to_string())),
                    _ => None,
                };
                if let Some(ft) = field_ty {
                    self.unify(&expected, &ft, span)
                } else {
                    Err(TypeError::NoSuchField { ty, field, span })
                }
            }
            // Option<T> field access: unwrap and access inner type
            Type::Option(inner) => {
                let inner = *inner.clone();
                self.resolve_field(inner, field, expected, span, self_type)
            }
            _ => Err(TypeError::NoSuchField {
                ty,
                field,
                span,
            }),
        }
    }

    pub(super) fn resolve_method(
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

        // to_string() on any type returns string
        if method == "to_string" && args.is_empty() {
            return self.unify(&ret, &Type::String, span);
        }

        // ER16: .origin() on any type returns the error origin string.
        // Set by `try` at first propagation (ER15). Returns "<no origin>" if unset.
        if method == "origin" && args.is_empty() {
            return self.unify(&ret, &Type::String, span);
        }

        match &ty {
            // Source error already reported — suppress cascading method errors
            Type::Error => Ok(false),
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
                    } else if method == "variants" && args.is_empty() {
                        // .variants() on fieldless enums returns Vec of all variant values (E7-E8)
                        let is_fieldless = self.types.get(*type_id).map(|def| {
                            if let TypeDef::Enum { variants, .. } = def {
                                variants.iter().all(|(_, fields)| fields.is_empty())
                            } else {
                                false
                            }
                        }).unwrap_or(false);
                        if is_fieldless {
                            let vec_ty = Type::Slice(Box::new(ty));
                            self.unify(&vec_ty, &ret, span)
                        } else {
                            Err(TypeError::NoSuchMethod {
                                ty,
                                method: "variants (requires fieldless enum)".to_string(),
                                span,
                            })
                        }
                    } else if method == "discriminant" && args.is_empty() {
                        // E9: .discriminant() returns u16 variant index
                        self.unify(&Type::U16, &ret, span)
                    } else if method == "from_value" && args.len() == 1 {
                        // E18: from_value(n) on fieldless enums returns Option<Enum>
                        let is_fieldless = self.types.get(*type_id).map(|def| {
                            if let TypeDef::Enum { variants, .. } = def {
                                variants.iter().all(|(_, fields)| fields.is_empty())
                            } else {
                                false
                            }
                        }).unwrap_or(false);
                        if is_fieldless {
                            // Accept any integer type for the discriminant value.
                            if let Some(arg_ty) = args.first() {
                                let resolved_arg = self.ctx.apply(arg_ty);
                                let is_int = matches!(
                                    resolved_arg,
                                    Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
                                    | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
                                );
                                if !is_int {
                                    self.unify(arg_ty, &Type::I64, span)?;
                                }
                            }
                            let opt_ty = Type::Option(Box::new(ty));
                            self.unify(&opt_ty, &ret, span)
                        } else {
                            Err(TypeError::NoSuchMethod {
                                ty,
                                method: "from_value (requires fieldless enum)".to_string(),
                                span,
                            })
                        }
                    } else {
                        // Method not in registered extend blocks. Check if this
                        // Named type corresponds to a builtin with hardcoded
                        // method resolution (Vec, Map, Shared, etc.).
                        let type_name = self.types.type_name(*type_id);
                        self.resolve_builtin_method_by_name(&type_name, &[], &method, &args, &ret, span)
                            .unwrap_or_else(|| Err(TypeError::NoSuchMethod { ty, method, span }))
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
            Type::UnresolvedGeneric { name, args: type_args } if name == "TaskHandle" => {
                self.resolve_task_handle_method(&type_args, &method, &args, &ret, span)
            }
            // Pool<T>
            Type::UnresolvedGeneric { name, args: type_args } if name == "Pool" => {
                self.resolve_pool_method(type_args, &method, &args, &ret, span)
            }
            // Handle<T> — value type, eq/ne only
            Type::UnresolvedGeneric { name, .. } if name == "Handle" => {
                match method.as_str() {
                    "eq" | "ne" if args.len() == 1 => self.unify(&ret, &Type::Bool, span),
                    _ => Err(TypeError::NoSuchMethod { ty, method, span }),
                }
            }
            // WeakHandle<T> — valid(), upgrade(), eq, ne
            Type::UnresolvedGeneric { name, args: type_args } if name == "WeakHandle" => {
                let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
                    *t.clone()
                } else {
                    self.ctx.fresh_var()
                };
                match method.as_str() {
                    "valid" if args.is_empty() => self.unify(&ret, &Type::Bool, span),
                    "upgrade" if args.is_empty() => {
                        let handle_ty = Type::UnresolvedGeneric {
                            name: "Handle".to_string(),
                            args: vec![GenericArg::Type(Box::new(inner_type))],
                        };
                        let opt_ty = Type::Option(Box::new(handle_ty));
                        self.unify(&ret, &opt_ty, span)
                    }
                    "eq" | "ne" if args.len() == 1 => self.unify(&ret, &Type::Bool, span),
                    _ => Err(TypeError::NoSuchMethod { ty, method, span }),
                }
            }
            // Pool (bare, for static constructors like Pool.new())
            Type::UnresolvedNamed(name) if name == "Pool" => {
                self.resolve_pool_static_method(&method, &args, &ret, span)
            }
            // Vec<T>
            Type::UnresolvedGeneric { name, args: type_args } if name == "Vec" => {
                self.resolve_vec_method(type_args, &method, &args, &ret, span)
            }
            // Vec (bare, for static constructors like Vec.new())
            Type::UnresolvedNamed(name) if name == "Vec" => {
                self.resolve_vec_static_method(&method, &args, &ret, span)
            }
            // Map<K, V>
            Type::UnresolvedGeneric { name, args: type_args } if name == "Map" => {
                self.resolve_map_method(type_args, &method, &args, &ret, span)
            }
            // Map (bare, for static constructors like Map.new())
            Type::UnresolvedNamed(name) if name == "Map" => {
                self.resolve_map_static_method(&method, &args, &ret, span)
            }
            // Rng (no type params — static and instance methods)
            Type::UnresolvedNamed(name) if name == "Rng" => {
                self.resolve_rng_method(&method, &args, &ret, span)
            }
            // Atomic types (AtomicBool, AtomicI8..AtomicU64, AtomicUsize, AtomicIsize)
            Type::UnresolvedNamed(name) if Self::is_atomic_type(name) => {
                self.resolve_atomic_method(name, &method, &args, &ret, span)
            }
            // Thread.spawn(closure) → ThreadHandle<T>
            Type::UnresolvedNamed(name) if name == "Thread" || name == "ThreadPool" => {
                if method == "spawn" && args.len() == 1 {
                    // Extract closure return type for ThreadHandle<T>
                    let inner = if let Type::Fn { ret: fn_ret, .. } = &args[0] {
                        *fn_ret.clone()
                    } else {
                        self.ctx.fresh_var()
                    };
                    let handle_ty = Type::UnresolvedGeneric {
                        name: "ThreadHandle".to_string(),
                        args: vec![GenericArg::Type(Box::new(inner))],
                    };
                    self.unify(&ret, &handle_ty, span)
                } else {
                    Err(TypeError::NoSuchMethod {
                        ty,
                        method,
                        span,
                    })
                }
            }
            // SIMD vector types (f32x4, f32x8, i32x4, i32x8, f64x2, f64x4)
            Type::UnresolvedNamed(name) if Self::is_simd_type(name) => {
                self.resolve_simd_method(name, &method, &args, &ret, span)
            }
            // Shared<T>, Sender<T>, Receiver<T>, Channel<T>
            Type::UnresolvedGeneric { name, args: type_args } if matches!(name.as_str(), "Cell" | "Shared" | "Mutex" | "Sender" | "Receiver" | "Channel") => {
                self.resolve_concurrency_generic_method(name, &type_args, &method, &args, &ret, span)
            }
            // Builtin runtime types: Instant, Duration, TcpListener, TcpConnection, Shared (bare)
            Type::UnresolvedNamed(name) if matches!(name.as_str(), "Instant" | "Duration" | "TcpListener" | "TcpConnection" | "Response" | "Request" | "Shared" | "Mutex")
                || rask_stdlib::StubRegistry::load().get_type(name).is_some()
            => {
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
                        let type_name = self.types.type_name(*base);
                        self.resolve_builtin_method_by_name(&type_name, generic_args, &method, &args, &ret, span)
                            .unwrap_or_else(|| Err(TypeError::NoSuchMethod { ty, method, span }))
                    }
                }
            }
            // Trait object: look up method in trait definition
            Type::TraitObject { ref trait_name } => {
                let trait_name = trait_name.clone();
                let checker = crate::traits::TraitChecker::new(&self.types);
                let trait_methods = checker.get_trait_methods_public(&trait_name);

                if let Some(method_sig) = trait_methods.iter().find(|m| m.name == method) {
                    // TR2: reject methods returning Self
                    if matches!(&method_sig.ret, Type::UnresolvedNamed(n) if n == "Self") {
                        return Err(TypeError::TraitObjectSelfReturn {
                            trait_name,
                            method,
                            span,
                        });
                    }

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
                    Err(TypeError::NoSuchMethod {
                        ty,
                        method,
                        span,
                    })
                }
            }
            // Primitive integer types — resolve operator methods directly
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
            | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128 => {
                self.resolve_integer_method(&ty, &method, &args, &ret, span)
            }
            // Primitive float types
            Type::F32 | Type::F64 => {
                self.resolve_float_method(&ty, &method, &args, &ret, span)
            }
            Type::Option(inner) => {
                let inner = *inner.clone();
                self.resolve_option_method(&inner, &method, &args, &ret, span)
            }
            Type::Result { ok, err } => {
                let ok = *ok.clone();
                let err = *err.clone();
                self.resolve_result_method(&ok, &err, &method, &args, &ret, span)
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

    pub(super) fn instantiate_builtin_enum_variant(
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

    pub(super) fn resolve_string_method(
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
            let ret_ty = super::builtins::parse_stub_type(&method_def.ret_ty);
            return self.unify(ret, &ret_ty, span);
        }

        match method {
            "add" => return Err(TypeError::StringAddForbidden { span }),
            "len" if args.is_empty() => self.unify(ret, &Type::U64, span),
            "is_empty" if args.is_empty() => self.unify(ret, &Type::Bool, span),
            "contains" if args.len() == 1 => {
                self.unify(&args[0], &Type::String, span)?;
                self.unify(ret, &Type::Bool, span)
            }
            "push" | "push_str" => self.unify(ret, &Type::Unit, span),
            "concat" if args.len() == 1 => {
                self.unify(&args[0], &Type::String, span)?;
                self.unify(ret, &Type::String, span)
            }
            _ => Ok(false),
        }
    }

    pub(super) fn resolve_array_method(
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
            let ret_ty = super::builtins::parse_stub_type(&method_def.ret_ty);
            return self.unify(ret, &ret_ty, span);
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

    pub(super) fn resolve_file_method(
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

    pub(super) fn resolve_thread_handle_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        // ThreadHandle<T> has two methods:
        // - join(self) -> T or JoinError
        // - detach(self) -> ()

        match method {
            "join" if args.is_empty() => {
                // Extract the T type parameter
                let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
                    *t.clone()
                } else {
                    self.ctx.fresh_var()
                };

                // join returns Result<T, JoinError>
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::UnresolvedNamed("JoinError".to_string())),
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

    pub(super) fn resolve_task_handle_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            "join" if args.is_empty() => {
                let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
                    *t.clone()
                } else {
                    self.ctx.fresh_var()
                };
                let result_type = Type::Result {
                    ok: Box::new(inner_type),
                    err: Box::new(Type::UnresolvedNamed("JoinError".to_string())),
                };
                self.unify(ret, &result_type, span)
            }
            "detach" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            "cancel" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: Type::UnresolvedGeneric {
                    name: "TaskHandle".to_string(),
                    args: type_args.to_vec(),
                },
                method: method.to_string(),
                span,
            }),
        }
    }

    pub(super) fn resolve_runtime_method(
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
            ("Duration", "from_millis") if args.len() == 1 => {
                self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
            }

            // Instant arithmetic: instant + duration -> Instant
            ("Instant", "add") if args.len() == 1 => {
                let duration_ty = Type::UnresolvedNamed("Duration".to_string());
                self.unify(&args[0], &duration_ty, span)?;
                self.unify(ret, &Type::UnresolvedNamed("Instant".to_string()), span)
            }
            // Instant subtraction: overloaded on argument type
            //   instant - instant -> Duration
            //   instant - duration -> Instant
            ("Instant", "sub") if args.len() == 1 => {
                let arg = self.ctx.apply(&args[0]);
                let arg = self.resolve_named(&arg);
                match &arg {
                    Type::UnresolvedNamed(n) if n == "Instant" => {
                        self.unify(ret, &Type::UnresolvedNamed("Duration".to_string()), span)
                    }
                    Type::UnresolvedNamed(n) if n == "Duration" => {
                        self.unify(ret, &Type::UnresolvedNamed("Instant".to_string()), span)
                    }
                    Type::Var(_) => {
                        // Argument type not yet resolved — defer
                        self.ctx.add_constraint(TypeConstraint::HasMethod {
                            ty: Type::UnresolvedNamed(type_name.to_string()),
                            method: method.to_string(),
                            args: args.to_vec(),
                            ret: ret.clone(),
                            span,
                        });
                        Ok(false)
                    }
                    _ => Err(TypeError::Mismatch {
                        expected: Type::UnresolvedNamed("Instant".to_string()),
                        found: arg.clone(),
                        span,
                    }),
                }
            }
            // Instant comparisons
            ("Instant", "eq" | "lt" | "le" | "gt" | "ge") if args.len() == 1 => {
                let instant_ty = Type::UnresolvedNamed("Instant".to_string());
                self.unify(&args[0], &instant_ty, span)?;
                self.unify(ret, &Type::Bool, span)
            }

            // Duration arithmetic: duration +/- duration -> Duration
            ("Duration", "add" | "sub") if args.len() == 1 => {
                let duration_ty = Type::UnresolvedNamed("Duration".to_string());
                self.unify(&args[0], &duration_ty, span)?;
                self.unify(ret, &duration_ty, span)
            }
            // Duration comparisons
            ("Duration", "eq" | "lt" | "le" | "gt" | "ge") if args.len() == 1 => {
                let duration_ty = Type::UnresolvedNamed("Duration".to_string());
                self.unify(&args[0], &duration_ty, span)?;
                self.unify(ret, &Type::Bool, span)
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
                    ok: Box::new(Type::UnresolvedNamed("Request".to_string())),
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
            // Response — allow method-style access for chaining
            ("Response", "status") if args.is_empty() => {
                self.unify(ret, &Type::U16, span)
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
            // Cell static constructor: Cell.new(value) -> Cell<T>
            ("Cell", "new") if args.len() == 1 => {
                let inner = args[0].clone();
                let cell_ty = Type::UnresolvedGeneric {
                    name: "Cell".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner))],
                };
                self.unify(ret, &cell_ty, span)
            }
            // Mutex static constructor: Mutex.new(value) -> Mutex<T>
            ("Mutex", "new") if args.len() == 1 => {
                let inner = args[0].clone();
                let mutex_ty = Type::UnresolvedGeneric {
                    name: "Mutex".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner))],
                };
                self.unify(ret, &mutex_ty, span)
            }
            _ => {
                // Try stub registry before falling through
                if let Some(stub) = rask_stdlib::lookup_method(type_name, method) {
                    let expected_params = stub.params.len();
                    if args.len() != expected_params {
                        return Err(TypeError::ArityMismatch {
                            expected: expected_params,
                            found: args.len(),
                            span,
                        });
                    }
                    for ((_, param_ty_str), arg) in stub.params.iter().zip(args.iter()) {
                        let param_ty = super::builtins::parse_stub_type(param_ty_str);
                        self.unify(arg, &param_ty, span)?;
                    }
                    let ret_ty = super::builtins::parse_stub_type(&stub.ret_ty);
                    return self.unify(ret, &ret_ty, span);
                }
                // Known runtime type but unknown method — hard error
                Err(TypeError::NoSuchMethod {
                    ty: Type::UnresolvedNamed(type_name.to_string()),
                    method: method.to_string(),
                    span,
                })
            }
        }
    }

    pub(super) fn resolve_concurrency_generic_method(
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
            // Shared<T>.read() -> T  (inline access, E5/R5)
            ("Shared", "read") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // Shared<T>.write() -> T  (inline access, E5/R5)
            ("Shared", "write") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // Shared<T>.read(|T| -> R) -> R  (closure-based, try_read)
            ("Shared", "read") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                self.unify(ret, &result_var, span)
            }
            // Shared<T>.write(|T| -> R) -> R  (closure-based, try_write)
            ("Shared", "write") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                self.unify(ret, &result_var, span)
            }
            // Shared<T>.try_read(|T| -> R) -> Option<R>  (non-blocking, R3)
            ("Shared", "try_read") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                let opt_ty = Type::Option(Box::new(result_var));
                self.unify(ret, &opt_ty, span)
            }
            // Shared<T>.try_write(|T| -> R) -> Option<R>  (non-blocking, R3)
            ("Shared", "try_write") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                let opt_ty = Type::Option(Box::new(result_var));
                self.unify(ret, &opt_ty, span)
            }
            // Shared<T>.clone() -> Shared<T>
            ("Shared", "clone") if args.is_empty() => {
                let shared_ty = Type::UnresolvedGeneric {
                    name: "Shared".to_string(),
                    args: type_args.to_vec(),
                };
                self.unify(ret, &shared_ty, span)
            }
            // Cell<T>.get() -> T (CE6: Copy types only, not enforced in type checker)
            ("Cell", "get") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // Cell<T>.set(value: T) -> ()
            ("Cell", "set") if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                self.unify(ret, &Type::Unit, span)
            }
            // Cell<T>.replace(value: T) -> T
            ("Cell", "replace") if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                self.unify(ret, &inner_type, span)
            }
            // Cell<T>.into_inner() -> T (consumes cell)
            ("Cell", "into_inner") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // Mutex<T>.lock() -> T  (inline access, E5/MX3)
            ("Mutex", "lock") if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // Mutex<T>.lock(|T| -> R) -> R  (closure-based)
            ("Mutex", "lock") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                self.unify(ret, &result_var, span)
            }
            // Mutex<T>.try_lock(|T| -> R) -> Option<R>
            ("Mutex", "try_lock") if args.len() == 1 => {
                let result_var = self.ctx.fresh_var();
                let opt_ty = Type::Option(Box::new(result_var));
                self.unify(ret, &opt_ty, span)
            }
            // Mutex<T>.clone() -> Mutex<T>
            ("Mutex", "clone") if args.is_empty() => {
                let mutex_ty = Type::UnresolvedGeneric {
                    name: "Mutex".to_string(),
                    args: type_args.to_vec(),
                };
                self.unify(ret, &mutex_ty, span)
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
            // Sender<T>.try_send(value: T) -> () or string
            ("Sender", "try_send") if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
                    err: Box::new(Type::String),
                };
                self.unify(ret, &result_type, span)
            }
            // Sender<T>.close() -> () or string
            ("Sender", "close") if args.is_empty() => {
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
            // Receiver<T>.close() -> () or string
            ("Receiver", "close") if args.is_empty() => {
                let result_type = Type::Result {
                    ok: Box::new(Type::Unit),
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
            // Shared<T>.new(value) -> Shared<T> (static constructor with explicit type param)
            ("Shared", "new") if args.len() == 1 => {
                let shared_ty = Type::UnresolvedGeneric {
                    name: "Shared".to_string(),
                    args: type_args.to_vec(),
                };
                self.unify(ret, &shared_ty, span)
            }
            // Mutex<T>.new(value) -> Mutex<T> (static constructor with explicit type param)
            ("Mutex", "new") if args.len() == 1 => {
                let mutex_ty = Type::UnresolvedGeneric {
                    name: "Mutex".to_string(),
                    args: type_args.to_vec(),
                };
                self.unify(ret, &mutex_ty, span)
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

    /// Resolve methods on Pool<T> instances.
    pub(super) fn resolve_pool_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };

        match method {
            // pool.insert(value: T) -> Handle<T> (panics on failure, like Vec.push)
            "alloc" | "insert" if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                let handle_ty = Type::UnresolvedGeneric {
                    name: "Handle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                self.unify(ret, &handle_ty, span)
            }
            // pool.get(h: Handle<T>) -> T?
            "get" if args.len() == 1 => {
                let result_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &result_ty, span)
            }
            // pool.remove(h: Handle<T>) -> T?
            "remove" if args.len() == 1 => {
                let result_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &result_ty, span)
            }
            // pool.len() -> u64
            "len" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            // pool.is_empty() -> bool
            "is_empty" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            // pool.handles() -> Vec<Handle<T>>
            "handles" if args.is_empty() => {
                let handle_ty = Type::UnresolvedGeneric {
                    name: "Handle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(handle_ty))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // pool.contains(h: Handle<T>) -> bool
            "contains" if args.len() == 1 => {
                self.unify(ret, &Type::Bool, span)
            }
            // pool.clear() -> ()
            "clear" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            // pool.get_mut(h) -> T?
            "get_mut" | "get_clone" if args.len() == 1 => {
                let result_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &result_ty, span)
            }
            // pool.try_insert(value: T) -> Handle<T>?
            "try_insert" if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                let handle_ty = Type::UnresolvedGeneric {
                    name: "Handle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                let opt_ty = Type::Option(Box::new(handle_ty));
                self.unify(ret, &opt_ty, span)
            }
            // pool.drain() -> Vec<T>
            "drain" | "take_all" if args.is_empty() => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // pool.entries() -> Vec<(Handle<T>, T)>
            "entries" if args.is_empty() => {
                let handle_ty = Type::UnresolvedGeneric {
                    name: "Handle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type.clone()))],
                };
                let pair_ty = Type::Tuple(vec![handle_ty, inner_type]);
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(pair_ty))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // pool.get_unchecked(h) -> T, pool.get_mut_unchecked(h) -> T
            "get_unchecked" | "get_mut_unchecked" if args.len() == 1 => {
                self.unify(ret, &inner_type, span)
            }
            // pool.read(h, closure) -> R?, pool.modify(h, closure) -> R?
            // pool.with_valid(h, closure) -> R?, pool.with_valid_mut(h, closure) -> R?
            "read" | "modify" | "with_valid" | "with_valid_mut" if args.len() == 2 => {
                let result_ty = Type::Option(Box::new(self.ctx.fresh_var()));
                self.unify(ret, &result_ty, span)
            }
            // pool.capacity() -> u64, pool.remaining() -> u64
            "capacity" | "remaining" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            // pool.weak(h: Handle<T>) -> WeakHandle<T>
            "weak" if args.len() == 1 => {
                let weak_ty = Type::UnresolvedGeneric {
                    name: "WeakHandle".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                self.unify(ret, &weak_ty, span)
            }
            // pool.snapshot() -> (Pool<T>, Pool<T>)
            "snapshot" if args.is_empty() => {
                let pool_ty = Type::UnresolvedGeneric {
                    name: "Pool".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type.clone()))],
                };
                let pair_ty = Type::Tuple(vec![pool_ty.clone(), pool_ty]);
                self.unify(ret, &pair_ty, span)
            }
            // pool.clone() -> Pool<T>
            "clone" if args.is_empty() => {
                let pool_ty = Type::UnresolvedGeneric {
                    name: "Pool".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                self.unify(ret, &pool_ty, span)
            }
            _ => {
                self.ctx.add_constraint(TypeConstraint::HasMethod {
                    ty: Type::UnresolvedGeneric {
                        name: "Pool".to_string(),
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

    /// Resolve static methods on bare Pool (e.g. Pool.new()).
    pub(super) fn resolve_pool_static_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            // Pool.new() -> Pool<T> where T is fresh
            "new" if args.is_empty() => {
                let fresh = self.ctx.fresh_var();
                let pool_ty = Type::UnresolvedGeneric {
                    name: "Pool".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &pool_ty, span)
            }
            _ => {
                Err(TypeError::NoSuchMethod {
                    ty: Type::UnresolvedNamed("Pool".to_string()),
                    method: method.to_string(),
                    span,
                })
            }
        }
    }

    /// Resolve static methods on bare Vec (e.g. Vec.new()).
    pub(super) fn resolve_vec_static_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            "new" if args.is_empty() => {
                let fresh = self.ctx.fresh_var();
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // Vec.from(array) — construct Vec from array literal
            "from" if args.len() == 1 => {
                // Extract element type from the argument (array literal or Vec)
                // and produce Vec<T>.
                let elem_ty = match &args[0] {
                    Type::Array { elem, .. } | Type::Slice(elem) => *elem.clone(),
                    Type::UnresolvedGeneric { name, args: type_args } if name == "Vec" => {
                        if let Some(GenericArg::Type(t)) = type_args.first() {
                            *t.clone()
                        } else {
                            self.ctx.fresh_var()
                        }
                    }
                    Type::Generic { args: type_args, .. } => {
                        if let Some(GenericArg::Type(t)) = type_args.first() {
                            *t.clone()
                        } else {
                            self.ctx.fresh_var()
                        }
                    }
                    _ => self.ctx.fresh_var(),
                };
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(elem_ty))],
                };
                self.unify(ret, &vec_ty, span)
            }
            _ => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(self.ctx.fresh_var()))],
                };
                Err(TypeError::NoSuchMethod {
                    ty: vec_ty,
                    method: method.to_string(),
                    span,
                })
            }
        }
    }

    /// Resolve instance methods on Vec<T>.
    pub(super) fn resolve_vec_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let inner_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };

        let self_ty = Type::UnresolvedGeneric {
            name: "Vec".to_string(),
            args: type_args.to_vec(),
        };

        match method {
            "push" if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                self.unify(ret, &Type::Unit, span)
            }
            "pop" if args.is_empty() => {
                let opt_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &opt_ty, span)
            }
            "len" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            "get" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                let opt_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &opt_ty, span)
            }
            "set" if args.len() == 2 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                let _ = self.unify(&args[1], &inner_type, span);
                self.unify(ret, &Type::Unit, span)
            }
            "clear" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            "is_empty" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            "capacity" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            // vec.insert(index, value) -> ()
            "insert" if args.len() == 2 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                let _ = self.unify(&args[1], &inner_type, span);
                self.unify(ret, &Type::Unit, span)
            }
            // vec.remove(index) -> T
            "remove" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                self.unify(ret, &inner_type, span)
            }
            // vec.chunks(size) -> Vec<Vec<T>>
            "chunks" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                let chunk_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(inner_type))],
                };
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(chunk_ty))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.to_vec() -> Vec<T>
            "to_vec" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            "iter" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            "skip" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                self.unify(ret, &self_ty, span)
            }
            "take" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                self.unify(ret, &self_ty, span)
            }
            "limit" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                self.unify(ret, &self_ty, span)
            }
            "collect" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            // vec.first() -> Option<T>
            "first" if args.is_empty() => {
                let opt_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &opt_ty, span)
            }
            // vec.last() -> Option<T>
            "last" if args.is_empty() => {
                let opt_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &opt_ty, span)
            }
            // vec.contains(value) -> bool
            "contains" if args.len() == 1 => {
                let _ = self.unify(&args[0], &inner_type, span);
                self.unify(ret, &Type::Bool, span)
            }
            // vec.reverse() -> ()
            "reverse" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.join(sep) -> string
            "join" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::String, span);
                self.unify(ret, &Type::String, span)
            }
            // vec.sort() -> ()
            "sort" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.sort_by(comparator) -> ()
            "sort_by" if args.len() == 1 => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.sort_by_key(key_fn) -> ()
            "sort_by_key" if args.len() == 1 => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.dedup() -> ()
            "dedup" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            // vec.filter(predicate) -> Vec<T>
            "filter" if args.len() == 1 => {
                self.unify(ret, &self_ty, span)
            }
            // vec.map(transform) -> Vec<U>
            "map" if args.len() == 1 => {
                let fresh = self.ctx.fresh_var();
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.flat_map(transform) -> Vec<U>
            "flat_map" if args.len() == 1 => {
                let fresh = self.ctx.fresh_var();
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.flatten() -> Vec<T>
            "flatten" if args.is_empty() => {
                let fresh = self.ctx.fresh_var();
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(fresh))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.fold(init, f) -> U
            "fold" if args.len() == 2 => {
                let _ = self.unify(ret, &args[0], span);
                Ok(true)
            }
            // vec.reduce(f) -> Option<T>
            "reduce" if args.len() == 1 => {
                let opt_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &opt_ty, span)
            }
            // vec.enumerate() -> Vec<(i64, T)>
            "enumerate" if args.is_empty() => {
                let pair_ty = Type::Tuple(vec![Type::I64, inner_type]);
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(pair_ty))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.zip(other) -> Vec<(T, U)>
            "zip" if args.len() == 1 => {
                let fresh = self.ctx.fresh_var();
                let pair_ty = Type::Tuple(vec![inner_type, fresh]);
                let result_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(pair_ty))],
                };
                self.unify(ret, &result_ty, span)
            }
            // vec.any(predicate) -> bool
            "any" if args.len() == 1 => {
                self.unify(ret, &Type::Bool, span)
            }
            // vec.all(predicate) -> bool
            "all" if args.len() == 1 => {
                self.unify(ret, &Type::Bool, span)
            }
            // vec.find(predicate) -> Option<T>
            "find" if args.len() == 1 => {
                let opt_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &opt_ty, span)
            }
            // vec.position(predicate) -> Option<i64>
            "position" if args.len() == 1 => {
                let opt_ty = Type::Option(Box::new(Type::I64));
                self.unify(ret, &opt_ty, span)
            }
            // vec.count() -> u64
            "count" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            // vec.take_all() -> Vec<T> (consuming iteration)
            "take_all" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            // vec.sum() -> T
            "sum" if args.is_empty() => {
                self.unify(ret, &inner_type, span)
            }
            // vec.min() -> Option<T>
            "min" if args.is_empty() => {
                let opt_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &opt_ty, span)
            }
            // vec.max() -> Option<T>
            "max" if args.is_empty() => {
                let opt_ty = Type::Option(Box::new(inner_type));
                self.unify(ret, &opt_ty, span)
            }
            // vec.clone() -> Vec<T>
            "clone" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }
            // vec.eq(other) -> bool
            "eq" | "ne" if args.len() == 1 => {
                self.unify(ret, &Type::Bool, span)
            }
            // Fall through to static methods (e.g. Vec<Route>.from(...))
            _ => self.resolve_vec_static_method(method, args, ret, span),
        }
    }

    /// Resolve static methods on bare Map (e.g. Map.new()).
    pub(super) fn resolve_map_static_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            "new" if args.is_empty() => {
                let fresh_k = self.ctx.fresh_var();
                let fresh_v = self.ctx.fresh_var();
                let map_ty = Type::UnresolvedGeneric {
                    name: "Map".to_string(),
                    args: vec![
                        GenericArg::Type(Box::new(fresh_k)),
                        GenericArg::Type(Box::new(fresh_v)),
                    ],
                };
                self.unify(ret, &map_ty, span)
            }
            // Map.from(vec_of_pairs) — construct Map from iterable
            "from" if args.len() == 1 => {
                let fresh_k = self.ctx.fresh_var();
                let fresh_v = self.ctx.fresh_var();
                let map_ty = Type::UnresolvedGeneric {
                    name: "Map".to_string(),
                    args: vec![
                        GenericArg::Type(Box::new(fresh_k)),
                        GenericArg::Type(Box::new(fresh_v)),
                    ],
                };
                self.unify(ret, &map_ty, span)
            }
            _ => {
                let map_ty = Type::UnresolvedGeneric {
                    name: "Map".to_string(),
                    args: vec![
                        GenericArg::Type(Box::new(self.ctx.fresh_var())),
                        GenericArg::Type(Box::new(self.ctx.fresh_var())),
                    ],
                };
                Err(TypeError::NoSuchMethod {
                    ty: map_ty,
                    method: method.to_string(),
                    span,
                })
            }
        }
    }

    /// Resolve instance methods on Map<K, V>.
    pub(super) fn resolve_map_method(
        &mut self,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let key_type = if let Some(GenericArg::Type(t)) = type_args.first() {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };
        let val_type = if let Some(GenericArg::Type(t)) = type_args.get(1) {
            *t.clone()
        } else {
            self.ctx.fresh_var()
        };

        match method {
            "insert" if args.len() == 2 => {
                let _ = self.unify(&args[0], &key_type, span);
                let _ = self.unify(&args[1], &val_type, span);
                self.unify(ret, &Type::I64, span)
            }
            "contains_key" if args.len() == 1 => {
                let _ = self.unify(&args[0], &key_type, span);
                self.unify(ret, &Type::Bool, span)
            }
            "get" if args.len() == 1 => {
                let _ = self.unify(&args[0], &key_type, span);
                let opt_ty = Type::Option(Box::new(val_type));
                self.unify(ret, &opt_ty, span)
            }
            "remove" if args.len() == 1 => {
                let _ = self.unify(&args[0], &key_type, span);
                self.unify(ret, &Type::I64, span)
            }
            "len" if args.is_empty() => {
                self.unify(ret, &Type::I64, span)
            }
            "is_empty" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            "clear" if args.is_empty() => {
                self.unify(ret, &Type::Unit, span)
            }
            "keys" if args.is_empty() => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(key_type))],
                };
                self.unify(ret, &vec_ty, span)
            }
            "values" if args.is_empty() => {
                let vec_ty = Type::UnresolvedGeneric {
                    name: "Vec".to_string(),
                    args: vec![GenericArg::Type(Box::new(val_type))],
                };
                self.unify(ret, &vec_ty, span)
            }
            // Fall through to static methods (e.g. Map<K,V>.new())
            _ => self.resolve_map_static_method(method, args, ret, span),
        }
    }

    /// Resolve methods on Rng (both static and instance — no type params).
    /// Try to resolve a method call via the hardcoded builtin handlers
    /// using the type's name. Returns None if the name isn't a known builtin,
    /// meaning the caller should produce its own error.
    fn resolve_builtin_method_by_name(
        &mut self,
        type_name: &str,
        type_args: &[GenericArg],
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Option<Result<bool, TypeError>> {
        // Strip generic params from name: "Vec<T>" → "Vec"
        let base_name = type_name.split('<').next().unwrap_or(type_name);
        match base_name {
            "Vec" if type_args.is_empty() => {
                Some(self.resolve_vec_static_method(method, args, ret, span))
            }
            "Vec" => {
                Some(self.resolve_vec_method(type_args, method, args, ret, span))
            }
            "Map" if type_args.is_empty() => {
                Some(self.resolve_map_static_method(method, args, ret, span))
            }
            "Map" => {
                Some(self.resolve_map_method(type_args, method, args, ret, span))
            }
            "Rng" => Some(self.resolve_rng_method(method, args, ret, span)),
            name if Self::is_atomic_type(name) => {
                Some(self.resolve_atomic_method(name, method, args, ret, span))
            }
            name if Self::is_simd_type(name) => {
                Some(self.resolve_simd_method(name, method, args, ret, span))
            }
            "Cell" | "Shared" | "Mutex" | "Sender" | "Receiver" | "Channel" if !type_args.is_empty() => {
                Some(self.resolve_concurrency_generic_method(type_name, type_args, method, args, ret, span))
            }
            name if matches!(name, "Instant" | "Duration" | "TcpListener" | "TcpConnection" | "Response" | "Request" | "Shared" | "Mutex")
                || rask_stdlib::StubRegistry::load().get_type(name).is_some() => {
                Some(self.resolve_runtime_method(name, method, args, ret, span))
            }
            _ => None,
        }
    }

    pub(super) fn resolve_rng_method(
        &mut self,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let rng_ty = Type::UnresolvedNamed("Rng".to_string());

        match method {
            "new" if args.is_empty() => {
                self.unify(ret, &rng_ty, span)
            }
            "from_seed" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                self.unify(ret, &rng_ty, span)
            }
            "u64" if args.is_empty() => {
                self.unify(ret, &Type::U64, span)
            }
            "i64" if args.is_empty() => {
                self.unify(ret, &Type::I64, span)
            }
            "f64" if args.is_empty() => {
                self.unify(ret, &Type::F64, span)
            }
            "f32" if args.is_empty() => {
                self.unify(ret, &Type::F32, span)
            }
            "bool" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            "range" if args.len() == 2 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                let _ = self.unify(&args[1], &Type::I64, span);
                self.unify(ret, &Type::I64, span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: rng_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Check whether a type name is a concrete atomic type.
    fn is_atomic_type(name: &str) -> bool {
        matches!(
            name,
            "AtomicBool"
                | "AtomicI8"
                | "AtomicU8"
                | "AtomicI16"
                | "AtomicU16"
                | "AtomicI32"
                | "AtomicU32"
                | "AtomicI64"
                | "AtomicU64"
                | "AtomicUsize"
                | "AtomicIsize"
        )
    }

    /// Map atomic type name to its value type.
    fn atomic_value_type(name: &str) -> Type {
        match name {
            "AtomicBool" => Type::Bool,
            "AtomicI8" => Type::I8,
            "AtomicU8" => Type::U8,
            "AtomicI16" => Type::I16,
            "AtomicU16" => Type::U16,
            "AtomicI32" => Type::I32,
            "AtomicU32" => Type::U32,
            "AtomicI64" => Type::I64,
            "AtomicU64" => Type::U64,
            "AtomicUsize" => Type::I64, // usize = i64 on 64-bit
            "AtomicIsize" => Type::I64, // isize = i64 on 64-bit
            _ => Type::I64,
        }
    }

    /// True for integer atomic types (not AtomicBool).
    fn is_integer_atomic(name: &str) -> bool {
        name != "AtomicBool"
    }

    /// Resolve methods on atomic types (mem.atomics spec).
    pub(super) fn resolve_atomic_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let val_ty = Self::atomic_value_type(type_name);
        let self_ty = Type::UnresolvedNamed(type_name.to_string());
        let ordering_ty = Type::UnresolvedNamed("Ordering".to_string());

        match method {
            // ── Construction ────────────────────────────────
            "new" if args.len() == 1 => {
                let _ = self.unify(&args[0], &val_ty, span);
                self.unify(ret, &self_ty, span)
            }
            "default" if args.is_empty() => {
                self.unify(ret, &self_ty, span)
            }

            // ── Load / Store / Swap ─────────────────────────
            "load" if args.len() == 1 => {
                let _ = self.unify(&args[0], &ordering_ty, span);
                self.unify(ret, &val_ty, span)
            }
            "store" if args.len() == 2 => {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &ordering_ty, span);
                self.unify(ret, &Type::Unit, span)
            }
            "swap" if args.len() == 2 => {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &ordering_ty, span);
                self.unify(ret, &val_ty, span)
            }

            // ── Compare-and-Exchange ────────────────────────
            "compare_exchange" | "compare_exchange_weak" if args.len() == 4 => {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &val_ty, span);
                let _ = self.unify(&args[2], &ordering_ty, span);
                let _ = self.unify(&args[3], &ordering_ty, span);
                let result_ty = Type::Result {
                    ok: Box::new(val_ty.clone()),
                    err: Box::new(val_ty),
                };
                self.unify(ret, &result_ty, span)
            }

            // ── Integer fetch operations ────────────────────
            "fetch_add" | "fetch_sub" | "fetch_max" | "fetch_min"
                if args.len() == 2 && Self::is_integer_atomic(type_name) =>
            {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &ordering_ty, span);
                self.unify(ret, &val_ty, span)
            }

            // ── Bitwise fetch (integers + bool) ─────────────
            "fetch_and" | "fetch_or" | "fetch_xor" | "fetch_nand" if args.len() == 2 => {
                let _ = self.unify(&args[0], &val_ty, span);
                let _ = self.unify(&args[1], &ordering_ty, span);
                self.unify(ret, &val_ty, span)
            }

            // ── Non-atomic access ───────────────────────────
            "into_inner" if args.is_empty() => {
                self.unify(ret, &val_ty, span)
            }

            // ── Integer-only fetch on AtomicBool → error ────
            "fetch_add" | "fetch_sub" | "fetch_max" | "fetch_min"
                if !Self::is_integer_atomic(type_name) =>
            {
                Err(TypeError::NoSuchMethod {
                    ty: self_ty,
                    method: method.to_string(),
                    span,
                })
            }

            _ => Err(TypeError::NoSuchMethod {
                ty: self_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Check whether a type name is a SIMD vector type.
    fn is_simd_type(name: &str) -> bool {
        matches!(
            name,
            "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8"
        )
    }

    /// Parse SIMD type name to (element Type, lane count).
    fn simd_elem_type(name: &str) -> (Type, usize) {
        match name {
            "f32x4" => (Type::F32, 4),
            "f32x8" => (Type::F32, 8),
            "f64x2" => (Type::F64, 2),
            "f64x4" => (Type::F64, 4),
            "i32x4" => (Type::I32, 4),
            "i32x8" => (Type::I32, 8),
            _ => (Type::F32, 4), // unreachable given is_simd_type guard
        }
    }

    /// Resolve methods on SIMD vector types (type.simd spec).
    pub(super) fn resolve_simd_method(
        &mut self,
        type_name: &str,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let (elem_ty, lanes) = Self::simd_elem_type(type_name);
        let self_ty = Type::UnresolvedNamed(type_name.to_string());
        let _vec_ty = Type::SimdVector {
            elem: Box::new(elem_ty.clone()),
            lanes,
        };

        match method {
            // ── Construction ────────────────────────────────
            // splat(scalar) → vec
            "splat" if args.len() == 1 => {
                let _ = self.unify(&args[0], &elem_ty, span);
                self.unify(ret, &self_ty, span)
            }
            // load(slice) → vec (static method)
            "load" if args.len() == 1 => {
                let slice_ty = Type::Slice(Box::new(elem_ty.clone()));
                let _ = self.unify(&args[0], &slice_ty, span);
                self.unify(ret, &self_ty, span)
            }

            // ── Memory ──────────────────────────────────────
            // store(slice) → ()
            "store" if args.len() == 1 => {
                let slice_ty = Type::Slice(Box::new(elem_ty.clone()));
                let _ = self.unify(&args[0], &slice_ty, span);
                self.unify(ret, &Type::Unit, span)
            }

            // ── Element-wise arithmetic ─────────────────────
            // add(other) → vec, sub(other) → vec, mul(other) → vec, div(other) → vec
            "add" | "sub" | "mul" | "div" if args.len() == 1 => {
                let _ = self.unify(&args[0], &self_ty, span);
                self.unify(ret, &self_ty, span)
            }

            // ── Scalar broadcast ops ────────────────────────
            // scale(scalar) → vec (multiply by scalar)
            "scale" if args.len() == 1 => {
                let _ = self.unify(&args[0], &elem_ty, span);
                self.unify(ret, &self_ty, span)
            }

            // ── Reductions ──────────────────────────────────
            "sum" | "product" | "min" | "max" if args.is_empty() => {
                self.unify(ret, &elem_ty, span)
            }

            // ── Lane access ─────────────────────────────────
            // get(index) → elem
            "get" if args.len() == 1 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                self.unify(ret, &elem_ty, span)
            }
            // set(index, value) → ()
            "set" if args.len() == 2 => {
                let _ = self.unify(&args[0], &Type::I64, span);
                let _ = self.unify(&args[1], &elem_ty, span);
                self.unify(ret, &Type::Unit, span)
            }

            _ => Err(TypeError::NoSuchMethod {
                ty: self_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Resolve methods on Option<T> (i.e., T?)
    fn resolve_option_method(
        &mut self,
        inner: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let self_ty = Type::Option(Box::new(inner.clone()));
        match method {
            "is_some" | "is_none" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            "unwrap" if args.is_empty() => {
                self.unify(ret, inner, span)
            }
            "unwrap_or" if args.len() == 1 => {
                let _ = self.unify(&args[0], inner, span);
                self.unify(ret, inner, span)
            }
            "map" if args.len() == 1 => {
                let result_inner = self.ctx.fresh_var();
                let expected_fn = Type::Fn {
                    params: vec![inner.clone()],
                    ret: Box::new(result_inner.clone()),
                };
                let _ = self.unify(&args[0], &expected_fn, span);
                self.unify(ret, &Type::Option(Box::new(result_inner)), span)
            }
            "filter" if args.len() == 1 => {
                let expected_fn = Type::Fn {
                    params: vec![inner.clone()],
                    ret: Box::new(Type::Bool),
                };
                let _ = self.unify(&args[0], &expected_fn, span);
                self.unify(ret, &self_ty, span)
            }
            // `x == none` desugars to `x.eq(none)` — presence/absence comparison
            "eq" if args.len() == 1 => self.unify(ret, &Type::Bool, span),
            _ => Err(TypeError::NoSuchMethod {
                ty: self_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Resolve methods on Result<T, E> (i.e., T or E)
    fn resolve_result_method(
        &mut self,
        ok: &Type,
        err: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let self_ty = Type::Result {
            ok: Box::new(ok.clone()),
            err: Box::new(err.clone()),
        };
        match method {
            "is_ok" | "is_err" if args.is_empty() => {
                self.unify(ret, &Type::Bool, span)
            }
            "unwrap" if args.is_empty() => {
                self.unify(ret, ok, span)
            }
            "unwrap_or" if args.len() == 1 => {
                let _ = self.unify(&args[0], ok, span);
                self.unify(ret, ok, span)
            }
            "map" if args.len() == 1 => {
                let result_inner = self.ctx.fresh_var();
                let expected_fn = Type::Fn {
                    params: vec![ok.clone()],
                    ret: Box::new(result_inner.clone()),
                };
                let _ = self.unify(&args[0], &expected_fn, span);
                let result_type = Type::Result {
                    ok: Box::new(result_inner),
                    err: Box::new(err.clone()),
                };
                self.unify(ret, &result_type, span)
            }
            "map_err" if args.len() == 1 => {
                let result_err = self.ctx.fresh_var();
                let expected_fn = Type::Fn {
                    params: vec![err.clone()],
                    ret: Box::new(result_err.clone()),
                };
                let _ = self.unify(&args[0], &expected_fn, span);
                let result_type = Type::Result {
                    ok: Box::new(ok.clone()),
                    err: Box::new(result_err),
                };
                self.unify(ret, &result_type, span)
            }
            "to_option" | "ok" if args.is_empty() => {
                self.unify(ret, &Type::Option(Box::new(ok.clone())), span)
            }
            _ => Err(TypeError::NoSuchMethod {
                ty: self_ty,
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Resolve methods on primitive integer types (i8..i128, u8..u128).
    /// Desugared operators (add, bit_and, etc.) resolve here instead of
    /// bouncing through HasMethod → unsolved constraint suppression.
    pub(super) fn resolve_integer_method(
        &mut self,
        ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        let is_signed = matches!(ty, Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128);
        match method {
            // Binary arithmetic → same type
            "add" | "sub" | "mul" | "div" | "rem"
            | "bit_and" | "bit_or" | "bit_xor" | "shl" | "shr"
            | "min" | "max" if args.len() == 1 => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, ty, span)
            }
            // Unary → same type
            "neg" if args.is_empty() && is_signed => self.unify(ret, ty, span),
            "bit_not" | "abs" if args.is_empty() => self.unify(ret, ty, span),
            // Comparison → bool
            "eq" | "ne" | "lt" | "le" | "gt" | "ge" if args.len() == 1 => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, &Type::Bool, span)
            }
            "compare" if args.len() == 1 => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, &Type::UnresolvedNamed("Ordering".to_string()), span)
            }
            "to_float" if args.is_empty() => self.unify(ret, &Type::F64, span),
            _ => Err(TypeError::NoSuchMethod {
                ty: ty.clone(),
                method: method.to_string(),
                span,
            }),
        }
    }

    /// Resolve methods on primitive float types (f32, f64).
    pub(super) fn resolve_float_method(
        &mut self,
        ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
    ) -> Result<bool, TypeError> {
        match method {
            "add" | "sub" | "mul" | "div"
            | "min" | "max" | "pow" if args.len() == 1 => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, ty, span)
            }
            "neg" | "abs" | "floor" | "ceil" | "round" | "sqrt" if args.is_empty() => {
                self.unify(ret, ty, span)
            }
            "eq" | "ne" | "lt" | "le" | "gt" | "ge" if args.len() == 1 => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, &Type::Bool, span)
            }
            "compare" if args.len() == 1 => {
                let _ = self.unify(&args[0], ty, span);
                self.unify(ret, &Type::UnresolvedNamed("Ordering".to_string()), span)
            }
            "to_int" if args.is_empty() => self.unify(ret, &Type::I64, span),
            _ => Err(TypeError::NoSuchMethod {
                ty: ty.clone(),
                method: method.to_string(),
                span,
            }),
        }
    }
}
