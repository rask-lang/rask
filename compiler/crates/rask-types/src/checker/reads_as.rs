// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! type.reads-as: a struct that reads as one of its own fields.
//!
//! `@reads_as(text)` on a struct lends it the read-only methods of its `text`
//! field. A method call the struct doesn't answer itself is asked of the field
//! and called on it: `slice.len()` is `slice.text.len()`. Four limits keep it
//! honest (RA1–RA4): it is declared on the type, it names a field so the
//! conversion is a field read and never a call, it is one step, and it lends
//! only methods that read. It applies to receivers, and to both operands of a
//! comparison operator (RA6) — a value passed where the field's type is
//! expected is a type error, because the callee could keep what it was given.
//! A comparison reads both sides and answers a `bool`, so it can't.
//!
//! The projection is recorded by node and written into the declarations after
//! the check, so every pass after this one sees `x.text.m()`.

use std::collections::HashMap;

use rask_ast::{NodeId, Span};

use super::errors::TypeError;
use super::type_defs::{SelfParam, TypeDef};
use super::TypeChecker;
use crate::types::Type;

/// The receivers the checker read through a field, by call node.
#[derive(Debug, Clone, Default)]
pub struct ReadsAs {
    /// A method call → the field access its receiver became, and the field.
    pub projections: HashMap<NodeId, (NodeId, String)>,
    /// A comparison → each operand read through a field: its position among
    /// the arguments, the field access it became, and the field.
    pub operands: HashMap<NodeId, Vec<(usize, NodeId, String)>>,
}

impl TypeChecker {
    /// The field a struct reads as, with its type: `None` for anything else.
    pub(super) fn reads_as_field(&self, ty: &Type) -> Option<(String, Type)> {
        let Type::Named(id) = self.resolve_named(&self.ctx.apply(ty)) else { return None };
        let Some(TypeDef::Struct { reads_as: Some(field), fields, .. }) = self.types.get(id) else {
            return None;
        };
        let field_ty = fields.iter().find(|(n, _)| n == field)?.1.clone();
        Some((field.clone(), field_ty))
    }

    /// RA2/RA5: a method the struct doesn't declare, asked of the field it
    /// reads as. `None` when that isn't the case: the struct declares the
    /// method (its own come first), or it reads as nothing.
    pub(super) fn read_through_field(
        &mut self,
        ty: &Type,
        method: &str,
        args: &[Type],
        ret: &Type,
        span: Span,
        call_node: Option<NodeId>,
    ) -> Option<Result<bool, TypeError>> {
        let (field, field_ty) = self.reads_as_field(ty)?;
        let Type::Named(id) = self.resolve_named(&self.ctx.apply(ty)) else { return None };
        if self.types.declares_method(id, method) {
            return None;
        }
        let call = call_node?;
        // RA3: one step. A field that reads as something else isn't followed.
        if let Some(_) = self.reads_as_field(&field_ty) {
            if let Type::Named(inner) = self.resolve_named(&field_ty) {
                if !self.types.declares_method(inner, method) {
                    return Some(Err(TypeError::NoSuchMethod { ty: ty.clone(), method: method.to_string(), span }));
                }
            }
        }
        // RA4: only what reads.
        if let Some(mode) = self.declared_self_mode(&field_ty, method) {
            if !matches!(mode, SelfParam::Value) {
                return Some(Err(TypeError::ReadsAsWrite {
                    ty: self.types.display_type_names(ty).to_string(),
                    field,
                    method: method.to_string(),
                    span,
                }));
            }
        } else if self.method_mutates_self_ty(&field_ty, method) {
            return Some(Err(TypeError::ReadsAsWrite {
                ty: self.types.display_type_names(ty).to_string(),
                field,
                method: method.to_string(),
                span,
            }));
        }

        let access = match self.reads_as.projections.get(&call) {
            Some((a, _)) => *a,
            None => {
                let a = self.derived_id();
                self.reads_as.projections.insert(call, (a, field.clone()));
                a
            }
        };
        self.node_types.insert(access, field_ty.clone());
        let resolved = self.resolve_method(field_ty, method.to_string(), args.to_vec(), ret.clone(), span, Some(call));
        // The field's type lacking the method is the struct lacking it.
        Some(resolved.map_err(|e| match e {
            TypeError::NoSuchMethod { method, span, .. } => TypeError::NoSuchMethod { ty: ty.clone(), method, span },
            other => other,
        }))
    }

    /// RA6: the operands of a comparison operator read through their field,
    /// the same as a receiver does, so `"abc" == slice` and `a == b` on two
    /// slices compare the text. The argument types the comparison resolves
    /// against, with each projected operand's replaced by its field's.
    pub(super) fn read_compared_operands(&mut self, method: &str, args: &[Type], call_node: Option<NodeId>) -> Vec<Type> {
        let comparison = rask_ast::expr::BinOp::ALL
            .iter()
            .any(|op| op.is_comparison() && rask_ast::expr::binary_op_method(*op) == Some(method));
        let Some(call) = call_node.filter(|_| comparison) else { return args.to_vec() };
        let mut read = Vec::with_capacity(args.len());
        for (i, arg) in args.iter().enumerate() {
            let Some((field, field_ty)) = self.reads_as_field(arg) else {
                read.push(arg.clone());
                continue;
            };
            let known = self.reads_as.operands.get(&call).and_then(|v| v.iter().find(|(at, _, _)| *at == i)).map(|(_, a, _)| *a);
            let access = match known {
                Some(a) => a,
                None => {
                    let a = self.derived_id();
                    self.reads_as.operands.entry(call).or_default().push((i, a, field));
                    a
                }
            };
            self.node_types.insert(access, field_ty.clone());
            read.push(field_ty);
        }
        read
    }

    /// How a type's own declaration takes `self` for `method`, when it
    /// declares one: a struct's, an enum's or a primitive's conformance block.
    fn declared_self_mode(&self, ty: &Type, method: &str) -> Option<SelfParam> {
        let id = self.types.conformance_target(&self.resolve_named(&self.ctx.apply(ty)))?;
        let methods = match self.types.get(id)? {
            TypeDef::Struct { methods, .. }
            | TypeDef::Enum { methods, .. }
            | TypeDef::NominalAlias { methods, .. }
            | TypeDef::Primitive { methods, .. } => methods,
            _ => return None,
        };
        methods.iter().find(|m| m.name == method).map(|m| m.self_param.clone())
    }
}

impl ReadsAs {
    /// Write the projections into `decls`: each recorded call's receiver
    /// becomes a read of the field.
    pub fn rewrite(&self, decls: &mut Vec<rask_ast::decl::Decl>) {
        if self.projections.is_empty() && self.operands.is_empty() {
            return;
        }
        rask_ast::rewrite::rewrite_decls(decls, &mut Rewriter { reads: self, done: Default::default() });
    }
}

struct Rewriter<'a> {
    reads: &'a ReadsAs,
    /// Calls already rewritten, which the walk reaches again.
    done: std::collections::HashSet<NodeId>,
}

impl rask_ast::rewrite::Rewrite for Rewriter<'_> {
    fn expr(&mut self, e: &mut rask_ast::expr::Expr) {
        use rask_ast::expr::{Expr, ExprKind};
        let project = |value: &mut Expr, access: NodeId, field: &str| {
            let span = value.span;
            let inner = std::mem::replace(value, Expr { id: access, kind: ExprKind::Bool(false), span });
            *value = Expr { id: access, kind: ExprKind::Field { object: Box::new(inner), field: field.to_string() }, span };
        };
        let receiver = self.reads.projections.get(&e.id);
        let operands = self.reads.operands.get(&e.id);
        if (receiver.is_none() && operands.is_none()) || !self.done.insert(e.id) {
            return;
        }
        if let ExprKind::MethodCall { object, args, .. } = &mut e.kind {
            if let Some((access, field)) = receiver {
                project(object.as_mut(), *access, field);
            }
            for (i, access, field) in operands.into_iter().flatten() {
                if let Some(arg) = args.get_mut(*i) {
                    project(&mut arg.expr, *access, field);
                }
            }
        }
    }
}
