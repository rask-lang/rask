// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! A `.view()` or `.span()` at the end of a slice chain, without the copies.
//!
//! A slice (`s[i..j]`, `s.trim()`) is expression-scoped (std.strings/S2), and
//! as a value it's a copy of the bytes it covers. So `.view()` on it shared the
//! copy rather than the source, which is not the zero-copy view V1 promises,
//! and `.span()` on it had forgotten where it was cut from. The chain is
//! compiled into span arithmetic on the source and one call at the end:
//!
//! ```text
//! s[i..j].span()                →  s.span_of(i, j)
//! line[0..c].trim().view()      →  line.view_of(line.trim_span(line.span_of(0, c)))
//! s.trim_end().span()           →  s.trim_end_span(s.span())
//! ```
//!
//! A source that could do something when evaluated is bound to a temporary
//! first, so it runs once. A chain on anything other than a string fails type
//! checking at the method it was compiled into, the same as it would have at
//! the slice.

use rask_ast::expr::{ArgMode, CallArg, Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::token::IntSuffix;

use crate::Desugarer;

/// One link of a slice chain, innermost first.
enum Link {
    Range { start: Option<Expr>, end: Option<Expr>, inclusive: bool },
    Trim(&'static str),
}

fn trim_span_method(method: &str) -> Option<&'static str> {
    match method {
        "trim" => Some("trim_span"),
        "trim_start" => Some("trim_start_span"),
        "trim_end" => Some("trim_end_span"),
        _ => None,
    }
}

impl Desugarer {
    /// Rewrite `expr` if it's a slice chain ending in `.view()` or `.span()`.
    pub(crate) fn rewrite_slice_chain(&mut self, expr: &mut Expr) {
        let ExprKind::MethodCall { object, method, type_args: None, args } = &expr.kind else {
            return;
        };
        if !args.is_empty() || !matches!(method.as_str(), "view" | "span") {
            return;
        }
        let ends_in_view = method == "view";
        // Walk down to the source, collecting links outermost first.
        let mut links = Vec::new();
        let mut at: &Expr = object;
        loop {
            match &at.kind {
                ExprKind::Index { object, index } => match &index.kind {
                    ExprKind::Range { start, end, inclusive } => {
                        links.push(Link::Range {
                            start: start.as_deref().cloned(),
                            end: end.as_deref().cloned(),
                            inclusive: *inclusive,
                        });
                        at = object;
                    }
                    _ => break,
                },
                ExprKind::MethodCall { object, method, type_args: None, args } if args.is_empty() => {
                    match trim_span_method(method) {
                        Some(m) => {
                            links.push(Link::Trim(m));
                            at = object;
                        }
                        None => break,
                    }
                }
                _ => break,
            }
        }
        if links.is_empty() {
            return;
        }
        links.reverse();
        let span = expr.span;
        let source = at.clone();

        let mut stmts = Vec::new();
        let base = if Self::reads_without_effect(&source) {
            source
        } else {
            let id = self.fresh_id();
            let name = format!("__slice_{}", id.0);
            stmts.push(Stmt {
                id,
                kind: StmtKind::Let { name: name.clone(), name_span: span, ty: None, init: source },
                span,
            });
            self.ident(&name, span)
        };

        // The chain's range so far, as an expression over `base`.
        let mut range = match links.first() {
            Some(Link::Range { .. }) => None,
            _ => Some(self.call_on(&base, "span", vec![], span)),
        };
        for link in links {
            range = Some(match link {
                Link::Range { start, end, inclusive } => {
                    // A range after a trim slices the trimmed text, which no
                    // span call says. Leave that chain as written.
                    if range.is_some() {
                        return;
                    }
                    let start = start.unwrap_or_else(|| self.usize_lit(0, span));
                    let end = match end {
                        Some(e) if inclusive => {
                            let one = self.usize_lit(1, span);
                            Expr {
                                id: self.fresh_id(),
                                kind: ExprKind::Binary {
                                    op: rask_ast::expr::BinOp::Add,
                                    left: Box::new(e),
                                    right: Box::new(one),
                                },
                                span,
                            }
                        }
                        Some(e) => e,
                        None => Expr { id: self.fresh_id(), kind: ExprKind::None, span },
                    };
                    self.call_on(&base, "span_of", vec![start, end], span)
                }
                Link::Trim(m) => {
                    let inner = range.take().expect("a trim follows a range or the whole string");
                    self.call_on(&base, m, vec![inner], span)
                }
            });
        }
        let range = range.expect("a chain has at least one link");
        let result = if ends_in_view { self.call_on(&base, "view_of", vec![range], span) } else { range };

        if stmts.is_empty() {
            *expr = result;
        } else {
            stmts.push(Stmt { id: self.fresh_id(), kind: StmtKind::Expr(result), span });
            *expr = Expr { id: self.fresh_id(), kind: ExprKind::Block(stmts), span };
        }
    }

    /// `object.method(args)`, with `object` copied under fresh ids: the
    /// source appears once per link, and each copy is its own node.
    fn call_on(&mut self, object: &Expr, method: &str, args: Vec<Expr>, span: rask_ast::Span) -> Expr {
        let mut object = object.clone();
        rask_ast::rewrite::rewrite_expr(&mut object, &mut crate::FreshIds(&mut self.next_id));
        Expr {
            id: self.fresh_id(),
            kind: ExprKind::MethodCall {
                object: Box::new(object),
                method: method.to_string(),
                type_args: None,
                args: args.into_iter().map(|expr| CallArg { name: None, mode: ArgMode::Default, expr }).collect(),
            },
            span,
        }
    }

    fn ident(&mut self, name: &str, span: rask_ast::Span) -> Expr {
        Expr { id: self.fresh_id(), kind: ExprKind::Ident(name.to_string()), span }
    }

    fn usize_lit(&mut self, n: i128, span: rask_ast::Span) -> Expr {
        Expr { id: self.fresh_id(), kind: ExprKind::Int(n, Some(IntSuffix::Usize)), span }
    }
}
