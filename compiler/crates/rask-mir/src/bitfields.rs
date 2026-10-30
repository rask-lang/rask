// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Writes to `@bits(n)` fields (struct.c-interop/TM7).
//!
//! Lowering stores a bitfield like any field: its value, at its storage
//! unit's offset, with the field named on the store. Bitfields share a unit,
//! so that store would overwrite their neighbours. Here each one becomes a
//! read of the unit, the field's bits cleared, the value masked and shifted in,
//! and the unit written back. A value that doesn't fit in `n` bits panics
//! first, the way an overflowing integer does (type.overflow/OV1), so the
//! block splits at the store: the test, then the panic or the write.
//!
//! Every field store already names its field (`MirStmtKind::Store::field`),
//! so this is one walk after lowering rather than a case at each store site.

use rask_types::layout::{BitSlot, StructLayout};

use crate::operand::{BinOp, FieldAccess, MirConst};
use crate::{
    BlockId, FieldSlot, FunctionRef, LocalId, MirBlock, MirFunction, MirLocal, MirOperand, MirRValue, MirStmt,
    MirStmtKind, MirTerminator, MirTerminatorKind, MirType,
};

pub fn expand_stores(functions: &mut [MirFunction], layouts: &[StructLayout]) {
    if !layouts.iter().any(|l| l.fields.iter().any(|f| f.bits.is_some())) {
        return;
    }
    let bits_of = |slot: &FieldSlot| -> Option<BitSlot> {
        layouts.get(slot.layout as usize)?.fields.get(slot.field as usize)?.bits
    };
    for func in functions {
        let mut next_local = func.params.iter().chain(func.locals.iter()).map(|l| l.id.0 + 1).max().unwrap_or(0);
        let mut next_block = func.blocks.iter().map(|b| b.id.0 + 1).max().unwrap_or(0);
        let mut new_locals: Vec<MirLocal> = Vec::new();
        let mut i = 0;
        while i < func.blocks.len() {
            let at = func.blocks[i].statements.iter().position(|st| {
                matches!(&st.kind, MirStmtKind::Store { field: Some(slot), .. } if bits_of(slot).is_some())
            });
            let Some(at) = at else {
                i += 1;
                continue;
            };
            // Everything after the store moves to a block that runs once the
            // value is known to fit; this one ends in the test.
            let rest: Vec<MirStmt> = func.blocks[i].statements.split_off(at + 1);
            let store = func.blocks[i].statements.pop().expect("the store is there");
            let MirStmtKind::Store { addr, offset, value, store_size, field: Some(slot) } = store.kind else {
                unreachable!("found as a bitfield store")
            };
            let bits = bits_of(&slot).expect("found as a bitfield store");
            let span = store.span;
            let ty = unit_type(store_size, bits.signed);
            let unit_bits = (store_size * 8) as i64;
            let mask = bits.mask() as i64;
            let int = |v: i64| MirOperand::Constant(MirConst::Int(v));
            let mut temp = |ty: MirType| {
                let id = LocalId(next_local);
                next_local += 1;
                new_locals.push(MirLocal { id, name: None, ty, is_param: false });
                id
            };
            let unit = temp(ty.clone());
            let mut op = |stmts: &mut Vec<MirStmt>, op: BinOp, left: MirOperand, right: MirOperand, ty: MirType| {
                let dst = temp(ty);
                stmts.push(MirStmt::new(MirStmtKind::Assign { dst, rvalue: MirRValue::BinaryOp { op, left, right } }, span));
                MirOperand::Local(dst)
            };

            // Doesn't fit: masking it (or, signed, sign-extending it from the
            // width) changes it.
            let mut check: Vec<MirStmt> = Vec::new();
            let fitted = if bits.signed {
                let k = unit_bits - bits.width as i64;
                let up = op(&mut check, BinOp::Shl, value.clone(), int(k), ty.clone());
                op(&mut check, BinOp::Shr, up, int(k), ty.clone())
            } else {
                op(&mut check, BinOp::BitAnd, value.clone(), int(mask), ty.clone())
            };
            let over = op(&mut check, BinOp::Ne, fitted, value.clone(), MirType::Bool);

            let mut write: Vec<MirStmt> = Vec::new();
            write.push(MirStmt::new(
                MirStmtKind::Assign {
                    dst: unit,
                    rvalue: MirRValue::Field {
                        base: MirOperand::Local(addr),
                        field_index: slot.field,
                        byte_offset: Some(offset),
                        access: FieldAccess::Sized(store_size),
                    },
                },
                span,
            ));
            let keep = !((bits.mask() << bits.shift) as i64);
            let cleared = op(&mut write, BinOp::BitAnd, MirOperand::Local(unit), int(keep), ty.clone());
            let masked = op(&mut write, BinOp::BitAnd, value, int(mask), ty.clone());
            let placed = if bits.shift > 0 {
                op(&mut write, BinOp::Shl, masked, int(bits.shift as i64), ty.clone())
            } else {
                masked
            };
            let merged = op(&mut write, BinOp::BitOr, cleared, placed, ty.clone());
            write.push(MirStmt::new(MirStmtKind::Store { addr, offset, value: merged, store_size, field: None }, span));
            write.extend(rest);

            let panic_id = BlockId(next_block);
            let cont_id = BlockId(next_block + 1);
            next_block += 2;
            let terminator = std::mem::replace(
                &mut func.blocks[i].terminator,
                MirTerminator::new(MirTerminatorKind::Branch { cond: over, then_block: panic_id, else_block: cont_id }, span),
            );
            func.blocks[i].statements.extend(check);
            func.blocks.push(MirBlock {
                id: panic_id,
                statements: vec![MirStmt::new(
                    MirStmtKind::Call {
                        dst: None,
                        func: FunctionRef::internal("panic".to_string()),
                        args: vec![MirOperand::Constant(MirConst::String(format!(
                            "value doesn't fit in a {}-bit field",
                            bits.width
                        )))],
                    },
                    span,
                )],
                terminator: MirTerminator::new(MirTerminatorKind::Unreachable, span),
            });
            func.blocks.push(MirBlock { id: cont_id, statements: write, terminator });
            i += 1;
        }
        func.locals.extend(new_locals);
    }
}

fn unit_type(size: u32, signed: bool) -> MirType {
    match (size, signed) {
        (1, true) => MirType::I8,
        (1, false) => MirType::U8,
        (2, true) => MirType::I16,
        (2, false) => MirType::U16,
        (4, true) => MirType::I32,
        (4, false) => MirType::U32,
        (_, true) => MirType::I64,
        (_, false) => MirType::U64,
    }
}
