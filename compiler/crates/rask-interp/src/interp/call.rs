// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Function calling and ensure blocks.

use rask_ast::decl::FnDecl;
use rask_ast::expr::ExprKind;
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;

use crate::value::Value;

use super::{Interpreter, RuntimeDiagnostic, RuntimeError};

impl Interpreter {
    pub(crate) fn call_function(&mut self, func: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeDiagnostic> {
        if args.len() != func.params.len() {
            return Err(RuntimeDiagnostic::new(
                RuntimeError::ArityMismatch {
                    expected: func.params.len(),
                    got: args.len(),
                },
                Span::new(0, 0) // Will be re-wrapped by caller with proper span
            ));
        }

        self.env.push_scope();

        for (param, arg) in func.params.iter().zip(args.into_iter()) {
            if let Some(proj_start) = param.ty.find(".{") {
                let proj_fields_str = &param.ty[proj_start + 2..param.ty.len() - 1];
                let proj_fields: Vec<&str> = proj_fields_str.split(',').map(|s| s.trim()).collect();
                if proj_fields.len() == 1 && param.name == proj_fields[0] {
                    if let Value::Struct { fields, .. } = &arg {
                        if let Some(field_val) = fields.get(proj_fields[0]) {
                            self.env.define(param.name.clone(), field_val.clone());
                        } else {
                            return Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "struct has no field '{}' for projection", proj_fields[0]
                                )),
                                Span::new(0, 0)
                            ));
                        }
                    } else {
                        return Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "projection parameter expects struct, got {}", arg.type_name()
                            )),
                            Span::new(0, 0)
                        ));
                    }
                } else {
                    self.env.define(param.name.clone(), arg);
                }
            } else {
                self.env.define(param.name.clone(), arg);
            }
        }

        let result = self.exec_stmts(&func.body);

        let scope_depth = self.env.scope_depth();
        let caller_depth = scope_depth.saturating_sub(1);
        match &result {
            Err(diag) if matches!(&diag.error, RuntimeError::Return(_)) => {
                if let RuntimeError::Return(v) = &diag.error {
                    self.transfer_resource_to_scope(v, caller_depth);
                }
            }
            Ok(v) => {
                self.transfer_resource_to_scope(v, caller_depth);
            }
            Err(diag) if matches!(&diag.error, RuntimeError::TryError(_)) => {
                if let RuntimeError::TryError(v) = &diag.error {
                    self.transfer_resource_to_scope(v, caller_depth);
                }
            }
            _ => {}
        }

        if let Err(msg) = self.resource_tracker.check_scope_exit(scope_depth) {
            self.env.pop_scope();
            return Err(RuntimeDiagnostic::new(RuntimeError::Panic(msg), Span::new(0, 0)));
        }

        self.env.pop_scope();

        let value = match result {
            Ok(_) => Value::Unit,
            Err(diag) if matches!(&diag.error, RuntimeError::Return(_)) => {
                if let RuntimeError::Return(v) = diag.error {
                    v
                } else {
                    unreachable!()
                }
            }
            Err(diag) if matches!(&diag.error, RuntimeError::TryError(_)) => {
                if let RuntimeError::TryError(v) = diag.error {
                    v
                } else {
                    unreachable!()
                }
            }
            Err(e) => return Err(e),
        };

        let returns_result = func.ret_ty.as_ref()
            .map(|t| t.starts_with("Result<"))
            .unwrap_or(false);
        if returns_result {
            match &value {
                Value::Enum { name, .. } if name == "Result" => Ok(value),
                _ => Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Ok".to_string(),
                    fields: vec![value],
                }),
            }
        } else {
            Ok(value)
        }
    }

    /// Runs ensure blocks in LIFO order on block exit.
    pub(super) fn exec_stmts(&mut self, stmts: &[Stmt]) -> Result<Value, RuntimeDiagnostic> {
        let mut last_value = Value::Unit;
        let mut ensures: Vec<&Stmt> = Vec::new();
        let mut exit_error: Option<RuntimeDiagnostic> = None;

        for stmt in stmts {
            if matches!(&stmt.kind, StmtKind::Ensure { .. }) {
                ensures.push(stmt);
            } else {
                match self.exec_stmt(stmt) {
                    Ok(v) => last_value = v,
                    Err(e) => {
                        exit_error = Some(e);
                        break;
                    }
                }
            }
        }

        let ensure_fatal = self.run_ensures(&ensures);

        if let Some(e) = exit_error {
            Err(e)
        } else if let Some(fatal) = ensure_fatal {
            Err(fatal)
        } else {
            Ok(last_value)
        }
    }

    /// Returns fatal error (Panic/Exit) if one occurs; non-fatal errors passed to else handlers.
    /// Skips ensure clauses whose receiver resource was already consumed.
    pub(super) fn run_ensures(&mut self, ensures: &[&Stmt]) -> Option<RuntimeDiagnostic> {
        for ensure_stmt in ensures.iter().rev() {
            if let StmtKind::Ensure { body, else_handler } = &ensure_stmt.kind {
                // Check if the ensure body's receiver is a consumed resource.
                // If so, skip — explicit consumption cancels ensure.
                if self.ensure_receiver_consumed(body) {
                    continue;
                }

                let result = self.exec_ensure_body(body);

                match result {
                    Ok(value) => {
                        if let Value::Enum { name, variant, fields } = &value {
                            if name == "Result" && variant == "Err" {
                                let err_val = fields.first().cloned().unwrap_or(Value::Unit);
                                self.handle_ensure_error(err_val, else_handler);
                            }
                        }
                    }
                    Err(diag) if matches!(&diag.error, RuntimeError::Panic(_)) => {
                        if let RuntimeError::Panic(msg) = diag.error {
                            return Some(RuntimeDiagnostic::new(RuntimeError::Panic(msg), diag.span));
                        }
                        unreachable!()
                    }
                    Err(diag) if matches!(&diag.error, RuntimeError::Exit(_)) => {
                        if let RuntimeError::Exit(code) = diag.error {
                            return Some(RuntimeDiagnostic::new(RuntimeError::Exit(code), diag.span));
                        }
                        unreachable!()
                    }
                    Err(diag) if matches!(&diag.error, RuntimeError::TryError(_)) => {
                        if let RuntimeError::TryError(val) = diag.error {
                            self.handle_ensure_error(val, else_handler);
                        }
                    }
                    Err(_) => {}
                }
            }
        }
        None
    }

    /// Check if the ensure body's receiver variable refers to a consumed resource.
    /// Handles `ensure var.method()` patterns.
    fn ensure_receiver_consumed(&self, body: &[Stmt]) -> bool {
        if let Some(first) = body.first() {
            if let StmtKind::Expr(expr) = &first.kind {
                let receiver_name = match &expr.kind {
                    ExprKind::MethodCall { object, .. } => {
                        if let ExprKind::Ident(name) = &object.kind {
                            Some(name.as_str())
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                if let Some(name) = receiver_name {
                    if let Some(value) = self.env.get(name) {
                        if let Some(id) = self.get_resource_id(value) {
                            return self.resource_tracker.is_consumed(id);
                        }
                    }
                }
            }
        }
        false
    }

    fn exec_ensure_body(&mut self, body: &[Stmt]) -> Result<Value, RuntimeDiagnostic> {
        let mut last_value = Value::Unit;
        for stmt in body {
            last_value = self.exec_stmt(stmt)?;
        }
        Ok(last_value)
    }

    fn handle_ensure_error(&mut self, error_value: Value, else_handler: &Option<(String, Vec<Stmt>)>) {
        if let Some((name, handler)) = else_handler {
            self.env.push_scope();
            self.env.define(name.clone(), error_value);
            let _ = self.exec_ensure_body(handler);
            self.env.pop_scope();
        }
    }
}

