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
pub use defaults::is_valid_default_expr;

use rask_ast::decl::{Decl, DeclKind, FnDecl, Param, StructDecl, EnumDecl, TraitDecl, ImplDecl};
use rask_ast::expr::{ArgMode, BinOp, CallArg, ConvertKind, Expr, ExprKind, MatchArm, Pattern, UnaryOp};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::{NodeId, Span};

/// First NodeId handed out for synthesized nodes.
///
/// NodeIds are partitioned into 1M-wide bands so ids stay unique across a whole
/// compilation: user code counts up from 0, and `rask-stdlib` parses its stub
/// sources into the 1M, 2M and 3M bands. Desugaring invents new nodes, so it
/// needs a band of its own — it sat at 1M and overlapped the stdlib's, which
/// made `node_types` lookups return another node's type. A stdlib `match self`
/// then read as a match on a `string` and its arm bindings were dropped, so
/// `IoError.message` failed to lower with an unresolved `msg` (#463).
pub const DESUGAR_ID_BASE: u32 = 10_000_000;

/// First NodeId handed out by default/named-argument desugaring, which runs
/// after operator desugaring and needs a band distinct from both it and the
/// stdlib's. See [`DESUGAR_ID_BASE`].
pub const DEFAULT_ARGS_ID_BASE: u32 = 20_000_000;

/// Run the whole desugar phase over a list of declarations.
///
/// Both sub-passes always run together. Splitting them was a trap: every
/// pipeline that called only the operator pass silently lost default
/// arguments and struct field defaults, so `Config {}` type-checked as a
/// missing-fields error under `rask test` while `rask build` accepted it
/// (#549).
pub fn desugar(decls: &mut [Decl]) {
    desugar_with_diagnostics(decls);
}

/// ER26 coverage error from @message desugaring.
#[derive(Debug, Clone)]
pub struct DesugarError {
    pub message: String,
    pub span: Span,
}

/// Desugar phase, returning any ER26 coverage errors.
pub fn desugar_with_diagnostics(decls: &mut [Decl]) -> Vec<DesugarError> {
    let mut desugarer = Desugarer::new(DESUGAR_ID_BASE);
    desugarer.scan_error_message_types(decls);
    for decl in decls.iter_mut() {
        desugarer.desugar_decl(decl);
    }
    let errors = std::mem::take(&mut desugarer.errors);

    // Defaults need the full declaration list to build their lookup table,
    // so they run as a second sweep rather than inline with the operators.
    defaults::desugar_default_args(decls);

    errors
}

/// One piece of a parsed `format` template.
enum TemplatePiece {
    Literal(String),
    /// `{}` or `{N}` — the argument at this index, with its spec.
    Positional(usize, Option<rask_ast::fmt_spec::FormatSpec>),
    /// `{name}` — an expression captured from the enclosing scope (F3).
    Named(Expr, Option<rask_ast::fmt_spec::FormatSpec>),
}

/// The desugaring context.
struct Desugarer {
    next_id: u32,
    errors: Vec<DesugarError>,
    /// Type names known to implement `Error` (ER37): `@message` enums and
    /// any type with a manual `message()` method. A single-payload `@message`
    /// variant whose payload is in this set auto-delegates to `inner.message()`.
    error_message_types: std::collections::HashSet<String>,
}

impl Desugarer {
    fn new(start_id: u32) -> Self {
        Self {
            next_id: start_id,
            errors: Vec::new(),
            error_message_types: std::collections::HashSet::new(),
        }
    }

    /// Collect the type names that implement `Error` so single-payload
    /// `@message` variants can decide whether to delegate. Purely syntactic:
    /// a `@message` enum, or any struct/enum/impl that defines `message()`.
    fn scan_error_message_types(&mut self, decls: &[Decl]) {
        let has_message = |methods: &[FnDecl]| methods.iter().any(|m| m.name == "message");
        for decl in decls {
            match &decl.kind {
                DeclKind::Enum(e) => {
                    if e.attrs.iter().any(|a| a == "message") || has_message(&e.methods) {
                        self.error_message_types.insert(e.name.clone());
                    }
                }
                DeclKind::Struct(s) => {
                    if has_message(&s.methods) {
                        self.error_message_types.insert(s.name.clone());
                    }
                }
                DeclKind::Impl(i) => {
                    if has_message(&i.methods) {
                        self.error_message_types.insert(i.target_ty.clone());
                    }
                }
                _ => {}
            }
        }
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
            DeclKind::Package(_) | DeclKind::CImport(_) => {}
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
            let template = self.extract_message_template(variant);

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

    /// Resolve a variant to its message template. Precedence (type.errors ER36/37/6):
    ///   1. explicit `@message("template")` on the variant,
    ///   2. single payload implementing `Error` → delegate to `inner.message()`,
    ///   3. ER6 fallback: humanized variant name, with payloads interpolated.
    /// Every variant resolves — there is no uncovered case.
    fn extract_message_template(&self, variant: &rask_ast::decl::Variant) -> MessageTemplate {
        // ER36: explicit @message("template") on the variant wins.
        for attr in &variant.attrs {
            if let Some(tmpl) = extract_message_attr_template(attr) {
                return MessageTemplate::Format(translate_positional_refs(&tmpl));
            }
        }
        // ER37: single payload that implements Error delegates to it. The
        // `ends_with("Error")` check keeps cross-module error types working when
        // their declaration isn't in this compilation unit's decl set.
        if variant.fields.len() == 1 {
            let payload_ty = &variant.fields[0].ty;
            if self.error_message_types.contains(payload_ty) || is_error_type_name(payload_ty) {
                return MessageTemplate::Delegate(variant.fields[0].name.clone());
            }
        }
        // ER6: humanized variant name, interpolating any payload fields.
        MessageTemplate::Format(humanize_variant(&variant.name, &variant.fields))
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
            StmtKind::Mut { init, .. } => self.desugar_expr(init),
            StmtKind::Let { init, .. } => self.desugar_expr(init),
            StmtKind::MutTuple { init, .. } => self.desugar_expr(init),
            StmtKind::LetTuple { init, .. } => self.desugar_expr(init),
            StmtKind::LetStruct { init, .. } => self.desugar_expr(init),
            StmtKind::Assign { target, value, .. } => {
                self.desugar_expr(target);
                self.desugar_expr(value);
            }
            StmtKind::Return(Some(e)) => self.desugar_expr(e),
            StmtKind::Return(None) => {}
            StmtKind::Break { value: Some(value), .. } => self.desugar_expr(value),
            StmtKind::Break { value: None, .. } | StmtKind::Continue(_) => {}
            StmtKind::While { cond, body, .. } => {
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
        // `format(template, args…)` is rewritten from its raw template before
        // anything walks into it — the template is compile-time input, and the
        // rewrite desugars the argument expressions itself (std.fmt/CM2, CM5).
        if self.desugar_format_call(expr) {
            return;
        }

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
                for arg in args.iter_mut() {
                    self.desugar_expr(&mut arg.expr);
                }
                // std.fmt/D3/D4: `print(x)` renders x, so it goes through
                // `to_string` exactly like `{x}` does. It didn't, and each
                // backend had its own idea of what to print — native the
                // address of an aggregate's storage and a char's code point,
                // the interpreter a debug rendering that ignored the type's own
                // `to_string`. Both wrong even for a type that opted in: a
                // `Point` with a Displayable impl printed 140729371079408
                // natively and `Point { x: 1, y: 2 }` on interp, where `{p}`
                // gave `(1, 2)` on both (#772).
                //
                // Desugaring rather than teaching two renderers about
                // Displayable keeps one renderer, and gets the type check for
                // free — a value that can't render fails on the `to_string`
                // call, with the message interpolation already produces.
                if Self::is_render_builtin(func) {
                    for arg in args.iter_mut() {
                        if Self::already_a_string_literal(&arg.expr) {
                            continue;
                        }
                        let arg_span = arg.expr.span;
                        let inner = std::mem::replace(
                            &mut arg.expr,
                            Expr {
                                id: rask_ast::NodeId::DUMMY,
                                kind: ExprKind::String(String::new()),
                                span: arg_span,
                            },
                        );
                        arg.expr = self.render_expr(inner, None);
                    }
                }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.desugar_expr(object);
                for arg in args {
                    self.desugar_expr(&mut arg.expr);
                }
                Self::desugar_conversion_method(expr);
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
                ..
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
            ExprKind::Try { expr: e } | ExprKind::Take { place: e } => self.desugar_expr(e),
            ExprKind::Catch { value, ref mut clause } => {
                self.desugar_expr(value);
                self.desugar_expr(&mut clause.body);
            }
            ExprKind::IsPresent { expr: e, .. } => self.desugar_expr(e),
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
            ExprKind::Cast { expr: inner, .. } | ExprKind::Convert { expr: inner, .. } => {
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
            | ExprKind::None
            => {}
            ExprKind::String(_) | ExprKind::StringInterp(_) => {
                // String interpolation desugaring handled below
            }
        }

        // Then, transform operators if applicable
        let span = expr.span;

        // `using Pool<T> { ... }` (and `using Pool<T>(cap) { ... }`) creates a
        // fresh pool for the block and binds it to the lowercase name `pool`,
        // so `pool.insert(...)` / `pool[h]` work with no separate `let`
        // (mem.context-clauses/CC4, #269). Rewriting this here — instead of
        // teaching the resolver and type checker a second, block-scoped
        // binding mechanism — means it's exactly as if the user had written
        // `mut pool = Pool<T>.new()` themselves, so every later pass (borrow
        // checking, closures capturing `pool` for `spawn`, native codegen)
        // already handles it. `Multitasking`/`ThreadPool` contexts are left
        // alone; they install a runtime, not a value.
        if matches!(&expr.kind, ExprKind::UsingBlock { name, .. } if base_type_name(name) == "Pool") {
            let old = std::mem::replace(&mut expr.kind, ExprKind::Bool(false));
            if let ExprKind::UsingBlock { name, args, mut body } = old {
                let ctor_method = if args.is_empty() { "new" } else { "with_capacity" };
                let pool_init = Expr {
                    id: self.fresh_id(),
                    kind: ExprKind::MethodCall {
                        object: Box::new(Expr { id: self.fresh_id(), kind: ExprKind::Ident(name), span }),
                        method: ctor_method.to_string(),
                        type_args: None,
                        args,
                    },
                    span,
                };
                let pool_stmt = Stmt {
                    id: self.fresh_id(),
                    kind: StmtKind::Mut {
                        name: "pool".to_string(),
                        name_span: span,
                        ty: None,
                        init: pool_init,
                    },
                    span,
                };
                let mut new_body = vec![pool_stmt];
                new_body.append(&mut body);
                expr.kind = ExprKind::Block(new_body);
            }
        }

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

        // Desugar StringInterp: segments → "lit".concat(expr.to_string()).concat("lit")...
        if let ExprKind::StringInterp(segments) = &expr.kind {
            let segments = segments.clone();
            expr.kind = match self.desugar_string_interp(&segments, span) {
                Some(desugared) => desugared,
                None => ExprKind::String(String::new()),
            };
            // Already handled. The legacy scan below must not see the result:
            // `"{{braces}}"` desugars to the literal `{braces}`, which that
            // scanner would happily read as an interpolation (#521).
            return;
        }
        // Legacy: raw strings with { that weren't parsed as StringInterp (shouldn't happen,
        // but kept for safety during transition)
        if let ExprKind::String(s) = &expr.kind {
            if s.contains('{') {
                if let Some(desugared) = self.desugar_string_interpolation(s, span) {
                    expr.kind = desugared;
                }
            }
        }
    }

    /// `format(template, args…)` → the string-building code the template
    /// describes (std.fmt/CM2, CM5). Returns true when it rewrote `expr`.
    ///
    /// The template is read here, not at runtime, so `{0}`, `{{`, and `{:spec}`
    /// all mean what the spec says they mean. Before this the template went
    /// through ordinary interpolation first: `{0}` came out as the integer
    /// zero, `{{x}}` turned back into a placeholder for a variable `x`, and
    /// nothing reached native codegen at all.
    fn desugar_format_call(&mut self, expr: &mut Expr) -> bool {
        let span = expr.span;
        let ExprKind::Call { func, args } = &expr.kind else { return false };
        if !matches!(&func.kind, ExprKind::Ident(n) if n == "format") {
            return false;
        }
        let Some(first) = args.first() else { return false };
        if first.name.is_some() {
            return false;
        }
        let ExprKind::String(template) = &first.expr.kind else { return false };

        let template = template.clone();
        let template_span = first.expr.span;
        let value_args: Vec<Expr> = args[1..].iter().map(|a| a.expr.clone()).collect();

        let Some(pieces) = self.parse_template(&template, template_span, value_args.len()) else {
            return false;
        };

        let mut parts: Vec<Expr> = Vec::new();
        for piece in pieces {
            match piece {
                TemplatePiece::Literal(text) => parts.push(self.string_lit(text, span)),
                TemplatePiece::Positional(idx, spec) => {
                    let Some(arg) = value_args.get(idx) else {
                        self.errors.push(DesugarError {
                            message: format!(
                                "format template wants argument {} but only {} were passed",
                                idx,
                                value_args.len()
                            ),
                            span: template_span,
                        });
                        return false;
                    };
                    let mut inner = arg.clone();
                    self.desugar_expr(&mut inner);
                    parts.push(self.render_expr(inner, spec));
                }
                TemplatePiece::Named(mut inner, spec) => {
                    self.desugar_expr(&mut inner);
                    parts.push(self.render_expr(inner, spec));
                }
            }
        }

        // An empty template is the empty string (std.fmt, edge cases).
        expr.kind = match self.concat_chain(parts, span) {
            Some(kind) => kind,
            None => ExprKind::String(String::new()),
        };
        true
    }

    /// Split a format template into literal text and placeholders (CM2).
    /// `None` means an error was recorded and the call should be left alone.
    fn parse_template(
        &mut self,
        template: &str,
        span: rask_ast::Span,
        arg_count: usize,
    ) -> Option<Vec<TemplatePiece>> {
        let chars: Vec<char> = template.chars().collect();
        let mut pieces = Vec::new();
        let mut literal = String::new();
        let mut i = 0;
        let mut next_auto = 0usize;
        // F2: auto (`{}`) and explicit (`{0}`) indexing can't be mixed.
        let mut saw_auto = false;
        let mut saw_explicit = false;

        while i < chars.len() {
            match chars[i] {
                // F4
                '{' if chars.get(i + 1) == Some(&'{') => {
                    literal.push('{');
                    i += 2;
                }
                '}' if chars.get(i + 1) == Some(&'}') => {
                    literal.push('}');
                    i += 2;
                }
                '{' => {
                    let mut depth = 1;
                    let mut j = i + 1;
                    while j < chars.len() && depth > 0 {
                        match chars[j] {
                            '{' => depth += 1,
                            '}' => depth -= 1,
                            _ => {}
                        }
                        if depth > 0 {
                            j += 1;
                        }
                    }
                    if depth != 0 {
                        self.errors.push(DesugarError {
                            message: "unclosed `{` in format template — write `{{` for a literal brace".to_string(),
                            span,
                        });
                        return None;
                    }
                    let inner: String = chars[i + 1..j].iter().collect();
                    i = j + 1;

                    let (arg_part, spec_text) = match rask_ast::fmt_spec::split_spec(&inner) {
                        Some(pos) => (inner[..pos].to_string(), Some(inner[pos + 1..].to_string())),
                        None => (inner.clone(), None),
                    };
                    let spec = match spec_text {
                        Some(text) => match rask_ast::fmt_spec::parse_spec(&text) {
                            Some(s) => Some(s),
                            None => {
                                self.errors.push(DesugarError {
                                    message: format!("`{}` is not a format spec", text),
                                    span,
                                });
                                return None;
                            }
                        },
                        None => None,
                    };

                    if !literal.is_empty() {
                        pieces.push(TemplatePiece::Literal(std::mem::take(&mut literal)));
                    }

                    let trimmed = arg_part.trim();
                    if trimmed.is_empty() {
                        saw_auto = true;
                        pieces.push(TemplatePiece::Positional(next_auto, spec));
                        next_auto += 1;
                    } else if let Ok(idx) = trimmed.parse::<usize>() {
                        saw_explicit = true;
                        pieces.push(TemplatePiece::Positional(idx, spec));
                    } else {
                        // F3: a name (or field path, or expression) captured
                        // from the enclosing scope.
                        let parsed = self.parse_placeholder_expr(trimmed, span)?;
                        pieces.push(TemplatePiece::Named(parsed, spec));
                    }
                }
                c => {
                    literal.push(c);
                    i += 1;
                }
            }
        }
        if !literal.is_empty() {
            pieces.push(TemplatePiece::Literal(literal));
        }

        if saw_auto && saw_explicit {
            self.errors.push(DesugarError {
                message: "format template mixes `{}` with `{0}` — pick one and use it throughout"
                    .to_string(),
                span,
            });
            return None;
        }
        if saw_auto && next_auto > arg_count {
            self.errors.push(DesugarError {
                message: format!(
                    "format template has {} placeholders but {} arguments were passed",
                    next_auto, arg_count
                ),
                span,
            });
            return None;
        }

        Some(pieces)
    }

    /// Parse the expression text inside a `{…}` placeholder.
    fn parse_placeholder_expr(&mut self, text: &str, span: rask_ast::Span) -> Option<Expr> {
        // The placeholder body is re-lexed, so its tokens need the enclosing
        // file stamped on them — the parser lifts token spans directly for
        // names, and without this everything inside a `"{…}"` claimed file 0.
        let lex = rask_lexer::Lexer::new_with_file_id(text, span.file_id).tokenize();
        if !lex.errors.is_empty() {
            self.errors.push(DesugarError {
                message: format!("`{{{}}}` in the format template isn't an expression", text),
                span,
            });
            return None;
        }
        let mut parser = rask_parser::Parser::new_with_file_id(lex.tokens, 0, span.file_id);
        match parser.parse_expr() {
            Ok(mut parsed) => {
                offset_expr_spans(&mut parsed, span.start);
                parsed.span = span;
                Some(parsed)
            }
            Err(_) => {
                self.errors.push(DesugarError {
                    message: format!("`{{{}}}` in the format template isn't an expression", text),
                    span,
                });
                None
            }
        }
    }

    /// Desugar pre-parsed StringInterp segments into a concat chain.
    fn desugar_string_interp(&mut self, segments: &[rask_ast::expr::StringSegment], span: rask_ast::Span) -> Option<ExprKind> {
        use rask_ast::expr::StringSegment;

        let mut exprs: Vec<Expr> = Vec::new();
        for seg in segments {
            match seg {
                StringSegment::Literal(text) => {
                    exprs.push(self.string_lit(text.clone(), span));
                }
                StringSegment::Expr(parsed, spec) => {
                    // Recursively desugar the interpolation expression
                    let mut inner = *parsed.clone();
                    self.desugar_expr(&mut inner);
                    exprs.push(self.render_expr(inner, *spec));
                }
            }
        }

        self.concat_chain(exprs, span)
    }

    /// A string literal that the interpolation scanners must leave alone —
    /// it's already the final text, braces and all.
    fn string_lit(&mut self, text: String, span: rask_ast::Span) -> Expr {
        Expr { id: self.fresh_id(), kind: ExprKind::String(text), span }
    }

    /// `print` / `println` — the builtins that turn each argument into text.
    ///
    /// CV11–CV16: the six conversion methods become `Convert` nodes.
    ///
    /// `x.to<i32>()` and its five siblings are methods in the source and a
    /// conversion everywhere after here, so the checker, both backends and the
    /// formatter see one node kind instead of a method call they'd each have to
    /// recognize. The phrase verbs they replaced already lowered to this node,
    /// which is why the new forms cost so little below this line.
    ///
    /// Deliberately narrow: exactly one type argument, no value arguments, and
    /// a type argument that names a numeric primitive. `x.floor()` on an `f64`
    /// is a different method — it answers `f64` and stays a method call — and
    /// the type argument is what tells the two apart.
    fn desugar_conversion_method(expr: &mut Expr) {
        let ExprKind::MethodCall { object, method, type_args, args } = &mut expr.kind else {
            return;
        };
        if !args.is_empty() {
            return;
        }
        let Some(targets) = type_args.as_ref() else { return };
        let [target] = targets.as_slice() else { return };
        if !is_numeric_primitive(target) {
            return;
        }
        let kind = match method.as_str() {
            "to" => ConvertKind::To,
            "wrap" => ConvertKind::Wrap,
            "clamp" => ConvertKind::Clamp,
            "round" => ConvertKind::Round,
            "floor" => ConvertKind::Floor,
            "ceil" => ConvertKind::Ceil,
            _ => return,
        };
        let target = target.clone();
        let placeholder = Expr {
            id: NodeId::DUMMY,
            kind: ExprKind::Int(0, None),
            span: expr.span,
        };
        let inner = std::mem::replace(object.as_mut(), placeholder);
        expr.kind = ExprKind::Convert {
            expr: Box::new(inner),
            target,
            kind,
        };
    }

    /// Not `panic`/`todo`/`unreachable`/`assert`: those take a message that is
    /// already a string, usually an interpolation whose placeholders were
    /// rendered on the way in. `format` is the same.
    fn is_render_builtin(func: &Expr) -> bool {
        matches!(&func.kind, ExprKind::Ident(n) if n == "print" || n == "println")
    }

    /// A bare string literal is already the text — wrapping it in `to_string()`
    /// would allocate a copy of every `print("\n")` in the program for nothing.
    /// An interpolation has already become a `__concat` chain by this point, so
    /// it isn't a `String` node and does get wrapped; `string.to_string()` is
    /// identity, and skipping it would need a type the desugar pass doesn't have.
    fn already_a_string_literal(e: &Expr) -> bool {
        matches!(&e.kind, ExprKind::String(_))
    }

    /// Render one value as a string: `to_string()` when there's no spec,
    /// `__fmt(…)` with the spec's five constants when there is (std.fmt/CM5).
    fn render_expr(&mut self, inner: Expr, spec: Option<rask_ast::fmt_spec::FormatSpec>) -> Expr {
        let span = inner.span;
        let Some(spec) = spec.filter(|s| !s.is_plain()) else {
            return Expr {
                id: self.fresh_id(),
                kind: ExprKind::MethodCall {
                    object: Box::new(inner),
                    method: "to_string".to_string(),
                    type_args: None,
                    args: vec![],
                },
                span,
            };
        };

        let (ty, width, precision, align, fill) = spec.encode();
        let int_arg = |this: &mut Self, n: i64| CallArg {
            name: None,
            mode: ArgMode::Default,
            expr: Expr { id: this.fresh_id(), kind: ExprKind::Int(i128::from(n), None), span },
        };
        let args = vec![
            int_arg(self, ty),
            int_arg(self, width),
            int_arg(self, precision),
            int_arg(self, align),
            CallArg {
                name: None,
                mode: ArgMode::Default,
                expr: Expr { id: self.fresh_id(), kind: ExprKind::Char(fill), span },
            },
        ];
        Expr {
            id: self.fresh_id(),
            kind: ExprKind::MethodCall {
                object: Box::new(inner),
                method: "__fmt".to_string(),
                type_args: None,
                args,
            },
            span,
        }
    }

    /// `first.concat(second).concat(third)…`
    fn concat_chain(&mut self, mut exprs: Vec<Expr>, span: rask_ast::Span) -> Option<ExprKind> {
        if exprs.is_empty() {
            return None;
        }
        if exprs.len() == 1 {
            return Some(exprs.remove(0).kind);
        }

        let mut result = exprs.remove(0);
        for seg_expr in exprs {
            result = Expr {
                id: self.fresh_id(),
                kind: ExprKind::MethodCall {
                    object: Box::new(result),
                    // Compiler-internal, like `__fmt`. Interpolation is the one
                    // way to combine strings, so there is no public `concat`
                    // for this to target (std.strings, #303).
                    method: "__concat".to_string(),
                    type_args: None,
                    args: vec![CallArg { name: None, mode: ArgMode::Default, expr: seg_expr }],
                },
                span,
            };
        }
        Some(result.kind)
    }

    /// Legacy: Parse string interpolation and produce a concat chain.
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
                    let lex = rask_lexer::Lexer::new_with_file_id(expr_str, span.file_id).tokenize();
                    if !lex.errors.is_empty() {
                        return None; // Parse error — leave as raw string
                    }
                    let mut parser = rask_parser::Parser::new_with_file_id(lex.tokens, 0, span.file_id);
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

        // Chain with the internal concat: first.__concat(second)...
        let mut result = exprs.remove(0);
        for seg_expr in exprs {
            result = Expr {
                id: self.fresh_id(),
                kind: ExprKind::MethodCall {
                    object: Box::new(result),
                    method: "__concat".to_string(),
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

/// ER6 fallback message: humanize the variant name (`NotFound` → "not found")
/// and interpolate any payload fields. `UnexpectedEnd(ctx)` → "unexpected end: {ctx}";
/// a positional payload interpolates as `{_0}`.
fn humanize_variant(name: &str, fields: &[rask_ast::decl::Field]) -> String {
    let mut base = String::new();
    for (i, c) in name.char_indices() {
        if c.is_ascii_uppercase() && i > 0 {
            base.push(' ');
        }
        base.push(c.to_ascii_lowercase());
    }
    if fields.is_empty() {
        return base;
    }
    let parts: Vec<String> = fields.iter().map(|f| format!("{{{}}}", f.name)).collect();
    format!("{}: {}", base, parts.join(", "))
}

/// ER36: positional payload refs `{0}`/`{1}` name the auto-generated tuple
/// fields, which the parser binds as `_0`/`_1`. Rewrite `{N}` → `{_N}` so the
/// string-interpolation pass resolves them to those bindings instead of the
/// integer literal `N`. Named refs (`{ctx}`) and escaped braces pass through.
fn translate_positional_refs(tmpl: &str) -> String {
    let mut out = String::with_capacity(tmpl.len());
    let mut chars = tmpl.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            // Collect the run of digits directly inside the braces.
            let mut digits = String::new();
            while let Some(&d) = chars.peek() {
                if d.is_ascii_digit() {
                    digits.push(d);
                    chars.next();
                } else {
                    break;
                }
            }
            if !digits.is_empty() && chars.peek() == Some(&'}') {
                out.push('{');
                out.push('_');
                out.push_str(&digits);
            } else {
                out.push('{');
                out.push_str(&digits);
            }
        } else {
            out.push(c);
        }
    }
    out
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

/// Strip generic args from a context-clause name: "Pool<Entity>" → "Pool".
fn base_type_name(name: &str) -> &str {
    name.split('<').next().unwrap_or(name)
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
        UnaryOp::Not | UnaryOp::Ref | UnaryOp::Deref | UnaryOp::Own => None,
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
        ExprKind::Try { expr } => offset_expr_spans(expr, offset),
        ExprKind::Take { place } => offset_expr_spans(place, offset),
        ExprKind::Catch { value, clause } => {
            offset_expr_spans(value, offset);
            offset_expr_spans(&mut clause.body, offset);
        }
        ExprKind::IsPresent { expr, .. } => offset_expr_spans(expr, offset),
        ExprKind::Unwrap { expr, .. } => offset_expr_spans(expr, offset),
        ExprKind::Cast { expr, .. } | ExprKind::Convert { expr, .. } => offset_expr_spans(expr, offset),
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
        // fmt/F4: `{{` and `}}` are literal braces, not an interpolation. This
        // scanner runs on strings the parser handed on untouched (the spec test
        // runner, mainly), so it needs the same rule.
        if c == '{' && chars.peek() == Some(&'{') {
            chars.next();
            byte_pos += 1;
            literal.push('{');
            has_interp = true;
            continue;
        }
        if c == '}' && chars.peek() == Some(&'}') {
            chars.next();
            byte_pos += 1;
            literal.push('}');
            has_interp = true;
            continue;
        }
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

/// The types a conversion can target (type.primitives/CV11–CV16).
fn is_numeric_primitive(name: &str) -> bool {
    matches!(
        name.trim(),
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize"
            | "u8" | "u16" | "u32" | "u64" | "u128" | "usize"
            | "f32" | "f64"
    )
}
