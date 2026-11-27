// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Borrow checking, scope management, and ESAD (Expression-Scoped Access Discipline).

use std::collections::HashMap;

use rask_ast::expr::{CallArg, Expr, ExprKind};
use rask_ast::stmt::StmtKind;
use rask_ast::Span;

use rask_types::Type;

use super::errors::TypeError;
use rask_types::{TypeDef, SelfParam};
use super::TypeChecker;

// ============================================================================
// Borrow Tracking for Aliasing Detection
// ============================================================================

/// Borrow mode for active borrows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BorrowMode {
    Shared,    // Read-only borrow
    Exclusive, // Mutable borrow
}

/// An active borrow tracked during expression evaluation.
#[derive(Debug, Clone)]
pub(crate) struct ActiveBorrow {
    pub(crate) var_name: String,
    pub(crate) mode: BorrowMode,
    pub(crate) span: Span,
}

impl TypeChecker {
    // ------------------------------------------------------------------------
    // Scope Management
    // ------------------------------------------------------------------------

    pub(super) fn push_scope(&mut self) {
        self.local_types.push(HashMap::new());
        self.comptime_string_names.push(HashMap::new());
    }

    pub(super) fn pop_scope(&mut self) {
        self.local_types.pop();
        // Leave at least the module scope: `pop_scope` is called in a few
        // places that never pushed, and an empty stack has no innermost to
        // record a binding in.
        if self.comptime_string_names.len() > 1 {
            self.comptime_string_names.pop();
        }
    }

    /// Record whether `name` holds a string the compiler will know (CT53).
    pub(super) fn note_comptime_string(&mut self, name: &str, known: bool) {
        if let Some(scope) = self.comptime_string_names.last_mut() {
            scope.insert(name.to_string(), known);
        }
    }

    /// Innermost binding of `name` wins, the way a lookup does.
    pub(super) fn is_comptime_string(&self, name: &str) -> bool {
        self.comptime_string_names
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .copied()
            .unwrap_or(false)
    }

    pub(super) fn define_local(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::Mut));
        }
    }

    /// Define a default (read-only) parameter. Use `define_local` for `mutate`/`take` params.
    pub(super) fn define_local_param(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::Param));
        }
    }

    /// Define a `let` binding. Deep-immutable: rebinding, index/field assign,
    /// and mutating method calls all forbidden.
    pub(super) fn define_local_const(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::Let));
        }
    }

    /// Define a module-level `const`. One instance for the whole program.
    pub(super) fn define_local_module_const(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::ModuleConst));
        }
    }

    /// Define a read-only with-binding (`with expr as v`). Same deep
    /// immutability as `let`, but errors point at `as mut v` instead.
    pub(super) fn define_local_with_read(&mut self, name: String, ty: Type) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::WithRead));
        }
    }

    /// Define a name a test or a pattern introduced — a payload bind or a
    /// read-only `for` element. Read-only like `let`, and the source is kept so
    /// the mutation error can name the remedy that form actually has.
    pub(super) fn define_local_bound(
        &mut self,
        name: String,
        ty: Type,
        from: super::BoundFrom,
    ) {
        if let Some(scope) = self.local_types.last_mut() {
            scope.insert(name, (ty, super::BindingKind::Bound(from)));
        }
    }

    pub(super) fn lookup_local(&self, name: &str) -> Option<Type> {
        for scope in self.local_types.iter().rev() {
            if let Some((ty, _)) = scope.get(name) {
                return Some(ty.clone());
            }
        }
        None
    }

    /// Does `name` here mean a module const: no local shadows it?
    pub(super) fn names_module_const(&self, name: &str) -> bool {
        match self.local_types.iter().rev().find_map(|scope| scope.get(name)) {
            Some((_, kind)) => matches!(kind, super::BindingKind::ModuleConst),
            None => true,
        }
    }

    /// Depth of the innermost scope defining `name` — 1 is the outermost.
    pub(super) fn local_depth(&self, name: &str) -> Option<usize> {
        self.local_types
            .iter()
            .rposition(|scope| scope.contains_key(name))
            .map(|i| i + 1)
    }

    /// Look up the binding kind for a local.
    pub(super) fn lookup_binding_kind(&self, name: &str) -> Option<super::BindingKind> {
        for scope in self.local_types.iter().rev() {
            if let Some((_, kind)) = scope.get(name) {
                return Some(*kind);
            }
        }
        None
    }

    /// Extract the root identifier name from an assignment target expression.
    pub(super) fn root_ident_name(expr: &Expr) -> Option<String> {
        match &expr.kind {
            ExprKind::Ident(name) => Some(name.clone()),
            ExprKind::Field { object, .. } => Self::root_ident_name(object),
            ExprKind::Index { object, .. } => Self::root_ident_name(object),
            _ => None,
        }
    }

    // ------------------------------------------------------------------------
    // Borrow Stack Management (ESAD Phase 1)
    // ------------------------------------------------------------------------

    /// Push a borrow onto the stack.
    pub(super) fn push_borrow(&mut self, var_name: String, mode: BorrowMode, span: Span) {
        self.borrow_stack.push(ActiveBorrow { var_name, mode, span });
    }

    /// Pop all borrows from the current expression (called at statement end).
    pub(super) fn clear_expression_borrows(&mut self) {
        self.borrow_stack.clear();
    }

    /// Check if accessing a variable would conflict with active borrows.
    /// Returns the conflicting borrow if found.
    pub(super) fn check_borrow_conflict(&self, var_name: &str, access_mode: BorrowMode) -> Option<&ActiveBorrow> {
        for borrow in self.borrow_stack.iter().rev() {
            if borrow.var_name == var_name {
                // Check conflict rules from ESAD spec
                match (borrow.mode, access_mode) {
                    (BorrowMode::Shared, BorrowMode::Shared) => {
                        // Shared + Shared = OK
                        continue;
                    }
                    (BorrowMode::Shared, BorrowMode::Exclusive) |
                    (BorrowMode::Exclusive, BorrowMode::Shared) |
                    (BorrowMode::Exclusive, BorrowMode::Exclusive) => {
                        // Any combination with Exclusive = ERROR
                        return Some(borrow);
                    }
                }
            }
        }
        None
    }

    /// Arguments are lent to a call as they're written, a method's receiver
    /// first (mem.aliasing), and the call holds each until it runs. A later
    /// argument that changes one of them, through a `mutate` or `deleting`
    /// argument of its own or a method that changes its receiver, conflicts
    /// with that loan: `pair(x, bump(mutate x))`, `pair(c.n, c.bump())`.
    ///
    /// Reading it is fine, since the reads all happen before the call does:
    /// `v.push(v.len())` lends `v` to `push` and reads it on the way.
    ///
    /// Allowed, the answer depended on the backend, which read the first
    /// argument either before the second ran or at the call (#1534). Checked
    /// as written, before any labels place the arguments.
    pub(super) fn check_argument_lending(&mut self, callee: &str, receiver: Option<&Expr>, args: &[CallArg]) {
        let mut lent: Vec<Vec<String>> = receiver.and_then(Self::access_path).into_iter().collect();
        for arg in args {
            for (path, span) in self.places_changed_inside(&arg.expr) {
                if let Some(held) = lent.iter().find(|held| super::check_expr::paths_overlap(held, &path)) {
                    self.errors.push(TypeError::ArgumentChangedLater {
                        callee: callee.to_string(),
                        place: super::check_expr::path_text(held),
                        span,
                    });
                    return;
                }
            }
            if let Some(path) = Self::access_path(&arg.expr) {
                lent.push(path);
            }
        }
    }

    /// The places a call inside `e` changes. A closure's body runs later, if
    /// at all, so it isn't looked into.
    fn places_changed_inside(&self, e: &Expr) -> Vec<(Vec<String>, Span)> {
        use rask_ast::expr::ArgMode;
        let mut out = Vec::new();
        rask_ast::visit::walk_expr_pruned(e, &mut |inner| {
            match &inner.kind {
                ExprKind::Call { args, .. } | ExprKind::MethodCall { args, .. } => {
                    for a in args {
                        if matches!(a.mode, ArgMode::Mutate | ArgMode::Deleting) {
                            if let Some(path) = Self::access_path(&a.expr) {
                                out.push((path, a.expr.span));
                            }
                        }
                    }
                }
                _ => {}
            }
            // A method that changes its receiver. Only once the receiver's
            // type is known: an open one answers by method name across every
            // type, and `close` would count.
            if let ExprKind::MethodCall { object, method, .. } = &inner.kind {
                if let Some(var) = object.name() {
                    let settled = self
                        .lookup_local(var)
                        .is_some_and(|t| !matches!(self.resolve_named(&self.ctx.apply(&t)), Type::Var(_)));
                    if settled && self.method_mutates_self(var, method) {
                        out.push((vec![var.to_string()], object.span));
                    }
                }
            }
            !matches!(inner.kind, ExprKind::Closure { .. })
        });
        out
    }

    /// Scan a closure body for variable accesses and check for conflicts.
    /// This implements ESAD Phase 2.
    pub(super) fn check_closure_aliasing(&mut self, params: &[rask_ast::expr::ClosureParam], body: &Expr) {
        let param_names: std::collections::HashSet<&str> = params.iter().map(|p| p.name.as_str()).collect();
        self.collect_closure_accesses(body, &param_names);
    }

    /// Recursively collect variable accesses in a closure body.
    /// Skip closure params — they're fresh bindings, not captures.
    pub(super) fn collect_closure_accesses(&mut self, expr: &Expr, skip: &std::collections::HashSet<&str>) {
        match &expr.kind {
            ExprKind::Ident(name) => {
                if skip.contains(name.as_str()) { return; }
                if let Some(borrow) = self.check_borrow_conflict(name, BorrowMode::Shared) {
                    self.errors.push(TypeError::AliasingViolation {
                        var: name.clone(),
                        borrow_span: borrow.span,
                        access_span: expr.span,
                    });
                }
            }
            ExprKind::MethodCall { object, method, args, .. } => {
                if let Some(name) = object.name() {
                    if !skip.contains(name) {
                        // `x + 1` is `x.add(1)`, a read; only a method that
                        // changes or consumes `x` collides with a read loan.
                        let access = self.method_borrow_mode(name, method);
                        if let Some(borrow) = self.check_borrow_conflict(name, access) {
                            self.errors.push(TypeError::AliasingViolation {
                                var: name.to_string(),
                                borrow_span: borrow.span,
                                access_span: object.span,
                            });
                        }
                    }
                }
                for arg in args {
                    self.collect_closure_accesses(&arg.expr, skip);
                }
            }
            ExprKind::Call { func, args } => {
                self.collect_closure_accesses(func, skip);
                for arg in args {
                    self.collect_closure_accesses(&arg.expr, skip);
                }
            }
            ExprKind::Block(stmts) => {
                for stmt in stmts {
                    if let StmtKind::Expr(e) = &stmt.kind {
                        self.collect_closure_accesses(e, skip);
                    }
                }
            }
            _ => {}
        }
    }

    pub(super) fn guard_escape_type_name(&self, ty: &Type) -> Option<String> {
        let resolved = self.ctx.apply(ty);
        let type_id = match &resolved {
            Type::Named(id) => Some(*id),
            Type::Generic { base, .. } => Some(*base),
            _ => None,
        }?;
        match self.types.get(type_id) {
            Some(TypeDef::Struct { .. }) | Some(TypeDef::Enum { .. }) | Some(TypeDef::Union { .. }) => {
                Some(self.types.type_name(type_id))
            }
            _ => None,
        }
    }

    /// True if the method mutates its receiver (`mutate self`). `take self`
    /// consumes — not mutates — and is allowed on const bindings.
    /// Falls back to the union of mutating method names across stdlib stubs
    /// when the receiver's type isn't resolved yet (catches
    /// `const v = Vec.new(); v.push(1)` before constraint solving runs).
    pub(super) fn method_mutates_self(&self, var_name: &str, method_name: &str) -> bool {
        let Some(ty) = self.lookup_local(var_name) else { return false };
        self.method_mutates_self_ty(&ty, method_name)
    }

    /// The same question asked of a type rather than a binding, so the deferred
    /// check in `validate_pending_self_mutations` can re-ask it once the
    /// receiver has actually been solved (#928).
    pub(super) fn method_mutates_self_ty(&self, ty: &Type, method_name: &str) -> bool {
        match self.receiver_self_param(ty, method_name) {
            Some(mode) => matches!(mode, SelfParam::Mutate),
            None => Self::unresolved_receiver_mutates(method_name),
        }
    }

    /// How a method takes its receiver, or `None` while the receiver's type is
    /// still open.
    fn receiver_self_param(&self, ty: &Type, method_name: &str) -> Option<SelfParam> {
        let resolved = self.resolve_named(&self.ctx.apply(ty));

        // A receiver whose type already failed has had its error. Treating it
        // as open asked "is this name `mutate self` anywhere", and `Set.add`
        // is: after a rejected `let n = s.parse<i64>() ?? 0`, `n + 1` (which
        // is `n.add(1)`) reported "cannot mutate `n`" too.
        if matches!(resolved, Type::Error) {
            return Some(SelfParam::Value);
        }

        // A box mutates through its heap slot, not through the binding
        // (`conc.sync/R2`) — `let c = Shared.new(0)`
        // followed by `c.write()` is the point of the type, not a mistake. This
        // has to be said explicitly because `write` *is* `mutate self` on
        // other stdlib types, and an unresolved receiver falls through to the
        // union of every mutating method name.
        if self.is_shared_box(&resolved)
            && matches!(method_name,
                "read" | "write" | "try_read" | "try_write" | "get" | "set" | "replace")
        {
            return Some(SelfParam::Value);
        }

        // `any Interface` values: the interface's own declared self mode is known.
        if let Type::InterfaceObject { interface_name, decl } = &resolved {
            let methods = crate::interfaces::InterfaceChecker::new(&self.types).interface_object_methods(interface_name, *decl);
            return Some(
                methods
                    .iter()
                    .find(|m| m.name == method_name)
                    .map_or(SelfParam::Value, |sig| sig.self_param),
            );
        }

        let stub_mode = |stub: &rask_stdlib::MethodStub| {
            if stub.mutate_self {
                SelfParam::Mutate
            } else if stub.take_self {
                SelfParam::Take
            } else {
                SelfParam::Value
            }
        };

        let type_id = match &resolved {
            Type::Named(id) => Some(*id),
            Type::Generic { base, .. } => Some(*base),
            _ => None,
        };

        if let Some(id) = type_id {
            // User-defined types: check the internal method table.
            let methods = match self.types.get(id) {
                Some(TypeDef::Struct { methods, .. }) |
                Some(TypeDef::Enum { methods, .. }) => Some(methods),
                _ => None,
            };
            if let Some(methods) = methods {
                if let Some(sig) = methods.iter().find(|m| m.name == method_name) {
                    return Some(sig.self_param);
                }
            }

            // Builtin types (Vec, Map, string, ...): check the stdlib stubs.
            let type_name = self.types.type_name(id);
            return Some(
                rask_stdlib::lookup_method(&type_name, method_name).map_or(SelfParam::Value, stub_mode),
            );
        }

        // A primitive receiver is `Type::String` / `Type::I64` / … rather than
        // a `Named`, so the stub lookup above never ran for one. `Vec.reverse`
        // is `mutate self`, so `let s = "abc"` then `s.reverse()` — which takes
        // `self` by value and returns a new string — was rejected as mutating
        // a `let` (#839).
        if let Some(name) = super::type_defs::receiver_name(&resolved, &self.types) {
            if let Some(stub) = rask_stdlib::lookup_method(&name, method_name) {
                return Some(stub_mode(stub));
            }
            // A primitive with no such stub has no mutating methods at all.
            if !matches!(resolved, Type::Var(_)) {
                return Some(SelfParam::Value);
            }
        }
        None
    }

    /// Receiver type unresolved: the set of method names that are `mutate
    /// self` across all stdlib stubs. `add`/`mul`/`eq` aren't in the set, so
    /// desugared arithmetic/comparison don't false-positive.
    ///
    /// `read` and `write` are held out. They are the sync box's two verbs
    /// (conc.sync/SH5) and a box mutates through its slot, not its binding —
    /// but `Writer.write` is `mutate self`, so the union claims the name and
    /// `let c = Shared.mutex(0)` then `with c.write() as v` was rejected for
    /// mutating a `let`. Holding them out only affects an *unresolved*
    /// receiver: a real `Writer` answers from its stub, and the one stdlib
    /// `write` that mutates takes an argument, which no box verb does.
    fn unresolved_receiver_mutates(method_name: &str) -> bool {
        !matches!(method_name, "read" | "write") && rask_stdlib::any_builtin_method_mutates(method_name)
    }

    /// Is this the `Shared<T, S>` box, however the checker spells it?
    pub(super) fn is_shared_box(&self, ty: &Type) -> bool {
        match ty {
            Type::Generic { base, .. } => self.types.type_name(*base) == "Shared",
            Type::UnresolvedGeneric { name, .. } | Type::UnresolvedNamed(name) => name == "Shared",
            _ => false,
        }
    }

    /// The borrow a method call holds on its receiver: exclusive when the
    /// method changes or consumes it, shared when it reads it. Asked of the
    /// same signature the const-mutation check reads; guessing from the
    /// method's name made `p > 0` on an `i64` (`p.gt(0)`) an exclusive borrow,
    /// and a closure reading `p` in the `if` body a conflict.
    pub(super) fn method_borrow_mode(&self, var_name: &str, method_name: &str) -> BorrowMode {
        let mode = self
            .lookup_local(var_name)
            .and_then(|ty| self.receiver_self_param(&ty, method_name));
        let exclusive = match mode {
            Some(m) => matches!(m, SelfParam::Mutate | SelfParam::Take),
            None => Self::unresolved_receiver_mutates(method_name),
        };
        if exclusive { BorrowMode::Exclusive } else { BorrowMode::Shared }
    }
}
