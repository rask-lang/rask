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

/// Get size and alignment for a type (after monomorphization)
pub fn type_size_align(ty: &Type) -> (u32, u32) {
    match ty {
        Type::Unit => (0, 1),
        Type::Bool | Type::I8 | Type::U8 => (1, 1),
        Type::I16 | Type::U16 => (2, 2),
        Type::I32 | Type::U32 | Type::F32 => (4, 4),
        Type::I64 | Type::U64 | Type::F64 => (8, 8),
        Type::Char => (4, 4), // Unicode scalar value
        Type::String => (16, 8), // Fat pointer: ptr + len
        Type::Slice(_) => (16, 8), // Fat pointer: ptr + len
        Type::Option(inner) => {
            // TODO: Niche optimization for Handle/Reference
            let (size, align) = type_size_align(inner);
            // Naive layout: u8 tag + padding + payload
            let tag_size = 1u32;
            let payload_offset = align_up(tag_size, align);
            (payload_offset + size, align.max(1))
        }
        Type::Result { ok, err } => {
            // u8 tag + padding + max(ok_size, err_size)
            let (ok_size, ok_align) = type_size_align(ok);
            let (err_size, err_align) = type_size_align(err);
            let max_size = ok_size.max(err_size);
            let max_align = ok_align.max(err_align);
            let tag_size = 1u32;
            let payload_offset = align_up(tag_size, max_align);
            (payload_offset + max_size, max_align.max(1))
        }
        Type::Tuple(types) => {
            // Layout like a struct with anonymous fields
            let mut offset = 0u32;
            let mut max_align = 1u32;
            for ty in types {
                let (size, align) = type_size_align(ty);
                max_align = max_align.max(align);
                offset = align_up(offset, align);
                offset += size;
            }
            let total_size = align_up(offset, max_align);
            (total_size, max_align)
        }
        Type::Array { elem, len } => {
            let (elem_size, elem_align) = type_size_align(elem);
            (elem_size * (*len as u32), elem_align)
        }
        Type::Fn { .. } => (8, 8), // Function pointer
        Type::Named(_) | Type::Generic { .. } => {
            // Must be resolved through type table
            // For now, assume pointer-sized (will be fixed during layout phase)
            (8, 8)
        }
        Type::Var(_) | Type::UnresolvedNamed(_) | Type::UnresolvedGeneric { .. } => {
            panic!("Unresolved type in layout computation: {:?}", ty)
        }
        Type::Union(_) => {
            // Union of error types - same as largest error
            // For now, assume pointer-sized
            (16, 8)
        }
        Type::Never => (0, 1),
        Type::Error => panic!("Error type in layout computation"),
        _ => (8, 8), // Default for unknown types
    }
}

/// Align a value up to the given alignment
fn align_up(val: u32, align: u32) -> u32 {
    (val + align - 1) & !(align - 1)
}

/// Compute struct layout with field offsets (spec rules S1-S4)
pub fn compute_struct_layout(struct_def: &Decl, type_args: &[Type]) -> StructLayout {
    use rask_ast::decl::DeclKind;

    let struct_decl = match &struct_def.kind {
        DeclKind::Struct(s) => s,
        _ => panic!("Expected struct declaration"),
    };

    // TODO: Type substitution for generic parameters
    // For now, work with concrete types
    let _ = type_args; // Will use for substitution

    let mut field_layouts = Vec::new();
    let mut offset = 0u32;
    let mut max_align = 1u32;

    // S1-S2: Process fields in source order, no reordering
    for field in &struct_decl.fields {
        // TODO: Parse type string to Type and substitute generics
        // For now, use placeholder
        let field_ty = Type::I32; // Placeholder

        let (field_size, field_align) = type_size_align(&field_ty);
        max_align = max_align.max(field_align);

        // S3: Align offset for this field
        offset = align_up(offset, field_align);

        field_layouts.push(FieldLayout {
            name: field.name.clone(),
            ty: field_ty,
            offset,
            size: field_size,
            align: field_align,
        });

        offset += field_size;
    }

    // S4: Total size with tail padding to struct alignment
    let total_size = align_up(offset, max_align);

    StructLayout {
        name: struct_decl.name.clone(),
        size: total_size,
        align: max_align,
        fields: field_layouts,
    }
}

/// Compute enum layout with tag and variant payloads (spec rules E1-E6)
pub fn compute_enum_layout(enum_def: &Decl, type_args: &[Type]) -> EnumLayout {
    use rask_ast::decl::DeclKind;

    let enum_decl = match &enum_def.kind {
        DeclKind::Enum(e) => e,
        _ => panic!("Expected enum declaration"),
    };

    let _ = type_args; // TODO: Use for type substitution

    let variant_count = enum_decl.variants.len();

    // E2: Determine discriminant type
    let tag_ty = if variant_count <= 256 {
        Type::U8
    } else {
        Type::U16
    };
    let (tag_size, tag_align) = type_size_align(&tag_ty);

    // Compute size and alignment of each variant payload
    let mut max_payload_size = 0u32;
    let mut max_payload_align = 1u32;
    let mut variant_layouts = Vec::new();

    for (tag, variant) in enum_decl.variants.iter().enumerate() {
        // Compute payload size for this variant
        let mut payload_size = 0u32;
        let mut payload_align = 1u32;

        if !variant.fields.is_empty() {
            // Variant has fields - compute struct-like layout
            let mut field_offset = 0u32;
            for field in &variant.fields {
                // TODO: Parse and substitute types
                let field_ty = Type::I32; // Placeholder
                let (size, align) = type_size_align(&field_ty);

                payload_align = payload_align.max(align);
                field_offset = align_up(field_offset, align);
                field_offset += size;
            }
            payload_size = align_up(field_offset, payload_align);
        }

        max_payload_size = max_payload_size.max(payload_size);
        max_payload_align = max_payload_align.max(payload_align);

        variant_layouts.push(VariantLayout {
            name: variant.name.clone(),
            tag: tag as u64,
            payload_offset: 0, // Will be computed from tag
            payload_size,
        });
    }

    // E5: Enum alignment = max(tag_align, max_payload_align)
    let enum_align = tag_align.max(max_payload_align);

    // E6: Padding after tag to align payload
    let payload_offset = align_up(tag_size, max_payload_align);

    // Update variant payload offsets
    for variant in &mut variant_layouts {
        variant.payload_offset = payload_offset;
    }

    // E4: Total size = tag + padding + max(all variant payloads)
    let total_size = align_up(payload_offset + max_payload_size, enum_align);

    EnumLayout {
        name: enum_decl.name.clone(),
        size: total_size,
        align: enum_align,
        tag_ty,
        tag_offset: 0, // E1: Tag is first
        variants: variant_layouts,
    }
}
