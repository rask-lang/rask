// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Closure environment support — allocation, capture storage, and indirect calls.
//!
//! Closure layout (same for heap and stack):
//!   [0..8]  function pointer (address of the closure's compiled code)
//!   [8..]   captured variables at known offsets
//!
//! The closure value is a single i64 pointer to this block. When calling
//! through a closure, (closure_ptr + 8) is passed as the environment
//! pointer — the implicit first argument to the closure function.
//!
//! Allocation strategy is chosen per-closure by the MIR escape analysis pass:
//! - `heap: true`  → rask_alloc (escaping closures: returned, stored, spawned)
//! - `heap: false` → stack slot (non-escaping: used locally via ClosureCall)

use cranelift::prelude::*;
use cranelift_codegen::ir::{FuncRef, InstBuilder, MemFlags, StackSlotData, StackSlotKind};
use cranelift_frontend::FunctionBuilder;
use rask_mir::LocalId;
use std::collections::HashMap;

use crate::{CodegenError, CodegenResult};

/// Byte offset of func_ptr within the closure block.
pub const CLOSURE_FUNC_OFFSET: i32 = 0;

/// Byte offset where captured variables begin (right after func_ptr).
pub const CLOSURE_ENV_OFFSET: i64 = 8;

/// Tracks captured variables and their layout in a closure environment.
pub struct ClosureEnvLayout {
    /// Total environment size in bytes (captures only, excludes func_ptr header)
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
    /// Returns the offset where the variable will be stored (relative to env start).
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

/// Heap-allocate a closure: `[func_ptr | captures...]`.
///
/// Calls `rask_alloc(8 + env_size)` and stores func_ptr + captures.
/// Used for escaping closures (returned, stored, sent to spawn).
pub fn allocate_closure_heap(
    builder: &mut FunctionBuilder,
    func_ptr: Value,
    layout: &ClosureEnvLayout,
    var_map: &HashMap<LocalId, Variable>,
    alloc_func: FuncRef,
) -> CodegenResult<Value> {
    let total_size = 8 + layout.size as i64;

    let size_val = builder.ins().iconst(types::I64, total_size);
    let call_inst = builder.ins().call(alloc_func, &[size_val]);
    let closure_ptr = builder.inst_results(call_inst)[0];

    store_closure_data(builder, closure_ptr, func_ptr, layout, var_map)
}

/// Stack-allocate a closure: `[func_ptr | captures...]`.
///
/// Creates a stack slot and stores func_ptr + captures.
/// Used for non-escaping closures (only used locally via ClosureCall).
pub fn allocate_closure_stack(
    builder: &mut FunctionBuilder,
    func_ptr: Value,
    layout: &ClosureEnvLayout,
    var_map: &HashMap<LocalId, Variable>,
) -> CodegenResult<Value> {
    let total_size = 8 + layout.size;

    let ss = builder.create_sized_stack_slot(StackSlotData::new(
        StackSlotKind::ExplicitSlot,
        total_size,
        0,
    ));
    let closure_ptr = builder.ins().stack_addr(types::I64, ss, 0);

    store_closure_data(builder, closure_ptr, func_ptr, layout, var_map)
}

/// Store func_ptr and captures into an already-allocated closure block.
fn store_closure_data(
    builder: &mut FunctionBuilder,
    closure_ptr: Value,
    func_ptr: Value,
    layout: &ClosureEnvLayout,
    var_map: &HashMap<LocalId, Variable>,
) -> CodegenResult<Value> {
    // Store func_ptr at offset 0
    builder
        .ins()
        .store(MemFlags::new(), func_ptr, closure_ptr, CLOSURE_FUNC_OFFSET);

    // Store captured variables at offset 8+
    for capture in &layout.captures {
        let var = var_map.get(&capture.local_id)
            .ok_or_else(|| CodegenError::UnsupportedFeature(
                format!("Closure capture variable {:?} not found", capture.local_id)
            ))?;
        let val = builder.use_var(*var);
        let store_offset = CLOSURE_ENV_OFFSET as i32 + capture.offset as i32;
        builder
            .ins()
            .store(MemFlags::new(), val, closure_ptr, store_offset);
    }

    Ok(closure_ptr)
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

/// Extract the function pointer from a closure value.
pub fn load_func_ptr(builder: &mut FunctionBuilder, closure_ptr: Value) -> Value {
    builder
        .ins()
        .load(types::I64, MemFlags::new(), closure_ptr, CLOSURE_FUNC_OFFSET)
}

/// Call through a closure value.
///
/// Loads func_ptr from offset 0, computes env_ptr = closure_ptr + 8,
/// prepends env_ptr to the argument list, and performs an indirect call.
pub fn call_closure(
    builder: &mut FunctionBuilder,
    closure_ptr: Value,
    mut sig: Signature,
    args: &[Value],
) -> cranelift_codegen::ir::Inst {
    let func_ptr = load_func_ptr(builder, closure_ptr);
    let env_ptr = builder.ins().iadd_imm(closure_ptr, CLOSURE_ENV_OFFSET);

    sig.params
        .insert(0, AbiParam::new(types::I64));
    let mut all_args = Vec::with_capacity(args.len() + 1);
    all_args.push(env_ptr);
    all_args.extend_from_slice(args);

    let sig_ref = builder.import_signature(sig);
    builder.ins().call_indirect(sig_ref, func_ptr, &all_args)
}

/// Free a heap-allocated closure.
pub fn free_closure(
    builder: &mut FunctionBuilder,
    closure_ptr: Value,
    free_func: FuncRef,
) {
    builder.ins().call(free_func, &[closure_ptr]);
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
