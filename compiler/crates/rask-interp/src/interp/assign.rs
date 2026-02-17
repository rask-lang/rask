// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Assignment and destructuring.

use rask_ast::expr::{Expr, ExprKind};

use crate::value::Value;

use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(super) fn destructure_tuple(&mut self, names: &[String], value: Value) -> Result<(), RuntimeError> {
        match value {
            Value::Vec(v) => {
                let vec = v.lock().unwrap();
                if vec.len() != names.len() {
                    return Err(RuntimeError::TypeError(format!(
                        "tuple destructuring: expected {} elements, got {}",
                        names.len(), vec.len()
                    )));
                }
                for (name, val) in names.iter().zip(vec.iter()) {
                    self.env.define(name.clone(), val.clone());
                }
            }
            Value::Struct { fields, .. } => {
                // Try named destructuring first, fall back to positional
                let has_named = names.iter().any(|n| fields.contains_key(n));
                if has_named {
                    for name in names {
                        let val = fields.get(name).cloned().unwrap_or(Value::Unit);
                        self.env.define(name.clone(), val);
                    }
                } else {
                    // Positional: iterate field values in insertion order
                    let vals: Vec<_> = fields.values().cloned().collect();
                    if vals.len() != names.len() {
                        return Err(RuntimeError::TypeError(format!(
                            "tuple destructuring: expected {} elements, got {}",
                            names.len(), vals.len()
                        )));
                    }
                    for (name, val) in names.iter().zip(vals) {
                        self.env.define(name.clone(), val);
                    }
                }
            }
            _ => {
                return Err(RuntimeError::TypeError(format!(
                    "cannot destructure {} into tuple", value.type_name()
                )));
            }
        }
        Ok(())
    }

    fn assign_nested_field(obj: &mut Value, field_chain: &[String], value: Value) -> Result<(), RuntimeError> {
        if field_chain.is_empty() {
            *obj = value;
            return Ok(());
        }
        let mut current = obj;
        for (i, field) in field_chain.iter().enumerate() {
            if i == field_chain.len() - 1 {
                match current {
                    Value::Struct { fields, .. } => {
                        fields.insert(field.clone(), value);
                        return Ok(());
                    }
                    _ => return Err(RuntimeError::TypeError(format!(
                        "cannot assign field '{}' on {}", field, current.type_name()
                    ))),
                }
            } else {
                current = match current {
                    Value::Struct { fields, .. } => {
                        fields.get_mut(field).ok_or_else(|| {
                            RuntimeError::TypeError(format!("no field '{}' on struct", field))
                        })?
                    }
                    _ => return Err(RuntimeError::TypeError(format!(
                        "cannot access field '{}' on {}", field, current.type_name()
                    ))),
                };
            }
        }
        unreachable!()
    }

    /// Evaluate an expression that will be the target of an index assignment.
    /// For bare idents, looks up in env. For nested Index exprs, evaluates to
    /// get the Arc-wrapped collection (Vec/Map/Pool share through Arc<Mutex>).
    fn eval_index_target(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match &expr.kind {
            ExprKind::Ident(var_name) => {
                self.env.get(var_name).cloned()
                    .ok_or_else(|| RuntimeError::UndefinedVariable(var_name.clone()))
            }
            _ => self.eval_expr(expr).map_err(|diag| diag.error),
        }
    }

    /// Assign `value` into `container[idx]`.
    fn assign_index(container: &Value, idx: &Value, value: Value) -> Result<(), RuntimeError> {
        match container {
            Value::Vec(v) => {
                if let Value::Int(i) = idx {
                    let idx = *i as usize;
                    let mut vec = v.lock().unwrap();
                    if idx < vec.len() {
                        vec[idx] = value;
                        Ok(())
                    } else {
                        Err(RuntimeError::IndexOutOfBounds { index: *i, len: vec.len() })
                    }
                } else {
                    Err(RuntimeError::TypeError("Vec index must be an integer".to_string()))
                }
            }
            Value::Pool(p) => {
                if let Value::Handle { pool_id, index, generation } = idx {
                    let mut pool = p.lock().unwrap();
                    let slot_idx = pool.validate(*pool_id, *index, *generation)
                        .map_err(|e| RuntimeError::Panic(e))?;
                    pool.slots[slot_idx].1 = Some(value);
                    Ok(())
                } else {
                    Err(RuntimeError::TypeError("Pool index must be a Handle".to_string()))
                }
            }
            Value::Map(m) => {
                let mut map = m.lock().unwrap();
                for (k, v) in map.iter_mut() {
                    if Self::value_eq(k, idx) {
                        *v = value;
                        return Ok(());
                    }
                }
                map.push((idx.clone(), value));
                Ok(())
            }
            _ => Err(RuntimeError::TypeError(format!(
                "cannot index-assign on {}", container.type_name()
            ))),
        }
    }

    /// Assign `value` into a field chain on `container[idx].field_chain...`.
    fn assign_index_field(
        container: &Value,
        idx: &Value,
        field_chain: &[String],
        value: Value,
    ) -> Result<(), RuntimeError> {
        match container {
            Value::Pool(p) => {
                if let Value::Handle { pool_id, index, generation } = idx {
                    let mut pool = p.lock().unwrap();
                    let slot_idx = pool.validate(*pool_id, *index, *generation)
                        .map_err(|e| RuntimeError::Panic(e))?;
                    if let Some(ref mut elem) = pool.slots[slot_idx].1 {
                        Self::assign_nested_field(elem, field_chain, value)
                    } else {
                        Err(RuntimeError::TypeError("pool slot is empty; the handle may have been removed".to_string()))
                    }
                } else {
                    Err(RuntimeError::TypeError("Pool indexing requires a Handle".to_string()))
                }
            }
            Value::Vec(v) => {
                if let Value::Int(i) = idx {
                    let idx = *i as usize;
                    let mut vec = v.lock().unwrap();
                    if idx < vec.len() {
                        Self::assign_nested_field(&mut vec[idx], field_chain, value)
                    } else {
                        Err(RuntimeError::IndexOutOfBounds { index: *i, len: vec.len() })
                    }
                } else {
                    Err(RuntimeError::TypeError("Vec index must be an integer".to_string()))
                }
            }
            Value::Map(m) => {
                let mut map = m.lock().unwrap();
                for (k, v) in map.iter_mut() {
                    if Self::value_eq(k, idx) {
                        return Self::assign_nested_field(v, field_chain, value);
                    }
                }
                Err(RuntimeError::Panic(format!("key not found in map")))
            }
            _ => Err(RuntimeError::TypeError(format!(
                "cannot index into {}; only Vec, Map, and Pool support indexing", container.type_name()
            ))),
        }
    }

    pub(super) fn assign_target(&mut self, target: &Expr, value: Value) -> Result<(), RuntimeError> {
        match &target.kind {
            ExprKind::Ident(name) => {
                if !self.env.assign(name, value) {
                    return Err(RuntimeError::UndefinedVariable(name.clone()));
                }
                Ok(())
            }
            ExprKind::Field { .. } => {
                let mut field_chain = Vec::new();
                let mut current = target;
                while let ExprKind::Field { object, field: f } = &current.kind {
                    field_chain.push(f.clone());
                    current = object;
                }
                field_chain.reverse();

                match &current.kind {
                    ExprKind::Ident(var_name) => {
                        if let Some(obj) = self.env.get_mut(var_name) {
                            Self::assign_nested_field(obj, &field_chain, value)
                        } else {
                            Err(RuntimeError::UndefinedVariable(var_name.clone()))
                        }
                    }
                    ExprKind::Index { object: idx_obj, index: idx_expr } => {
                        let idx_val = self.eval_expr(idx_expr).map_err(|diag| diag.error)?;
                        let container = self.eval_index_target(idx_obj)?;
                        Self::assign_index_field(&container, &idx_val, &field_chain, value)
                    }
                    _ => Err(RuntimeError::TypeError("invalid assignment target; assign to a variable, field, or index".to_string())),
                }
            }
            ExprKind::Index { object, index } => {
                let idx = self.eval_expr(index).map_err(|diag| diag.error)?;
                let obj = self.eval_index_target(object)?;
                Self::assign_index(&obj, &idx, value)
            }
            _ => Err(RuntimeError::TypeError(
                "invalid assignment target".to_string(),
            )),
        }
    }
}

