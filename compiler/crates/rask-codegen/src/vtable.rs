// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! VTable layout and generation for trait objects.
//!
//! Layout: [size:i64, align:i64, drop_fn:i64, method_0:i64, method_1:i64, ...]
//! Offsets: 0, 8, 16, 24, 32, ...

/// Byte offset of the size field in a vtable.
pub const VTABLE_SIZE_OFFSET: u32 = 0;
/// Byte offset of the alignment field.
pub const VTABLE_ALIGN_OFFSET: u32 = 8;
/// Byte offset of the drop function pointer (null if trivial drop).
pub const VTABLE_DROP_OFFSET: u32 = 16;
/// Byte offset where method pointers begin.
pub const VTABLE_METHODS_START: u32 = 24;

/// Metadata for a single vtable: one (concrete type, trait) pair.
#[derive(Debug, Clone)]
pub struct VTableInfo {
    /// Data section name: ".vtable.Button__Widget"
    pub data_name: String,
    /// Concrete type name: "Button"
    pub concrete_type: String,
    /// Trait name: "Widget"
    pub trait_name: String,
    /// sizeof(concrete_type) in bytes
    pub concrete_size: u32,
    /// alignof(concrete_type) in bytes
    pub concrete_align: u32,
    /// Compatible methods in vtable order (trait declaration order, minus incompatible)
    pub methods: Vec<VTableMethod>,
}

/// A single method entry in a vtable.
#[derive(Debug, Clone)]
pub struct VTableMethod {
    /// Method name: "draw"
    pub name: String,
    /// Monomorphized function name: "Button_draw"
    pub func_name: String,
    /// Byte offset in the vtable: 24, 32, ...
    pub vtable_offset: u32,
}

impl VTableInfo {
    /// Total size of the vtable in bytes.
    pub fn byte_size(&self) -> u32 {
        VTABLE_METHODS_START + (self.methods.len() as u32) * 8
    }
}

/// Build the vtable data section name from concrete type and trait name.
pub fn vtable_data_name(concrete_type: &str, trait_name: &str) -> String {
    format!(".vtable.{}__{}", concrete_type, trait_name)
}

/// Compute vtable offset for a method by its index (0-based among compatible methods).
pub fn method_offset(index: usize) -> u32 {
    VTABLE_METHODS_START + (index as u32) * 8
}
