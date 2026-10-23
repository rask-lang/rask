// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! String slices as values (std.strings/S2).
//!
//! `s[i..j]`, `s.trim()` and the items of `s.split(…)` are `StringSlice`s: a
//! view of the bytes plus where they sit in the string they were cut from.
//! `s[i..j]` itself is the `Slice` operator, conformed to in stdlib/string.rk.
//! Two rules make a slice usable wherever a string reads:
//!
//! - A method a slice or a view doesn't declare is the string's. A view is
//!   laid out as a string on both backends, so it is the receiver as it is; a
//!   slice is its `.view()` first.
//! - A slice passed where a `string` parameter is copies: `.to_string()`.
//!   One compared against is read through `.view()`, like a receiver.
//!
//! Everything is recorded by node and written into the declarations after the
//! check, so every pass after this one sees ordinary method calls.

use std::collections::HashMap;

use rask_ast::{NodeId, Span};

use super::errors::TypeError;
use rask_ast::coercion::CoercionSite;
use super::TypeChecker;
use crate::types::Type;

/// What the checker decided about slices, by node.
#[derive(Debug, Clone, Default)]
pub struct StringSlices {
    /// A method call on a slice that the string answers → the `.view()` call
    /// its receiver goes through.
    pub receivers: HashMap<NodeId, NodeId>,
    /// A slice passed for a `string` → the `.to_string()` call around it.
    pub copies: HashMap<NodeId, NodeId>,
    /// A comparison call with slices among its arguments → each such
    /// argument's position and the `.view()` call it goes through.
    pub compared: HashMap<NodeId, Vec<(usize, NodeId)>>,
}

/// Which of the three string types a value is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StringKind {
    String,
    View,
    Slice,
}

impl TypeChecker {
    pub(super) fn string_kind(&self, ty: &Type) -> Option<StringKind> {
        match self.resolve_named(&self.ctx.apply(ty)) {
            Type::String => Some(StringKind::String),
            Type::Named(id) => match self.types.type_name(id).as_str() {
                "StringView" => Some(StringKind::View),
                "StringSlice" => Some(StringKind::Slice),
                _ => None,
            },
            _ => None,
        }
    }

    /// A method a view or a slice doesn't declare, asked of the string it
    /// reads as. `None` when that isn't the case.
    pub(super) fn redirect_string_reader(
        &mut self,
        ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
        call_node: Option<NodeId>,
    ) -> Option<Result<bool, TypeError>> {
        if method == "to_vec" && self.yields_slices(ty) {
            return Some(Err(TypeError::SlicesToVec { span }));
        }
        let kind = self.string_kind(ty)?;
        // Equality, order and hashing are the text's, whatever was derived
        // from the fields: two slices of different strings can be equal. A
        // view compared against is read as the string it's laid out as, and a
        // slice as its view.
        let compares = matches!(method, "eq" | "lt" | "le" | "gt" | "ge" | "compare" | "hash");
        let views_read: Vec<Type> = if compares {
            let mut read = Vec::with_capacity(args.len());
            for (i, a) in args.iter().enumerate() {
                match self.string_kind(a) {
                    Some(StringKind::View) => read.push(Type::String),
                    Some(StringKind::Slice) => {
                        let call = call_node?;
                        if let Err(e) = self.view_of_compared(call, i, a, span) {
                            return Some(Err(e));
                        }
                        read.push(Type::String);
                    }
                    _ => read.push(a.clone()),
                }
            }
            read
        } else {
            args.to_vec()
        };
        if kind == StringKind::String {
            if views_read.as_slice() == args {
                return None;
            }
            return Some(self.resolve_method(Type::String, method.to_string(), views_read, ret.clone(), span, call_node));
        }
        let args = views_read.as_slice();
        let Type::Named(id) = self.resolve_named(&self.ctx.apply(ty)) else { return None };
        // A clone of a view is a view, not the string it reads as.
        if method == "clone" || (!compares && self.types.declares_method(id, method)) {
            return None;
        }
        if kind == StringKind::Slice {
            let call = call_node?;
            let view_call = match self.string_slices.receivers.get(&call) {
                Some(v) => *v,
                None => {
                    let v = self.derived_id();
                    self.string_slices.receivers.insert(call, v);
                    v
                }
            };
            let view = self.types.get_type_id("StringView").map(Type::Named).unwrap_or(Type::Error);
            self.node_types.insert(view_call, view.clone());
            if let Err(e) = self.resolve_method(ty.clone(), "view".to_string(), Vec::new(), view, span, Some(view_call)) {
                return Some(Err(e));
            }
        }
        Some(self.resolve_method(Type::String, method.to_string(), args.to_vec(), ret.clone(), span, call_node))
    }

    /// The `.view()` a slice compared against goes through, at argument `i`
    /// of `call`.
    fn view_of_compared(&mut self, call: NodeId, i: usize, slice: &Type, span: Span) -> Result<bool, TypeError> {
        let existing = self.string_slices.compared.get(&call).and_then(|v| v.iter().find(|(at, _)| *at == i)).map(|(_, v)| *v);
        let view_call = match existing {
            Some(v) => v,
            None => {
                let v = self.derived_id();
                self.string_slices.compared.entry(call).or_default().push((i, v));
                v
            }
        };
        let view = self.types.get_type_id("StringView").map(Type::Named).unwrap_or(Type::Error);
        self.node_types.insert(view_call, view.clone());
        self.resolve_method(slice.clone(), "view".to_string(), Vec::new(), view, span, Some(view_call))
    }

    /// A slice matched against patterns is matched as a string: string
    /// literal patterns are what it meets. The type to match against.
    pub(super) fn match_slice_as_string(&mut self, node: NodeId, ty: Type, span: Span) -> Type {
        if self.string_kind(&ty) != Some(StringKind::Slice) {
            return ty;
        }
        if let Some(Err(e)) = self.copy_slice_argument(&ty, &Type::String, CoercionSite::Argument, Some(node), span) {
            self.errors.push(e);
        }
        Type::String
    }

    /// Is this a `Sequence<StringSlice>`?
    fn yields_slices(&self, ty: &Type) -> bool {
        match self.resolve_named(&self.ctx.apply(ty)) {
            Type::Generic { base, args } if self.types.type_name(base) == "Sequence" => matches!(
                args.first(),
                Some(crate::GenericArg::Type(t)) if self.string_kind(t) == Some(StringKind::Slice)
            ),
            _ => false,
        }
    }

    /// A slice passed for a `string` parameter is copied into one. `None`
    /// when that isn't the case.
    pub(super) fn copy_slice_argument(
        &mut self,
        value: &Type,
        expected: &Type,
        site: CoercionSite,
        node: Option<NodeId>,
        span: Span,
    ) -> Option<Result<bool, TypeError>> {
        if !matches!(site, CoercionSite::Argument) {
            return None;
        }
        let node = node?;
        if self.string_kind(value)? != StringKind::Slice || self.string_kind(expected)? != StringKind::String {
            return None;
        }
        let call = match self.string_slices.copies.get(&node) {
            Some(c) => *c,
            None => {
                let c = self.derived_id();
                self.string_slices.copies.insert(node, c);
                c
            }
        };
        self.node_types.insert(call, Type::String);
        Some(self.resolve_method(value.clone(), "to_string".to_string(), Vec::new(), Type::String, span, Some(call)))
    }
}

impl StringSlices {
    /// Write the calls into `decls`: each range index becomes its slice call,
    /// each slice receiver a string reads goes through `.view()`, and each
    /// slice passed for a `string` is wrapped in `.to_string()`.
    pub fn rewrite(&self, decls: &mut Vec<rask_ast::decl::Decl>) {
        if self.receivers.is_empty() && self.copies.is_empty() && self.compared.is_empty() {
            return;
        }
        rask_ast::rewrite::rewrite_decls(
            decls,
            &mut Rewriter { slices: self, done: Default::default(), compared_done: Default::default() },
        );
    }
}

struct Rewriter<'a> {
    slices: &'a StringSlices,
    /// Nodes already wrapped, which the walk reaches again inside the wrapper.
    done: std::collections::HashSet<NodeId>,
    /// Comparison calls whose slice arguments are already wrapped.
    compared_done: std::collections::HashSet<NodeId>,
}

impl rask_ast::rewrite::Rewrite for Rewriter<'_> {
    fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
        use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
        let call = |id: NodeId, object: Expr, method: &str, args: Vec<Expr>, span: Span| Expr {
            id,
            kind: ExprKind::MethodCall {
                object: Box::new(object),
                method: method.to_string(),
                type_args: None,
                args: args.into_iter().map(|expr| CallArg { name: None, mode: ArgMode::Default, expr }).collect(),
            },
            span,
        };
        let span = e.span;

        if let Some(&copy) = self.slices.copies.get(&e.id) {
            if self.done.insert(e.id) {
                let value = std::mem::replace(e, Expr { id: copy, kind: ExprKind::Bool(false), span });
                *e = call(copy, value, "to_string", Vec::new(), span);
                return;
            }
        }

        if let Some(&view) = self.slices.receivers.get(&e.id) {
            if self.done.insert(e.id) {
                if let ExprKind::MethodCall { object, .. } = &mut e.kind {
                    let receiver = std::mem::replace(object.as_mut(), Expr { id: view, kind: ExprKind::Bool(false), span });
                    **object = call(view, receiver, "view", Vec::new(), span);
                }
            }
        }

        if let Some(views) = self.slices.compared.get(&e.id) {
            if self.compared_done.insert(e.id) {
                if let ExprKind::MethodCall { args, .. } = &mut e.kind {
                    for &(i, view) in views {
                        let Some(arg) = args.get_mut(i) else { continue };
                        let arg_span = arg.expr.span;
                        let value = std::mem::replace(&mut arg.expr, Expr { id: view, kind: ExprKind::Bool(false), span: arg_span });
                        arg.expr = call(view, value, "view", Vec::new(), arg_span);
                    }
                }
            }
        }
    }
}
