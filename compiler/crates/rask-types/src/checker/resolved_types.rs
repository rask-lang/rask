// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Every binding must end up with a type.
//!
//! Inference can finish with a variable still open — nothing in scope
//! constrained it, or inference has a gap. Either way there's no type to
//! compile against, and the alternative to saying so is guessing a size, which
//! silently corrupts a float, a string or a struct rather than failing. So this
//! reports instead, naming the binding the programmer can annotate.
//!
//! Scoped to `let`/`mut` bindings in program code on purpose. That's the place
//! an annotation actually goes, so the message can show the fix; and stdlib
//! signatures carry their own open variables for generic parameters, which are
//! abstract-but-known rather than unresolved.

use rask_ast::decl::{Decl, DeclKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::NodeId;
use std::collections::HashMap;

use super::errors::TypeError;
use super::TypeChecker;
use crate::types::{GenericArg, Type};

/// Does this type still contain an inference variable anywhere inside it?
pub(crate) fn is_open_type(ty: &Type) -> bool {
    is_open(ty)
}

/// Does this type still contain an inference variable anywhere inside it?
fn is_open(ty: &Type) -> bool {
    match ty {
        Type::Var(_) => true,
        Type::Result { ok, err } => is_open(ok) || is_open(err),
        Type::RawPtr(inner) | Type::Slice(inner) => is_open(inner),
        Type::Array { elem, .. } => is_open(elem),
        Type::Tuple(elems) | Type::Union(elems) => elems.iter().any(is_open),
        Type::Fn { params, ret } => params.iter().any(is_open) || is_open(ret),
        Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args
            .iter()
            .any(|a| matches!(a, GenericArg::Type(t) if is_open(t))),
        _ => false,
    }
}

/// A type to show in the suggested annotation. The outer shape is usually
/// known — it's the inside that's open — so `Vec<?>` becomes `Vec<i64>` rather
/// than a bare placeholder, which reads as a real fix instead of a shrug.
fn suggest(ty: &Type, names: &HashMap<crate::TypeId, String>) -> Option<String> {
    let (head, args) = match ty {
        Type::Generic { base, args } => {
            let n = names.get(base)?.clone();
            (n.split('<').next()?.trim().to_string(), args)
        }
        Type::UnresolvedGeneric { name, args } => {
            (name.split('<').next()?.trim().to_string(), args)
        }
        _ => return None,
    };
    let filled = match head.as_str() {
        "Vec" | "Pool" => "Vec<i64>".replace("Vec", &head),
        "Map" => "Map<string, i64>".to_string(),
        _ if args.is_empty() => head.clone(),
        _ => format!("{head}<…>"),
    };
    Some(filled)
}

impl TypeChecker {
    pub(super) fn validate_resolved_binding_types(
        &mut self,
        decls: &[Decl],
        node_types: &HashMap<NodeId, Type>,
    ) {
        let names: HashMap<crate::TypeId, String> = self
            .types
            .type_names
            .iter()
            .map(|(n, i)| (*i, n.clone()))
            .collect();
        let mut found = Vec::new();
        for decl in decls {
            walk_decl(decl, node_types, &names, &mut found);
        }
        for err in found {
            self.errors.push(err);
        }
    }
}

fn walk_decl(
    decl: &Decl,
    node_types: &HashMap<NodeId, Type>,
    names: &HashMap<crate::TypeId, String>,
    out: &mut Vec<TypeError>,
) {
    match &decl.kind {
        DeclKind::Fn(f) => walk_body(&f.body, node_types, names, out),
        DeclKind::Impl(i) => {
            for m in &i.methods {
                walk_body(&m.body, node_types, names, out);
            }
        }
        DeclKind::Test(t) => walk_body(&t.body, node_types, names, out),
        _ => {}
    }
}

fn walk_body(
    body: &[Stmt],
    node_types: &HashMap<NodeId, Type>,
    names: &HashMap<crate::TypeId, String>,
    out: &mut Vec<TypeError>,
) {
    for stmt in body {
        walk_stmt(stmt, node_types, names, out);
    }
}

fn walk_stmt(
    stmt: &Stmt,
    node_types: &HashMap<NodeId, Type>,
    names: &HashMap<crate::TypeId, String>,
    out: &mut Vec<TypeError>,
) {
    match &stmt.kind {
        // An annotated binding already said what it is; if inference still
        // disagrees that's a mismatch, reported elsewhere.
        // `let _ = f()` throws the value away — there's nothing to annotate and
        // nothing downstream that could care what it was.
        StmtKind::Let { name, .. } if name == "_" || name.starts_with('_') => {}
        StmtKind::Let { name, name_span, ty: None, init } => {
            if let Some(t) = node_types.get(&init.id) {
                if is_open(t) {
                    out.push(TypeError::UnresolvedType {
                        name: name.clone(),
                        hint: suggest(t, names),
                        span: *name_span,
                    });
                }
            }
            walk_expr(init, node_types, names, out);
        }
        StmtKind::Let { init, .. } => walk_expr(init, node_types, names, out),
        StmtKind::Expr(e) | StmtKind::Return(Some(e)) => walk_expr(e, node_types, names, out),
        StmtKind::While { body, .. }
        | StmtKind::WhileLet { body, .. }
        | StmtKind::Loop { body, .. }
        | StmtKind::For { body, .. }
        | StmtKind::Comptime(body) => walk_body(body, node_types, names, out),
        StmtKind::Ensure { body, else_handler } => {
            walk_body(body, node_types, names, out);
            if let Some((_, handler)) = else_handler {
                walk_body(handler, node_types, names, out);
            }
        }
        _ => {}
    }
}

/// Recurse into the block-bearing expressions, so a binding inside an `if` or a
/// loop body is checked too.
fn walk_expr(
    expr: &Expr,
    node_types: &HashMap<NodeId, Type>,
    names: &HashMap<crate::TypeId, String>,
    out: &mut Vec<TypeError>,
) {
    match &expr.kind {
        ExprKind::Block(body) | ExprKind::Loop { body, .. } | ExprKind::Spawn { body } => {
            walk_body(body, node_types, names, out)
        }
        ExprKind::If { then_branch, else_branch, .. } => {
            walk_expr(then_branch, node_types, names, out);
            if let Some(e) = else_branch {
                walk_expr(e, node_types, names, out);
            }
        }
        _ => {}
    }
}
