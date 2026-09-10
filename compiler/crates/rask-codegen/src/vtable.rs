// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! VTable contents for trait objects: what goes in one, and what it's called.
//!
//! The offsets live in `rask_mir::vtable_layout`, because lowering picks a
//! `TraitCall`'s offset and this crate writes the pointers it will read.

pub use rask_mir::vtable_layout::{
    method_offset, VTABLE_ALIGN_OFFSET, VTABLE_METHODS_START, VTABLE_OWNED_RELEASE_OFFSET,
    VTABLE_SIZE_OFFSET,
};

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
    /// What the concrete value owns, for a box that owns the value — a moved-in
    /// one. Empty leaves `owned_release` null. A box built for a call borrows
    /// instead, and `TraitDrop` never reads this slot.
    pub owned: Vec<crate::drop_fields::DropField>,
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
