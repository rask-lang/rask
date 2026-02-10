// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Function calling and ensure blocks.

use rask_ast::decl::FnDecl;
use rask_ast::stmt::{Stmt, StmtKind};

use crate::value::Value;

use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(crate) fn call_function(&mut self, func: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeError> {
        if args.len() != func.params.len() {
            return Err(RuntimeError::ArityMismatch {
                expected: func.params.len(),
                got: args.len(),
            });
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
                            return Err(RuntimeError::TypeError(format!(
                                "struct has no field '{}' for projection", proj_fields[0]
                            )));
                        }
                    } else {
                        return Err(RuntimeError::TypeError(format!(
                            "projection parameter expects struct, got {}", arg.type_name()
                        )));
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
            Err(RuntimeError::Return(v)) | Ok(v) => {
                self.transfer_resource_to_scope(v, caller_depth);
            }
            Err(RuntimeError::TryError(v)) => {
                self.transfer_resource_to_scope(v, caller_depth);
            }
            _ => {}
        }

        if let Err(msg) = self.resource_tracker.check_scope_exit(scope_depth) {
            self.env.pop_scope();
            return Err(RuntimeError::Panic(msg));
        }

        self.env.pop_scope();

        let value = match result {
            Ok(_) => Value::Unit,
            Err(RuntimeError::Return(v)) => v,
            Err(RuntimeError::TryError(v)) => v,
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
    pub(super) fn exec_stmts(&mut self, stmts: &[Stmt]) -> Result<Value, RuntimeError> {
        let mut last_value = Value::Unit;
        let mut ensures: Vec<&Stmt> = Vec::new();
        let mut exit_error: Option<RuntimeError> = None;

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

    /// Returns fatal error (Panic/Exit) if one occurs; non-fatal errors passed to catch handlers.
    pub(super) fn run_ensures(&mut self, ensures: &[&Stmt]) -> Option<RuntimeError> {
        for ensure_stmt in ensures.iter().rev() {
            if let StmtKind::Ensure { body, catch } = &ensure_stmt.kind {
                let result = self.exec_ensure_body(body);

                match result {
                    Ok(value) => {
                        if let Value::Enum { name, variant, fields } = &value {
                            if name == "Result" && variant == "Err" {
                                let err_val = fields.first().cloned().unwrap_or(Value::Unit);
                                self.handle_ensure_error(err_val, catch);
                            }
                        }
                    }
                    Err(RuntimeError::Panic(msg)) => return Some(RuntimeError::Panic(msg)),
                    Err(RuntimeError::Exit(code)) => return Some(RuntimeError::Exit(code)),
                    Err(RuntimeError::TryError(val)) => {
                        self.handle_ensure_error(val, catch);
                    }
                    Err(_) => {}
                }
            }
        }
        None
    }

    fn exec_ensure_body(&mut self, body: &[Stmt]) -> Result<Value, RuntimeError> {
        let mut last_value = Value::Unit;
        for stmt in body {
            last_value = self.exec_stmt(stmt)?;
        }
        Ok(last_value)
    }

    fn handle_ensure_error(&mut self, error_value: Value, catch: &Option<(String, Vec<Stmt>)>) {
        if let Some((name, handler)) = catch {
            self.env.push_scope();
            self.env.define(name.clone(), error_value);
            let _ = self.exec_ensure_body(handler);
            self.env.pop_scope();
        }
    }
}

