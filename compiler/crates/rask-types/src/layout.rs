// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Memory layout computation - field offsets, sizes, alignments.

use rask_ast::decl::Decl;
use rask_ast::ty::TypeExpr;
use crate::Type;
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
    /// Declared `@resource`, so its values must be consumed exactly once
    /// (mem.linear/L1).
    ///
    /// The compiler enforces that for named bindings; no container may hold a
    /// linear value at all (`mem.resource-types/RC1`-RC3).
    pub is_resource: bool,
}

/// Field layout within struct
#[derive(Debug, Clone)]
pub struct FieldLayout {
    pub name: String,
    pub ty: Type,
    pub offset: u32,
    pub size: u32,
    pub align: u32,
    /// Field annotations, verbatim (`rename("user_name")`, `no_serialize`, …).
    /// Serialization reads these — see `rask_ast::decl::field_attrs`.
    pub attrs: Vec<rask_ast::attr::Attr>,
    /// Whether the field has a declared default value (`x: T = v`). Separate
    /// from `@default(...)` in `attrs`, which is a decode-only override
    /// (`type.structs/FD6`) — `reflect.fields<T>()`'s `has_default` is true
    /// for either.
    pub has_declared_default: bool,
    /// V5 visibility. Carried because `reflect.fields<T>()` reports it: without
    /// it the native side had nothing to read and answered `true` for every
    /// field, so a `private` one looked public there and not on the
    /// interpreter (std.encoding/E13).
    pub is_public: bool,
    /// Where the field sits in the *declaration*, which is not where it sits in
    /// memory: `@layout(Rask)` reorders by alignment (S1/L4).
    ///
    /// `reflect.fields<T>()` reports declaration order, because that is what the
    /// author wrote and what a serializer's key order follows. It used to walk
    /// the layout, and matched by accident: every field had the same alignment
    /// while every scalar was a machine word, so the stable sort never moved
    /// anything. Real field widths made the sort real, and native started
    /// reporting `role` before `login_count` while the interpreter — the
    /// reference — reported the declaration (#1083).
    pub decl_index: u32,
    /// The field was declared with one of the type's parameters — `value: T` —
    /// so `ty` here is whatever got substituted in, not what the source said.
    ///
    /// The *shared* layout for a generic type substitutes `i64` for every
    /// parameter, which is the right size for anything that fits a word. It is
    /// not the right register class: `Box<f64>` reading its field through the
    /// shared layout loaded the double's bits into an integer register, and the
    /// conversion on the way out printed `wrap(3.14).value` as
    /// 4614253070214988800 (#820). Codegen has the field's real MIR type at the
    /// read; this says when to prefer it.
    pub is_type_param: bool,
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
    /// Enum-level annotations, verbatim (`tag("type")`, …). JSON encoding
    /// reads these for internal tagging (std.encoding/E24) — see
    /// `rask_ast::decl::field_attrs`.
    pub attrs: Vec<rask_ast::attr::Attr>,
}

impl EnumLayout {
    /// How many bytes the tag occupies: its declared width, never a word.
    pub fn tag_bytes(&self) -> u32 {
        self.tag_ty.scalar_bytes().unwrap_or(1)
    }
}

/// Variant layout within enum
#[derive(Debug, Clone)]
pub struct VariantLayout {
    pub name: String,
    pub tag: u64,
    pub payload_offset: u32,
    pub payload_size: u32,
    pub fields: Vec<FieldLayout>,
    /// Variant-level annotations, verbatim (`rename("...")`, …). JSON
    /// encoding reads these for variant rename (std.encoding/E25).
    pub attrs: Vec<rask_ast::attr::Attr>,
}

/// The size and alignment of a scalar, by its type. Panics on anything else.
fn scalar_by_name(ty: &Type) -> (u32, u32) {
    let n = ty.scalar_bytes().expect("a scalar knows its width");
    (n, n)
}

/// A user generic's size: its instance layout when one was made, else the
/// shared one. Both are in the cache by the time a type holding one is laid
/// out, since declarations go in dependency order.
///
/// Not a zero. A stdlib generic like `Sequence<T>` is an empty struct standing
/// in for a runtime value, so the cache says it's nothing at all, and taking
/// that gave `Holder { inner: Sequence<i64> }` a field with no room in it.
fn cached_generic_layout(
    name: &str,
    args: &[crate::GenericArg],
    cache: &LayoutCache,
) -> Option<(u32, u32)> {
    let arg_tys: Vec<Type> = args
        .iter()
        .filter_map(|a| match a {
            crate::GenericArg::Type(t) => Some((**t).clone()),
            _ => None,
        })
        .collect();
    generic_instance_name(name, &arg_tys, &HashMap::new())
        .and_then(|n| cache.get(&n).copied())
        .or_else(|| cache.get(name).copied())
        .filter(|(size, _)| *size > 0)
}

fn is_link(ty: &Type) -> bool {
    matches!(ty, Type::UnresolvedGeneric { name, .. } if name == "Link")
}

/// Where a `T?`, `T or E` or error union keeps its payload, and its size, by
/// `crate::abi`'s rules. `None` for anything else and for a niche `Link<T>?`,
/// which is its payload.
///
/// An error union is `[member:1][payload]`: the members are nominally distinct
/// types with nothing in their bytes to tell them apart, so the index is
/// stored (#776).
pub fn wrapper_layout(ty: &Type, cache: &LayoutCache) -> Option<crate::abi::WrapperLayout> {
    wrapper_layout_in(ty, cache, &mut Lenient)
}

fn wrapper_layout_in(ty: &Type, cache: &LayoutCache, on_unknown: &mut dyn Unknown) -> Option<crate::abi::WrapperLayout> {
    let mut widest = |types: &[&Type]| {
        let (mut size, mut align) = (0u32, 1u32);
        for t in types {
            let (s, a) = size_align_in(t, cache, on_unknown)?;
            size = size.max(s);
            align = align.max(a);
        }
        Some((size, align))
    };
    match ty {
        Type::Result { ok, err } if !ty.is_option() => {
            let (size, align) = widest(&[ok.as_ref(), err.as_ref()])?;
            Some(crate::abi::result_layout(size, align))
        }
        Type::Union(members) => {
            let (size, align) = widest(&members.iter().collect::<Vec<_>>())?;
            Some(crate::abi::union_layout(size, align))
        }
        _ => {
            let inner = ty.as_option().filter(|inner| !is_link(inner))?;
            let (size, align) = size_align_in(inner, cache, on_unknown)?;
            Some(crate::abi::option_layout(size, align))
        }
    }
}

/// What to do about a type the layout can't place: an unresolved generic, an
/// inference variable, a name nothing declares.
trait Unknown {
    /// The size to use instead, or `None` to give up on the whole question.
    fn unknown(&mut self, what: std::fmt::Arguments) -> Option<(u32, u32)>;
}

/// Codegen's answer: a word, with a warning. Laying out has to finish.
struct Lenient;
impl Unknown for Lenient {
    fn unknown(&mut self, what: std::fmt::Arguments) -> Option<(u32, u32)> {
        eprintln!("warning: {what}, defaulting to (8, 8)");
        Some((8, 8))
    }
}

/// A question about the language's size of a type — `reflect.size_of`, the
/// `transmute` test — gets no answer rather than a guess.
struct Strict;
impl Unknown for Strict {
    fn unknown(&mut self, _: std::fmt::Arguments) -> Option<(u32, u32)> {
        None
    }
}

/// Get size and alignment for a type (after monomorphization).
/// `cache` maps type names to already-computed (size, align) for user-defined types.
pub fn type_size_align(ty: &Type, cache: &LayoutCache) -> (u32, u32) {
    size_align_in(ty, cache, &mut Lenient).expect("the lenient layout always answers")
}

/// `type_size_align`, or `None` where it would have had to guess.
pub fn known_size_align(ty: &Type, cache: &LayoutCache) -> Option<(u32, u32)> {
    size_align_in(ty, cache, &mut Strict)
}

fn size_align_in(ty: &Type, cache: &LayoutCache, on_unknown: &mut dyn Unknown) -> Option<(u32, u32)> {
    Some(match ty {
        // Every value holds its own width, wherever it sits: a struct field,
        // an enum tag or payload field, a wrapper's payload. Nothing is padded
        // out to a word.
        Type::Unit => (0, 1),
        // Widths live on the type (`Type::scalar_bytes`), so this pass and the
        // atomic-payload rule read the same numbers. Natural alignment.
        Type::Bool | Type::I8 | Type::U8 | Type::I16 | Type::U16
        | Type::I32 | Type::U32 | Type::F32
        | Type::I64 | Type::U64 | Type::F64 | Type::Char
        | Type::I128 | Type::U128 => {
            let n = ty.scalar_bytes().expect("a scalar knows its width");
            (n, n)
        }
        Type::String => (16, 8), // 16-byte SSO inline (RaskStr union)
        // Niche optimization: `Link<T>?` is the node's address with null
        // for `none` — one word, no tag.
        ty if ty.as_option().is_some_and(is_link) => (8, 8),
        Type::Result { .. } => {
            let l = wrapper_layout_in(ty, cache, on_unknown)?;
            (l.size, l.align)
        }
        Type::Tuple(types) => {
            let mut offset = 0u32;
            let mut max_align = 1u32;
            for ty in types {
                let (size, align) = size_align_in(ty, cache, on_unknown)?;
                max_align = max_align.max(align);
                offset = align_up(offset, align);
                offset += size;
            }
            let total_size = align_up(offset, max_align);
            (total_size, max_align)
        }
        Type::Array { elem, len } => {
            let (elem_size, elem_align) = size_align_in(elem, cache, on_unknown)?;
            (elem_size * (*len as u32), elem_align)
        }
        Type::Fn { .. } => (8, 8), // Function pointer
        Type::Named(_) | Type::Generic { .. } => {
            // Named types carry a TypeId — can't resolve by name here.
            // Assume pointer-sized; struct/enum layouts are computed separately.
            (8, 8)
        }
        Type::UnresolvedGeneric { name, .. } if generic_is_one_word(name) => (8, 8),
        Type::UnresolvedGeneric { name, args } => {
            if let Some(found) = cached_generic_layout(name, args, cache) {
                return Some(found);
            }
            return on_unknown.unknown(format_args!(
                "unresolved generic type in layout: {}<{} arg(s)>",
                name,
                args.len()
            ));
        }
        Type::Var(id) => {
            return on_unknown.unknown(format_args!("type variable ?{} in layout computation", id.0));
        }
        // AT6: projections resolve during type checking. One that got here
        // named a conformance that doesn't exist, and the error for that is
        // already reported — lay it out as a word rather than panicking on top.
        Type::Assoc { .. } => (8, 8),
        // A raw pointer field written `*u8` arrives as a name too. It's a
        // pointer, so the fallback size was right — but it went through the
        // unknown-type branch and warned about a program with nothing wrong
        // with it.
        Type::UnresolvedNamed(name) if name.starts_with('*') => (8, 8),
        // A `c_int` field out of an `import c` header is an `i32` — the widths
        // live in one table (`c_type_spelling`) so the checker and the layout
        // can't drift. Without this an imported C struct sized every field at a
        // pointer and the offsets missed what C put there.
        Type::UnresolvedNamed(name) if rask_ast::primitives::c_type_spelling(name).is_some() => {
            let spelling = rask_ast::primitives::c_type_spelling(name).unwrap();
            return size_align_in(&Type::UnresolvedNamed(spelling.to_string()), cache, on_unknown);
        }
        Type::UnresolvedNamed(name) => {
            match name.as_str() {
                // A `StringView` is a `RaskStr` that shares the source's buffer
                // (std.strings/V1) — same 16 bytes as a string.
                "string" | "Path" | "StringView" => (16, 8),
                // A scalar written as a name is the same scalar. `char` used
                // to answer 4 here and 8 through the typed arm above; one
                // source now.
                "bool" => scalar_by_name(&Type::Bool),
                "i8" => scalar_by_name(&Type::I8),
                "u8" => scalar_by_name(&Type::U8),
                "i16" => scalar_by_name(&Type::I16),
                "u16" => scalar_by_name(&Type::U16),
                "i32" => scalar_by_name(&Type::I32),
                "u32" => scalar_by_name(&Type::U32),
                "f32" => scalar_by_name(&Type::F32),
                "i64" => scalar_by_name(&Type::I64),
                "u64" => scalar_by_name(&Type::U64),
                "f64" => scalar_by_name(&Type::F64),
                "char" => scalar_by_name(&Type::Char),
                // One word: the address of storage the runtime made. Its
                // declaration has no fields, so the layout cache says zero.
                name if rask_stdlib::is_runtime_handle(name) => (8, 8),
                // A word, but not for the reason the line above is: these two
                // are an `int64_t` of nanoseconds in the runtime, not a pointer
                // to anything. Their Rask declarations are empty structs, so
                // without an entry here the layout cache answered with the
                // declaration's size — zero — and a struct holding one gave the
                // field no room at all (#924).
                "Duration" | "Instant" => (8, 8),
                _ => {
                    // A declared type's layout. A handle never gets here: its
                    // declaration is empty, so the cache would say zero for
                    // `Vec` (#1081).
                    let cached_size = cache.get(name.as_str());
                    if let Some(&cached) = cached_size {
                        cached
                    } else if is_typevar_name(name) {
                        // Unsubstituted type parameter — pointer-sized fallback,
                        // silent because mono always picks a concrete size on real call sites.
                        (8, 8)
                    } else {
                        // Treat as opaque pointer-sized. If this is a user type,
                        // it should have been caught by the type checker.
                        return on_unknown.unknown(format_args!("unknown type '{}' in layout", name));
                    }
                }
            }
        }
        Type::Union(_) => {
            let l = wrapper_layout_in(ty, cache, on_unknown)?;
            (l.size, l.align)
        }
        Type::SimdVector { elem, lanes } => {
            let (elem_size, _) = size_align_in(elem, cache, on_unknown)?;
            let total = elem_size * *lanes as u32;
            (total, total.min(32)) // natural SIMD alignment, cap at 32
        }
        Type::Never => (0, 1),
        Type::None => (0, 1),
        Type::InterfaceObject { .. } => (16, 8), // Fat pointer: data_ptr + vtable_ptr
        Type::RawPtr(_) => (8, 8), // Pointer-sized
        Type::Error => return on_unknown.unknown(format_args!("Error type in layout computation")),
    })
}

/// Align a value up to the given alignment
fn align_up(val: u32, align: u32) -> u32 {
    (val + align - 1) & !(align - 1)
}

/// Heuristic: single uppercase letter (or letter + digit) is a type parameter
/// like `T`, `K`, `V`, `T1`. These should never reach layout in monomorphized
/// code; the warning is noise on every compile.
fn is_typevar_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    match bytes.len() {
        1 => bytes[0].is_ascii_uppercase(),
        2 => bytes[0].is_ascii_uppercase() && bytes[1].is_ascii_digit(),
        _ => false,
    }
}

/// A written field type as the `Type` layout works with. Public because the
/// interpreter reads type arguments the same way: one written in a
/// `reflect.fields<Pair<string>>()` call has to become the `Type` the layout
/// pass expects (#1104).
pub fn field_type(ty: &TypeExpr) -> Type {
    match ty {
        TypeExpr::Unit => Type::Unit,
        TypeExpr::Optional(inner) => Type::option(field_type(inner)),
        TypeExpr::Result { ok, err } => Type::Result {
            ok: Box::new(field_type(ok)),
            err: Box::new(field_type(err)),
        },
        // A symbolic length (`[T; SIZE]`) has no value here — there is no const
        // table in this pass. Keeping it a name keeps its warning rather than
        // sizing the field at zero (#906).
        TypeExpr::Array { elem, len } => match len.parse::<usize>() {
            Ok(len) => Type::Array { elem: Box::new(field_type(elem)), len },
            Err(_) => Type::UnresolvedNamed(ty.to_string()),
        },
        TypeExpr::Tuple(elems) => Type::Tuple(elems.iter().map(field_type).collect()),
        // An error union, `T or A | B`. It fell to the catch-all below and was
        // sized as a name spelled `A#133|B`, one word, so a struct holding one
        // got a slot too small for the value MIR stored in it (#1521).
        TypeExpr::Union(members) => Type::Union(members.iter().map(field_type).collect()),
        TypeExpr::Func { params, ret } => Type::Fn {
            params: params
                .iter()
                .map(|p| crate::FnParam { mode: p.mode, ty: field_type(&p.ty) })
                .collect(),
            ret: Box::new(field_type(ret)),
        },
        TypeExpr::RawPtr(inner) => Type::RawPtr(Box::new(field_type(inner))),
        // A fat pointer. Left as the name "any Shape" it sized right only by a
        // spelling check, and `any Shape?` became an option of a one-word
        // name: eight bytes short of the fat pointer stored in it (#1308).
        // The written name is the interface's symbol by now (#1426), and a
        // size is all this needs.
        TypeExpr::Any(interface) => Type::InterfaceObject { interface_name: interface.to_string(), decl: None },
        // Whatever a field's type is reached *through* says nothing about its
        // size, so the last segment is the whole question: `time.Duration` and
        // an aliased import's `h.Response` both size as the type they name.
        TypeExpr::Named { path, args } => {
            let name = path.last().map(String::as_str).unwrap_or_default();
            match (name, args.as_slice()) {
                (_, []) => primitive_or_named(name),
                ("Option", [inner]) => Type::option(field_type(inner)),
                ("Result", [ok, err]) => Type::Result {
                    ok: Box::new(field_type(ok)),
                    err: Box::new(field_type(err)),
                },
                _ => Type::UnresolvedGeneric {
                    name: name.to_string(),
                    args: args
                        .iter()
                        .map(|a| crate::GenericArg::Type(Box::new(field_type(a))))
                        .collect(),
                },
            }
        }
        _ => Type::UnresolvedNamed(ty.to_string()),
    }
}

fn primitive_or_named(name: &str) -> Type {
    match name {
        "bool" => Type::Bool,
        "i8" => Type::I8,
        "i16" => Type::I16,
        "i32" => Type::I32,
        "i64" => Type::I64,
        "isize" => Type::isize_ty(),
        "i128" => Type::I128,
        "u8" => Type::U8,
        "u16" => Type::U16,
        "u32" => Type::U32,
        "u64" => Type::U64,
        "usize" => Type::usize_ty(),
        "u128" => Type::U128,
        "f32" => Type::F32,
        "f64" => Type::F64,
        "char" => Type::Char,
        "string" => Type::String,
        name => Type::UnresolvedNamed(name.to_string()),
    }
}


/// Build a substitution map from type param names to concrete types.
///
/// Names come from PC1, not from the explicit `<T>` list: a single letter in a
/// field or payload type is a parameter whether or not it was declared, and the
/// type arguments are ordered by that same list. Reading only the explicit list
/// left an implicit-param struct with an empty map, so its fields kept their
/// placeholder types and the layout came out wrong (#913).
fn build_subst<'a>(
    param_names: &'a [String],
    type_args: &'a [Type],
) -> std::collections::HashMap<&'a str, &'a Type> {
    let mut subst = std::collections::HashMap::new();
    for (name, arg) in param_names.iter().zip(type_args.iter()) {
        subst.insert(name.as_str(), arg);
    }
    subst
}

/// A field's type with generic substitution applied.
/// If the parsed type is an unresolved name that matches a type parameter,
/// replace it with the concrete type from type_args.
///
/// The flag says the field was declared with one of the type's parameters, so the
/// returned type is a substitution rather than what the source wrote — see
/// `FieldLayout::is_type_param`.
fn resolve_field_type(
    field_ty: &TypeExpr,
    subst: &std::collections::HashMap<&str, &Type>,
) -> (Type, bool) {
    let parsed = field_type(field_ty);
    match &parsed {
        Type::UnresolvedNamed(name) => {
            if let Some(concrete) = subst.get(name.as_str()) {
                ((*concrete).clone(), true)
            } else {
                (parsed, false)
            }
        }
        _ => (parsed, false),
    }
}

/// The parameters inside a field type replaced, for sizing it: `Tasks<T>` in
/// a `Group<string>` is `Tasks<string>`, whose instance layout is wider than
/// the shared one. The recorded type keeps its `T` — reflection substitutes
/// the real argument itself, and the shared layout's stand-in `i64` would
/// read as the answer.
pub fn substitute_inside(ty: &Type, subst: &std::collections::HashMap<&str, &Type>) -> Type {
    use crate::GenericArg;
    let go = |t: &Type| substitute_inside(t, subst);
    match ty {
        Type::UnresolvedNamed(name) => match subst.get(name.as_str()) {
            Some(concrete) => (*concrete).clone(),
            None => ty.clone(),
        },
        Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
            name: name.clone(),
            args: args
                .iter()
                .map(|a| match a {
                    GenericArg::Type(t) => GenericArg::Type(Box::new(go(t))),
                    other => other.clone(),
                })
                .collect(),
        },
        t if t.is_option() => Type::option(go(t.as_option().unwrap())),
        Type::Result { ok, err } => Type::Result { ok: Box::new(go(ok)), err: Box::new(go(err)) },
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(go).collect()),
        Type::Array { elem, len } => Type::Array { elem: Box::new(go(elem)), len: *len },
        _ => ty.clone(),
    }
}

/// A builtin generic that is one pointer whatever its arguments are: a
/// runtime handle (`rask_stdlib::handles`).
///
/// Laying out a type that holds one never needs its arguments' layouts, so
/// `Vec<Expr>` inside `Wrapped` doesn't make `Wrapped` wait for `Expr`.
pub fn generic_is_one_word(name: &str) -> bool {
    rask_stdlib::is_runtime_handle(name)
}

/// A type argument that owns storage of its own.
///
/// The shared layout a generic type gets puts `i64` in every parameter slot,
/// which is the right *size* for a handle and the wrong type: the release walk
/// reads the layout, sees a scalar, and frees nothing — so `Pair<i64,
/// Vec<i64>>`'s vector field belonged to nobody. An instance layout is what
/// puts the real type back, and this is what asks for one.
///
/// Deliberately wider than the set anything actually frees. An extra instance
/// layout costs a little compile time; a missing one costs the value. A
/// `string` argument is not here and doesn't need to be — a string is
/// refcounted, so the frame's own name gives it back.
pub fn arg_owns_storage(name: &str) -> bool {
    rask_stdlib::is_runtime_handle(name)
}

/// Check whether a struct has `@layout(C)` attribute.
fn has_c_layout(attrs: &[rask_ast::attr::Attr]) -> bool {
    attrs.iter().any(|a| a.is("layout") && a.positional().and_then(|v| v.ident()) == Some("C"))
}

/// Compute struct layout with field offsets (spec rules S1-S4, L4)
/// Was this declaration parsed out of a stdlib stub?
///
/// The stdlib's sources occupy the top of the `file_id` range by construction
/// (`STDLIB_FILE_ID_BASE`), so the span answers it — no flag has to be threaded
/// down from whoever collected the declarations.
pub fn is_stdlib_span(span: rask_ast::Span) -> bool {
    span.file_id >= rask_stdlib::stubs::STDLIB_FILE_ID_BASE
}

pub fn compute_struct_layout(struct_def: &Decl, type_args: &[Type], cache: &LayoutCache) -> StructLayout {
    struct_layout(struct_def, type_args, cache, true)
}

/// A generic struct's shared layout: a word standing in for each type
/// parameter. The stand-in sizes the fields and isn't recorded in them —
/// `Vec<T>` stays `Vec<T>`, or reflection on `Ring<string>` read `Vec<i64>`.
pub fn compute_shared_struct_layout(struct_def: &Decl, cache: &LayoutCache) -> StructLayout {
    struct_layout(struct_def, &stand_ins(struct_def), cache, false)
}

/// The enum version of `compute_shared_struct_layout`.
pub fn compute_shared_enum_layout(enum_def: &Decl, cache: &LayoutCache) -> EnumLayout {
    enum_layout(enum_def, &stand_ins(enum_def), cache, false)
}

fn stand_ins(decl: &Decl) -> Vec<Type> {
    use rask_ast::decl::DeclKind;
    let n = match &decl.kind {
        DeclKind::Struct(s) => crate::struct_type_param_names(s).len(),
        DeclKind::Enum(e) => crate::enum_type_param_names(e).len(),
        _ => 0,
    };
    vec![Type::I64; n]
}

/// `record_arguments`: write the arguments into field types nested inside
/// others (`Tasks<T>` → `Tasks<string>`), which an instance layout needs so
/// the field's own instance is found. A bare `T` field is substituted either
/// way and marked as coming from a parameter.
fn struct_layout(
    struct_def: &Decl,
    type_args: &[Type],
    cache: &LayoutCache,
    record_arguments: bool,
) -> StructLayout {
    use rask_ast::decl::DeclKind;

    let struct_decl = match &struct_def.kind {
        DeclKind::Struct(s) => s,
        _ => panic!("Expected struct declaration"),
    };

    let param_names = crate::struct_type_param_names(struct_decl);
    let subst = build_subst(&param_names, type_args);
    let c_layout = has_c_layout(&struct_decl.attrs);
    // A `@binary` struct's declared field types are wire specifiers
    // (type.binary/F2), not type names — `u16be` says "16 bits, big-endian".
    // The in-memory layout is the runtime type each one stands for; reading the
    // specifier as a type name gave every field a pointer-sized slot and a
    // warning per compile.
    let is_binary = struct_decl.attrs.iter().any(|a| a.is("binary"));

    // Resolve types and compute sizes for all fields first
    let mut resolved: Vec<(String, Type, u32, u32, Vec<rask_ast::attr::Attr>, bool, bool, bool, u32)> = struct_decl.fields.iter()
        .enumerate()
        .map(|(decl_index, field)| {
            let (field_ty, from_param) = if is_binary {
                match crate::binary_field_runtime_type(&field.ty) {
                    Some(ty) => (ty, false),
                    None => resolve_field_type(&field.ty, &subst),
                }
            } else {
                resolve_field_type(&field.ty, &subst)
            };
            let sized = substitute_inside(&field_ty, &subst);
            let (field_size, field_align) = type_size_align(&sized, cache);
            let field_ty = if record_arguments { sized } else { field_ty };
            (
                field.name.clone(),
                field_ty,
                field_size,
                field_align,
                field.attrs.clone(),
                field.default.is_some(),
                field.visibility.is_pub(),
                from_param,
                decl_index as u32,
            )
        })
        .collect();

    // type.structs: `@packed` puts every field at alignment 1, in the order
    // written — with no padding to save there is nothing to reorder for.
    // `@align(N)` raises the struct's own alignment and so its tail padding.
    // A malformed request is the checker's error; here it reads as none.
    let request = rask_ast::attr::layout_request(&struct_decl.attrs).unwrap_or_default();
    if request.packed {
        for field in &mut resolved {
            field.3 = 1;
        }
    }

    // S1/L4: Default @layout(Rask) reorders by alignment (largest first).
    // Stable sort preserves source order for fields with equal alignment.
    // S2: @layout(C) preserves source order for FFI.
    if !c_layout && !request.packed {
        resolved.sort_by(|a, b| b.3.cmp(&a.3));
    }

    let mut field_layouts = Vec::new();
    let mut offset = 0u32;
    let mut max_align = 1u32;

    for (name, ty, size, align, attrs, has_declared_default, is_public, is_type_param, decl_index) in resolved {
        max_align = max_align.max(align);
        // S3: Align offset for this field
        offset = align_up(offset, align);

        field_layouts.push(FieldLayout {
            name,
            ty,
            offset,
            size,
            align,
            attrs,
            has_declared_default,
            is_public,
            decl_index,
            is_type_param,
        });

        offset += size;
    }

    if let Some(n) = request.align {
        max_align = max_align.max(n);
    }

    // S4: Total size with tail padding to struct alignment
    let total_size = align_up(offset, max_align);

    StructLayout {
        name: struct_decl.name.clone(),
        size: total_size,
        align: max_align,
        fields: field_layouts,
        is_resource: struct_decl.attrs.iter().any(|a| a.is("resource")),
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

    for (decl_index, field) in union_decl.fields.iter().enumerate() {
        let field_ty = field_type(&field.ty);
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
            attrs: field.attrs.clone(),
            has_declared_default: field.default.is_some(),
            is_public: field.visibility.is_pub(),
            decl_index: decl_index as u32,
            is_type_param: false,
        });
    }

    let total_size = align_up(max_size, max_align);

    StructLayout {
        name: union_decl.name.clone(),
        size: total_size,
        align: max_align,
        fields: field_layouts,
        is_resource: false,
    }
}

/// The layout for `Ordering`.
///
/// `Ordering` is registered by the compiler rather than declared in source, so
/// no decl reaches `compute_enum_layout` for it. Without a layout the backends
/// had no variant tags to read, every stage carried its own
/// `enum_name == "Ordering"` branch, and `compare` gave up and handed back a
/// bare integer (#729).
///
/// Built from `ORDERING_VARIANTS` so the variant list stays in one place, and
/// sized like any other fieldless enum: a one-byte tag. Every store of its tag
/// has to be one byte too. When stores were a word and this slot one byte,
/// `Ordering.Less` wrote eight bytes into it and `==` read eight back, so two
/// equal values compared equal or not depending on what was next to them on
/// the stack.
pub fn ordering_layout() -> EnumLayout {
    let (tag_size, tag_align) = type_size_align(&Type::U8, &LayoutCache::new());
    EnumLayout {
        name: "Ordering".to_string(),
        size: tag_size,
        align: tag_align,
        tag_ty: Type::U8,
        tag_offset: 0,
        variants: rask_stdlib::ORDERING_VARIANTS
            .iter()
            .enumerate()
            .map(|(tag, name)| VariantLayout {
                name: (*name).to_string(),
                tag: tag as u64,
                payload_offset: tag_size,
                payload_size: 0,
                fields: Vec::new(),
                attrs: Vec::new(),
            })
            .collect(),
        attrs: Vec::new(),
    }
}

/// Compute enum layout with tag and variant payloads (spec rules E1-E6)
pub fn compute_enum_layout(enum_def: &Decl, type_args: &[Type], cache: &LayoutCache) -> EnumLayout {
    enum_layout(enum_def, type_args, cache, true)
}

fn enum_layout(
    enum_def: &Decl,
    type_args: &[Type],
    cache: &LayoutCache,
    record_arguments: bool,
) -> EnumLayout {
    use rask_ast::decl::DeclKind;

    let enum_decl = match &enum_def.kind {
        DeclKind::Enum(e) => e,
        _ => panic!("Expected enum declaration"),
    };

    let param_names = crate::enum_type_param_names(enum_decl);
    let subst = build_subst(&param_names, type_args);

    let variant_count = enum_decl.variants.len();

    // E2/E14: Determine discriminant type
    let tag_ty = if let Some(ref bt) = enum_decl.backing_type {
        match bt.bare_name().unwrap_or_default() {
            "u8" => Type::U8,
            "u16" => Type::U16,
            "u32" => Type::U32,
            "i8" => Type::I8,
            "i16" => Type::I16,
            "i32" => Type::I32,
            "i64" => Type::I64,
            _ => if variant_count <= 256 { Type::U8 } else { Type::U16 },
        }
    } else if variant_count <= 256 {
        Type::U8
    } else {
        Type::U16
    };
    // The tag and every payload field hold their real widths, the same as a
    // struct's fields (#1083).
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
            for (decl_index, field) in variant.fields.iter().enumerate() {
                let (field_ty, from_param) = resolve_field_type(&field.ty, &subst);
                let sized = substitute_inside(&field_ty, &subst);
                let (size, align) = type_size_align(&sized, cache);
                let field_ty = if record_arguments { sized } else { field_ty };

                payload_align = payload_align.max(align);
                field_offset = align_up(field_offset, align);

                variant_fields.push(FieldLayout {
                    name: field.name.clone(),
                    ty: field_ty,
                    offset: field_offset,
                    size,
                    align,
                    attrs: Vec::new(),
                    has_declared_default: field.default.is_some(),
                    // A variant's payload has no visibility of its own.
                    is_public: true,
                    decl_index: decl_index as u32,
                    is_type_param: from_param,
                });

                field_offset += size;
            }
            payload_size = align_up(field_offset, payload_align);
        }

        max_payload_size = max_payload_size.max(payload_size);
        max_payload_align = max_payload_align.max(payload_align);

        variant_layouts.push(VariantLayout {
            name: variant.name.clone(),
            tag: variant.discriminant.unwrap_or(tag as i128) as u64,
            payload_offset: 0, // Will be computed from tag
            payload_size,
            fields: variant_fields,
            attrs: variant.attrs.clone(),
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
        attrs: enum_decl.attrs.clone(),
    }
}

/// A checked type respelled the way the layout cache is keyed — by name, not
/// by `TypeId` — or `None` when it has no layout: a type parameter (a single
/// capital, PC3), an open variable, a projection.
pub fn layout_spelling(ty: &Type, types: &crate::TypeTable) -> Option<Type> {
    spell(ty, types, false)
}

/// `layout_spelling` for a question that has to be answered whatever the type
/// holds — the copy rule inside a generic body. A type parameter or an open
/// variable stands in as a word, the size the shared layout gives it.
fn spell_lenient(ty: &Type, types: &crate::TypeTable) -> Type {
    spell(ty, types, true).expect("a lenient spelling always answers")
}

fn spell(ty: &Type, types: &crate::TypeTable, lenient: bool) -> Option<Type> {
    let each = |tys: &[Type]| tys.iter().map(|t| spell(t, types, lenient)).collect::<Option<Vec<_>>>();
    let args_of = |args: &[crate::GenericArg]| {
        args.iter()
            .map(|a| match a {
                crate::GenericArg::Type(t) => {
                    Some(crate::GenericArg::Type(Box::new(spell(t, types, lenient)?)))
                }
                other => Some(other.clone()),
            })
            .collect::<Option<Vec<_>>>()
    };
    let layout_spelling = |t: &Type, types: &crate::TypeTable| spell(t, types, lenient);
    Some(match ty {
        Type::Var(_) | Type::Assoc { .. } | Type::Error if lenient => Type::I64,
        Type::UnresolvedNamed(name) if lenient && crate::is_type_param_name(name) => Type::I64,
        Type::Var(_) | Type::Assoc { .. } | Type::Error => return None,
        Type::UnresolvedNamed(name) if crate::is_type_param_name(name) => return None,
        Type::Named(id) => Type::UnresolvedNamed(types.type_name(*id)),
        Type::Generic { base, args } => Type::UnresolvedGeneric {
            name: types.type_name(*base),
            args: args_of(args)?,
        },
        Type::UnresolvedGeneric { name, args } => Type::UnresolvedGeneric {
            name: name.clone(),
            args: args_of(args)?,
        },
        Type::Tuple(elems) => Type::Tuple(each(elems)?),
        Type::Union(members) => Type::Union(each(members)?),
        Type::Array { elem, len } => Type::Array { elem: Box::new(layout_spelling(elem, types)?), len: *len },
        Type::Result { ok, err } => Type::Result {
            ok: Box::new(layout_spelling(ok, types)?),
            err: Box::new(layout_spelling(err, types)?),
        },
        Type::RawPtr(inner) => Type::RawPtr(Box::new(layout_spelling(inner, types)?)),
        Type::SimdVector { elem, lanes } => Type::SimdVector {
            elem: Box::new(layout_spelling(elem, types)?),
            lanes: *lanes,
        },
        other => other.clone(),
    })
}

/// The layout name of one instantiation of a generic type — `One$Big`,
/// `Pair$i64$Big`.
///
/// `mangle_name` does this for functions, whose type arguments arrive already
/// normalized to names. A type argument here hasn't been: it can be a
/// `Named(TypeId)`, which prints as `<type#N>`. So the id gets resolved, and an
/// argument that can't be named at all means there is no instance layout to make
/// — the caller falls back to the shared placeholder layout.
///
/// MIR asks the same question of the same function, so the two agree by
/// construction rather than by convention.
pub fn generic_instance_name(
    base: &str,
    args: &[Type],
    type_names: &HashMap<crate::TypeId, String>,
) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    let mut parts = Vec::with_capacity(args.len());
    for arg in args {
        parts.push(type_arg_key(arg, type_names)?);
    }
    Some(format!("{}${}", base, parts.join("$")))
}

/// One type argument, spelled so it can key a layout. `None` for anything whose
/// identity isn't settled — an inference variable, an unresolved parameter name,
/// a shape with one of those inside.
fn type_arg_key(
    ty: &Type,
    type_names: &HashMap<crate::TypeId, String>,
) -> Option<String> {
    use crate::GenericArg;
    Some(match ty {
        Type::Bool | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::I128
        | Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::U128
        | Type::F32 | Type::F64 | Type::Char | Type::String | Type::Unit => format!("{}", ty),
        Type::Named(id) => type_names.get(id)?.to_string(),
        // An argument substituted into an instantiated copy is named, not
        // interned — the copy's types were built by rewriting strings, not by
        // going back through the checker's table (#814).
        Type::UnresolvedNamed(name) => name.to_string(),
        Type::UnresolvedGeneric { name, args } => {
            let base = name.to_string();
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                let GenericArg::Type(inner) = arg else { return None };
                parts.push(type_arg_key(inner, type_names)?);
            }
            format!("{}${}", base, parts.join("$"))
        }
        Type::Generic { base, args } => {
            let base = type_names.get(base)?.to_string();
            let mut parts = Vec::with_capacity(args.len());
            for arg in args {
                let GenericArg::Type(inner) = arg else { return None };
                parts.push(type_arg_key(inner, type_names)?);
            }
            format!("{}${}", base, parts.join("$"))
        }
        Type::Tuple(elems) => {
            let mut parts = Vec::with_capacity(elems.len());
            for elem in elems {
                parts.push(type_arg_key(elem, type_names)?);
            }
            format!("tup{}", parts.join("$"))
        }
        // A closure argument. The arity goes in the key because the parameters
        // and the return run together otherwise, and `func(i64, i64) -> void`
        // would key the same as `func(i64) -> func(i64) -> void`.
        Type::Fn { params, ret } => {
            let mut parts = Vec::with_capacity(params.len() + 1);
            // The mode is part of the type (FT1): `func(take T)` and
            // `func(T)` are two instantiations, not one.
            for p in params {
                let key = type_arg_key(&p.ty, type_names)?;
                parts.push(match p.mode.keyword() {
                    Some(kw) => format!("{}_{}", kw, key),
                    None => key,
                });
            }
            parts.push(type_arg_key(ret, type_names)?);
            format!("fn{}${}", params.len(), parts.join("$"))
        }
        // Spelled exactly as the source writes it, because MIR reaches the same
        // layout from a type *string* — `Wrap<i64?>` there splits into the
        // argument `i64?`, and the two have to agree on the key (#872).
        Type::Result { ok, err } if **err == Type::None => {
            format!("{}?", type_arg_key(ok, type_names)?)
        }
        Type::Result { ok, err } => format!(
            "{} or {}",
            type_arg_key(ok, type_names)?,
            type_arg_key(err, type_names)?,
        ),
        _ => return None,
    })
}

/// The one size model: what a type occupies in memory, laid out by the rules
/// above. The copy rule (mem.value/VS1), `@small`, `reflect.size_of` and the
/// `transmute` size test all read it, and codegen lays values out by the same
/// functions, so a type has one size everywhere.
impl crate::TypeTable {
    /// Record a struct, enum or union declaration for laying out.
    pub(crate) fn note_layout_decl(&mut self, id: crate::TypeId, kind: rask_ast::decl::DeclKind) {
        // A stdlib generic with no fields (`Sequence<T>`) stands in for a
        // value the runtime holds: one word, whatever its declaration says.
        let stand_in = self.stdlib_mode
            && matches!(&kind, rask_ast::decl::DeclKind::Struct(s)
                if s.fields.is_empty() && !crate::struct_type_param_names(s).is_empty());
        if stand_in {
            self.layout_stand_ins.insert(self.type_name(id));
            return;
        }
        let decl = Decl { id: rask_ast::NodeId(0), kind, span: rask_ast::Span::new(0, 0) };
        self.layout_decls.insert(self.type_name(id), decl);
    }

    /// Size and alignment of `ty`, or `None` for a type with no layout yet: one
    /// naming a type parameter or an inference variable nobody solved.
    pub fn size_align(&self, ty: &Type) -> Option<(u32, u32)> {
        let spelled = layout_spelling(ty, self)?;
        let mut cache = LayoutCache::new();
        self.fill_layouts(&spelled, &mut cache, &mut Vec::new());
        known_size_align(&spelled, &cache)
    }

    /// `size_align` for the copy rule, which has to answer inside a generic
    /// body too: a type parameter counts as a word there.
    pub fn copy_size(&self, ty: &Type) -> usize {
        self.size_align_spelled(&spell_lenient(ty, self)).0 as usize
    }

    /// Size and alignment of a type already spelled by name, as a declaration
    /// field is.
    pub fn size_align_spelled(&self, spelled: &Type) -> (u32, u32) {
        let mut cache = LayoutCache::new();
        self.fill_layouts(spelled, &mut cache, &mut Vec::new());
        type_size_align(spelled, &cache)
    }

    /// The layout of the struct `name`, with these type arguments.
    pub fn struct_layout_named(&self, name: &str, args: &[Type]) -> Option<StructLayout> {
        let decl = self.layout_decls.get(name)?;
        let rask_ast::decl::DeclKind::Struct(_) = &decl.kind else { return None };
        let mut cache = LayoutCache::new();
        for ty in args {
            self.fill_layouts(ty, &mut cache, &mut Vec::new());
        }
        self.fill_decl_fields(decl, args, &mut cache, &mut vec![name.to_string()]);
        Some(compute_struct_layout(decl, args, &cache))
    }

    /// The field of struct `ty` that runs past byte `limit` first, in memory
    /// order: its name, its size, and its type. What a `@small` error names.
    pub fn first_field_past(&self, ty: &Type, limit: u32) -> Option<FieldLayout> {
        let (Type::UnresolvedNamed(name) | Type::UnresolvedGeneric { name, .. }) = layout_spelling(ty, self)? else {
            return None;
        };
        let args: Vec<Type> = match layout_spelling(ty, self)? {
            Type::UnresolvedGeneric { args, .. } => args
                .into_iter()
                .filter_map(|a| match a {
                    crate::GenericArg::Type(t) => Some(*t),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        let mut layout = self.struct_layout_named(&name, &args)?;
        layout.fields.sort_by_key(|f| f.offset);
        layout.fields.into_iter().find(|f| f.offset + f.size > limit)
    }

    /// Put the layout of every declared type `ty` holds by value into `cache`,
    /// innermost first, so `type_size_align` finds each by name.
    fn fill_layouts(&self, ty: &Type, cache: &mut LayoutCache, visiting: &mut Vec<String>) {
        match ty {
            Type::UnresolvedNamed(name) => self.fill_named(name, &[], cache, visiting),
            Type::UnresolvedGeneric { name, args } => {
                // A handle's arguments live behind its pointer.
                if generic_is_one_word(name) || rask_stdlib::is_runtime_handle(name) {
                    return;
                }
                let args: Vec<Type> = args
                    .iter()
                    .filter_map(|a| match a {
                        crate::GenericArg::Type(t) => Some((**t).clone()),
                        _ => None,
                    })
                    .collect();
                for arg in &args {
                    self.fill_layouts(arg, cache, visiting);
                }
                self.fill_named(name, &args, cache, visiting);
            }
            Type::Tuple(elems) | Type::Union(elems) => {
                for e in elems {
                    self.fill_layouts(e, cache, visiting);
                }
            }
            Type::Array { elem, .. } | Type::SimdVector { elem, .. } => self.fill_layouts(elem, cache, visiting),
            Type::Result { ok, err } => {
                self.fill_layouts(ok, cache, visiting);
                self.fill_layouts(err, cache, visiting);
            }
            _ => {}
        }
    }

    fn fill_named(&self, name: &str, args: &[Type], cache: &mut LayoutCache, visiting: &mut Vec<String>) {
        let key = generic_instance_name(name, args, &HashMap::new()).unwrap_or_else(|| name.to_string());
        if cache.contains_key(&key) || visiting.contains(&key) {
            return;
        }
        if self.layout_stand_ins.contains(name) {
            cache.insert(key, (8, 8));
            return;
        }
        let Some(decl) = self.layout_decls.get(name) else {
            // The compiler registers `Ordering` without a declaration.
            let ordering = ordering_layout();
            if name == ordering.name {
                cache.insert(key, (ordering.size, ordering.align));
                return;
            }
            // A nominal newtype is laid out as what it wraps.
            if let Some(crate::TypeDef::NominalAlias { underlying, .. }) =
                self.get_type_id(name).and_then(|id| self.get(id))
            {
                let wrapped = spell_lenient(underlying, self);
                visiting.push(key.clone());
                self.fill_layouts(&wrapped, cache, visiting);
                visiting.pop();
                let size_align = type_size_align(&wrapped, cache);
                cache.insert(key, size_align);
            }
            return;
        };
        visiting.push(key.clone());
        self.fill_decl_fields(decl, args, cache, visiting);
        visiting.pop();
        use rask_ast::decl::DeclKind;
        let generic = match &decl.kind {
            DeclKind::Struct(s) => !crate::struct_type_param_names(s).is_empty(),
            DeclKind::Enum(e) => !crate::enum_type_param_names(e).is_empty(),
            _ => false,
        };
        let size_align = match &decl.kind {
            DeclKind::Struct(_) if generic && args.is_empty() => {
                let l = compute_shared_struct_layout(decl, cache);
                (l.size, l.align)
            }
            DeclKind::Struct(_) => {
                let l = compute_struct_layout(decl, args, cache);
                (l.size, l.align)
            }
            DeclKind::Enum(_) if generic && args.is_empty() => {
                let l = compute_shared_enum_layout(decl, cache);
                (l.size, l.align)
            }
            DeclKind::Enum(_) => {
                let l = compute_enum_layout(decl, args, cache);
                (l.size, l.align)
            }
            DeclKind::Union(_) => {
                let l = compute_union_layout(decl, cache);
                (l.size, l.align)
            }
            _ => return,
        };
        cache.insert(key, size_align);
    }

    /// The layouts a declaration's fields need, with `args` written in.
    fn fill_decl_fields(&self, decl: &Decl, args: &[Type], cache: &mut LayoutCache, visiting: &mut Vec<String>) {
        use rask_ast::decl::DeclKind;
        let (params, fields): (Vec<String>, Vec<&TypeExpr>) = match &decl.kind {
            DeclKind::Struct(s) => (crate::struct_type_param_names(s), s.fields.iter().map(|f| &f.ty).collect()),
            DeclKind::Enum(e) => (
                crate::enum_type_param_names(e),
                e.variants.iter().flat_map(|v| v.fields.iter().map(|f| &f.ty)).collect(),
            ),
            DeclKind::Union(u) => (Vec::new(), u.fields.iter().map(|f| &f.ty).collect()),
            _ => return,
        };
        let subst: HashMap<&str, &Type> = params.iter().map(String::as_str).zip(args.iter()).collect();
        for field in fields {
            let ty = substitute_inside(&field_type(field), &subst);
            self.fill_layouts(&ty, cache, visiting);
        }
    }
}

// ─── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{Decl, DeclKind, EnumDecl, Field, FieldVisibility, StructDecl, Variant};
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
                        ty: rask_parser::parse_type(ty).unwrap(),
                        visibility: FieldVisibility::Package,
                        attrs: vec![],
                        default: None,
                        doc: None,
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
                        name_span: dummy_span(),
                        fields: field_tys
                            .into_iter()
                            .enumerate()
                            .map(|(i, ty)| Field {
                                name: format!("f{}", i),
                                name_span: dummy_span(),
                                ty: rask_parser::parse_type(ty).unwrap(),
                                visibility: FieldVisibility::Package,
                                attrs: vec![],
                                default: None,
                                doc: None,
                            })
                            .collect(),
                        attrs: vec![],
                        discriminant: None,
                    })
                    .collect(),
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
                backing_type: None,
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
    fn bare_generic_container_name_is_pointer_sized() {
        // `type Counts = Map` stores the alias target as the raw string "Map",
        // so it reaches type_size_align as UnresolvedNamed rather than
        // UnresolvedGeneric. It must resolve like the generic form does (#545),
        // not fall into the unknown-type warning path.
        assert_eq!(tsa(&Type::UnresolvedNamed("Map".to_string())), (8, 8));
        assert_eq!(tsa(&Type::UnresolvedNamed("Vec".to_string())), (8, 8));
        assert_eq!(tsa(&Type::UnresolvedNamed("Mutex".to_string())), (8, 8));
    }

    #[test]
    fn user_type_named_like_a_builtin_container_uses_its_own_cached_size() {
        // A user struct can share a name with a builtin container (e.g. `Wide`,
        // also the name of the SIMD-vector generic). Its real cached layout
        // must win over the builtin bare-name guess above.
        let mut cache = LayoutCache::new();
        cache.insert("Wide".to_string(), (24, 8));
        assert_eq!(type_size_align(&Type::UnresolvedNamed("Wide".to_string()), &cache), (24, 8));
    }

    #[test]
    fn primitive_sizes() {
        // A scalar is as wide as it says it is — the same answer
        // `MirType::size` has always given (#1083).
        assert_eq!(tsa(&Type::Bool), (1, 1));
        assert_eq!(tsa(&Type::I8), (1, 1));
        assert_eq!(tsa(&Type::U8), (1, 1));
        assert_eq!(tsa(&Type::I16), (2, 2));
        assert_eq!(tsa(&Type::U16), (2, 2));
        assert_eq!(tsa(&Type::I32), (4, 4));
        assert_eq!(tsa(&Type::U32), (4, 4));
        assert_eq!(tsa(&Type::F32), (4, 4));
        assert_eq!(tsa(&Type::I64), (8, 8));
        assert_eq!(tsa(&Type::U64), (8, 8));
        assert_eq!(tsa(&Type::F64), (8, 8));
        assert_eq!(tsa(&Type::Char), (4, 4));
    }

    #[test]
    fn string_is_sso_inline() {
        let (size, align) = tsa(&Type::String);
        assert_eq!(size, 16); // 16-byte SSO inline (RaskStr union)
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
        // [tag:1][pad:3][i32:4]
        let (size, align) = tsa(&Type::option(Type::I32));
        assert_eq!(align, 4);
        assert_eq!(size, 8);
    }

    #[test]
    fn option_i8_layout() {
        // [tag:1][i8:1]
        let (size, align) = tsa(&Type::option(Type::I8));
        assert_eq!(align, 1);
        assert_eq!(size, 2);
    }

    #[test]
    fn option_link_niche_optimized() {
        // `Link<T>?` uses null for `none` — same size as a link, no tag
        let link_ty = Type::UnresolvedGeneric {
            name: "Link".to_string(),
            args: vec![crate::GenericArg::Type(Box::new(Type::I32))],
        };
        let (size, align) = tsa(&Type::option(link_ty));
        assert_eq!(size, 8);
        assert_eq!(align, 8);
    }

    #[test]
    fn link_size() {
        // `Link<T>` is the node's address — 8 bytes, nothing else
        let link_ty = Type::UnresolvedGeneric {
            name: "Link".to_string(),
            args: vec![crate::GenericArg::Type(Box::new(Type::I32))],
        };
        let (size, align) = tsa(&link_ty);
        assert_eq!(size, 8);
        assert_eq!(align, 8);
    }

    #[test]
    fn result_i32_i64_layout() {
        // [tag:1][pad:3][origin_line:4][origin_file:8][payload:8]
        let (size, align) = tsa(&Type::Result {
            ok: Box::new(Type::I32),
            err: Box::new(Type::I64),
        });
        assert_eq!(align, 8);
        assert_eq!(size, 24);
    }

    #[test]
    fn result_same_types() {
        let (size, align) = tsa(&Type::Result {
            ok: Box::new(Type::I32),
            err: Box::new(Type::I32),
        });
        // The 4-byte payload at 16, rounded up to the origin pointer's 8.
        assert_eq!(align, 8);
        assert_eq!(size, 24);
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
        let (size, align) = tsa(&Type::Tuple(vec![Type::I8, Type::I8]));
        assert_eq!(align, 1);
        assert_eq!(size, 2);
    }

    #[test]
    fn array_layout() {
        // [i32; 5] → 4 * 5
        let (size, align) = tsa(&Type::Array {
            elem: Box::new(Type::I32),
            len: 5,
        });
        assert_eq!(size, 20);
        assert_eq!(align, 4);
    }

    #[test]
    fn fn_pointer_size() {
        let (size, align) = tsa(&Type::Fn {
            params: vec![crate::FnParam::borrowed(Type::I32)],
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
        assert_eq!(layout.size, 4);
        assert_eq!(layout.align, 4);
        assert_eq!(layout.fields.len(), 1);
        assert_eq!(layout.fields[0].name, "x");
        assert_eq!(layout.fields[0].offset, 0);
        assert_eq!(layout.fields[0].size, 4);
    }

    #[test]
    fn two_field_struct() {
        let decl = make_struct("Point", vec![("x", "i32"), ("y", "i32")]);
        let layout = compute_struct_layout(&decl, &[], &empty_cache());
        assert_eq!(layout.size, 8);
        assert_eq!(layout.align, 4);
        assert_eq!(layout.fields[0].offset, 0);
        assert_eq!(layout.fields[1].offset, 4);
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
        // No payload: the one-byte tag is the whole value.
        assert_eq!(layout.size, 1);
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

        // Each field at its own width.
        assert_eq!(layout.variants[0].payload_size, 4);
        assert_eq!(layout.variants[1].payload_size, 8);

        // All variants share the same payload_offset, aligned for an i32.
        assert_eq!(layout.variants[0].payload_offset, layout.variants[1].payload_offset);
        assert_eq!(layout.variants[0].payload_offset, 4);

        // Tag (1) + padding (3) + max payload (8).
        assert_eq!(layout.size, 12);
        assert_eq!(layout.align, 4);
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
        assert_eq!(layout.variants[1].payload_size, 4);
        assert_eq!(layout.variants[2].payload_size, 8);

        // Tag (1) + padding (3) + max payload (8).
        assert_eq!(layout.size, 12);
    }

    // ── Field reordering (S1/L4) ──────────────────────────────────

    fn make_struct_with_attrs(name: &str, fields: Vec<(&str, &str)>, attrs: Vec<rask_ast::attr::Attr>) -> Decl {
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
                        ty: rask_parser::parse_type(ty).unwrap(),
                        visibility: FieldVisibility::Package,
                        attrs: vec![],
                        default: None,
                        doc: None,
                    })
                    .collect(),
                methods: vec![],
                is_pub: false,
                attrs,
                doc: None,
            }),
            span: dummy_span(),
        }
    }

    #[test]
    fn field_reorder_largest_alignment_first() {
        // Source: a: u8, b: u64, c: u16. S1/L4 sorts by alignment, largest
        // first, so the word comes before the halfword before the byte. This
        // used to assert source order, because every scalar had the same
        // alignment and the stable sort never moved anything (#1083).
        let decl = make_struct("Mixed", vec![("a", "u8"), ("b", "u64"), ("c", "u16")]);
        let layout = compute_struct_layout(&decl, &[], &empty_cache());
        assert_eq!(layout.fields[0].name, "b");
        assert_eq!(layout.fields[1].name, "c");
        assert_eq!(layout.fields[2].name, "a");
        // Declaration order is still recorded — `reflect.fields<T>()` reports it.
        assert_eq!(layout.fields[0].decl_index, 1);
        assert_eq!(layout.fields[1].decl_index, 2);
        assert_eq!(layout.fields[2].decl_index, 0);
        // 8 + 2 + 1, padded to the struct's alignment.
        assert_eq!(layout.size, 16);
        assert_eq!(layout.align, 8);
    }

    #[test]
    fn field_reorder_with_different_alignments() {
        // Use cache to give types different alignments
        let mut cache = LayoutCache::new();
        cache.insert("Small".to_string(), (1, 1));   // 1-byte align
        cache.insert("Medium".to_string(), (4, 4));   // 4-byte align
        cache.insert("Large".to_string(), (16, 8));    // 8-byte align

        let decl = make_struct("Ordered", vec![
            ("s", "Small"), ("m", "Medium"), ("l", "Large"),
        ]);
        let layout = compute_struct_layout(&decl, &[], &cache);

        // Reordered: Large (align 8), Medium (align 4), Small (align 1)
        assert_eq!(layout.fields[0].name, "l");
        assert_eq!(layout.fields[1].name, "m");
        assert_eq!(layout.fields[2].name, "s");
    }

    #[test]
    fn field_reorder_reduces_padding() {
        let mut cache = LayoutCache::new();
        cache.insert("Small".to_string(), (1, 1));
        cache.insert("Big".to_string(), (8, 8));

        // Source order: Small, Big, Small → 1 + 7pad + 8 + 1 + 7pad = 24
        // Reordered:   Big, Small, Small → 8 + 1 + 1 + 6pad = 16 (if true sizes)
        // With current codegen (all 8-byte), reordering still sorts by alignment desc
        let decl = make_struct("Padded", vec![
            ("s1", "Small"), ("b", "Big"), ("s2", "Small"),
        ]);
        let layout = compute_struct_layout(&decl, &[], &cache);
        assert_eq!(layout.fields[0].name, "b");
        // s1, s2 have equal alignment — stable sort preserves relative order
        assert_eq!(layout.fields[1].name, "s1");
        assert_eq!(layout.fields[2].name, "s2");
    }

    #[test]
    fn c_layout_preserves_source_order() {
        let mut cache = LayoutCache::new();
        cache.insert("Small".to_string(), (1, 1));
        cache.insert("Big".to_string(), (8, 8));

        let decl = make_struct_with_attrs(
            "CStruct",
            vec![("s", "Small"), ("b", "Big")],
            vec![rask_ast::attr::Attr::synthetic(
                "layout",
                Some(vec![rask_ast::attr::AttrArg {
                    label: None,
                    value: rask_ast::attr::AttrValue::synthetic_name("C"),
                }]),
            )],
        );
        let layout = compute_struct_layout(&decl, &[], &cache);

        // @layout(C): source order preserved
        assert_eq!(layout.fields[0].name, "s");
        assert_eq!(layout.fields[1].name, "b");
    }

    #[test]
    fn empty_struct_reorder_noop() {
        let decl = make_struct("Empty", vec![]);
        let layout = compute_struct_layout(&decl, &[], &empty_cache());
        assert_eq!(layout.size, 0);
        assert_eq!(layout.fields.len(), 0);
    }

}
