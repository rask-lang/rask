// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Memory layout computation - field offsets, sizes, alignments.

use rask_ast::decl::Decl;
use rask_types::Type;

/// Struct memory layout
#[derive(Debug, Clone)]
pub struct StructLayout {
    pub name: String,
    pub size: u32,
    pub align: u32,
    pub fields: Vec<FieldLayout>,
}

/// Field layout within struct
#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub name: String,
    pub ty: Type,
    pub offset: u32,
    pub size: u32,
    pub align: u32,
}

/// Enum memory layout
#[derive(Debug, Clone)]
pub struct EnumLayout {
    pub name: String,
    pub size: u32,
    pub align: u32,
    pub tag_ty: Type,
    pub tag_offset: u32,
    pub variants: Vec<VariantLayout>,
}

/// Variant layout within enum
#[derive(Debug, Clone)]
pub struct VariantLayout {
    pub name: String,
    pub tag: u64,
    pub payload_offset: u32,
    pub payload_size: u32,
}

/// Compute struct layout with field offsets
pub fn compute_struct_layout(struct_def: &Decl, type_args: &[Type]) -> StructLayout {
    todo!("Implement struct layout computation")
}

/// Compute enum layout with tag and variant payloads
pub fn compute_enum_layout(enum_def: &Decl, type_args: &[Type]) -> EnumLayout {
    todo!("Implement enum layout computation")
}
