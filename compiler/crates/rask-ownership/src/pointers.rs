// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Where raw pointers point (mem.unsafe/PT1–PT4).
//!
//! `unsafe` gives up one guarantee: that a dereference lands on live memory of
//! the right type. It doesn't give up the compiler knowing where a pointer came
//! from. A pointer is made by a call, and a call that answers a pointer answers
//! one into the arguments it borrows: `xs.as_ptr()` points into `xs`. So each
//! binding that holds a pointer carries the names of the values in this body
//! whose storage it points into, the same way a binding carries a closure's
//! frame limit. From that, the mistakes the compiler can see are errors:
//!
//! - PT2: the pointer leaves for somewhere that outlives what it points into
//!   (a return, a caller's field, a task).
//! - PT3: what it points into changes size, moves, is replaced or goes out of
//!   scope, and the pointer is used afterwards.
//! - PT4: a write through it lands on a place a `with` or `for mutate` holds.
//!
//! A pointer from `alloc`, a C function or an integer points into nothing here
//! and carries nothing. What's behind it is the `unsafe` block's to vouch for.

use std::collections::{HashMap, HashSet};

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::visit::{self, Visit};
use rask_ast::Span;
use rask_types::Type;
use rask_ast::ty::ParamMode;

use crate::error::{OwnershipError, OwnershipErrorKind, PointerLoss, PointerSink, Sink};
use crate::state::BindingState;
use crate::OwnershipChecker;

impl<'a> OwnershipChecker<'a> {
    /// Can a value of this type hold a raw pointer?
    pub(crate) fn type_holds_pointer(&self, ty: &Type) -> bool {
        self.type_holds(ty, &|t| matches!(t, Type::RawPtr(_)), &mut Vec::new())
    }

    /// A value with no storage of its own to point into: a number, a flag, a
    /// pointer, a function.
    pub(crate) fn has_no_storage(ty: &Type) -> bool {
        matches!(
            ty,
            Type::Unit | Type::Never | Type::Bool | Type::Char
                | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
                | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
                | Type::F32 | Type::F64
                | Type::RawPtr(_) | Type::Fn { .. }
        )
    }

    /// The value whose storage `name` is: the name itself, or the collection a
    /// loop item or `with` element was lent from.
    pub(crate) fn pointee_root(&self, name: &str) -> String {
        let mut cur = name;
        for _ in 0..32 {
            match self.ptr_lenders.get(cur) {
                Some(next) if next != cur => cur = next,
                _ => break,
            }
        }
        cur.to_string()
    }

    /// PT1: the values in this body that `expr`'s pointers point into.
    pub(crate) fn pointer_sources(&self, expr: &Expr) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_pointer_sources(expr, &mut out);
        out.sort();
        out.dedup();
        out
    }

    fn collect_pointer_sources(&self, expr: &Expr, out: &mut Vec<String>) {
        let mut values = Vec::new();
        self.values_reaching(expr, true, &mut values);
        for value in values {
            match &value.kind {
                ExprKind::Ident(name) => {
                    if let Some(s) = self.ptr_sources.get(name) {
                        out.extend(s.iter().cloned());
                    }
                }
                // A closure carries what it captured, pointers included.
                ExprKind::Closure { params, body, .. } => {
                    let locals: std::collections::HashSet<String> =
                        params.iter().map(|p| p.name.clone()).collect();
                    let mut captures = Vec::new();
                    self.collect_free_vars(body, &locals, &mut captures);
                    for name in captures {
                        if let Some(s) = self.ptr_sources.get(&name) {
                            out.extend(s.iter().cloned());
                        }
                    }
                }
                ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                    if !self.answers_pointer(value) {
                        continue;
                    }
                    for arg in self.pointer_call_args(value) {
                        self.collect_arg_pointee(arg, out);
                    }
                }
                _ => {
                    // A part read out of a binding that holds pointers. Out of
                    // a `@resource`, the pointer points into what it owns (UT4).
                    let Some(root) = Self::projection_root(value) else { continue };
                    let holds = self.node_ty(&value.id).is_some_and(|t| self.type_holds_pointer(t));
                    if holds {
                        if let Some(s) = self.ptr_sources.get(root) {
                            out.extend(s.iter().cloned());
                        }
                        let owner = self
                            .binding_types
                            .get(root)
                            .is_some_and(|t| self.program.types.is_linear_value(t));
                        if owner {
                            out.push(self.pointee_root(root));
                        }
                    }
                }
            }
        }
    }

    fn answers_pointer(&self, call: &Expr) -> bool {
        self.node_ty(&call.id).is_some_and(|t| self.type_holds_pointer(t))
    }

    /// What one argument of a call that answers a pointer can be pointed into
    /// through: the pointers it already holds, or its own storage when it is a
    /// place with any. A temporary (`"{a}".as_ptr()`) has no name to track;
    /// native keeps it alive while the pointer is in use (PT5).
    fn collect_arg_pointee(&self, arg: &Expr, out: &mut Vec<String>) {
        let Some(ty) = self.node_ty(&arg.id) else { return };
        if self.type_holds_pointer(ty) {
            self.collect_pointer_sources(arg, out);
            // UT4: a `@resource` owns what its pointers point at, so a pointer
            // made from one points into the resource itself.
            if !self.program.types.is_linear_value(ty) {
                return;
            }
        }
        if Self::has_no_storage(ty) {
            return;
        }
        let root = match &arg.kind {
            ExprKind::Ident(name) => Some(name.as_str()),
            _ => Self::projection_root(arg),
        };
        if let Some(root) = root {
            out.push(self.pointee_root(root));
        }
    }

    /// PT1: names bound from `from` point wherever `from`'s pointers do.
    /// Recomputed, not accumulated: a name rebound points at what it holds now.
    pub(crate) fn inherit_pointer_sources(&mut self, names: &[String], from: &Expr) {
        let sources = self.pointer_sources(from);
        for name in names {
            let holds = self
                .binding_types
                .get(name)
                .cloned()
                .is_some_and(|t| self.type_holds_pointer(&t));
            if holds && !sources.is_empty() {
                self.ptr_sources.insert(name.clone(), sources.clone());
            } else {
                self.ptr_sources.remove(name);
            }
        }
    }

    /// A pointer put into part of `root` (`h.p = …`, `ptrs.push(…)`): `root`
    /// now points wherever it does, as well as wherever it already did.
    /// A parameter is left out: what goes into one leaves the frame, and PT2
    /// has already judged that at the store.
    pub(crate) fn add_pointer_sources(&mut self, root: &str, from: &Expr) {
        if self.param_types.contains_key(root) {
            return;
        }
        let sources = self.pointer_sources(from);
        if sources.is_empty() {
            return;
        }
        let entry = self.ptr_sources.entry(root.to_string()).or_default();
        for s in sources {
            if !entry.contains(&s) {
                entry.push(s);
            }
        }
    }

    /// A name lent out of a collection: a pointer into it is a pointer into
    /// the collection, which outlives the loop or `with` that lent it.
    pub(crate) fn record_pointer_lender(&mut self, name: &str, collection: &Expr) {
        let place = match &collection.kind {
            ExprKind::MethodCall { object, .. } => object.as_ref(),
            _ => collection,
        };
        let root = match &place.kind {
            ExprKind::Ident(n) => Some(n.as_str()),
            _ => Self::projection_root(place),
        };
        match root {
            Some(r) if r != name => {
                self.ptr_lenders.insert(name.to_string(), r.to_string());
            }
            _ => {
                self.ptr_lenders.remove(name);
            }
        }
    }

    /// A loop item: lent from the collection when the loop borrows it, and
    /// pointing wherever the collection's pointers do when it holds some.
    pub(crate) fn record_loop_item(&mut self, names: &[String], iter: &Expr, lends: bool) {
        let place = match &iter.kind {
            ExprKind::MethodCall { object, .. } => object.as_ref(),
            _ => iter,
        };
        let root = match &place.kind {
            ExprKind::Ident(n) => Some(n.clone()),
            _ => Self::projection_root(place).map(str::to_string),
        };
        let sources = root.as_ref().and_then(|r| self.ptr_sources.get(r)).cloned();
        for name in names {
            if lends {
                self.record_pointer_lender(name, iter);
            } else {
                self.ptr_lenders.remove(name);
            }
            match &sources {
                Some(s) => {
                    self.ptr_sources.insert(name.clone(), s.clone());
                }
                None => {
                    self.ptr_sources.remove(name);
                }
            }
        }
    }

    /// PT3: every pointer into `pointee` is dead from here. With `outliving`,
    /// only those held by a name declared outside that block depth: the end of
    /// a block kills nothing that dies with it.
    pub(crate) fn kill_pointers_into(
        &mut self,
        pointee: &str,
        loss: PointerLoss,
        span: Span,
        outliving: Option<u32>,
    ) {
        let pointee = self.pointee_root(pointee);
        let victims: Vec<String> = self
            .ptr_sources
            .iter()
            .filter(|(name, sources)| {
                sources.contains(&pointee)
                    && matches!(self.bindings.get(name.as_str()), Some(BindingState::Owned))
                    && outliving.is_none_or(|inner| {
                        self.binding_decl_blocks.get(name.as_str()).is_none_or(|&d| d < inner)
                    })
            })
            .map(|(name, _)| name.clone())
            .collect();
        if victims.is_empty() {
            return;
        }
        for name in victims {
            self.bindings.insert(name, BindingState::PointerDead { at: span });
        }
        self.ptr_deaths.entry(span).or_default().push((pointee, loss));
    }

    /// The end of a block at depth `inner`: what was declared in it is gone,
    /// and so is every pointer into it held outside.
    pub(crate) fn kill_pointers_at_block_exit(&mut self, inner: u32, span: Span) {
        let dying: Vec<String> = self
            .binding_decl_blocks
            .iter()
            .filter(|(name, &d)| d >= inner && !self.ptr_lenders.contains_key(name.as_str()))
            .map(|(name, _)| name.clone())
            .collect();
        for name in dying {
            self.kill_pointers_into(&name, PointerLoss::OutOfScope, span, Some(inner));
        }
    }

    /// PT3 for a binding the walk marked gone at `at`: the error, if it was a
    /// pointer whose pointee went then.
    pub(crate) fn pointer_death(&self, name: &str, at: Span) -> Option<OwnershipErrorKind> {
        let sources = self.ptr_sources.get(name)?;
        let (pointee, loss) = self
            .ptr_deaths
            .get(&at)?
            .iter()
            .find(|(p, _)| sources.contains(p))?;
        Some(OwnershipErrorKind::PointerDangles {
            name: name.to_string(),
            pointee: pointee.clone(),
            loss: loss.clone(),
            lost_at: at,
        })
    }

    /// PT3 at a use of a live `name`: anything it points into that has since
    /// been moved or consumed. Asked at the use rather than marked at the
    /// move, so every way a value can go is covered by the one state it ends
    /// up in.
    pub(crate) fn check_pointer_use(&mut self, name: &str, span: Span) {
        let Some(sources) = self.ptr_sources.get(name) else { return };
        for pointee in sources.clone() {
            let Some(state) = self.bindings.get(&pointee) else { continue };
            if Self::is_available(state) {
                continue;
            }
            let Some(at) = Self::unavailable_span(state) else { continue };
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::PointerDangles {
                    name: name.to_string(),
                    pointee,
                    loss: PointerLoss::Moved,
                    lost_at: at,
                },
                span,
            });
            return;
        }
    }

    /// PT2: a pointer into this frame's own values may not go anywhere that
    /// outlives the frame. Into a borrowed parameter it may: what it points
    /// into is the caller's.
    pub(crate) fn check_pointer_escape(&mut self, expr: &Expr, into: &Sink) -> bool {
        let sink = match into {
            Sink::Return => PointerSink::Return,
            Sink::Place { place, root }
                if self.param_types.contains_key(root)
                    && (place != root || self.mutate_params.contains_key(root)) =>
            {
                PointerSink::Place(place.clone())
            }
            _ => return false,
        };
        self.report_owned_pointee(expr, sink, expr.span)
    }

    /// PT2 for a task: a pointer into anything of this frame's, a borrowed
    /// parameter's included, can't cross. The task may outlive the call.
    pub(crate) fn check_pointer_sent(&mut self, name: &str, span: Span) {
        let Some(pointee) = self.ptr_sources.get(name).and_then(|s| s.first()).cloned() else {
            return;
        };
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::PointerEscapes {
                pointee,
                into: PointerSink::Task(name.to_string()),
            },
            span,
        });
    }

    fn report_owned_pointee(&mut self, expr: &Expr, sink: PointerSink, span: Span) -> bool {
        let owned = self.pointer_sources(expr).into_iter().find(|s| {
            !self.param_types.contains_key(s) || self.taken_params.contains(s)
        });
        let Some(pointee) = owned else { return false };
        self.errors.push(OwnershipError {
            kind: OwnershipErrorKind::PointerEscapes { pointee, into: sink },
            span,
        });
        true
    }

    /// PT4: a write through `ptr`, where something it points into has a place
    /// held by a `with` or a `for mutate`.
    pub(crate) fn check_pointer_write(&mut self, ptr: &Expr, span: Span) {
        let sources = self.pointer_sources(ptr);
        if sources.is_empty() {
            return;
        }
        let held_by_with = self
            .active_with_bindings
            .iter()
            .find(|w| sources.contains(&self.pointee_root(&w.root)))
            .map(|w| (w.collection_name.clone(), w.span, false));
        let held = held_by_with.or_else(|| {
            self.active_for_mutates
                .iter()
                .find(|f| sources.contains(&self.pointee_root(&f.root)))
                .map(|f| (f.collection_name.clone(), f.span, true))
        });
        if let Some((collection, holder_span, in_loop)) = held {
            self.errors.push(OwnershipError {
                kind: OwnershipErrorKind::PointerWriteUnderView {
                    pointee: collection,
                    holder_span,
                    in_loop,
                },
                span,
            });
        }
    }

    /// The pointer a write goes through, if `target` is one: `*p = …`.
    pub(crate) fn written_pointer(target: &Expr) -> Option<&Expr> {
        match &target.kind {
            ExprKind::Unary { op: UnaryOp::Deref, operand } => Some(operand),
            _ => None,
        }
    }
}

// ─── Where a function's returned pointer can point ─────────────────────────

/// One place a pointer a function returns can point into, by the position of
/// the parameter it was handed in (the receiver counts as a parameter).
type ParamOrigins = HashSet<usize>;

impl<'a> OwnershipChecker<'a> {
    /// PT1, inferred: for every function and method with a body, which of its
    /// parameters the pointers it returns can point into. Computed to a fixed
    /// point over the calls between bodies, starting from "none", so recursion
    /// settles on what some path actually returns. A body this can't see (a
    /// native or builtin, an interface method, a closure value) keeps the
    /// signature rule at the call: every argument it borrows.
    pub(crate) fn infer_pointer_returns(&mut self, groups: &[&[Decl]]) {
        let mut bodies: Vec<(Option<String>, &FnDecl)> = Vec::new();
        for decls in groups {
            for decl in decls.iter() {
                match &decl.kind {
                    DeclKind::Fn(f) if !f.body_lives_elsewhere() => bodies.push((None, f)),
                    DeclKind::Impl(i) => {
                        let ty = i.target_ty.name().unwrap_or_default();
                        for m in i.methods.iter().filter(|m| !m.body_lives_elsewhere()) {
                            bodies.push((Some(ty.clone()), m));
                        }
                    }
                    _ => {}
                }
            }
        }
        loop {
            let mut changed = false;
            for (owner, f) in &bodies {
                let origins = self.returned_param_origins(f);
                let flags: Vec<bool> = (0..f.params.len()).map(|i| origins.contains(&i)).collect();
                let key = (owner.clone(), f.name.clone());
                if self.pointer_returns.get(&key) != Some(&flags) {
                    self.pointer_returns.insert(key, flags);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// The parameters `f`'s returned pointers can point into, read off its
    /// body without regard to order: every way a name is given a value counts,
    /// so a name points wherever any of them points.
    fn returned_param_origins(&self, f: &FnDecl) -> ParamOrigins {
        struct Walk<'c, 'a> {
            checker: &'c OwnershipChecker<'a>,
            params: Vec<String>,
            origins: HashMap<String, ParamOrigins>,
            returned: ParamOrigins,
            in_closure: u32,
            changed: bool,
        }
        impl<'c, 'a> Walk<'c, 'a> {
            fn param(&self, name: &str) -> Option<usize> {
                self.params.iter().position(|p| p == name)
            }
            /// Where the storage of a name is: a parameter's own, or what a
            /// local was lent or copied from.
            fn name_origins(&self, name: &str) -> ParamOrigins {
                match self.param(name) {
                    Some(i) => HashSet::from([i]),
                    None => self.origins.get(name).cloned().unwrap_or_default(),
                }
            }
            fn root_origins(&self, expr: &Expr) -> ParamOrigins {
                let place = match &expr.kind {
                    ExprKind::MethodCall { object, .. } => object.as_ref(),
                    _ => expr,
                };
                let root = match &place.kind {
                    ExprKind::Ident(n) => Some(n.as_str()),
                    _ => OwnershipChecker::projection_root(place),
                };
                root.map(|r| self.name_origins(r)).unwrap_or_default()
            }
            /// What a value's pointers point into.
            fn pointer_origins(&self, expr: &Expr) -> ParamOrigins {
                let c = self.checker;
                let mut out = ParamOrigins::new();
                let mut values = Vec::new();
                c.values_reaching(expr, true, &mut values);
                for v in values {
                    let holds = c.node_ty(&v.id).is_some_and(|t| c.type_holds_pointer(t));
                    if !holds && !matches!(v.kind, ExprKind::Closure { .. }) {
                        continue;
                    }
                    match &v.kind {
                        ExprKind::Ident(n) => out.extend(self.name_origins(n)),
                        ExprKind::Closure { params, body, .. } => {
                            let locals: HashSet<String> = params.iter().map(|p| p.name.clone()).collect();
                            let mut captures = Vec::new();
                            c.collect_free_vars(body, &locals, &mut captures);
                            for n in captures {
                                out.extend(self.name_origins(&n));
                            }
                        }
                        ExprKind::Call { .. } | ExprKind::MethodCall { .. } => {
                            for arg in c.pointer_call_args(v) {
                                out.extend(self.storage_origins(arg));
                            }
                        }
                        _ => out.extend(self.root_origins(v)),
                    }
                }
                out
            }
            /// What a call argument can be pointed into through.
            fn storage_origins(&self, arg: &Expr) -> ParamOrigins {
                let c = self.checker;
                let Some(ty) = c.node_ty(&arg.id) else { return ParamOrigins::new() };
                if c.type_holds_pointer(ty) {
                    return self.pointer_origins(arg);
                }
                if OwnershipChecker::has_no_storage(ty) {
                    return ParamOrigins::new();
                }
                self.root_origins(arg)
            }
            /// What a name bound from `init` stands for: where its pointers
            /// point, or where its storage is.
            fn bind(&mut self, names: &[String], init: &Expr) {
                let c = self.checker;
                let from = if c.node_ty(&init.id).is_some_and(|t| c.type_holds_pointer(t)) {
                    self.pointer_origins(init)
                } else {
                    self.storage_origins(init)
                };
                self.add(names, from);
            }
            fn add(&mut self, names: &[String], from: ParamOrigins) {
                if from.is_empty() {
                    return;
                }
                for n in names {
                    if self.param(n).is_some() {
                        continue;
                    }
                    let entry = self.origins.entry(n.clone()).or_default();
                    let before = entry.len();
                    entry.extend(from.iter().copied());
                    self.changed |= entry.len() != before;
                }
            }
        }
        impl<'c, 'a, 'e> Visit<'e> for Walk<'c, 'a> {
            fn expr(&mut self, expr: &'e Expr) -> bool {
                match &expr.kind {
                    // A `return` inside a closure is the closure's.
                    ExprKind::Closure { body, .. } => {
                        self.in_closure += 1;
                        visit::visit_expr(body, self);
                        self.in_closure -= 1;
                        return false;
                    }
                    ExprKind::IsPresent { expr: inner, binding: Some(name) } => {
                        self.bind(std::slice::from_ref(name), inner);
                    }
                    ExprKind::IfLet { expr: scrutinee, pattern, .. } => {
                        let names = rask_ast::stmt::pattern_binding_names(pattern);
                        self.bind(&names, scrutinee);
                    }
                    ExprKind::Match { scrutinee, arms } => {
                        for arm in arms {
                            let names = rask_ast::stmt::pattern_binding_names(&arm.pattern);
                            self.bind(&names, scrutinee);
                        }
                    }
                    ExprKind::WithAs { bindings, .. } => {
                        for b in bindings {
                            let from = self.root_origins(&b.source);
                            self.add(std::slice::from_ref(&b.name), from);
                        }
                    }
                    _ => {}
                }
                true
            }
            fn stmt(&mut self, stmt: &'e Stmt) -> bool {
                match &stmt.kind {
                    StmtKind::Let { name, init, .. } | StmtKind::Mut { name, init, .. } => {
                        self.bind(std::slice::from_ref(name), init);
                    }
                    StmtKind::LetTuple { patterns, init } | StmtKind::MutTuple { patterns, init } => {
                        let names: Vec<String> = rask_ast::stmt::tuple_pats_flat_names(patterns)
                            .into_iter()
                            .map(str::to_string)
                            .collect();
                        self.bind(&names, init);
                    }
                    StmtKind::LetStruct { pattern, init, .. } => {
                        let names = rask_ast::stmt::pattern_binding_names(pattern);
                        self.bind(&names, init);
                    }
                    StmtKind::WhileLet { pattern, expr, .. } => {
                        let names = rask_ast::stmt::pattern_binding_names(pattern);
                        self.bind(&names, expr);
                    }
                    StmtKind::Assign { target, value, .. } => {
                        if let Some(root) = OwnershipChecker::assign_root(target) {
                            self.bind(&[root], value);
                        }
                    }
                    StmtKind::For { binding, iter, .. } => {
                        let names: Vec<String> = binding.names().into_iter().map(str::to_string).collect();
                        let from = self.root_origins(iter);
                        self.add(&names, from);
                    }
                    StmtKind::Return(Some(value)) if self.in_closure == 0 => {
                        let from = self.pointer_origins(value);
                        self.returned.extend(from);
                    }
                    _ => {}
                }
                true
            }
        }
        let mut w = Walk {
            checker: self,
            params: f.params.iter().map(|p| p.name.clone()).collect(),
            origins: HashMap::new(),
            returned: ParamOrigins::new(),
            in_closure: 0,
            changed: true,
        };
        // Order doesn't matter, so walk until nothing new is learned.
        while w.changed {
            w.changed = false;
            w.returned.clear();
            visit::visit_body(&f.body, &mut w);
        }
        w.returned
    }

    /// The arguments of a call answering a pointer that the pointer can point
    /// into: those its body says (`infer_pointer_returns`), or, with no body to
    /// read, every argument it borrows. A C function's point into nothing.
    pub(crate) fn pointer_call_args<'e>(&self, call: &'e Expr) -> Vec<&'e Expr> {
        let mut out = Vec::new();
        match &call.kind {
            ExprKind::Call { func, args } => {
                if func.name().is_some_and(|n| self.c_functions.contains(n)) {
                    return out;
                }
                let inferred = func.name().and_then(|n| self.pointer_returns.get(&(None, n.to_string())));
                match inferred {
                    Some(flags) => {
                        for (i, a) in args.iter().enumerate() {
                            if flags.get(i).copied().unwrap_or(false) {
                                out.push(&a.expr);
                            }
                        }
                    }
                    None => {
                        let takes = self.callee_take_params(func).unwrap_or_default();
                        for (i, a) in args.iter().enumerate() {
                            if !takes.get(i).copied().unwrap_or(false) {
                                out.push(&a.expr);
                            }
                        }
                    }
                }
            }
            ExprKind::MethodCall { object, method, args, .. } => {
                let inferred = self
                    .receiver_type_name(object)
                    .and_then(|ty| self.pointer_returns.get(&(Some(ty), method.clone())));
                let self_at = self.method_self_position(object, method);
                match (inferred, self_at) {
                    (Some(flags), Some(k)) => {
                        if flags.get(k).copied().unwrap_or(false) {
                            out.push(object.as_ref());
                        }
                        let rest = (0..flags.len()).filter(|&i| i != k);
                        for (a, i) in args.iter().zip(rest) {
                            if flags[i] {
                                out.push(&a.expr);
                            }
                        }
                    }
                    (Some(flags), None) => {
                        for (i, a) in args.iter().enumerate() {
                            if flags.get(i).copied().unwrap_or(false) {
                                out.push(&a.expr);
                            }
                        }
                    }
                    (None, _) => {
                        if !self.is_take_self_method(object, method) {
                            out.push(object.as_ref());
                        }
                        let modes = self.method_param_modes(call.id, object, method).unwrap_or_default();
                        for (i, a) in args.iter().enumerate() {
                            if !matches!(modes.get(i), Some(ParamMode::Take)) {
                                out.push(&a.expr);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Where `self` sits among a method's declared parameters, if it has one:
    /// `None` for a static method, whose arguments line up from the start.
    fn method_self_position(&self, object: &Expr, method: &str) -> Option<usize> {
        let ty = self.receiver_type_name(object)?;
        self.method_params.get(&(ty, method.to_string()))?.iter().position(|n| n == "self")
    }
}
