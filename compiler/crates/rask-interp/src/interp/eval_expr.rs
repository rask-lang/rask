// SPDX-License-Identifier: (MIT OR Apache-2.0)
//\! Expression evaluation.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use rask_ast::expr::{BinOp, Expr, ExprKind, UnaryOp};

use crate::value::{ModuleKind, PoolTask, ThreadHandleInner, ThreadPoolInner, TypeConstructorKind, Value};

use super::{Interpreter, RuntimeDiagnostic, RuntimeError};

impl Interpreter {
    pub(crate) fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeDiagnostic> {
        match &expr.kind {
            ExprKind::Int(n, suffix) => {
                use rask_ast::token::IntSuffix;
                match suffix {
                    Some(IntSuffix::I128) => Ok(Value::Int128(*n as i128)),
                    Some(IntSuffix::U128) => Ok(Value::Uint128(*n as u128)),
                    _ => Ok(Value::Int(*n)),
                }
            }
            ExprKind::Float(n, _) => Ok(Value::Float(*n)),
            ExprKind::String(s) => {
                if s.contains('{') {
                    let interpolated = self.interpolate_string(s)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                    Ok(Value::String(Arc::new(Mutex::new(interpolated))))
                } else {
                    Ok(Value::String(Arc::new(Mutex::new(s.clone()))))
                }
            }
            ExprKind::Char(c) => Ok(Value::Char(*c)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),

            ExprKind::Ident(name) => {
                if let Some(val) = self.env.get(name) {
                    return Ok(val.clone());
                }
                if self.functions.contains_key(name) {
                    return Ok(Value::Function { name: name.clone() });
                }
                // Check for generic type constructors (e.g., Pool<Node>)
                let (base_name, type_param) = if let Some(lt_pos) = name.find('<') {
                    if let Some(gt_pos) = name.rfind('>') {
                        let base = &name[..lt_pos];
                        let param = name[lt_pos + 1..gt_pos].trim();
                        (base, Some(param.to_string()))
                    } else {
                        (name.as_str(), None)
                    }
                } else {
                    (name.as_str(), None)
                };

                match base_name {
                    "Vec" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Vec,
                        type_param,
                    }),
                    "Map" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Map,
                        type_param,
                    }),
                    "string" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::String,
                        type_param,
                    }),
                    "Pool" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Pool,
                        type_param,
                    }),
                    "Channel" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Channel,
                        type_param,
                    }),
                    "Shared" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Shared,
                        type_param,
                    }),
                    "Atomic" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Atomic,
                        type_param,
                    }),
                    "Ordering" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Ordering,
                        type_param,
                    }),
                    "f32x8" => return Ok(Value::Type("f32x8".to_string())),
                    _ => {}
                }
                // User-defined struct types (e.g., Box, Pair)
                if self.struct_decls.contains_key(base_name) {
                    return Ok(Value::Type(base_name.to_string()));
                }
                Err(RuntimeDiagnostic::new(RuntimeError::UndefinedVariable(name.clone()), expr.span))
            }

            ExprKind::Call { func, args } => {
                if let ExprKind::OptionalField { object, field } = &func.kind {
                    let obj_val = self.eval_expr(object)?;
                    let arg_vals: Vec<Value> = args
                        .iter()
                        .map(|a| self.eval_expr(&a.expr))
                        .collect::<Result<_, _>>()?;

                    if let Value::Enum {
                        name,
                        variant,
                        fields,
                    } = &obj_val
                    {
                        if name == "Result" {
                            match variant.as_str() {
                                "Ok" => {
                                    let inner = fields.first().cloned().unwrap_or(Value::Unit);
                                    return self.call_method(inner, field, arg_vals)
                                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span));
                                }
                                "Err" => {
                                    return Err(RuntimeDiagnostic::new(RuntimeError::TryError(obj_val), expr.span));
                                }
                                _ => {}
                            }
                        } else if name == "Option" {
                            match variant.as_str() {
                                "Some" => {
                                    let inner = fields.first().cloned().unwrap_or(Value::Unit);
                                    let result = self.call_method(inner, field, arg_vals)
                                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                                    return Ok(Value::Enum {
                                        name: "Option".to_string(),
                                        variant: "Some".to_string(),
                                        fields: vec![result],
                                    });
                                }
                                "None" => {
                                    return Ok(Value::Enum {
                                        name: "Option".to_string(),
                                        variant: "None".to_string(),
                                        fields: vec![],
                                    });
                                }
                                _ => {}
                            }
                        }
                    }

                    return self.call_method(obj_val, field, arg_vals)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span));
                }

                let func_val = self.eval_expr(func)?;
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(&a.expr))
                    .collect::<Result<_, _>>()?;
                self.call_value(func_val, arg_vals)
                    .map_err(|e| RuntimeDiagnostic::new(e, expr.span))
            }

            ExprKind::MethodCall {
                object,
                method,
                type_args,
                args,
            } => {
                if let ExprKind::Ident(name) = &object.kind {
                    if let Some(enum_decl) = self.enums.get(name).cloned() {
                        if let Some(variant) = enum_decl.variants.iter().find(|v| &v.name == method)
                        {
                            let field_count = variant.fields.len();
                            let arg_vals: Vec<Value> = args
                                .iter()
                                .map(|a| self.eval_expr(&a.expr))
                                .collect::<Result<_, _>>()?;
                            if arg_vals.len() != field_count {
                                return Err(RuntimeDiagnostic::new(
                                    RuntimeError::ArityMismatch {
                                        expected: field_count,
                                        got: arg_vals.len(),
                                    },
                                    expr.span
                                ));
                            }
                            return Ok(Value::Enum {
                                name: name.clone(),
                                variant: method.clone(),
                                fields: arg_vals,
                            });
                        }
                    }

                    if let Some(type_methods) = self.methods.get(name).cloned() {
                        if let Some(method_fn) = type_methods.get(method) {
                            let is_static = method_fn
                                .params
                                .first()
                                .map(|p| p.name != "self")
                                .unwrap_or(true);
                            if is_static {
                                let arg_vals: Vec<Value> = args
                                    .iter()
                                    .map(|a| self.eval_expr(&a.expr))
                                    .collect::<Result<_, _>>()?;
                                return self.call_function(method_fn, arg_vals);
                            }
                        }
                    }
                }

                let receiver = self.eval_expr(object)?;
                let mut arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(&a.expr))
                    .collect::<Result<_, _>>()?;

                // Inject type_args for generic methods (e.g. json.decode<T>)
                if let Some(ta) = type_args {
                    if let Some(first_type) = ta.first() {
                        if let Value::Module(ModuleKind::Json) = &receiver {
                            if method == "decode" || method == "from_value" {
                                arg_vals.insert(
                                    0,
                                    Value::String(Arc::new(Mutex::new(first_type.clone()))),
                                );
                            }
                        }
                    }
                }

                if let Value::Type(type_name) = &receiver {
                    return self.call_type_method(type_name, method, arg_vals)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span));
                }

                self.call_method(receiver, method, arg_vals)
                    .map_err(|e| RuntimeDiagnostic::new(e, expr.span))
            }

            ExprKind::Binary { op, left, right } => match op {
                BinOp::And => {
                    let l = self.eval_expr(left)?;
                    if !self.is_truthy(&l) {
                        Ok(Value::Bool(false))
                    } else {
                        let r = self.eval_expr(right)?;
                        Ok(Value::Bool(self.is_truthy(&r)))
                    }
                }
                BinOp::Or => {
                    let l = self.eval_expr(left)?;
                    if self.is_truthy(&l) {
                        Ok(Value::Bool(true))
                    } else {
                        let r = self.eval_expr(right)?;
                        Ok(Value::Bool(self.is_truthy(&r)))
                    }
                }
                _ => {
                    // Arithmetic ops: handle directly (needed for string interpolation
                    // expressions which bypass the desugaring pass)
                    let l = self.eval_expr(left)?;
                    let r = self.eval_expr(right)?;
                    self.eval_binop(*op, l, r)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span))
                }
            },

            ExprKind::Unary { op, operand } => {
                let val = self.eval_expr(operand)?;
                match op {
                    UnaryOp::Not => match val {
                        Value::Bool(b) => Ok(Value::Bool(!b)),
                        _ => Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "! requires bool, got {}",
                                val.type_name()
                            )),
                            expr.span
                        )),
                    },
                    UnaryOp::Neg => match val {
                        Value::Int(n) => Ok(Value::Int(-n)),
                        Value::Float(n) => Ok(Value::Float(-n)),
                        _ => Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "- requires number, got {}",
                                val.type_name()
                            )),
                            expr.span
                        )),
                    },
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "unhandled unary op {:?}",
                            op
                        )),
                        expr.span
                    )),
                }
            }

            ExprKind::Block(stmts) => {
                self.env.push_scope();
                let result = self.exec_stmts(stmts);
                self.env.pop_scope();
                result
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_val = self.eval_expr(cond)?;
                if self.is_truthy(&cond_val) {
                    self.eval_expr(then_branch)
                } else if let Some(else_br) = else_branch {
                    self.eval_expr(else_br)
                } else {
                    Ok(Value::Unit)
                }
            }

            ExprKind::Range {
                start,
                end,
                inclusive,
            } => {
                let start_val = if let Some(s) = start {
                    match self.eval_expr(s)? {
                        Value::Int(n) => n,
                        v => {
                            return Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "range start must be int, got {}",
                                    v.type_name()
                                )),
                                expr.span
                            ))
                        }
                    }
                } else {
                    0
                };
                let end_val = if let Some(e) = end {
                    match self.eval_expr(e)? {
                        Value::Int(n) => n,
                        v => {
                            return Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "range end must be int, got {}",
                                    v.type_name()
                                )),
                                expr.span
                            ))
                        }
                    }
                } else {
                    i64::MAX
                };
                Ok(Value::Range {
                    start: start_val,
                    end: end_val,
                    inclusive: *inclusive,
                })
            }

            ExprKind::StructLit { name, fields, spread } => {
                // Check if this is a generic instantiation
                let concrete_name = if name.contains('<') {
                    // Parse generic arguments and monomorphize
                    self.monomorphize_struct_from_name(name)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?
                } else {
                    name.clone()
                };

                let mut field_values = HashMap::new();

                if let Some(spread_expr) = spread {
                    if let Value::Struct {
                        fields: base_fields,
                        ..
                    } = self.eval_expr(spread_expr)?
                    {
                        field_values.extend(base_fields);
                    }
                }

                for field in fields {
                    let value = self.eval_expr(&field.value)?;
                    field_values.insert(field.name.clone(), value);
                }

                let resource_id = if self.is_resource_type(&concrete_name) {
                    Some(self.resource_tracker.register(&concrete_name, self.env.scope_depth()))
                } else {
                    None
                };

                Ok(Value::Struct {
                    name: concrete_name,
                    fields: field_values,
                    resource_id,
                })
            }

            ExprKind::Field { object, field } => {
                if let ExprKind::Ident(enum_name) = &object.kind {
                    if let Some(enum_decl) = self.enums.get(enum_name).cloned() {
                        if let Some(variant) =
                            enum_decl.variants.iter().find(|v| &v.name == field)
                        {
                            let field_count = variant.fields.len();
                            if field_count == 0 {
                                return Ok(Value::Enum {
                                    name: enum_name.clone(),
                                    variant: field.clone(),
                                    fields: vec![],
                                });
                            } else {
                                return Ok(Value::EnumConstructor {
                                    enum_name: enum_name.clone(),
                                    variant_name: field.clone(),
                                    field_count,
                                });
                            }
                        }
                    }
                }

                let obj = self.eval_expr(object)?;
                match obj {
                    Value::Struct { fields, .. } => {
                        Ok(fields.get(field).cloned().unwrap_or(Value::Unit))
                    }
                    Value::Module(ModuleKind::Time) => {
                        match field.as_str() {
                            "Instant" => Ok(Value::Type("Instant".to_string())),
                            "Duration" => Ok(Value::Type("Duration".to_string())),
                            _ => Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "time module has no member '{}'",
                                    field
                                )),
                                expr.span
                            )),
                        }
                    }
                    Value::Module(ModuleKind::Math) => {
                        self.get_math_field(field)
                            .map_err(|e| RuntimeDiagnostic::new(e, expr.span))
                    }
                    Value::Module(ModuleKind::Path) => {
                        match field.as_str() {
                            "Path" => Ok(Value::Type("Path".to_string())),
                            _ => Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "path module has no member '{}'",
                                    field
                                )),
                                expr.span
                            )),
                        }
                    }
                    Value::Module(ModuleKind::Random) => {
                        match field.as_str() {
                            "Rng" => Ok(Value::Type("Rng".to_string())),
                            _ => Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "random module has no member '{}'",
                                    field
                                )),
                                expr.span
                            )),
                        }
                    }
                    Value::Module(ModuleKind::Json) => {
                        match field.as_str() {
                            "JsonValue" => Ok(Value::Type("JsonValue".to_string())),
                            _ => Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "json module has no member '{}'",
                                    field
                                )),
                                expr.span
                            )),
                        }
                    }
                    Value::Module(ModuleKind::Cli) => {
                        match field.as_str() {
                            "Parser" => Ok(Value::Type("Parser".to_string())),
                            _ => Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "cli module has no member '{}'",
                                    field
                                )),
                                expr.span
                            )),
                        }
                    }
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "cannot access field on {}",
                            obj.type_name()
                        )),
                        expr.span
                    )),
                }
            }

            ExprKind::Index { object, index } => {
                let obj = self.eval_expr(object)?;
                let idx = self.eval_expr(index)?;

                match (&obj, &idx) {
                    (Value::Vec(v), Value::Int(i)) => {
                        let vec = v.lock().unwrap();
                        Ok(vec.get(*i as usize).cloned().unwrap_or(Value::Unit))
                    }
                    (Value::Vec(v), Value::Range { start, end, inclusive }) => {
                        let vec = v.lock().unwrap();
                        let len = vec.len() as i64;
                        let start_idx = (*start).max(0).min(len) as usize;
                        let end_idx = if *end == i64::MAX {
                            vec.len()
                        } else {
                            let e = if *inclusive { *end + 1 } else { *end };
                            e.max(0).min(len) as usize
                        };
                        let slice: Vec<Value> = vec[start_idx..end_idx].to_vec();
                        Ok(Value::Vec(Arc::new(Mutex::new(slice))))
                    }
                    (Value::String(s), Value::Int(i)) => Ok(s
                        .lock().unwrap()
                        .chars()
                        .nth(*i as usize)
                        .map(Value::Char)
                        .unwrap_or(Value::Unit)),
                    (Value::String(s), Value::Range { start, end, inclusive }) => {
                        let str_val = s.lock().unwrap();
                        let len = str_val.len() as i64;
                        let start_idx = (*start).max(0).min(len) as usize;
                        let end_idx = if *end == i64::MAX {
                            str_val.len()
                        } else {
                            let e = if *inclusive { *end + 1 } else { *end };
                            e.max(0).min(len) as usize
                        };
                        let slice = &str_val[start_idx..end_idx];
                        Ok(Value::String(Arc::new(Mutex::new(slice.to_string()))))
                    }
                    (
                        Value::Pool(p),
                        Value::Handle {
                            pool_id,
                            index,
                            generation,
                        },
                    ) => {
                        let pool = p.lock().unwrap();
                        let idx = pool
                            .validate(*pool_id, *index, *generation)
                            .map_err(|e| RuntimeDiagnostic::new(RuntimeError::Panic(e), expr.span))?;
                        Ok(pool.slots[idx].1.as_ref().unwrap().clone())
                    }
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "cannot index {} with {}",
                            obj.type_name(),
                            idx.type_name()
                        )),
                        expr.span
                    )),
                }
            }

            ExprKind::Array(elements) => {
                let values: Vec<Value> = elements
                    .iter()
                    .map(|e| self.eval_expr(e))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Vec(Arc::new(Mutex::new(values))))
            }

            ExprKind::ArrayRepeat { value, count } => {
                let val = self.eval_expr(value)?;
                let n = match self.eval_expr(count)? {
                    Value::Int(n) => n as usize,
                    other => return Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "array repeat count must be integer, found {}", other.type_name()
                        )),
                        expr.span
                    )),
                };
                let values: Vec<Value> = (0..n).map(|_| val.clone()).collect();
                Ok(Value::Vec(Arc::new(Mutex::new(values))))
            }

            ExprKind::Tuple(elements) => {
                let values: Vec<Value> = elements
                    .iter()
                    .map(|e| self.eval_expr(e))
                    .collect::<Result<_, _>>()?;
                Ok(Value::Vec(Arc::new(Mutex::new(values))))
            }

            ExprKind::Match { scrutinee, arms } => {
                let value = self.eval_expr(scrutinee)?;

                for arm in arms {
                    if let Some(bindings) = self.match_pattern(&arm.pattern, &value) {
                        if let Some(guard) = &arm.guard {
                            self.env.push_scope();
                            for (name, val) in &bindings {
                                self.env.define(name.clone(), val.clone());
                            }
                            let guard_result = self.eval_expr(guard)?;
                            self.env.pop_scope();
                            if !self.is_truthy(&guard_result) {
                                continue;
                            }
                        }

                        self.env.push_scope();
                        for (name, val) in bindings {
                            self.env.define(name, val);
                        }
                        let result = self.eval_expr(&arm.body);
                        self.env.pop_scope();
                        return result;
                    }
                }

                Err(RuntimeDiagnostic::new(RuntimeError::NoMatchingArm, expr.span))
            }

            ExprKind::IfLet {
                expr,
                pattern,
                then_branch,
                else_branch,
            } => {
                let value = self.eval_expr(expr)?;

                if let Some(bindings) = self.match_pattern(pattern, &value) {
                    self.env.push_scope();
                    for (name, val) in bindings {
                        self.env.define(name, val);
                    }
                    let result = self.eval_expr(then_branch);
                    self.env.pop_scope();
                    result
                } else if let Some(else_br) = else_branch {
                    self.eval_expr(else_br)
                } else {
                    Ok(Value::Unit)
                }
            }

            ExprKind::IsPattern { expr: inner, pattern } => {
                let value = self.eval_expr(inner)?;
                let matched = self.match_pattern(pattern, &value).is_some();
                Ok(Value::Bool(matched))
            }

            ExprKind::Try(inner) => {
                let val = self.eval_expr(inner)?;
                match &val {
                    Value::Enum {
                        variant, fields, ..
                    } => match variant.as_str() {
                        "Ok" | "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                        "Err" | "None" => Err(RuntimeDiagnostic::new(RuntimeError::TryError(val), expr.span)),
                        _ => Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "? operator requires Ok/Some or Err/None variant, got {}",
                                variant
                            )),
                            expr.span
                        )),
                    },
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "? operator requires Result or Option, got {}",
                            val.type_name()
                        )),
                        expr.span
                    )),
                }
            }

            ExprKind::Unwrap { expr: inner, message } => {
                let val = self.eval_expr(inner)?;
                match &val {
                    Value::Enum {
                        variant, fields, ..
                    } => match variant.as_str() {
                        "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                        "None" => {
                            if let Some(msg) = message {
                                Err(RuntimeDiagnostic::new(
                                    RuntimeError::Panic(msg.clone()),
                                    expr.span
                                ))
                            } else {
                                Err(RuntimeDiagnostic::new(RuntimeError::UnwrapError, expr.span))
                            }
                        }
                        "Ok" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                        "Err" => {
                            if let Some(msg) = message {
                                Err(RuntimeDiagnostic::new(
                                    RuntimeError::Panic(msg.clone()),
                                    expr.span
                                ))
                            } else {
                                Err(RuntimeDiagnostic::new(RuntimeError::UnwrapError, expr.span))
                            }
                        }
                        _ => Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "! operator requires Option or Result, got {}",
                                variant
                            )),
                            expr.span
                        )),
                    },
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "! operator requires Option or Result, got {}",
                            val.type_name()
                        )),
                        expr.span
                    )),
                }
            }

            ExprKind::Closure { params, body, .. } => {
                let captured = self.env.capture();
                Ok(Value::Closure {
                    params: params.iter().map(|p| p.name.clone()).collect(),
                    body: (**body).clone(),
                    captured_env: captured,
                })
            }

            ExprKind::Cast { expr, ty } => {
                let val = self.eval_expr(expr)?;
                match (val, ty.as_str()) {
                    (Value::Int(n), "f64" | "f32" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Float(n), "i64" | "i32" | "int" | "i16" | "i8") => {
                        Ok(Value::Int(n as i64))
                    }
                    (Value::Float(n), "u64" | "u32" | "u16" | "u8" | "usize") => {
                        Ok(Value::Int(n as i64))
                    }
                    (Value::Int(n), "i64" | "i32" | "int" | "i16" | "i8" | "u64" | "u32"
                        | "u16" | "u8" | "usize") => Ok(Value::Int(n)),
                    (Value::Int(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Float(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Char(c), "i32" | "i64" | "int" | "u32" | "u8" | "u64" | "usize") => {
                        Ok(Value::Int(c as i64))
                    }
                    (Value::Int(n), "char") => {
                        Ok(Value::Char(char::from_u32(n as u32).unwrap_or('\0')))
                    }
                    // i128 conversions
                    (Value::Int(n), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Int(n), "u128") => Ok(Value::Uint128(n as u128)),
                    (Value::Int128(n), "i64" | "i32" | "int" | "i16" | "i8") => Ok(Value::Int(n as i64)),
                    (Value::Int128(n), "u64" | "u32" | "u16" | "u8" | "usize" | "u128") => Ok(Value::Uint128(n as u128)),
                    (Value::Int128(n), "f64" | "f32" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Int128(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    // u128 conversions
                    (Value::Uint128(n), "i64" | "i32" | "int" | "i16" | "i8") => Ok(Value::Int(n as i64)),
                    (Value::Uint128(n), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Uint128(n), "f64" | "f32" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Uint128(n), "u128" | "u64" | "u32" | "u16" | "u8" | "usize") => Ok(Value::Uint128(n)),
                    (Value::Uint128(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Float(n), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Float(n), "u128") => Ok(Value::Uint128(n as u128)),
                    (v, _) => Ok(v),
                }
            }

            ExprKind::NullCoalesce { value, default } => {
                let val = self.eval_expr(value)?;
                match &val {
                    Value::Enum { name, variant, fields, .. }
                        if name == "Option" && variant == "Some" =>
                    {
                        Ok(fields.first().cloned().unwrap_or(Value::Unit))
                    }
                    Value::Enum { name, variant, .. }
                        if name == "Option" && variant == "None" =>
                    {
                        self.eval_expr(default)
                    }
                    _ => Ok(val),
                }
            }

            ExprKind::BlockCall { name, body } if name == "spawn_raw" => {
                let body = body.clone();
                let captured = self.env.capture();
                let child = self.spawn_child(captured);

                let join_handle = std::thread::spawn(move || {
                    let mut interp = child;
                    let mut result = Value::Unit;
                    for stmt in &body {
                        match interp.exec_stmt(stmt) {
                            Ok(val) => result = val,
                            Err(e) => return Err(format!("{}", e)),
                        }
                    }
                    Ok(result)
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                })))
            }

            ExprKind::BlockCall { name, body } if name == "spawn_thread" => {
                let pool = self.env.get("__thread_pool").cloned();
                let pool = match pool {
                    Some(Value::ThreadPool(p)) => p,
                    _ => {
                        return Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(
                                "spawn_thread requires `ThreadPool` in scope".to_string(),
                            ),
                            expr.span
                        ))
                    }
                };

                let body = body.clone();
                let captured = self.env.capture();
                let child = self.spawn_child(captured);

                let (result_tx, result_rx) = mpsc::sync_channel::<Result<Value, String>>(1);

                let task = PoolTask {
                    work: Box::new(move || {
                        let mut interp = child;
                        let mut result = Value::Unit;
                        for stmt in &body {
                            match interp.exec_stmt(stmt) {
                                Ok(val) => result = val,
                                Err(e) => {
                                    let _ = result_tx.send(Err(format!("{}", e)));
                                    return;
                                }
                            }
                        }
                        let _ = result_tx.send(Ok(result));
                    }),
                };

                let sender = pool.sender.lock().unwrap();
                if let Some(ref tx) = *sender {
                    tx.send(task).map_err(|_| {
                        RuntimeDiagnostic::new(
                            RuntimeError::ResourceClosed { resource_type: "ThreadPool".to_string(), operation: "spawn on".to_string() },
                            expr.span
                        )
                    })?;
                } else {
                    return Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(
                            "thread pool is shut down".to_string(),
                        ),
                        expr.span
                    ));
                }

                let join_handle = std::thread::spawn(move || {
                    result_rx
                        .recv()
                        .unwrap_or(Err("thread pool task dropped".to_string()))
                });

                Ok(Value::ThreadHandle(Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                })))
            }

            ExprKind::UsingBlock { name, args, body }
                if name == "ThreadPool" || name == "threading" =>
            {
                let num_threads = if args.is_empty() {
                    std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(4)
                } else {
                    self.eval_expr(&args[0].expr)?.as_int()
                        .map_err(|e| RuntimeDiagnostic::new(RuntimeError::TypeError(e), expr.span))? as usize
                };

                let (tx, rx) = mpsc::channel::<PoolTask>();
                let rx = Arc::new(Mutex::new(rx));
                let mut workers = Vec::with_capacity(num_threads);

                for _ in 0..num_threads {
                    let rx = Arc::clone(&rx);
                    workers.push(std::thread::spawn(move || {
                        loop {
                            let task = {
                                let rx = rx.lock().unwrap();
                                rx.recv()
                            };
                            match task {
                                Ok(task) => (task.work)(),
                                Err(_) => break,
                            }
                        }
                    }));
                }

                let pool = Arc::new(ThreadPoolInner {
                    sender: Mutex::new(Some(tx)),
                    workers: Mutex::new(Vec::new()),
                    size: num_threads,
                });

                self.env.push_scope();
                self.env.define("__thread_pool".to_string(), Value::ThreadPool(pool.clone()));

                let mut result = Value::Unit;
                for stmt in body {
                    match self.exec_stmt(stmt) {
                        Ok(val) => result = val,
                        Err(e) => {
                            *pool.sender.lock().unwrap() = None;
                            for w in workers {
                                let _ = w.join();
                            }
                            self.env.pop_scope();
                            return Err(e);
                        }
                    }
                }

                *pool.sender.lock().unwrap() = None;
                for w in workers {
                    let _ = w.join();
                }
                self.env.pop_scope();
                Ok(result)
            }

            ExprKind::UsingBlock { name, args, body }
                if name == "Multitasking" || name == "multitasking" =>
            {
                use crate::value::MultitaskingRuntime;

                let num_workers = if args.is_empty() {
                    std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(4)
                } else {
                    self.eval_expr(&args[0].expr)?.as_int()
                        .map_err(|e| RuntimeDiagnostic::new(RuntimeError::TypeError(e), expr.span))? as usize
                };

                let runtime = Arc::new(MultitaskingRuntime {
                    workers: num_workers,
                });

                self.env.push_scope();
                let scope_depth = self.env.scope_depth();
                self.env.define("__multitasking_ctx".to_string(), Value::MultitaskingRuntime(runtime));

                let mut result = Value::Unit;
                for stmt in body {
                    match self.exec_stmt(stmt) {
                        Ok(val) => result = val,
                        Err(e) => {
                            self.env.pop_scope();
                            return Err(e);
                        }
                    }
                }

                // Check for unconsumed handles (conc.async/H1)
                if let Err(msg) = self.resource_tracker.check_scope_exit(scope_depth) {
                    self.env.pop_scope();
                    return Err(RuntimeDiagnostic::new(
                        RuntimeError::Panic(msg),
                        expr.span,
                    ));
                }

                self.env.pop_scope();
                Ok(result)
            }

            ExprKind::Spawn { body } => {
                let body = body.clone();
                let captured = self.env.capture();
                let child = self.spawn_child(captured);

                let join_handle = std::thread::spawn(move || {
                    let mut interp = child;
                    let mut result = Value::Unit;
                    for stmt in &body {
                        match interp.exec_stmt(stmt) {
                            Ok(val) => result = val,
                            Err(e) => return Err(format!("{}", e)),
                        }
                    }
                    Ok(result)
                });

                // Return TaskHandle when inside using Multitasking, ThreadHandle otherwise
                let handle_inner = Arc::new(ThreadHandleInner {
                    handle: Mutex::new(Some(join_handle)),
                });
                let in_multitasking = self.env.get("__multitasking_ctx").is_some();

                // Register handle for affine tracking (conc.async/H1)
                let ptr = Arc::as_ptr(&handle_inner) as usize;
                let type_name = if in_multitasking { "TaskHandle" } else { "ThreadHandle" };
                self.resource_tracker.register_handle(ptr, type_name, self.env.scope_depth());

                if in_multitasking {
                    Ok(Value::TaskHandle(handle_inner))
                } else {
                    Ok(Value::ThreadHandle(handle_inner))
                }
            }

            ExprKind::Assert { condition, message } => {
                let cond_val = self.eval_expr(condition)?;
                if self.is_truthy(&cond_val) {
                    Ok(Value::Unit)
                } else {
                    let msg = if let Some(msg_expr) = message {
                        let v = self.eval_expr(msg_expr)?;
                        format!("{}", v)
                    } else {
                        "assertion failed".to_string()
                    };
                    Err(RuntimeDiagnostic::new(RuntimeError::AssertionFailed(msg), expr.span))
                }
            }

            ExprKind::Check { condition, message } => {
                let cond_val = self.eval_expr(condition)?;
                if self.is_truthy(&cond_val) {
                    Ok(Value::Unit)
                } else {
                    let msg = if let Some(msg_expr) = message {
                        let v = self.eval_expr(msg_expr)?;
                        format!("{}", v)
                    } else {
                        "check failed".to_string()
                    };
                    Err(RuntimeDiagnostic::new(RuntimeError::CheckFailed(msg), expr.span))
                }
            }

            ExprKind::WithAs { bindings, body } => {
                // Collect (collection_value, key_value, binding_name, cloned_element)
                struct BindingInfo {
                    collection: Value,
                    key: Value,
                    name: String,
                }
                let mut infos: Vec<BindingInfo> = Vec::new();

                for (source_expr, binding_name) in bindings {
                    // Source must be an Index expression: collection[key]
                    if let ExprKind::Index { object, index } = &source_expr.kind {
                        let collection = self.eval_expr(object)?;
                        let key = self.eval_expr(index)?;
                        infos.push(BindingInfo {
                            collection,
                            key,
                            name: binding_name.clone(),
                        });
                    } else {
                        return Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(
                                "with...as source must be a collection index (e.g., pool[h])".to_string(),
                            ),
                            expr.span
                        ));
                    }
                }

                // Check aliasing: same-collection bindings must have different keys
                for i in 0..infos.len() {
                    for j in (i + 1)..infos.len() {
                        if Self::value_eq(&infos[i].collection, &infos[j].collection)
                            && Self::value_eq(&infos[i].key, &infos[j].key)
                        {
                            return Err(RuntimeDiagnostic::new(
                                RuntimeError::Panic(
                                    "with...as: duplicate key in same collection (aliasing)".to_string(),
                                ),
                                expr.span
                            ));
                        }
                    }
                }

                // Read current values and push scope with bindings
                self.env.push_scope();
                for info in &infos {
                    let elem = self.index_into(&info.collection, &info.key)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                    self.env.define(info.name.clone(), elem);
                }

                // Execute body
                let mut result = Value::Unit;
                for stmt in body {
                    result = self.exec_stmt(stmt)?;
                }

                // Writeback: read binding values and write back to collections
                for info in &infos {
                    if let Some(updated) = self.env.get(&info.name).cloned() {
                        self.write_back_index(&info.collection, &info.key, updated)
                            .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                    }
                }

                self.env.pop_scope();
                Ok(result)
            }

            ExprKind::Comptime { body } => {
                self.env.push_scope();
                let result = self.exec_stmts(body);
                self.env.pop_scope();
                result
            }

            ExprKind::Unsafe { body } => {
                // Unsafe relaxes static checks — the interpreter has none, so evaluate as block
                self.env.push_scope();
                let result = self.exec_stmts(body);
                self.env.pop_scope();
                result
            }

            // Select: channel multiplexing (conc.select/A1-A3, P1-P2)
            ExprKind::Select { arms, is_priority } => {
                use rask_ast::expr::SelectArmKind;

                if arms.is_empty() {
                    return Err(RuntimeDiagnostic::new(
                        RuntimeError::Panic("select with zero arms [conc.select/P3]".to_string()),
                        expr.span,
                    ));
                }

                // Evaluate channel expressions up front
                struct SelectEntry {
                    kind: EvalSelectKind,
                    arm_idx: usize,
                }
                enum EvalSelectKind {
                    Recv {
                        rx: Arc<Mutex<mpsc::Receiver<Value>>>,
                        binding: String,
                    },
                    Send {
                        tx: Arc<Mutex<mpsc::SyncSender<Value>>>,
                        value: Value,
                    },
                    Default,
                }

                let mut entries = Vec::new();
                let mut default_idx: Option<usize> = None;

                for (i, arm) in arms.iter().enumerate() {
                    match &arm.kind {
                        SelectArmKind::Recv { channel, binding } => {
                            let ch_val = self.eval_expr(channel)?;
                            match ch_val {
                                Value::Receiver(rx) => {
                                    entries.push(SelectEntry {
                                        kind: EvalSelectKind::Recv {
                                            rx,
                                            binding: binding.clone(),
                                        },
                                        arm_idx: i,
                                    });
                                }
                                _ => {
                                    return Err(RuntimeDiagnostic::new(
                                        RuntimeError::TypeError(format!(
                                            "select recv arm expects Receiver, got {}",
                                            ch_val.type_name()
                                        )),
                                        expr.span,
                                    ));
                                }
                            }
                        }
                        SelectArmKind::Send { channel, value } => {
                            let ch_val = self.eval_expr(channel)?;
                            let send_val = self.eval_expr(value)?;
                            match ch_val {
                                Value::Sender(tx) => {
                                    entries.push(SelectEntry {
                                        kind: EvalSelectKind::Send {
                                            tx,
                                            value: send_val,
                                        },
                                        arm_idx: i,
                                    });
                                }
                                _ => {
                                    return Err(RuntimeDiagnostic::new(
                                        RuntimeError::TypeError(format!(
                                            "select send arm expects Sender, got {}",
                                            ch_val.type_name()
                                        )),
                                        expr.span,
                                    ));
                                }
                            }
                        }
                        SelectArmKind::Default => {
                            default_idx = Some(i);
                        }
                    }
                }

                // Build poll order: sequential for priority, shuffled for fair
                let mut poll_order: Vec<usize> = (0..entries.len()).collect();
                if !is_priority {
                    // Simple shuffle using system time as seed (P1: random fair)
                    let seed = std::time::SystemTime::now()
                        .duration_since(std::time::SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .subsec_nanos() as u64;
                    let mut rng = seed;
                    for i in (1..poll_order.len()).rev() {
                        rng = rng.wrapping_mul(6364136223846793005).wrapping_add(1);
                        let j = (rng as usize) % (i + 1);
                        poll_order.swap(i, j);
                    }
                }

                // Poll loop with backoff
                let mut backoff_us: u64 = 10; // start at 10μs
                let max_backoff_us: u64 = 1000; // cap at 1ms

                loop {
                    let mut all_closed = true;

                    for &entry_idx in &poll_order {
                        let entry = &entries[entry_idx];
                        match &entry.kind {
                            EvalSelectKind::Recv { rx, binding } => {
                                let rx_guard = rx.lock().unwrap();
                                match rx_guard.try_recv() {
                                    Ok(val) => {
                                        drop(rx_guard);
                                        // Execute this arm's body with binding
                                        self.env.push_scope();
                                        self.env.define(binding.clone(), val);
                                        let result = self.eval_expr(&arms[entry.arm_idx].body)?;
                                        self.env.pop_scope();
                                        return Ok(result);
                                    }
                                    Err(mpsc::TryRecvError::Empty) => {
                                        all_closed = false;
                                    }
                                    Err(mpsc::TryRecvError::Disconnected) => {
                                        // Channel closed, skip
                                    }
                                }
                            }
                            EvalSelectKind::Send { tx, value } => {
                                let tx_guard = tx.lock().unwrap();
                                match tx_guard.try_send(value.clone()) {
                                    Ok(()) => {
                                        drop(tx_guard);
                                        let result = self.eval_expr(&arms[entry.arm_idx].body)?;
                                        return Ok(result);
                                    }
                                    Err(mpsc::TrySendError::Full(_)) => {
                                        all_closed = false;
                                    }
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        // Channel closed
                                    }
                                }
                            }
                            EvalSelectKind::Default => unreachable!(),
                        }
                    }

                    // All channels closed (CL1)
                    if all_closed && default_idx.is_none() {
                        return Ok(Value::Enum {
                            name: "Result".to_string(),
                            variant: "Err".to_string(),
                            fields: vec![Value::String(Arc::new(Mutex::new(
                                "all channels closed".to_string(),
                            )))],
                        });
                    }

                    // Default arm fires if nothing ready (A3)
                    if let Some(idx) = default_idx {
                        return self.eval_expr(&arms[idx].body);
                    }

                    // Backoff
                    std::thread::sleep(std::time::Duration::from_micros(backoff_us));
                    backoff_us = (backoff_us * 2).min(max_backoff_us);
                }
            }

            _ => Ok(Value::Unit),
        }
    }
}

