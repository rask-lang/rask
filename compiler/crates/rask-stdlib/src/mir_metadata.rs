// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! MIR-level metadata derived from stdlib stub files.
//!
//! Carries each stdlib function's return type verbatim, plus a coarse reading
//! of it for the two questions that aren't about layout. Keeps the stub files
//! as the single source of truth for stdlib API shapes.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::stubs::StubRegistry;

/// A coarse reading of a stub's return type.
///
/// No longer a type: MIR resolves `ret_ty` with `resolve_type_str`, the one
/// parser that knows `f32` from `f64`, transparent aliases, arrays, unions,
/// generic instantiations, and which named stdlib types are word-sized runtime
/// handles. This answers the two questions that aren't about layout —
/// `names_a_type_param` and `ret_type_prefix` — and nothing else reads it.
///
/// It used to answer the layout question too, and got two things wrong that no
/// caller could see: `f32` and `f64` shared a variant, and every named stdlib
/// type came back `i64`, which is right for a handle and wrong for anything
/// else (`StringView` needed a hard-coded exception). See #1025.
#[derive(Debug, Clone, PartialEq)]
pub enum RetCategory {
    Void,
    Bool,
    I64,
    /// An integer at a width other than 64-bit-signed, spelled as its Rask name.
    ///
    /// Every integer return used to collapse to `I64`. Mostly invisible, because
    /// most of them are lengths and counts — but a value that spans its range
    /// renders as the signed reading of its bits: `string.hash()` is `u64` and
    /// FNV-1a fills the range, so half of its answers printed negative (#823).
    Int(IntWidth),
    F64,
    String,
    /// A `char` — 4 bytes, not 8. Folding it into I64 gave `char?` an 8-byte
    /// payload slot while the rest of the compiler used 4, so a function
    /// forwarding `char_at`'s result read the payload from the wrong offset
    /// (#693).
    Char,
    Ptr,
    Option(Box<RetCategory>),
    Result {
        ok: Box<RetCategory>,
        err: Box<RetCategory>,
    },
    /// A named stdlib type (e.g., "File", "Vec", "Shared").
    Named(std::string::String),
    /// The stub's own type parameter — `Vec.remove` is declared `-> T`.
    ///
    /// This used to be recorded as `I64`, which is where "`T` reaches MIR as a
    /// bare i64" came from: a `Vec<string>.remove(i)` destination got an 8-byte
    /// slot, half the string was copied, and no refcount was taken on the
    /// payload. The stub genuinely doesn't know — only the call site does — so
    /// the honest answer is to say which parameter it is and let the caller ask
    /// something that knows (#1020).
    TypeParam(std::string::String),
    /// Tuple of types (e.g., `(Request, Responder)`).
    Tuple(Vec<RetCategory>),
}

impl RetCategory {
    /// Does this return type still mention one of the stub's type parameters?
    ///
    /// `Vec.remove` is `-> T` and `Vec.get` is `-> T?`; both are holes the call
    /// site has to fill. A caller that can't fill one is better off knowing that
    /// than being handed a width the stub invented.
    pub fn names_a_type_param(&self) -> bool {
        match self {
            RetCategory::TypeParam(_) => true,
            RetCategory::Option(inner) => inner.names_a_type_param(),
            RetCategory::Result { ok, err } => {
                ok.names_a_type_param() || err.names_a_type_param()
            }
            RetCategory::Tuple(elems) => elems.iter().any(|e| e.names_a_type_param()),
            _ => false,
        }
    }
}

/// An integer width other than the `I64` default, by its Rask name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntWidth {
    I8, I16, I32, I128,
    U8, U16, U32, U64, U128,
    /// `usize`/`isize` — pointer-sized, decided by `rask_ast::primitives`.
    Usize, Isize,
}

/// Metadata for a single stdlib method, derived from stubs.
#[derive(Debug, Clone)]
pub struct StdlibMethodMeta {
    /// Qualified name as MIR sees it: "Vec_push", "fs_open".
    pub qualified_name: std::string::String,
    /// Return type category. Down to one live question — does the stub's
    /// return type name one of the type's parameters? — now that MIR resolves
    /// the type string itself. See `ret_ty`.
    pub ret_category: RetCategory,
    /// The stub's return type, verbatim.
    ///
    /// MIR's `resolve_type_str` already parses these, and does it properly: it
    /// knows `f32` from `f64`, transparent aliases, arrays, unions, generic
    /// instantiations, and which named stdlib types are word-sized runtime
    /// handles. `RetCategory` was a second, coarser parser standing beside it
    /// (#1025).
    pub ret_ty: std::string::String,
    /// Type prefix of the return value (for local_type_prefix tracking).
    /// E.g., "fs_open" returns File → prefix "File".
    pub ret_type_prefix: Option<std::string::String>,
    /// Declared `self`, in any mode — so MIR's argument zero is the receiver.
    pub takes_self: bool,
    /// Declared `take self` — the call consumes its receiver.
    pub take_self: bool,
    /// Which declared parameters are `take`, positionally. The receiver is not
    /// among them; see `keeps_argument` for the index shift.
    pub takes: Vec<bool>,
}

/// Cached metadata derived from StubRegistry.
struct MetadataCache {
    type_names: HashSet<std::string::String>,
    module_names: HashSet<std::string::String>,
    method_metas: Vec<StdlibMethodMeta>,
    /// qualified_name → index into method_metas
    by_name: HashMap<std::string::String, usize>,
}

static CACHE: OnceLock<MetadataCache> = OnceLock::new();

fn build_cache() -> MetadataCache {
    let reg = StubRegistry::load();

    let mut type_names = HashSet::new();
    let mut module_names = HashSet::new();
    let mut method_metas = Vec::new();

    for type_name in reg.type_names() {
        // Module-like types start lowercase (fs, cli, io, etc.)
        // Actual types start uppercase (Vec, Map, File, etc.) or are "string"
        let is_type = type_name == "string"
            || type_name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false);
        if is_type {
            type_names.insert(type_name.to_string());
        } else {
            module_names.insert(type_name.to_string());
        }

        for method in reg.methods(type_name) {
            let qualified = format!("{}_{}", type_name, method.name);
            let ret_cat = parse_ret_ty(&method.ret_ty);
            let ret_prefix = ret_type_prefix(&ret_cat);
            method_metas.push(StdlibMethodMeta {
                qualified_name: qualified,
                ret_category: ret_cat,
                ret_ty: method.ret_ty.clone(),
                ret_type_prefix: ret_prefix,
                takes_self: method.takes_self,
                take_self: method.take_self,
                takes: method.param_modes.iter().map(|m| m.is_take).collect(),
            });
        }
    }

    // Top-level functions (println, print, etc. are builtins — skip them)
    for func in reg.functions() {
        let ret_cat = parse_ret_ty(&func.ret_ty);
        let ret_prefix = ret_type_prefix(&ret_cat);
        method_metas.push(StdlibMethodMeta {
            qualified_name: func.name.clone(),
            ret_category: ret_cat,
            ret_ty: func.ret_ty.clone(),
            ret_type_prefix: ret_prefix,
            takes_self: false,
            take_self: false,
            takes: func.param_modes.iter().map(|m| m.is_take).collect(),
        });
    }

    let by_name = method_metas
        .iter()
        .enumerate()
        .map(|(i, m)| (m.qualified_name.clone(), i))
        .collect();

    MetadataCache {
        type_names,
        module_names,
        method_metas,
        by_name,
    }
}

fn cache() -> &'static MetadataCache {
    CACHE.get_or_init(build_cache)
}

// ── Public API ──────────────────────────────────────────────────

/// All stdlib type names (uppercase + "string").
pub fn stdlib_type_names() -> &'static HashSet<std::string::String> {
    &cache().type_names
}

/// All stdlib module names (lowercase except "string").
pub fn stdlib_module_names() -> &'static HashSet<std::string::String> {
    &cache().module_names
}

/// All method metadata entries.
pub fn method_metas() -> &'static [StdlibMethodMeta] {
    &cache().method_metas
}

/// Look up metadata for a specific qualified name.
pub fn lookup(qualified_name: &str) -> Option<&'static StdlibMethodMeta> {
    let idx = cache().by_name.get(qualified_name)?;
    Some(&cache().method_metas[*idx])
}

/// Does stdlib type `prefix` define a method `method`? Keyed on the stub API.
/// Whether `prefix.method` is declared `@unimplemented` — a signature with
/// nothing behind it on either backend.
///
/// Callers get a diagnostic at their call site instead of discovering it as
/// `Function not found: Vec_reserve` out of codegen, or as a runtime error
/// after the program has already started.
pub fn is_unimplemented(prefix: &str, method: &str) -> bool {
    StubRegistry::load()
        .get_type(prefix)
        .and_then(|t| t.methods.iter().find(|m| m.name == method))
        .is_some_and(|m| m.unimplemented)
}

pub fn type_has_method(prefix: &str, method: &str) -> bool {
    cache().by_name.contains_key(&format!("{}_{}", prefix, method))
}

// ── What a call does with what it is handed ─────────────────────
//
// These three used to be lists of method-name prefixes kept by hand in
// `rc_elide`, `rc_insert` and `container_drop`, which had drifted apart from
// each other. The declaration in `stdlib/*.rk` already says all three, in the
// language's own words, so that is what they read now.

/// Names MIR mints for an operation the stdlib declares under another
/// spelling, mapped to that declaration.
///
/// `v[i]` reaches MIR as `Vec_index`, and the bounds-checked and unchecked
/// forms of the same read get their own names, but there is one declaration
/// behind all of them — `Vec.get`, whose signature says the read points into
/// the vector's own buffer. A `with` block on a `Shared` is the same story:
/// three strategies, one `acquire` each, all standing for the declared
/// accessor.
///
/// Without this the questions below answer "no declaration, so assume the
/// caller owns it", and a string read out of a vector got a release it never
/// owed — the markdown renderer printed freed bytes where the code fence's
/// language should be.
const INTERNAL_SPELLINGS: &[(&str, &str)] = &[
    ("Vec_index", "Vec_get"),
    ("Vec_get_opt", "Vec_get"),
    ("Vec_get_unchecked", "Vec_get"),
    ("Vec_slice", "Vec_get"),
    ("Cell_acquire", "Shared_read"),
    ("Cell_data", "Shared_read"),
    ("Mutex_acquire", "Shared_read"),
    ("Mutex_data", "Shared_read"),
    ("Shared_read_acquire", "Shared_read"),
    ("Shared_write_acquire", "Shared_read"),
    ("Cell_new", "Shared_local"),
    ("Mutex_new", "Shared_mutex"),
];

/// The declaration MIR is calling, by the name it uses — a monomorphized `$`
/// suffix and any module path stripped, and an internal spelling resolved.
fn declared(qualified_name: &str) -> Option<&'static StdlibMethodMeta> {
    let head = qualified_name.rsplit("::").next().unwrap_or(qualified_name);
    let base = head.split('$').next().unwrap_or(head);
    let base = INTERNAL_SPELLINGS
        .iter()
        .find(|(n, _)| *n == base)
        .map(|(_, d)| *d)
        .unwrap_or(base);
    lookup(base)
}

#[cfg(test)]
mod internal_spelling_tests {
    use super::*;

    /// Every internal spelling has to name a declaration that exists. A
    /// renamed stdlib method would otherwise turn one of these into a silent
    /// "no declaration", which reads as "the caller owns what came back" and
    /// frees a string the container still holds.
    #[test]
    fn every_internal_spelling_resolves() {
        for (internal, declared_as) in INTERNAL_SPELLINGS {
            assert!(
                lookup(declared_as).is_some(),
                "{internal} stands for {declared_as}, which no stdlib file declares"
            );
        }
    }
}

/// Does this call keep the argument at `arg_index`, rather than just read it?
///
/// `take` in the declaration is the whole answer: `Vec.push(mutate self, take
/// value: T)` keeps what it is handed and the caller owes no release on it,
/// while `Map.get(self, key: K)` only compares the key and hands it back. MIR
/// counts the receiver as argument zero, so a declared parameter sits one
/// further along on a method.
pub fn keeps_argument(qualified_name: &str, arg_index: usize) -> bool {
    let Some(m) = declared(qualified_name) else { return false };
    let param_index = if m.takes_self {
        match arg_index.checked_sub(1) {
            Some(i) => i,
            None => return m.take_self,
        }
    } else {
        arg_index
    };
    m.takes.get(param_index).copied().unwrap_or(false)
}

/// Does this call hand back a view into storage its receiver keeps owning?
///
/// `Vec.get(self, index: i64) -> Option<T>` points into the vector's buffer:
/// the caller may read it but never release it, because the vector will. The
/// rule is that the return type names one of the receiver type's own
/// parameters — the value came out of the receiver's storage rather than
/// being made here. `Vec.len() -> usize` doesn't, and neither does
/// `string.trim() -> string`, which builds a new one.
pub fn returns_a_view(qualified_name: &str) -> bool {
    declared(qualified_name)
        .is_some_and(|m| m.takes_self && m.ret_category.names_a_type_param())
}

/// Does this call borrow its receiver rather than consume it? True for
/// anything declared `self` or `mutate self`, false for `take self` and for a
/// static method, which has no receiver at all.
pub fn borrows_receiver(qualified_name: &str) -> bool {
    declared(qualified_name).is_some_and(|m| m.takes_self && !m.take_self)
}

// ── Return type string parsing ──────────────────────────────────

/// Parse a return type string from a stub into a RetCategory.
///
/// The parser transforms `T or E` syntax into `Result<T, E>`, so we
/// handle both forms. Examples:
///   "" → Void
///   "()" → Void
///   "bool" → Bool
///   "string" → String
///   "i64" → I64, "usize" / "u64" / "u8" / … → Int(width)
///   "f64" / "f32" → F64
///   "Result<File, IoError>" → Result { ok: Named("File"), err: Named("IoError") }
///   "Result<(), IoError>" → Result { ok: Void, err: Named("IoError") }
///   "Option<T>" → Option(I64)
///   "T?" → Option(I64)
///   "string?" → Option(String)
///   "*u8" → Ptr
fn parse_ret_ty(ret_ty: &str) -> RetCategory {
    let s = ret_ty.trim();
    if s.is_empty() || s == "()" {
        return RetCategory::Void;
    }

    // "Result<T, E>" — parser transforms "T or E" into this form
    if let Some(inner) = strip_generic(s, "Result") {
        if let Some(comma) = find_top_level_comma(inner) {
            let ok_str = inner[..comma].trim();
            let err_str = inner[comma + 1..].trim();
            return RetCategory::Result {
                ok: Box::new(parse_simple_type(ok_str)),
                err: Box::new(parse_simple_type(err_str)),
            };
        }
    }

    // "T or E" pattern (in case raw syntax appears)
    if let Some(idx) = find_or_keyword(s) {
        let ok_str = s[..idx].trim();
        let err_str = s[idx + 4..].trim();
        return RetCategory::Result {
            ok: Box::new(parse_simple_type(ok_str)),
            err: Box::new(parse_simple_type(err_str)),
        };
    }

    // "T?" shorthand for Option<T>
    if s.ends_with('?') {
        let inner = &s[..s.len() - 1];
        return RetCategory::Option(Box::new(parse_simple_type(inner)));
    }

    // "Option<T>"
    if let Some(inner) = strip_generic(s, "Option") {
        return RetCategory::Option(Box::new(parse_simple_type(inner)));
    }

    parse_simple_type(s)
}

/// Parse a simple (non-result, non-option) type string.
fn parse_simple_type(s: &str) -> RetCategory {
    let s = s.trim();
    match s {
        "" | "()" => RetCategory::Void,
        "bool" => RetCategory::Bool,
        "char" => RetCategory::Char,
        "string" => RetCategory::String,
        "i64" => RetCategory::I64,
        "i8" => RetCategory::Int(IntWidth::I8),
        "i16" => RetCategory::Int(IntWidth::I16),
        "i32" => RetCategory::Int(IntWidth::I32),
        "i128" => RetCategory::Int(IntWidth::I128),
        "u8" => RetCategory::Int(IntWidth::U8),
        "u16" => RetCategory::Int(IntWidth::U16),
        "u32" => RetCategory::Int(IntWidth::U32),
        "u64" => RetCategory::Int(IntWidth::U64),
        "u128" => RetCategory::Int(IntWidth::U128),
        "usize" => RetCategory::Int(IntWidth::Usize),
        "isize" => RetCategory::Int(IntWidth::Isize),
        "f32" | "f64" => RetCategory::F64,
        _ if s.starts_with('*') => RetCategory::Ptr,
        _ if s.starts_with('(') && s.ends_with(')') => {
            let inner = &s[1..s.len() - 1];
            if inner.is_empty() {
                return RetCategory::Void;
            }
            let parts = split_top_level(inner, ',');
            RetCategory::Tuple(parts.into_iter().map(|p| parse_simple_type(p.trim())).collect())
        }
        _ => {
            // Named type: "File", "Vec<string>", "Iterator<char>", etc.
            // Extract the base name before any '<'
            let base = if let Some(idx) = s.find('<') {
                &s[..idx]
            } else {
                s
            };
            // A generic type variable like "T" is not a type — it's a hole.
            if base.len() == 1 && base.chars().next().unwrap().is_uppercase() {
                RetCategory::TypeParam(base.to_string())
            } else {
                RetCategory::Named(base.to_string())
            }
        }
    }
}

/// Extract the type prefix from a return category.
fn ret_type_prefix(cat: &RetCategory) -> Option<std::string::String> {
    match cat {
        RetCategory::Void | RetCategory::Bool | RetCategory::I64 | RetCategory::Int(_)
        | RetCategory::F64 => None,
        // The stub doesn't know what `T` is, so it can't name a prefix for it.
        // Same answer it gave when `T` was recorded as I64.
        RetCategory::TypeParam(_) => None,
        RetCategory::String => Some("string".to_string()),
        // Keep the "char" prefix it had as a Named type, so `char` methods
        // still resolve on the result of a char-returning stdlib call.
        RetCategory::Char => Some("char".to_string()),
        RetCategory::Ptr => Some("Ptr".to_string()),
        RetCategory::Named(name) => Some(name.clone()),
        RetCategory::Tuple(_) => None,
        RetCategory::Option(_) => Some("Option".to_string()),
        RetCategory::Result { ok, .. } => {
            // The prefix is the ok type's prefix (e.g., Result<File, _> → "File")
            ret_type_prefix(ok)
        }
    }
}

/// Split a string by a separator at nesting depth 0 (respecting `<...>` and `(...)` brackets).
fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    for (i, c) in s.char_indices() {
        match c {
            '<' | '(' => depth += 1,
            '>' | ')' => depth = depth.saturating_sub(1),
            c2 if c2 == sep && depth == 0 => {
                parts.push(&s[start..i]);
                start = i + c2.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Find first comma at nesting depth 0 (respecting `<...>` brackets).
fn find_top_level_comma(s: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => return Some(i),
            _ => {}
        }
    }
    None
}

/// Find " or " keyword at top level (not inside <...> brackets).
fn find_or_keyword(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut depth = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' => depth = depth.saturating_sub(1),
            b' ' if depth == 0 && i + 4 <= bytes.len() => {
                if &bytes[i..i + 4] == b" or " {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Strip a generic wrapper: "Option<string>" → Some("string"), "Vec<T>" → Some("T")
fn strip_generic<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(prefix)?;
    let rest = rest.strip_prefix('<')?;
    let rest = rest.strip_suffix('>')?;
    Some(rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_void() {
        assert_eq!(parse_ret_ty(""), RetCategory::Void);
        assert_eq!(parse_ret_ty("()"), RetCategory::Void);
    }

    #[test]
    fn parse_primitives() {
        assert_eq!(parse_ret_ty("bool"), RetCategory::Bool);
        assert_eq!(parse_ret_ty("string"), RetCategory::String);
        assert_eq!(parse_ret_ty("i64"), RetCategory::I64);
        assert_eq!(parse_ret_ty("f64"), RetCategory::F64);
        // Every other width carries itself. They all used to collapse to `I64`,
        // so a `u64` that filled its range rendered as the signed reading of its
        // bits (#823).
        assert_eq!(parse_ret_ty("usize"), RetCategory::Int(IntWidth::Usize));
        assert_eq!(parse_ret_ty("u64"), RetCategory::Int(IntWidth::U64));
        assert_eq!(parse_ret_ty("u8"), RetCategory::Int(IntWidth::U8));
        assert_eq!(parse_ret_ty("i32"), RetCategory::Int(IntWidth::I32));
        assert_eq!(parse_ret_ty("u128"), RetCategory::Int(IntWidth::U128));
    }

    #[test]
    fn parse_result() {
        // Parser transforms "File or IoError" → "Result<File, IoError>"
        // The error side is parsed, not assumed. It used to be hardcoded I64,
        // which cost `T or JoinError` its enum identity all the way to codegen
        // (#677) and left every other error enum in the same shape.
        assert_eq!(
            parse_ret_ty("Result<File, IoError>"),
            RetCategory::Result {
                ok: Box::new(RetCategory::Named("File".into())),
                err: Box::new(RetCategory::Named("IoError".into())),
            }
        );
        assert_eq!(
            parse_ret_ty("Result<(), IoError>"),
            RetCategory::Result {
                ok: Box::new(RetCategory::Void),
                err: Box::new(RetCategory::Named("IoError".into())),
            }
        );
        // Also handle raw "or" syntax as fallback
        assert_eq!(
            parse_ret_ty("File or IoError"),
            RetCategory::Result {
                ok: Box::new(RetCategory::Named("File".into())),
                err: Box::new(RetCategory::Named("IoError".into())),
            }
        );
    }

    #[test]
    fn parse_option() {
        assert_eq!(
            parse_ret_ty("string?"),
            RetCategory::Option(Box::new(RetCategory::String))
        );
        assert_eq!(
            parse_ret_ty("Option<usize>"),
            RetCategory::Option(Box::new(RetCategory::Int(IntWidth::Usize)))
        );
    }

    #[test]
    fn parse_named() {
        assert_eq!(parse_ret_ty("File"), RetCategory::Named("File".into()));
        assert_eq!(parse_ret_ty("Vec<string>"), RetCategory::Named("Vec".into()));
    }

    #[test]
    fn parse_ptr() {
        assert_eq!(parse_ret_ty("*u8"), RetCategory::Ptr);
    }

    #[test]
    fn parse_generic_t() {
        // A single-letter type variable is a hole, not a type. It used to be
        // recorded as I64, which is how a `Vec<string>.remove(i)` destination
        // got an 8-byte slot (#1020).
        assert_eq!(parse_ret_ty("T"), RetCategory::TypeParam("T".into()));
        assert!(parse_ret_ty("T").names_a_type_param());
        assert!(parse_ret_ty("T?").names_a_type_param());
        assert!(parse_ret_ty("T or IoError").names_a_type_param());
        assert!(!parse_ret_ty("string").names_a_type_param());
        assert!(!parse_ret_ty("File").names_a_type_param());
    }

    #[test]
    fn result_prefix_is_ok_type() {
        let cat = parse_ret_ty("File or IoError");
        assert_eq!(ret_type_prefix(&cat), Some("File".into()));
    }

    #[test]
    fn void_result_has_no_prefix() {
        let cat = parse_ret_ty("() or IoError");
        assert_eq!(ret_type_prefix(&cat), None);
    }

    #[test]
    fn cache_has_types_and_modules() {
        let types = stdlib_type_names();
        let mods = stdlib_module_names();
        assert!(types.contains("Vec"), "missing Vec type");
        assert!(types.contains("string"), "missing string type");
        assert!(mods.contains("fs"), "missing fs module");
        assert!(mods.contains("cli"), "missing cli module");
    }

    #[test]
    fn cache_has_method_metas() {
        let metas = method_metas();
        assert!(!metas.is_empty(), "no method metas");
        // Spot-check a known method
        let vec_push = lookup("Vec_push");
        assert!(vec_push.is_some(), "missing Vec_push meta");
        assert_eq!(vec_push.unwrap().ret_category, RetCategory::Void);
    }

    #[test]
    fn tcp_listener_accept_returns_result() {
        let meta = lookup("TcpListener_accept").expect("missing TcpListener_accept");
        assert!(matches!(meta.ret_category, RetCategory::Result { .. }),
            "expected Result, got {:?}", meta.ret_category);
    }

    #[test]
    fn fs_open_returns_result_with_file_prefix() {
        let meta = lookup("fs_open").expect("missing fs_open");
        assert!(matches!(meta.ret_category, RetCategory::Result { .. }));
        assert_eq!(meta.ret_type_prefix, Some("File".into()));
    }

    #[test]
    fn string_from_raw_returns_string() {
        let meta = lookup("string_from_raw").expect("missing string_from_raw");
        assert_eq!(meta.ret_category, RetCategory::String);
    }

    /// Names a stub signature may mention that `stdlib_type_names` doesn't hold.
    ///
    /// Two groups. Declared elsewhere and genuinely resolvable: `Never` and
    /// `Ordering` are registered by the checker rather than by a stub;
    /// `Reader`/`Writer` are stdlib traits in io.rk, and the stub registry only
    /// collects structs and enums; `Iterator` is special-cased in the resolver;
    /// `Self` isn't a type name at all. Genuinely still missing: `InsertError`
    /// belongs to the pool API (`mem.pools/PL8`), and `Error` is the `any Error`
    /// catch-all that ER32 auto-boxing will register (#708).
    const PENDING_STUB_TYPES: &[&str] = &[
        "Never", "Ordering", "Reader", "Writer", "Iterator", "Self",
        "InsertError", "Error",
    ];

    /// PascalCase type names a signature string mentions, with generic
    /// arguments, `T or E` branches and pointer/optional markers peeled off.
    /// Single letters are type parameters, not types.
    fn named_types_in(ty: &str) -> Vec<std::string::String> {
        let mut out = Vec::new();
        let mut word = std::string::String::new();
        for ch in ty.chars() {
            if ch.is_alphanumeric() || ch == '_' {
                word.push(ch);
            } else {
                take_word(&mut word, &mut out);
            }
        }
        take_word(&mut word, &mut out);
        out
    }

    fn take_word(word: &mut std::string::String, out: &mut Vec<std::string::String>) {
        let w = std::mem::take(word);
        if w.len() > 1 && w.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
            out.push(w);
        }
    }

    /// Every named type a stub signature mentions has to be one that exists.
    ///
    /// `Vec.try_push` was declared `-> void or PushError<T>` for as long as the
    /// stub existed, and `PushError` was never declared anywhere: the error had
    /// no variants, no constructor and no `message()`, so the rejected value it
    /// was meant to hand back could not be read (#666). Nothing caught it
    /// because stub signatures aren't type-checked — a name that resolves to
    /// nothing just stays unresolved until some unlucky call site trips on it.
    #[test]
    fn stub_signatures_only_name_types_that_exist() {
        let reg = StubRegistry::load();
        let known = stdlib_type_names();
        let mut missing: Vec<std::string::String> = Vec::new();

        let mut check = |ty: &str, at: std::string::String, missing: &mut Vec<std::string::String>| {
            for name in named_types_in(ty) {
                if !known.contains(&name) && !PENDING_STUB_TYPES.contains(&name.as_str()) {
                    missing.push(format!("{} names unknown type `{}`", at, name));
                }
            }
        };

        for type_name in reg.type_names() {
            for m in reg.methods(type_name) {
                check(&m.ret_ty, format!("{}.{} return", type_name, m.name), &mut missing);
                for (pname, pty) in &m.params {
                    check(pty, format!("{}.{} param `{}`", type_name, m.name, pname), &mut missing);
                }
            }
        }
        for f in reg.functions() {
            check(&f.ret_ty, format!("{} return", f.name), &mut missing);
        }

        missing.sort();
        assert!(
            missing.is_empty(),
            "stub signatures name types that don't exist:\n  {}",
            missing.join("\n  ")
        );
    }

    /// The three ownership questions, answered from the declarations.
    ///
    /// These replaced prefix lists kept by hand in `rc_elide`, `rc_insert` and
    /// `container_drop` — lists that had drifted apart from each other and from
    /// the stdlib. Pinning the answers here means a stdlib signature can't
    /// quietly change what MIR believes about a call: getting one wrong is a
    /// leak in one direction and a double free in the other.
    #[test]
    fn ownership_questions_follow_the_declarations() {
        // Stored: the reference moves in with the value.
        assert!(keeps_argument("Vec_push", 1), "Vec.push keeps its value");
        assert!(keeps_argument("Map_insert", 1), "Map.insert keeps its key");
        assert!(keeps_argument("Map_insert", 2), "Map.insert keeps its value");
        assert!(keeps_argument("Vec_insert", 2), "Vec.insert keeps its value");
        assert!(keeps_argument("Shared_new", 0), "Shared.new keeps its payload");
        assert!(keeps_argument("Cell_new", 0), "Cell.new is Shared.local");

        // Read and forgotten: the caller still owns what it passed.
        assert!(!keeps_argument("Map_get", 1), "Map.get only reads the key");
        assert!(!keeps_argument("Map_contains_key", 1));
        assert!(!keeps_argument("Vec_insert", 1), "the index is a number");
        // The receiver is never "kept" by a borrowing method.
        assert!(!keeps_argument("Vec_push", 0));

        // A view into the receiver's own storage: releasing it here would free
        // what the container still holds.
        assert!(returns_a_view("Vec_get"));
        assert!(returns_a_view("Vec_index"), "`v[i]` is Vec.get by another name");
        assert!(returns_a_view("Vec_get_unchecked"));
        assert!(returns_a_view("Map_get"));
        assert!(!returns_a_view("Vec_len"), "a length is not a view");
        assert!(!returns_a_view("Vec_new"), "a constructor has no receiver");

        // Borrowed receivers, so the drop pass may still free the container.
        assert!(borrows_receiver("Vec_push"));
        assert!(borrows_receiver("Vec_get"));
        assert!(!borrows_receiver("Vec_new"), "static: no receiver at all");
        assert!(!borrows_receiver("TaskHandle_join"), "declared `take self`");
    }

    /// A monomorphized name carries a `$` suffix, and a path-qualified one a
    /// `::` prefix. Both have to reach the same declaration, or the answer
    /// changes the moment a generic gets instantiated.
    #[test]
    fn mangled_names_reach_the_same_declaration() {
        assert!(keeps_argument("Vec_push$string", 1));
        assert!(returns_a_view("std::collections::Vec_get$string"));
        assert!(borrows_receiver("Vec_push$Holder"));
    }
}
