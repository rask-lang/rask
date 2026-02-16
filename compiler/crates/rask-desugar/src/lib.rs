// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Operator desugaring pass for Rask.
//!
//! Transforms binary operators into method calls:
//! - `a + b` → `a.add(b)`
//! - `a - b` → `a.sub(b)`
//! - `a == b` → `a.eq(b)`
//! - etc.
//!
//! This pass runs before type checking.

use rask_ast::decl::{Decl, DeclKind, FnDecl, StructDecl, EnumDecl, TraitDecl, ImplDecl};
use rask_ast::expr::{ArgMode, BinOp, CallArg, Expr, ExprKind, MatchArm, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::NodeId;

/// Desugar all operators in a list of declarations.
pub fn desugar(decls: &mut [Decl]) {
    let mut desugarer = Desugarer::new();
    for decl in decls {
        desugarer.desugar_decl(decl);
    }
}

/// The desugaring context.
struct Desugarer {
    next_id: u32,
}

impl Desugarer {
    fn new() -> Self {
        // Start at a high number to avoid collisions with parser-assigned IDs
        Self { next_id: 1_000_000 }
    }

    fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn desugar_decl(&mut self, decl: &mut Decl) {
        match &mut decl.kind {
            DeclKind::Fn(f) => self.desugar_fn(f),
            DeclKind::Struct(s) => self.desugar_struct(s),
            DeclKind::Enum(e) => self.desugar_enum(e),
            DeclKind::Trait(t) => self.desugar_trait(t),
            DeclKind::Impl(i) => self.desugar_impl(i),
            DeclKind::Const(c) => {
                self.desugar_expr(&mut c.init);
            }
            DeclKind::Test(t) => {
                for stmt in &mut t.body {
                    self.desugar_stmt(stmt);
                }
            }
            DeclKind::Benchmark(b) => {
                for stmt in &mut b.body {
                    self.desugar_stmt(stmt);
                }
            }
            DeclKind::Import(_) => {}
            DeclKind::Export(_) => {}
            DeclKind::Extern(_) => {}
            DeclKind::Package(_) => {}
        }
    }

    fn desugar_fn(&mut self, f: &mut FnDecl) {
        for stmt in &mut f.body {
            self.desugar_stmt(stmt);
        }
    }

    fn desugar_struct(&mut self, s: &mut StructDecl) {
        for method in &mut s.methods {
            self.desugar_fn(method);
        }
    }

    fn desugar_enum(&mut self, e: &mut EnumDecl) {
        for method in &mut e.methods {
            self.desugar_fn(method);
        }
    }

    fn desugar_trait(&mut self, t: &mut TraitDecl) {
        for method in &mut t.methods {
            self.desugar_fn(method);
        }
    }

    fn desugar_impl(&mut self, i: &mut ImplDecl) {
        for method in &mut i.methods {
            self.desugar_fn(method);
        }
    }

    fn desugar_stmt(&mut self, stmt: &mut Stmt) {
        match &mut stmt.kind {
            StmtKind::Expr(e) => self.desugar_expr(e),
            StmtKind::Let { init, .. } => self.desugar_expr(init),
            StmtKind::Const { init, .. } => self.desugar_expr(init),
            StmtKind::LetTuple { init, .. } => self.desugar_expr(init),
            StmtKind::ConstTuple { init, .. } => self.desugar_expr(init),
            StmtKind::Assign { target, value } => {
                self.desugar_expr(target);
                self.desugar_expr(value);
            }
            StmtKind::Return(Some(e)) => self.desugar_expr(e),
            StmtKind::Return(None) => {}
            StmtKind::Break { value: Some(value), .. } => self.desugar_expr(value),
            StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
            StmtKind::While { cond, body } => {
                self.desugar_expr(cond);
                for s in body {
                    self.desugar_stmt(s);
                }
            }
            StmtKind::WhileLet { expr, body, .. } => {
                self.desugar_expr(expr);
                for s in body {
                    self.desugar_stmt(s);
                }
            }
            StmtKind::Loop { body, .. } => {
                for s in body {
                    self.desugar_stmt(s);
                }
            }
            StmtKind::For { iter, body, .. } => {
                self.desugar_expr(iter);
                for s in body {
                    self.desugar_stmt(s);
                }
            }
            StmtKind::Ensure { body, else_handler } => {
                for s in body {
                    self.desugar_stmt(s);
                }
                if let Some((_name, handler)) = else_handler {
                    for s in handler {
                        self.desugar_stmt(s);
                    }
                }
            }
            StmtKind::Comptime(body) => {
                for s in body {
                    self.desugar_stmt(s);
                }
            }
        }
    }

    fn desugar_expr(&mut self, expr: &mut Expr) {
        // First, recursively desugar child expressions
        match &mut expr.kind {
            ExprKind::Binary { left, right, .. } => {
                self.desugar_expr(left);
                self.desugar_expr(right);
            }
            ExprKind::Unary { operand, .. } => {
                self.desugar_expr(operand);
            }
            ExprKind::Call { func, args } => {
                self.desugar_expr(func);
                for arg in args {
                    self.desugar_expr(&mut arg.expr);
                }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.desugar_expr(object);
                for arg in args {
                    self.desugar_expr(&mut arg.expr);
                }
            }
            ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
                self.desugar_expr(object);
            }
            ExprKind::Index { object, index } => {
                self.desugar_expr(object);
                self.desugar_expr(index);
            }
            ExprKind::Block(stmts) => {
                for s in stmts {
                    self.desugar_stmt(s);
                }
            }
            ExprKind::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.desugar_expr(cond);
                self.desugar_expr(then_branch);
                if let Some(e) = else_branch {
                    self.desugar_expr(e);
                }
            }
            ExprKind::IfLet {
                expr,
                then_branch,
                else_branch,
                ..
            } => {
                self.desugar_expr(expr);
                self.desugar_expr(then_branch);
                if let Some(e) = else_branch {
                    self.desugar_expr(e);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.desugar_expr(scrutinee);
                for arm in arms {
                    self.desugar_match_arm(arm);
                }
            }
            ExprKind::Try(e) => self.desugar_expr(e),
            ExprKind::Unwrap { expr: e, message: _ } => self.desugar_expr(e),
            ExprKind::GuardPattern {
                expr,
                else_branch,
                ..
            } => {
                self.desugar_expr(expr);
                self.desugar_expr(else_branch);
            }
            ExprKind::IsPattern { expr, .. } => {
                self.desugar_expr(expr);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.desugar_expr(value);
                self.desugar_expr(default);
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.desugar_expr(s);
                }
                if let Some(e) = end {
                    self.desugar_expr(e);
                }
            }
            ExprKind::StructLit { fields, spread, .. } => {
                for field in fields {
                    self.desugar_expr(&mut field.value);
                }
                if let Some(s) = spread {
                    self.desugar_expr(s);
                }
            }
            ExprKind::Array(elems) => {
                for e in elems {
                    self.desugar_expr(e);
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.desugar_expr(value);
                self.desugar_expr(count);
            }
            ExprKind::Tuple(elems) => {
                for e in elems {
                    self.desugar_expr(e);
                }
            }
            ExprKind::WithAs { bindings, body } => {
                for (expr, _) in bindings {
                    self.desugar_expr(expr);
                }
                for s in body {
                    self.desugar_stmt(s);
                }
            }
            ExprKind::Closure { body, .. } => {
                self.desugar_expr(body);
            }
            ExprKind::Cast { expr: inner, .. } => {
                self.desugar_expr(inner);
            }
            ExprKind::Spawn { body } | ExprKind::Unsafe { body } | ExprKind::BlockCall { body, .. } | ExprKind::Comptime { body } => {
                for s in body {
                    self.desugar_stmt(s);
                }
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                self.desugar_expr(condition);
                if let Some(msg) = message {
                    self.desugar_expr(msg);
                }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &mut arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, .. } => {
                            self.desugar_expr(channel);
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.desugar_expr(channel);
                            self.desugar_expr(value);
                        }
                        rask_ast::expr::SelectArmKind::Default => {}
                    }
                    self.desugar_expr(&mut arm.body);
                }
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args {
                    self.desugar_expr(&mut arg.expr);
                }
                for s in body {
                    self.desugar_stmt(s);
                }
            }
            // Literals and identifiers don't need desugaring
            ExprKind::Int(_, _)
            | ExprKind::Float(_, _)
            | ExprKind::Char(_)
            | ExprKind::Bool(_)
            | ExprKind::Ident(_)
            => {}
            ExprKind::String(_) => {
                // String interpolation desugaring handled below
            }
        }

        // Then, transform operators if applicable
        let span = expr.span;
        if let ExprKind::Binary { op, left, right } = &mut expr.kind {
            if let Some(method) = binary_op_method(*op) {
                // Transform: a op b → a.method(b)
                let left_expr = std::mem::replace(
                    left.as_mut(),
                    Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::Bool(false), // placeholder
                        span,
                    },
                );
                let right_expr = std::mem::replace(
                    right.as_mut(),
                    Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::Bool(false), // placeholder
                        span,
                    },
                );

                // Special case for != which is !a.eq(b)
                if *op == BinOp::Ne {
                    let eq_call = Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::MethodCall {
                            object: Box::new(left_expr),
                            method: "eq".to_string(),
                            type_args: None,
                            args: vec![CallArg { mode: ArgMode::Default, expr: right_expr }],
                        },
                        span,
                    };
                    expr.kind = ExprKind::Unary {
                        op: UnaryOp::Not,
                        operand: Box::new(eq_call),
                    };
                } else {
                    expr.kind = ExprKind::MethodCall {
                        object: Box::new(left_expr),
                        method: method.to_string(),
                        type_args: None,
                        args: vec![CallArg { mode: ArgMode::Default, expr: right_expr }],
                    };
                }
            }
            // And/Or are short-circuiting, leave as binary
        }

        // Transform unary operators
        if let ExprKind::Unary { op, operand } = &mut expr.kind {
            if let Some(method) = unary_op_method(*op) {
                let operand_expr = std::mem::replace(
                    operand.as_mut(),
                    Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::Bool(false),
                        span,
                    },
                );
                expr.kind = ExprKind::MethodCall {
                    object: Box::new(operand_expr),
                    method: method.to_string(),
                    type_args: None,
                    args: vec![],
                };
            }
            // Not and Ref remain as unary
        }

        // Desugar string interpolation: "hello {name}" → "hello ".concat(name.to_string())
        if let ExprKind::String(s) = &expr.kind {
            if s.contains('{') {
                if let Some(desugared) = self.desugar_string_interpolation(s, span) {
                    expr.kind = desugared;
                }
            }
        }
    }

    /// Parse string interpolation and produce a concat chain.
    ///
    /// `"hello {name}, you are {age}"` becomes:
    /// `"hello ".concat(name.to_string()).concat(", you are ").concat(age.to_string())`
    fn desugar_string_interpolation(&mut self, s: &str, span: rask_ast::Span) -> Option<ExprKind> {
        let segments = parse_interpolation_segments(s)?;

        // Build expressions for each segment
        let mut exprs: Vec<Expr> = Vec::new();
        for seg in &segments {
            match seg {
                InterpSegment::Literal(text) => {
                    exprs.push(Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::String(text.clone()),
                        span,
                    });
                }
                InterpSegment::Expr(expr_str) => {
                    // Parse the expression using the real lexer/parser
                    let lex = rask_lexer::Lexer::new(expr_str).tokenize();
                    if !lex.errors.is_empty() {
                        return None; // Parse error — leave as raw string
                    }
                    let mut parser = rask_parser::Parser::new(lex.tokens);
                    let parsed = parser.parse_expr().ok()?;

                    // Wrap in to_string() call
                    let to_string_call = Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::MethodCall {
                            object: Box::new(parsed),
                            method: "to_string".to_string(),
                            type_args: None,
                            args: vec![],
                        },
                        span,
                    };
                    exprs.push(to_string_call);
                }
            }
        }

        if exprs.is_empty() {
            return None;
        }
        if exprs.len() == 1 {
            return Some(exprs.remove(0).kind);
        }

        // Chain with concat: first.concat(second).concat(third)...
        let mut result = exprs.remove(0);
        for seg_expr in exprs {
            result = Expr {
                id: self.fresh_id(),
                kind: ExprKind::MethodCall {
                    object: Box::new(result),
                    method: "concat".to_string(),
                    type_args: None,
                    args: vec![CallArg { mode: ArgMode::Default, expr: seg_expr }],
                },
                span,
            };
        }
        Some(result.kind)
    }

    fn desugar_match_arm(&mut self, arm: &mut MatchArm) {
        if let Some(guard) = &mut arm.guard {
            self.desugar_expr(guard);
        }
        self.desugar_expr(&mut arm.body);
    }
}

/// Map binary operators to method names (if they should be desugared).
fn binary_op_method(op: BinOp) -> Option<&'static str> {
    match op {
        // Arithmetic
        BinOp::Add => Some("add"),
        BinOp::Sub => Some("sub"),
        BinOp::Mul => Some("mul"),
        BinOp::Div => Some("div"),
        BinOp::Mod => Some("rem"),
        // Comparison
        BinOp::Eq => Some("eq"),
        BinOp::Ne => Some("eq"), // Handled specially: !a.eq(b)
        BinOp::Lt => Some("lt"),
        BinOp::Gt => Some("gt"),
        BinOp::Le => Some("le"),
        BinOp::Ge => Some("ge"),
        // Bitwise
        BinOp::BitAnd => Some("bit_and"),
        BinOp::BitOr => Some("bit_or"),
        BinOp::BitXor => Some("bit_xor"),
        BinOp::Shl => Some("shl"),
        BinOp::Shr => Some("shr"),
        // Logical - keep as binary (short-circuiting)
        BinOp::And | BinOp::Or => None,
    }
}

/// Map unary operators to method names (if they should be desugared).
fn unary_op_method(op: UnaryOp) -> Option<&'static str> {
    match op {
        UnaryOp::Neg => Some("neg"),
        UnaryOp::BitNot => Some("bit_not"),
        // Logical not, reference, and deref remain as unary operators
        UnaryOp::Not | UnaryOp::Ref | UnaryOp::Deref => None,
    }
}

/// Segment of an interpolated string.
enum InterpSegment {
    Literal(String),
    Expr(String),
}

/// Parse a string containing `{expr}` interpolation into segments.
///
/// Returns `None` if no interpolation is found.
fn parse_interpolation_segments(s: &str) -> Option<Vec<InterpSegment>> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut chars = s.chars().peekable();
    let mut has_interp = false;

    while let Some(c) = chars.next() {
        if c == '{' {
            has_interp = true;
            if !literal.is_empty() {
                segments.push(InterpSegment::Literal(std::mem::take(&mut literal)));
            }
            let mut expr_str = String::new();
            let mut depth = 1;
            for ch in chars.by_ref() {
                if ch == '{' {
                    depth += 1;
                    expr_str.push(ch);
                } else if ch == '}' {
                    depth -= 1;
                    if depth == 0 { break; }
                    expr_str.push(ch);
                } else {
                    expr_str.push(ch);
                }
            }
            segments.push(InterpSegment::Expr(expr_str));
        } else {
            literal.push(c);
        }
    }
    if !literal.is_empty() {
        segments.push(InterpSegment::Literal(literal));
    }

    if has_interp { Some(segments) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_interpolation_segments() {
        let segs = parse_interpolation_segments("hello {name}").unwrap();
        assert_eq!(segs.len(), 2);
        assert!(matches!(&segs[0], InterpSegment::Literal(s) if s == "hello "));
        assert!(matches!(&segs[1], InterpSegment::Expr(s) if s == "name"));
    }

    #[test]
    fn test_no_interpolation() {
        assert!(parse_interpolation_segments("hello world").is_none());
    }

    #[test]
    fn test_multiple_segments() {
        let segs = parse_interpolation_segments("a {x} b {y} c").unwrap();
        assert_eq!(segs.len(), 5);
    }
}
