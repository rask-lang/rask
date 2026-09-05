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
    /// Declared in the stdlib rather than in the program.
    ///
    /// Layouts live in one flat `Vec` looked up by bare name, so a program's
    /// `struct Timer` and `stdlib/time.rk`'s both answer to `Timer` and the
    /// first one wins. The stdlib's is `public struct Timer { }` — no fields —
    /// so every field of the user's landed at offset 0 and the literal
    /// segfaulted (#975). `find_struct` prefers the program's when both exist,
    /// which is the same rule the checker's `type_names` /
    /// `stdlib_type_names` split already applies to types (#515).
    pub is_stdlib: bool,
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
    pub attrs: Vec<String>,
    /// Whether the field has a declared default value (`x: T = v`). Separate
    /// from `@default(...)` in `attrs`, which is a decode-only override
    /// (`type.structs/FD6`) — `reflect.fields<T>()`'s `has_default` is true
    /// for either.
    pub has_declared_default: bool,
    /// The declared default's literal text, when it is one (`port: i32 = 8080`
    /// → `"8080"`). Same shape as the `@default(…)` argument in `attrs`, so the
    /// decoder can treat the two the same: a field the input leaves out takes
    /// its declared default (`type.structs/FD6`). `None` for a default that
    /// isn't a plain literal, which stays construction-only.
    pub declared_default: Option<String>,
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
    pub attrs: Vec<String>,
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
    pub attrs: Vec<String>,
}

/// The size and alignment of a scalar, by its type. Panics on anything else.
fn scalar_by_name(ty: &Type) -> (u32, u32) {
    let n = ty.scalar_bytes().expect("a scalar knows its width");
    (n, n)
}

/// Get size and alignment for a type (after monomorphization).
/// `cache` maps type names to already-computed (size, align) for user-defined types.
pub fn type_size_align(ty: &Type, cache: &LayoutCache) -> (u32, u32) {
    match ty {
        // A struct field holds its declared width. Everything that reads one
        // reads it at that width — `FieldLayout.size` is what says so, and
        // `slot_scalar_bytes` turns it into a load.
        //
        // The aggregate *slots* below don't shrink with it: an `Option`
        // payload, an enum tag and an enum payload field are word slots by
        // codegen's convention, and each says so where it computes its own
        // layout. That used to fall out of every scalar being eight bytes, so
        // none of them had to state it (#1083).
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
        Type::Slice(_) => (16, 8), // Fat pointer: ptr + len
        ty if ty.is_option() => {
            let inner = ty.as_option().unwrap();
            // Niche optimization: Option<Handle<T>> uses sentinel value instead of tag.
            if matches!(inner, Type::UnresolvedGeneric { name, .. }
                if name == "Handle" || name == "Link")
            {
                return (8, 8);
            }
            let (size, align) = type_size_align(inner, cache);
            // The payload sits in a word slot, the same convention the `Result`
            // arm below states: codegen writes a scalar payload full-width, so
            // the slot is floored at a word however narrow the value is.
            let size = size.max(crate::abi::PAYLOAD_SLOT_BYTES);
            let align = align.max(crate::abi::PAYLOAD_SLOT_BYTES);
            let tag_size = 1u32;
            let payload_offset = align_up(tag_size, align);
            (payload_offset + size, align)
        }
        Type::Result { ok, err } => {
            let (ok_size, ok_align) = type_size_align(ok, cache);
            let (err_size, err_align) = type_size_align(err, cache);
            // Layout must match rask-codegen/rask-mir (ER15 origin fields):
            // [tag:8][origin_file:8][origin_line:8][payload:max(ok,err)]. Scalars
            // occupy 8-byte slots in codegen, so floor each side at 8.
            let max_size = ok_size.max(8).max(err_size.max(8));
            let max_align = ok_align.max(err_align).max(8);
            (align_up(crate::abi::RESULT_PAYLOAD_OFFSET + max_size, max_align), max_align)
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
        // A link is the node's address; a rack is a pointer to its slab.
        Type::UnresolvedGeneric { name, .. } if name == "Link" => (8, 8),
        Type::UnresolvedGeneric { name, .. } if name == "Rack" => (8, 8),
        Type::UnresolvedGeneric { name, .. } if name == "Vec" => (8, 8), // Opaque pointer (runtime uses RaskVec*)
        Type::UnresolvedGeneric { name, .. } if name == "Wide" => (8, 8), // Opaque pointer (runtime uses RaskVec* — conc.data-parallel)
        Type::UnresolvedGeneric { name, .. } if name == "Map" => (8, 8),  // Pointer to map
        Type::UnresolvedGeneric { name, .. } if name == "Random" => (8, 8),  // Pointer to rng state
        Type::UnresolvedGeneric { name, .. } if name == "Channel" => (8, 8),
        // Box family — all opaque runtime pointers, same as the collections
        // above. Without these a `Mutex<T>` field warned about an unresolved
        // generic on every build even though (8, 8) is the right answer.
        Type::UnresolvedGeneric { name, .. }
            if matches!(name.as_str(),
                "Mutex" | "Shared" | "Cell" | "Heap" | "Atomic"
                | "Sender" | "Receiver" | "TaskHandle") => (8, 8),
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
        // A field written `any Trait` reaches here as a name, not a parsed
        // TraitObject. It's still a fat pointer, and sizing it at 8 gave a
        // struct field half the room for one — the vtable half landed in
        // whatever followed (#474).
        Type::UnresolvedNamed(name) if name.starts_with("any ") => (16, 8),
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
            type_size_align(&Type::UnresolvedNamed(spelling.to_string()), cache)
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
                // Stdlib types backed by opaque runtime pointers
                "TcpListener" | "TcpConnection" | "File" | "ThreadHandle"
                | "TaskHandle" | "Sender" | "Receiver" | "ThreadPool"
                | "MultitaskingRuntime" | "Random" | "Iterator" | "StringBuilder" => (8, 8),
                // A word, but not for the reason the line above is: these two
                // are an `int64_t` of nanoseconds in the runtime, not a pointer
                // to anything. Their Rask declarations are empty structs, so
                // without an entry here the layout cache answered with the
                // declaration's size — zero — and a struct holding one gave the
                // field no room at all (#924).
                "Duration" | "Instant" => (8, 8),
                _ => {
                    // Look up user-defined types from the layout cache first — a user
                    // struct can be named the same as a builtin container (e.g. `Wide`),
                    // and its real, cached size must win over the builtin guess below.
                    //
                    // Except a zero. A container's stdlib declaration is an empty
                    // struct standing in for an opaque runtime pointer, so the
                    // cache reports `Vec` as nothing at all — and a `Vec<i64>?`
                    // type argument, whose name loses its `<i64>` on the way here,
                    // sized as one byte of tag with no payload. The instance
                    // layout then came out smaller than the shared one, was
                    // dropped as pointless, and the struct literal wrapped its
                    // field against the shared layout's placeholder: a bare `Vec`
                    // stored where a `Vec?` goes, tag never written, and the read
                    // came back `none` (#1081). No user struct is zero-sized *and*
                    // named after a container, so this costs nothing.
                    let cached_size = cache
                        .get(name.as_str())
                        .filter(|(size, _)| *size > 0 || !is_opaque_container_name(name));
                    if let Some(&cached) = cached_size {
                        cached
                    } else if is_typevar_name(name) {
                        // Unsubstituted type parameter — pointer-sized fallback,
                        // silent because mono always picks a concrete size on real call sites.
                        (8, 8)
                    } else if matches!(name.as_str(),
                        // Generic containers/boxes written without their type args (e.g. a
                        // type alias target like `type Counts = Map`) arrive here as a bare
                        // name instead of `UnresolvedGeneric` — same opaque-pointer types as
                        // the `UnresolvedGeneric` arm above, just missing their `<...>`.
                        "Vec" | "Wide" | "Map" | "Handle" | "Pool"
                        | "Mutex" | "Shared" | "Cell" | "Heap" | "Atomic" | "Channel") {
                        (8, 8)
                    } else {
                        // Treat as opaque pointer-sized. If this is a user type,
                        // it should have been caught by the type checker.
                        eprintln!(
                            "warning: unknown type '{}' in layout, defaulting to pointer size (8, 8)",
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
        Type::None => (0, 1),
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

/// Parse a field type string (from AST) to a Type for layout computation.
pub(crate) fn parse_field_type(s: &str) -> Type {
    // `d: time.Duration` on a field. Left dotted it fell through to the unknown
    // name at the bottom of `type_size_align`, and the field got pointer-sized
    // room by default — right for `Duration` by luck, and for anything wider a
    // hard "field holds 32 bytes but its slot is 8" telling the user to report a
    // compiler bug.
    //
    // Whatever a field's type is reached *through* says nothing about its size,
    // so the last segment is the whole question here. The checker's
    // `strip_module_qualifier` is narrower on purpose — it drops the head only
    // when it names a real module, because there a wrong strip changes which type
    // resolves (`c.Rect` is the C namespace's, and bare `Rect` is nobody's, #948).
    // Layout has no such worry: the fallback it replaces was an outright guess.
    // Being broader is also what makes an *aliased* import work — `import http as h`
    // binds the module under a name no module list knows, and `h.Response` on a
    // field hit exactly that hard error.
    let s = rask_ast::type_str::bare_name(s);

    // Option shorthand: T? → Option<T>
    if s.ends_with('?') {
        let inner = parse_field_type(&s[..s.len() - 1]);
        return Type::option(inner);
    }

    // Result type: "T or E"
    if let Some(idx) = s.find(" or ") {
        let ok = parse_field_type(&s[..idx]);
        let err = parse_field_type(&s[idx + 4..]);
        return Type::Result {
            ok: Box::new(ok),
            err: Box::new(err),
        };
    }

    // Slice: []T
    if let Some(elem) = s.strip_prefix("[]") {
        return Type::Slice(Box::new(parse_field_type(elem)));
    }

    // Fixed array `[T; N]`, and `[T]` for a slice written the other way. Without
    // this the whole bracket form fell through to the unknown-name branch below
    // and every fixed array field was sized as a pointer (#895).
    if let Some(inner) = s.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        match inner.split_once(';') {
            Some((elem, len)) => {
                // A symbolic length (`[T; SIZE]`) has no value here — there is no
                // const table in this pass. Falling through to the unknown-name
                // branch keeps its warning rather than sizing the field at zero,
                // which is what pretending the length is 0 would do (#906).
                if let Ok(len) = len.trim().parse::<usize>() {
                    return Type::Array {
                        elem: Box::new(parse_field_type(elem)),
                        len,
                    };
                }
            }
            None => return Type::Slice(Box::new(parse_field_type(inner))),
        }
    }

    // Generic types: Name<Args>
    if let Some(angle) = s.find('<') {
        if s.ends_with('>') {
            let name = &s[..angle];
            let inner = &s[angle + 1..s.len() - 1];

            // Option<T> → T or none
            if name == "Option" {
                return Type::option(parse_field_type(inner));
            }

            // Result<T, E> — the checker sometimes normalizes `T or E` to this
            // string form. Without this it falls through to UnresolvedGeneric and
            // gets mis-sized as a pointer.
            if name == "Result" {
                let parts = split_type_args(inner);
                if parts.len() == 2 {
                    return Type::Result {
                        ok: Box::new(parse_field_type(parts[0])),
                        err: Box::new(parse_field_type(parts[1])),
                    };
                }
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

/// Parse a field type string and apply generic substitution.
/// If the parsed type is an unresolved name that matches a type parameter,
/// replace it with the concrete type from type_args.
///
/// The flag says the field was declared with one of the type's parameters, so the
/// returned type is a substitution rather than what the source wrote — see
/// `FieldLayout::is_type_param`.
fn resolve_field_type(
    field_ty_str: &str,
    subst: &std::collections::HashMap<&str, &Type>,
) -> (Type, bool) {
    let parsed = parse_field_type(field_ty_str);
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

/// A builtin container or box, written without its type arguments.
///
/// Each is an opaque runtime pointer whose Rask declaration is an empty struct,
/// so the layout cache holds a zero for it that isn't the truth.
fn is_opaque_container_name(name: &str) -> bool {
    matches!(
        name,
        "Vec" | "Wide" | "Map" | "Set" | "Handle" | "Pool" | "Rack" | "Link"
            | "Mutex" | "Shared" | "Cell" | "Heap" | "Atomic" | "Channel"
            | "Sender" | "Receiver"
    )
}

/// Check whether a struct has `@layout(C)` attribute.
fn has_c_layout(attrs: &[String]) -> bool {
    attrs.iter().any(|a| a == "layout(C)")
}

/// A declared default's literal text, for the decoder.
///
/// `type.structs/FD1` limits a declared default to a compile-time constant, and
/// the decoder needs it in the same shape `@default(…)` already arrives in:
/// verbatim source text. Anything that isn't a plain literal answers `None` and
/// stays construction-only rather than being guessed at.
fn literal_text(e: &rask_ast::expr::Expr) -> Option<String> {
    use rask_ast::expr::{ExprKind, UnaryOp};
    match &e.kind {
        ExprKind::Int(n, _) => Some(n.to_string()),
        ExprKind::Float(f, _) => Some(f.to_string()),
        ExprKind::Bool(b) => Some(b.to_string()),
        ExprKind::String(s) => Some(format!("{:?}", s)),
        ExprKind::Char(c) => Some(format!("'{}'", c)),
        // `-1` is a negation over a literal by the time it gets here.
        ExprKind::Unary { op: UnaryOp::Neg, operand } => {
            literal_text(operand).map(|t| format!("-{t}"))
        }
        _ => None,
    }
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
    use rask_ast::decl::DeclKind;

    let struct_decl = match &struct_def.kind {
        DeclKind::Struct(s) => s,
        _ => panic!("Expected struct declaration"),
    };

    let param_names = rask_types::struct_type_param_names(struct_decl);
    let subst = build_subst(&param_names, type_args);
    let c_layout = has_c_layout(&struct_decl.attrs);
    // A `@binary` struct's declared field types are wire specifiers
    // (type.binary/F2), not type names — `u16be` says "16 bits, big-endian".
    // The in-memory layout is the runtime type each one stands for; reading the
    // specifier as a type name gave every field a pointer-sized slot and a
    // warning per compile.
    let is_binary = struct_decl.attrs.iter().any(|a| a == "binary");

    // Resolve types and compute sizes for all fields first
    let mut resolved: Vec<(String, Type, u32, u32, Vec<String>, bool, Option<String>, bool, bool, u32)> = struct_decl.fields.iter()
        .enumerate()
        .map(|(decl_index, field)| {
            let (field_ty, from_param) = if is_binary {
                match rask_types::binary_field_runtime_type(&field.ty) {
                    Some(ty) => (ty, false),
                    None => resolve_field_type(&field.ty, &subst),
                }
            } else {
                resolve_field_type(&field.ty, &subst)
            };
            let (field_size, field_align) = type_size_align(&field_ty, cache);
            (
                field.name.clone(),
                field_ty,
                field_size,
                field_align,
                field.attrs.clone(),
                field.default.is_some(),
                field.default.as_ref().and_then(literal_text),
                field.visibility.is_pub(),
                from_param,
                decl_index as u32,
            )
        })
        .collect();

    // S1/L4: Default @layout(Rask) reorders by alignment (largest first).
    // Stable sort preserves source order for fields with equal alignment.
    // S2: @layout(C) preserves source order for FFI.
    if !c_layout {
        resolved.sort_by(|a, b| b.3.cmp(&a.3));
    }

    let mut field_layouts = Vec::new();
    let mut offset = 0u32;
    let mut max_align = 1u32;

    for (name, ty, size, align, attrs, has_declared_default, declared_default, is_public, is_type_param, decl_index) in resolved {
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
            declared_default,
            is_public,
            decl_index,
            is_type_param,
        });

        offset += size;
    }

    // S4: Total size with tail padding to struct alignment
    let total_size = align_up(offset, max_align);

    StructLayout {
        name: struct_decl.name.clone(),
        size: total_size,
        align: max_align,
        fields: field_layouts,
        is_stdlib: is_stdlib_span(struct_def.span),
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
            attrs: field.attrs.clone(),
            has_declared_default: field.default.is_some(),
            declared_default: field.default.as_ref().and_then(literal_text),
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
        is_stdlib: is_stdlib_span(union_def.span),
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
/// sized through `type_size_align` rather than by hand, because a `u8` tag does
/// not occupy one byte here: codegen gives every scalar an 8-byte slot, so
/// `compute_enum_layout` gives a fieldless enum size 8, align 8. Written as
/// `size: 1` this layout was the only enum in the program whose stack slot was
/// narrower than the stores and loads aimed at it — `Ordering.Less` wrote eight
/// bytes into a one-byte slot, a returned `Ordering` was copied one byte out of
/// eight, and `==` read all eight back, so two equal values compared equal or
/// not depending on what was next to them on the stack. It moved when a test
/// was added above the one that used it.
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
    use rask_ast::decl::DeclKind;

    let enum_decl = match &enum_def.kind {
        DeclKind::Enum(e) => e,
        _ => panic!("Expected enum declaration"),
    };

    let param_names = rask_types::enum_type_param_names(enum_decl);
    let subst = build_subst(&param_names, type_args);

    let variant_count = enum_decl.variants.len();

    // E2/E14: Determine discriminant type
    let tag_ty = if let Some(ref bt) = enum_decl.backing_type {
        match bt.as_str() {
            "u8" => Type::U8,
            "u16" => Type::U16,
            "u32" => Type::U32,
            "i8" => Type::I8,
            "i16" => Type::I16,
            "i32" => Type::I32,
            "i64" | "int" => Type::I64,
            _ => if variant_count <= 256 { Type::U8 } else { Type::U16 },
        }
    } else if variant_count <= 256 {
        Type::U8
    } else {
        Type::U16
    };
    // The tag and every payload field sit in word slots, the same convention
    // `Option` and `Result` state: codegen writes a scalar full-width and the
    // match paths read a word. Struct fields hold their real widths (#1083);
    // an enum's don't.
    let (tag_size, tag_align) = {
        let (s, a) = type_size_align(&tag_ty, cache);
        (s.max(crate::abi::PAYLOAD_SLOT_BYTES), a.max(crate::abi::PAYLOAD_SLOT_BYTES))
    };

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
                let (size, align) = type_size_align(&field_ty, cache);
                let size = size.max(crate::abi::PAYLOAD_SLOT_BYTES);
                let align = align.max(crate::abi::PAYLOAD_SLOT_BYTES);

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
                    declared_default: field.default.as_ref().and_then(literal_text),
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
                        ty: ty.to_string(),
                        visibility: FieldVisibility::Package,
                        attrs: vec![],
                        default: None,
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
                                ty: ty.to_string(),
                                visibility: FieldVisibility::Package,
                                attrs: vec![],
                                default: None,
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
        // `char` is a full word, not four bytes: the runtime carries a
        // code point in one.
        assert_eq!(tsa(&Type::Char), (8, 8));
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
        // tag (8 bytes) + i32 payload (8 bytes, codegen uses i64) = 16
        let (size, align) = tsa(&Type::option(Type::I32));
        assert_eq!(align, 8);
        assert_eq!(size, 16);
    }

    #[test]
    fn option_i8_layout() {
        // tag (8) + i8 payload (8, codegen uses i64) = 16
        let (size, align) = tsa(&Type::option(Type::I8));
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
        let (size, align) = tsa(&Type::option(handle_ty));
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
        // [tag:8][origin_file:8][origin_line:8][payload:8] — ER15 origin fields
        // (must match rask-codegen; see rask_mono::abi).
        let (size, align) = tsa(&Type::Result {
            ok: Box::new(Type::I32),
            err: Box::new(Type::I64),
        });
        assert_eq!(align, 8);
        assert_eq!(size, 32); // 24 (tag + origin) + 8 payload
    }

    #[test]
    fn result_same_types() {
        let (size, align) = tsa(&Type::Result {
            ok: Box::new(Type::I32),
            err: Box::new(Type::I32),
        });
        assert_eq!(align, 8);
        assert_eq!(size, 32); // 24 (tag + origin) + 8 payload
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

    // ── Field reordering (S1/L4) ──────────────────────────────────

    fn make_struct_with_attrs(name: &str, fields: Vec<(&str, &str)>, attrs: Vec<&str>) -> Decl {
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
                        visibility: FieldVisibility::Package,
                        attrs: vec![],
                        default: None,
                    })
                    .collect(),
                methods: vec![],
                is_pub: false,
                attrs: attrs.into_iter().map(|a| a.to_string()).collect(),
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
            vec!["layout(C)"],
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
