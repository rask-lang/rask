// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Named arguments. A label places its argument on the parameter it names,
//! wherever it's written (SYNTAX.md, named arguments; `rask_ast::call`).
//!
//! A call to a free function is placed before its arguments are checked, on
//! the function the resolver found; a method call where its method resolves.
//! Both fill the parameters a call leaves out from their defaults. `attach_derived`
//! then writes each placement into the call, so every pass after the checker
//! sees arguments in parameter order. Arguments still run in written order:
//! where placing moves one that runs code ahead of another, they're bound to
//! temporaries first (`plan_hoist`).

use rask_ast::call::{is_identity, place_args, runs_code, ArgSlot, ParamSpec, PlaceError};
use rask_ast::expr::{ArgMode, CallArg, Expr};
use rask_ast::{NodeId, Span};
use rask_resolve::{SymbolId, SymbolKind};

use super::errors::{ArgProblem, TypeError};
use super::type_defs::{Callee, MethodSig, TypeDef};
use super::TypeChecker;

use crate::types::Type;

/// A call that wrote at least one label.
pub(super) struct LabeledCall {
    pub call: NodeId,
    /// How the call names its callee, for the message when no resolved
    /// target says better.
    pub written: String,
    /// The whole call.
    pub span: Span,
    /// Each written argument's label, in written order.
    pub labels: Vec<Option<String>>,
    /// Each written argument's span.
    pub arg_spans: Vec<Span>,
    /// Each written argument's expression id.
    pub arg_ids: Vec<NodeId>,
    /// Whether each written argument runs code (`rask_ast::call::runs_code`).
    pub runs_code: Vec<bool>,
    /// Parameter names, when the call's own path knew them directly — a
    /// variant's fields, a module function's stub. Everything else is read off
    /// the resolved target.
    pub names: Option<Vec<String>>,
}

/// Arguments of one placed call bound to temporaries, so they still run in
/// the order written.
#[derive(Clone)]
pub(super) struct ArgHoist {
    /// The block that replaces the call.
    pub block: NodeId,
    /// One per argument that runs code, in written order.
    pub binds: Vec<HoistBind>,
}

#[derive(Clone)]
pub(super) struct HoistBind {
    /// The written argument's index.
    pub arg: usize,
    pub let_id: NodeId,
    /// The temporary's use, in the call.
    pub ident: NodeId,
    /// The argument's own expression, whose type the temporary has.
    pub source: NodeId,
}

/// Where one parameter's argument comes from, in parameter order.
#[derive(Clone)]
pub(super) enum Placement {
    /// The argument written at this index.
    Written(usize),
    /// The parameter's default, copied for this call.
    Default(Expr),
}

impl TypeChecker {
    /// Remember a call's labels. A body can be inferred more than once; the
    /// first record stands.
    pub(super) fn note_arg_labels(&mut self, call: NodeId, written: String, args: &[CallArg], span: Span) {
        if !args.iter().any(|a| a.name.is_some()) || self.labeled_calls.iter().any(|c| c.call == call) {
            return;
        }
        self.labeled_calls.push(LabeledCall {
            call,
            written,
            span,
            labels: args.iter().map(|a| a.name.clone()).collect(),
            arg_spans: args.iter().map(|a| a.expr.span).collect(),
            arg_ids: args.iter().map(|a| a.expr.id).collect(),
            runs_code: args
                .iter()
                .map(|a| a.mode == rask_ast::expr::ArgMode::Default && runs_code(&a.expr))
                .collect(),
            names: None,
        });
    }

    /// The call's own path knows its parameter names: record them.
    pub(super) fn note_param_names(&mut self, call: NodeId, names: Vec<String>) {
        if let Some(c) = self.labeled_calls.iter_mut().find(|c| c.call == call) {
            c.names.get_or_insert(names);
        }
    }

    fn written_labels(&self, call: NodeId, count: usize) -> Vec<Option<String>> {
        self.labeled_calls
            .iter()
            .find(|c| c.call == call && c.labels.len() == count)
            .map(|c| c.labels.clone())
            .unwrap_or_else(|| vec![None; count])
    }

    /// A free call's arguments in parameter order, with defaults in place for
    /// the parameters it leaves out. `names` and `defaults` are the parameters
    /// of the function the resolver found the call reaching.
    ///
    /// Desugaring used to fill defaults before there was a resolver, by the
    /// callee's name, so a closure bound to a function's name got the
    /// function's defaults pasted into its calls.
    ///
    /// `None` when the call is already in parameter order, or doesn't fit:
    /// the arity and label checks report that.
    pub(super) fn place_free_call(
        &mut self,
        call: NodeId,
        names: &[String],
        defaults: &[Option<Expr>],
        args: &[CallArg],
    ) -> Option<Vec<CallArg>> {
        // A call can be inferred more than once; the first placement stands.
        if let Some(placed) = self.arg_places.get(&call) {
            return Self::placed_call_args(placed, args);
        }
        let labeled = args.iter().any(|a| a.name.is_some());
        let defaulted = defaults.iter().any(Option::is_some);
        if !labeled && (args.len() >= names.len() || !defaulted) {
            return None;
        }
        let specs: Vec<ParamSpec> = names
            .iter()
            .enumerate()
            .map(|(i, n)| ParamSpec { name: n, has_default: defaults.get(i).is_some_and(Option::is_some) })
            .collect();
        let labels: Vec<Option<&str>> = args.iter().map(|a| a.name.as_deref()).collect();
        let slots = place_args(&specs, &labels).ok()?;
        if is_identity(&slots) {
            return None;
        }
        let mut placed = Vec::with_capacity(slots.len());
        for (i, slot) in slots.iter().enumerate() {
            placed.push(match slot {
                ArgSlot::Written(a) => Placement::Written(*a),
                ArgSlot::Default => {
                    let mut expr = defaults[i].clone().expect("a defaulted slot has a default");
                    self.renumber(&mut expr);
                    // A compile-time constant (FD1), so it's computed while
                    // compiling.
                    Placement::Default(self.comptime_block(expr))
                }
            });
        }
        let out = Self::placed_call_args(&placed, args)?;
        self.plan_hoist(call, &placed);
        self.arg_places.insert(call, placed);
        Some(out)
    }

    fn placed_call_args(placed: &[Placement], args: &[CallArg]) -> Option<Vec<CallArg>> {
        placed
            .iter()
            .map(|p| match p {
                Placement::Written(i) => args.get(*i).cloned(),
                Placement::Default(expr) => {
                    Some(CallArg { name: None, mode: ArgMode::Default, expr: expr.clone() })
                }
            })
            .collect()
    }

    /// A method call's argument types in parameter order, with defaults in
    /// place for the parameters it leaves out.
    ///
    /// Done here, where `sig` is the method the receiver's type actually
    /// declares. Desugaring used to fill defaults before there were types, so
    /// it matched a call to its method by name: a program declaring any
    /// `shrink` took the default away from `Vec.shrink()` (#1312).
    ///
    /// Anything that doesn't fit is left as written, for the arity and label
    /// checks to report.
    pub(super) fn place_method_args(&mut self, call_node: Option<NodeId>, sig: &MethodSig, args: Vec<Type>) -> Vec<Type> {
        let Some(call) = call_node else { return args };
        // A call can be resolved more than once; the first placement stands.
        if let Some(placed) = self.arg_places.get(&call) {
            return self.apply_placement(placed, args);
        }
        let labels = self.written_labels(call, args.len());
        let labeled = labels.iter().any(Option::is_some);
        let defaults = sig.defaults.iter().any(Option::is_some);
        if !labeled && (args.len() >= sig.params.len() || !defaults) {
            return args;
        }
        if sig.param_names.len() != sig.params.len() {
            return args;
        }
        let specs: Vec<ParamSpec> = sig
            .param_names
            .iter()
            .enumerate()
            .map(|(i, n)| ParamSpec { name: n, has_default: sig.defaults.get(i).is_some_and(Option::is_some) })
            .collect();
        let label_refs: Vec<Option<&str>> = labels.iter().map(|l| l.as_deref()).collect();
        let Ok(slots) = place_args(&specs, &label_refs) else { return args };
        if is_identity(&slots) {
            return args;
        }
        let mut placed = Vec::with_capacity(slots.len());
        for (i, slot) in slots.iter().enumerate() {
            placed.push(match slot {
                ArgSlot::Written(a) => Placement::Written(*a),
                ArgSlot::Default => {
                    let mut expr = sig.defaults[i].clone().expect("a defaulted slot has a default");
                    self.renumber(&mut expr);
                    // A default argument is a compile-time constant, filled
                    // in as a `comptime` block the way desugar fills in a
                    // free function's, so it's computed while compiling.
                    let expr = self.comptime_block(expr);
                    self.infer_expr(&expr);
                    Placement::Default(expr)
                }
            });
        }
        let out = self.apply_placement(&placed, args);
        self.plan_hoist(call, &placed);
        self.arg_places.insert(call, placed);
        out
    }

    /// Arguments run in the order written. Placing puts them in parameter
    /// order, and when that moves one that runs code ahead of another that
    /// does, each of those is bound to a temporary first, in written order:
    /// `f(b: g(), a: h())` becomes
    /// `{ let __arg_1 = g(); let __arg_2 = h(); f(__arg_2, __arg_1) }`.
    /// A call whose working arguments keep their order is left alone.
    fn plan_hoist(&mut self, call: NodeId, placed: &[Placement]) {
        let Some(labeled) = self.labeled_calls.iter().find(|c| c.call == call) else { return };
        let working: Vec<usize> = placed
            .iter()
            .filter_map(|p| match p {
                Placement::Written(i) if labeled.runs_code.get(*i) == Some(&true) => Some(*i),
                _ => None,
            })
            .collect();
        if working.windows(2).all(|w| w[0] < w[1]) {
            return;
        }
        let sources: Vec<(usize, NodeId)> = labeled
            .runs_code
            .iter()
            .enumerate()
            .filter(|(_, runs)| **runs)
            .map(|(i, _)| (i, labeled.arg_ids[i]))
            .collect();
        let mut fresh = || {
            let id = NodeId(self.next_derived_id);
            self.next_derived_id += 1;
            id
        };
        let block = fresh();
        let binds = sources
            .into_iter()
            .map(|(arg, source)| HoistBind { arg, let_id: fresh(), ident: fresh(), source })
            .collect();
        self.arg_hoists.insert(call, ArgHoist { block, binds });
    }

    /// The temporaries' and their block's types: each temporary is the
    /// argument it holds, and the block is the call.
    pub(super) fn type_hoisted_args(&mut self) {
        for (call, hoist) in &self.arg_hoists {
            if let Some(ty) = self.node_types.get(call).cloned() {
                self.node_types.insert(hoist.block, ty);
            }
            for bind in &hoist.binds {
                if let Some(ty) = self.node_types.get(&bind.source).cloned() {
                    self.node_types.insert(bind.ident, ty);
                }
            }
        }
    }

    fn apply_placement(&self, placed: &[Placement], args: Vec<Type>) -> Vec<Type> {
        if placed.iter().any(|p| matches!(p, Placement::Written(i) if *i >= args.len())) {
            return args;
        }
        placed
            .iter()
            .map(|p| match p {
                Placement::Written(i) => args[*i].clone(),
                Placement::Default(expr) => self.node_types.get(&expr.id).cloned().unwrap_or(Type::Error),
            })
            .collect()
    }

    /// The written argument filling parameter `param` of a placed call.
    /// `None` when a default fills it.
    pub(super) fn written_arg_for(&self, call: NodeId, param: usize) -> Option<usize> {
        match self.arg_places.get(&call) {
            Some(placed) => match placed.get(param)? {
                Placement::Written(i) => Some(*i),
                Placement::Default(..) => None,
            },
            None => Some(param),
        }
    }

    /// Report the calls whose labels don't fit their callee, once inference
    /// has settled every callee.
    pub(super) fn validate_arg_labels(&mut self) {
        let calls = std::mem::take(&mut self.labeled_calls);
        for call in &calls {
            let (callee, names) = match &call.names {
                Some(names) => (call.written.clone(), Some(names.clone())),
                None => self.callee_param_names(call),
            };
            let Some(names) = names else {
                let (at, label) = call
                    .labels
                    .iter()
                    .enumerate()
                    .find_map(|(i, l)| l.as_ref().map(|l| (i, l.clone())))
                    .expect("a labeled call has a label");
                self.push_arg_problem(call, &callee, ArgProblem::NoNames(label), None, at);
                continue;
            };
            // Missing parameters are the arity check's to report.
            let specs: Vec<ParamSpec> = names.iter().map(|n| ParamSpec { name: n, has_default: true }).collect();
            let labels: Vec<Option<&str>> = call.labels.iter().map(|l| l.as_deref()).collect();
            let label_at = |i: usize| call.labels[i].clone().unwrap_or_default();
            let (problem, at) = match place_args(&specs, &labels) {
                Ok(slots) if is_identity(&slots) || self.arg_places.contains_key(&call.call) => continue,
                Ok(_) => (ArgProblem::NotReordered, 0),
                Err(PlaceError::UnknownLabel { arg }) => (ArgProblem::UnknownLabel(label_at(arg)), arg),
                Err(PlaceError::Repeated { arg, .. }) => (ArgProblem::Repeated(label_at(arg)), arg),
                Err(PlaceError::PositionalAfterLabel { arg }) => (ArgProblem::PositionalAfterLabel, arg),
                // Arity's to report.
                Err(PlaceError::TooMany { .. } | PlaceError::Missing { .. }) => continue,
            };
            self.push_arg_problem(call, &callee, problem, Some(names), at);
        }
    }

    fn push_arg_problem(&mut self, call: &LabeledCall, callee: &str, problem: ArgProblem, params: Option<Vec<String>>, at: usize) {
        let span = call.arg_spans.get(at).copied().unwrap_or(call.span);
        self.errors.push(TypeError::ArgPlacement { callee: callee.to_string(), problem, params, span });
    }

    /// The callee's display name and its parameter names, in order. `None`
    /// names when the callee has none to give: a closure value, an extern, a
    /// signature the checker supplied.
    fn callee_param_names(&self, call: &LabeledCall) -> (String, Option<Vec<String>>) {
        match self.call_targets.get(&call.call) {
            Some(Callee::Free(sym_id)) => (call.written.clone(), self.function_param_names(*sym_id)),
            Some(Callee::Method { recv, method, .. }) => {
                let recv = self.resolve_named(&self.ctx.apply(recv));
                let owner = super::receiver_name(&recv, &self.types)
                    .unwrap_or_else(|| call.written.clone());
                (format!("{owner}.{method}"), self.method_param_names(&recv, method))
            }
            None => (call.written.clone(), None),
        }
    }

    /// A declared function's parameter names, from the resolver's symbols.
    /// `None` for anything that isn't a declared function: a closure value
    /// bound to a name, an extern, a builtin.
    pub(super) fn function_param_names(&self, sym_id: SymbolId) -> Option<Vec<String>> {
        match &self.resolved.symbols.get(sym_id)?.kind {
            SymbolKind::Function { params, .. } => Some(
                params
                    .iter()
                    .filter_map(|p| self.resolved.symbols.get(*p))
                    .map(|p| p.name.clone())
                    .filter(|n| n != "self")
                    .collect(),
            ),
            _ => None,
        }
    }

    fn method_param_names(&self, recv: &Type, method: &str) -> Option<Vec<String>> {
        let type_id = match recv {
            Type::Named(id) | Type::Generic { base: id, .. } => Some(*id),
            Type::InterfaceObject { decl, .. } => *decl,
            _ => None,
        };
        let declared = type_id.and_then(|id| match self.types.get(id)? {
            TypeDef::Struct { methods, .. }
            | TypeDef::Enum { methods, .. }
            | TypeDef::Interface { methods, .. } => methods.iter().find(|m| m.name == method),
            _ => None,
        });
        if let Some(sig) = declared {
            // A supplied signature has parameters and no names for them.
            return (sig.param_names.len() == sig.params.len()).then(|| sig.param_names.clone());
        }
        let prefix = super::receiver_name(recv, &self.types)?;
        rask_stdlib::lookup_method(&prefix, method)
            .map(|stub| stub.params.iter().map(|(n, _)| n.clone()).collect())
    }

    /// Give a copy of a default its own node ids: it is a new expression at
    /// every call that takes it.
    /// Fresh ids for a copy of a default, each name keeping what it resolved
    /// to at the declaration. The copy is made here, after resolution, so a
    /// fresh id has no entry of its own: a method default calling a free
    /// function, `func send(self, n: i64 = tries())`, was "undefined name
    /// `tries`" at every call that left `n` out.
    /// `expr` as the body of a `comptime { }` of its own.
    fn comptime_block(&mut self, expr: rask_ast::expr::Expr) -> rask_ast::expr::Expr {
        let span = expr.span;
        let stmt_id = NodeId(self.next_derived_id);
        let block_id = NodeId(self.next_derived_id + 1);
        self.next_derived_id += 2;
        rask_ast::expr::Expr {
            id: block_id,
            kind: rask_ast::expr::ExprKind::Comptime {
                body: vec![rask_ast::stmt::Stmt { id: stmt_id, kind: rask_ast::stmt::StmtKind::Expr(expr), span }],
            },
            span,
        }
    }

    fn renumber(&mut self, expr: &mut rask_ast::expr::Expr) {
        struct Fresh<'a> {
            next: &'a mut u32,
            moved: Vec<(NodeId, NodeId)>,
        }
        impl rask_ast::rewrite::Rewrite for Fresh<'_> {
            fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
                let fresh = NodeId(*self.next);
                *self.next += 1;
                self.moved.push((e.id, fresh));
                e.id = fresh;
            }
        }
        let mut fresh = Fresh { next: &mut self.next_derived_id, moved: Vec::new() };
        rask_ast::rewrite::rewrite_expr(expr, &mut fresh);
        for (old, new) in fresh.moved {
            if let Some(&sym) = self.resolved.resolutions.get(&old) {
                self.resolved.resolutions.insert(new, sym);
            }
        }
    }
}
