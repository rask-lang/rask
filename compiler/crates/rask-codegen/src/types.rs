// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! MirType → Cranelift type translation.

use cranelift::prelude::*;
use rask_mir::MirType;
use crate::CodegenResult;

/// Translate MirType to Cranelift IR type.
///
/// Rule B3: Structs/enums become pointers in function signatures.
/// Inside function bodies, we allocate stack slots for aggregates.
pub fn mir_to_cranelift_type(ty: &MirType) -> CodegenResult<Type> {
    match ty {
        MirType::Void => Ok(types::I64), // Void is 0-sized, use i64 as placeholder
        MirType::Bool => Ok(types::I8),
        MirType::I8 => Ok(types::I8),
        MirType::I16 => Ok(types::I16),
        MirType::I32 => Ok(types::I32),
        MirType::I64 => Ok(types::I64),
        MirType::U8 => Ok(types::I8),
        MirType::U16 => Ok(types::I16),
        MirType::U32 => Ok(types::I32),
        MirType::U64 => Ok(types::I64),
        MirType::F32 => Ok(types::F32),
        MirType::F64 => Ok(types::F64),
        MirType::Char => Ok(types::I32), // Unicode scalar value
        MirType::Ptr => Ok(types::I64),  // Pointer
        MirType::String => Ok(types::I64), // String data pointer
        MirType::Struct(_) => Ok(types::I64), // Pointer to struct
        MirType::Enum(_) => Ok(types::I64),   // Pointer to enum
        MirType::Array { .. } => Ok(types::I64), // Pointer to array
        MirType::FuncPtr(_) => Ok(types::I64), // Function pointer
    }
}

/// Get the size in bytes of a MirType.
/// Used for stack slot allocation.
pub fn mir_type_size(ty: &MirType) -> u32 {
    match ty {
        MirType::Void => 0,
        MirType::Bool | MirType::I8 | MirType::U8 => 1,
        MirType::I16 | MirType::U16 => 2,
        MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
        MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr | MirType::FuncPtr(_) => 8,
        MirType::String => 16, // ptr + len
        MirType::Struct(_) => 8, // Pointer for now (should use layout)
        MirType::Enum(_) => 8,   // Pointer for now
        MirType::Array { elem, len } => mir_type_size(elem) * len,
    }
}

/// Get the alignment of a MirType.
pub fn mir_type_alignment(ty: &MirType) -> u32 {
    match ty {
        MirType::Void => 1,
        MirType::Bool | MirType::I8 | MirType::U8 => 1,
        MirType::I16 | MirType::U16 => 2,
        MirType::I32 | MirType::U32 | MirType::F32 | MirType::Char => 4,
        MirType::I64 | MirType::U64 | MirType::F64 | MirType::Ptr | MirType::String | MirType::FuncPtr(_) => 8,
        MirType::Struct(_) => 8, // Should use layout
        MirType::Enum(_) => 8,
        MirType::Array { elem, .. } => mir_type_alignment(elem),
    }
}
