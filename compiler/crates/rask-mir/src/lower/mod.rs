// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR lowering - transform AST to MIR CFG.

mod expr;
mod stmt;

use crate::{
    operand::MirConst, BlockBuilder, FunctionRef, MirFunction, MirOperand, MirRValue, MirStmt,
    MirTerminator, MirType,
};
use rask_ast::{
    decl::{Decl, DeclKind},
    expr::{BinOp, Expr, ExprKind, UnaryOp},
    stmt::{Stmt, StmtKind},
};
use std::collections::HashMap;

pub struct MirLowerer {
    builder: BlockBuilder,
    locals: HashMap<String, crate::LocalId>,
}

impl MirLowerer {
    pub fn lower_function(decl: &Decl) -> Result<MirFunction, LoweringError> {
        let fn_decl = match &decl.kind {
            DeclKind::Fn(f) => f,
            _ => {
                return Err(LoweringError::InvalidConstruct(
                    "Expected function declaration".to_string(),
                ))
            }
        };

        // TODO: Parse return type string to MirType
        let ret_ty = MirType::Void; // Placeholder

        let mut lowerer = MirLowerer {
            builder: BlockBuilder::new(fn_decl.name.clone(), ret_ty),
            locals: HashMap::new(),
        };

        // Add parameters
        for param in &fn_decl.params {
            // TODO: Parse param type
            let param_ty = MirType::I32; // Placeholder
            let local_id = lowerer.builder.add_param(param.name.clone(), param_ty);
            lowerer.locals.insert(param.name.clone(), local_id);
        }

        // Lower function body
        for stmt in &fn_decl.body {
            lowerer.lower_stmt(stmt)?;
        }

        // Ensure function ends with return (add implicit return if missing)
        // TODO: Check if last statement is already a return

        Ok(lowerer.builder.finish())
    }

    fn lower_expr(&mut self, expr: &Expr) -> Result<MirOperand, LoweringError> {
        match &expr.kind {
            // Literals - map directly to constants
            ExprKind::Int(val, _suffix) => Ok(MirOperand::Constant(MirConst::Int(*val))),
            ExprKind::Float(val, _suffix) => Ok(MirOperand::Constant(MirConst::Float(*val))),
            ExprKind::String(s) => Ok(MirOperand::Constant(MirConst::String(s.clone()))),
            ExprKind::Char(c) => Ok(MirOperand::Constant(MirConst::Char(*c))),
            ExprKind::Bool(b) => Ok(MirOperand::Constant(MirConst::Bool(*b))),

            // Variable reference - lookup in locals
            ExprKind::Ident(name) => self
                .locals
                .get(name)
                .copied()
                .map(MirOperand::Local)
                .ok_or_else(|| LoweringError::UnresolvedVariable(name.clone())),

            // Binary operations - lower to method calls
            ExprKind::Binary { op, left, right } => {
                let left_op = self.lower_expr(left)?;
                let right_op = self.lower_expr(right)?;

                // Allocate temporary for result
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type

                // Emit call to operator method (e.g., add, sub, mul)
                let method_name = binop_method_name(*op);
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef {
                        name: method_name,
                    },
                    args: vec![left_op, right_op],
                });

                Ok(MirOperand::Local(result_local))
            }

            // Unary operations
            ExprKind::Unary { op, operand } => {
                let operand_op = self.lower_expr(operand)?;
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type

                let method_name = unaryop_method_name(*op);
                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef {
                        name: method_name,
                    },
                    args: vec![operand_op],
                });

                Ok(MirOperand::Local(result_local))
            }

            // Function call
            ExprKind::Call { func, args } => {
                // Lower function expression (for now, assume it's an identifier)
                let func_name = match &func.kind {
                    ExprKind::Ident(name) => name.clone(),
                    _ => {
                        return Err(LoweringError::InvalidConstruct(
                            "Complex function expressions not yet supported".to_string(),
                        ))
                    }
                };

                // Lower arguments
                let arg_operands: Result<Vec<_>, _> =
                    args.iter().map(|a| self.lower_expr(a)).collect();
                let arg_operands = arg_operands?;

                // Allocate temporary for result
                let result_local = self.builder.alloc_temp(MirType::I32); // TODO: Infer type

                self.builder.push_stmt(MirStmt::Call {
                    dst: Some(result_local),
                    func: FunctionRef { name: func_name },
                    args: arg_operands,
                });

                Ok(MirOperand::Local(result_local))
            }

            _ => Err(LoweringError::InvalidConstruct(format!(
                "Expression variant not yet implemented: {:?}",
                expr.kind
            ))),
        }
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LoweringError> {
        match &stmt.kind {
            // Expression statement
            StmtKind::Expr(e) => {
                self.lower_expr(e)?;
                Ok(())
            }

            // Let binding
            StmtKind::Let { name, init, .. } => {
                let init_op = self.lower_expr(init)?;
                // TODO: Parse type
                let var_ty = MirType::I32; // Placeholder
                let local_id = self.builder.alloc_local(name.clone(), var_ty);
                self.locals.insert(name.clone(), local_id);

                self.builder.push_stmt(MirStmt::Assign {
                    dst: local_id,
                    rvalue: MirRValue::Use(init_op),
                });
                Ok(())
            }

            // Const binding
            StmtKind::Const { name, init, .. } => {
                let init_op = self.lower_expr(init)?;
                let var_ty = MirType::I32; // Placeholder
                let local_id = self.builder.alloc_local(name.clone(), var_ty);
                self.locals.insert(name.clone(), local_id);

                self.builder.push_stmt(MirStmt::Assign {
                    dst: local_id,
                    rvalue: MirRValue::Use(init_op),
                });
                Ok(())
            }

            // Return statement
            StmtKind::Return(opt_expr) => {
                let value = if let Some(e) = opt_expr {
                    Some(self.lower_expr(e)?)
                } else {
                    None
                };
                self.builder.terminate(MirTerminator::Return { value });
                Ok(())
            }

            _ => Err(LoweringError::InvalidConstruct(format!(
                "Statement variant not yet implemented: {:?}",
                stmt.kind
            ))),
        }
    }
}

/// Get method name for binary operator
fn binop_method_name(op: BinOp) -> String {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Mod => "mod",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Gt => "gt",
        BinOp::Le => "le",
        BinOp::Ge => "ge",
        BinOp::And => "and",
        BinOp::Or => "or",
        BinOp::BitAnd => "bitand",
        BinOp::BitOr => "bitor",
        BinOp::BitXor => "bitxor",
        BinOp::Shl => "shl",
        BinOp::Shr => "shr",
    }
    .to_string()
}

/// Get method name for unary operator
fn unaryop_method_name(op: UnaryOp) -> String {
    match op {
        UnaryOp::Neg => "neg",
        UnaryOp::Not => "not",
        UnaryOp::BitNot => "bitnot",
        UnaryOp::Ref => "ref",
        UnaryOp::Deref => "deref",
    }
    .to_string()
}

#[derive(Debug)]
pub enum LoweringError {
    UnresolvedVariable(String),
    UnresolvedGeneric(String),
    InvalidConstruct(String),
}
