// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Function calling and ensure blocks.

use rask_ast::decl::FnDecl;
use rask_ast::expr::ExprKind;
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::Span;
use std::collections::HashSet;

use crate::value::Value;

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
    pub(crate) fn call_function(&mut self, func: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeDiagnostic> {
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
            return crate::grow_interp_stack(move || self.call_counted(func, args));
        }
        self.call_counted(func, args)
    }

    fn call_counted(&mut self, func: &FnDecl, args: Vec<Value>) -> Result<Value, RuntimeDiagnostic> {
        self.call_depth += 1;
        let result = self.call_function_at_depth(func, args);
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

    fn call_function_at_depth(&mut self, func: &FnDecl, mut args: Vec<Value>) -> Result<Value, RuntimeDiagnostic> {
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

        // What this call's type parameters resolved to, read off the arguments:
        // `value: T` given a `Point` means `T = "Point"` for the body. Scoped to
        // the call, like `env`. Without it `reflect.fields<T>()` inside a generic
        // body saw the literal "T" (#699).
        //
        // PC1 makes a single uppercase letter a type parameter wherever it
        // appears, so `func print_fields(value: T)` declares one without writing
        // `<T>` — reading only `type_params` found nothing to bind.
        let mut type_frame: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();

        // Written type arguments bind positionally against the declaration's
        // own list: `count<Plain>()` on `func count<T>()` means `T = "Plain"`.
        // Taken, so it can't leak into a later call, and applied before the
        // argument-derived bindings below — which use `or_insert`, so an
        // inferred value can still fill a parameter this didn't name.
        if let Some(written) = self.pending_type_args.take() {
            for (tp, concrete) in func
                .type_params
                .iter()
                .filter(|tp| !tp.is_comptime)
                .zip(written)
            {
                type_frame.insert(tp.name.clone(), concrete);
            }
        }

        for (idx, param) in func.params.iter().enumerate() {
            let declared = param.ty.trim();
            let named_here = func
                .type_params
                .iter()
                .any(|tp| !tp.is_comptime && tp.name == declared);
            if !(named_here || is_type_param_name(declared)) {
                continue;
            }
            if let Some(concrete) = args.get(idx).and_then(Self::runtime_type_name) {
                type_frame.entry(declared.to_string()).or_insert(concrete);
            }
        }
        self.type_bindings.push(type_frame);

        for (param, arg) in func.params.iter().zip(args.into_iter()) {
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
            let arg = wrap_optional_layers(arg, &param.ty);
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

        if let Err(msg) = self.resource_tracker.check_scope_exit(scope_depth) {
            let guard_diag = RuntimeDiagnostic::new(RuntimeError::Panic(msg), Span::new(0, 0));
            // E3: a guard (R5/H1) firing while the body is already failing is a
            // secondary panic — contained and reported, not a replacement for
            // the original.
            //
            // Any failure, not just a panic. A body that dies on `no method
            // seek on type File` leaves its resource unconsumed *because* it
            // died, so replacing the error with "resource leak: File 'f' not
            // consumed" hides the only line that says what went wrong — and
            // points at the import instead of the call. Return, break,
            // continue and `try` are control flow rather than failure, and a
            // resource leaked on the way out through one of those is the real
            // problem, so those still lose to the guard.
            let body_failed = matches!(
                &result,
                Err(diag) if !matches!(
                    diag.error,
                    RuntimeError::Return(_)
                        | RuntimeError::TryError(_)
                        | RuntimeError::Break(_, _)
                        | RuntimeError::Continue(_)
                )
            );
            if body_failed {
                self.report_secondary_panic(&guard_diag);
            } else {
                self.type_bindings.pop();
                self.env.pop_scope();
                return Err(guard_diag);
            }
        }

        // mem.parameters/PM2: snapshot the final values of `mutate` params before
        // the scope is dropped, so the call site can write each back to its
        // argument place. Keyed by parameter index (self is param 0 for methods).
        self.mutate_writebacks = func.params.iter().enumerate()
            .filter(|(_, p)| p.is_mutate)
            .filter_map(|(i, p)| self.env.get(&p.name).map(|v| (i, v.clone())))
            .collect();

        self.type_bindings.pop();
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
        // Both spellings. `wrap_optional_layers` has always understood
        // `Option<T>` as well as `T?` — this gate didn't, so a function
        // declared the long way handed back a bare `T` and the caller's `!`
        // said "requires Option or Result, got i64". `get_clone` in
        // `stdlib/collections.rk` is written that way, so every `get_clone` on
        // the interpreter was broken (#1211).
        let returns_option = func.ret_ty.as_ref()
            .map(|t| {
                let t = t.trim();
                t.ends_with('?') || (t.starts_with("Option<") && t.ends_with('>'))
            })
            .unwrap_or(false);
        if returns_result {
            // Already a Result: pass through.
            if matches!(&value, Value::Enum { name, .. } if name == "Result") {
                return Ok(value);
            }
            // ER9: pick the branch by the value's runtime type. If the value
            // matches E (or a variant of a union E), wrap as Err; else Ok.
            // Disjointness (ER3) makes this unambiguous.
            let err_names = func.ret_ty.as_ref()
                .map(|t| extract_result_err_names(t))
                .unwrap_or_default();
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
            let ok_ty = func.ret_ty.as_ref().and_then(|t| result_ok_type(t));
            let payload = match ok_ty {
                Some(ok) => wrap_optional_layers(value, &ok),
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
                None => Ok(wrap_optional_layers(value, "T?")),
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
                    Err(_) => {}
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

/// PC1: a single uppercase ASCII letter is a type parameter wherever it appears
/// in a signature, whether or not the function also writes `<T>`.
fn is_type_param_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!((chars.next(), chars.next()), (Some(c), None) if c.is_ascii_uppercase())
}

/// The T of a `Result<T, E>` string, as written.
fn result_ok_type(ret_ty: &str) -> Option<String> {
    rask_ast::type_str::result_parts(ret_ty).map(|(ok, _)| ok.to_string())
}

/// Add whatever optional layers `ty` asks for that `value` doesn't already
/// carry. `KV?` / `Option<KV>` want one; `KV??` wants two.
fn wrap_optional_layers(value: Value, ty: &str) -> Value {
    let ty = ty.trim();
    let want = if ty.ends_with('?') {
        ty.chars().rev().take_while(|c| *c == '?').count()
    } else if ty.starts_with("Option<") && ty.ends_with('>') {
        1
    } else {
        return value;
    };
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

/// The E type names of a `Result<T, E>`, one per arm of a union error.
///
/// The split is `rask_ast::type_str`'s. Three hand-written copies of it lived
/// in this crate, all counting the `>` of a function type's `->` as a closing
/// bracket — so `Result<(func(i64) -> i64), Oops>` had no top-level comma, this
/// answered with nothing, and `return Oops.Bad` was wrapped as the *success*
/// branch. The caller then bound the enum and reported "enum is not callable"
/// at the call site, while native ran it (#1244).
fn extract_result_err_names(ret_ty: &str) -> Vec<String> {
    let Some((_, err_str)) = rask_ast::type_str::result_parts(ret_ty) else {
        return Vec::new();
    };
    // `(E1 | E2)` — those parens belong to the union, not to a type.
    let err_str = err_str
        .strip_prefix('(').and_then(|s| s.strip_suffix(')'))
        .map(str::trim)
        .unwrap_or(err_str);
    rask_ast::type_str::split_all_top_level(err_str, '|')
        .into_iter()
        .map(str::to_string)
        .collect()
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
    /// A trait object matches when the value's type provides the trait's
    /// methods. ER4 already restricts an error side to `Error`, so the
    /// compiler-provided method lists cover every case that can legally appear
    /// here — a user trait can't be an error type on its own.
    fn value_matches_any_err(&self, value: &Value, names: &[String]) -> bool {
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
            let trait_name = match rask_ast::traits::trait_object_name(n) {
                Some(t) => t,
                None if rask_ast::traits::is_bare_error(n) => "Error",
                None => return false,
            };
            let required = rask_types::builtin_trait_method_names(trait_name);
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
fn value_matches_any_type(value: &Value, names: &[String]) -> bool {
    if names.is_empty() {
        return false;
    }
    // "none" in the error type names matches the interpreter's `none` runtime value,
    // which is represented as Option.None (from ExprKind::None).
    if names.iter().any(|n| n == "none") {
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
fn same_nominal(a: &str, b: &str) -> bool {
    fn base(n: &str) -> &str {
        let n = n.split('<').next().unwrap_or(n).trim();
        n.rsplit('.').next().unwrap_or(n).trim()
    }
    base(a) == base(b)
}

