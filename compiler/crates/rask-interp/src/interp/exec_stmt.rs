// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Statement execution.

use rask_ast::stmt::{ForBinding, Stmt, StmtKind};

use crate::value::Value;

use super::{Interpreter, RuntimeDiagnostic, RuntimeError};

impl Interpreter {
    pub(super) fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Value, RuntimeDiagnostic> {
        match &stmt.kind {
            StmtKind::Expr(expr) => self.eval_expr(expr),

            StmtKind::Const { name, init, ty, .. } => {
                let mut value = self.eval_expr(init)?;
                if let Some(ty_str) = ty {
                    value = auto_wrap_for_annotation(value, ty_str);
                }
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            StmtKind::Mut { name, name_span: _, ty, init } => {
                let value = self.eval_expr(init)?;
                // Coerce Vec to SimdF32x8 when type annotation says f32x8
                let value = if ty.as_deref() == Some("f32x8") {
                    Self::coerce_to_simd_f32x8(value)
                        .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?
                } else {
                    value
                };
                // OPT6: auto-wrap bare T into T? / T or E when annotated.
                let value = if let Some(ty_str) = ty {
                    auto_wrap_for_annotation(value, ty_str)
                } else {
                    value
                };
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            StmtKind::MutTuple { patterns, init } => {
                let value = self.eval_expr(init)?;
                self.destructure_tuple_pats(patterns, value)
                    .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?;
                Ok(Value::Unit)
            }

            StmtKind::ConstTuple { patterns, init } => {
                let value = self.eval_expr(init)?;
                self.destructure_tuple_pats(patterns, value)
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
                        Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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
                            Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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
                    Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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

            StmtKind::Break { label, value } => {
                let val = match value {
                    Some(expr) => self.eval_expr(expr)?,
                    None => {
                        // Ambiguity: `break ident` parsed as label — if the label
                        // is actually a variable name, evaluate it as a value.
                        if let Some(name) = label {
                            if let Some(v) = self.env.get(name) {
                                v.clone()
                            } else {
                                Value::Unit
                            }
                        } else {
                            Value::Unit
                        }
                    }
                };
                Err(RuntimeDiagnostic::new(RuntimeError::Break(val), stmt.span))
            }

            StmtKind::Continue(_) => Err(RuntimeDiagnostic::new(RuntimeError::Continue, stmt.span)),

            StmtKind::For {
                binding,
                mutate,
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
                                Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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
                    Value::Vec(ref v) if *mutate => {
                        let len = v.lock().unwrap().len();
                        for i in 0..len {
                            let item = v.lock().unwrap()[i].clone();
                            self.env.push_scope();
                            self.define_for_binding(binding, item);
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
                                    // Write back before breaking
                                    if let ForBinding::Single(name) = binding {
                                        if let Some(val) = self.env.get(name) {
                                            let val = val.clone();
                                            v.lock().unwrap()[i] = val;
                                        }
                                    }
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                                    // Write back before continuing
                                    if let ForBinding::Single(name) = binding {
                                        if let Some(val) = self.env.get(name) {
                                            let val = val.clone();
                                            v.lock().unwrap()[i] = val;
                                        }
                                    }
                                    self.env.pop_scope();
                                    continue;
                                }
                                Err(e) => {
                                    self.env.pop_scope();
                                    return Err(e);
                                }
                            }
                            // Write back the (possibly mutated) value
                            if let ForBinding::Single(name) = binding {
                                if let Some(val) = self.env.get(name) {
                                    let val = val.clone();
                                    v.lock().unwrap()[i] = val;
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(Value::Unit)
                    }
                    // LP13: for mutate on Map — write back values by key
                    Value::Map(ref m) if *mutate => {
                        let map_arc = std::sync::Arc::clone(m);
                        let pairs: Vec<(Value, Value)> = map_arc.lock().unwrap().clone();
                        for (key, val) in pairs {
                            self.env.push_scope();
                            // Bind as tuple (k, v) or single pair
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    self.env.define(names[0].clone(), key.clone());
                                    self.env.define(names[1].clone(), val);
                                }
                            } else {
                                let pair = Value::Vec(std::sync::Arc::new(std::sync::Mutex::new(vec![key.clone(), val])));
                                self.define_for_binding(binding, pair);
                            }
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
                                    // Write back before breaking
                                    if let ForBinding::Tuple(names) = binding {
                                        if names.len() >= 2 {
                                            if let Some(v) = self.env.get(&names[1]).cloned() {
                                                let mut guard = map_arc.lock().unwrap();
                                                if let Some(pair) = guard.iter_mut().find(|(k, _)| Self::value_eq(k, &key)) {
                                                    pair.1 = v;
                                                }
                                            }
                                        }
                                    }
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                                    if let ForBinding::Tuple(names) = binding {
                                        if names.len() >= 2 {
                                            if let Some(v) = self.env.get(&names[1]).cloned() {
                                                let mut guard = map_arc.lock().unwrap();
                                                if let Some(pair) = guard.iter_mut().find(|(k, _)| Self::value_eq(k, &key)) {
                                                    pair.1 = v;
                                                }
                                            }
                                        }
                                    }
                                    self.env.pop_scope();
                                    continue;
                                }
                                Err(e) => {
                                    self.env.pop_scope();
                                    return Err(e);
                                }
                            }
                            // Write back value
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    if let Some(v) = self.env.get(&names[1]).cloned() {
                                        let mut guard = m.lock().unwrap();
                                        if let Some(pair) = guard.iter_mut().find(|(k, _)| Self::value_eq(k, &key)) {
                                            pair.1 = v;
                                        }
                                    }
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(Value::Unit)
                    }
                    // LP13: for mutate on Pool entries — write back values by handle
                    Value::Pool(ref p) if *mutate => {
                        let pool_arc = std::sync::Arc::clone(p);
                        let pool = pool_arc.lock().unwrap();
                        let pool_id = pool.pool_id;
                        let entries: Vec<(Value, Value)> = pool
                            .slots
                            .iter()
                            .enumerate()
                            .filter_map(|(i, (gen, slot))| {
                                slot.as_ref().map(|val| (
                                    Value::Handle { pool_id, index: i as u32, generation: *gen },
                                    val.clone(),
                                ))
                            })
                            .collect();
                        drop(pool);

                        for (handle, val) in entries {
                            self.env.push_scope();
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    self.env.define(names[0].clone(), handle.clone());
                                    self.env.define(names[1].clone(), val);
                                }
                            } else {
                                self.define_for_binding(binding, handle.clone());
                            }
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
                                    if let ForBinding::Tuple(names) = binding {
                                        if names.len() >= 2 {
                                            if let (Some(v), Value::Handle { index, .. }) = (self.env.get(&names[1]).cloned(), &handle) {
                                                let mut pool = pool_arc.lock().unwrap();
                                                if let Some((_, slot)) = pool.slots.get_mut(*index as usize) {
                                                    *slot = Some(v);
                                                }
                                            }
                                        }
                                    }
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if matches!(diag.error, RuntimeError::Continue) => {
                                    if let ForBinding::Tuple(names) = binding {
                                        if names.len() >= 2 {
                                            if let (Some(v), Value::Handle { index, .. }) = (self.env.get(&names[1]).cloned(), &handle) {
                                                let mut pool = pool_arc.lock().unwrap();
                                                if let Some((_, slot)) = pool.slots.get_mut(*index as usize) {
                                                    *slot = Some(v);
                                                }
                                            }
                                        }
                                    }
                                    self.env.pop_scope();
                                    continue;
                                }
                                Err(e) => {
                                    self.env.pop_scope();
                                    return Err(e);
                                }
                            }
                            // Write back
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    if let (Some(v), Value::Handle { index, .. }) = (self.env.get(&names[1]).cloned(), &handle) {
                                        let mut pool = p.lock().unwrap();
                                        if let Some((_, slot)) = pool.slots.get_mut(*index as usize) {
                                            *slot = Some(v);
                                        }
                                    }
                                }
                            }
                            self.env.pop_scope();
                        }
                        Ok(Value::Unit)
                    }
                    // Map iteration (non-mutating): yield (key, value) tuples
                    Value::Map(m) => {
                        let pairs: Vec<(Value, Value)> = m.lock().unwrap().clone();
                        for (key, val) in pairs {
                            self.env.push_scope();
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    self.env.define(names[0].clone(), key);
                                    self.env.define(names[1].clone(), val);
                                } else if names.len() == 1 {
                                    let pair = Value::Vec(std::sync::Arc::new(std::sync::Mutex::new(vec![key, val])));
                                    self.define_for_binding(binding, pair);
                                }
                            } else {
                                let pair = Value::Vec(std::sync::Arc::new(std::sync::Mutex::new(vec![key, val])));
                                self.define_for_binding(binding, pair);
                            }
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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
                                Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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
                                Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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
                                        Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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

            StmtKind::Discard { name, .. } => {
                // Remove the binding from the environment (D1: invalidates binding)
                self.env.remove(name);
                Ok(Value::Unit)
            }

            StmtKind::Comptime(body) => {
                self.env.push_scope();
                let result = self.exec_stmts(body);
                self.env.pop_scope();
                result
            }

            // CT48: comptime for — in the interpreter, runs like a regular for loop
            StmtKind::ComptimeFor { binding, iter, body, .. } => {
                let iter_val = self.eval_expr(iter)?;
                match iter_val {
                    Value::Vec(v) => {
                        let items: Vec<Value> = v.lock().unwrap().clone();
                        for item in items {
                            self.env.push_scope();
                            self.define_for_binding(binding, item);
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if matches!(diag.error, RuntimeError::Break(_)) => {
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
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "comptime for requires a Vec iterable, got {}",
                            iter_val.type_name()
                        )),
                        stmt.span,
                    )),
                }
            }

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

/// OPT6: wrap `value` to match the declared `T?` or `Result<T, E>` annotation.
/// No-op for non-Option/non-Result annotations, or when the value is already
/// shaped like the annotation. For Result, picks Ok vs Err by the value's type.
fn auto_wrap_for_annotation(value: Value, ty: &str) -> Value {
    let ty = ty.trim();
    if ty.ends_with('?') && !ty.starts_with('(') {
        // T? annotation
        if matches!(&value, Value::Enum { name, .. } if name == "Option") {
            return value;
        }
        return Value::Enum {
            name: "Option".to_string(),
            variant: "Some".to_string(),
            fields: vec![value],
            variant_index: 0,
            origin: None,
        };
    }
    if ty.starts_with("Result<") && ty.ends_with('>') {
        if matches!(&value, Value::Enum { name, .. } if name == "Result") {
            return value;
        }
        let err_names = extract_err_names(ty);
        let is_err = match &value {
            Value::Enum { name, .. } => err_names.iter().any(|n| n == name),
            Value::Struct(s) => {
                let guard = s.lock().unwrap();
                err_names.iter().any(|n| n == &guard.name)
            }
            _ => false,
        };
        return Value::Enum {
            name: "Result".to_string(),
            variant: if is_err { "Err".to_string() } else { "Ok".to_string() },
            fields: vec![value],
            variant_index: if is_err { 1 } else { 0 },
            origin: None,
        };
    }
    value
}

/// Parse `Result<T, E>` and return the error type component names.
fn extract_err_names(ty: &str) -> Vec<String> {
    let Some(rest) = ty.strip_prefix("Result<").and_then(|s| s.strip_suffix('>')) else {
        return Vec::new();
    };
    let mut depth: i32 = 0;
    let mut split_at: Option<usize> = None;
    for (i, c) in rest.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            ',' if depth == 0 => { split_at = Some(i); break; }
            _ => {}
        }
    }
    let Some(idx) = split_at else { return Vec::new() };
    let err_str = rest[idx + 1..].trim();
    let err_str = err_str
        .strip_prefix('(').and_then(|s| s.strip_suffix(')'))
        .map(str::trim)
        .unwrap_or(err_str);
    let mut out = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in err_str.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth -= 1,
            '|' if depth == 0 => {
                out.push(err_str[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    if start < err_str.len() {
        out.push(err_str[start..].trim().to_string());
    }
    out
}

