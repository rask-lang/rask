// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Display implementations for MIR types.

use crate::*;
use crate::operand::{BinOp, UnaryOp, MirConst};
use std::fmt;

impl fmt::Display for MirType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MirType::Void => write!(f, "void"),
            MirType::Bool => write!(f, "bool"),
            MirType::I8 => write!(f, "i8"),
            MirType::I16 => write!(f, "i16"),
            MirType::I32 => write!(f, "i32"),
            MirType::I64 => write!(f, "i64"),
            MirType::U8 => write!(f, "u8"),
            MirType::U16 => write!(f, "u16"),
            MirType::U32 => write!(f, "u32"),
            MirType::U64 => write!(f, "u64"),
            MirType::F32 => write!(f, "f32"),
            MirType::F64 => write!(f, "f64"),
            MirType::Char => write!(f, "char"),
            MirType::Ptr => write!(f, "ptr"),
            MirType::String => write!(f, "string"),
            MirType::Struct(id) => write!(f, "struct#{}", id.0),
            MirType::Enum(id) => write!(f, "enum#{}", id.0),
            MirType::Array { elem, len } => write!(f, "[{}; {}]", elem, len),
            MirType::FuncPtr(id) => write!(f, "fn#{}", id.0),
        }
    }
}

impl fmt::Display for MirOperand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MirOperand::Local(id) => write!(f, "_{}", id.0),
            MirOperand::Constant(c) => write!(f, "{}", c),
        }
    }
}

impl fmt::Display for MirConst {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MirConst::Int(v) => write!(f, "{}", v),
            MirConst::Float(v) => write!(f, "{}", v),
            MirConst::Bool(v) => write!(f, "{}", v),
            MirConst::Char(c) => write!(f, "'{}'", c),
            MirConst::String(s) => write!(f, "\"{}\"", s),
        }
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sym = match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Mod => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Gt => ">",
            BinOp::Le => "<=",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
            BinOp::BitAnd => "&",
            BinOp::BitOr => "|",
            BinOp::BitXor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        };
        write!(f, "{}", sym)
    }
}

impl fmt::Display for UnaryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sym = match self {
            UnaryOp::Neg => "-",
            UnaryOp::Not => "!",
            UnaryOp::BitNot => "~",
        };
        write!(f, "{}", sym)
    }
}

impl fmt::Display for MirRValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MirRValue::Use(op) => write!(f, "{}", op),
            MirRValue::Ref(id) => write!(f, "&_{}", id.0),
            MirRValue::Deref(op) => write!(f, "*{}", op),
            MirRValue::BinaryOp { op, left, right } => {
                write!(f, "{} {} {}", left, op, right)
            }
            MirRValue::UnaryOp { op, operand } => {
                write!(f, "{}{}", op, operand)
            }
            MirRValue::Cast { value, target_ty } => {
                write!(f, "{} as {}", value, target_ty)
            }
            MirRValue::Field { base, field_index } => {
                write!(f, "{}.{}", base, field_index)
            }
            MirRValue::EnumTag { value } => {
                write!(f, "tag({})", value)
            }
        }
    }
}

impl fmt::Display for MirStmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MirStmt::Assign { dst, rvalue } => {
                write!(f, "_{} = {}", dst.0, rvalue)
            }
            MirStmt::Store { addr, offset, value } => {
                write!(f, "*(_{}+{}) = {}", addr.0, offset, value)
            }
            MirStmt::Call { dst, func, args } => {
                if let Some(d) = dst {
                    write!(f, "_{} = ", d.0)?;
                }
                write!(f, "{}(", func.name)?;
                for (i, arg) in args.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}", arg)?;
                }
                write!(f, ")")
            }
            MirStmt::ResourceRegister { dst, type_name, scope_depth } => {
                write!(f, "_{} = resource_register({}, depth={})", dst.0, type_name, scope_depth)
            }
            MirStmt::ResourceConsume { resource_id } => {
                write!(f, "resource_consume(_{})", resource_id.0)
            }
            MirStmt::ResourceScopeCheck { scope_depth } => {
                write!(f, "resource_scope_check(depth={})", scope_depth)
            }
            MirStmt::EnsurePush { cleanup_block } => {
                write!(f, "ensure_push(bb{})", cleanup_block.0)
            }
            MirStmt::EnsurePop => write!(f, "ensure_pop"),
            MirStmt::PoolCheckedAccess { dst, pool, handle } => {
                write!(f, "_{} = pool_access(_{}[_{}])", dst.0, pool.0, handle.0)
            }
            MirStmt::SourceLocation { line, col } => {
                write!(f, "// {}:{}", line, col)
            }
            MirStmt::ClosureCreate { dst, func_name, captures } => {
                write!(f, "_{} = closure({}, [", dst.0, func_name)?;
                for (i, cap) in captures.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "_{}@{}", cap.local_id.0, cap.offset)?;
                }
                write!(f, "])")
            }
            MirStmt::ClosureCall { dst, closure, args } => {
                if let Some(d) = dst {
                    write!(f, "_{} = ", d.0)?;
                }
                write!(f, "closure_call(_{}", closure.0)?;
                for arg in args {
                    write!(f, ", {}", arg)?;
                }
                write!(f, ")")
            }
            MirStmt::LoadCapture { dst, env_ptr, offset } => {
                write!(f, "_{} = load_capture(_{}+{})", dst.0, env_ptr.0, offset)
            }
        }
    }
}

impl fmt::Display for MirTerminator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MirTerminator::Return { value: Some(v) } => write!(f, "return {}", v),
            MirTerminator::Return { value: None } => write!(f, "return"),
            MirTerminator::Goto { target } => write!(f, "goto bb{}", target.0),
            MirTerminator::Branch { cond, then_block, else_block } => {
                write!(f, "if {} then bb{} else bb{}", cond, then_block.0, else_block.0)
            }
            MirTerminator::Switch { value, cases, default } => {
                write!(f, "switch {} [", value)?;
                for (i, (val, block)) in cases.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "{}: bb{}", val, block.0)?;
                }
                write!(f, ", default: bb{}]", default.0)
            }
            MirTerminator::Unreachable => write!(f, "unreachable"),
            MirTerminator::CleanupReturn { value, cleanup_chain } => {
                if let Some(v) = value {
                    write!(f, "cleanup_return {} [", v)?;
                } else {
                    write!(f, "cleanup_return [",)?;
                }
                for (i, b) in cleanup_chain.iter().enumerate() {
                    if i > 0 { write!(f, ", ")?; }
                    write!(f, "bb{}", b.0)?;
                }
                write!(f, "]")
            }
        }
    }
}

impl fmt::Display for MirFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Signature
        write!(f, "func {}(", self.name)?;
        for (i, p) in self.params.iter().enumerate() {
            if i > 0 { write!(f, ", ")?; }
            if let Some(name) = &p.name {
                write!(f, "{}: {}", name, p.ty)?;
            } else {
                write!(f, "_{}: {}", p.id.0, p.ty)?;
            }
        }
        writeln!(f, ") -> {} {{", self.ret_ty)?;

        // Locals (non-param)
        for local in &self.locals {
            if !local.is_param {
                if let Some(name) = &local.name {
                    writeln!(f, "  let {}: {}  // _{}", name, local.ty, local.id.0)?;
                } else {
                    writeln!(f, "  let _{}: {}", local.id.0, local.ty)?;
                }
            }
        }
        if self.locals.iter().any(|l| !l.is_param) {
            writeln!(f)?;
        }

        // Blocks
        for block in &self.blocks {
            writeln!(f, "  bb{}:", block.id.0)?;
            for stmt in &block.statements {
                writeln!(f, "    {}", stmt)?;
            }
            writeln!(f, "    {}", block.terminator)?;
        }
        write!(f, "}}")
    }
}
