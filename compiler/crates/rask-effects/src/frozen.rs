// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Frozen context enforcement (comp.advanced EF4) and missing-frozen lint (FL1).
//!
//! EF4: Functions with `using frozen Pool<T>` that perform Grow or Shrink
//! effects get a compile error.
//!
//! FL1: Public functions with `using Pool<T>` (not frozen) that only Access
//! get a warning suggesting they add `frozen`.

use rask_ast::decl::{Decl, DeclKind, FnDecl};

use crate::EffectMap;

/// A frozen context violation or lint finding.
#[derive(Debug, Clone)]
pub struct FrozenDiagnostic {
    /// Spec rule code.
    pub code: &'static str,
    pub message: String,
    /// Span of the function declaration.
    pub span: rask_ast::Span,
    /// Span of the context clause.
    pub clause_span: Option<rask_ast::Span>,
    pub is_error: bool,
}

/// Check frozen context violations (EF4) and missing-frozen lint (FL1).
pub fn check(decls: &[Decl], effects: &EffectMap) -> Vec<FrozenDiagnostic> {
    let mut results = Vec::new();

    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => check_fn(f, &f.name, effects, &mut results),
            DeclKind::Struct(s) => {
                for m in &s.methods {
                    let qname = format!("{}.{}", s.name, m.name);
                    check_fn(m, &qname, effects, &mut results);
                }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods {
                    let qname = format!("{}.{}", e.name, m.name);
                    check_fn(m, &qname, effects, &mut results);
                }
            }
            DeclKind::Impl(i) => {
                for m in &i.methods {
                    let qname = format!("{}.{}", i.target_ty, m.name);
                    check_fn(m, &qname, effects, &mut results);
                }
            }
            _ => {}
        }
    }

    results
}

fn check_fn(
    f: &FnDecl,
    qname: &str,
    effects: &EffectMap,
    results: &mut Vec<FrozenDiagnostic>,
) {
    let fx = effects.get(qname).copied().unwrap_or_default();

    for clause in &f.context_clauses {
        if !is_pool_context(&clause.ty) {
            continue;
        }

        if clause.is_frozen {
            // EF4: Frozen context with Grow or Shrink is an error
            if fx.grow || fx.shrink {
                let effect_name = if fx.grow && fx.shrink {
                    "Grow and Shrink"
                } else if fx.grow {
                    "Grow"
                } else {
                    "Shrink"
                };
                results.push(FrozenDiagnostic {
                    code: "comp.advanced/EF4",
                    message: format!(
                        "structural mutation in frozen context: `{}` has {} effect but `{}` is frozen",
                        f.name, effect_name, clause.ty,
                    ),
                    span: f.span,
                    clause_span: None,
                    is_error: true,
                });
            }
        } else if f.is_pub && !fx.grow && !fx.shrink {
            // FL1: Public function with pool context that only reads — suggest frozen
            results.push(FrozenDiagnostic {
                code: "comp.advanced/FL1",
                message: format!(
                    "public function `{}` only reads from pool — consider `using frozen {}`",
                    f.name, clause.ty,
                ),
                span: f.span,
                clause_span: None,
                is_error: false,
            });
        }
    }
}

/// Check if a type string refers to a Pool context.
fn is_pool_context(ty: &str) -> bool {
    ty.starts_with("Pool<") || ty == "Pool"
}

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{ContextClause, Decl, DeclKind, FnDecl};
    use rask_ast::expr::{Expr, ExprKind};
    use rask_ast::stmt::{Stmt, StmtKind};
    use rask_ast::{NodeId, Span};
    use std::collections::HashMap;

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn ident(name: &str) -> Expr {
        Expr {
            id: NodeId(0),
            kind: ExprKind::Ident(name.into()),
            span: sp(),
        }
    }

    fn method_call(obj: &str, method: &str) -> Expr {
        Expr {
            id: NodeId(0),
            kind: ExprKind::MethodCall {
                object: Box::new(ident(obj)),
                method: method.into(),
                type_args: None,
                args: vec![],
            },
            span: sp(),
        }
    }

    fn expr_stmt(e: Expr) -> Stmt {
        Stmt {
            id: NodeId(0),
            kind: StmtKind::Expr(e),
            span: sp(),
        }
    }

    fn make_fn_with_clause(
        name: &str,
        is_pub: bool,
        frozen: bool,
        body: Vec<Stmt>,
    ) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.into(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                context_clauses: vec![ContextClause {
                    name: None,
                    ty: "Pool<Entity>".into(),
                    is_frozen: frozen,
                }],
                body,
                is_pub,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: sp(),
            }),
            span: sp(),
        }
    }

    fn effects_with(name: &str, grow: bool, shrink: bool) -> EffectMap {
        let mut m = HashMap::new();
        m.insert(
            name.into(),
            crate::Effects {
                io: false,
                async_: false,
                grow,
                shrink,
            },
        );
        m
    }

    /// EF4: frozen context with pool.remove → error
    #[test]
    fn ef4_frozen_with_remove() {
        let decls = vec![make_fn_with_clause(
            "render",
            true,
            true, // frozen
            vec![expr_stmt(method_call("pool", "remove"))],
        )];
        let effects = effects_with("render", false, true);
        let diags = check(&decls, &effects);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "comp.advanced/EF4");
        assert!(diags[0].is_error);
        assert!(diags[0].message.contains("Shrink"));
    }

    /// EF4: frozen context with pool.insert → error
    #[test]
    fn ef4_frozen_with_insert() {
        let decls = vec![make_fn_with_clause(
            "grow",
            false,
            true,
            vec![expr_stmt(method_call("pool", "insert"))],
        )];
        let effects = effects_with("grow", true, false);
        let diags = check(&decls, &effects);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "comp.advanced/EF4");
        assert!(diags[0].is_error);
    }

    /// Frozen context with only reads → no error
    #[test]
    fn frozen_read_only_is_ok() {
        let decls = vec![make_fn_with_clause(
            "read",
            true,
            true,
            vec![expr_stmt(ident("x"))],
        )];
        let effects = effects_with("read", false, false);
        let diags = check(&decls, &effects);
        assert!(diags.is_empty());
    }

    /// FL1: public function without frozen that only reads → warning
    #[test]
    fn fl1_missing_frozen_warning() {
        let decls = vec![make_fn_with_clause(
            "get_health",
            true,  // public
            false, // NOT frozen
            vec![expr_stmt(ident("x"))],
        )];
        let effects = effects_with("get_health", false, false);
        let diags = check(&decls, &effects);
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].code, "comp.advanced/FL1");
        assert!(!diags[0].is_error);
        assert!(diags[0].message.contains("frozen"));
    }

    /// FL2: private function without frozen that only reads → no warning
    #[test]
    fn fl2_private_exempt() {
        let decls = vec![make_fn_with_clause(
            "helper",
            false, // private
            false, // NOT frozen
            vec![expr_stmt(ident("x"))],
        )];
        let effects = effects_with("helper", false, false);
        let diags = check(&decls, &effects);
        assert!(diags.is_empty(), "FL2: private functions exempt from lint");
    }

    /// Public function with mutations → no FL1 warning
    #[test]
    fn no_fl1_when_mutating() {
        let decls = vec![make_fn_with_clause(
            "cleanup",
            true,
            false,
            vec![expr_stmt(method_call("pool", "remove"))],
        )];
        let effects = effects_with("cleanup", false, true);
        let diags = check(&decls, &effects);
        assert!(diags.is_empty(), "No FL1 when function actually mutates");
    }
}
