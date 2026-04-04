// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Effect-based compiler warnings (comp.effects/CW1, CW2).
//!
//! CW1: IO function called inside ThreadPool.spawn — blocks pool thread
//! CW2: IO function called in a loop without `using Multitasking` context

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::expr::{Expr, ExprKind};
use rask_ast::stmt::{Stmt, StmtKind};

use crate::{EffectMap, EffectWarning};

/// Detect CW1 and CW2 warnings from declarations.
pub fn detect(decls: &[Decl], effects: &EffectMap) -> Vec<EffectWarning> {
    let mut warnings = Vec::new();
    let mut ctx = WarnContext { effects, in_thread_pool: false, in_loop: false, in_multitasking: false };

    for decl in decls {
        match &decl.kind {
            DeclKind::Fn(f) => ctx.check_fn(f, &mut warnings),
            DeclKind::Struct(s) => {
                for m in &s.methods { ctx.check_fn(m, &mut warnings); }
            }
            DeclKind::Enum(e) => {
                for m in &e.methods { ctx.check_fn(m, &mut warnings); }
            }
            DeclKind::Impl(i) => {
                for m in &i.methods { ctx.check_fn(m, &mut warnings); }
            }
            DeclKind::Trait(t) => {
                for m in &t.methods { ctx.check_fn(m, &mut warnings); }
            }
            _ => {}
        }
    }

    warnings
}

struct WarnContext<'a> {
    effects: &'a EffectMap,
    in_thread_pool: bool,
    in_loop: bool,
    in_multitasking: bool,
}

impl<'a> WarnContext<'a> {
    fn check_fn(&mut self, f: &FnDecl, warnings: &mut Vec<EffectWarning>) {
        // Reset context per function
        self.in_thread_pool = false;
        self.in_loop = false;
        self.in_multitasking = false;
        self.check_stmts(&f.body, warnings);
    }

    fn check_stmts(&mut self, stmts: &[Stmt], warnings: &mut Vec<EffectWarning>) {
        for stmt in stmts {
            self.check_stmt(stmt, warnings);
        }
    }

    fn check_stmt(&mut self, stmt: &Stmt, warnings: &mut Vec<EffectWarning>) {
        match &stmt.kind {
            StmtKind::Expr(e) => self.check_expr(e, warnings),
            StmtKind::Let { init, .. } | StmtKind::Const { init, .. } => {
                self.check_expr(init, warnings);
            }
            StmtKind::LetTuple { init, .. } | StmtKind::ConstTuple { init, .. } => {
                self.check_expr(init, warnings);
            }
            StmtKind::Assign { target, value } => {
                self.check_expr(target, warnings);
                self.check_expr(value, warnings);
            }
            StmtKind::Return(Some(e)) => self.check_expr(e, warnings),
            StmtKind::Return(None) => {}
            StmtKind::Break { value: Some(v), .. } => self.check_expr(v, warnings),
            StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}

            // CW2: Track loop context
            StmtKind::While { cond, body } => {
                self.check_expr(cond, warnings);
                let was_in_loop = self.in_loop;
                self.in_loop = true;
                self.check_stmts(body, warnings);
                self.in_loop = was_in_loop;
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.check_expr(expr, warnings);
                let was_in_loop = self.in_loop;
                self.in_loop = true;
                self.check_stmts(body, warnings);
                self.in_loop = was_in_loop;
            }
            StmtKind::Loop { body, .. } => {
                let was_in_loop = self.in_loop;
                self.in_loop = true;
                self.check_stmts(body, warnings);
                self.in_loop = was_in_loop;
            }
            StmtKind::For { iter, body, .. } => {
                self.check_expr(iter, warnings);
                let was_in_loop = self.in_loop;
                self.in_loop = true;
                self.check_stmts(body, warnings);
                self.in_loop = was_in_loop;
            }

            StmtKind::Ensure { body, else_handler } => {
                self.check_stmts(body, warnings);
                if let Some((_, handler)) = else_handler {
                    self.check_stmts(handler, warnings);
                }
            }
            StmtKind::Comptime(body) => self.check_stmts(body, warnings),
            StmtKind::Discard { .. } => {}
        }
    }

    fn check_expr(&mut self, expr: &Expr, warnings: &mut Vec<EffectWarning>) {
        match &expr.kind {
            ExprKind::Call { func, args } => {
                let callee_name = extract_callee_name(func);
                if let Some(ref name) = callee_name {
                    self.maybe_warn_io_call(name, expr.span, warnings);
                }
                self.check_expr(func, warnings);
                for arg in args {
                    self.check_expr(&arg.expr, warnings);
                }
            }

            ExprKind::MethodCall { object, method, args, .. } => {
                // Check qualified form
                if let ExprKind::Ident(type_name) = &object.kind {
                    let qname = format!("{}.{}", type_name, method);
                    self.maybe_warn_io_call(&qname, expr.span, warnings);
                }
                self.maybe_warn_io_call(method, expr.span, warnings);
                self.check_expr(object, warnings);
                for arg in args {
                    self.check_expr(&arg.expr, warnings);
                }
            }

            // CW1: ThreadPool.spawn — track context for body
            ExprKind::UsingBlock { name, args, body } if is_thread_pool(name) => {
                for arg in args {
                    self.check_expr(&arg.expr, warnings);
                }
                let was_in_tp = self.in_thread_pool;
                self.in_thread_pool = true;
                self.check_stmts(body, warnings);
                self.in_thread_pool = was_in_tp;
            }

            // Track Multitasking context (suppresses CW2)
            ExprKind::UsingBlock { name, args, body } if is_multitasking(name) => {
                for arg in args {
                    self.check_expr(&arg.expr, warnings);
                }
                let was_mt = self.in_multitasking;
                self.in_multitasking = true;
                self.check_stmts(body, warnings);
                self.in_multitasking = was_mt;
            }

            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args {
                    self.check_expr(&arg.expr, warnings);
                }
                self.check_stmts(body, warnings);
            }

            // BlockCall: ThreadPool.spawn(|| { ... }) parsed as BlockCall
            ExprKind::BlockCall { name, body } if is_thread_pool(name) => {
                let was_in_tp = self.in_thread_pool;
                self.in_thread_pool = true;
                self.check_stmts(body, warnings);
                self.in_thread_pool = was_in_tp;
            }

            ExprKind::Spawn { body } => self.check_stmts(body, warnings),

            // Recurse into other expressions
            ExprKind::Binary { left, right, .. } => {
                self.check_expr(left, warnings);
                self.check_expr(right, warnings);
            }
            ExprKind::Unary { operand, .. } => self.check_expr(operand, warnings),
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                self.check_expr(object, warnings);
            }
            ExprKind::Index { object, index } => {
                self.check_expr(object, warnings);
                self.check_expr(index, warnings);
            }
            ExprKind::Block(stmts) => self.check_stmts(stmts, warnings),
            ExprKind::If { cond, then_branch, else_branch } => {
                self.check_expr(cond, warnings);
                self.check_expr(then_branch, warnings);
                if let Some(e) = else_branch { self.check_expr(e, warnings); }
            }
            ExprKind::IfLet { expr, then_branch, else_branch, .. } => {
                self.check_expr(expr, warnings);
                self.check_expr(then_branch, warnings);
                if let Some(e) = else_branch { self.check_expr(e, warnings); }
            }
            ExprKind::GuardPattern { expr, else_branch, .. } => {
                self.check_expr(expr, warnings);
                self.check_expr(else_branch, warnings);
            }
            ExprKind::IsPattern { expr, .. } => self.check_expr(expr, warnings),
            ExprKind::Match { scrutinee, arms } => {
                self.check_expr(scrutinee, warnings);
                for arm in arms {
                    if let Some(g) = &arm.guard { self.check_expr(g, warnings); }
                    self.check_expr(&arm.body, warnings);
                }
            }
            ExprKind::Try { expr: e, else_clause } => {
                self.check_expr(e, warnings);
                if let Some(ec) = else_clause {
                    self.check_expr(&ec.body, warnings);
                }
            }
            ExprKind::Unwrap { expr: e, .. } | ExprKind::Cast { expr: e, .. } => {
                self.check_expr(e, warnings);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.check_expr(value, warnings);
                self.check_expr(default, warnings);
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start { self.check_expr(s, warnings); }
                if let Some(e) = end { self.check_expr(e, warnings); }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for f in fields { self.check_expr(&f.value, warnings); }
                if let Some(s) = spread { self.check_expr(s, warnings); }
            }
            ExprKind::Array(elems) | ExprKind::Tuple(elems) => {
                for e in elems { self.check_expr(e, warnings); }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.check_expr(value, warnings);
                self.check_expr(count, warnings);
            }
            ExprKind::WithAs { bindings, body } => {
                for b in bindings { self.check_expr(&b.source, warnings); }
                self.check_stmts(body, warnings);
            }
            ExprKind::Closure { body, .. } => self.check_expr(body, warnings),
            ExprKind::Unsafe { body } | ExprKind::Comptime { body }
            | ExprKind::BlockCall { body, .. } | ExprKind::Loop { body, .. } => {
                self.check_stmts(body, warnings);
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                self.check_expr(condition, warnings);
                if let Some(m) = message { self.check_expr(m, warnings); }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                            self.check_expr(channel, warnings);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.check_expr(channel, warnings);
                            self.check_expr(value, warnings);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    self.check_expr(&arm.body, warnings);
                }
            }
            // Leaves
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_)
            | ExprKind::Char(_) | ExprKind::Bool(_) | ExprKind::Null
            | ExprKind::Ident(_) => {}
        }
    }

    /// Check if a callee has IO effects and we're in a warning context.
    fn maybe_warn_io_call(
        &self,
        callee: &str,
        span: rask_ast::Span,
        warnings: &mut Vec<EffectWarning>,
    ) {
        let has_io = self.effects.get(callee)
            .map_or(false, |e| e.io)
            || crate::sources::classify_call(callee).io;

        if !has_io {
            return;
        }

        // CW1: IO in ThreadPool context
        if self.in_thread_pool {
            warnings.push(EffectWarning {
                code: "comp.effects/CW1",
                message: format!(
                    "I/O function `{}` called in thread pool context — blocks pool thread",
                    callee,
                ),
                span,
                callee_name: callee.to_string(),
            });
        }

        // CW2: IO in loop without Multitasking
        if self.in_loop && !self.in_multitasking {
            warnings.push(EffectWarning {
                code: "comp.effects/CW2",
                message: format!(
                    "I/O function `{}` in loop without `using Multitasking` — blocks thread on each iteration",
                    callee,
                ),
                span,
                callee_name: callee.to_string(),
            });
        }
    }
}

fn is_thread_pool(name: &str) -> bool {
    name == "ThreadPool" || name == "thread_pool"
}

fn is_multitasking(name: &str) -> bool {
    name == "Multitasking" || name == "multitasking"
}

fn extract_callee_name(func: &Expr) -> Option<String> {
    match &func.kind {
        ExprKind::Ident(name) => Some(name.clone()),
        ExprKind::Field { object, field } => {
            if let ExprKind::Ident(obj_name) = &object.kind {
                Some(format!("{}.{}", obj_name, field))
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{Decl, DeclKind, FnDecl};
    use rask_ast::expr::{Expr, ExprKind};
    use rask_ast::stmt::{Stmt, StmtKind, ForBinding};
    use rask_ast::{NodeId, Span};
    use std::collections::HashMap;

    fn sp() -> Span { Span::new(0, 0) }

    fn ident(name: &str) -> Expr {
        Expr { id: NodeId(0), kind: ExprKind::Ident(name.into()), span: sp() }
    }

    fn call(func_name: &str) -> Expr {
        Expr {
            id: NodeId(0),
            kind: ExprKind::Call {
                func: Box::new(ident(func_name)),
                args: vec![],
            },
            span: sp(),
        }
    }

    fn field_call(obj: &str, field: &str) -> Expr {
        Expr {
            id: NodeId(0),
            kind: ExprKind::Call {
                func: Box::new(Expr {
                    id: NodeId(0),
                    kind: ExprKind::Field {
                        object: Box::new(ident(obj)),
                        field: field.into(),
                    },
                    span: sp(),
                }),
                args: vec![],
            },
            span: sp(),
        }
    }

    fn expr_stmt(e: Expr) -> Stmt {
        Stmt { id: NodeId(0), kind: StmtKind::Expr(e), span: sp() }
    }

    fn make_fn(name: &str, body: Vec<Stmt>) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.into(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                context_clauses: vec![],
                body,
                is_pub: false,
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

    fn effects_with_io(name: &str) -> EffectMap {
        let mut m = HashMap::new();
        m.insert(name.into(), crate::Effects { io: true, async_: false, grow: false, shrink: false });
        m
    }

    #[test]
    fn cw1_io_in_thread_pool() {
        // ThreadPool.spawn { println() }
        let body = vec![expr_stmt(Expr {
            id: NodeId(0),
            kind: ExprKind::UsingBlock {
                name: "ThreadPool".into(),
                args: vec![],
                body: vec![expr_stmt(call("println"))],
            },
            span: sp(),
        })];
        let decls = vec![make_fn("worker", body)];
        let effects = effects_with_io("println");
        let warnings = detect(&decls, &effects);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "comp.effects/CW1");
        assert!(warnings[0].message.contains("println"));
    }

    #[test]
    fn cw2_io_in_loop_without_multitasking() {
        // for x in items { println() }
        let body = vec![Stmt {
            id: NodeId(0),
            kind: StmtKind::For {
                label: None,
                binding: ForBinding::Single("x".into()),
                mutate: false,
                iter: ident("items"),
                body: vec![expr_stmt(call("println"))],
            },
            span: sp(),
        }];
        let decls = vec![make_fn("process", body)];
        let effects = effects_with_io("println");
        let warnings = detect(&decls, &effects);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "comp.effects/CW2");
    }

    #[test]
    fn no_cw2_with_multitasking_context() {
        // using Multitasking { for x in items { println() } }
        let loop_body = vec![Stmt {
            id: NodeId(0),
            kind: StmtKind::For {
                label: None,
                binding: ForBinding::Single("x".into()),
                mutate: false,
                iter: ident("items"),
                body: vec![expr_stmt(call("println"))],
            },
            span: sp(),
        }];
        let body = vec![expr_stmt(Expr {
            id: NodeId(0),
            kind: ExprKind::UsingBlock {
                name: "Multitasking".into(),
                args: vec![],
                body: loop_body,
            },
            span: sp(),
        })];
        let decls = vec![make_fn("process", body)];
        let effects = effects_with_io("println");
        let warnings = detect(&decls, &effects);
        assert!(warnings.is_empty(), "No CW2 inside Multitasking context");
    }

    #[test]
    fn no_warning_for_pure_call_in_loop() {
        let body = vec![Stmt {
            id: NodeId(0),
            kind: StmtKind::For {
                label: None,
                binding: ForBinding::Single("x".into()),
                mutate: false,
                iter: ident("items"),
                body: vec![expr_stmt(call("add"))],
            },
            span: sp(),
        }];
        let decls = vec![make_fn("process", body)];
        let effects = HashMap::new(); // add has no effects
        let warnings = detect(&decls, &effects);
        assert!(warnings.is_empty());
    }

    #[test]
    fn cw1_with_file_read() {
        let body = vec![expr_stmt(Expr {
            id: NodeId(0),
            kind: ExprKind::UsingBlock {
                name: "ThreadPool".into(),
                args: vec![],
                body: vec![expr_stmt(field_call("File", "read"))],
            },
            span: sp(),
        })];
        let decls = vec![make_fn("bad", body)];
        let effects = effects_with_io("File.read");
        let warnings = detect(&decls, &effects);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "comp.effects/CW1");
    }
}
