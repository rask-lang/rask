// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Desugaring passes for Rask.
//!
//! Operator desugaring transforms binary operators into method calls:
//! - `a + b` → `a.add(b)`
//! - `a - b` → `a.sub(b)`
//! - `a == b` → `a.eq(b)`
//! - etc.
//!
//! Default argument desugaring fills in missing call arguments from
//! parameter defaults and resolves named arguments to positional form.
//!
//! These passes run before type checking.

mod defaults;
pub use defaults::{desugar_default_args, is_valid_default_expr};

use rask_ast::decl::{Decl, DeclKind, FnDecl, Param, StructDecl, EnumDecl, TraitDecl, ImplDecl};
use rask_ast::expr::{ArgMode, BinOp, CallArg, Expr, ExprKind, MatchArm, Pattern, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};

/// Desugar all operators in a list of declarations.
pub fn desugar(decls: &mut [Decl]) {
    let mut desugarer = Desugarer::new(1_000_000);
    for decl in decls {
        desugarer.desugar_decl(decl);
    }
}

/// Desugar with a custom starting NodeId to avoid collisions.
pub fn desugar_with_start_id(decls: &mut [Decl], start_id: u32) {
    let mut desugarer = Desugarer::new(start_id);
    for decl in decls {
        desugarer.desugar_decl(decl);
    }
}

/// ER26 coverage error from @message desugaring.
#[derive(Debug, Clone)]
pub struct DesugarError {
    pub message: String,
    pub span: Span,
}

/// Desugar all operators, returning any ER26 coverage errors.
pub fn desugar_with_diagnostics(decls: &mut [Decl]) -> Vec<DesugarError> {
    let mut desugarer = Desugarer::new(1_000_000);
    for decl in decls {
        desugarer.desugar_decl(decl);
    }
    desugarer.errors
}

/// The desugaring context.
struct Desugarer {
    next_id: u32,
    errors: Vec<DesugarError>,
}

impl Desugarer {
    fn new(start_id: u32) -> Self {
        Self { next_id: start_id, errors: Vec::new() }
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
            DeclKind::Union(_) => {}
            DeclKind::TypeAlias(_) => {}
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
        // Generate message() method if @message attribute is present
        if e.attrs.iter().any(|a| a == "message") {
            if let Some(method) = self.generate_message_method(e) {
                e.methods.push(method);
            }
        }
        for method in &mut e.methods {
            self.desugar_fn(method);
        }
    }

    /// Generate `func message(self) -> string` from @message annotations.
    fn generate_message_method(&mut self, e: &EnumDecl) -> Option<FnDecl> {
        let sp = Span::new(0, 0);
        let mut arms = Vec::new();

        for variant in &e.variants {
            let template = match self.extract_message_template(variant) {
                Some(t) => t,
                None => {
                    // ER26: missing coverage — record error, use variant name as fallback
                    self.errors.push(DesugarError {
                        message: format!(
                            "@message variant `{}` on `{}` has no message template and cannot auto-delegate",
                            variant.name, e.name
                        ),
                        span: sp,
                    });
                    MessageTemplate::Format(variant.name.clone())
                }
            };

            // Build pattern bindings for this variant
            let field_patterns: Vec<Pattern> = if variant.fields.is_empty() {
                vec![]
            } else {
                variant.fields.iter().map(|f| {
                    Pattern::Ident(f.name.clone())
                }).collect()
            };

            let pattern = if variant.fields.is_empty() {
                Pattern::Ident(variant.name.clone())
            } else {
                Pattern::Constructor {
                    name: variant.name.clone(),
                    fields: field_patterns,
                }
            };

            let body_expr = match template {
                MessageTemplate::Format(tmpl) => {
                    // String with interpolation — desugaring pass handles {name}
                    Expr { id: self.fresh_id(), kind: ExprKind::String(tmpl), span: sp }
                }
                MessageTemplate::Delegate(binding) => {
                    // e.message() — delegate to inner error
                    Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::MethodCall {
                            object: Box::new(Expr {
                                id: self.fresh_id(),
                                kind: ExprKind::Ident(binding),
                                span: sp,
                            }),
                            method: "message".to_string(),
                            type_args: None,
                            args: vec![],
                        },
                        span: sp,
                    }
                }
            };

            arms.push(MatchArm {
                pattern,
                guard: None,
                body: Box::new(body_expr),
            });
        }

        let match_expr = Expr {
            id: self.fresh_id(),
            kind: ExprKind::Match {
                scrutinee: Box::new(Expr {
                    id: self.fresh_id(),
                    kind: ExprKind::Ident("self".to_string()),
                    span: sp,
                }),
                arms,
            },
            span: sp,
        };

        let return_stmt = Stmt {
            id: self.fresh_id(),
            kind: StmtKind::Return(Some(match_expr)),
            span: sp,
        };

        Some(FnDecl {
            name: "message".to_string(),
            type_params: vec![],
            params: vec![Param {
                name: "self".to_string(),
                name_span: sp,
                ty: "Self".to_string(),
                is_take: false,
                is_mutate: false,
                default: None,
            }],
            ret_ty: Some("string".to_string()),
            context_clauses: vec![],
            body: vec![return_stmt],
            is_pub: true,
            is_private: false,
            is_comptime: false,
            is_unsafe: false,
            abi: None,
            attrs: vec![],
            doc: None,
            span: sp,
        })
    }

    /// Extract the message template for a variant.
    ///
    /// ER24: explicit @message("template") on the variant.
    /// ER25: single-field variant with Error-typed payload auto-delegates to inner.message().
    /// ER26: variants without coverage return None (caller must handle).
    fn extract_message_template(&self, variant: &rask_ast::decl::Variant) -> Option<MessageTemplate> {
        // Check for @message("template") on the variant
        for attr in &variant.attrs {
            if let Some(tmpl) = extract_message_attr_template(attr) {
                return Some(MessageTemplate::Format(tmpl));
            }
        }
        // No-payload variants use the variant name as a reasonable default
        if variant.fields.is_empty() {
            return Some(MessageTemplate::Format(variant.name.clone()));
        }
        // ER25: auto-delegate for single-field variants with Error-typed payload
        if variant.fields.len() == 1 && is_error_type_name(&variant.fields[0].ty) {
            return Some(MessageTemplate::Delegate(variant.fields[0].name.clone()));
        }
        // ER26: missing coverage — caller should report error
        None
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
            StmtKind::ComptimeFor { iter, body, .. } => {
                self.desugar_expr(iter);
                for s in body {
                    self.desugar_stmt(s);
                }
            }
            StmtKind::Discard { .. } => {}
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
            ExprKind::DynamicField { object, field_expr } => {
                self.desugar_expr(object);
                self.desugar_expr(field_expr);
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
            ExprKind::Try { expr: e, ref mut else_clause } => {
                self.desugar_expr(e);
                if let Some(ec) = else_clause {
                    self.desugar_expr(&mut ec.body);
                }
            }
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
                for binding in bindings {
                    self.desugar_expr(&mut binding.source);
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
            ExprKind::Spawn { body } | ExprKind::Unsafe { body } | ExprKind::BlockCall { body, .. }
            | ExprKind::Comptime { body } | ExprKind::Loop { body, .. } => {
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
            | ExprKind::Null
            => {}
            ExprKind::String(_) => {
                // String interpolation desugaring handled below
            }
        }

        // Then, transform operators if applicable
        let span = expr.span;
        if matches!(&expr.kind, ExprKind::Binary { op, .. } if binary_op_method(*op).is_some()) {
            // Take ownership of the entire Binary node to avoid placeholder values
            let old = std::mem::replace(&mut expr.kind, ExprKind::Bool(false));
            if let ExprKind::Binary { op, left, right } = old {
                let method = binary_op_method(op).unwrap();
                let left_expr = *left;
                let right_expr = *right;

                // Special case for != which is !a.eq(b)
                if op == BinOp::Ne {
                    let eq_call = Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::MethodCall {
                            object: Box::new(left_expr),
                            method: "eq".to_string(),
                            type_args: None,
                            args: vec![CallArg { name: None, mode: ArgMode::Default, expr: right_expr }],
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
                        args: vec![CallArg { name: None, mode: ArgMode::Default, expr: right_expr }],
                    };
                }
            }
        } else if matches!(&expr.kind, ExprKind::Binary { .. }) {
            // And/Or are short-circuiting, leave as binary
        }

        // Transform unary operators
        if matches!(&expr.kind, ExprKind::Unary { op, .. } if unary_op_method(*op).is_some()) {
            let old = std::mem::replace(&mut expr.kind, ExprKind::Bool(false));
            if let ExprKind::Unary { op, operand } = old {
                let method = unary_op_method(op).unwrap();
                expr.kind = ExprKind::MethodCall {
                    object: operand,
                    method: method.to_string(),
                    type_args: None,
                    args: vec![],
                };
            }
        }
        // Not and Ref remain as unary

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
                InterpSegment::Expr(expr_str, offset_in_str) => {
                    // Parse the expression using the real lexer/parser
                    let lex = rask_lexer::Lexer::new(expr_str).tokenize();
                    if !lex.errors.is_empty() {
                        return None; // Parse error — leave as raw string
                    }
                    let mut parser = rask_parser::Parser::new(lex.tokens);
                    let mut parsed = parser.parse_expr().ok()?;

                    // Remap spans from 0-based (within expr_str) to absolute file position.
                    // span.start is the opening quote, +1 for the content start, +offset for position within content.
                    let abs_offset = span.start + 1 + *offset_in_str;
                    offset_expr_spans(&mut parsed, abs_offset);

                    let expr_span = parsed.span;
                    // Wrap in to_string() call
                    let to_string_call = Expr {
                        id: self.fresh_id(),
                        kind: ExprKind::MethodCall {
                            object: Box::new(parsed),
                            method: "to_string".to_string(),
                            type_args: None,
                            args: vec![],
                        },
                        span: expr_span,
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
                    args: vec![CallArg { name: None, mode: ArgMode::Default, expr: seg_expr }],
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

/// What a variant's @message resolves to.
enum MessageTemplate {
    /// Format string with interpolation: `"error: {name}"`
    Format(String),
    /// Delegate to inner value: `inner.message()`
    Delegate(String),
}

/// Heuristic: does this type name look like an error type?
/// Matches names ending in "Error" (e.g., IoError, ManifestError).
fn is_error_type_name(ty: &str) -> bool {
    ty.ends_with("Error")
}

/// Extract the template from a `message("template")` attribute string.
fn extract_message_attr_template(attr: &str) -> Option<String> {
    let stripped = attr.strip_prefix("message(")?;
    let stripped = stripped.strip_suffix(')')?;
    // Remove surrounding quotes
    let stripped = stripped.trim();
    let stripped = stripped.strip_prefix('"')?;
    let stripped = stripped.strip_suffix('"')?;
    Some(stripped.to_string())
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

/// Shift all spans in an expression tree by `offset` bytes.
fn offset_expr_spans(expr: &mut Expr, offset: usize) {
    expr.span.start += offset;
    expr.span.end += offset;
    match &mut expr.kind {
        ExprKind::Binary { left, right, .. } => {
            offset_expr_spans(left, offset);
            offset_expr_spans(right, offset);
        }
        ExprKind::Unary { operand, .. } => offset_expr_spans(operand, offset),
        ExprKind::Call { func, args } => {
            offset_expr_spans(func, offset);
            for arg in args { offset_expr_spans(&mut arg.expr, offset); }
        }
        ExprKind::MethodCall { object, args, .. } => {
            offset_expr_spans(object, offset);
            for arg in args { offset_expr_spans(&mut arg.expr, offset); }
        }
        ExprKind::Field { object, .. } | ExprKind::OptionalField { object, .. } => {
            offset_expr_spans(object, offset);
        }
        ExprKind::Index { object, index } => {
            offset_expr_spans(object, offset);
            offset_expr_spans(index, offset);
        }
        ExprKind::Try { expr, else_clause } => {
            offset_expr_spans(expr, offset);
            if let Some(tc) = else_clause { offset_expr_spans(&mut tc.body, offset); }
        }
        ExprKind::Unwrap { expr, .. } => offset_expr_spans(expr, offset),
        ExprKind::Cast { expr, .. } => offset_expr_spans(expr, offset),
        ExprKind::NullCoalesce { value, default } => {
            offset_expr_spans(value, offset);
            offset_expr_spans(default, offset);
        }
        ExprKind::Array(exprs) | ExprKind::Tuple(exprs) => {
            for e in exprs { offset_expr_spans(e, offset); }
        }
        // Leaf nodes and complex variants unlikely in interpolation — no nested Exprs to fix
        _ => {}
    }
}

/// Segment of an interpolated string.
enum InterpSegment {
    Literal(String),
    /// Expression text and its byte offset within the original string content.
    Expr(String, usize),
}

/// Parse a string containing `{expr}` interpolation into segments.
///
/// Returns `None` if no interpolation is found.
fn parse_interpolation_segments(s: &str) -> Option<Vec<InterpSegment>> {
    let mut segments = Vec::new();
    let mut literal = String::new();
    let mut chars = s.chars().peekable();
    let mut has_interp = false;
    let mut byte_pos: usize = 0;

    while let Some(c) = chars.next() {
        byte_pos += c.len_utf8();
        if c == '{' {
            has_interp = true;
            if !literal.is_empty() {
                segments.push(InterpSegment::Literal(std::mem::take(&mut literal)));
            }
            let expr_start = byte_pos; // byte offset right after '{'
            let mut expr_str = String::new();
            let mut depth = 1;
            for ch in chars.by_ref() {
                byte_pos += ch.len_utf8();
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
            segments.push(InterpSegment::Expr(expr_str, expr_start));
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
        assert!(matches!(&segs[1], InterpSegment::Expr(s, 7) if s == "name"));
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
