// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Statement execution.

use rask_ast::stmt::{ForBinding, Stmt, StmtKind};

use crate::value::{map_entries_seeded, FloatKind, MapKey, Value};

use super::{Interpreter, RuntimeDiagnostic, RuntimeError};

/// Does this break stop at a loop labeled `label` (CF23)? An unlabeled break
/// stops at the nearest loop; a labeled one only at the loop it names, so
/// `break outer` from an inner loop keeps unwinding.
fn breaks_here(err: &RuntimeError, label: Option<&str>) -> bool {
    matches!(err, RuntimeError::Break(_, target) if target.is_none() || target.as_deref() == label)
}

/// Same question for `continue` (CF24).
fn continues_here(err: &RuntimeError, label: Option<&str>) -> bool {
    matches!(err, RuntimeError::Continue(target) if target.is_none() || target.as_deref() == label)
}

impl Interpreter {
    pub(super) fn exec_stmt(&mut self, stmt: &Stmt) -> Result<Value, RuntimeDiagnostic> {
        match &stmt.kind {
            StmtKind::Expr(expr) => self.eval_expr(expr),

            StmtKind::Let { name, init, ty, .. } => {
                let mut value = self.eval_owned(init)?;
                if let Some(ty_str) = ty {
                    value = auto_wrap_for_annotation(value, ty_str, is_none_literal(init));
                    backfill_pool_type_param(&value, ty_str);
                }
                if let Some(id) = self.get_resource_id(&value) {
                    self.resource_tracker.set_var_name(id, name.clone());
                }
                self.env.define(name.clone(), value);
                Ok(Value::Unit)
            }

            StmtKind::Mut { name, name_span: _, ty, init } => {
                let value = self.eval_owned(init)?;
                // Coerce Vec to SimdF32x8 when type annotation says f32x8
                let value = if ty.as_deref() == Some("f32x8") {
                    Self::coerce_to_simd_f32x8(value)
                        .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?
                } else {
                    value
                };
                // OPT6: auto-wrap bare T into T? / T or E when annotated.
                let value = if let Some(ty_str) = ty {
                    let value = auto_wrap_for_annotation(value, ty_str, is_none_literal(init));
                    backfill_pool_type_param(&value, ty_str);
                    value
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
                let value = self.eval_owned(init)?;
                self.destructure_tuple_pats(patterns, value)
                    .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?;
                Ok(Value::Unit)
            }

            StmtKind::LetTuple { patterns, init } => {
                let value = self.eval_owned(init)?;
                self.destructure_tuple_pats(patterns, value)
                    .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))?;
                Ok(Value::Unit)
            }

            // `let Point { x, .. } = p` — the same match a `match` arm does, then
            // the bindings it produced go into the current scope.
            StmtKind::LetStruct { pattern, init, .. } => {
                let value = self.eval_owned(init)?;
                let Some(bindings) = self.match_pattern(pattern, &value) else {
                    return Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(
                            "destructuring binding didn't match the value".to_string(),
                        ),
                        stmt.span,
                    ));
                };
                for (name, val) in bindings {
                    self.env.define(name, val);
                }
                Ok(Value::Unit)
            }

            StmtKind::Assign { target, value, .. } => {
                let val = self.eval_owned(value)?;
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

            StmtKind::While { label, cond, body } => {
                let loop_label = label.as_deref();
                // `while x is Pat(v) && …` binds v for the rest of the condition
                // and for the body, so the condition is evaluated inside the same
                // scope the body runs in (#256).
                let binds = super::eval_expr::cond_binds_pattern(cond);
                loop {
                    if binds {
                        self.env.push_scope();
                        let taken = match self.eval_cond_bindings(cond) {
                            Ok(t) => t,
                            Err(e) => {
                                self.env.pop_scope();
                                return Err(e);
                            }
                        };
                        if !taken {
                            self.env.pop_scope();
                            break;
                        }
                    } else {
                        let cond_val = self.eval_expr(cond)?;
                        if !self.is_truthy(&cond_val) {
                            break;
                        }
                        self.env.push_scope();
                    }
                    match self.exec_stmts(body) {
                        Ok(_) => {}
                        Err(diag) if breaks_here(&diag.error, loop_label) => {
                            self.env.pop_scope();
                            break;
                        }
                        Err(diag) if continues_here(&diag.error, loop_label) => {
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
                label,
                pattern,
                expr,
                body,
            } => {
                let loop_label = label.as_deref();
                loop {
                    let value = self.eval_expr(expr)?;

                    if let Some(bindings) = self.match_pattern(pattern, &value) {
                        self.env.push_scope();
                        for (name, val) in bindings {
                            self.env.define(name, val);
                        }
                        match self.exec_stmts(body) {
                            Ok(_) => {}
                            Err(diag) if breaks_here(&diag.error, loop_label) => {
                                self.env.pop_scope();
                                break;
                            }
                            Err(diag) if continues_here(&diag.error, loop_label) => {
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

            StmtKind::Loop { body, label } => {
                let loop_label = label.as_deref();
                loop {
                    self.env.push_scope();
                    match self.exec_stmts(body) {
                        Ok(_) => {}
                        Err(diag) if breaks_here(&diag.error, loop_label) => {
                            self.env.pop_scope();
                            break Ok(Value::Unit);
                        }
                        Err(diag) if continues_here(&diag.error, loop_label) => {
                            self.env.pop_scope();
                            continue;
                        }
                        Err(e) => {
                            self.env.pop_scope();
                            break Err(e);
                        }
                    }
                    self.env.pop_scope();
                }
            }

            StmtKind::Break { label, value } => {
                let val = match value {
                    Some(expr) => self.eval_expr(expr)?,
                    None => Value::Unit,
                };
                Err(RuntimeDiagnostic::new(
                    RuntimeError::Break(val, label.clone()),
                    stmt.span,
                ))
            }

            StmtKind::Continue(label) => Err(RuntimeDiagnostic::new(
                RuntimeError::Continue(label.clone()),
                stmt.span,
            )),

            StmtKind::For {
                label,
                binding,
                mutate,
                iter,
                body,
            } => {
                let loop_label = label.as_deref();
                let iter_val = self.eval_expr(iter)?;

                match iter_val {
                    Value::Range {
                        start,
                        end,
                        inclusive,
                        step,
                        rev,
                    } => {
                        let n = crate::value::range_count(start, end, inclusive, step);
                        for k in 0..n {
                            let idx = if rev { n - 1 - k } else { k };
                            let i = start.wrapping_add(idx.wrapping_mul(step));
                            self.env.push_scope();
                            self.define_for_binding(binding, Value::int(i));
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if breaks_here(&diag.error, loop_label) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if continues_here(&diag.error, loop_label) => {
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
                            let outcome = self.exec_stmts(body);
                            // Write back however the body ended. It used to be
                            // written three times — once each for break, continue
                            // and falling off the end — and the fourth way out was
                            // missing, so `return item` inside the body handed back
                            // the new value and left the collection unchanged (#650).
                            if let ForBinding::Single(name) = binding {
                                if let Some(val) = self.env.get(name) {
                                    let val = val.clone();
                                    v.lock().unwrap()[i] = val;
                                }
                            }
                            self.env.pop_scope();
                            match outcome {
                                Ok(_) => {}
                                Err(diag) if breaks_here(&diag.error, loop_label) => break,
                                Err(diag) if continues_here(&diag.error, loop_label) => continue,
                                Err(e) => return Err(e),
                            }
                        }
                        Ok(Value::Unit)
                    }
                    // LP13: for mutate on Map — write back values by key
                    Value::Map(ref m) if *mutate => {
                        let map_arc = std::sync::Arc::clone(m);
                        let pairs = map_entries_seeded(&map_arc.lock().unwrap());
                        for (key, val) in pairs {
                            self.env.push_scope();
                            // Bind as tuple (k, v) or single pair
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    self.env.define(names[0].clone(), key.clone());
                                    self.env.define(names[1].clone(), val);
                                }
                            } else {
                                let pair = Value::tuple(vec![key.clone(), val]);
                                self.define_for_binding(binding, pair);
                            }
                            let outcome = self.exec_stmts(body);
                            // However the body ended — see the Vec arm above (#650).
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    if let Some(v) = self.env.get(&names[1]) {
                                        let mut guard = map_arc.lock().unwrap();
                                        if let Some(slot) = guard.get_mut(&MapKey(key.clone())) {
                                            *slot = v;
                                        }
                                    }
                                }
                            }
                            self.env.pop_scope();
                            match outcome {
                                Ok(_) => {}
                                Err(diag) if breaks_here(&diag.error, loop_label) => break,
                                Err(diag) if continues_here(&diag.error, loop_label) => continue,
                                Err(e) => return Err(e),
                            }
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
                            let outcome = self.exec_stmts(body);
                            // However the body ended — see the Vec arm above (#650).
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    if let (Some(v), Value::Handle { index, .. }) = (self.env.get(&names[1]), &handle) {
                                        let mut pool = pool_arc.lock().unwrap();
                                        if let Some((_, slot)) = pool.slots.get_mut(*index as usize) {
                                            *slot = Some(v);
                                        }
                                    }
                                }
                            }
                            self.env.pop_scope();
                            match outcome {
                                Ok(_) => {}
                                Err(diag) if breaks_here(&diag.error, loop_label) => break,
                                Err(diag) if continues_here(&diag.error, loop_label) => continue,
                                Err(e) => return Err(e),
                            }
                        }
                        Ok(Value::Unit)
                    }
                    // Map iteration (non-mutating): yield (key, value) tuples
                    Value::Map(m) => {
                        let pairs = map_entries_seeded(&m.lock().unwrap());
                        for (key, val) in pairs {
                            self.env.push_scope();
                            if let ForBinding::Tuple(names) = binding {
                                if names.len() >= 2 {
                                    self.env.define(names[0].clone(), key);
                                    self.env.define(names[1].clone(), val);
                                } else if names.len() == 1 {
                                    let pair = Value::tuple(vec![key, val]);
                                    self.define_for_binding(binding, pair);
                                }
                            } else {
                                let pair = Value::tuple(vec![key, val]);
                                self.define_for_binding(binding, pair);
                            }
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if breaks_here(&diag.error, loop_label) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if continues_here(&diag.error, loop_label) => {
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
                        let items: Vec<Value> = v.lock().unwrap().items.clone();
                        for item in items {
                            self.env.push_scope();
                            self.define_for_binding(binding, item);
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if breaks_here(&diag.error, loop_label) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if continues_here(&diag.error, loop_label) => {
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
                                Err(diag) if breaks_here(&diag.error, loop_label) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if continues_here(&diag.error, loop_label) => {
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
                                        Err(diag) if breaks_here(&diag.error, loop_label) => {
                                            self.env.pop_scope();
                                            break;
                                        }
                                        Err(diag) if continues_here(&diag.error, loop_label) => {
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
                    // type.sequence/SEQ6: a Sequence is a function taking a
                    // yield. Iterating one is calling it — the loop hands over a
                    // yield that runs the body and answers whether to continue,
                    // so the sequence keeps its own traversal state in its own
                    // frame and nothing has to be stored between items (SEQ38).
                    seq @ (Value::Closure { .. } | Value::Function { .. }) => {
                        self.yield_stack.push(super::YieldFrame {
                            binding: binding.clone(),
                            body: body.clone(),
                            label: label.clone(),
                            escaped: None,
                            scope: self.env.capture_shared(),
                        });
                        let driven = self.call_value(seq, vec![Value::Builtin(
                            crate::value::BuiltinKind::SequenceYield,
                        )]);
                        let frame = self.yield_stack.pop();
                        // The body's own escape wins over anything the sequence
                        // says on the way out: a `return` in the loop body is
                        // what the program asked for, and a source unwinding
                        // afterwards is a consequence of it (SEQ8).
                        if let Some(diag) = frame.and_then(|f| f.escaped) {
                            return Err(diag);
                        }
                        driven
                            .map(|_| Value::Unit)
                            .map_err(|e| RuntimeDiagnostic::new(e, stmt.span))
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
                let loop_label: Option<&str> = None;
                let iter_val = self.eval_expr(iter)?;
                match iter_val {
                    Value::Vec(v) => {
                        let items: Vec<Value> = v.lock().unwrap().items.clone();
                        for item in items {
                            self.env.push_scope();
                            self.define_for_binding(binding, item);
                            match self.exec_stmts(body) {
                                Ok(_) => {}
                                Err(diag) if breaks_here(&diag.error, loop_label) => {
                                    self.env.pop_scope();
                                    break;
                                }
                                Err(diag) if continues_here(&diag.error, loop_label) => {
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

    /// One yield from a `Sequence<T>` being driven by a `for` loop: run the
    /// loop body with the item bound, and answer whether to keep going
    /// (type.sequence/SEQ3).
    ///
    /// The frame comes off the stack for the length of the body so a nested
    /// `for` over another sequence pushes its own on top, and goes back on
    /// afterwards.
    pub(crate) fn run_yield_body(&mut self, args: Vec<Value>) -> Value {
        let Some(mut frame) = self.yield_stack.pop() else {
            // Nothing is driving a loop, so nobody is owed another item. Only
            // reachable if a sequence stored the yield and called it later,
            // which SL2 says it may not.
            return Value::Bool(false);
        };
        // A source that ignored an earlier `false` is calling again after the
        // body already returned or broke out (SEQ13a). Answer `false` again
        // rather than run the body a second time — the loop is over.
        if frame.escaped.is_some() {
            self.yield_stack.push(frame);
            return Value::Bool(false);
        }

        let item = args.into_iter().next().unwrap_or(Value::Unit);
        // Rebind the loop's own variables on top of the sequence's frame. The
        // body reads and writes the scope the `for` sits in, not the one the
        // yield was called from.
        self.env.push_scope();
        for (name, cell) in &frame.scope {
            self.env.define_slot(name.clone(), std::sync::Arc::clone(cell));
        }
        self.define_for_binding(&frame.binding, item);
        let outcome = self.exec_stmts(&frame.body);
        self.env.pop_scope();

        let label = frame.label.as_deref();
        let keep_going = match outcome {
            Ok(_) => true,
            // SEQ7: `break` is `return false`, `continue` is `return true`.
            Err(diag) if breaks_here(&diag.error, label) => false,
            Err(diag) if continues_here(&diag.error, label) => true,
            // A `return`, a `try` propagation, or a `break` aimed further out.
            // Park it for the loop to re-raise and stop the source (SEQ8).
            Err(diag) => {
                frame.escaped = Some(diag);
                false
            }
        };
        self.yield_stack.push(frame);
        Value::Bool(keep_going)
    }

    fn define_for_binding(&mut self, binding: &ForBinding, value: Value) {
        match binding {
            ForBinding::Single(name) => self.env.define(name.clone(), value),
            ForBinding::Tuple(names) => {
                // Destructure tuple/array value into bindings
                if let Some(items) = value.as_tuple_elements() {
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

/// Stamp a freshly created `Pool`'s element type from its `let`/`mut`
/// annotation. `Pool.new()` written bare (the normal style — the element
/// type is already on the left of the `=`) never learns its own element
/// type otherwise: nothing else records it anywhere on the value. Named
/// context-clause resolution (mem.context/CC4) needs that to tell one pool
/// from another when more than one is in scope (`pool_for_context`, #867).
/// A no-op once the pool already carries one (e.g. `Pool<Item>.new()`
/// written out, or a pool passed in from elsewhere).
pub(crate) fn backfill_pool_type_param(value: &Value, ty: &str) {
    if let Value::Pool(p) = value {
        if let Some(elem) = ty.trim().strip_prefix("Pool<").and_then(|s| s.strip_suffix('>')) {
            let mut guard = p.lock().unwrap();
            if guard.type_param.is_none() {
                guard.type_param = Some(elem.trim().to_string());
            }
        }
    }
}

/// OPT6/OPT29: wrap `value` to match the declared `T?` / `T??` / `Result<T, E>`
/// annotation. No-op for non-Option/non-Result annotations, or when the value
/// already has as many optional layers as the annotation asks for. For Result,
/// picks Ok vs Err by the value's type.
///
/// `rhs_is_none_literal` marks a bare `none` on the right-hand side. It names
/// the *outermost* absent, so it never gains a layer no matter how deep the
/// annotation is — `const x: T?? = none` means "nothing at all", not "an empty
/// inner slot" (OPT29).
pub(crate) fn auto_wrap_for_annotation(value: Value, ty: &str, rhs_is_none_literal: bool) -> Value {
    let ty = ty.trim();
    // `i128`/`u128` are 16-byte types (type.primitives), and the interpreter has
    // a value variant for each with full 128-bit arithmetic behind it. What was
    // missing was ever *producing* one: `IntKind` is a tag on an i64 payload and
    // has no 128-bit width, so `let a: i128 = …` bound a plain `Value::Int` and
    // `a + a` wrapped at 64 bits — `i64::MAX + i64::MAX` came back as -2,
    // silently (#762). The annotation is where the width is known.
    if let Value::Int(n, kind) = value {
        // A literal above `i64::MAX` is carried as its *bit pattern* in an i64
        // with an unsigned kind (that's how `u64::MAX` became writable at all —
        // #517), so widening it has to go through u64 or `18446744073709551615`
        // arrives as -1.
        let widened = if kind.signed() { n as i128 } else { n as u64 as i128 };
        match ty {
            "i128" => return Value::Int128(widened),
            // A genuinely negative value has no u128 to widen into; leave it for
            // the ordinary signedness check rather than wrapping it here.
            "u128" if widened >= 0 => return Value::Uint128(widened as u128),
            // `let a: f64 = 1` — type.primitives/L1 lets an unsuffixed literal
            // take the annotated type, and a float is one of the types it can
            // take. This bound a plain `Value::Int`, so the binding printed as
            // `1` and then `a * 2.0` failed with "expected int, got f64" while
            // native computed 2 (#798). The annotation is where the width and
            // the kind are both known.
            "f64" => return Value::Float(n as f64, FloatKind::F64),
            "f32" => return Value::Float(n as f32 as f64, FloatKind::F32),
            _ => {}
        }
    }
    if ty.ends_with('?') && !ty.starts_with('(') {
        if rhs_is_none_literal {
            return value;
        }
        let want = ty.chars().rev().take_while(|c| *c == '?').count();
        let mut out = value;
        for _ in value_option_depth(&out)..want {
            out = Value::Enum {
                name: "Option".to_string(),
                variant: "Some".to_string(),
                fields: vec![out],
                variant_index: 0,
                origin: None,
            };
        }
        return out;
    }
    // A container's elements need the same treatment its binding got. Only the
    // outer type was ever looked at, so `let xs: Vec<i64?> = [1, none, 3]`
    // bound the 1 and the 3 bare — `xs[0]!` then failed with "! requires Option
    // or Result, got i64" while native handled it (#909). The element type is
    // right there in the annotation.
    if let Some(elem_ty) = container_element_type(ty) {
        if let Value::Vec(items) = &value {
            let mut guard = items.lock().unwrap();
            for item in guard.items.iter_mut() {
                // `false` for the none-literal flag: an element that already is
                // `none` carries its layer, and `value_option_depth` counts it,
                // so nothing double-wraps.
                let taken = std::mem::replace(item, Value::Unit);
                *item = auto_wrap_for_annotation(taken, &elem_ty, false);
            }
        }
        return value;
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

/// The element type of a sequence annotation — `Vec<T>`, `[T; N]`, `[]T`.
///
/// `None` for anything else, including a `Map`: a map literal's values would
/// want the same treatment, but its annotation carries two type arguments and
/// the interpreter's map value isn't a `Value::Vec`, so that's its own case
/// rather than a widening of this one.
fn container_element_type(ty: &str) -> Option<String> {
    let ty = ty.trim();
    if let Some(inner) = ty.strip_prefix("Vec<").and_then(|r| r.strip_suffix('>')) {
        return Some(inner.trim().to_string());
    }
    if let Some(inner) = ty.strip_prefix("[]") {
        return Some(inner.trim().to_string());
    }
    if let Some(inner) = ty.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        let elem = inner.split_once(';').map_or(inner, |(e, _)| e);
        return Some(elem.trim().to_string());
    }
    None
}

/// OPT29: is this expression a bare `none` literal?
pub(crate) fn is_none_literal(expr: &rask_ast::expr::Expr) -> bool {
    matches!(expr.kind, rask_ast::expr::ExprKind::None)
}

/// How many optional layers a value already carries at its head.
/// `none` is one layer; `Some(none)` is two; a bare payload is zero.
fn value_option_depth(value: &Value) -> usize {
    match value {
        Value::Enum { name, fields, .. } if name == "Option" => {
            1 + fields.first().map_or(0, value_option_depth)
        }
        _ => 0,
    }
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

