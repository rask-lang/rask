// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function instantiation - clone AST and substitute type parameters.

use rask_ast::{
    decl::{Decl, DeclKind, FnDecl, Param, TypeParam},
    expr::{
        CallArg, ClosureParam, Expr, ExprKind, FieldInit, MatchArm, Pattern, SelectArm, SelectArmKind, WithBinding,
    },
    stmt::{Stmt, StmtKind},
    NodeId,
};
use rask_ast::ty::TypeExpr;
use rask_types::Type;
use std::collections::HashMap;

/// Type substitutor - clones AST while replacing type parameters
struct TypeSubstitutor {
    /// Mapping from type parameter name to concrete type
    substitutions: HashMap<String, Type>,
    /// AT6: what each `T.Out` reads on this instance, worked out by the caller
    /// from `T`'s bound and its argument's conformance. Keyed `("T", "Out")`.
    projections: HashMap<(String, String), TypeExpr>,
    /// Counter for generating fresh NodeIds. Seeded by the caller so copies
    /// never reuse the original program's ids.
    next_node_id: u32,
    /// New node id -> the original node it was cloned from, so the checker's
    /// per-node records can be carried onto the copy.
    node_origin: HashMap<NodeId, NodeId>,
}

impl TypeSubstitutor {
    fn new(type_params: &[TypeParam], type_args: &[Type]) -> Self {
        let mut substitutions = HashMap::new();
        for (param, arg) in type_params.iter().zip(type_args.iter()) {
            substitutions.insert(param.name.clone(), arg.clone());
        }
        Self {
            substitutions,
            projections: HashMap::new(),
            next_node_id: 0,
            node_origin: HashMap::new(),
        }
    }

    fn fresh_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id += 1;
        id
    }

    /// A fresh id that remembers which original node it replaces.
    fn fresh_id_from(&mut self, origin: NodeId) -> NodeId {
        let id = self.fresh_id();
        self.node_origin.insert(id, origin);
        id
    }

    /// The written type with each type parameter replaced by its argument.
    ///
    /// Function types are walked too: `f: func() -> V` in a `Map<K, V>` method
    /// has to come out as `func() -> string` in the copy, or the call through
    /// it takes the return as a word (#887).
    fn substitute_type(&self, ty: &TypeExpr) -> TypeExpr {
        let ty = if self.projections.is_empty() {
            ty.clone()
        } else {
            ty.substitute_projections(&|head, tail| {
                self.projections.get(&(head.to_string(), tail.to_string())).cloned()
            })
        };
        ty.substitute(&|name| self.substitutions.get(name).map(Type::to_type_expr))
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
                ty: self.substitute_type(&f.ty),
                visibility: f.visibility,
                attrs: f.attrs.clone(),
                default: f.default.clone(),
                doc: f.doc.clone(),
            }).collect(),
            methods: s.methods.iter().map(|m| self.clone_fn_decl(m)).collect(),
            is_pub: s.is_pub,
            attrs: s.attrs.clone(),
            doc: s.doc.clone(),
        }
    }

    fn clone_enum_decl(&mut self, e: &rask_ast::decl::EnumDecl) -> rask_ast::decl::EnumDecl {
        rask_ast::decl::EnumDecl {
            name: e.name.clone(),
            type_params: Vec::new(), // Removed after instantiation
            variants: e.variants.iter().map(|v| rask_ast::decl::Variant {
                name: v.name.clone(),
                name_span: v.name_span,
                fields: v.fields.iter().map(|f| rask_ast::decl::Field {
                    name: f.name.clone(),
                    name_span: f.name_span.clone(),
                    ty: self.substitute_type(&f.ty),
                    visibility: f.visibility,
                    attrs: f.attrs.clone(),
                    default: f.default.clone(),
                    doc: f.doc.clone(),
                }).collect(),
                attrs: v.attrs.clone(),
                discriminant: v.discriminant,
            }).collect(),
            methods: e.methods.iter().map(|m| self.clone_fn_decl(m)).collect(),
            is_pub: e.is_pub,
            attrs: e.attrs.clone(),
            doc: e.doc.clone(),
            backing_type: e.backing_type.clone(),
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
                .map(|ty| self.substitute_type(ty)),
            body: fn_decl.body.iter().map(|s| self.clone_stmt(s)).collect(),
            is_pub: fn_decl.is_pub,
            is_private: fn_decl.is_private,
            is_comptime: fn_decl.is_comptime,
            is_unsafe: fn_decl.is_unsafe,
            abi: fn_decl.abi.clone(),
            attrs: fn_decl.attrs.clone(),
            doc: fn_decl.doc.clone(),
            span: fn_decl.span,
            decl_start: fn_decl.decl_start,
        }
    }

    fn clone_param(&mut self, param: &Param) -> Param {
        Param {
            name: param.name.clone(),
            name_span: param.name_span.clone(),
            ty: param.ty.as_ref().map(|t| self.substitute_type(t)),
            is_take: param.is_take,
            is_mutate: param.is_mutate, is_deleting: false,
            default: param.default.as_ref().map(|e| self.clone_expr(e)),
        }
    }

    // ── Statements ──────────────────────────────────────────────────

    fn clone_stmt(&mut self, stmt: &Stmt) -> Stmt {
        Stmt {
            id: self.fresh_id_from(stmt.id),
            kind: match &stmt.kind {
                StmtKind::Expr(e) => StmtKind::Expr(self.clone_expr(e)),

                StmtKind::Mut {
                    name,
                    name_span,
                    ty,
                    init,
                } => StmtKind::Mut {
                    name: name.clone(),
                    name_span: name_span.clone(),
                    ty: ty.as_ref().map(|t| self.substitute_type(t)),
                    init: self.clone_expr(init),
                },

                StmtKind::MutTuple { patterns, init } => StmtKind::MutTuple {
                    patterns: patterns.clone(),
                    init: self.clone_expr(init),
                },

                StmtKind::Let {
                    name,
                    name_span,
                    ty,
                    init,
                } => StmtKind::Let {
                    name: name.clone(),
                    name_span: name_span.clone(),
                    ty: ty.as_ref().map(|t| self.substitute_type(t)),
                    init: self.clone_expr(init),
                },

                StmtKind::LetTuple { patterns, init } => StmtKind::LetTuple {
                    patterns: patterns.clone(),
                    init: self.clone_expr(init),
                },

                StmtKind::LetStruct { pattern, init, is_mut } => StmtKind::LetStruct {
                    pattern: self.clone_pattern(pattern),
                    init: self.clone_expr(init),
                    is_mut: *is_mut,
                },

                StmtKind::Assign { target, value, op } => StmtKind::Assign {
                    target: self.clone_expr(target),
                    value: self.clone_expr(value),
                    op: *op,
                },

                StmtKind::Return(opt_expr) => {
                    StmtKind::Return(opt_expr.as_ref().map(|e| self.clone_expr(e)))
                }

                StmtKind::Break { label, value } => StmtKind::Break {
                    label: label.clone(),
                    value: value.as_ref().map(|e| self.clone_expr(e)),
                },

                StmtKind::Continue(label) => StmtKind::Continue(label.clone()),

                StmtKind::While { label, cond, body } => StmtKind::While {
                    label: label.clone(),
                    cond: self.clone_expr(cond),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },

                StmtKind::WhileLet {
                    label,
                    pattern,
                    expr,
                    body,
                } => StmtKind::WhileLet {
                    label: label.clone(),
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
                    mutate,
                    iter,
                    body,
                } => StmtKind::For {
                    label: label.clone(),
                    binding: binding.clone(),
                    mutate: *mutate,
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

                StmtKind::ComptimeFor { binding, iter, body } => {
                    StmtKind::ComptimeFor {
                        binding: binding.clone(),
                        iter: self.clone_expr(iter),
                        body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                    }
                }

                StmtKind::Discard { name, name_span } => StmtKind::Discard {
                    name: name.clone(),
                    name_span: name_span.clone(),
                },
            },
            span: stmt.span.clone(),
        }
    }

    // ── Expressions ─────────────────────────────────────────────────

    fn clone_expr(&mut self, expr: &Expr) -> Expr {
        Expr {
            id: self.fresh_id_from(expr.id),
            kind: match &expr.kind {
                // Literals
                ExprKind::Int(val, suffix) => ExprKind::Int(*val, *suffix),
                ExprKind::Float(val, suffix) => ExprKind::Float(*val, *suffix),
                ExprKind::String(s) => ExprKind::String(s.clone()),
                ExprKind::StringInterp(segments) => ExprKind::StringInterp(segments.clone()),
                ExprKind::Char(c) => ExprKind::Char(*c),
                ExprKind::Bool(b) => ExprKind::Bool(*b),
                ExprKind::Null => ExprKind::Null,
                ExprKind::None => ExprKind::None,

                // Variables
                ExprKind::Ident(name) => ExprKind::Ident(name.clone()),
                ExprKind::GenericName { name, type_args } => ExprKind::GenericName {
                    name: name.clone(),
                    type_args: type_args.iter().map(|t| self.substitute_type(t)).collect(),
                },

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
                    args: args.iter().map(|a| CallArg { name: a.name.clone(), mode: a.mode, expr: self.clone_expr(&a.expr) }).collect(),
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
                            .map(|t| self.substitute_type(t))
                            .collect()
                    }),
                    args: args.iter().map(|a| CallArg { name: a.name.clone(), mode: a.mode, expr: self.clone_expr(&a.expr) }).collect(),
                },

                // Access
                ExprKind::Field { object, field } => ExprKind::Field {
                    object: Box::new(self.clone_expr(object)),
                    field: field.clone(),
                },
                ExprKind::DynamicField { object, field_expr } => ExprKind::DynamicField {
                    object: Box::new(self.clone_expr(object)),
                    field_expr: Box::new(self.clone_expr(field_expr)),
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
                    else_binding,
                } => ExprKind::If {
                    cond: Box::new(self.clone_expr(cond)),
                    then_branch: Box::new(self.clone_expr(then_branch)),
                    else_branch: else_branch.as_ref().map(|e| Box::new(self.clone_expr(e))),
                    else_binding: else_binding.clone(),
                },
                ExprKind::IfLet {
                    expr,
                    pattern,
                    then_branch,
                    else_branch, else_binding } => ExprKind::IfLet {
                    expr: Box::new(self.clone_expr(expr)),
                    pattern: self.clone_pattern(pattern),
                    then_branch: Box::new(self.clone_expr(then_branch)),
                    else_branch: else_branch.as_ref().map(|e| Box::new(self.clone_expr(e))),
                    else_binding: else_binding.clone(),
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
                ExprKind::Try { expr: inner } => ExprKind::Try {
                    expr: Box::new(self.clone_expr(inner)),
                },
                ExprKind::Take { place } => ExprKind::Take {
                    place: Box::new(self.clone_expr(place)),
                },
                ExprKind::Catch { value, ref clause } => ExprKind::Catch {
                    value: Box::new(self.clone_expr(value)),
                    clause: rask_ast::expr::CatchClause {
                        binder: clause.binder.clone(),
                        body: Box::new(self.clone_expr(&clause.body)),
                    },
                },
                ExprKind::IsPresent { expr: inner, binding } => ExprKind::IsPresent {
                    expr: Box::new(self.clone_expr(inner)),
                    binding: binding.clone(),
                },
                ExprKind::Unwrap { expr: inner, message, bang } => ExprKind::Unwrap {
                    expr: Box::new(self.clone_expr(inner)),
                    message: message.clone(),
                    bang: *bang,
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
                    type_args,
                    fields,
                    spread,
                } => ExprKind::StructLit {
                    name: name.clone(),
                    type_args: type_args.iter().map(|t| self.substitute_type(t)).collect(),
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
                            name_span: p.name_span,
                            ty: p.ty.as_ref().map(|t| self.substitute_type(t)),
                            is_mutate: false,
                            is_take: false,
                        })
                        .collect(),
                    ret_ty: ret_ty.as_ref().map(|t| self.substitute_type(t)),
                    body: Box::new(self.clone_expr(body)),
                },

                // Type cast
                ExprKind::Cast { expr, ty } => ExprKind::Cast {
                    expr: Box::new(self.clone_expr(expr)),
                    ty: self.substitute_type(ty),
                },

                // Explicit conversion (CV5–CV10)
                ExprKind::Convert { expr, target, kind } => ExprKind::Convert {
                    expr: Box::new(self.clone_expr(expr)),
                    target: self.substitute_type(target),
                    kind: *kind,
                },

                // Context blocks
                ExprKind::UsingBlock { name, args, body } => ExprKind::UsingBlock {
                    name: name.clone(),
                    args: args.iter().map(|a| CallArg { name: a.name.clone(), mode: a.mode, expr: self.clone_expr(&a.expr) }).collect(),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },
                ExprKind::WithAs { bindings, body } => ExprKind::WithAs {
                    bindings: bindings
                        .iter()
                        .map(|b| WithBinding {
                            source: self.clone_expr(&b.source),
                            name: b.name.clone(),
                        })
                        .collect(),
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },

                // Unsafe / comptime
                ExprKind::Unsafe { body } => ExprKind::Unsafe {
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },
                ExprKind::Comptime { body } => ExprKind::Comptime {
                    body: body.iter().map(|s| self.clone_stmt(s)).collect(),
                },
                ExprKind::Loop { label, body } => ExprKind::Loop {
                    label: label.clone(),
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
            Pattern::Range { start, end } => Pattern::Range {
                start: start.clone(),
                end: end.clone(),
            },
            Pattern::TypePat { ty, binding } => Pattern::TypePat {
                ty: self.substitute_type(ty),
                binding: binding.clone(),
            },
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
///
/// Node IDs in the copy come from the caller's allocator so they can't collide
/// with the original program's — see [`instantiate_function_from`].
pub fn instantiate_function(decl: &Decl, type_args: &[Type]) -> Decl {
    let mut next = 0u32;
    instantiate_function_from(decl, type_args, &mut next).0
}

/// Instantiate, allocating node IDs from `next` and reporting where each one
/// came from.
///
/// Every copy used to number its nodes from zero. Those numbers are the key
/// into everything the checker recorded — types, dispatch targets, type
/// arguments — so an instantiated body didn't just lose that information, it
/// silently read *another* function's: node 7 of a generic copy answered with
/// whatever node 7 of the original program happened to be. Lowering compensated
/// with a layer of guessing from AST shape, which is why so much of it is
/// reconstruction rather than lookup.
///
/// Allocating from a shared counter above the program's range makes a miss a
/// miss. The returned map says which original node each copy came from, so the
/// recorded facts can be carried across instead.
/// The type-parameter names a declaration's arguments bind to, in order. Same
/// derivation `instantiate_function_from` uses, exposed so callers can map a
/// parameter name back to the argument it stands for.
pub fn type_param_names(decl: &Decl, type_args: &[Type]) -> Vec<String> {
    match &decl.kind {
        DeclKind::Fn(f) => {
            if f.type_params.len() < type_args.len() {
                rask_types::signature_type_param_names(f)
            } else {
                f.type_params.iter().map(|p| p.name.clone()).collect()
            }
        }
        // PC1 applies to field and payload types too, so a struct that never
        // wrote `<T>` still has parameters to bind (#913).
        DeclKind::Struct(s) => rask_types::struct_type_param_names(s),
        DeclKind::Enum(e) => rask_types::enum_type_param_names(e),
        _ => Vec::new(),
    }
}

/// Instantiate binding an explicit list of parameter names, rather than reading
/// them off the declaration.
///
/// A method on a generic type takes some of its parameters from the `extend`
/// header — `extend One<A>` gives `get` its `A`, and `get`'s own signature has no
/// record of that. The caller knows both lists, so it passes the joined one here
/// (#814).
pub fn instantiate_function_with_params(
    decl: &Decl,
    param_names: &[String],
    type_args: &[Type],
    projections: HashMap<(String, String), TypeExpr>,
    next_node_id: &mut u32,
) -> (Decl, HashMap<NodeId, NodeId>) {
    let params: Vec<TypeParam> = param_names
        .iter()
        .map(|name| TypeParam {
            name: name.clone(),
            is_comptime: false,
            comptime_type: None,
            bounds: Vec::new(),
            default: None,
        })
        .collect();
    let mut substitutor = TypeSubstitutor::new(&params, type_args);
    substitutor.projections = projections;
    substitutor.next_node_id = *next_node_id;
    let cloned = substitutor.clone_decl(decl);
    *next_node_id = substitutor.next_node_id;
    (cloned, substitutor.node_origin)
}

/// Apply an instantiation's type arguments to a checker type, as written.
///
/// The same substitution `instantiate_function_with_params` does to a copy's
/// signature, for a type the declaration didn't carry — the return type the
/// checker inferred, which can name a type parameter.
pub fn substitute_written_type(
    ty: &Type,
    param_names: &[String],
    type_args: &[Type],
) -> TypeExpr {
    let params: Vec<TypeParam> = param_names
        .iter()
        .map(|name| TypeParam {
            name: name.clone(),
            is_comptime: false,
            comptime_type: None,
            bounds: Vec::new(),
            default: None,
        })
        .collect();
    TypeSubstitutor::new(&params, type_args).substitute_type(&ty.to_type_expr())
}

/// Turn a PC1 name list back into `TypeParam`s, keeping whatever the explicit
/// `<T>` list already recorded (bounds, comptime-ness) for the names it has.
fn named_params(names: Vec<String>, declared: &[TypeParam]) -> Vec<TypeParam> {
    names
        .into_iter()
        .map(|name| {
            declared
                .iter()
                .find(|p| p.name == name)
                .cloned()
                .unwrap_or(TypeParam {
                    name,
                    is_comptime: false,
                    comptime_type: None,
                    bounds: Vec::new(),
                    default: None,
                })
        })
        .collect()
}

pub fn instantiate_function_from(
    decl: &Decl,
    type_args: &[Type],
    next_node_id: &mut u32,
) -> (Decl, HashMap<NodeId, NodeId>) {
    let implicit_params: Vec<TypeParam>;
    let type_params: &[TypeParam] = match &decl.kind {
        DeclKind::Fn(f) => {
            if f.type_params.len() < type_args.len() {
                // PC1: implicit single-letter type params — the checker's
                // call-site type args are ordered by the same shared list
                implicit_params = rask_types::signature_type_param_names(f)
                    .into_iter()
                    .map(|name| {
                        f.type_params
                            .iter()
                            .find(|p| p.name == name)
                            .cloned()
                            .unwrap_or(TypeParam {
                                name,
                                is_comptime: false,
                                comptime_type: None,
                                bounds: Vec::new(),
                                default: None,
                            })
                    })
                    .collect();
                &implicit_params
            } else {
                &f.type_params
            }
        }
        // Same PC1 widening as the `Fn` arm above: the implicit single letters
        // in field and payload types are parameters, and the checker's type
        // args are ordered by that same shared list. Reading only the explicit
        // `<T>` list here left an implicit-param struct with nothing to
        // substitute, so a function returning one handed back an unsubstituted
        // layout and the caller segfaulted reading it (#913).
        DeclKind::Struct(s) => {
            implicit_params = named_params(rask_types::struct_type_param_names(s), &s.type_params);
            &implicit_params
        }
        DeclKind::Enum(e) => {
            implicit_params = named_params(rask_types::enum_type_param_names(e), &e.type_params);
            &implicit_params
        }
        _ => {
            // No type parameters to substitute — return a clone
            return (decl.clone(), HashMap::new());
        }
    };

    let mut substitutor = TypeSubstitutor::new(type_params, type_args);
    substitutor.next_node_id = *next_node_id;
    let cloned = substitutor.clone_decl(decl);
    *next_node_id = substitutor.next_node_id;
    (cloned, substitutor.node_origin)
}
