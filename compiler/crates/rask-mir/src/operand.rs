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
        /// Pre-computed byte offset and size from struct layout (when available).
        /// Codegen uses byte_offset directly; size > 8 means aggregate (return address).
        byte_offset: Option<u32>,
        field_size: Option<u32>,
    },
    EnumTag {
        value: MirOperand,
    },
    /// Load element from a fixed-size array: base_ptr + index * elem_size
    ArrayIndex {
        base: MirOperand,
        index: MirOperand,
        elem_size: u32,
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
    /// True for extern "C" functions — bypasses stdlib dispatch adaptation.
    pub is_extern: bool,
}

impl FunctionRef {
    /// Internal Rask or stdlib call.
    pub fn internal(name: String) -> Self {
        Self { name, is_extern: false }
    }

    /// Extern "C" call.
    pub fn extern_c(name: String) -> Self {
        Self { name, is_extern: true }
    }
}
