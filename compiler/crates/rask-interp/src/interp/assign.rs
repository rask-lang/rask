// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Assignment and destructuring.

use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::TuplePat;

use crate::value::Value;

use super::{Interpreter, RuntimeError};

impl Interpreter {
    /// Destructure a value according to a list of TuplePat patterns.
    /// Handles nested patterns like `(a, (b, c), _)` recursively.
    pub(super) fn destructure_tuple_pats(&mut self, pats: &[TuplePat], value: Value) -> Result<(), RuntimeError> {
        let elements = Self::value_to_elements(value, pats.len())?;
        for (pat, val) in pats.iter().zip(elements) {
            self.bind_tuple_pat(pat, val)?;
        }
        Ok(())
    }

    fn bind_tuple_pat(&mut self, pat: &TuplePat, value: Value) -> Result<(), RuntimeError> {
        match pat {
            TuplePat::Name(name) => {
                self.env.define(name.clone(), value);
            }
            TuplePat::Wildcard => {
                // discard
            }
            TuplePat::Nested(inner_pats) => {
                let elements = Self::value_to_elements(value, inner_pats.len())?;
                for (p, v) in inner_pats.iter().zip(elements) {
                    self.bind_tuple_pat(p, v)?;
                }
            }
        }
        Ok(())
    }

    /// Extract positional elements from a tuple-like value (Vec or Struct).
    fn value_to_elements(value: Value, expected: usize) -> Result<Vec<Value>, RuntimeError> {
        match value {
            Value::Vec(v) => {
                let vec = v.lock().unwrap();
                if vec.len() != expected {
                    return Err(RuntimeError::TypeError(format!(
                        "tuple destructuring: expected {} elements, got {}",
                        expected, vec.len()
                    )));
                }
                Ok(vec.clone())
            }
            Value::Struct(ref s) => {
                let guard = s.lock().unwrap();
                let vals: Vec<_> = guard.fields.values().cloned().collect();
                if vals.len() != expected {
                    return Err(RuntimeError::TypeError(format!(
                        "tuple destructuring: expected {} elements, got {}",
                        expected, vals.len()
                    )));
                }
                Ok(vals)
            }
            _ => {
                Err(RuntimeError::TypeError(format!(
                    "cannot destructure {} into tuple", value.type_name()
                )))
            }
        }
    }

    fn assign_nested_field(obj: &Value, field_chain: &[String], value: Value) -> Result<(), RuntimeError> {
        if field_chain.is_empty() {
            return Err(RuntimeError::TypeError(
                "cannot assign to a value directly through nested field chain".to_string(),
            ));
        }

        if field_chain.len() == 1 {
            let field = &field_chain[0];
            match obj {
                Value::Struct(s) => {
                    s.lock().unwrap().fields.insert(field.clone(), value);
                    return Ok(());
                }
                Value::Vec(v) if field.parse::<usize>().is_ok() => {
                    let idx = field.parse::<usize>().unwrap();
                    let mut vec = v.lock().unwrap();
                    if idx < vec.len() {
                        vec[idx] = value;
                        return Ok(());
                    }
                    return Err(RuntimeError::IndexOutOfBounds { index: idx as i64, len: vec.len() });
                }
                _ => return Err(RuntimeError::TypeError(format!(
                    "cannot assign field '{}' on {}", field, obj.type_name()
                ))),
            }
        }

        // Multi-level: drill down through struct fields
        let first = &field_chain[0];
        match obj {
            Value::Struct(s) => {
                let guard = s.lock().unwrap();
                let inner = guard.fields.get(first).cloned().ok_or_else(|| {
                    RuntimeError::TypeError(format!("no field '{}' on struct", first))
                })?;
                drop(guard);
                Self::assign_nested_field(&inner, &field_chain[1..], value)
            }
            _ => Err(RuntimeError::TypeError(format!(
                "cannot access field '{}' on {}", first, obj.type_name()
            ))),
        }
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
                    if let Some(ref elem) = pool.slots[slot_idx].1 {
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
                        Self::assign_nested_field(&vec[idx], field_chain, value)
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
                        return Self::assign_nested_field(&v, field_chain, value);
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
                        if let Some(obj) = self.env.get(var_name) {
                            let obj = obj.clone();
                            Self::assign_nested_field(&obj, &field_chain, value)
                        } else {
                            Err(RuntimeError::UndefinedVariable(var_name.clone()))
                        }
                    }
                    ExprKind::Index { object: idx_obj, index: idx_expr } => {
                        let idx_val = self.eval_expr(idx_expr).map_err(|diag| diag.error)?;
                        let container = self.eval_index_target(idx_obj)?;
                        Self::assign_index_field(&container, &idx_val, &field_chain, value)
                    }
                    // Inline sync access: shared.write().field = value, mutex.lock().field = value
                    ExprKind::MethodCall { object, method, args, .. }
                        if args.is_empty() && matches!(method.as_str(), "write" | "lock" | "read") =>
                    {
                        let receiver = self.eval_expr(object).map_err(|diag| diag.error)?;
                        let obj = self.call_inline_sync_access(&receiver, method)
                            .map_err(|_| RuntimeError::TypeError(format!(
                                "invalid inline sync access: .{}() on {}",
                                method, receiver.type_name()
                            )))?;
                        Self::assign_nested_field(&obj, &field_chain, value)
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

