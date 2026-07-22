// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Width-aware checked integer arithmetic (type.overflow OV1–OV4, SH1).
//!
//! The interpreter stores every integer type in `Value::Int(i64)`, so the
//! operand width isn't visible at the operation site. The type checker's
//! `node_types` map supplies it: the receiver of the desugared `a.add(b)`
//! keeps its original NodeId, so `node_types[object.id]` yields the static
//! integer type. From that we recover the width and range-check every
//! operation, panicking on overflow like the spec requires.
//!
//! `i128`/`u128` are their own `Value` variants and are checked directly in
//! `builtins/primitives.rs` — they never reach here.

use rask_ast::NodeId;
use rask_types::Type;

use super::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// The concrete integer width of the expression `id`, if the checker
    /// resolved it to i8..u64. Returns None for i128/u128, non-integers, and
    /// generic (un-monomorphized) code where no concrete width is known.
    pub(crate) fn int_width_of(&self, id: NodeId) -> Option<IntWidth> {
        self.node_types.get(&id).and_then(IntWidth::from_type)
    }

    /// Intercept the desugared arithmetic operator methods on `Value::Int`
    /// (i8..u64) and run them width-aware. Returns None to fall through to the
    /// normal dispatch (non-arithmetic method, unknown width, or non-int
    /// receiver). `recv_id` is the NodeId of the receiver expression.
    pub(crate) fn try_checked_int_arith(
        &self,
        receiver: &Value,
        recv_id: NodeId,
        method: &str,
        args: &[Value],
    ) -> Option<Result<Value, RuntimeError>> {
        let a = match receiver {
            Value::Int(a) => *a,
            _ => return None,
        };
        let w = self.int_width_of(recv_id)?;

        if method == "neg" {
            return Some(checked_neg(w, a).map(Value::Int));
        }
        let op = ArithOp::from_method(method)?;
        let b = match args.first() {
            Some(Value::Int(b)) => *b,
            _ => return None,
        };
        Some(checked_binop(w, op, a, b).map(Value::Int))
    }
}

/// A concrete integer width the interpreter can range-check (i8..u64).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct IntWidth {
    pub signed: bool,
    pub bits: u32,
}

/// The arithmetic operations that can overflow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Shl,
    Shr,
}

impl ArithOp {
    fn symbol(self) -> &'static str {
        match self {
            ArithOp::Add => "+",
            ArithOp::Sub => "-",
            ArithOp::Mul => "*",
            ArithOp::Div => "/",
            ArithOp::Rem => "%",
            ArithOp::Shl => "<<",
            ArithOp::Shr => ">>",
        }
    }

    /// Method name (post-desugar) for the operators that can overflow.
    pub fn from_method(name: &str) -> Option<ArithOp> {
        Some(match name {
            "add" => ArithOp::Add,
            "sub" => ArithOp::Sub,
            "mul" => ArithOp::Mul,
            "div" => ArithOp::Div,
            "rem" => ArithOp::Rem,
            "shl" => ArithOp::Shl,
            "shr" => ArithOp::Shr,
            _ => return None,
        })
    }
}

impl IntWidth {
    /// The width for a concrete integer type, or None for i128/u128 and
    /// non-integer / generic types (which are handled elsewhere or skipped).
    pub fn from_type(ty: &Type) -> Option<IntWidth> {
        Some(match ty {
            Type::I8 => IntWidth { signed: true, bits: 8 },
            Type::I16 => IntWidth { signed: true, bits: 16 },
            Type::I32 => IntWidth { signed: true, bits: 32 },
            Type::I64 => IntWidth { signed: true, bits: 64 },
            Type::U8 => IntWidth { signed: false, bits: 8 },
            Type::U16 => IntWidth { signed: false, bits: 16 },
            Type::U32 => IntWidth { signed: false, bits: 32 },
            Type::U64 => IntWidth { signed: false, bits: 64 },
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match (self.signed, self.bits) {
            (true, 8) => "i8",
            (true, 16) => "i16",
            (true, 32) => "i32",
            (true, 64) => "i64",
            (false, 8) => "u8",
            (false, 16) => "u16",
            (false, 32) => "u32",
            (false, 64) => "u64",
            _ => "int",
        }
    }

    fn min(self) -> i128 {
        if self.signed {
            -(1i128 << (self.bits - 1))
        } else {
            0
        }
    }

    fn max(self) -> i128 {
        if self.signed {
            (1i128 << (self.bits - 1)) - 1
        } else {
            (1i128 << self.bits) - 1
        }
    }

    /// Read the stored i64 as this type's logical value.
    /// Unsigned widths reinterpret the bit pattern (u64 values above
    /// i64::MAX are stored as negative i64).
    fn logical(self, raw: i64) -> i128 {
        if self.signed {
            raw as i128
        } else {
            (raw as u64) as i128
        }
    }

    /// Store a logical value back into the i64 slot (bit pattern for unsigned).
    fn store(self, val: i128) -> i64 {
        if self.signed {
            val as i64
        } else {
            (val as u64) as i64
        }
    }

    fn overflow(self, msg: String) -> RuntimeError {
        RuntimeError::IntegerOverflow(msg)
    }
}

/// Checked binary arithmetic for a known width. `a`/`b` are the raw i64 slots.
pub(crate) fn checked_binop(
    w: IntWidth,
    op: ArithOp,
    a: i64,
    b: i64,
) -> Result<i64, RuntimeError> {
    let la = w.logical(a);
    let lb = w.logical(b);

    // Shifts: only the shift amount is checked (SH1); the value wraps to width.
    if matches!(op, ArithOp::Shl | ArithOp::Shr) {
        if lb < 0 || lb >= w.bits as i128 {
            return Err(w.overflow(format!(
                "shift amount {} exceeds {} bit width ({})",
                lb, w.name(), w.bits
            )));
        }
        let shifted = match op {
            ArithOp::Shl => la.wrapping_shl(lb as u32),
            ArithOp::Shr => {
                if w.signed {
                    // Arithmetic shift on the sign-extended value.
                    (la as i128) >> (lb as u32)
                } else {
                    // Logical shift on the unsigned value.
                    ((la as u128) >> (lb as u32)) as i128
                }
            }
            _ => unreachable!(),
        };
        // Wrap the result into the type's width.
        return Ok(w.store(wrap_to_width(w, shifted)));
    }

    // Division / remainder: zero and signed MIN/-1 (OV2, OV3).
    if matches!(op, ArithOp::Div | ArithOp::Rem) {
        if lb == 0 {
            return Err(RuntimeError::DivisionByZero);
        }
        if w.signed && la == w.min() && lb == -1 {
            return Err(w.overflow(format!(
                "integer overflow: {} {} {} exceeds {} range [{}, {}]",
                la, op.symbol(), lb, w.name(), w.min(), w.max()
            )));
        }
        let result = match op {
            ArithOp::Div => la / lb,
            ArithOp::Rem => la % lb,
            _ => unreachable!(),
        };
        return range_check(w, op, la, lb, result);
    }

    // Add / sub / mul (OV1). i128 checked ops never spuriously overflow for
    // widths <= u64, so None here is a genuine out-of-range result.
    let result = match op {
        ArithOp::Add => la.checked_add(lb),
        ArithOp::Sub => la.checked_sub(lb),
        ArithOp::Mul => la.checked_mul(lb),
        _ => unreachable!(),
    };
    match result {
        Some(r) => range_check(w, op, la, lb, r),
        None => Err(w.overflow(format!(
            "integer overflow: {} {} {} exceeds {} range [{}, {}]",
            la, op.symbol(), lb, w.name(), w.min(), w.max()
        ))),
    }
}

/// Checked unary negation (OV1: `-i32.MIN`, and unsigned negation of nonzero).
pub(crate) fn checked_neg(w: IntWidth, a: i64) -> Result<i64, RuntimeError> {
    let la = w.logical(a);
    let result = -la;
    if result < w.min() || result > w.max() {
        Err(w.overflow(format!(
            "integer overflow: negating {} exceeds {} range [{}, {}]",
            la, w.name(), w.min(), w.max()
        )))
    } else {
        Ok(w.store(result))
    }
}

fn range_check(
    w: IntWidth,
    op: ArithOp,
    la: i128,
    lb: i128,
    result: i128,
) -> Result<i64, RuntimeError> {
    if result < w.min() || result > w.max() {
        Err(w.overflow(format!(
            "integer overflow: {} {} {} exceeds {} range [{}, {}]",
            la, op.symbol(), lb, w.name(), w.min(), w.max()
        )))
    } else {
        Ok(w.store(result))
    }
}

/// Mask a value into `w`'s bit width, sign-extending for signed types.
fn wrap_to_width(w: IntWidth, val: i128) -> i128 {
    if w.bits >= 128 {
        return val;
    }
    let mask = (1i128 << w.bits) - 1;
    let masked = val & mask;
    if w.signed && (masked & (1i128 << (w.bits - 1))) != 0 {
        masked - (1i128 << w.bits)
    } else {
        masked
    }
}
