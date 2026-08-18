// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! MIR-level metadata derived from stdlib stub files.
//!
//! Parses return type strings from MethodStub/FunctionStub into a
//! lightweight enum that rask-mir can convert to MirType. Keeps the
//! stub files as the single source of truth for stdlib API shapes.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use crate::stubs::StubRegistry;

/// Return type category — does not depend on rask-mir types.
#[derive(Debug, Clone, PartialEq)]
pub enum RetCategory {
    Void,
    Bool,
    I64,
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
    /// Tuple of types (e.g., `(Request, Responder)`).
    Tuple(Vec<RetCategory>),
}

/// Metadata for a single stdlib method, derived from stubs.
#[derive(Debug, Clone)]
pub struct StdlibMethodMeta {
    /// Qualified name as MIR sees it: "Vec_push", "fs_open".
    pub qualified_name: std::string::String,
    /// Return type category.
    pub ret_category: RetCategory,
    /// Type prefix of the return value (for local_type_prefix tracking).
    /// E.g., "fs_open" returns File → prefix "File".
    pub ret_type_prefix: Option<std::string::String>,
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
                ret_type_prefix: ret_prefix,
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
            ret_type_prefix: ret_prefix,
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

// ── Return type string parsing ──────────────────────────────────

/// Parse a return type string from a stub into a RetCategory.
///
/// The parser transforms `T or E` syntax into `Result<T, E>`, so we
/// handle both forms. Examples:
///   "" → Void
///   "()" → Void
///   "bool" → Bool
///   "string" → String
///   "usize" / "i64" / "u64" → I64
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
        "i8" | "u8" | "i16" | "u16" | "i32" | "u32" | "i64" | "u64" | "i128" | "u128"
        | "isize" | "usize" => RetCategory::I64,
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
            // Generic type variables like "T" → treat as I64
            if base.len() == 1 && base.chars().next().unwrap().is_uppercase() {
                RetCategory::I64
            } else {
                RetCategory::Named(base.to_string())
            }
        }
    }
}

/// Extract the type prefix from a return category.
fn ret_type_prefix(cat: &RetCategory) -> Option<std::string::String> {
    match cat {
        RetCategory::Void | RetCategory::Bool | RetCategory::I64 | RetCategory::F64 => None,
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
        assert_eq!(parse_ret_ty("usize"), RetCategory::I64);
        assert_eq!(parse_ret_ty("f64"), RetCategory::F64);
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
            RetCategory::Option(Box::new(RetCategory::I64))
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
        // Single-letter type variables are opaque → I64
        assert_eq!(parse_ret_ty("T"), RetCategory::I64);
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
}


