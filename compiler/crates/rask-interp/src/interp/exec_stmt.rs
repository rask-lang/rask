// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Statement execution.

use rask_ast::stmt::{ForBinding, Stmt, StmtKind};

use crate::value::Value;

use super::{Interpreter, RuntimeDiagnostic, RuntimeError};

impl Interpreter {
    pub(super) fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Value, RuntimeDiagnostic> {
        match &stmt.kind {
            StmtKind::Expr(expr) => self.eval_expr(expr),

            StmtKind::Const { name, init, .. } => {
                let value = self.eval_expr(init)?;
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            StmtKind::Let { name, name_span: _, ty, init } => {
                let value = self.eval_expr(init)?;
                // Coerce Vec to SimdF32x8 when type annotation says f32x8
                let value = if ty.as_deref() == Some("f32x8") {
                    Self::coerce_to_simd_f32x8(value)
                        .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?
                } else {
                    value
                };
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            StmtKind::LetTuple { names, init } => {
                let value = self.eval_expr(init)?;
                self.destructure_tuple(names, value)
                    .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?;
                Ok(Value::Unit)
            }

            StmtKind::ConstTuple { names, init } => {
                let value = self.eval_expr(init)?;
                self.destructure_tuple(names, value)
                    .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?;
                Ok(Value::Unit)
            }

            StmtKind::Assign { target, value } => {
                let val = self.eval_expr(value)?;
                self.assign_target(target, val)
                    .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?;
                Ok(Value::Unit)
            }

            StmtKind::Return(expr) => {
                let value = if let Some(e) = expr {
                    self.eval_expr(e)?
                } else {
                    Value::Unit
                };
                Err(RuntimeDiagnostic::new(RuntimeError::Return(value), stmt.span))
            }

            StmtKind::While { cond, body } => {
                loop {
                    let cond_val = self.eval_expr(cond)?;
                    if !self.is_truthy(&cond_val) {
                        break;
                    }
                    self.env.push_scope();
                    match self.exec_stmts(body) {
                        Ok(_) => {}
                        Err(diag) if matches!(diag.error, RuntimeError::Break) => {
                            self.env.pop_scope();
                            break;
                        }
                        Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                            self.env.pop_scope();
                            continue;
                        }
                        Err(e) => {
                            self.env.pop_scope();
                            return Err(e);
                        }
                    }
                    self.env.pop_scope();
                }
                Ok(Value::Unit)
            }

            StmtKind::WhileLet {
                pattern,
                expr,
                body,
            } => {
                loop {
                    let value = self.eval_expr(expr)?;

                    if let Some(bindings) = self.match_pattern(pattern, &value) {
                        self.env.push_scope();
                        for (name, val) in bindings {
                            self.env.define(name, val);
                        }
                        match self.exec_stmts(body) {
                            Ok(_) => {}
                            Err(diag) if matches!(diag.error, RuntimeError::Break) => {
                                self.env.pop_scope();
                                break;
                            }
                            Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                                self.env.pop_scope();
                                continue;
                            }
                            Err(e) => {
                                self.env.pop_scope();
                                return Err(e);
                            }
                        }
                        self.env.pop_scope();
                    } else {
                        break;
                    }
                }
                Ok(Value::Unit)
            }

            StmtKind::Loop { body, .. } => loop {
                self.env.push_scope();
                match self.exec_stmts(body) {
                    Ok(_) => {}
                    Err(diag) if matches!(diag.error, RuntimeError::Break) => {
                        self.env.pop_scope();
                        break Ok(Value::Unit);
                    }
                    Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                        self.env.pop_scope();
                        continue;
                    }
                    Err(e) => {
                        self.env.pop_scope();
                        break Err(e);
                    }
                }
                self.env.pop_scope();
            },

            StmtKind::Break { .. } => Err(RuntimeDiagnostic::new(RuntimeError::Break, stmt.span)),

            StmtKind::Continue(_) => Err(RuntimeDiagnostic::new(RuntimeError::Continue, stmt.span)),

            StmtKind::For {
                binding,
                iter,
                body,
                ..
            } => {
                let iter_val = self.eval_expr(iter)?;

                match iter_val {
                    Value::Range {
                        start,
                        end,
                        inclusive,
                    } => {
                        let end_val = if inclusive { end + 1 } else { end };
                        for i in start..end_val {
                            self.env.push_scope();
                            self.define_for_binding(binding, Value::Int(i));
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if matches!(diag.error, RuntimeError::Break) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                                    self.env.pop_scope();
                                    continue;
                                }
                                Err(e) => {
                                    self.env.pop_scope();
                                    return Err(e);
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(Value::Unit)
                    }
                    Value::Vec(v) => {
                        let items: Vec<Value> = v.lock().unwrap().clone();
                        for item in items {
                            self.env.push_scope();
                            self.define_for_binding(binding, item);
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if matches!(diag.error, RuntimeError::Break) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                                    self.env.pop_scope();
                                    continue;
                                }
                                Err(e) => {
                                    self.env.pop_scope();
                                    return Err(e);
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(Value::Unit)
                    }
                    Value::Pool(p) => {
                        // Handle mode (default): yield handles as snapshot
                        let pool = p.lock().unwrap();
                        let pool_id = pool.pool_id;
                        let items: Vec<Value> = pool
                            .slots
                            .iter()
                            .enumerate()
                            .filter_map(|(i, (gen, slot))| {
                                slot.as_ref().map(|_| Value::Handle {
                                    pool_id,
                                    index: i as u32,
                                    generation: *gen,
                                })
                            })
                            .collect();
                        drop(pool);

                        for item in items {
                            self.env.push_scope();
                            self.define_for_binding(binding, item);
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if matches!(diag.error, RuntimeError::Break) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                                    self.env.pop_scope();
                                    continue;
                                }
                                Err(e) => {
                                    self.env.pop_scope();
                                    return Err(e);
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(Value::Unit)
                    }
                    Value::Iterator(iter) => {
                        loop {
                            match self.iter_next(&iter)
                                .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))? {
                                Some(item) => {
                                    self.env.push_scope();
                                    self.define_for_binding(binding, item);
                                    match self.exec_stmts(body) {
                                        Ok(_) => {}
                                        Err(diag) if matches!(diag.error, RuntimeError::Break) => {
                                            self.env.pop_scope();
                                            break;
                                        }
                                        Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                                            self.env.pop_scope();
                                            continue;
                                        }
                                        Err(e) => {
                                            self.env.pop_scope();
                                            return Err(e);
                                        }
                                    }
                                    self.env.pop_scope();
                                }
                                None => break,
                            }
                        }
                        Ok(Value::Unit)
                    }
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "cannot iterate over {}",
                            iter_val.type_name()
                        )),
                        stmt.span
                    )),
                }
            }

            StmtKind::Ensure { .. } => Ok(Value::Unit),

            StmtKind::Comptime(body) => {
                self.env.push_scope();
                let result = self.exec_stmts(body);
                self.env.pop_scope();
                result
            }

            _ => Ok(Value::Unit),
        }
    }

    fn define_for_binding(&mut self, binding: &ForBinding, value: Value) {
        match binding {
            ForBinding::Single(name) => self.env.define(name.clone(), value),
            ForBinding::Tuple(names) => {
                // Destructure tuple/array value into bindings
                if let Value::Vec(v) = &value {
                    let items = v.lock().unwrap();
                    for (i, name) in names.iter().enumerate() {
                        let val = items.get(i).cloned().unwrap_or(Value::Unit);
                        self.env.define(name.clone(), val);
                    }
                } else {
                    // Single value bound to first name, rest get Unit
                    for (i, name) in names.iter().enumerate() {
                        if i == 0 {
                            self.env.define(name.clone(), value.clone());
                        } else {
                            self.env.define(name.clone(), Value::Unit);
                        }
                    }
                }
            }
        }
    }
}

