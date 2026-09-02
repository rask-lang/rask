// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Synthetic function body generation for auto-derived trait methods.
//!
//! After typechecking confirms which methods are auto-derived (compare, eq,
//! hash, clone), this pass generates actual AST function bodies so they
//! compile through the normal pipeline (mono → MIR → codegen).

use rask_ast::decl::{Decl, DeclKind, FnDecl, ImplDecl, Param};
use rask_ast::expr::{BinOp, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};
use rask_types::TypedProgram;

const DUMMY: Span = Span::new(0, 0);

fn expr(kind: ExprKind) -> Expr {
    Expr { id: NodeId(0), kind, span: DUMMY }
}

fn stmt(kind: StmtKind) -> Stmt {
    Stmt { id: NodeId(0), kind, span: DUMMY }
}

/// `Ordering.Less` and friends — the same AST a hand-written `compare` gets.
fn ordering(variant: &str) -> Expr {
    expr(ExprKind::Field {
        object: Box::new(expr(ExprKind::Ident("Ordering".to_string()))),
        field: variant.to_string(),
    })
}

/// Generate synthetic function bodies for auto-derived methods on structs.
///
/// Currently handles:
/// - `compare`: lexicographic field-by-field comparison returning `Ordering`
///
/// The generated functions are added as Impl declarations to `decls`.
pub fn generate_derived_methods(decls: &mut Vec<Decl>, typed: &TypedProgram) {
    let mut new_impls = Vec::new();

    for type_def in typed.types.iter() {
        match type_def {
            rask_types::TypeDef::Struct { name, fields, methods, .. } => {
                // Annotation declarations register as struct types for
                // `has<A>()` name resolution (type.annotations/AN6) but are
                // comptime-only — no layout exists, so a generated compare
                // would read fields off a type mono never laid out.
                let is_annotation = decls.iter().any(|d| matches!(
                    &d.kind, DeclKind::Annotation(a) if a.name == *name
                ));
                if is_annotation {
                    continue;
                }

                // Check for user-provided compare
                let has_user_compare = decls.iter().any(|d| match &d.kind {
                    DeclKind::Impl(imp) if imp.target_ty == *name => {
                        imp.methods.iter().any(|m| m.name == "compare")
                    }
                    _ => false,
                });

                // Only generate if type checker derived it and no user impl exists
                if !has_user_compare && methods.iter().any(|m| m.name == "compare") {
                    if let Some(fn_decl) = gen_struct_compare(&name, &fields) {
                        new_impls.push(Decl {
                            id: NodeId(0),
                            kind: DeclKind::Impl(ImplDecl {
                                trait_names: vec![],
                                target_ty: name.clone(),
                                methods: vec![fn_decl],
                                is_unsafe: false,
                                is_scoped: false,
                                where_bounds: vec![],
                                doc: None,
                            }),
                            span: DUMMY,
                        });
                    }
                }
            }
            _ => {}
        }
    }

    decls.extend(new_impls);
}

/// Generate a `compare` function body for a struct with the given fields.
///
/// Produces:
/// ```text
/// func compare(self, other: TypeName) -> Ordering {
///     if self.f1 < other.f1 { return Ordering.Less }
///     if self.f1 > other.f1 { return Ordering.Greater }
///     if self.f2 < other.f2 { return Ordering.Less }
///     if self.f2 > other.f2 { return Ordering.Greater }
///     ...
///     return Ordering.Equal
/// }
/// ```
///
/// `Ordering`, not a raw `-1 / 0 / 1`, because that is what `compare` is —
/// `Comparable::compare(self, other: Self) -> Ordering` (type.operators/ORD1),
/// and it is what the checker already tells every caller this returns. The
/// body used to answer with the integers so a C comparator could read the
/// sign; the declared type and the generated one disagreed, and everything
/// that believed the declaration was reading a tag out of an integer.
/// Converting for the sort runtime is the sort lowering's job, at the one
/// boundary that needs it.
fn gen_struct_compare(
    type_name: &str,
    fields: &[(String, rask_types::Type)],
) -> Option<FnDecl> {
    // Only generate for structs with comparable fields
    if fields.is_empty() {
        return None;
    }

    let mut body = Vec::new();

    for (field_name, _field_ty) in fields {
        // self.field
        let self_field = expr(ExprKind::Field {
            object: Box::new(expr(ExprKind::Ident("self".to_string()))),
            field: field_name.clone(),
        });
        // other.field
        let other_field = expr(ExprKind::Field {
            object: Box::new(expr(ExprKind::Ident("other".to_string()))),
            field: field_name.clone(),
        });

        // if self.field < other.field { return Ordering.Less }
        body.push(stmt(StmtKind::Expr(expr(ExprKind::If {
            cond: Box::new(expr(ExprKind::Binary {
                op: BinOp::Lt,
                left: Box::new(self_field.clone()),
                right: Box::new(other_field.clone()),
            })),
            then_branch: Box::new(expr(ExprKind::Block(vec![
                stmt(StmtKind::Return(Some(ordering("Less")))),
            ]))),
            else_branch: None,
            else_binding: None,
        }))));

        // if self.field > other.field { return Ordering.Greater }
        body.push(stmt(StmtKind::Expr(expr(ExprKind::If {
            cond: Box::new(expr(ExprKind::Binary {
                op: BinOp::Gt,
                left: Box::new(self_field),
                right: Box::new(other_field),
            })),
            then_branch: Box::new(expr(ExprKind::Block(vec![
                stmt(StmtKind::Return(Some(ordering("Greater")))),
            ]))),
            else_branch: None,
            else_binding: None,
        }))));
    }

    // return Ordering.Equal
    body.push(stmt(StmtKind::Return(Some(ordering("Equal")))));

    Some(FnDecl {
        name: "compare".to_string(),
        type_params: vec![],
        params: vec![
            Param {
                name: "self".to_string(),
                name_span: DUMMY,
                ty: type_name.to_string(),
                is_take: false,
                is_mutate: false, is_deleting: false,
                default: None,
            },
            Param {
                name: "other".to_string(),
                name_span: DUMMY,
                ty: type_name.to_string(),
                is_take: false,
                is_mutate: false, is_deleting: false,
                default: None,
            },
        ],
        ret_ty: Some("Ordering".to_string()),
        context_clauses: vec![],
        body,
        is_pub: false,
        is_private: false,
        is_comptime: false,
        is_unsafe: false,
        abi: None,
        attrs: vec![],
        doc: None,
        span: DUMMY,
    })
}
