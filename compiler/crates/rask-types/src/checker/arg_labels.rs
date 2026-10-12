// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Named arguments. A label places its argument on the parameter it names,
//! wherever it's written (SYNTAX.md, named arguments; `rask_ast::call`).
//!
//! Calls to free functions arrive placed: the desugarer did it by name. A
//! call the desugarer couldn't place is placed here before its arguments are
//! checked, and a method call where its method resolves. `attach_derived`
//! then writes each placement into the call, so every pass after the checker
//! sees arguments in parameter order. Arguments still run in written order:
//! the desugarer bound the ones that run code (`bind_labeled_args_in_order`).

use rask_ast::call::{is_identity, place_args, ArgSlot, ParamSpec, PlaceError};
use rask_ast::expr::{CallArg, Expr};
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
    /// Parameter names, when the call's own path knew them directly — a
    /// variant's fields, a module function's stub. Everything else is read off
    /// the resolved target.
    pub names: Option<Vec<String>>,
}

/// Where one parameter's argument comes from, in parameter order.
#[derive(Clone)]
pub(super) enum Placement {
    /// The argument written at this index.
    Written(usize),
    /// The parameter's default, copied for this call, and its type.
    Default(Expr, Type),
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

    /// A free call the desugarer couldn't place — one reaching a package's
    /// function, say. `names` are the callee's parameters. The arguments in
    /// parameter order when the labels move any; `None` otherwise, including
    /// when they don't fit, which `validate_arg_labels` reports.
    pub(super) fn place_free_call(&mut self, call: NodeId, names: &[String], args: &[CallArg]) -> Option<Vec<CallArg>> {
        if !args.iter().any(|a| a.name.is_some()) {
            return None;
        }
        let specs: Vec<ParamSpec> = names.iter().map(|n| ParamSpec { name: n, has_default: false }).collect();
        let labels: Vec<Option<&str>> = args.iter().map(|a| a.name.as_deref()).collect();
        let slots = place_args(&specs, &labels).ok()?;
        if is_identity(&slots) {
            return None;
        }
        let placed: Vec<Placement> = slots
            .iter()
            .map(|s| match s {
                ArgSlot::Written(i) => Placement::Written(*i),
                ArgSlot::Default => unreachable!("no parameter here has a default"),
            })
            .collect();
        let out = placed
            .iter()
            .map(|p| match p {
                Placement::Written(i) => args[*i].clone(),
                Placement::Default(..) => unreachable!(),
            })
            .collect();
        self.arg_places.entry(call).or_insert(placed);
        Some(out)
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
            return Self::apply_placement(placed, args);
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
                    let ty = self.infer_expr(&expr);
                    Placement::Default(expr, ty)
                }
            });
        }
        let out = Self::apply_placement(&placed, args);
        self.arg_places.insert(call, placed);
        out
    }

    fn apply_placement(placed: &[Placement], args: Vec<Type>) -> Vec<Type> {
        if placed.iter().any(|p| matches!(p, Placement::Written(i) if *i >= args.len())) {
            return args;
        }
        placed
            .iter()
            .map(|p| match p {
                Placement::Written(i) => args[*i].clone(),
                Placement::Default(_, ty) => ty.clone(),
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
    fn renumber(&mut self, expr: &mut rask_ast::expr::Expr) {
        struct Fresh<'a>(&'a mut u32);
        impl rask_ast::rewrite::Rewrite for Fresh<'_> {
            fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
                e.id = NodeId(*self.0);
                *self.0 += 1;
            }
        }
        rask_ast::rewrite::rewrite_expr(expr, &mut Fresh(&mut self.next_derived_id));
    }
}
