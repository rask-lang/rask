// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MirType → Cranelift type translation.

use cranelift::prelude::*;
use rask_mir::MirType;
use crate::CodegenResult;

/// The MIR type of a checked type in an extern function signature.
///
/// Handles primitive types and raw pointers used in FFI declarations. A C
/// scalar name (`c_int`) and an alias of one have been resolved to the
/// primitive by the checker. Anything else crosses as a word.
pub fn extern_type_to_mir(ty: &rask_types::Type) -> MirType {
    use rask_types::Type;
    match ty {
        Type::RawPtr(_) => MirType::RawPtr,
        Type::Unit | Type::Never => MirType::Void,
        Type::I8 => MirType::I8,
        Type::I16 => MirType::I16,
        Type::I32 => MirType::I32,
        Type::I64 => MirType::I64,
        Type::U8 => MirType::U8,
        Type::U16 => MirType::U16,
        Type::U32 => MirType::U32,
        Type::U64 => MirType::U64,
        Type::F32 => MirType::F32,
        Type::F64 => MirType::F64,
        Type::Bool => MirType::Bool,
        _ => MirType::I64, // default to pointer-sized
    }
}

/// Translate MirType to Cranelift IR type.
///
/// Rule B3: Structs/enums become pointers in function signatures.
/// Inside function bodies, we allocate stack slots for aggregates.
pub fn mir_to_cranelift_type(ty: &MirType) -> CodegenResult<Type> {
    match ty {
        // Void locals are declared as i64 because Cranelift requires a concrete type.
        // Call sites skip storing to void-typed destinations (builder.rs), so these
        // variables are never written or read in practice. Function return signatures
        // already omit Void (module.rs).
        MirType::Void => Ok(types::I64),
        MirType::Bool => Ok(types::I8),
        MirType::I8 => Ok(types::I8),
        MirType::I16 => Ok(types::I16),
        MirType::I32 => Ok(types::I32),
        MirType::I64 => Ok(types::I64),
        // Cranelift has a real I128 — two machine words, with add/sub/mul
        // lowered on x64. Division and remainder have no lowering rule and go
        // through runtime helpers instead (#762).
        MirType::I128 => Ok(types::I128),
        MirType::U8 => Ok(types::I8),
        MirType::U16 => Ok(types::I16),
        MirType::U32 => Ok(types::I32),
        MirType::U64 => Ok(types::I64),
        MirType::U128 => Ok(types::I128),
        MirType::F32 => Ok(types::F32),
        MirType::F64 => Ok(types::F64),
        MirType::Char => Ok(types::I32), // Unicode scalar value
        MirType::Ptr => Ok(types::I64),  // Pointer
        MirType::RawPtr => Ok(types::I64), // Raw pointer (mem.unsafe)
        MirType::Container(_) => Ok(types::I64), // Container handle
        MirType::Heap(_) => Ok(types::I64),      // Block address
        MirType::Buffer => Ok(types::I64), // String data pointer
        MirType::Struct(_) => Ok(types::I64), // Pointer to struct
        MirType::Enum(_) => Ok(types::I64),   // Pointer to enum
        MirType::Array { .. } => Ok(types::I64), // Pointer to array
        MirType::FuncPtr(_) => Ok(types::I64), // Function pointer
        MirType::Link(_) => Ok(types::I64),    // Node address (mem.racks/RK2)
        MirType::Tuple(_) => Ok(types::I64),   // Pointer to tuple on stack
        MirType::Option(_) => Ok(types::I64),  // Pointer to tagged union
        MirType::Result { .. } => Ok(types::I64), // Pointer to tagged union
        MirType::Union(_) => Ok(types::I64),   // Pointer to union
        MirType::SimdVector { .. } => Ok(types::I64), // Pointer to SIMD buffer
        MirType::InterfaceObject { .. } => Ok(types::I64), // Pointer to fat pointer (data_ptr + vtable_ptr)
    }
}

