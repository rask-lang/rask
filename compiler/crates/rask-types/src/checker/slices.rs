// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! String slices as values (std.strings/S2).
//!
//! `s[i..j]`, `s.trim()` and the items of `s.split(…)` are `StringSlice`s: a
//! view of the bytes plus where they sit in the string they were cut from.
//! Three rules make one usable wherever a string reads:
//!
//! - `s[i..j]` on a string, a view or a slice is a call to that type's
//!   `slice_at`/`slice_from` (stdlib/string.rk), written here and placed into
//!   the declarations by `TypedProgram::rewrite_checked_nodes`.
//! - A method a slice or a view doesn't declare is the string's. A view is
//!   laid out as a string on both backends, so it is the receiver as it is; a
//!   slice is its `.view()` first.
//! - A slice passed where a `string` parameter is copies: `.to_string()`.
//!
//! Everything is recorded by node and written into the declarations after the
//! check, so every pass after this one sees ordinary method calls.

use std::collections::HashMap;

use rask_ast::{NodeId, Span};

use super::errors::TypeError;
use rask_ast::coercion::CoercionSite;
use super::TypeChecker;
use crate::types::Type;

/// The call a range index on a string becomes.
#[derive(Debug, Clone)]
pub struct SliceCall {
    /// The call's node: its type and target are recorded on it.
    pub call: NodeId,
    /// `slice_at` with an end, `slice_from` without one.
    pub method: &'static str,
    /// The `0` an index with no start passes, on a node of its own.
    pub zero: Option<NodeId>,
    /// `slice_at`'s `through` flag, on a node of its own.
    pub through: Option<NodeId>,
}

/// The pieces of a range index the call is built from.
#[derive(Debug, Clone, Copy)]
pub(super) struct RangeShape {
    pub start: Option<NodeId>,
    pub end: Option<NodeId>,
    pub inclusive: bool,
}

/// What the checker decided about slices, by node.
#[derive(Debug, Clone, Default)]
pub struct StringSlices {
    /// A range index → the call that makes the slice.
    pub indexes: HashMap<NodeId, SliceCall>,
    /// A method call on a slice that the string answers → the `.view()` call
    /// its receiver goes through.
    pub receivers: HashMap<NodeId, NodeId>,
    /// A slice passed for a `string` → the `.to_string()` call around it.
    pub copies: HashMap<NodeId, NodeId>,
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

    fn string_slice_type(&self) -> Type {
        self.types
            .get_type_id("StringSlice")
            .map(Type::Named)
            .unwrap_or(Type::Error)
    }

    /// `object[range]` on a string, a view or a slice: the slice it makes, as a
    /// call to the receiver's `slice_at`/`slice_from`. `None` when the object
    /// isn't one of the three.
    pub(super) fn slice_index(
        &mut self,
        node: NodeId,
        object: &Type,
        result: &Type,
        span: Span,
    ) -> Option<Result<bool, TypeError>> {
        self.string_kind(object)?;
        let shape = *self.range_indexes.get(&node)?;
        let slice = self.string_slice_type();
        if self.string_slices.indexes.contains_key(&node) {
            return Some(self.unify(result, &slice, span));
        }
        let call = self.derived_id();
        let start_ty = match shape.start {
            Some(id) => self.node_types.get(&id).cloned().unwrap_or(Type::U64),
            None => Type::U64,
        };
        let zero = shape.start.is_none().then(|| {
            let id = self.derived_id();
            self.node_types.insert(id, Type::U64);
            id
        });
        let (method, args, through) = match shape.end {
            Some(end) => {
                let end_ty = self.node_types.get(&end).cloned().unwrap_or(Type::U64);
                let through = self.derived_id();
                self.node_types.insert(through, Type::Bool);
                ("slice_at", vec![start_ty, end_ty, Type::Bool], Some(through))
            }
            None => ("slice_from", vec![start_ty], None),
        };
        self.node_types.insert(call, slice.clone());
        self.string_slices.indexes.insert(node, SliceCall { call, method, zero, through });
        let resolved = self.resolve_method(object.clone(), method.to_string(), args, slice.clone(), span, Some(call));
        Some(resolved.and_then(|_| self.unify(result, &slice, span)))
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
        let kind = self.string_kind(ty)?;
        // Equality, order and hashing are the text's, whatever was derived
        // from the fields: two slices of different strings can be equal. A
        // view compared against is read as the string it's laid out as.
        let compares = matches!(method, "eq" | "lt" | "le" | "gt" | "ge" | "compare" | "hash");
        let views_read: Vec<Type> = if compares {
            args.iter()
                .map(|a| match self.string_kind(a) {
                    Some(StringKind::View) => Type::String,
                    _ => a.clone(),
                })
                .collect()
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
        if self.indexes.is_empty() && self.receivers.is_empty() && self.copies.is_empty() {
            return;
        }
        rask_ast::rewrite::rewrite_decls(decls, &mut Rewriter { slices: self, done: Default::default() });
    }
}

struct Rewriter<'a> {
    slices: &'a StringSlices,
    /// Nodes already wrapped, which the walk reaches again inside the wrapper.
    done: std::collections::HashSet<NodeId>,
}

impl rask_ast::rewrite::Rewrite for Rewriter<'_> {
    fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
        use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
        use rask_ast::token::IntSuffix;
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

        if let Some(slice) = self.slices.indexes.get(&e.id) {
            let ExprKind::Index { object, index } = std::mem::replace(&mut e.kind, ExprKind::Bool(false)) else {
                return;
            };
            let ExprKind::Range { start, end, inclusive } = index.kind else { return };
            let start = match (start, slice.zero) {
                (Some(s), _) => *s,
                (None, Some(zero)) => Expr { id: zero, kind: ExprKind::Int(0, Some(IntSuffix::Usize)), span },
                (None, None) => return,
            };
            let mut args = vec![start];
            if let (Some(end), Some(through)) = (end, slice.through) {
                args.push(*end);
                args.push(Expr { id: through, kind: ExprKind::Bool(inclusive), span });
            }
            *e = call(slice.call, *object, slice.method, args, span);
        }
    }
}
