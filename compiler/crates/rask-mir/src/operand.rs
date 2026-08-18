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
    /// A 128-bit constant, already widened correctly for its type: an `i128`
    /// sign-extends from the literal, a `u128` zero-extends. The distinction is
    /// only visible here, where the MIR type is still in hand — Cranelift's
    /// integer types carry no signedness (#762).
    Int128(i128),
    Float(f64),
    Bool(bool),
    Char(char),
    String(String),
}

/// How a field read hands its value back.
///
/// This used to be a bare `Option<u32>` called `field_size`, carrying two
/// different facts depending on which branch of codegen read it: "how many
/// bytes" in one place, "MIR already decided this is an aggregate" in another.
/// The two only stayed apart because a byte-offset check happened to separate
/// them.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldAccess {
    /// No layout info — read a word at `field_index * 8`.
    Word,
    /// The field is this many bytes. It comes back loaded if it fits a
    /// register, and as an address if it doesn't.
    Sized(u32),
    /// The field lives in place — hand back its address whatever its size.
    /// A `T? or E` payload is this: `ok` and `err` can disagree about being an
    /// aggregate, so the size alone can't decide it (#383/#389).
    InPlace(u32),
}

impl FieldAccess {
    /// How a field of this type comes back. Aggregates live in their own
    /// storage, so the address is the answer even for the small ones — a
    /// fieldless enum is one byte, and loading that byte as a value handed
    /// codegen a tag where it expected a pointer (#561).
    pub fn for_field(ty: &MirType, size: u32) -> FieldAccess {
        if ty.passed_by_address() {
            FieldAccess::InPlace(size)
        } else {
            FieldAccess::Sized(size)
        }
    }

    /// Does reading this field yield an address rather than a loaded value?
    pub fn is_address(&self) -> bool {
        match self {
            FieldAccess::Word => false,
            FieldAccess::Sized(size) => *size > 8,
            FieldAccess::InPlace(_) => true,
        }
    }

    /// The field's size in bytes, when it's known.
    pub fn size(&self) -> Option<u32> {
        match self {
            FieldAccess::Word => None,
            FieldAccess::Sized(size) | FieldAccess::InPlace(size) => Some(*size),
        }
    }
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
    /// Explicit lossy conversion (type.primitives CV5–CV10). Carries the source
    /// type so codegen has signedness/width without re-deriving it.
    Convert {
        value: MirOperand,
        source_ty: MirType,
        target_ty: MirType,
        kind: rask_ast::expr::ConvertKind,
    },
    Field {
        base: MirOperand,
        field_index: u32,
        /// Pre-computed byte offset from struct layout, when available.
        byte_offset: Option<u32>,
        /// How the field comes back.
        access: FieldAccess,
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
    /// std.bits B1 — rotation, distinct from Shl/Shr in that bits wrap around
    /// the receiver's width instead of falling off the end.
    RotateLeft,
    RotateRight,
}

#[derive(Debug, Clone, Copy)]
pub enum UnaryOp {
    Neg,
    Not,
    BitNot,
    // std.bits B1. These map to single machine instructions, so they're MIR
    // ops rather than runtime calls — and because codegen carries the real
    // width per value, each one counts over the receiver's own type rather
    // than whatever register it happens to sit in.
    // `count_zeros`/`leading_ones`/`trailing_ones` compose from these with
    // BitNot instead of getting variants of their own.
    CountOnes,
    LeadingZeros,
    TrailingZeros,
    ReverseBits,
    SwapBytes,
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
