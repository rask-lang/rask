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

/// Generate synthetic function bodies for auto-derived methods on structs.
///
/// Currently handles:
/// - `compare`: lexicographic field-by-field comparison returning i64 (-1, 0, 1)
///
/// The generated functions are added as Impl declarations to `decls`.
pub fn generate_derived_methods(decls: &mut Vec<Decl>, typed: &TypedProgram) {
    let mut new_impls = Vec::new();

    for type_def in typed.types.iter() {
        match type_def {
            rask_types::TypeDef::Struct { name, fields, methods, .. } => {
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
                                trait_name: None,
                                target_ty: name.clone(),
                                methods: vec![fn_decl],
                                is_unsafe: false,
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
/// func compare(self, other: TypeName) -> i64 {
///     if self.f1 < other.f1 { return -1 }
///     if self.f1 > other.f1 { return 1 }
///     if self.f2 < other.f2 { return -1 }
///     if self.f2 > other.f2 { return 1 }
///     ...
///     return 0
/// }
/// ```
///
/// Uses raw i64 return (-1/0/1) rather than Ordering enum because the
/// runtime (sort_by, etc.) operates on i64 values directly.
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

        // if self.field < other.field { return -1 }
        body.push(stmt(StmtKind::Expr(expr(ExprKind::If {
            cond: Box::new(expr(ExprKind::Binary {
                op: BinOp::Lt,
                left: Box::new(self_field.clone()),
                right: Box::new(other_field.clone()),
            })),
            then_branch: Box::new(expr(ExprKind::Block(vec![
                stmt(StmtKind::Return(Some(expr(ExprKind::Unary {
                    op: rask_ast::expr::UnaryOp::Neg,
                    operand: Box::new(expr(ExprKind::Int(1, None))),
                })))),
            ]))),
            else_branch: None,
        }))));

        // if self.field > other.field { return 1 }
        body.push(stmt(StmtKind::Expr(expr(ExprKind::If {
            cond: Box::new(expr(ExprKind::Binary {
                op: BinOp::Gt,
                left: Box::new(self_field),
                right: Box::new(other_field),
            })),
            then_branch: Box::new(expr(ExprKind::Block(vec![
                stmt(StmtKind::Return(Some(expr(ExprKind::Int(1, None))))),
            ]))),
            else_branch: None,
        }))));
    }

    // return 0
    body.push(stmt(StmtKind::Return(Some(expr(ExprKind::Int(0, None))))));

    Some(FnDecl {
        name: "compare".to_string(),
        type_params: vec![],
        params: vec![
            Param {
                name: "self".to_string(),
                name_span: DUMMY,
                ty: type_name.to_string(),
                is_take: false,
                is_mutate: false,
                default: None,
            },
            Param {
                name: "other".to_string(),
                name_span: DUMMY,
                ty: type_name.to_string(),
                is_take: false,
                is_mutate: false,
                default: None,
            },
        ],
        ret_ty: Some("i64".to_string()),
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
