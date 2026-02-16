// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Closure environment support — heap allocation, capture storage, and indirect calls.
//!
//! A closure is a heap-allocated block:
//!   [0..8]  function pointer (address of the closure's compiled code)
//!   [8..]   captured variables at known offsets
//!
//! The closure value passed around is a single i64 pointer to this block.
//! When calling through a closure, (closure_ptr + 8) is passed as the
//! environment pointer — the implicit first argument to the function.
//!
//! Heap allocation lets closures escape their creating scope: they can be
//! returned from functions, stored in structs, or sent to spawn().

use cranelift::prelude::*;
use cranelift_codegen::ir::{FuncRef, InstBuilder, MemFlags};
use cranelift_frontend::FunctionBuilder;
use rask_mir::LocalId;
use std::collections::HashMap;

use crate::{CodegenError, CodegenResult};

/// Byte offset of func_ptr within the closure heap block.
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
/// Calls `rask_alloc(8 + env_size)` to get a heap block, stores the function
/// pointer at offset 0 and each captured variable at offset 8+. Returns a
/// pointer to the block.
///
/// The heap allocation means the closure survives beyond its creating scope —
/// it can be returned, stored, or sent to spawn().
pub fn allocate_closure(
    builder: &mut FunctionBuilder,
    func_ptr: Value,
    layout: &ClosureEnvLayout,
    var_map: &HashMap<LocalId, Variable>,
    alloc_func: FuncRef,
) -> CodegenResult<Value> {
    let total_size = 8 + layout.size as i64; // func_ptr header + captures

    // Call rask_alloc(total_size)
    let size_val = builder.ins().iconst(types::I64, total_size);
    let call_inst = builder.ins().call(alloc_func, &[size_val]);
    let closure_ptr = builder.inst_results(call_inst)[0];

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
    // env_ptr is right after func_ptr in the heap block
    let env_ptr = builder.ins().iadd_imm(closure_ptr, CLOSURE_ENV_OFFSET);

    // Prepend env_ptr as first parameter
    sig.params
        .insert(0, AbiParam::new(types::I64));
    let mut all_args = Vec::with_capacity(args.len() + 1);
    all_args.push(env_ptr);
    all_args.extend_from_slice(args);

    let sig_ref = builder.import_signature(sig);
    builder.ins().call_indirect(sig_ref, func_ptr, &all_args)
}

/// Free a closure's heap allocation.
///
/// Calls `rask_free(closure_ptr)`. Use when a closure goes out of scope.
/// Currently not wired into automatic drop — callers must emit this explicitly.
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
