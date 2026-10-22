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
use crate::types::Type;

/// Is the open-node census switched on?
pub(crate) fn tracing_open_nodes() -> bool {
    std::env::var_os("RASK_TRACE_OPEN_NODES").is_some()
}

/// Does this type still contain an inference variable anywhere inside it?
pub(crate) fn is_open_type(ty: &Type) -> bool {
    ty.has_unsolved_var()
}

/// A type to show in the suggested annotation. The outer shape is usually
/// known — it's the inside that's open — so `Vec<?>` becomes `Vec<i64>` rather
/// than a bare placeholder, which reads as a real fix instead of a shrug.
fn suggest(ty: &Type, names: &HashMap<crate::TypeId, String>) -> Option<String> {
    let (head, args) = match ty {
        Type::Generic { base, args } => {
            let n = names.get(base)?.clone();
            (n.trim().to_string(), args)
        }
        Type::UnresolvedGeneric { name, args } => {
            (name.trim().to_string(), args)
        }
        _ => return None,
    };
    let filled = match head.as_str() {
        "Vec" => "Vec<i64>".replace("Vec", &head),
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
        // A binding can't hold a slice inside something that outlives the
        // expression (std.strings/S2). A slice bound directly is E0303's
        // `StringSliceStored`, reported where it's made.
        if let Some(slice) = self.types.get_type_id("StringSlice") {
            let is_slice = |t: &Type| match t {
                Type::Named(id) => *id == slice,
                Type::UnresolvedNamed(n) => n == "StringSlice",
                _ => false,
            };
            // A sequence of slices makes them one yield at a time, and a
            // function type names them without storing one, so neither holds
            // any: `let words = line.split(" ")` is a lazy loop, not storage.
            let sequence = self.types.get_type_id("Sequence");
            fn stores(t: &Type, is_slice: &dyn Fn(&Type) -> bool, sequence: Option<crate::TypeId>) -> bool {
                match t {
                    _ if is_slice(t) => true,
                    Type::Generic { base, .. } if Some(*base) == sequence => false,
                    Type::UnresolvedGeneric { name, .. } if name == "Sequence" => false,
                    Type::Fn { .. } => false,
                    Type::Result { ok, err } => stores(ok, is_slice, sequence) || stores(err, is_slice, sequence),
                    Type::RawPtr(inner) => stores(inner, is_slice, sequence),
                    Type::Array { elem, .. } | Type::SimdVector { elem, .. } => stores(elem, is_slice, sequence),
                    Type::Tuple(elems) | Type::Union(elems) => elems.iter().any(|e| stores(e, is_slice, sequence)),
                    Type::Generic { args, .. } | Type::UnresolvedGeneric { args, .. } => args
                        .iter()
                        .any(|a| matches!(a, crate::GenericArg::Type(t) if stores(t, is_slice, sequence))),
                    _ => false,
                }
            }
            let holds = |t: &Type| !is_slice(t) && stores(t, &is_slice, sequence);
            for decl in decls {
                held_slices(decl, node_types, &holds, &|t| self.types.display_type_names(t).to_string(), &mut found);
            }
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
                if t.has_unsolved_var() {
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
        ExprKind::Block(body) | ExprKind::Loop { body, .. } => {
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

/// `let`/`mut` bindings whose type holds a slice somewhere inside it.
fn held_slices(
    decl: &Decl,
    node_types: &HashMap<NodeId, Type>,
    holds: &dyn Fn(&Type) -> bool,
    show: &dyn Fn(&Type) -> String,
    out: &mut Vec<TypeError>,
) {
    struct Lets<'a> {
        node_types: &'a HashMap<NodeId, Type>,
        holds: &'a dyn Fn(&Type) -> bool,
        show: &'a dyn Fn(&Type) -> String,
        out: &'a mut Vec<TypeError>,
    }
    impl<'d> rask_ast::visit::Visit<'d> for Lets<'_> {
        fn expr(&mut self, _: &'d Expr) -> bool {
            true
        }
        fn stmt(&mut self, s: &'d Stmt) -> bool {
            if let StmtKind::Let { name, name_span, init, .. } | StmtKind::Mut { name, name_span, init, .. } = &s.kind {
                if let Some(t) = self.node_types.get(&init.id) {
                    if (self.holds)(t) {
                        self.out.push(TypeError::SliceHeld {
                            name: name.clone(),
                            held: (self.show)(t),
                            span: *name_span,
                        });
                    }
                }
            }
            true
        }
    }
    rask_ast::visit::visit_decl(decl, &mut Lets { node_types, holds, show, out });
}
