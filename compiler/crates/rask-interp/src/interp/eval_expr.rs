// SPDX-License-Identifier: (MIT OR Apache-2.0)
//\! Expression evaluation.

use indexmap::IndexMap;
use std::sync::{Arc, Mutex, RwLock, mpsc};

use rask_ast::expr::{BinOp, Expr, ExprKind, UnaryOp};

use crate::value::{MapKey, ModuleKind, PoolTask, ThreadHandleInner, ThreadPoolInner, TypeConstructorKind, Value};

use super::{Interpreter, RuntimeDiagnostic, RuntimeError};

/// CC3 runtime panic message for spawn() without an active `using Multitasking` block.
const SPAWN_NO_RUNTIME_MSG: &str =
    "RUNTIME PANIC: spawn() called with no active `using Multitasking` scope\n\
     \n\
     This can happen when:\n\
     - A closure containing spawn is stored and called outside a block\n\
     - A trait object dispatches to an impl that spawns\n\
     - FFI calls back into Rask outside any scope\n\
     \n\
     Install a `using Multitasking { ... }` block that encloses the call.";

/// Copy scalar primitives are copied into a `mutate` param, so a whole-variable
/// argument of scalar type isn't written back (mem.parameters Copy interaction).
fn value_is_copy_scalar(v: &Value) -> bool {
    matches!(
        v,
        Value::Int(..)
            | Value::Int128(_)
            | Value::Uint128(_)
            | Value::Float(_)
            | Value::Bool(_)
            | Value::Char(_)
            | Value::Unit
    )
}

/// type.primitives/NT1 — associated constants on the numeric types.
/// `MIN`/`MAX` carry the receiver's own width so overflow checks see the right
/// bounds; `ZERO`/`ONE` are the same value everywhere.
fn primitive_type_constant(type_name: &str, field: &str) -> Option<Value> {
    use crate::value::IntKind;
    if matches!(type_name, "f32" | "f64") {
        let v = match field {
            "ZERO" => 0.0,
            "ONE" => 1.0,
            "MIN" if type_name == "f32" => f32::MIN as f64,
            "MIN" => f64::MIN,
            "MAX" if type_name == "f32" => f32::MAX as f64,
            "MAX" => f64::MAX,
            "EPSILON" if type_name == "f32" => f32::EPSILON as f64,
            "EPSILON" => f64::EPSILON,
            "INFINITY" => f64::INFINITY,
            "NAN" => f64::NAN,
            _ => return None,
        };
        return Some(Value::Float(v));
    }
    let (min, max, kind) = match type_name {
        "i8" => (i8::MIN as i64, i8::MAX as i64, IntKind::I8),
        "i16" => (i16::MIN as i64, i16::MAX as i64, IntKind::I16),
        "i32" => (i32::MIN as i64, i32::MAX as i64, IntKind::I32),
        "i64" | "isize" => (i64::MIN, i64::MAX, IntKind::I64),
        "u8" => (0, u8::MAX as i64, IntKind::U8),
        "u16" => (0, u16::MAX as i64, IntKind::U16),
        "u32" => (0, u32::MAX as i64, IntKind::U32),
        // u64::MAX doesn't fit an i64; it's carried as the same bit pattern and
        // read back unsigned because the kind says U64.
        "u64" | "usize" => (0, u64::MAX as i64, IntKind::U64),
        _ => return None,
    };
    let n = match field {
        "MIN" => min,
        "MAX" => max,
        "ZERO" => 0,
        "ONE" => 1,
        _ => return None,
    };
    Some(Value::Int(n, kind))
}

/// True when a condition contains an `is` pattern whose bindings the rest of
/// the condition and the taken branch need to see. Only `&&` chains qualify:
/// under `||` or `!` a match on one side says nothing about the other.
pub(super) fn cond_binds_pattern(cond: &Expr) -> bool {
    match &cond.kind {
        ExprKind::IsPattern { .. } => true,
        ExprKind::Binary { op: BinOp::And, left, right } => {
            cond_binds_pattern(left) || cond_binds_pattern(right)
        }
        _ => false,
    }
}

/// Set origin on an error value (the inner payload of Err). Only sets if not already set (ER15).
fn set_error_origin(val: Value, origin: &Arc<str>) -> Value {
    match val {
        Value::Enum { name, variant, fields, variant_index, origin: existing } => {
            Value::Enum {
                name,
                variant,
                fields,
                variant_index,
                origin: Some(existing.unwrap_or_else(|| origin.clone())),
            }
        }
        other => other,
    }
}

/// The absent value an emptied optional slot is left holding (OPT32).
fn absent_value() -> Value {
    Value::Enum {
        name: "Option".to_string(),
        variant: "None".to_string(),
        fields: vec![],
        variant_index: 0,
        origin: None,
    }
}

/// ER31a: build the boundary-enum variant around a propagated error.
impl Interpreter {
    /// Run a `catch` handler with the error bound. `catch _ =>` binds nothing —
    /// the discard is visible in the source, so there's no name to introduce.
    fn run_catch_body(
        &mut self,
        clause: &rask_ast::expr::CatchClause,
        err: Value,
    ) -> Result<Value, RuntimeDiagnostic> {
        self.env.push_scope();
        if !clause.is_discard() {
            self.env.define(clause.binder.clone(), err);
        }
        let result = self.eval_expr(&clause.body);
        self.env.pop_scope();
        result
    }

    fn wrap_propagated_error(&self, wrap: &rask_types::ErrorWrap, inner: Value) -> Value {
        let variant_index = self
            .enums
            .get(&wrap.enum_name)
            .and_then(|d| d.variants.iter().position(|v| v.name == wrap.variant))
            .unwrap_or(0) as u32;
        Value::Enum {
            name: wrap.enum_name.clone(),
            variant: wrap.variant.clone(),
            fields: vec![inner],
            variant_index,
            origin: None,
        }
    }
}

/// Set origin on a Result.Err or Option.None wrapper and its inner error value.
/// Only sets if not already set (first propagation site wins, per ER15).
fn set_result_origin(val: Value, origin: &Arc<str>) -> Value {
    match val {
        Value::Enum { name, variant, fields, variant_index, origin: existing }
            if variant == "Err" || variant == "None" =>
        {
            let fields_with_origin: Vec<Value> = fields.into_iter()
                .map(|f| set_error_origin(f, origin))
                .collect();
            Value::Enum {
                name,
                variant,
                fields: fields_with_origin,
                variant_index,
                origin: Some(existing.unwrap_or_else(|| origin.clone())),
            }
        }
        other => other,
    }
}

/// Map comparison method names back to operator symbols.
fn comparison_op_symbol(method: &str) -> Option<&'static str> {
    match method {
        "eq" => Some("=="),
        "lt" => Some("<"),
        "gt" => Some(">"),
        "le" => Some("<="),
        "ge" => Some(">="),
        _ => None,
    }
}

/// Build a descriptive failure message for assert/check.
///
/// After desugaring, `a == b` becomes `a.eq(b)` and `a != b` becomes
/// `!a.eq(b)`. This function recognizes both forms and re-evaluates the
/// operands to show actual values in the error message.
fn build_comparison_message(interp: &mut Interpreter, condition: &Expr, prefix: &str) -> String {
    match &condition.kind {
        // Desugared comparison: a.eq(b), a.lt(b), etc.
        ExprKind::MethodCall { object, method, args, .. }
            if args.len() == 1 && comparison_op_symbol(method).is_some() =>
        {
            let op_str = comparison_op_symbol(method).unwrap();
            let left_val = interp.eval_expr(object).ok();
            let right_val = interp.eval_expr(&args[0].expr).ok();
            match (left_val, right_val) {
                (Some(l), Some(r)) => format!("{}: {} {} {} (left: {}, right: {})", prefix, l, op_str, r, l, r),
                _ => prefix.to_string(),
            }
        }
        // Desugared != : !(a.eq(b))
        ExprKind::Unary { op: UnaryOp::Not, operand } => {
            match &operand.kind {
                ExprKind::MethodCall { object, method, args, .. }
                    if method == "eq" && args.len() == 1 =>
                {
                    let left_val = interp.eval_expr(object).ok();
                    let right_val = interp.eval_expr(&args[0].expr).ok();
                    match (left_val, right_val) {
                        (Some(l), Some(r)) => format!("{}: {} != {} (left: {}, right: {})", prefix, l, r, l, r),
                        _ => prefix.to_string(),
                    }
                }
                _ => {
                    let val = interp.eval_expr(operand).ok();
                    match val {
                        Some(v) => format!("{}: !({}) — value was {}", prefix, v, v),
                        None => prefix.to_string(),
                    }
                }
            }
        }
        // Pre-desugar Binary (in case desugar was skipped, e.g. spec test runner)
        ExprKind::Binary { op, left, right } if matches!(op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
        ) => {
            let op_str = match op {
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Gt => ">",
                BinOp::Le => "<=",
                BinOp::Ge => ">=",
                _ => unreachable!(),
            };
            let left_val = interp.eval_expr(left).ok();
            let right_val = interp.eval_expr(right).ok();
            match (left_val, right_val) {
                (Some(l), Some(r)) => format!("{}: {} {} {} (left: {}, right: {})", prefix, l, op_str, r, l, r),
                _ => prefix.to_string(),
            }
        }
        // is pattern: assert x is Some
        ExprKind::IsPattern { expr, pattern, .. } => {
            let pat_name = match pattern {
                rask_ast::expr::Pattern::Constructor { name, .. } => name.as_str(),
                rask_ast::expr::Pattern::Ident(n) => n.as_str(),
                rask_ast::expr::Pattern::Wildcard => "_",
                _ => "pattern",
            };
            let val = interp.eval_expr(expr).ok();
            match val {
                Some(v) => format!("{}: {} is not {}", prefix, v, pat_name),
                None => prefix.to_string(),
            }
        }
        _ => prefix.to_string(),
    }
}

impl Interpreter {
    /// Evaluate a condition, defining each `is` pattern's bindings in the
    /// current scope as it matches, so later `&&` operands can use them.
    /// Callers push the scope that holds them.
    pub(super) fn eval_cond_bindings(&mut self, cond: &Expr) -> Result<bool, RuntimeDiagnostic> {
        match &cond.kind {
            ExprKind::Binary { op: BinOp::And, left, right } => {
                if !self.eval_cond_bindings(left)? {
                    return Ok(false);
                }
                self.eval_cond_bindings(right)
            }
            ExprKind::IsPattern { expr: inner, pattern } => {
                let value = self.eval_expr(inner)?;
                match self.match_pattern(pattern, &value) {
                    Some(bindings) => {
                        for (name, val) in bindings {
                            self.env.define(name, val);
                        }
                        Ok(true)
                    }
                    None => Ok(false),
                }
            }
            _ => {
                let value = self.eval_expr(cond)?;
                Ok(self.is_truthy(&value))
            }
        }
    }

    /// Pick which string parse a `parse` call wants. An explicit
    /// `parse<f64>()` names the target; `const x: f64 = s.parse()` leaves it to
    /// inference, so fall back to the checker's type for the call node. Names
    /// other than `parse` pass through untouched.
    fn parse_target_method(
        &self,
        method: &str,
        type_args: &Option<Vec<std::string::String>>,
        node_id: rask_ast::NodeId,
    ) -> std::string::String {
        if method != "parse" {
            return method.to_string();
        }
        let float_target = match type_args.as_ref().and_then(|ta| ta.first()) {
            Some(name) => matches!(name.as_str(), "f32" | "f64"),
            None => matches!(
                self.node_types.get(&node_id),
                Some(rask_types::Type::Result { ok, .. })
                    if matches!(**ok, rask_types::Type::F32 | rask_types::Type::F64)
            ),
        };
        if float_target { "parse_float".to_string() } else { method.to_string() }
    }

    /// Find the `Pool` backing a handle by its pool id. Handle auto-deref
    /// (mem.context/CC1) resolves the element through whichever `Pool<T>` is in
    /// scope; the handle's pool id names it unambiguously, so a match by id
    /// agrees with the compiler's CC4 resolution without needing the name.
    /// Searches struct fields too, so a pool held in `self` (CC4 priority 3) is
    /// reached from a method body.
    pub(crate) fn pool_for_handle(&self, pool_id: u32) -> Option<Arc<Mutex<crate::value::PoolData>>> {
        self.env.find_map(|v| Self::search_pool(v, pool_id, 0))
    }

    fn search_pool(v: &Value, pool_id: u32, depth: usize) -> Option<Arc<Mutex<crate::value::PoolData>>> {
        // Bound the walk so a cyclic struct graph can't loop forever.
        if depth > 8 {
            return None;
        }
        match v {
            Value::Pool(p) => (p.lock().unwrap().pool_id == pool_id).then(|| p.clone()),
            Value::Struct(s) => {
                // Clone field values out before recursing so a self-referential
                // struct can't deadlock on its own lock.
                let fields: Vec<Value> = s.lock().unwrap().fields.values().cloned().collect();
                fields.iter().find_map(|fv| Self::search_pool(fv, pool_id, depth + 1))
            }
            _ => None,
        }
    }

    /// Evaluate an expression whose result is transferred into a new owner
    /// (a binding, an assignment target, a struct field, a collection slot).
    /// Reading a place — a variable, field, or index — copies value-type
    /// aggregates so the new owner can't alias the source (VS1). Fresh
    /// temporaries (literals, calls, arithmetic) are already independent and
    /// pass through untouched.
    pub(crate) fn eval_owned(&mut self, expr: &Expr) -> Result<Value, RuntimeDiagnostic> {
        let value = self.eval_expr(expr)?;
        if Self::expr_is_place(expr) {
            Ok(value.copy_on_bind())
        } else {
            Ok(value)
        }
    }

    /// A place expression names existing storage that may still be live after
    /// the read, so copying it on transfer is required for value semantics.
    fn expr_is_place(expr: &Expr) -> bool {
        matches!(
            expr.kind,
            ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::Index { .. }
        )
    }

    /// Write each `mutate` parameter's captured final value back to its argument
    /// place (mem.parameters/PM2). Consumes the pending writebacks. Parameter
    /// index i maps to `args[i]` for a plain call.
    fn apply_mutate_writebacks(&mut self, args: &[rask_ast::expr::CallArg]) -> Result<(), RuntimeError> {
        let writebacks = std::mem::take(&mut self.mutate_writebacks);
        for (param_idx, final_value) in writebacks {
            if let Some(call_arg) = args.get(param_idx) {
                self.writeback_mutate_place(&call_arg.expr, final_value)?;
            }
        }
        Ok(())
    }

    /// Write a `mutate` param's final value back to its argument place. A whole
    /// Copy scalar variable is copied in — the caller keeps the original — so
    /// only field/index projections and aggregate values write back (this is the
    /// `modify_int(x)` vs `swap_fields(p.x, p.y)` distinction in the spec).
    fn writeback_mutate_place(&mut self, arg: &Expr, value: Value) -> Result<(), RuntimeError> {
        match &arg.kind {
            ExprKind::Ident(_) if value_is_copy_scalar(&value) => Ok(()),
            ExprKind::Ident(_) | ExprKind::Field { .. } | ExprKind::Index { .. } => {
                self.assign_target(arg, value)
            }
            // Non-place arguments (temporaries) have nowhere to write back.
            _ => Ok(()),
        }
    }

    pub(crate) fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeDiagnostic> {
        match &expr.kind {
            ExprKind::Int(n, suffix) => {
                use rask_ast::token::IntSuffix;
                use crate::value::IntKind;
                // Kind comes from an explicit suffix, else the checker's
                // inferred type for this literal (defaults to i32). This is
                // where width first attaches to a value (type.overflow).
                let kind = match suffix {
                    Some(IntSuffix::I128) => return Ok(Value::Int128(*n as i128)),
                    Some(IntSuffix::U128) => return Ok(Value::Uint128(*n as u128)),
                    Some(IntSuffix::I8) => IntKind::I8,
                    Some(IntSuffix::I16) => IntKind::I16,
                    Some(IntSuffix::I32) => IntKind::I32,
                    Some(IntSuffix::I64) | Some(IntSuffix::Isize) => IntKind::I64,
                    Some(IntSuffix::U8) => IntKind::U8,
                    Some(IntSuffix::U16) => IntKind::U16,
                    Some(IntSuffix::U32) => IntKind::U32,
                    Some(IntSuffix::U64) | Some(IntSuffix::Usize)
                    | Some(IntSuffix::U64ByMagnitude) => IntKind::U64,
                    None => self.node_types.get(&expr.id).map(IntKind::from_type).unwrap_or(IntKind::Untyped),
                };
                Ok(Value::Int(*n, kind))
            }
            ExprKind::Float(n, _) => Ok(Value::Float(*n)),
            // A plain string is literal text. It used to be re-scanned for
            // `{...}` at runtime, which broke escapes: `"{{braces}}"` desugars
            // to the literal `{braces}`, and the re-scan read that back as an
            // interpolation and went looking for a variable (#521).
            ExprKind::String(s) => Ok(Value::String(Arc::new(Mutex::new(s.clone())))),

            // Parsed segments, for the paths that skip desugar (the spec test
            // runner). Desugar turns these into a concat chain instead.
            ExprKind::StringInterp(segments) => {
                use rask_ast::expr::StringSegment;
                let mut out = String::new();
                for seg in segments {
                    match seg {
                        StringSegment::Literal(text) => out.push_str(text),
                        StringSegment::Expr(inner, spec) => {
                            let v = self.eval_expr(inner)?;
                            let display = format!("{}", v);
                            match spec {
                                Some(spec) => out.push_str(&self.render_spec(&v, *spec, display)),
                                None => out.push_str(&display),
                            }
                        }
                    }
                }
                Ok(Value::String(Arc::new(Mutex::new(out))))
            }
            ExprKind::Char(c) => Ok(Value::Char(*c)),
            ExprKind::Bool(b) => Ok(Value::Bool(*b)),
            // OPT3: `none` is Option::None — a stateless sentinel, cheaply cloned.
            ExprKind::None => Ok(Value::Enum {
                name: "Option".to_string(),
                variant: "None".to_string(),
                fields: vec![],
                variant_index: 1,
                origin: None,
            }),

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
                    "char" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Char,
                        type_param,
                    }),
                    "Pool" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Pool,
                        type_param,
                    }),
                    "Cell" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Cell,
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
                    "Mutex" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::Mutex,
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
                    "TaskGroup" => return Ok(Value::TypeConstructor {
                        kind: TypeConstructorKind::TaskGroup,
                        type_param,
                    }),
                    "f32x8" => return Ok(Value::Type("f32x8".to_string())),
                    _ => {}
                }
                // User-defined struct types (e.g., Box, Pair)
                if self.struct_decls.contains_key(base_name) {
                    return Ok(Value::Type(base_name.to_string()));
                }
                // Prelude free functions from stdlib/async.rk. These are usable
                // unqualified inside `using Multitasking`, without an import.
                if let Some(kind) = super::register::prelude_builtin(base_name) {
                    return Ok(Value::Builtin(kind));
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
                        ..
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
                                        variant_index: 0, origin: None,
                                    });
                                }
                                "None" => {
                                    return Ok(Value::Enum {
                                        name: "Option".to_string(),
                                        variant: "None".to_string(),
                                        fields: vec![],
                                        variant_index: 0, origin: None,
                                    });
                                }
                                _ => {}
                            }
                        }
                    }

                    return self.call_method(obj_val, field, arg_vals)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span));
                }

                // A bare name in call position is a function, not a variable —
                // report it as one so the help points at the right thing.
                let func_val = self.eval_expr(func).map_err(|diag| {
                    match (&diag.error, &func.kind) {
                        (RuntimeError::UndefinedVariable(n), ExprKind::Ident(_)) => {
                            RuntimeDiagnostic::new(
                                RuntimeError::UndefinedFunction(n.clone()),
                                diag.span,
                            )
                        }
                        _ => diag,
                    }
                })?;
                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(&a.expr))
                    .collect::<Result<_, _>>()?;

                // Clear any writebacks left by sub-calls during arg evaluation, so
                // only this call's `mutate` finals are applied below.
                self.mutate_writebacks.clear();
                let result = self.call_value(func_val, arg_vals)
                    .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                // mem.parameters/PM2: write each `mutate` param's final value back
                // to its argument place. For a plain call, param index i is args[i].
                self.apply_mutate_writebacks(args)
                    .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                Ok(result)
            }

            ExprKind::MethodCall {
                object,
                method,
                type_args,
                args,
            } => {
                if let ExprKind::Ident(name) = &object.kind {
                    if let Some(enum_decl) = self.enums.get(name).cloned() {
                        // .variants() — return Vec of all fieldless variant values
                        if method == "variants" {
                            let has_payload = enum_decl.variants.iter().any(|v| !v.fields.is_empty());
                            if has_payload {
                                return Err(RuntimeDiagnostic::new(
                                    RuntimeError::TypeError(format!(
                                        "variants() requires fieldless enum, but `{}` has variants with fields",
                                        name
                                    )),
                                    expr.span
                                ));
                            }
                            let values: Vec<Value> = enum_decl.variants.iter().enumerate().map(|(idx, v)| {
                                Value::Enum {
                                    name: name.clone(),
                                    variant: v.name.clone(),
                                    fields: vec![],
                                    variant_index: idx as u32, origin: None,
                                }
                            }).collect();
                            return Ok(Value::Vec(Arc::new(Mutex::new(values))));
                        }

                        // E18: from_value(n) — construct enum from integer discriminant
                        if method == "from_value" {
                            let has_payload = enum_decl.variants.iter().any(|v| !v.fields.is_empty());
                            if has_payload {
                                return Err(RuntimeDiagnostic::new(
                                    RuntimeError::TypeError(format!(
                                        "from_value() requires fieldless enum, but `{}` has variants with fields",
                                        name
                                    )),
                                    expr.span
                                ));
                            }
                            let arg_vals: Vec<Value> = args
                                .iter()
                                .map(|a| self.eval_expr(&a.expr))
                                .collect::<Result<_, _>>()?;
                            let target_val = match arg_vals.first() {
                                Some(Value::Int(n, _)) => *n,
                                _ => return Err(RuntimeDiagnostic::new(
                                    RuntimeError::TypeError("from_value() expects an integer argument".to_string()),
                                    expr.span
                                )),
                            };
                            for (idx, v) in enum_decl.variants.iter().enumerate() {
                                let disc = v.discriminant.unwrap_or(idx as i128);
                                if disc == target_val as i128 {
                                    return Ok(Value::Enum {
                                        name: "Option".to_string(),
                                        variant: "Some".to_string(),
                                        fields: vec![Value::Enum {
                                            name: name.clone(),
                                            variant: v.name.clone(),
                                            fields: vec![],
                                            variant_index: idx as u32,
                                            origin: None,
                                        }],
                                        variant_index: 0,
                                        origin: None,
                                    });
                                }
                            }
                            return Ok(Value::Enum {
                                name: "Option".to_string(),
                                variant: "None".to_string(),
                                fields: vec![],
                                variant_index: 1,
                                origin: None,
                            });
                        }

                        if let Some((vidx, variant)) = enum_decl.variants.iter().enumerate().find(|(_, v)| &v.name == method)
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
                                variant_index: vidx as u32, origin: None,
                            });
                        }
                    }

                    // @binary static methods (e.g. IpHeader.parse(data))
                    if self.binary_structs.contains_key(name) {
                        let arg_vals: Vec<Value> = args
                            .iter()
                            .map(|a| self.eval_expr(&a.expr))
                            .collect::<Result<_, _>>()?;
                        if let Some(result) = self.try_binary_static_method(name, method, arg_vals, expr.span) {
                            return result;
                        }
                    }

                    if let Some(type_methods) = self.methods.get(name).cloned() {
                        if let Some(method_fn) = type_methods.get(method) {
                            // Skip empty-body stubs (e.g. fs.write_bytes) —
                            // they exist for native codegen and should fall
                            // through to the built-in module dispatch.
                            let has_body = !method_fn.body.is_empty();
                            let is_static = method_fn
                                .params
                                .first()
                                .map(|p| p.name != "self")
                                .unwrap_or(true);
                            if is_static && has_body {
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

                // Inject type_args for generic methods (e.g. json.decode<T>, reflect.fields<T>)
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
                        if let Value::Module(ModuleKind::Reflect) = &receiver {
                            arg_vals.insert(
                                0,
                                Value::String(Arc::new(Mutex::new(first_type.clone()))),
                            );
                        }
                        // `"3.5".parse<f64>()` — without the type name, parse
                        // has nothing to go on and read every string as an
                        // integer, so parsing a float reported an error (#480).
                        if matches!(&receiver, Value::String(_)) && method == "parse" {
                            arg_vals.insert(
                                0,
                                Value::String(Arc::new(Mutex::new(first_type.clone()))),
                            );
                        }
                    }
                }

                if let Value::Type(type_name) = &receiver {
                    return self.call_type_method(type_name, method, arg_vals)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span));
                }

                // Package-qualified call: lib.greet() → look up lib$greet
                if let Value::Package(pkg_name) = &receiver {
                    let prefixed = format!("{}${}", pkg_name, method);
                    if let Some(func) = self.functions.get(&prefixed).cloned() {
                        return self.call_function(&func, arg_vals);
                    }
                    return Err(RuntimeDiagnostic::new(
                        RuntimeError::UndefinedVariable(method.clone()),
                        expr.span,
                    ));
                }

                // Width-aware integer overflow (type.overflow OV1–OV4, SH1).
                // The width comes from the operand values' IntKind, so this is
                // correct even in generic code. i128/u128 are checked in their
                // own method impls.
                if let Some(result) =
                    self.try_checked_int_arith(&receiver, method, &arg_vals)
                {
                    return result.map_err(|e| RuntimeDiagnostic::new(e, expr.span));
                }

                // `parse` picks its runtime by target type. An explicit
                // `parse<f64>()` carries it in type_args; `const x: f64 =
                // s.parse()` infers it, so fall back to the checker's type for
                // the call. Without this every parse ran the integer path and
                // "3.5" came back as an error (#480).
                let method = self.parse_target_method(method, type_args, expr.id);

                self.call_method(receiver, &method, arg_vals)
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
                        Value::Int(n, kind) => super::overflow::checked_neg(kind, n)
                            .map(|v| Value::Int(v, kind))
                            .map_err(|e| RuntimeDiagnostic::new(e, expr.span)),
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

            ExprKind::Loop { body, label } => {
                let loop_label = label.as_deref();
                loop {
                    self.env.push_scope();
                    match self.exec_stmts(body) {
                        Ok(_) => {}
                        Err(diag) => {
                            self.env.pop_scope();
                            match diag.error {
                                // `break search i` from inside a nested loop
                                // lands here, at the loop that owns the label.
                                RuntimeError::Break(v, ref target)
                                    if target.is_none() || target.as_deref() == loop_label =>
                                {
                                    break Ok(v);
                                }
                                RuntimeError::Continue(ref target)
                                    if target.is_none() || target.as_deref() == loop_label =>
                                {
                                    continue;
                                }
                                _ => break Err(diag),
                            }
                        }
                    }
                    self.env.pop_scope();
                }
            }

            ExprKind::If {
                cond,
                then_branch,
                else_branch,
                else_binding,
            } => {
                // OPT19/OPT20 + ER19/ER20/ER21/ER22: `if x?` or `if expr? as v`
                // evaluates the scrutinee once and rebinds the payload as the
                // narrow name (scrutinee ident for plain, `v` for `as v`,
                // `else as e` for the else branch).
                if let ExprKind::IsPresent { expr: inner, binding } = &cond.kind {
                    let then_name = match (binding, &inner.kind) {
                        (Some(v), _) => Some(v.clone()),
                        (None, ExprKind::Ident(n)) => Some(n.clone()),
                        _ => None,
                    };
                    let else_name = else_binding.clone().or_else(|| then_name.clone());
                    if then_name.is_some() || else_name.is_some() {
                        let scrutinee_val = self.eval_expr(inner)?;
                        let present = matches!(
                            &scrutinee_val,
                            Value::Enum { variant, .. } if matches!(variant.as_str(), "Some" | "Ok")
                        );
                        if present {
                            let payload = match &scrutinee_val {
                                Value::Enum { fields, .. } => fields.first().cloned().unwrap_or(Value::Unit),
                                _ => Value::Unit,
                            };
                            self.env.push_scope();
                            if let Some(name) = then_name {
                                self.env.define(name, payload);
                            }
                            let result = self.eval_expr(then_branch);
                            self.env.pop_scope();
                            return result;
                        } else if let Some(else_br) = else_branch {
                            let payload = match &scrutinee_val {
                                Value::Enum { fields, .. } => fields.first().cloned(),
                                _ => None,
                            };
                            if let (Some(name), Some(p)) = (else_name, payload) {
                                // Result Err branch binds E; Option None has no payload.
                                self.env.push_scope();
                                self.env.define(name, p);
                                let result = self.eval_expr(else_br);
                                self.env.pop_scope();
                                return result;
                            }
                            return self.eval_expr(else_br);
                        } else {
                            return Ok(Value::Unit);
                        }
                    }
                }

                // ER24: early-exit narrowing. `if x == none { <diverges> }` with
                // no `else` — reaching the code after this `if` means the guard
                // didn't fire, so `x` holds the success side. Rebind it directly
                // in the current scope (not a pushed one, unlike the `if x?`
                // case above) so the narrowing is visible to the statements that
                // follow the whole `if`, not just inside a branch.
                // (The `if x is ErrType { … }` shape is parsed as `IfLet`, not
                // `If` — its equivalent narrowing lives in the `IfLet` arm.)
                if else_branch.is_none() {
                    // `x == none` desugars to `x.eq(none)`; a bare `Binary` survives
                    // when desugar was skipped (e.g. the spec test runner).
                    let none_check = match &cond.kind {
                        ExprKind::MethodCall { object, method, args, .. }
                            if method == "eq"
                                && args.len() == 1
                                && matches!(args[0].expr.kind, ExprKind::None) =>
                        {
                            match &object.kind {
                                ExprKind::Ident(name) => Some((name.clone(), object.as_ref())),
                                _ => None,
                            }
                        }
                        ExprKind::Binary { op: BinOp::Eq, left, right }
                            if matches!(right.kind, ExprKind::None) =>
                        {
                            match &left.kind {
                                ExprKind::Ident(name) => Some((name.clone(), left.as_ref())),
                                _ => None,
                            }
                        }
                        _ => None,
                    };
                    if let Some((name, inner)) = none_check {
                        let scrutinee_val = self.eval_expr(inner)?;
                        let is_none = matches!(
                            &scrutinee_val,
                            Value::Enum { variant, .. } if variant == "None"
                        );
                        if is_none {
                            return self.eval_expr(then_branch);
                        }
                        if let Value::Enum { variant, fields, .. } = &scrutinee_val {
                            if variant == "Some" {
                                let payload = fields.first().cloned().unwrap_or(Value::Unit);
                                self.env.define(name, payload);
                            }
                        }
                        return Ok(Value::Unit);
                    }
                }

                // `if x is Pat(v) && …` — the payload has to be in scope for the
                // rest of the condition and for the then-branch (#256).
                if cond_binds_pattern(cond) {
                    self.env.push_scope();
                    let taken = match self.eval_cond_bindings(cond) {
                        Ok(t) => t,
                        Err(e) => {
                            self.env.pop_scope();
                            return Err(e);
                        }
                    };
                    if taken {
                        let result = self.eval_expr(then_branch);
                        self.env.pop_scope();
                        return result;
                    }
                    self.env.pop_scope();
                    return match else_branch {
                        Some(else_br) => self.eval_expr(else_br),
                        None => Ok(Value::Unit),
                    };
                }

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
                        Value::Int(n, _) => n,
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
                        Value::Int(n, _) => n,
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
                    step: 1,
                    rev: false,
                })
            }

            ExprKind::StructLit { name, fields, spread } => {
                // Explicit generic args (`Ring<i64> { }`): run monomorphization
                // for its side effects, but the value carries the BASE name —
                // methods and field decls register under the stripped name
                // (like the inferred `Ring { }` form), so dispatch keys match.
                let concrete_name = if name.contains('<') {
                    self.monomorphize_struct_from_name(name)
                        .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                    name.split('<').next().unwrap_or(name).to_string()
                } else {
                    name.clone()
                };

                let mut field_values = IndexMap::new();

                if let Some(spread_expr) = spread {
                    if let Value::Struct(ref s) = self.eval_expr(spread_expr)? {
                        let guard = s.lock().unwrap();
                        // Spread copies the source's fields into the new struct
                        // (VS1) — sharing them would alias the spread source.
                        for (k, v) in guard.fields.iter() {
                            field_values.insert(k.clone(), v.copy_on_bind());
                        }
                    }
                }

                // A field declared `T?` or `T or E` given a bare `T` has to be
                // wrapped here, the same way an annotated binding is — without
                // it `Holder { slot: 77 }` stored a raw 77 and `h.slot?` then
                // complained the value wasn't an optional at all (#376).
                let field_types = self.struct_decls.get(&concrete_name).map(|d| {
                    d.fields.iter()
                        .map(|f| (f.name.clone(), f.ty.clone()))
                        .collect::<Vec<_>>()
                });
                for field in fields {
                    let value = self.eval_owned(&field.value)?;
                    let value = match field_types.as_ref()
                        .and_then(|ts| ts.iter().find(|(n, _)| *n == field.name))
                    {
                        Some((_, ty)) => super::exec_stmt::auto_wrap_for_annotation(
                            value, ty, super::exec_stmt::is_none_literal(&field.value),
                        ),
                        None => value,
                    };
                    field_values.insert(field.name.clone(), value);
                }

                let resource_id = if self.is_resource_type(&concrete_name) {
                    Some(self.resource_tracker.register(&concrete_name, self.env.scope_depth()))
                } else {
                    None
                };

                Ok(Value::new_struct(concrete_name, field_values, resource_id))
            }

            ExprKind::Field { object, field } => {
                // type.primitives/NT1 — `i32.MAX`, `u8.MIN`, `f64.EPSILON`, …
                // The interpreter had no answer for these at all; only MIR did.
                if let ExprKind::Ident(type_name) = &object.kind {
                    if let Some(v) = primitive_type_constant(type_name, field) {
                        return Ok(v);
                    }
                }
                if let ExprKind::Ident(enum_name) = &object.kind {
                    if let Some(enum_decl) = self.enums.get(enum_name).cloned() {
                        if let Some((vidx, variant)) =
                            enum_decl.variants.iter().enumerate().find(|(_, v)| &v.name == field)
                        {
                            let field_count = variant.fields.len();
                            if field_count == 0 {
                                return Ok(Value::Enum {
                                    name: enum_name.clone(),
                                    variant: field.clone(),
                                    fields: vec![],
                                    variant_index: vidx as u32, origin: None,
                                });
                            } else {
                                return Ok(Value::EnumConstructor {
                                    enum_name: enum_name.clone(),
                                    variant_name: field.clone(),
                                    field_count,
                                    variant_index: vidx as u32,
                                });
                            }
                        }
                    }
                }

                let obj = self.eval_expr(object)?;
                match obj {
                    Value::Struct(ref s) => {
                        Ok(s.lock().unwrap().fields.get(field).cloned().unwrap_or(Value::Unit))
                    }
                    // mem.context/CC1: `h.field` auto-resolves through the active
                    // Pool<T> context — read the element's field. Same generation
                    // check as `pool[h]` (PF5 note: reads check in any context).
                    Value::Handle { pool_id, index, generation } => {
                        let pool = self.pool_for_handle(pool_id).ok_or_else(|| {
                            RuntimeDiagnostic::new(
                                RuntimeError::Panic(format!(
                                    "no Pool in scope to resolve handle field `.{}`",
                                    field
                                )),
                                expr.span,
                            )
                        })?;
                        let pool = pool.lock().unwrap();
                        let idx = pool
                            .validate(pool_id, index, generation)
                            .map_err(|e| RuntimeDiagnostic::new(RuntimeError::Panic(e), expr.span))?;
                        match pool.slots[idx].1.as_ref() {
                            Some(Value::Struct(s)) => {
                                Ok(s.lock().unwrap().fields.get(field).cloned().unwrap_or(Value::Unit))
                            }
                            other => Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "cannot access field '{}' on pool element {}",
                                    field,
                                    other.map(|v| v.type_name()).unwrap_or("empty slot")
                                )),
                                expr.span,
                            )),
                        }
                    }
                    // Nominal type .value extraction
                    Value::Nominal { ref inner, .. } if field == "value" => {
                        Ok(*inner.clone())
                    }
                    // Tuple field access: tuple.0, tuple.1, ...
                    Value::Vec(v) if field.parse::<usize>().is_ok() => {
                        let idx = field.parse::<usize>().unwrap();
                        let vec = v.lock().unwrap();
                        Ok(vec.get(idx).cloned().unwrap_or(Value::Unit))
                    }
                    // `time.Instant`, `http.Response`, `json.JsonValue` — an
                    // exported type name resolves to the type, so the qualified
                    // and unqualified spellings mean the same thing. `math` is the
                    // one module whose members are values rather than types.
                    Value::Module(kind) => {
                        if kind.exports_type(field) {
                            return Ok(Value::Type(field.clone()));
                        }
                        if kind == ModuleKind::Math {
                            return self.get_math_field(field)
                                .map_err(|e| RuntimeDiagnostic::new(e, expr.span));
                        }
                        Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "module has no member '{}'",
                                field
                            )),
                            expr.span
                        ))
                    }
                    // Package field access: lib.Color → look up lib$Color
                    Value::Package(pkg_name) => {
                        let prefixed = format!("{}${}", pkg_name, field);
                        // Enums and structs both resolve to Value::Type so
                        // `lib.Color.Red` works through the normal enum
                        // variant dispatch on the next `.Red` access.
                        if self.enums.contains_key(&prefixed)
                            || self.struct_decls.contains_key(&prefixed)
                            || self.methods.contains_key(&prefixed)
                        {
                            return Ok(Value::Type(prefixed));
                        }
                        if let Some(func) = self.functions.get(&prefixed) {
                            return Ok(Value::Function { name: func.name.clone() });
                        }
                        Err(RuntimeDiagnostic::new(
                            RuntimeError::UndefinedVariable(field.clone()),
                            expr.span,
                        ))
                    }
                    // Type-level field access: handles lib.Color.Red after
                    // lib.Color resolved to Value::Type("lib$Color").
                    Value::Type(type_name) => {
                        if let Some(enum_decl) = self.enums.get(&type_name).cloned() {
                            if let Some((vidx, variant)) = enum_decl.variants.iter().enumerate().find(|(_, v)| v.name == *field) {
                                let field_count = variant.fields.len();
                                if field_count == 0 {
                                    return Ok(Value::Enum {
                                        name: type_name,
                                        variant: field.clone(),
                                        fields: vec![],
                                        variant_index: vidx as u32, origin: None,
                                    });
                                } else {
                                    return Ok(Value::EnumConstructor {
                                        enum_name: type_name,
                                        variant_name: field.clone(),
                                        field_count,
                                        variant_index: vidx as u32,
                                    });
                                }
                            }
                        }
                        // G4: @binary SIZE and SIZE_BITS constants
                        if let Some(meta) = self.binary_structs.get(&type_name) {
                            match field.as_str() {
                                "SIZE" => return Ok(Value::int(meta.size_bytes as i64)),
                                "SIZE_BITS" => return Ok(Value::int(meta.total_bits as i64)),
                                _ => {}
                            }
                        }
                        Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "type '{}' has no field '{}'",
                                type_name, field
                            )),
                            expr.span,
                        ))
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

            // CT49: Dynamic field access — value.(expr) resolves to field access by string
            ExprKind::DynamicField { object, field_expr } => {
                let field_name_val = self.eval_expr(field_expr)?;
                let field_name = match &field_name_val {
                    Value::String(s) => s.lock().unwrap().clone(),
                    _ => {
                        return Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(
                                "dynamic field access requires a string expression".into(),
                            ),
                            expr.span,
                        ));
                    }
                };
                let obj = self.eval_expr(object)?;
                match obj {
                    Value::Struct(ref s) => {
                        let guard = s.lock().unwrap();
                        match guard.fields.get(&field_name) {
                            Some(val) => Ok(val.clone()),
                            None => Err(RuntimeDiagnostic::new(
                                RuntimeError::TypeError(format!(
                                    "struct '{}' has no field '{}'",
                                    guard.name, field_name
                                )),
                                expr.span,
                            )),
                        }
                    }
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "cannot access dynamic field on {}",
                            obj.type_name()
                        )),
                        expr.span,
                    )),
                }
            }

            ExprKind::OptionalField { object, field } => {
                let obj_val = self.eval_expr(object)?;
                match obj_val {
                    Value::Enum { variant, fields, .. } if variant == "Some" => {
                        let inner = fields.into_iter().next().unwrap_or(Value::Unit);
                        // Access field on the inner value
                        let field_val = match inner {
                            Value::Struct(ref s) => {
                                s.lock().unwrap().fields.get(field).cloned().unwrap_or(Value::Unit)
                            }
                            _ => {
                                return Err(RuntimeDiagnostic::new(
                                    RuntimeError::TypeError(format!(
                                        "cannot access field '{}' on {}",
                                        field, inner.type_name()
                                    )),
                                    expr.span,
                                ));
                            }
                        };
                        // If the field is already an Option, return it directly
                        if let Value::Enum { ref name, .. } = field_val {
                            if name == "Option" {
                                return Ok(field_val);
                            }
                        }
                        // Wrap in Some
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "Some".to_string(),
                            fields: vec![field_val],
                            variant_index: 0, origin: None,
                        })
                    }
                    Value::Enum { variant, .. } if variant == "None" => {
                        Ok(Value::Enum {
                            name: "Option".to_string(),
                            variant: "None".to_string(),
                            fields: vec![],
                            variant_index: 0, origin: None,
                        })
                    }
                    _ => {
                        Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "?. requires Option type, got {}",
                                obj_val.type_name()
                            )),
                            expr.span,
                        ))
                    }
                }
            }

            ExprKind::Index { object, index } => {
                let obj = self.eval_expr(object)?;
                let idx = self.eval_expr(index)?;

                match (&obj, &idx) {
                    (Value::Vec(v), Value::Int(i, _)) => {
                        let vec = v.lock().unwrap();
                        let idx = *i as usize;
                        match vec.get(idx).cloned() {
                            Some(val) => Ok(val),
                            None => Err(RuntimeDiagnostic::new(
                                RuntimeError::Panic(format!(
                                    "index out of bounds: index is {} but length is {}",
                                    i, vec.len()
                                )),
                                expr.span,
                            )),
                        }
                    }
                    (Value::Vec(v), Value::Range { start, end, inclusive, .. }) => {
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
                    (Value::String(s), Value::Int(i, _)) => {
                        let str_val = s.lock().unwrap();
                        match str_val.chars().nth(*i as usize) {
                            Some(c) => Ok(Value::Char(c)),
                            None => Err(RuntimeDiagnostic::new(
                                RuntimeError::Panic(format!(
                                    "string index out of bounds: index is {} but length is {}",
                                    i, str_val.chars().count()
                                )),
                                expr.span,
                            )),
                        }
                    }
                    (Value::String(s), Value::Range { start, end, inclusive, .. }) => {
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
                    (Value::Map(m), _) => {
                        let map = m.lock().unwrap();
                        map.get(&MapKey(idx.clone())).cloned().ok_or_else(|| {
                            RuntimeDiagnostic::new(
                                RuntimeError::Panic("key not found in map".to_string()),
                                expr.span,
                            )
                        })
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
                    Value::Int(n, _) => n as usize,
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
                else_binding,
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
                    // ER22: `else as e` binds the branch the test ruled out.
                    self.env.push_scope();
                    if let Some(name) = else_binding {
                        let payload = match &value {
                            Value::Enum { fields, .. } => {
                                fields.first().cloned().unwrap_or(Value::Unit)
                            }
                            other => other.clone(),
                        };
                        self.env.define(name.clone(), payload);
                    }
                    let result = self.eval_expr(else_br);
                    self.env.pop_scope();
                    result
                } else {
                    Ok(Value::Unit)
                }
            }

            ExprKind::IsPattern { expr: inner, pattern } => {
                let value = self.eval_expr(inner)?;
                let matched = self.match_pattern(pattern, &value).is_some();
                Ok(Value::Bool(matched))
            }


            // Guard pattern: const v = expr is Ok(v) else { diverge }
            ExprKind::GuardPattern { expr: inner, pattern, else_branch } => {
                let value = self.eval_expr(inner)?;
                if let Some(bindings) = self.match_pattern(pattern, &value) {
                    // Pattern matched — bind variables in the current scope
                    for (name, val) in &bindings {
                        self.env.define(name.clone(), val.clone());
                    }
                    // Return the payload (first field of Ok/Some variant)
                    match &value {
                        Value::Enum { fields, .. } => {
                            Ok(fields.first().cloned().unwrap_or(Value::Unit))
                        }
                        _ => Ok(value),
                    }
                } else {
                    // Pattern didn't match — execute else branch (should diverge)
                    self.eval_expr(else_branch)?;
                    Ok(Value::Unit)
                }
            }

            ExprKind::Try { expr: inner } => {
                // ER17: `try { … }` block form. Inner `try`s raise TryError;
                // nothing catches it here, so it keeps going to the caller.
                if matches!(&inner.kind, ExprKind::Block(_)) {
                    return self.eval_expr(inner);
                }
                let val = self.eval_expr(inner)?;
                match &val {
                    Value::Enum {
                        variant, fields, ..
                    } => match variant.as_str() {
                        "Ok" | "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                        "Err" | "None" => {
                            let origin = self.origin_string(expr.span);
                            if let Some(wrap) = self.error_wraps.get(&expr.id).cloned() {
                                // ER31a: the caller declared a boundary enum, so
                                // the error leaves wearing it.
                                let inner = fields.first().cloned().unwrap_or(Value::Unit);
                                let wrapped = Value::Enum {
                                    name: "Result".to_string(),
                                    variant: "Err".to_string(),
                                    fields: vec![set_error_origin(
                                        self.wrap_propagated_error(&wrap, inner),
                                        &origin,
                                    )],
                                    variant_index: 0,
                                    origin: Some(origin),
                                };
                                Err(RuntimeDiagnostic::new(RuntimeError::TryError(wrapped), expr.span))
                            } else {
                                let propagated = set_result_origin(val, &origin);
                                Err(RuntimeDiagnostic::new(RuntimeError::TryError(propagated), expr.span))
                            }
                        }
                        _ => Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "`try` requires an Ok/Some or Err/None variant, got {}",
                                variant
                            )),
                            expr.span
                        )),
                    },
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "`try` requires a result or an optional, got {}",
                            val.type_name()
                        )),
                        expr.span
                    )),
                }
            }

            // ER14: `r catch e => body`. The body runs only on failure, with
            // the error bound; `catch _ =>` binds nothing.
            ExprKind::Catch { value, ref clause } => {
                // ER18: on a `try { … }` operand the handler covers the whole
                // block — the first inner `try` that fails lands here.
                let is_try_block = match &value.kind {
                    ExprKind::Try { expr: e } => matches!(e.kind, ExprKind::Block(_)),
                    _ => false,
                };
                if is_try_block {
                    return match self.eval_expr(value) {
                        Ok(v) => Ok(v),
                        Err(diag) => match diag.error {
                            RuntimeError::TryError(err_val) => {
                                let bound = match &err_val {
                                    Value::Enum { fields, .. } => {
                                        fields.first().cloned().unwrap_or(Value::Unit)
                                    }
                                    _ => err_val,
                                };
                                self.run_catch_body(clause, bound)
                            }
                            other => Err(RuntimeDiagnostic::new(other, diag.span)),
                        },
                    };
                }
                let val = self.eval_expr(value)?;
                // ER14a: a handler that produces a two-branch value (`catch _ =>
                // none` is the common one) keeps the shape, so a success goes
                // back wrapped rather than unwrapped.
                let keeps_shape = self.fallback_keeps_shape.contains(&expr.id);
                match &val {
                    Value::Enum { variant, fields, .. } => match variant.as_str() {
                        "Ok" | "Some" if keeps_shape => Ok(val.clone()),
                        "Ok" | "Some" => Ok(fields.first().cloned().unwrap_or(Value::Unit)),
                        "Err" | "None" => {
                            let bound = fields.first().cloned().unwrap_or(Value::Unit);
                            self.run_catch_body(clause, bound)
                        }
                        _ => Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "`catch` requires a result, got variant {}",
                                variant
                            )),
                            expr.span,
                        )),
                    },
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "`catch` requires a result, got {}",
                            val.type_name()
                        )),
                        expr.span,
                    )),
                }
            }

            // OPT32: `take slot` — hand back what was there, leave `none`.
            ExprKind::Take { place } => {
                let val = self.eval_expr(place)?;
                self.assign_target(place, absent_value())
                    .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                Ok(val)
            }

            // Postfix `?` — presence predicate. OPT10/ER12.
            // When in a condition with narrowing, the outer If handler evaluates
            // the scrutinee directly; this path only fires for bare `x?` uses
            // outside a condition.
            ExprKind::IsPresent { expr: inner, .. } => {
                let val = self.eval_expr(inner)?;
                match &val {
                    Value::Enum { variant, .. } => match variant.as_str() {
                        "Some" | "Ok" => Ok(Value::Bool(true)),
                        "None" | "Err" => Ok(Value::Bool(false)),
                        _ => Err(RuntimeDiagnostic::new(
                            RuntimeError::TypeError(format!(
                                "? presence predicate requires Option or Result, got variant {}",
                                variant
                            )),
                            expr.span,
                        )),
                    },
                    _ => Err(RuntimeDiagnostic::new(
                        RuntimeError::TypeError(format!(
                            "? presence predicate requires Option or Result, got {}",
                            val.type_name()
                        )),
                        expr.span,
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
                    // CV1/CV4: int→float rounds. f32 rounds at 24 bits, so the
                    // result has to go through f32 — keeping full i64 precision
                    // here made the interpreter answer 16777217 where native
                    // (which really has an f32) answered 16777216 (#334).
                    (Value::Int(n, _), "f32") => Ok(Value::Float(n as f32 as f64)),
                    (Value::Int(n, _), "f64" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Float(n), t @ ("i64" | "i32" | "int" | "i16" | "i8"
                        | "u64" | "u32" | "u16" | "u8" | "usize")) => {
                        let kind = crate::value::IntKind::from_name(t).unwrap_or(crate::value::IntKind::Untyped);
                        Ok(Value::Int(kind.wrap(n as i64), kind))
                    }
                    (Value::Int(n, _), t @ ("i64" | "i32" | "int" | "i16" | "i8" | "u64" | "u32"
                        | "u16" | "u8" | "usize")) => {
                        let kind = crate::value::IntKind::from_name(t).unwrap_or(crate::value::IntKind::Untyped);
                        Ok(Value::Int(kind.wrap(n), kind))
                    }
                    (Value::Int(n, _), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Float(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Char(c), "i32" | "i64" | "int" | "u32" | "u8" | "u64" | "usize") => {
                        Ok(Value::int(c as i64))
                    }
                    (Value::Int(n, _), "char") => {
                        Ok(Value::Char(char::from_u32(n as u32).unwrap_or('\0')))
                    }
                    // i128 conversions
                    (Value::Int(n, _), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Int(n, _), "u128") => Ok(Value::Uint128(n as u128)),
                    (Value::Int128(n), "i64" | "i32" | "int" | "i16" | "i8") => Ok(Value::int(n as i64)),
                    (Value::Int128(n), "u64" | "u32" | "u16" | "u8" | "usize" | "u128") => Ok(Value::Uint128(n as u128)),
                    (Value::Int128(n), "f32") => Ok(Value::Float(n as f32 as f64)),
                    (Value::Int128(n), "f64" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Int128(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    // u128 conversions
                    (Value::Uint128(n), "i64" | "i32" | "int" | "i16" | "i8") => Ok(Value::int(n as i64)),
                    (Value::Uint128(n), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Uint128(n), "f32") => Ok(Value::Float(n as f32 as f64)),
                    (Value::Uint128(n), "f64" | "float") => Ok(Value::Float(n as f64)),
                    (Value::Uint128(n), "u128" | "u64" | "u32" | "u16" | "u8" | "usize") => Ok(Value::Uint128(n)),
                    (Value::Uint128(n), "string") => {
                        Ok(Value::String(Arc::new(Mutex::new(n.to_string()))))
                    }
                    (Value::Float(n), "i128") => Ok(Value::Int128(n as i128)),
                    (Value::Float(n), "u128") => Ok(Value::Uint128(n as u128)),
                    // E18: fieldless enum to integer cast
                    (Value::Enum { name, variant, fields, variant_index, .. }, target)
                        if fields.is_empty()
                        && (rask_ast::primitives::is_machine_integer(target)
                            || rask_ast::primitives::INT_ALIASES.contains(&target)) =>
                    {
                        let disc = if let Some(enum_decl) = self.enums.get(&name) {
                            enum_decl.variants.iter()
                                .find(|v| v.name == variant)
                                .and_then(|v| v.discriminant)
                                .unwrap_or(variant_index as i128)
                        } else {
                            variant_index as i128
                        };
                        Ok(Value::int(disc as i64))
                    }
                    (v, _) => Ok(v),
                }
            }

            ExprKind::Convert { expr: inner, target, kind } => {
                let val = self.eval_expr(inner)?;
                super::overflow::convert(val, target, *kind)
                    .map_err(|e| RuntimeDiagnostic::new(e, expr.span))
            }

            ExprKind::NullCoalesce { value, default } => {
                let val = self.eval_expr(value)?;
                // ER14a: when the right side is still wrapped the chain carries
                // the layer onward, so a present left operand goes back
                // untouched. Only a collapsing `??` hands back the payload.
                let keeps_shape = self.fallback_keeps_shape.contains(&expr.id);
                match &val {
                    Value::Enum { name, variant, fields, .. }
                        if matches!(name.as_str(), "Option" | "Result")
                            && matches!(variant.as_str(), "Some" | "Ok") =>
                    {
                        if keeps_shape {
                            Ok(val.clone())
                        } else {
                            Ok(fields.first().cloned().unwrap_or(Value::Unit))
                        }
                    }
                    Value::Enum { name, variant, .. }
                        if matches!(name.as_str(), "Option" | "Result")
                            && matches!(variant.as_str(), "None" | "Err") =>
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
                    receiver: Mutex::new(None),
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
                    receiver: Mutex::new(None),
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
                use crate::value::{MultitaskingRuntime, ACTIVE_RUNTIME};

                let num_workers = if args.is_empty() {
                    std::thread::available_parallelism()
                        .map(|n| n.get())
                        .unwrap_or(4)
                } else {
                    self.eval_expr(&args[0].expr)?.as_int()
                        .map_err(|e| RuntimeDiagnostic::new(RuntimeError::TypeError(e), expr.span))? as usize
                };

                let runtime = Arc::new(MultitaskingRuntime::new(num_workers));

                // C1: exactly one active block per process — nested blocks panic at runtime
                {
                    let mut slot = ACTIVE_RUNTIME.write().unwrap();
                    if slot.is_some() {
                        return Err(RuntimeDiagnostic::new(
                            RuntimeError::Panic(
                                "nested `using Multitasking` blocks are not allowed\n\
                                 only one block may be active per process at a time".to_string(),
                            ),
                            expr.span,
                        ));
                    }
                    *slot = Some(runtime.clone());
                }

                self.env.push_scope();
                let scope_depth = self.env.scope_depth();

                let mut result = Value::Unit;
                for stmt in body {
                    match self.exec_stmt(stmt) {
                        Ok(val) => result = val,
                        Err(e) => {
                            runtime.shutdown();
                            *ACTIVE_RUNTIME.write().unwrap() = None;
                            self.env.pop_scope();
                            return Err(e);
                        }
                    }
                }

                // Check for unconsumed handles (conc.async/H1)
                if let Err(msg) = self.resource_tracker.check_scope_exit(scope_depth) {
                    runtime.shutdown();
                    *ACTIVE_RUNTIME.write().unwrap() = None;
                    self.env.pop_scope();
                    return Err(RuntimeDiagnostic::new(
                        RuntimeError::Panic(msg),
                        expr.span,
                    ));
                }

                runtime.shutdown();
                *ACTIVE_RUNTIME.write().unwrap() = None;
                self.env.pop_scope();
                Ok(result)
            }

            ExprKind::Spawn { body } => {
                use crate::value::ACTIVE_RUNTIME;

                let body = body.clone();
                let captured = self.env.capture();
                let child = self.spawn_child(captured);

                // Read the active runtime from the process-global slot (CC3 fallback if None)
                let runtime = ACTIVE_RUNTIME.read().unwrap().clone();
                let rt = match runtime {
                    Some(rt) => rt,
                    None => {
                        return Err(RuntimeDiagnostic::new(
                            RuntimeError::Panic(SPAWN_NO_RUNTIME_MSG.to_string()),
                            expr.span,
                        ));
                    }
                };

                // Submit to the multitasking thread pool
                let (result_tx, result_rx) = std::sync::mpsc::channel();
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

                {
                    let sender = rt.sender.lock().unwrap();
                    if let Some(tx) = sender.as_ref() {
                        let _ = tx.send(task);
                    }
                }

                let handle_inner = Arc::new(ThreadHandleInner {
                    handle: Mutex::new(None),
                    receiver: Mutex::new(Some(result_rx)),
                });

                // Register handle for affine tracking (conc.async/H1)
                let ptr = Arc::as_ptr(&handle_inner) as usize;
                self.resource_tracker.register_handle(ptr, "TaskHandle", self.env.scope_depth());

                Ok(Value::TaskHandle(handle_inner))
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
                        build_comparison_message(self, condition, "assertion failed")
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
                        build_comparison_message(self, condition, "check failed")
                    };
                    Err(RuntimeDiagnostic::new(RuntimeError::CheckFailed(msg), expr.span))
                }
            }

            ExprKind::WithAs { bindings, body } => {
                // Classify each binding source
                enum WithSource {
                    /// pool[handle] — index-based collection access
                    Index { collection: Value, key: Value },
                    /// Mutex — exclusive lock
                    Mutex(Arc<Mutex<Value>>),
                    /// Cell<T> — exclusive access (CE4/CE5)
                    Cell(Arc<Mutex<Value>>),
                    /// Shared.read() — shared read lock
                    SharedRead(Arc<RwLock<Value>>),
                    /// Shared.write() — exclusive write lock
                    SharedWrite(Arc<RwLock<Value>>),
                }

                struct WithInfo {
                    source: WithSource,
                    name: String,
                }

                let mut infos: Vec<WithInfo> = Vec::new();

                for binding in bindings {
                    let source = if let ExprKind::Index { object, index } = &binding.source.kind {
                        // pool[handle] pattern
                        let collection = self.eval_expr(object)?;
                        let key = self.eval_expr(index)?;
                        WithSource::Index { collection, key }
                    } else if let ExprKind::MethodCall { object, method, .. } = &binding.source.kind {
                        // shared.read() or shared.write()
                        let obj = self.eval_expr(object)?;
                        match (&obj, method.as_str()) {
                            (Value::Shared(s), "read") => WithSource::SharedRead(Arc::clone(s)),
                            (Value::Shared(s), "write") => WithSource::SharedWrite(Arc::clone(s)),
                            (Value::RaskMutex(m), "lock") => WithSource::Mutex(Arc::clone(m)),
                            _ => {
                                return Err(RuntimeDiagnostic::new(
                                    RuntimeError::TypeError(format!(
                                        "with...as: unsupported method call .{}() on {}",
                                        method, obj.type_name()
                                    )),
                                    expr.span,
                                ));
                            }
                        }
                    } else {
                        // Plain expression — evaluate and check type
                        let val = self.eval_expr(&binding.source)?;
                        match val {
                            Value::RaskMutex(m) => WithSource::Mutex(m),
                            Value::Cell(c) => WithSource::Cell(c),
                            _ => {
                                return Err(RuntimeDiagnostic::new(
                                    RuntimeError::TypeError(format!(
                                        "with...as: expected Cell, Mutex, Shared, or collection index, got {}",
                                        val.type_name()
                                    )),
                                    expr.span,
                                ));
                            }
                        }
                    };

                    infos.push(WithInfo {
                        source,
                        name: binding.name.clone(),
                    });
                }

                // Check aliasing for index-based bindings
                for i in 0..infos.len() {
                    for j in (i + 1)..infos.len() {
                        if let (
                            WithSource::Index { collection: c1, key: k1 },
                            WithSource::Index { collection: c2, key: k2 },
                        ) = (&infos[i].source, &infos[j].source) {
                            if Self::value_eq(c1, c2) && Self::value_eq(k1, k2) {
                                return Err(RuntimeDiagnostic::new(
                                    RuntimeError::Panic(
                                        "with...as: duplicate key in same collection (aliasing)".to_string(),
                                    ),
                                    expr.span,
                                ));
                            }
                        }
                    }
                }

                // Acquire locks and bind values
                self.env.push_scope();

                // Hold lock guards in scope for Mutex/Shared
                let mut mutex_guards: Vec<(String, std::sync::MutexGuard<'_, Value>)> = Vec::new();
                let mut rw_read_guards: Vec<std::sync::RwLockReadGuard<'_, Value>> = Vec::new();
                let mut rw_write_guards: Vec<(String, std::sync::RwLockWriteGuard<'_, Value>)> = Vec::new();

                for info in &infos {
                    match &info.source {
                        WithSource::Index { collection, key } => {
                            let elem = self.index_into(collection, key)
                                .map_err(|e| RuntimeDiagnostic::new(e, expr.span))?;
                            self.env.define(info.name.clone(), elem);
                        }
                        WithSource::Mutex(m) => {
                            let guard = m.lock().map_err(|e| RuntimeDiagnostic::new(
                                RuntimeError::Panic(format!("Mutex.lock: poisoned: {}", e)),
                                expr.span,
                            ))?;
                            self.env.define(info.name.clone(), guard.clone());
                            mutex_guards.push((info.name.clone(), guard));
                        }
                        WithSource::Cell(c) => {
                            let guard = c.lock().map_err(|_| RuntimeDiagnostic::new(
                                RuntimeError::Panic(
                                    "Cell is exclusively borrowed — recursive access in with block".to_string(),
                                ),
                                expr.span,
                            ))?;
                            self.env.define(info.name.clone(), guard.clone());
                            mutex_guards.push((info.name.clone(), guard));
                        }
                        WithSource::SharedRead(s) => {
                            let guard = s.read().map_err(|e| RuntimeDiagnostic::new(
                                RuntimeError::Panic(format!("Shared.read: poisoned: {}", e)),
                                expr.span,
                            ))?;
                            self.env.define(info.name.clone(), guard.clone());
                            rw_read_guards.push(guard);
                        }
                        WithSource::SharedWrite(s) => {
                            let guard = s.write().map_err(|e| RuntimeDiagnostic::new(
                                RuntimeError::Panic(format!("Shared.write: poisoned: {}", e)),
                                expr.span,
                            ))?;
                            self.env.define(info.name.clone(), guard.clone());
                            rw_write_guards.push((info.name.clone(), guard));
                        }
                    }
                }

                // Execute body. Capture the exit instead of `?`-returning: unwind
                // releases access but keeps writes (ctrl.panic/U2), so the
                // writeback and scope-pop below must run even on panic.
                let mut body_result: Result<Value, RuntimeDiagnostic> = Ok(Value::Unit);
                for stmt in body {
                    match self.exec_stmt(stmt) {
                        Ok(v) => body_result = Ok(v),
                        Err(e) => { body_result = Err(e); break; }
                    }
                }

                // Writeback (U2: mutations made before the panic are flushed,
                // not rolled back). Read locks never write back — the checker
                // rejects mutation through them (conc.sync/R1).
                let mut writeback_err: Option<RuntimeDiagnostic> = None;
                for info in &infos {
                    if !matches!(info.source, WithSource::SharedRead(_)) {
                        if let Some(updated) = self.env.get(&info.name).cloned() {
                            match &info.source {
                                WithSource::Index { collection, key } => {
                                    if let Err(e) = self.write_back_index(collection, key, updated) {
                                        if writeback_err.is_none() {
                                            writeback_err = Some(RuntimeDiagnostic::new(e, expr.span));
                                        }
                                    }
                                }
                                WithSource::Mutex(_) | WithSource::Cell(_) | WithSource::SharedWrite(_) => {
                                    // Writeback handled via guards below
                                }
                                WithSource::SharedRead(_) => {
                                    // Read-only, no writeback
                                }
                            }
                        }
                    }
                }

                // Write back to Mutex/Cell guards
                for (name, mut guard) in mutex_guards {
                    if let Some(updated) = self.env.get(&name) {
                        *guard = updated.clone();
                    }
                }

                // Write back to Shared write guards
                for (name, mut guard) in rw_write_guards {
                    if let Some(updated) = self.env.get(&name) {
                        *guard = updated.clone();
                    }
                }

                // Read guards dropped automatically
                drop(rw_read_guards);

                self.env.pop_scope();

                // A body panic/error wins; otherwise surface a writeback failure.
                match body_result {
                    Err(e) => Err(e),
                    Ok(v) => match writeback_err {
                        Some(e) => Err(e),
                        None => Ok(v),
                    },
                }
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
                #[allow(dead_code)]
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

                    // All channels closed (CL1). This used to hand back an
                    // `Err(...)` — but a select's type is its arms' type, so a
                    // Result appearing there is a value nothing can use:
                    // `const got: i64 = select { … }` would be holding an enum.
                    // Native panics here, and now so does this.
                    if all_closed && default_idx.is_none() {
                        return Err(RuntimeDiagnostic::new(
                            RuntimeError::Panic(
                                "select: every channel is closed [conc.select/CL1]".to_string(),
                            ),
                            expr.span,
                        ));
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

