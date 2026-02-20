// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Memory layout computation - field offsets, sizes, alignments.

use rask_ast::decl::Decl;
use rask_types::Type;
use std::collections::HashMap;

/// Cache of already-computed type layouts, keyed by type name.
/// Used so struct fields referencing other user-defined types get correct sizes.
pub type LayoutCache = HashMap<String, (u32, u32)>;

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
    pub fields: Vec<FieldLayout>,
}

/// Get size and alignment for a type (after monomorphization).
/// `cache` maps type names to already-computed (size, align) for user-defined types.
pub fn type_size_align(ty: &Type, cache: &LayoutCache) -> (u32, u32) {
    match ty {
        // All scalar types use 8-byte size/align because the codegen stores
        // every value as i64. Using true type sizes (bool=1, i32=4) causes
        // overlapping stores in struct fields.
        Type::Unit => (0, 1),
        Type::Bool | Type::I8 | Type::U8 => (8, 8),
        Type::I16 | Type::U16 => (8, 8),
        Type::I32 | Type::U32 | Type::F32 => (8, 8),
        Type::I64 | Type::U64 | Type::F64 => (8, 8),
        Type::I128 | Type::U128 => (16, 16),
        Type::Char => (8, 8),
        Type::String => (8, 8), // Opaque pointer (runtime uses RaskString*)
        Type::Slice(_) => (16, 8), // Fat pointer: ptr + len
        Type::Option(inner) => {
            // Niche optimization: Option<Handle<T>> uses sentinel value instead of tag.
            if matches!(inner.as_ref(), Type::UnresolvedGeneric { name, .. } if name == "Handle") {
                return (8, 8);
            }
            let (size, align) = type_size_align(inner, cache);
            let tag_size = 1u32;
            let payload_offset = align_up(tag_size, align);
            (payload_offset + size, align.max(1))
        }
        Type::Result { ok, err } => {
            let (ok_size, ok_align) = type_size_align(ok, cache);
            let (err_size, err_align) = type_size_align(err, cache);
            let max_size = ok_size.max(err_size);
            let max_align = ok_align.max(err_align);
            let tag_size = 1u32;
            let payload_offset = align_up(tag_size, max_align);
            (payload_offset + max_size, max_align.max(1))
        }
        Type::Tuple(types) => {
            let mut offset = 0u32;
            let mut max_align = 1u32;
            for ty in types {
                let (size, align) = type_size_align(ty, cache);
                max_align = max_align.max(align);
                offset = align_up(offset, align);
                offset += size;
            }
            let total_size = align_up(offset, max_align);
            (total_size, max_align)
        }
        Type::Array { elem, len } => {
            let (elem_size, elem_align) = type_size_align(elem, cache);
            (elem_size * (*len as u32), elem_align)
        }
        Type::Fn { .. } => (8, 8), // Function pointer
        Type::Named(_) | Type::Generic { .. } => {
            // Named types carry a TypeId — can't resolve by name here.
            // Assume pointer-sized; struct/enum layouts are computed separately.
            (8, 8)
        }
        // Generic builtins with known sizes
        Type::UnresolvedGeneric { name, .. } if name == "Handle" => (8, 8),
        Type::UnresolvedGeneric { name, .. } if name == "Pool" => (8, 8),
        Type::UnresolvedGeneric { name, .. } if name == "Vec" => (8, 8), // Opaque pointer (runtime uses RaskVec*)
        Type::UnresolvedGeneric { name, .. } if name == "Map" => (8, 8),  // Pointer to map
        Type::UnresolvedGeneric { name, .. } if name == "Rng" => (8, 8),  // Pointer to rng state
        Type::UnresolvedGeneric { name, .. } if name == "Channel" => (8, 8),
        Type::UnresolvedGeneric { name, args } => {
            eprintln!(
                "warning: unresolved generic type in layout: {}<{} arg(s)>, defaulting to (8, 8)",
                name,
                args.len()
            );
            (8, 8)
        }
        Type::Var(id) => {
            eprintln!(
                "warning: type variable ?{} in layout computation, defaulting to (8, 8)",
                id.0
            );
            (8, 8)
        }
        Type::UnresolvedNamed(name) => {
            match name.as_str() {
                "string" => (16, 8),
                "bool" => (1, 1),
                "i8" | "u8" => (1, 1),
                "i16" | "u16" => (2, 2),
                "i32" | "u32" | "f32" => (4, 4),
                "i64" | "u64" | "f64" => (8, 8),
                "char" => (4, 4),
                _ => {
                    // Look up user-defined types from the layout cache
                    if let Some(&cached) = cache.get(name.as_str()) {
                        cached
                    } else {
                        eprintln!(
                            "warning: unknown type '{}' in layout, defaulting to (8, 8)",
                            name
                        );
                        (8, 8)
                    }
                }
            }
        }
        Type::Union(variants) => {
            let mut max_size = 0u32;
            let mut max_align = 1u32;
            for v in variants {
                let (s, a) = type_size_align(v, cache);
                max_size = max_size.max(s);
                max_align = max_align.max(a);
            }
            if max_size == 0 {
                (8, 8)
            } else {
                (max_size, max_align)
            }
        }
        Type::SimdVector { elem, lanes } => {
            let (elem_size, _) = type_size_align(elem, cache);
            let total = elem_size * *lanes as u32;
            (total, total.min(32)) // natural SIMD alignment, cap at 32
        }
        Type::Never => (0, 1),
        Type::TraitObject { .. } => (16, 8), // Fat pointer: data_ptr + vtable_ptr
        Type::RawPtr(_) => (8, 8), // Pointer-sized
        Type::Error => {
            eprintln!("warning: Error type in layout computation, defaulting to (8, 8)");
            (8, 8)
        }
    }
}

/// Align a value up to the given alignment
fn align_up(val: u32, align: u32) -> u32 {
    (val + align - 1) & !(align - 1)
}

/// Parse a field type string (from AST) to a Type for layout computation.
fn parse_field_type(s: &str) -> Type {
    let s = s.trim();

    // Result type: "T or E"
    if let Some(idx) = s.find(" or ") {
        let ok = parse_field_type(&s[..idx]);
        let err = parse_field_type(&s[idx + 4..]);
        return Type::Result {
            ok: Box::new(ok),
            err: Box::new(err),
        };
    }

    // Generic types: Name<Args>
    if let Some(angle) = s.find('<') {
        if s.ends_with('>') {
            let name = &s[..angle];
            let inner = &s[angle + 1..s.len() - 1];

            // Option<T> → Type::Option
            if name == "Option" {
                return Type::Option(Box::new(parse_field_type(inner)));
            }

            // Split comma-separated type args (respecting nested angle brackets)
            let args: Vec<rask_types::GenericArg> = split_type_args(inner)
                .into_iter()
                .map(|a| rask_types::GenericArg::Type(Box::new(parse_field_type(a))))
                .collect();

            return Type::UnresolvedGeneric {
                name: name.to_string(),
                args,
            };
        }
    }

    match s {
        "()" => Type::Unit,
        "bool" => Type::Bool,
        "i8" => Type::I8,
        "i16" => Type::I16,
        "i32" => Type::I32,
        "i64" | "isize" => Type::I64,
        "i128" => Type::I128,
        "u8" => Type::U8,
        "u16" => Type::U16,
        "u32" => Type::U32,
        "u64" | "usize" => Type::U64,
        "u128" => Type::U128,
        "f32" => Type::F32,
        "f64" => Type::F64,
        "char" => Type::Char,
        "string" => Type::String,
        name => Type::UnresolvedNamed(name.to_string()),
    }
}

/// Split comma-separated type arguments, respecting nested angle brackets.
fn split_type_args(s: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                result.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = s[start..].trim();
    if !last.is_empty() {
        result.push(last);
    }
    result
}

/// Build a substitution map from type param names to concrete types.
fn build_subst<'a>(
    type_params: &'a [rask_ast::decl::TypeParam],
    type_args: &'a [Type],
) -> std::collections::HashMap<&'a str, &'a Type> {
    let mut subst = std::collections::HashMap::new();
    for (param, arg) in type_params.iter().zip(type_args.iter()) {
        subst.insert(param.name.as_str(), arg);
    }
    subst
}

/// Parse a field type string and apply generic substitution.
/// If the parsed type is an unresolved name that matches a type parameter,
/// replace it with the concrete type from type_args.
fn resolve_field_type(
    field_ty_str: &str,
    subst: &std::collections::HashMap<&str, &Type>,
) -> Type {
    let parsed = parse_field_type(field_ty_str);
    match &parsed {
        Type::UnresolvedNamed(name) => {
            if let Some(concrete) = subst.get(name.as_str()) {
                (*concrete).clone()
            } else {
                parsed
            }
        }
        _ => parsed,
    }
}

/// Compute struct layout with field offsets (spec rules S1-S4)
pub fn compute_struct_layout(struct_def: &Decl, type_args: &[Type], cache: &LayoutCache) -> StructLayout {
    use rask_ast::decl::DeclKind;

    let struct_decl = match &struct_def.kind {
        DeclKind::Struct(s) => s,
        _ => panic!("Expected struct declaration"),
    };

    let subst = build_subst(&struct_decl.type_params, type_args);

    let mut field_layouts = Vec::new();
    let mut offset = 0u32;
    let mut max_align = 1u32;

    // S1-S2: Process fields in source order, no reordering
    for field in &struct_decl.fields {
        let field_ty = resolve_field_type(&field.ty, &subst);

        let (field_size, field_align) = type_size_align(&field_ty, cache);
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

/// Compute union layout — all fields at offset 0, size = max field size (spec rules UN1-UN3).
/// Returns a StructLayout since unions reuse the same representation.
pub fn compute_union_layout(union_def: &Decl, cache: &LayoutCache) -> StructLayout {
    use rask_ast::decl::DeclKind;

    let union_decl = match &union_def.kind {
        DeclKind::Union(u) => u,
        _ => panic!("Expected union declaration"),
    };

    let mut field_layouts = Vec::new();
    let mut max_size = 0u32;
    let mut max_align = 1u32;

    for field in &union_decl.fields {
        let field_ty = parse_field_type(&field.ty);
        let (field_size, field_align) = type_size_align(&field_ty, cache);
        max_size = max_size.max(field_size);
        max_align = max_align.max(field_align);

        // All union fields at offset 0
        field_layouts.push(FieldLayout {
            name: field.name.clone(),
            ty: field_ty,
            offset: 0,
            size: field_size,
            align: field_align,
        });
    }

    let total_size = align_up(max_size, max_align);

    StructLayout {
        name: union_decl.name.clone(),
        size: total_size,
        align: max_align,
        fields: field_layouts,
    }
}

/// Compute enum layout with tag and variant payloads (spec rules E1-E6)
pub fn compute_enum_layout(enum_def: &Decl, type_args: &[Type], cache: &LayoutCache) -> EnumLayout {
    use rask_ast::decl::DeclKind;

    let enum_decl = match &enum_def.kind {
        DeclKind::Enum(e) => e,
        _ => panic!("Expected enum declaration"),
    };

    let subst = build_subst(&enum_decl.type_params, type_args);

    let variant_count = enum_decl.variants.len();

    // E2: Determine discriminant type
    let tag_ty = if variant_count <= 256 {
        Type::U8
    } else {
        Type::U16
    };
    let (tag_size, tag_align) = type_size_align(&tag_ty, cache);

    // Compute size and alignment of each variant payload
    let mut max_payload_size = 0u32;
    let mut max_payload_align = 1u32;
    let mut variant_layouts = Vec::new();

    for (tag, variant) in enum_decl.variants.iter().enumerate() {
        // Compute payload size for this variant
        let mut payload_size = 0u32;
        let mut payload_align = 1u32;

        let mut variant_fields = Vec::new();

        if !variant.fields.is_empty() {
            let mut field_offset = 0u32;
            for field in &variant.fields {
                let field_ty = resolve_field_type(&field.ty, &subst);
                let (size, align) = type_size_align(&field_ty, cache);

                payload_align = payload_align.max(align);
                field_offset = align_up(field_offset, align);

                variant_fields.push(FieldLayout {
                    name: field.name.clone(),
                    ty: field_ty,
                    offset: field_offset,
                    size,
                    align,
                });

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
            fields: variant_fields,
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

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{Decl, DeclKind, EnumDecl, Field, StructDecl, Variant};
    use rask_ast::{NodeId, Span};

    fn dummy_span() -> Span {
        Span::new(0, 0)
    }

    fn make_struct(name: &str, fields: Vec<(&str, &str)>) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Struct(StructDecl {
                name: name.to_string(),
                type_params: vec![],
                fields: fields
                    .into_iter()
                    .map(|(n, ty)| Field {
                        name: n.to_string(),
                        name_span: dummy_span(),
                        ty: ty.to_string(),
                        is_pub: false,
                    })
                    .collect(),
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
            }),
            span: dummy_span(),
        }
    }

    fn make_enum(name: &str, variants: Vec<(&str, Vec<&str>)>) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Enum(EnumDecl {
                name: name.to_string(),
                type_params: vec![],
                variants: variants
                    .into_iter()
                    .map(|(vname, field_tys)| Variant {
                        name: vname.to_string(),
                        fields: field_tys
                            .into_iter()
                            .enumerate()
                            .map(|(i, ty)| Field {
                                name: format!("f{}", i),
                                name_span: dummy_span(),
                                ty: ty.to_string(),
                                is_pub: false,
                            })
                            .collect(),
                    })
                    .collect(),
                methods: vec![],
                is_pub: false,
                doc: None,
            }),
            span: dummy_span(),
        }
    }

    // ── align_up ────────────────────────────────────────────────

    #[test]
    fn align_up_works() {
        // Already aligned
        assert_eq!(align_up(8, 4), 8);
        assert_eq!(align_up(0, 4), 0);
        // Needs padding
        assert_eq!(align_up(1, 4), 4);
        assert_eq!(align_up(5, 4), 8);
        assert_eq!(align_up(6, 8), 8);
        assert_eq!(align_up(9, 8), 16);
    }

    // ── type_size_align ─────────────────────────────────────────

    fn empty_cache() -> LayoutCache {
        LayoutCache::new()
    }

    /// Shorthand: type_size_align with empty cache (for primitive tests)
    fn tsa(ty: &Type) -> (u32, u32) {
        type_size_align(ty, &empty_cache())
    }

    #[test]
    fn primitive_sizes() {
        // All scalars are 8 bytes — codegen stores everything as i64
        assert_eq!(tsa(&Type::Bool), (8, 8));
        assert_eq!(tsa(&Type::I8), (8, 8));
        assert_eq!(tsa(&Type::U8), (8, 8));
        assert_eq!(tsa(&Type::I16), (8, 8));
        assert_eq!(tsa(&Type::U16), (8, 8));
        assert_eq!(tsa(&Type::I32), (8, 8));
        assert_eq!(tsa(&Type::U32), (8, 8));
        assert_eq!(tsa(&Type::F32), (8, 8));
        assert_eq!(tsa(&Type::I64), (8, 8));
        assert_eq!(tsa(&Type::U64), (8, 8));
        assert_eq!(tsa(&Type::F64), (8, 8));
        assert_eq!(tsa(&Type::Char), (8, 8));
    }

    #[test]
    fn string_is_opaque_pointer() {
        let (size, align) = tsa(&Type::String);
        assert_eq!(size, 8);
        assert_eq!(align, 8);
    }

    #[test]
    fn unit_is_zero_size() {
        assert_eq!(tsa(&Type::Unit), (0, 1));
    }

    #[test]
    fn never_is_zero_size() {
        assert_eq!(tsa(&Type::Never), (0, 1));
    }

    #[test]
    fn option_i32_layout() {
        // tag (8 bytes) + i32 payload (8 bytes, codegen uses i64) = 16
        let (size, align) = tsa(&Type::Option(Box::new(Type::I32)));
        assert_eq!(align, 8);
        assert_eq!(size, 16);
    }

    #[test]
    fn option_i8_layout() {
        // tag (8) + i8 payload (8, codegen uses i64) = 16
        let (size, align) = tsa(&Type::Option(Box::new(Type::I8)));
        assert_eq!(align, 8);
        assert_eq!(size, 16);
    }

    #[test]
    fn option_handle_niche_optimized() {
        // Option<Handle<T>> uses niche sentinel — same size as Handle (8 bytes, no tag)
        let handle_ty = Type::UnresolvedGeneric {
            name: "Handle".to_string(),
            args: vec![rask_types::GenericArg::Type(Box::new(Type::I32))],
        };
        let (size, align) = tsa(&Type::Option(Box::new(handle_ty)));
        assert_eq!(size, 8);
        assert_eq!(align, 8);
    }

    #[test]
    fn handle_size() {
        // Handle<T> is 8 bytes (packed i64: index:32 | gen:32)
        let handle_ty = Type::UnresolvedGeneric {
            name: "Handle".to_string(),
            args: vec![rask_types::GenericArg::Type(Box::new(Type::I32))],
        };
        let (size, align) = tsa(&handle_ty);
        assert_eq!(size, 8);
        assert_eq!(align, 8);
    }

    #[test]
    fn result_i32_i64_layout() {
        // u8 tag (1) + padding to 8 + max(4, 8) = 16
        let (size, align) = tsa(&Type::Result {
            ok: Box::new(Type::I32),
            err: Box::new(Type::I64),
        });
        assert_eq!(align, 8);
        assert_eq!(size, 16); // 1 tag + 7 padding + 8 payload
    }

    #[test]
    fn result_same_types() {
        let (size, align) = tsa(&Type::Result {
            ok: Box::new(Type::I32),
            err: Box::new(Type::I32),
        });
        assert_eq!(align, 8);
        assert_eq!(size, 16); // 8 tag + 8 payload (all scalars stored as i64)
    }

    #[test]
    fn tuple_layout() {
        // (i32, i64) → offset 0: i32(4), pad to 8, offset 8: i64(8) → total 16
        let (size, align) = tsa(&Type::Tuple(vec![Type::I32, Type::I64]));
        assert_eq!(align, 8);
        assert_eq!(size, 16);
    }

    #[test]
    fn tuple_i8_i8() {
        // All scalars stored as i64: two 8-byte fields
        let (size, align) = tsa(&Type::Tuple(vec![Type::I8, Type::I8]));
        assert_eq!(align, 8);
        assert_eq!(size, 16);
    }

    #[test]
    fn array_layout() {
        // [i32; 5] → 8 * 5 = 40, align 8 (all scalars stored as i64)
        let (size, align) = tsa(&Type::Array {
            elem: Box::new(Type::I32),
            len: 5,
        });
        assert_eq!(size, 40);
        assert_eq!(align, 8);
    }

    #[test]
    fn fn_pointer_size() {
        let (size, align) = tsa(&Type::Fn {
            params: vec![Type::I32],
            ret: Box::new(Type::I32),
        });
        assert_eq!(size, 8);
        assert_eq!(align, 8);
    }

    #[test]
    fn cache_resolves_user_defined_type() {
        let mut cache = LayoutCache::new();
        cache.insert("Color".to_string(), (1, 1));
        let (size, align) = type_size_align(&Type::UnresolvedNamed("Color".to_string()), &cache);
        assert_eq!(size, 1);
        assert_eq!(align, 1);
    }

    #[test]
    fn struct_field_uses_cache() {
        // Struct Inner { x: i32, y: i32 } → size 16, align 8 (i32 stored as i64)
        // Struct Outer { inner: Inner, z: i32 }
        let mut cache = LayoutCache::new();
        cache.insert("Inner".to_string(), (16, 8));
        let decl = make_struct("Outer", vec![("inner", "Inner"), ("z", "i32")]);
        let layout = compute_struct_layout(&decl, &[], &cache);
        assert_eq!(layout.fields[0].size, 16); // Inner
        assert_eq!(layout.fields[0].align, 8);
        assert_eq!(layout.fields[1].offset, 16); // z at offset 16
        assert_eq!(layout.size, 24); // 16 + 8 = 24
        assert_eq!(layout.align, 8);
    }

    // ── compute_struct_layout ───────────────────────────────────

    #[test]
    fn empty_struct() {
        let decl = make_struct("Empty", vec![]);
        let layout = compute_struct_layout(&decl, &[], &empty_cache());
        assert_eq!(layout.name, "Empty");
        assert_eq!(layout.size, 0);
        assert_eq!(layout.align, 1);
        assert!(layout.fields.is_empty());
    }

    #[test]
    fn single_field_struct() {
        let decl = make_struct("Point", vec![("x", "i32")]);
        let layout = compute_struct_layout(&decl, &[], &empty_cache());
        assert_eq!(layout.name, "Point");
        assert_eq!(layout.size, 8);
        assert_eq!(layout.align, 8);
        assert_eq!(layout.fields.len(), 1);
        assert_eq!(layout.fields[0].name, "x");
        assert_eq!(layout.fields[0].offset, 0);
        assert_eq!(layout.fields[0].size, 8);
    }

    #[test]
    fn two_field_struct() {
        let decl = make_struct("Point", vec![("x", "i32"), ("y", "i32")]);
        let layout = compute_struct_layout(&decl, &[], &empty_cache());
        assert_eq!(layout.size, 16);
        assert_eq!(layout.align, 8);
        assert_eq!(layout.fields[0].offset, 0);
        assert_eq!(layout.fields[1].offset, 8);
    }

    // ── compute_enum_layout ─────────────────────────────────────

    #[test]
    fn fieldless_enum() {
        // enum Color { Red, Green, Blue } → tag only, no payload
        let decl = make_enum(
            "Color",
            vec![("Red", vec![]), ("Green", vec![]), ("Blue", vec![])],
        );
        let layout = compute_enum_layout(&decl, &[], &empty_cache());
        assert_eq!(layout.name, "Color");
        assert_eq!(layout.tag_offset, 0); // E1: tag first
        assert_eq!(layout.variants.len(), 3);
        assert_eq!(layout.variants[0].tag, 0);
        assert_eq!(layout.variants[1].tag, 1);
        assert_eq!(layout.variants[2].tag, 2);
        // No payload → size is just tag (U8 stored as i64 = 8 bytes)
        assert_eq!(layout.size, 8);
    }

    #[test]
    fn enum_with_payload() {
        // enum Shape { Circle(i32), Rect(i32, i32) }
        let decl = make_enum(
            "Shape",
            vec![("Circle", vec!["i32"]), ("Rect", vec!["i32", "i32"])],
        );
        let layout = compute_enum_layout(&decl, &[], &empty_cache());
        assert_eq!(layout.tag_offset, 0);
        assert!(matches!(layout.tag_ty, Type::U8)); // <=256 variants

        // Circle payload: 1 field × 8 bytes = 8 (i32 stored as i64)
        assert_eq!(layout.variants[0].payload_size, 8);
        // Rect payload: 2 fields × 8 bytes = 16
        assert_eq!(layout.variants[1].payload_size, 16);

        // All variants share the same payload_offset
        assert_eq!(layout.variants[0].payload_offset, layout.variants[1].payload_offset);

        // Total: tag (8) + max_payload (16) = 24
        assert_eq!(layout.size, 24);
        assert_eq!(layout.align, 8);
    }

    #[test]
    fn enum_mixed_payload_sizes() {
        // enum Msg { Empty, Single(i32), Pair(i32, i32) }
        let decl = make_enum(
            "Msg",
            vec![
                ("Empty", vec![]),
                ("Single", vec!["i32"]),
                ("Pair", vec!["i32", "i32"]),
            ],
        );
        let layout = compute_enum_layout(&decl, &[], &empty_cache());

        assert_eq!(layout.variants[0].payload_size, 0);
        assert_eq!(layout.variants[1].payload_size, 8);
        assert_eq!(layout.variants[2].payload_size, 16);

        // Size = tag (8) + max_payload (16) = 24
        assert_eq!(layout.size, 24);
    }

}
