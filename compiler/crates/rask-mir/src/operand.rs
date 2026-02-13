// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MIR operands and rvalues.

use crate::MirType;

pub use crate::function::LocalId;

/// MIR operand - value that can be used
#[derive(Debug, Clone)]
pub enum MirOperand {
    Local(LocalId),
    Constant(MirConst),
}

/// MIR constant value
#[derive(Debug, Clone)]
pub enum MirConst {
    Int(i64),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
}

/// MIR rvalue - right-hand side of assignment
#[derive(Debug, Clone)]
pub enum MirRValue {
    Use(MirOperand),
    Ref(LocalId),
    Deref(MirOperand),
    BinaryOp {
        op: BinOp,
        left: MirOperand,
        right: MirOperand,
    },
    UnaryOp {
        op: UnaryOp,
        operand: MirOperand,
    },
    Cast {
        value: MirOperand,
        target_ty: MirType,
    },
    Field {
        base: MirOperand,
        field_index: u32,
    },
    EnumTag {
        value: MirOperand,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
}

/// Function reference for calls
#[derive(Debug, Clone)]
pub struct FunctionRef {
    pub name: String,
}
