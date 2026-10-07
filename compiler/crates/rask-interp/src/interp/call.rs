// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Function calling and ensure blocks.

use rask_ast::decl::FnDecl;
use rask_ast::expr::ExprKind;
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;
use rask_ast::ty::TypeExpr;
use std::collections::HashSet;

use crate::value::{GenericFrame, Value};

use super::{Interpreter, RuntimeDiagnostic, RuntimeError};

impl Interpreter {
    /// Keep recursing past the end of the host stack by moving onto a new one.
    ///
    /// The interpreter spends one host stack frame per Rask call, and those
    /// frames are large — `eval_expr` is a single match over 80 expression kinds
    /// and Rust sizes a frame for the union of every arm's locals. 16 MiB is
    /// therefore only ~465 Rask calls deep. Running out used to be a SIGABRT
    /// with no message at all, then an R0023 diagnostic (#759) — but both are
    /// wrong answers to `down(300)`, because the same program compiled natively
    /// recurses into the millions and the interpreter is the reference for what
    /// the answer should be.
    ///
    /// So a call that can't fit continues on a thread with a fresh stack; this
    /// one blocks in `join` until it returns. Nothing the program can observe
    /// changes — same interpreter, same environment, same values.
    ///
    /// Whether there's room is measured against the stack pointer rather than
    /// counted, because one Rask frame costs anywhere from a few KB to tens of
    /// KB depending on how deeply nested the expressions in the body are: 467
    /// frames of `return 1 + down(n - 1)` fit in the same stack as 227 of a body
    /// doing nested arithmetic and interpolation. A fixed depth limit is either
    /// wrong for the heavy body or needlessly low for the light one.
    ///
    /// The chain of stacks is capped, so runaway recursion is still a diagnostic
    /// rather than a machine out of threads.
    ///
    /// `generics` is what the body's type parameters stand for in this call,
    /// handed over by whoever made it: the call site's own record for a call
    /// written in the source, the function value's for one passed around.
    pub(crate) fn call_function(
        &mut self,
        func: &FnDecl,
        args: Vec<Value>,
        generics: GenericFrame,
    ) -> Result<Value, RuntimeDiagnostic> {
        if crate::stack_nearly_exhausted() {
            if crate::stack_segments_exhausted() {
                return Err(RuntimeDiagnostic::new(
                    RuntimeError::RecursionTooDeep {
                        function: func.name.clone(),
                        depth: self.call_depth,
                    },
                    func.span,
                ));
            }
            return crate::grow_interp_stack(move || self.call_counted(func, args, generics));
        }
        self.call_counted(func, args, generics)
    }

    fn call_counted(
        &mut self,
        func: &FnDecl,
        args: Vec<Value>,
        generics: GenericFrame,
    ) -> Result<Value, RuntimeDiagnostic> {
        self.call_depth += 1;
        // XC4: the package whose code is now running. A conformance another
        // package also declares is looked up against this, the same way native
        // puts the declaring package in the symbol — so `liba`'s own `label`
        // call reaches `liba`'s body whoever linked it.
        let pushed = !self.file_packages.is_empty();
        if pushed {
            let pkg = self.file_packages.get(&func.span.file_id).cloned();
            self.package_stack.push(pkg);
        }
        let result = self.call_function_at_depth(func, args, generics);
        if pushed {
            self.package_stack.pop();
        }
        self.call_depth -= 1;
        // Where it actually happened, for the callers that lose it. Everything
        // between a method call and this point hands back a bare
        // `RuntimeError`, so the span is gone by the time anyone rebuilds a
        // diagnostic and each frame re-attached its own — leaving the outermost
        // call in `main` as the reported line (#1110). Read and restored around
        // the call in `eval_expr`, so a swallowed error can't leave a stale one
        // for something later.
        if let Err(diag) = &result {
            self.failed_call_span = Some(diag.span);
        }
        result
    }

    fn call_function_at_depth(
        &mut self,
        func: &FnDecl,
        mut args: Vec<Value>,
        generics: GenericFrame,
    ) -> Result<Value, RuntimeDiagnostic> {
        // Taken before anything else runs, so a default argument's own calls
        // can't see it. Only the call it was made for may use it.
        let lent: Vec<Option<crate::env::Slot>> = self
            .lent_args
            .take()
            .filter(|l| l.depth + 1 == self.call_depth && l.callee == func.name)
            .map(|l| l.slots)
            .unwrap_or_default();
        // Fill in default values for missing trailing arguments
        if args.len() < func.params.len() {
            for i in args.len()..func.params.len() {
                if let Some(ref default_expr) = func.params[i].default {
                    let val = self.eval_expr(default_expr)?;
                    args.push(val);
                } else {
                    return Err(RuntimeDiagnostic::new(
                        RuntimeError::ArityMismatch {
                            expected: func.params.len(),
                            got: i,
                        },
                        Span::new(0, 0)
                    ));
                }
            }
        }
        if args.len() != func.params.len() {
            return Err(RuntimeDiagnostic::new(
                RuntimeError::ArityMismatch {
                    expected: func.params.len(),
                    got: args.len(),
                },
                Span::new(0, 0)
            ));
        }

        self.env.push_scope();

        self.generic_frames.push(generics);

        for (i, (param, arg)) in func.params.iter().zip(args.into_iter()).enumerate() {
            // A by-value parameter receives an independent copy (VS1): mutating
            // it inside the callee can't alias the caller's value. `mutate`/`self`
            // borrows share the caller's storage by design; `take` moves it, so
            // the source is already dead — none of those copy.
            let arg = if param.is_mutate || param.is_take || param.name == "self" {
                arg
            } else {
                arg.copy_on_bind()
            };
            // OPT6: a bare `T` passed where the parameter is `T?` widens at the
            // call. Without it `present_bare(3)` bound a raw 3 and `x?` had no
            // tag to read (#393).
            let arg = match &param.ty {
                Some(ty) => wrap_optional_layers(arg, ty),
                None => arg,
            };
            // PM2: a `mutate` parameter is the caller's place, bound as such.
            // An argument that isn't one (a temporary) gets storage of its
            // own, since nobody can see what is written there. Either way the
            // binding is borrowed, which a closure built here has to know (CM3).
            if param.is_mutate {
                let cell = lent.get(i).cloned().flatten().unwrap_or_else(|| crate::env::slot(arg));
                self.env.define_lent(param.name.clone(), cell);
                continue;
            }
            self.env.define(param.name.clone(), arg);
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
        for param in func.params.iter().filter(|p| p.is_mutate) {
            if let Some(v) = self.env.get(&param.name) {
                self.hand_resources_to_caller(&v, caller_depth);
            }
        }

        self.resource_tracker.end_scope(scope_depth);

        self.generic_frames.pop();
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

        let result_sides = func.ret_ty.as_ref().and_then(result_sides);
        let returns_result = result_sides.is_some();
        // Both spellings. `wrap_optional_layers` has always understood
        // `Option<T>` as well as `T?` — this gate didn't, so a function
        // declared the long way handed back a bare `T` and the caller's `!`
        // said "requires Option or Result, got i64". `get_clone` in
        // `stdlib/collections.rk` is written that way, so every `get_clone` on
        // the interpreter was broken (#1211).
        let returns_option = func.ret_ty.as_ref().is_some_and(|t| optional_depth(t) > 0);
        if returns_result {
            // Already a Result: pass through.
            if matches!(&value, Value::Enum { name, .. } if name == "Result") {
                return Ok(value);
            }
            // ER9: pick the branch by the value's runtime type. If the value
            // matches E (or a variant of a union E), wrap as Err; else Ok.
            // Disjointness (ER3) makes this unambiguous.
            let err_names = result_sides.map(|(_, err)| err_arms(err)).unwrap_or_default();
            let is_err_branch = self.value_matches_any_err(&value, &err_names);
            if is_err_branch {
                return Ok(Value::Enum {
                    name: "Result".to_string(),
                    variant: "Err".to_string(),
                    fields: vec![value],
                    variant_index: 1, origin: None,
                });
            }
            // ER9: the ok value still has to satisfy the ok side. `KV? or E`
            // returning a bare KV needs the optional layer too — without it,
            // `try f()` handed back a KV where the caller expected a KV? and
            // read it as absent (#383).
            let payload = match result_sides {
                Some((ok, _)) => wrap_optional_layers(value, ok),
                None => value,
            };
            return Ok(Value::Enum {
                name: "Result".to_string(),
                variant: "Ok".to_string(),
                fields: vec![payload],
                variant_index: 0, origin: None,
            });
        } else if returns_option {
            // As many layers as the signature declares, not one: `-> T??`
            // returning a bare `T` got a single Some, and the caller's second
            // peel found a `T` where it expected an Option and read it as
            // absent. Same helper the ok side and the arguments use.
            match func.ret_ty.as_ref() {
                Some(ret) => Ok(wrap_optional_layers(value, ret)),
                None => Ok(value),
            }
        } else {
            Ok(value)
        }
    }

    /// Runs ensure blocks in LIFO order on block exit.
    pub(super) fn exec_stmts(&mut self, stmts: &[Stmt]) -> Result<Value, RuntimeDiagnostic> {
        crate::preempt_point();
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

        // P5/EX3: `os.exit` terminates immediately — no unwind, no ensures. The
        // ensure-inside-ensure case was already handled below; a plain
        // `os.exit(7)` in the body still ran every scheduled cleanup on the way
        // out, which is the opposite of what exit means.
        if matches!(&exit_error, Some(d) if matches!(d.error, RuntimeError::Exit(_))) {
            return Err(exit_error.unwrap());
        }

        // A panic exiting the body means we're already unwinding; ensure-body
        // panics during that unwind are secondary (ctrl.panic/E3).
        let body_panicked = matches!(&exit_error, Some(d) if matches!(d.error, RuntimeError::Panic(_)));
        // L6: carrying a linear value out of this scope is consuming it, so the
        // `ensure` scheduled here is cancelled — whoever catches the value owes
        // the consumption now. Without this the ensure ran on the way out and
        // closed what was about to be handed over, so `let a = open(); ensure
        // a.close(); return a` gave back a closed handle. Native was doing the
        // same thing and saying nothing, because it has no tracker to notice a
        // second consume; `check_resource_moved` in the lowering is that half.
        //
        // `break a` counts for the same reason: it leaves the loop body carrying
        // the resource, and the loop's own ensure is on this block.
        let handed_back = match &exit_error {
            Some(d) => match &d.error {
                RuntimeError::Return(v) | RuntimeError::Break(v, _) => self.resource_ids_in(v),
                _ => HashSet::new(),
            },
            None => self.resource_ids_in(&last_value),
        };
        let ensure_fatal = self.run_ensures_except(&ensures, body_panicked, &handed_back);

        match (exit_error, ensure_fatal) {
            // os.exit() inside an ensure terminates immediately, no matter what (P5).
            (_, Some(f)) if matches!(f.error, RuntimeError::Exit(_)) => Err(f),
            // A panic from the body is primary; ensure panics were already
            // reported as secondary inside run_ensures. (A body `Exit` never
            // reaches here — it returned above without running any ensure.)
            (Some(e), _) if matches!(e.error, RuntimeError::Panic(_)) => Err(e),
            // A panic raised by an ensure kills the task (E1), overriding a
            // non-panic body exit (error propagation, return, break, continue).
            (_, Some(f)) => Err(f),
            // No ensure fatal: the body's own exit propagates.
            (Some(e), _) => Err(e),
            (None, None) => Ok(last_value),
        }
    }

    /// Runs ensures in LIFO order. Every scheduled ensure runs even if an earlier
    /// one panics (E2). Returns the first ensure-body panic (E3) — or an `Exit`,
    /// which stops the remaining ensures (P5). Later panics, and every ensure
    /// panic when already unwinding from a prior panic, are reported to stderr as
    /// secondary panics. Skips ensures whose receiver was already consumed.
    pub(super) fn run_ensures(&mut self, ensures: &[&Stmt], unwinding: bool) -> Option<RuntimeDiagnostic> {
        self.run_ensures_except(ensures, unwinding, &HashSet::new())
    }

    /// `run_ensures`, skipping the ones whose receiver is being handed to the
    /// caller.
    pub(super) fn run_ensures_except(
        &mut self,
        ensures: &[&Stmt],
        unwinding: bool,
        handed_back: &HashSet<u64>,
    ) -> Option<RuntimeDiagnostic> {
        let mut first_panic: Option<RuntimeDiagnostic> = None;
        for ensure_stmt in ensures.iter().rev() {
            if let StmtKind::Ensure { body, else_handler } = &ensure_stmt.kind {
                // Explicit consumption cancels ensure.
                if self.ensure_receiver_consumed(body) {
                    continue;
                }
                if self
                    .ensure_receiver_id(body)
                    .is_some_and(|id| handed_back.contains(&id))
                {
                    continue;
                }

                let result = self.exec_ensure_body(body);

                match result {
                    Ok(value) => {
                        if let Value::Enum { name, variant, fields, .. } = &value {
                            if name == "Result" && variant == "Err" {
                                let err_val = fields.first().cloned().unwrap_or(Value::Unit);
                                self.handle_ensure_error(err_val, else_handler);
                            }
                        }
                    }
                    Err(diag) if matches!(&diag.error, RuntimeError::Panic(_)) => {
                        // E3: first panic wins. A panic here while already unwinding,
                        // or after a prior ensure-panic, is contained + reported.
                        if unwinding || first_panic.is_some() {
                            self.report_secondary_panic(&diag);
                        } else {
                            first_panic = Some(diag);
                        }
                        // E2: keep running the remaining ensures.
                    }
                    Err(diag) if matches!(&diag.error, RuntimeError::Exit(_)) => {
                        return Some(diag);
                    }
                    Err(diag) if matches!(&diag.error, RuntimeError::TryError(_)) => {
                        if let RuntimeError::TryError(val) = diag.error {
                            self.handle_ensure_error(val, else_handler);
                        }
                    }
                    // Anything else is the interpreter failing, not the
                    // program: a missing method, a type error. Dropping it made
                    // an `ensure` that couldn't run look like one that did.
                    Err(diag) => {
                        if first_panic.is_none() {
                            first_panic = Some(diag);
                        }
                    }
                }
            }
        }
        first_panic
    }

    /// Report a panic that fired during unwind and was contained (E3). The first
    /// panic remains the task's panic; this one goes to stderr and continues.
    fn report_secondary_panic(&self, diag: &RuntimeDiagnostic) {
        if let RuntimeError::Panic(msg) = &diag.error {
            eprintln!(
                "secondary panic during unwind at {}: {}",
                self.origin_string(diag.span),
                msg
            );
        }
    }

    /// Has the thing this ensure would consume already been consumed?
    fn ensure_receiver_consumed(&self, body: &[Stmt]) -> bool {
        self.ensure_receiver_id(body)
            .is_some_and(|id| self.resource_tracker.is_consumed(id))
    }

    /// The resource this ensure body would consume.
    ///
    /// Three spellings reach here: `ensure c.close()`, `ensure w.conn.close()`
    /// where the receiver is a field of a holder, and `ensure drop(p)` — which
    /// is the whole cleanup vocabulary of `Heap<T>`, and a call rather than a
    /// method, so reading only the method form found no receiver and ran the
    /// cleanup a second time.
    fn ensure_receiver_id(&self, body: &[Stmt]) -> Option<u64> {
        let StmtKind::Expr(expr) = &body.first()?.kind else {
            return None;
        };
        let receiver = match &expr.kind {
            ExprKind::MethodCall { object, .. } => object.as_ref(),
            ExprKind::Call { func, args } => {
                match &func.kind {
                    ExprKind::Ident(n) if n == "drop" => &args.first()?.expr,
                    _ => return None,
                }
            }
            _ => return None,
        };
        let value = self.resolve_place(receiver)?;
        self.get_resource_id(&value)
    }

    /// Read an `a` or an `a.b.c` without evaluating anything that could run.
    fn resolve_place(&self, expr: &rask_ast::expr::Expr) -> Option<Value> {
        match &expr.kind {
            ExprKind::Ident(name) => self.env.get(name),
            ExprKind::Field { object, field } => {
                let base = self.resolve_place(object)?;
                match base {
                    Value::Struct(ref s) => s.lock().unwrap().fields.get(field).cloned(),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// Every linear value inside a value, however deeply nested.
    fn resource_ids_in(&self, value: &Value) -> HashSet<u64> {
        let mut out = HashSet::new();
        if !self.resource_tracker.is_empty() {
            self.collect_resource_ids(value, &mut out);
        }
        out
    }

    fn collect_resource_ids(&self, value: &Value, out: &mut HashSet<u64>) {
        if let Some(id) = self.get_resource_id(value) {
            out.insert(id);
        }
        match value {
            Value::Struct(ref s) => {
                let fields: Vec<Value> = s.lock().unwrap().fields.values().cloned().collect();
                for f in &fields {
                    self.collect_resource_ids(f, out);
                }
            }
            Value::Enum { fields, .. } => {
                for f in fields {
                    self.collect_resource_ids(f, out);
                }
            }
            Value::Tuple(items) => {
                for item in items.iter() {
                    self.collect_resource_ids(item, out);
                }
            }
            _ => {}
        }
    }

    fn exec_ensure_body(&mut self, body: &[Stmt]) -> Result<Value, RuntimeDiagnostic> {
        let mut last_value = Value::Unit;
        for stmt in body {
            last_value = self.exec_stmt(stmt)?;
            // A binding statement evaluates to Unit, but the cleanup call whose
            // failure the `else` handler exists for is its initializer — so
            // `ensure { let n = s.close() }` has to read `n` back, or a failed
            // close looks like a body that produced nothing.
            if let Some(name) = binding_name(&stmt.kind) {
                if let Some(bound) = self.env.get(name) {
                    last_value = bound.clone();
                }
            }
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

/// The name a single-binding statement binds, if it is one. Destructuring forms
/// are deliberately absent: a `T or E` can't be taken apart by one.
fn binding_name(kind: &StmtKind) -> Option<&str> {
    match kind {
        StmtKind::Let { name, .. } | StmtKind::Mut { name, .. } => Some(name),
        _ => None,
    }
}

/// The two sides of a result type, either spelling: `T or E`, `Result<T, E>`.
pub(super) fn result_sides(ty: &TypeExpr) -> Option<(&TypeExpr, &TypeExpr)> {
    match ty {
        TypeExpr::Result { ok, err } => Some((ok, err)),
        TypeExpr::Named { path, args } if path.len() == 1 && path[0] == "Result" => match args.as_slice() {
            [ok, err] => Some((ok, err)),
            _ => None,
        },
        _ => None,
    }
}

/// How many optional layers a written type asks for: `KV?` and `Option<KV>`
/// one, `KV??` two.
pub(super) fn optional_depth(ty: &TypeExpr) -> usize {
    match ty {
        TypeExpr::Optional(inner) => 1 + optional_depth(inner),
        TypeExpr::Named { path, args } if path.len() == 1 && path[0] == "Option" && args.len() == 1 => {
            1 + optional_depth(&args[0])
        }
        _ => 0,
    }
}

/// Add whatever optional layers `ty` asks for that `value` doesn't already
/// carry.
fn wrap_optional_layers(value: Value, ty: &TypeExpr) -> Value {
    let want = optional_depth(ty);
    let mut out = value;
    for _ in option_depth(&out)..want {
        out = Value::Enum {
            name: "Option".to_string(),
            variant: "Some".to_string(),
            fields: vec![out],
            variant_index: 0,
            origin: None,
        };
    }
    out
}

/// How many optional layers a value already carries at its head.
pub(crate) fn option_depth(value: &Value) -> usize {
    match value {
        Value::Enum { name, fields, .. } if name == "Option" => {
            1 + fields.first().map_or(0, option_depth)
        }
        _ => 0,
    }
}

/// The error side of a result, one entry per arm of a union.
pub(super) fn err_arms(err: &TypeExpr) -> Vec<TypeExpr> {
    match err {
        TypeExpr::Union(arms) => arms.clone(),
        one => vec![one.clone()],
    }
}

impl Interpreter {
    /// ER9 branch selection where the error side is erased.
    ///
    /// A name like `any Error` names no runtime type, so the plain name compare
    /// below never matched it and a directly returned concrete error was
    /// wrapped as the *success* branch: `return StoreError.Missing(k)` from an
    /// `i64 or any Error` came back to the caller as an ok value holding the
    /// error, and `catch` never fired (#708).
    ///
    /// An interface object matches when the value's type provides the interface's
    /// methods. ER4 already restricts an error side to `Error`, so the
    /// compiler-provided method lists cover every case that can legally appear
    /// here — a user interface can't be an error type on its own.
    fn value_matches_any_err(&self, value: &Value, names: &[TypeExpr]) -> bool {
        if value_matches_any_type(value, names) {
            return true;
        }
        let type_name = match value {
            Value::Enum { name, .. } => name.clone(),
            Value::Struct(s) => s.lock().unwrap().name.clone(),
            _ => return false,
        };
        names.iter().any(|n| {
            // `Error` and `any Error` are one type written two ways (#1095).
            // Only the long spelling matched, so an `i64 or Error` function
            // returning a concrete error handed it back as the *ok* branch —
            // the same #708 bug, in the spelling most of the corpus uses.
            let interface_name = match n {
                TypeExpr::Any(t) => t.to_string(),
                _ if n.is_name("Error") => "Error".to_string(),
                _ => return false,
            };
            let required = rask_types::builtin_interface_method_names(&interface_name);
            if required.is_empty() {
                return false;
            }
            let provided = self.methods.get(&type_name);
            required
                .iter()
                .all(|m| provided.is_some_and(|ms| ms.contains_key(m)))
        })
    }
}

/// Does the runtime value match any of the named types?
fn value_matches_any_type(value: &Value, names: &[TypeExpr]) -> bool {
    if names.is_empty() {
        return false;
    }
    // "none" in the error type names matches the interpreter's `none` runtime value,
    // which is represented as Option.None (from ExprKind::None).
    if names.iter().any(|n| *n == TypeExpr::NoneType) {
        if matches!(value, Value::Enum { name, variant, .. } if name == "Option" && variant == "None") {
            return true;
        }
    }
    let value_type_name: Option<&str> = match value {
        Value::Enum { name, .. } => Some(name.as_str()),
        Value::Struct(s) => {
            let guard = s.lock().unwrap();
            if names.iter().any(|n| same_nominal(n, &guard.name)) {
                return true;
            }
            None
        }
        _ => None,
    };
    if let Some(vn) = value_type_name {
        names.iter().any(|n| same_nominal(n, vn))
    } else {
        false
    }
}

/// Do these two spellings name the same nominal type?
///
/// The declared error type carries the type arguments the source wrote —
/// `Refused<i64>` — and a runtime value's name is the bare one. Exact equality
/// therefore missed for every generic error type, so `return Refused.Full(n)`
/// from a `-> i64 or Refused<i64>` was wrapped as `Result.Ok(…)`: the wrong
/// side. `catch` then never fired and the error came back as the value.
///
/// Native had the same bug in its own spelling — `is Refused<i64>` compared
/// against a layout named `Refused` and routed to the success arm — so
/// `Vec.try_push`, declared `void or GrowError<T>`, read backwards on both
/// backends in different ways.
pub(super) fn same_nominal(written: &TypeExpr, runtime_name: &str) -> bool {
    written.last_segment() == runtime_name.rsplit('.').next()
}

