// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR lowering - transform AST to MIR CFG.

mod expr;
mod stmt;

use crate::{BlockBuilder, MirFunction, MirOperand};
use rask_ast::{decl::Decl, expr::Expr, stmt::Stmt};
use std::collections::HashMap;

pub struct MirLowerer {
    builder: BlockBuilder,
    locals: HashMap<String, crate::LocalId>,
}

impl MirLowerer {
    pub fn lower_function(decl: &Decl) -> Result<MirFunction, LoweringError> {
        todo!("Implement function lowering")
    }

    fn lower_expr(&mut self, expr: &Expr) -> Result<MirOperand, LoweringError> {
        todo!("Implement expression lowering")
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), LoweringError> {
        todo!("Implement statement lowering")
    }
}

#[derive(Debug)]
pub enum LoweringError {
    UnresolvedVariable(String),
    UnresolvedGeneric(String),
    InvalidConstruct(String),
}
