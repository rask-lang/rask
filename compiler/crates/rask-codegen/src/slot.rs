// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! A frame slot that holds a value at its own alignment.
//!
//! Cranelift keeps the frame 16-byte aligned and lays slots out from there, so
//! a slot asking for 64 gets an offset that is a multiple of 64 from a base
//! that is only a multiple of 16. A value aligned past 16 (`@align(64)`) gets
//! a slot `align` bytes larger and lives at the first aligned address in it.
//! That address is computed once, where the slot is made, and every access
//! goes through it.

use cranelift::prelude::*;
use cranelift_codegen::ir::{StackSlot, StackSlotData, StackSlotKind};
use cranelift_frontend::FunctionBuilder;

/// What a Cranelift frame guarantees on its own.
const FRAME_ALIGN: u32 = rask_mir::MALLOC_ALIGN;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LocalSlot {
    ss: StackSlot,
    /// The realigned address, for a slot past `FRAME_ALIGN`. `None` reads and
    /// writes the slot itself.
    base: Option<Value>,
}

impl LocalSlot {
    /// A slot for `size` bytes aligned to `align`. Past 16 the aligned address
    /// is computed here, so make the slot where that value dominates its uses
    /// (the entry block, for a local).
    pub(crate) fn new(builder: &mut FunctionBuilder, size: u32, align: u32) -> Self {
        let align = align.max(1);
        if align <= FRAME_ALIGN {
            let ss = builder.create_sized_stack_slot(StackSlotData::new(
                StackSlotKind::ExplicitSlot,
                size,
                align.trailing_zeros() as u8,
            ));
            return LocalSlot { ss, base: None };
        }
        let ss = builder.create_sized_stack_slot(StackSlotData::new(
            StackSlotKind::ExplicitSlot,
            size + align,
            FRAME_ALIGN.trailing_zeros() as u8,
        ));
        let raw = builder.ins().stack_addr(types::I64, ss, 0);
        let bumped = builder.ins().iadd_imm(raw, i64::from(align - 1));
        let base = builder.ins().band_imm(bumped, -i64::from(align));
        LocalSlot { ss, base: Some(base) }
    }

    pub(crate) fn plain(ss: StackSlot) -> Self {
        LocalSlot { ss, base: None }
    }

    pub(crate) fn addr(&self, builder: &mut FunctionBuilder, offset: i32) -> Value {
        match self.base {
            None => builder.ins().stack_addr(types::I64, self.ss, offset),
            Some(base) if offset == 0 => base,
            Some(base) => builder.ins().iadd_imm(base, i64::from(offset)),
        }
    }

    pub(crate) fn store(&self, builder: &mut FunctionBuilder, value: Value, offset: i32) {
        match self.base {
            None => {
                builder.ins().stack_store(value, self.ss, offset);
            }
            Some(base) => {
                builder.ins().store(MemFlags::trusted(), value, base, offset);
            }
        }
    }

    pub(crate) fn load(&self, builder: &mut FunctionBuilder, ty: Type, offset: i32) -> Value {
        match self.base {
            None => builder.ins().stack_load(ty, self.ss, offset),
            Some(base) => builder.ins().load(ty, MemFlags::trusted(), base, offset),
        }
    }
}
