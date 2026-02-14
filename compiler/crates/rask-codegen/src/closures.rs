// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Closure environment support — allocation, capture storage, and indirect calls.
//!
//! A closure is represented as a 16-byte struct:
//!   [0..8]  function pointer (address of the closure's compiled code)
//!   [8..16] environment pointer (address of captured variable storage)
//!
//! The environment is a stack-allocated struct containing captured variables
//! at known offsets. When calling through a closure, the environment pointer
//! is passed as an implicit first argument to the function.

use cranelift::prelude::*;
use cranelift_codegen::ir::{InstBuilder, MemFlags, StackSlotData, StackSlotKind};
use cranelift_frontend::FunctionBuilder;
use rask_mir::LocalId;
use std::collections::HashMap;

/// Closure struct layout: { func_ptr: i64, env_ptr: i64 }
pub const CLOSURE_SIZE: u32 = 16;
pub const CLOSURE_FUNC_OFFSET: i32 = 0;
pub const CLOSURE_ENV_OFFSET: i32 = 8;

/// Tracks captured variables and their layout in a closure environment.
pub struct ClosureEnvLayout {
    /// Total environment size in bytes
    pub size: u32,
    /// Captured variables: (local_id, offset, byte_size)
    pub captures: Vec<CaptureInfo>,
}

pub struct CaptureInfo {
    pub local_id: LocalId,
    pub offset: u32,
    pub size: u32,
}

impl ClosureEnvLayout {
    pub fn new() -> Self {
        ClosureEnvLayout {
            size: 0,
            captures: Vec::new(),
        }
    }

    /// Add a captured variable to the environment layout.
    /// Returns the offset where the variable will be stored.
    pub fn add_capture(&mut self, local_id: LocalId, size: u32) -> u32 {
        // Align to 8 bytes
        let offset = (self.size + 7) & !7;
        self.captures.push(CaptureInfo {
            local_id,
            offset,
            size,
        });
        self.size = offset + size;
        offset
    }
}

/// Allocate a closure environment on the stack and store captured values.
///
/// Returns the environment pointer (i64 address of the stack slot).
pub fn allocate_env(
    builder: &mut FunctionBuilder,
    layout: &ClosureEnvLayout,
    var_map: &HashMap<LocalId, Variable>,
) -> Value {
    if layout.size == 0 {
        // No captures — use null pointer
        return builder.ins().iconst(types::I64, 0);
    }

    let ss = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        layout.size,
        0,
    ));
    let env_ptr = builder.ins().stack_addr(types::I64, ss, 0);

    // Store each captured variable into the environment
    for capture in &layout.captures {
        if let Some(var) = var_map.get(&capture.local_id) {
            let val = builder.use_var(*var);
            builder
                .ins()
                .store(MemFlags::new(), val, env_ptr, capture.offset as i32);
        }
    }

    env_ptr
}

/// Load a captured variable from a closure environment.
pub fn load_capture(
    builder: &mut FunctionBuilder,
    env_ptr: Value,
    offset: u32,
    ty: Type,
) -> Value {
    builder
        .ins()
        .load(ty, MemFlags::new(), env_ptr, offset as i32)
}

/// Create a closure value on the stack: { func_ptr, env_ptr }.
///
/// Returns a pointer (i64) to the closure struct.
pub fn create_closure(
    builder: &mut FunctionBuilder,
    func_ptr: Value,
    env_ptr: Value,
) -> Value {
    let ss = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        CLOSURE_SIZE,
        0,
    ));
    let closure_ptr = builder.ins().stack_addr(types::I64, ss, 0);

    builder
        .ins()
        .store(MemFlags::new(), func_ptr, closure_ptr, CLOSURE_FUNC_OFFSET);
    builder
        .ins()
        .store(MemFlags::new(), env_ptr, closure_ptr, CLOSURE_ENV_OFFSET);

    closure_ptr
}

/// Extract the function pointer from a closure value.
pub fn load_func_ptr(builder: &mut FunctionBuilder, closure_ptr: Value) -> Value {
    builder
        .ins()
        .load(types::I64, MemFlags::new(), closure_ptr, CLOSURE_FUNC_OFFSET)
}

/// Extract the environment pointer from a closure value.
pub fn load_env_ptr(builder: &mut FunctionBuilder, closure_ptr: Value) -> Value {
    builder
        .ins()
        .load(types::I64, MemFlags::new(), closure_ptr, CLOSURE_ENV_OFFSET)
}

/// Call through a closure value.
///
/// Loads func_ptr and env_ptr from the closure, prepends env_ptr to the
/// argument list, and performs an indirect call.
pub fn call_closure(
    builder: &mut FunctionBuilder,
    closure_ptr: Value,
    mut sig: Signature,
    args: &[Value],
) -> cranelift_codegen::ir::Inst {
    let func_ptr = load_func_ptr(builder, closure_ptr);
    let env_ptr = load_env_ptr(builder, closure_ptr);

    // Prepend env_ptr as first parameter
    sig.params
        .insert(0, AbiParam::new(types::I64));
    let mut all_args = Vec::with_capacity(args.len() + 1);
    all_args.push(env_ptr);
    all_args.extend_from_slice(args);

    let sig_ref = builder.import_signature(sig);
    builder.ins().call_indirect(sig_ref, func_ptr, &all_args)
}

/// Perform a plain indirect call through a function pointer (no closure env).
pub fn call_indirect(
    builder: &mut FunctionBuilder,
    func_ptr: Value,
    sig: Signature,
    args: &[Value],
) -> cranelift_codegen::ir::Inst {
    let sig_ref = builder.import_signature(sig);
    builder.ins().call_indirect(sig_ref, func_ptr, args)
}
