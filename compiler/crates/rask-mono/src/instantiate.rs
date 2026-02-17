// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function instantiation - clone AST and substitute type parameters.

use rask_ast::{
    decl::{Decl, DeclKind, FnDecl, Param, TypeParam},
    expr::{
        CallArg, ClosureParam, Expr, ExprKind, FieldInit, MatchArm, Pattern, SelectArm, SelectArmKind,
    },
    stmt::{Stmt, StmtKind},
    NodeId,
};
use rask_types::Type;
use std::collections::HashMap;

/// Split a comma-separated type argument string, respecting nested angle brackets.
/// e.g. "Vec<i32>, E" → ["Vec<i32>", "E"]
fn split_type_args(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, ch) in s.char_indices() {
        match ch {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    parts
}

/// Type substitutor - clones AST while replacing type parameters
struct TypeSubstitutor {
    /// Mapping from type parameter name to concrete type
    substitutions: HashMap<String, Type>,
    /// Counter for generating fresh NodeIds
    next_node_id: u32,
}

impl TypeSubstitutor {
    fn new(type_params: &[TypeParam], type_args: &[Type]) -> Self {
        let mut substitutions = HashMap::new();
        for (param, arg) in type_params.iter().zip(type_args.iter()) {
            substitutions.insert(param.name.clone(), arg.clone());
        }
        Self {
            substitutions,
            next_node_id: 0,
        }
    }

    fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    /// Substitute type parameter names with concrete types.
    ///
    /// Handles bare names ("T"), compound generics ("Vec<T>", "Result<T, E>"),
    /// option shorthand ("T?"), and result infix ("T or E").
    fn substitute_type_string(&self, type_str: &str) -> String {
        let s = type_str.trim();

        // Exact match on a type parameter name
        if let Some(ty) = self.substitutions.get(s) {
            return format!("{}", ty);
        }

        // Option shorthand: "T?" → substitute T, re-append "?"
        if let Some(inner) = s.strip_suffix('?') {
            let inner_sub = self.substitute_type_string(inner);
            return format!("{}?", inner_sub);
        }

        // Result infix: "T or E" → substitute both sides
        if let Some(idx) = s.find(" or ") {
            let ok_part = self.substitute_type_string(&s[..idx]);
            let err_part = self.substitute_type_string(&s[idx + 4..]);
            return format!("{} or {}", ok_part, err_part);
        }

        // Compound generic: "Name<A, B, ...>" → substitute each argument
        if let Some(open) = s.find('<') {
            if s.ends_with('>') {
                let base = &s[..open];
                let args_str = &s[open + 1..s.len() - 1];
                let substituted_args = split_type_args(args_str)
                    .iter()
                    .map(|a| self.substitute_type_string(a))
                    .collect::<Vec<_>>()
                    .join(", ");
                // Substitute the base name itself in case it's a type param
                let base_sub = if let Some(ty) = self.substitutions.get(base) {
                    format!("{}", ty)
                } else {
                    base.to_string()
                };
                return format!("{}<{}>", base_sub, substituted_args);
            }
        }

        // Tuple: "(A, B)" → substitute each element
        if s.starts_with('(') && s.ends_with(')') {
            let inner = &s[1..s.len() - 1];
            if inner.is_empty() {
                return s.to_string(); // unit "()"
            }
            let substituted = split_type_args(inner)
                .iter()
                .map(|a| self.substitute_type_string(a))
                .collect::<Vec<_>>()
                .join(", ");
            return format!("({})", substituted);
        }

        s.to_string()
    }

    fn clone_decl(&mut self, decl: &Decl) -> Decl {
        Decl {
            id: self.fresh_id(),
            kind: match &decl.kind {
                DeclKind::Fn(fn_decl) => DeclKind::Fn(self.clone_fn_decl(fn_decl)),
                DeclKind::Struct(s) => DeclKind::Struct(self.clone_struct_decl(s)),
                DeclKind::Enum(e) => DeclKind::Enum(self.clone_enum_decl(e)),
                // Other declaration kinds don't contain type parameters to substitute
                other => other.clone(),
            },
            span: decl.span.clone(),
        }
    }

    fn clone_struct_decl(&mut self, s: &rask_ast::decl::StructDecl) -> rask_ast::decl::StructDecl {
        rask_ast::decl::StructDecl {
            name: s.name.clone(),
            type_params: Vec::new(), // Removed after instantiation
            fields: s.fields.iter().map(|f| rask_ast::decl::Field {
                name: f.name.clone(),
                name_span: f.name_span.clone(),
                ty: self.substitute_type_string(&f.ty),
                is_pub: f.is_pub,
            }).collect(),
            methods: s.methods.iter().map(|m| self.clone_fn_decl(m)).collect(),
            is_pub: s.is_pub,
            attrs: s.attrs.clone(),
        }
    }

    fn clone_enum_decl(&mut self, e: &rask_ast::decl::EnumDecl) -> rask_ast::decl::EnumDecl {
        rask_ast::decl::EnumDecl {
            name: e.name.clone(),
            type_params: Vec::new(), // Removed after instantiation
            variants: e.variants.iter().map(|v| rask_ast::decl::Variant {
                name: v.name.clone(),
                fields: v.fields.iter().map(|f| rask_ast::decl::Field {
                    name: f.name.clone(),
                    name_span: f.name_span.clone(),
                    ty: self.substitute_type_string(&f.ty),
                    is_pub: f.is_pub,
                }).collect(),
            }).collect(),
            methods: e.methods.iter().map(|m| self.clone_fn_decl(m)).collect(),
            is_pub: e.is_pub,
        }
    }

    fn clone_fn_decl(&mut self, fn_decl: &FnDecl) -> FnDecl {
        FnDecl {
            name: fn_decl.name.clone(),
            type_params: Vec::new(), // Removed after instantiation
            params: fn_decl.params.iter().map(|p| self.clone_param(p)).collect(),
            ret_ty: fn_decl
                .ret_ty
                .as_ref()
                .map(|ty| self.substitute_type_string(ty)),
            context_clauses: fn_decl.context_clauses.clone(),
            body: fn_decl.body.iter().map(|s| self.clone_stmt(s)).collect(),
            is_pub: fn_decl.is_pub,
            is_comptime: fn_decl.is_comptime,
            is_unsafe: fn_decl.is_unsafe,
            abi: fn_decl.abi.clone(),
            attrs: fn_decl.attrs.clone(),
        }
    }

    fn clone_param(&mut self, param: &Param) -> Param {
        Param {
            name: param.name.clone(),
            name_span: param.name_span.clone(),
            ty: self.substitute_type_string(&param.ty),
            is_take: param.is_take,
            is_mutate: param.is_mutate,
            default: param.default.as_ref().map(|e| self.clone_expr(e)),
        }
    }

    // ── Statements ──────────────────────────────────────────────────

    fn clone_stmt(&mut self, stmt: &Stmt) -> Stmt {
        Stmt {
            id: self.fresh_id(),
            kind: match &stmt.kind {
                StmtKind::Expr(e) => StmtKind::Expr(self.clone_expr(e)),

                StmtKind::Let {
                    name,
                    name_span,
                    ty,
                    init,
                } => StmtKind::Let {
                    name: name.clone(),
                    name_span: name_span.clone(),
                    ty: ty.as_ref().map(|t| self.substitute_type_string(t)),
                    init: self.clone_expr(init),
                },

                StmtKind::LetTuple { names, init } => StmtKind::LetTuple {
                    names: names.clone(),
                    init: self.clone_expr(init),
                },

                StmtKind::Const {
                    name,
                    name_span,
                    ty,
                    init,
                } => StmtKind::Const {
                    name: name.clone(),
                    name_span: name_span.clone(),
                    ty: ty.as_ref().map(|t| self.substitute_type_string(t)),
                    init: self.clone_expr(init),
                },

                StmtKind::ConstTuple { names, init } => StmtKind::ConstTuple {
                    names: names.clone(),
                    init: self.clone_expr(init),
                },

                StmtKind::Assign { target, value } => StmtKind::Assign {
                    target: self.clone_expr(target),
                    value: self.clone_expr(value),
                },

                StmtKind::Return(opt_expr) => {
                    StmtKind::Return(opt_expr.as_ref().map(|e| self.clone_expr(e)))
                }

                StmtKind::Break { label, value } => StmtKind::Break {
                    label: label.clone(),
                    value: value.as_ref().map(|e| self.clone_expr(e)),
                },

                StmtKind::Continue(label) => StmtKind::Continue(label.clone()),

                StmtKind::While { cond, body } => StmtKind::While {
                    cond: self.clone_expr(cond),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },

                StmtKind::WhileLet {
                    pattern,
                    expr,
                    body,
                } => StmtKind::WhileLet {
                    pattern: self.clone_pattern(pattern),
                    expr: self.clone_expr(expr),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },

                StmtKind::Loop { label, body } => StmtKind::Loop {
                    label: label.clone(),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },

                StmtKind::For {
                    label,
                    binding,
                    iter,
                    body,
                } => StmtKind::For {
                    label: label.clone(),
                    binding: binding.clone(),
                    iter: self.clone_expr(iter),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },

                StmtKind::Ensure {
                    body,
                    else_handler,
                } => StmtKind::Ensure {
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                    else_handler: else_handler.as_ref().map(|(name, stmts)| {
                        (
                            name.clone(),
                            stmts.iter().map(|s| self.clone_stmt(s)).collect(),
                        )
                    }),
                },

                StmtKind::Comptime(stmts) => {
                    StmtKind::Comptime(stmts.iter().map(|s| self.clone_stmt(s)).collect())
                }
            },
            span: stmt.span.clone(),
        }
    }

    // ── Expressions ─────────────────────────────────────────────────

    fn clone_expr(&mut self, expr: &Expr) -> Expr {
        Expr {
            id: self.fresh_id(),
            kind: match &expr.kind {
                // Literals
                ExprKind::Int(val, suffix) => ExprKind::Int(*val, *suffix),
                ExprKind::Float(val, suffix) => ExprKind::Float(*val, *suffix),
                ExprKind::String(s) => ExprKind::String(s.clone()),
                ExprKind::Char(c) => ExprKind::Char(*c),
                ExprKind::Bool(b) => ExprKind::Bool(*b),
                ExprKind::Null => ExprKind::Null,

                // Variables
                ExprKind::Ident(name) => ExprKind::Ident(name.clone()),

                // Operators
                ExprKind::Binary { op, left, right } => ExprKind::Binary {
                    op: *op,
                    left: Box::new(self.clone_expr(left)),
                    right: Box::new(self.clone_expr(right)),
                },
                ExprKind::Unary { op, operand } => ExprKind::Unary {
                    op: *op,
                    operand: Box::new(self.clone_expr(operand)),
                },

                // Calls
                ExprKind::Call { func, args } => ExprKind::Call {
                    func: Box::new(self.clone_expr(func)),
                    args: args.iter().map(|a| CallArg { mode: a.mode, expr: self.clone_expr(&a.expr) }).collect(),
                },
                ExprKind::MethodCall {
                    object,
                    method,
                    type_args,
                    args,
                } => ExprKind::MethodCall {
                    object: Box::new(self.clone_expr(object)),
                    method: method.clone(),
                    type_args: type_args.as_ref().map(|tas| {
                        tas.iter()
                            .map(|t| self.substitute_type_string(t))
                            .collect()
                    }),
                    args: args.iter().map(|a| CallArg { mode: a.mode, expr: self.clone_expr(&a.expr) }).collect(),
                },

                // Access
                ExprKind::Field { object, field } => ExprKind::Field {
                    object: Box::new(self.clone_expr(object)),
                    field: field.clone(),
                },
                ExprKind::OptionalField { object, field } => ExprKind::OptionalField {
                    object: Box::new(self.clone_expr(object)),
                    field: field.clone(),
                },
                ExprKind::Index { object, index } => ExprKind::Index {
                    object: Box::new(self.clone_expr(object)),
                    index: Box::new(self.clone_expr(index)),
                },

                // Blocks
                ExprKind::Block(stmts) => {
                    ExprKind::Block(stmts.iter().map(|s| self.clone_stmt(s)).collect())
                }

                // Control flow
                ExprKind::If {
                    cond,
                    then_branch,
                    else_branch,
                } => ExprKind::If {
                    cond: Box::new(self.clone_expr(cond)),
                    then_branch: Box::new(self.clone_expr(then_branch)),
                    else_branch: else_branch.as_ref().map(|e| Box::new(self.clone_expr(e))),
                },
                ExprKind::IfLet {
                    expr,
                    pattern,
                    then_branch,
                    else_branch,
                } => ExprKind::IfLet {
                    expr: Box::new(self.clone_expr(expr)),
                    pattern: self.clone_pattern(pattern),
                    then_branch: Box::new(self.clone_expr(then_branch)),
                    else_branch: else_branch.as_ref().map(|e| Box::new(self.clone_expr(e))),
                },
                ExprKind::GuardPattern {
                    expr,
                    pattern,
                    else_branch,
                } => ExprKind::GuardPattern {
                    expr: Box::new(self.clone_expr(expr)),
                    pattern: self.clone_pattern(pattern),
                    else_branch: Box::new(self.clone_expr(else_branch)),
                },
                ExprKind::IsPattern { expr, pattern } => ExprKind::IsPattern {
                    expr: Box::new(self.clone_expr(expr)),
                    pattern: self.clone_pattern(pattern),
                },
                ExprKind::Match { scrutinee, arms } => ExprKind::Match {
                    scrutinee: Box::new(self.clone_expr(scrutinee)),
                    arms: arms.iter().map(|a| self.clone_match_arm(a)).collect(),
                },

                // Error handling
                ExprKind::Try(inner) => ExprKind::Try(Box::new(self.clone_expr(inner))),
                ExprKind::Unwrap { expr: inner, message } => ExprKind::Unwrap {
                    expr: Box::new(self.clone_expr(inner)),
                    message: message.clone(),
                },
                ExprKind::NullCoalesce { value, default } => ExprKind::NullCoalesce {
                    value: Box::new(self.clone_expr(value)),
                    default: Box::new(self.clone_expr(default)),
                },

                // Ranges
                ExprKind::Range {
                    start,
                    end,
                    inclusive,
                } => ExprKind::Range {
                    start: start.as_ref().map(|e| Box::new(self.clone_expr(e))),
                    end: end.as_ref().map(|e| Box::new(self.clone_expr(e))),
                    inclusive: *inclusive,
                },

                // Aggregates
                ExprKind::StructLit {
                    name,
                    fields,
                    spread,
                } => ExprKind::StructLit {
                    name: name.clone(),
                    fields: fields
                        .iter()
                        .map(|f| FieldInit {
                            name: f.name.clone(),
                            value: self.clone_expr(&f.value),
                        })
                        .collect(),
                    spread: spread.as_ref().map(|e| Box::new(self.clone_expr(e))),
                },
                ExprKind::Array(elems) => {
                    ExprKind::Array(elems.iter().map(|e| self.clone_expr(e)).collect())
                }
                ExprKind::ArrayRepeat { value, count } => ExprKind::ArrayRepeat {
                    value: Box::new(self.clone_expr(value)),
                    count: Box::new(self.clone_expr(count)),
                },
                ExprKind::Tuple(elems) => {
                    ExprKind::Tuple(elems.iter().map(|e| self.clone_expr(e)).collect())
                }

                // Closures
                ExprKind::Closure {
                    params,
                    ret_ty,
                    body,
                } => ExprKind::Closure {
                    params: params
                        .iter()
                        .map(|p| ClosureParam {
                            name: p.name.clone(),
                            ty: p.ty.as_ref().map(|t| self.substitute_type_string(t)),
                        })
                        .collect(),
                    ret_ty: ret_ty.as_ref().map(|t| self.substitute_type_string(t)),
                    body: Box::new(self.clone_expr(body)),
                },

                // Type cast
                ExprKind::Cast { expr, ty } => ExprKind::Cast {
                    expr: Box::new(self.clone_expr(expr)),
                    ty: self.substitute_type_string(ty),
                },

                // Context blocks
                ExprKind::UsingBlock { name, args, body } => ExprKind::UsingBlock {
                    name: name.clone(),
                    args: args.iter().map(|a| CallArg { mode: a.mode, expr: self.clone_expr(&a.expr) }).collect(),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },
                ExprKind::WithAs { bindings, body } => ExprKind::WithAs {
                    bindings: bindings
                        .iter()
                        .map(|(e, name)| (self.clone_expr(e), name.clone()))
                        .collect(),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },

                // Spawn / block call / unsafe / comptime
                ExprKind::Spawn { body } => ExprKind::Spawn {
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },
                ExprKind::BlockCall { name, body } => ExprKind::BlockCall {
                    name: name.clone(),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },
                ExprKind::Unsafe { body } => ExprKind::Unsafe {
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },
                ExprKind::Comptime { body } => ExprKind::Comptime {
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },

                // Select
                ExprKind::Select { arms, is_priority } => ExprKind::Select {
                    arms: arms.iter().map(|a| self.clone_select_arm(a)).collect(),
                    is_priority: *is_priority,
                },

                // Assert / check
                ExprKind::Assert { condition, message } => ExprKind::Assert {
                    condition: Box::new(self.clone_expr(condition)),
                    message: message.as_ref().map(|m| Box::new(self.clone_expr(m))),
                },
                ExprKind::Check { condition, message } => ExprKind::Check {
                    condition: Box::new(self.clone_expr(condition)),
                    message: message.as_ref().map(|m| Box::new(self.clone_expr(m))),
                },
            },
            span: expr.span.clone(),
        }
    }

    // ── Helpers ─────────────────────────────────────────────────────

    fn clone_pattern(&self, pattern: &Pattern) -> Pattern {
        match pattern {
            Pattern::Wildcard => Pattern::Wildcard,
            Pattern::Ident(name) => Pattern::Ident(name.clone()),
            Pattern::Literal(expr) => {
                // Patterns don't need fresh IDs - they're structural
                Pattern::Literal(expr.clone())
            }
            Pattern::Constructor { name, fields } => Pattern::Constructor {
                name: name.clone(),
                fields: fields.iter().map(|p| self.clone_pattern(p)).collect(),
            },
            Pattern::Struct { name, fields, rest } => Pattern::Struct {
                name: name.clone(),
                fields: fields
                    .iter()
                    .map(|(n, p)| (n.clone(), self.clone_pattern(p)))
                    .collect(),
                rest: *rest,
            },
            Pattern::Tuple(pats) => {
                Pattern::Tuple(pats.iter().map(|p| self.clone_pattern(p)).collect())
            }
            Pattern::Or(pats) => {
                Pattern::Or(pats.iter().map(|p| self.clone_pattern(p)).collect())
            }
        }
    }

    fn clone_match_arm(&mut self, arm: &MatchArm) -> MatchArm {
        MatchArm {
            pattern: self.clone_pattern(&arm.pattern),
            guard: arm.guard.as_ref().map(|g| Box::new(self.clone_expr(g))),
            body: Box::new(self.clone_expr(&arm.body)),
        }
    }

    fn clone_select_arm(&mut self, arm: &SelectArm) -> SelectArm {
        SelectArm {
            kind: match &arm.kind {
                SelectArmKind::Recv { channel, binding } => SelectArmKind::Recv {
                    channel: self.clone_expr(channel),
                    binding: binding.clone(),
                },
                SelectArmKind::Send { channel, value } => SelectArmKind::Send {
                    channel: self.clone_expr(channel),
                    value: self.clone_expr(value),
                },
                SelectArmKind::Default => SelectArmKind::Default,
            },
            body: Box::new(self.clone_expr(&arm.body)),
        }
    }
}

/// Instantiate a generic declaration with concrete type arguments.
///
/// Clones the AST and replaces all type parameters with concrete types.
/// Works for functions, structs, and enums.
pub fn instantiate_function(decl: &Decl, type_args: &[Type]) -> Decl {
    let type_params = match &decl.kind {
        DeclKind::Fn(f) => &f.type_params,
        DeclKind::Struct(s) => &s.type_params,
        DeclKind::Enum(e) => &e.type_params,
        _ => {
            // No type parameters to substitute — return a clone
            return decl.clone();
        }
    };

    let mut substitutor = TypeSubstitutor::new(type_params, type_args);
    substitutor.clone_decl(decl)
}
