// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Named-argument labels. A label documents the call and never reorders it
//! (SYNTAX.md, named arguments), so each one has to name the parameter in its
//! own position. Checked once inference has settled every callee.

use rask_ast::expr::CallArg;
use rask_ast::{NodeId, Span};
use rask_resolve::{SymbolId, SymbolKind};

use super::errors::TypeError;
use super::type_defs::{Callee, TypeDef};
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
    /// `(position, label, argument span)` for each labeled argument.
    pub labels: Vec<(usize, String, Span)>,
    /// Parameter names, when the call's own path knew them directly — a
    /// variant's fields, a module function's stub. Everything else is read off
    /// the resolved target.
    pub names: Option<Vec<String>>,
}

impl TypeChecker {
    /// Remember a call's labels for `validate_arg_labels`. A body can be
    /// inferred more than once; the first record stands.
    pub(super) fn note_arg_labels(&mut self, call: NodeId, written: String, args: &[CallArg], span: Span) {
        let labels: Vec<(usize, String, Span)> = args
            .iter()
            .enumerate()
            .filter_map(|(i, a)| a.name.as_ref().map(|n| (i, n.clone(), a.expr.span)))
            .collect();
        if labels.is_empty() || self.labeled_calls.iter().any(|c| c.call == call) {
            return;
        }
        self.labeled_calls.push(LabeledCall { call, written, span, labels, names: None });
    }

    /// The call's own path knows its parameter names: record them.
    pub(super) fn note_param_names(&mut self, call: NodeId, names: Vec<String>) {
        if let Some(c) = self.labeled_calls.iter_mut().find(|c| c.call == call) {
            c.names.get_or_insert(names);
        }
    }

    pub(super) fn validate_arg_labels(&mut self) {
        let calls = std::mem::take(&mut self.labeled_calls);
        for call in &calls {
            let (callee, names) = match &call.names {
                Some(names) => (call.written.clone(), Some(names.clone())),
                None => self.callee_param_names(call),
            };
            // One error per call: a swap mislabels two arguments, and the
            // second report says nothing the first didn't.
            let wrong = call.labels.iter().find(|(i, label, _)| {
                names.as_ref().and_then(|n| n.get(*i)) != Some(label)
            });
            if let Some((position, label, span)) = wrong {
                // An out-of-order call with defaults is one the desugarer
                // couldn't fill, so it also arrives a few arguments short.
                // The label is the cause; "add the missing argument" would
                // send the reader the wrong way.
                self.errors.retain(|e| {
                    !matches!(e, TypeError::ArityMismatch { span, .. } if *span == call.span)
                });
                self.errors.push(TypeError::ArgLabelMismatch {
                    callee,
                    label: label.clone(),
                    position: *position,
                    params: names,
                    span: *span,
                });
            }
        }
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
}
