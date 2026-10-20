// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Which methods a stdlib type or module has, by name, for "did you mean" and
//! the interpreter's drift tests, and which layer each lives in.
//!
//! A type declared in `stdlib/*.rk` answers from its declaration, the
//! primitives included (stdlib/int.rk, float.rk, char.rk). Only the compiler's
//! own types with no stdlib file — the SIMD vectors, `Atomic` — have lists
//! here.

/// Where a stdlib type or module lives in the compilation pipeline.
///
/// - `Runtime`: needs OS access — implemented in the C runtime
/// - `Pure`: no OS access — can be rewritten in Rask once codegen works
/// - `Hybrid`: mix of both (e.g., Duration is pure, Instant needs OS)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdlibLayer {
    Runtime,
    Pure,
    Hybrid,
}

/// Classify a builtin type by its runtime requirements.
pub fn type_layer(type_name: &str) -> StdlibLayer {
    match type_name {
        "i8" | "i16" | "i32" | "i64" | "i128"
        | "u8" | "u16" | "u32" | "u64" | "u128"
        | "f64" | "bool" | "char" | "string"
        | "Vec" | "Map" | "Rack" | "Link"
        | "Result" | "Option"
        | "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8"
        | "JsonValue" | "Path" | "Args" | "Duration" => StdlibLayer::Pure,

        "Handle" | "Sender" | "Receiver" | "Shared" | "Atomic"
        | "File" | "Metadata"
        | "TcpListener" | "TcpConnection"
        | "Instant" => StdlibLayer::Runtime,

        _ => StdlibLayer::Runtime,
    }
}

/// Classify a stdlib module by its runtime requirements.
pub fn module_layer(module: &str) -> StdlibLayer {
    match module {
        "json" | "math" | "path" | "num" => StdlibLayer::Pure,
        "fs" | "io" | "net" | "os" | "cli" => StdlibLayer::Runtime,
        "time" | "random" => StdlibLayer::Hybrid,
        _ => StdlibLayer::Runtime,
    }
}

// ---------------------------------------------------------------------------
// Instance methods by type
// ---------------------------------------------------------------------------

/// A compiler type's methods, each with the argument counts it accepts.
///
/// Two lists — one of names, one of arities — is one list too many. This
/// expands to both from a single set of lines, so a method added to a resolver
/// can't reach the names and miss the counts, or the other way round.
///
/// A name can carry more than one count where it has more than one form. Where
/// it has exactly one, a call that misses it can be told how many it wanted;
/// where it has several, no single number is the answer and the diagnostic says
/// what didn't fit instead (#1150).
macro_rules! methods_with_arity {
    ($names:ident, $arities:ident, $($m:literal => [$($n:literal),*]),* $(,)?) => {
        const $names: &[&str] = &[$($m),*];
        const $arities: &[(&str, &[usize])] = &[$(($m, &[$($n),*])),*];
    };
}

const SIMD_METHODS: &[&str] = &[
    "splat", "load", "store",
    "add", "sub", "mul", "div", "scale",
    "sum", "product", "min", "max",
    "get", "set",
];

// mem.atomics: one type, so one list. Which of these a given payload actually
// gets is GA3's business — a `bool` has no `fetch_add` — and the checker
// answers that from the payload rather than from a second list here.
//
// `new` and `default` are constructors — they answer on the type, not on a
// value, the same way `Shared`'s do, so they aren't here.
methods_with_arity!(ATOMIC_METHODS, ATOMIC_ARITIES,
    "load" => [1],
    "store" => [2],
    "swap" => [2],
    "compare_exchange" => [4],
    "compare_exchange_weak" => [4],
    "fetch_add" => [2],
    "fetch_sub" => [2],
    "fetch_and" => [2],
    "fetch_or" => [2],
    "fetch_xor" => [2],
    "fetch_nand" => [2],
    "fetch_max" => [2],
    "fetch_min" => [2],
    "take" => [0],
);

// ---------------------------------------------------------------------------
// Lookup
// ---------------------------------------------------------------------------

/// All types with registered instance methods.
pub const REGISTERED_TYPES: &[&str] = &[
    "i8", "i16", "i32", "i64", "i128",
    "u8", "u16", "u32", "u64", "u128",
    "f64", "bool", "char", "string",
    "Vec", "Map", "Rack", "Link",
    "File", "Metadata",
    "TcpListener", "TcpConnection",
    "JsonValue",
    "Duration", "Instant",
    "Path", "Args",
    "Handle", "Sender", "Receiver", "Shared",
    "Atomic",
    "f32x4", "f32x8", "f64x2", "f64x4", "i32x4", "i32x8",
];

/// All modules with registered functions.
pub const REGISTERED_MODULES: &[&str] = &[
    "fs", "net", "json", "time", "math", "random", "os", "io", "cli",
];

/// Types whose methods are written in Rask, in `stdlib/*.rk`, rather than
/// natively in each backend.
///
/// Both backends run that source — native through `compilable_decls`, the
/// interpreter through the same — so there is one implementation and it can't
/// disagree with itself. A Rust implementation in `rask-interp/src/stdlib/` for
/// one of these would be a *second* one, which is what the drift test checks.
///
/// `Path` was the first: 46 lines of declarations, 192 lines of C and 184 lines
/// of Rust, for pure string manipulation that the two backends got different
/// answers from (#688).
///
/// The HTTP types followed: the interpreter answered them from Rust, and the
/// two drifted three times in one week — native lost the url, the
/// interpreter never closed a connection, native dropped response headers
/// (#1378).
pub const RASK_IMPLEMENTED_TYPES: &[&str] = &[
    "Path", "Handles", "Wide",
    "Request", "Response", "Headers", "Method", "HttpServer", "Responder", "HttpClient",
];

/// True when this type's methods live in Rask rather than in the backends.
pub fn is_rask_implemented(type_name: &str) -> bool {
    RASK_IMPLEMENTED_TYPES.contains(&type_name)
}

/// The methods a program can call on `type_name`: the public, implemented
/// ones its declaration in `stdlib/*.rk` has. A hand list per type drifted from
/// the declarations, so "did you mean" offered methods that didn't exist.
///
/// The SIMD vectors and `Atomic` have no declaration — they're the
/// compiler's own — so they answer from the lists above.
pub fn type_method_names(type_name: &str) -> Vec<&'static str> {
    let compiler_type: &[&str] = match type_name {
        "Atomic" => ATOMIC_METHODS,
        "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8" => SIMD_METHODS,
        _ => return declared_method_names(type_name),
    };
    compiler_type.to_vec()
}

/// What a program can call on a stdlib type or module, from its declaration.
/// A type declaring `as_sequence` also has `Sequence`'s methods
/// (type.sequence/SEQ49): `v.filter(p)` is a `Vec` call.
fn declared_method_names(name: &str) -> Vec<&'static str> {
    let reg = crate::StubRegistry::load();
    let callable = |type_name: &str| -> Vec<&'static str> {
        reg.methods(type_name)
            .iter()
            .filter(|m| m.is_pub && !m.unimplemented)
            .map(|m| m.name.as_str())
            .collect()
    };
    let mut names = callable(name);
    if names.contains(&"as_sequence") {
        for m in callable("Sequence") {
            if !names.contains(&m) {
                names.push(m);
            }
        }
    }
    names
}

/// The argument counts `type_name.method` accepts, where nothing else knows.
///
/// Almost every builtin declares its methods in `stdlib/*.rk`, so a wrong count
/// is caught against the declared signature and the error names the number —
/// that is why `v.len(3)` has always read correctly. `Atomic` has no stdlib
/// file at all: it is a compiler type, and its resolver is the only description
/// of it, so its arities live with its names here and the resolver asks this
/// before falling through to "no such method" (#1150).
///
/// `None` means nothing is known about the name, which is not the same as
/// "takes no arguments".
pub fn type_method_arity(type_name: &str, method: &str) -> Option<&'static [usize]> {
    let table = match type_name {
        "Atomic" => ATOMIC_ARITIES,
        _ => return None,
    };
    table.iter().find(|(m, _)| *m == method).map(|(_, counts)| *counts)
}

/// The functions a program can call in a stdlib module, from its declaration.
pub fn module_method_names(module: &str) -> Vec<&'static str> {
    declared_method_names(module)
}

/// Types that exist only for codegen — the interpreter doesn't dispatch them.
/// Drift tests should skip these.
pub fn is_codegen_only_type(type_name: &str) -> bool {
    matches!(type_name,
        "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8"
    )
}

/// Check if a type has a registered method.
pub fn has_type_method(type_name: &str, method: &str) -> bool {
    type_method_names(type_name).contains(&method)
}

/// Check if a module has a registered function.
pub fn has_module_method(module: &str, method: &str) -> bool {
    module_method_names(module).contains(&method)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names come from the declarations, so a method the language dropped
    /// isn't offered back. The old hand list still had `string.push_str`, so
    /// a typo near it was told to call a method strings no longer have.
    #[test]
    fn names_are_what_the_declaration_has() {
        let string = type_method_names("string");
        assert!(string.contains(&"to_lowercase"));
        assert!(!string.contains(&"push_str"));
    }

    /// A chain head carries the sequence surface (type.sequence/SEQ49).
    #[test]
    fn a_chain_head_has_the_sequence_methods() {
        let vec = type_method_names("Vec");
        assert!(vec.contains(&"push"));
        assert!(vec.contains(&"filter"));
        assert!(vec.contains(&"take_while"));
    }

    #[test]
    fn compiler_types_answer_from_their_lists() {
        assert!(type_method_names("Atomic").contains(&"fetch_add"));
    }

    /// Every integer width has the methods stdlib/int.rk writes once for the
    /// family, and every float width float.rk's.
    #[test]
    fn primitive_widths_answer_from_their_family_block() {
        for width in ["i8", "i32", "u8", "u64", "i128", "u128"] {
            let names = type_method_names(width);
            assert!(names.contains(&"count_ones"), "{width}: {names:?}");
            let narrow = !width.ends_with("128");
            assert_eq!(names.contains(&"checked_add"), narrow, "{width}: {names:?}");
        }
        for width in ["f32", "f64"] {
            let names = type_method_names(width);
            assert!(names.contains(&"floor"), "{width}: {names:?}");
            assert!(names.contains(&"to_bits"), "{width}: {names:?}");
        }
    }
}
