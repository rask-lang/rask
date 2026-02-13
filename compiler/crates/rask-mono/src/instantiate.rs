// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Function instantiation - clone AST and substitute type parameters.

use rask_ast::{
    decl::{Decl, DeclKind, FnDecl, Param, TypeParam},
    expr::{Expr, ExprKind},
    stmt::{Stmt, StmtKind},
    NodeId, Span,
};
use rask_types::Type;
use std::collections::HashMap;

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

    /// Substitute type parameter name with concrete type
    fn substitute_type_string(&self, type_str: &str) -> String {
        // Simple substitution - just replace type parameter names
        // TODO: Handle complex types like Vec<T>, Result<T, E>
        if let Some(ty) = self.substitutions.get(type_str) {
            format!("{:?}", ty) // Placeholder - need proper type string formatting
        } else {
            type_str.to_string()
        }
    }

    /// Clone declaration with type substitution
    fn clone_decl(&mut self, decl: &Decl) -> Decl {
        Decl {
            id: self.fresh_id(),
            kind: match &decl.kind {
                DeclKind::Fn(fn_decl) => DeclKind::Fn(self.clone_fn_decl(fn_decl)),
                _ => panic!("Only function declarations supported for now"),
            },
            span: decl.span.clone(),
        }
    }

    fn clone_fn_decl(&mut self, fn_decl: &FnDecl) -> FnDecl {
        FnDecl {
            name: fn_decl.name.clone(),
            type_params: Vec::new(), // Removed after instantiation
            params: fn_decl
                .params
                .iter()
                .map(|p| self.clone_param(p))
                .collect(),
            ret_ty: fn_decl
                .ret_ty
                .as_ref()
                .map(|ty| self.substitute_type_string(ty)),
            context_clauses: fn_decl.context_clauses.clone(), // TODO: Substitute
            body: fn_decl
                .body
                .iter()
                .map(|s| self.clone_stmt(s))
                .collect(),
            is_pub: fn_decl.is_pub,
            is_comptime: fn_decl.is_comptime,
            is_unsafe: fn_decl.is_unsafe,
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

    /// Clone statement with type substitution
    fn clone_stmt(&mut self, stmt: &Stmt) -> Stmt {
        Stmt {
            id: self.fresh_id(),
            kind: match &stmt.kind {
                StmtKind::Expr(e) => StmtKind::Expr(self.clone_expr(e)),
                StmtKind::Let { name, name_span, ty, init } => StmtKind::Let {
                    name: name.clone(),
                    name_span: name_span.clone(),
                    ty: ty.as_ref().map(|t| self.substitute_type_string(t)),
                    init: self.clone_expr(init),
                },
                StmtKind::Const { name, name_span, ty, init } => StmtKind::Const {
                    name: name.clone(),
                    name_span: name_span.clone(),
                    ty: ty.as_ref().map(|t| self.substitute_type_string(t)),
                    init: self.clone_expr(init),
                },
                StmtKind::Return(opt_expr) => {
                    StmtKind::Return(opt_expr.as_ref().map(|e| self.clone_expr(e)))
                }
                StmtKind::Assign { target, value } => StmtKind::Assign {
                    target: self.clone_expr(target),
                    value: self.clone_expr(value),
                },
                // TODO: Handle all statement variants
                _ => panic!("Statement variant not yet implemented: {:?}", stmt.kind),
            },
            span: stmt.span.clone(),
        }
    }

    /// Clone expression with type substitution
    fn clone_expr(&mut self, expr: &Expr) -> Expr {
        Expr {
            id: self.fresh_id(),
            kind: match &expr.kind {
                ExprKind::Int(val, suffix) => ExprKind::Int(*val, *suffix),
                ExprKind::Float(val, suffix) => ExprKind::Float(*val, *suffix),
                ExprKind::String(s) => ExprKind::String(s.clone()),
                ExprKind::Char(c) => ExprKind::Char(*c),
                ExprKind::Bool(b) => ExprKind::Bool(*b),
                ExprKind::Ident(name) => ExprKind::Ident(name.clone()),
                ExprKind::Binary { op, left, right } => ExprKind::Binary {
                    op: *op,
                    left: Box::new(self.clone_expr(left)),
                    right: Box::new(self.clone_expr(right)),
                },
                ExprKind::Unary { op, operand } => ExprKind::Unary {
                    op: *op,
                    operand: Box::new(self.clone_expr(operand)),
                },
                ExprKind::Call { func, args } => ExprKind::Call {
                    func: Box::new(self.clone_expr(func)),
                    args: args.iter().map(|a| self.clone_expr(a)).collect(),
                },
                ExprKind::Block(stmts) => {
                    ExprKind::Block(stmts.iter().map(|s| self.clone_stmt(s)).collect())
                }
                // TODO: Handle all expression variants
                _ => panic!("Expression variant not yet implemented: {:?}", expr.kind),
            },
            span: expr.span.clone(),
        }
    }
}

/// Instantiate a generic function with concrete type arguments
///
/// Clones the function AST and replaces all type parameters with concrete types
pub fn instantiate_function(decl: &Decl, type_args: &[Type]) -> Decl {
    let fn_decl = match &decl.kind {
        DeclKind::Fn(f) => f,
        _ => panic!("Expected function declaration"),
    };

    let mut substitutor = TypeSubstitutor::new(&fn_decl.type_params, type_args);
    substitutor.clone_decl(decl)
}
