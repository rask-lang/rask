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
