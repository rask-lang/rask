// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Builtin module method signatures, derived from stdlib stub files.

use std::collections::HashMap;

use super::type_defs::ModuleMethodSig;

use crate::types::Type;

/// Modules with type-checked signatures.
const TYPED_MODULES: &[&str] = &["fs", "net", "json", "cli", "io", "std"];

/// Registry of builtin modules and their methods.
#[derive(Debug, Default)]
pub(super) struct BuiltinModules {
    pub(super) modules: HashMap<String, Vec<ModuleMethodSig>>,
}

impl BuiltinModules {
    pub fn new() -> Self {
        let mut modules = HashMap::new();
        let reg = rask_stdlib::StubRegistry::load();

        for &module_name in TYPED_MODULES {
            let methods = reg.methods(module_name);
            if methods.is_empty() {
                continue;
            }
            let sigs: Vec<ModuleMethodSig> = methods.iter().map(|m| {
                ModuleMethodSig {
                    name: m.name.clone(),
                    params: m.params.iter().map(|(_, ty)| parse_stub_type(ty)).collect(),
                    ret: parse_stub_type(&m.ret_ty),
                    type_param_bounds: m.type_param_bounds.clone(),
                }
            }).collect();
            modules.insert(module_name.to_string(), sigs);
        }

        Self { modules }
    }

    pub fn get_method(&self, module: &str, method: &str) -> Option<&ModuleMethodSig> {
        self.modules.get(module)?.iter().find(|m| m.name == method)
    }

    pub fn is_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }
}

/// Parse a type string from a stub file into a Type.
///
/// The parser normalizes `T or E` to `Result<T, E>` and `T?` to `Option<T>`
/// in the string representation. This handles both forms plus primitives,
/// generic placeholders, and named types. Single uppercase letters become
/// `_Any` wildcards for the type checker's freshening logic.
pub(super) fn parse_stub_type(s: &str) -> Type {
    let s = s.trim();

    // Handle "X or Y" result types (raw form, just in case)
    if let Some((ok_str, err_str)) = split_or_type(s) {
        return Type::Result {
            ok: Box::new(parse_stub_type(ok_str)),
            err: Box::new(parse_stub_type(err_str)),
        };
    }

    // Handle "Result<T, E>" (parser-normalized form)
    if let Some(inner) = s.strip_prefix("Result<").and_then(|r| r.strip_suffix('>')) {
        if let Some((ok_str, err_str)) = split_comma(inner) {
            return Type::Result {
                ok: Box::new(parse_stub_type(ok_str)),
                err: Box::new(parse_stub_type(err_str)),
            };
        }
    }

    // Handle "Option<T>" (parser-normalized form)
    if let Some(inner) = s.strip_prefix("Option<").and_then(|r| r.strip_suffix('>')) {
        return Type::option(parse_stub_type(inner));
    }

    // `T?` — the way optionals are actually written in the stubs. Without this
    // `byte_at(i) -> u8?` came back as the *name* "u8?", so the value was never
    // an optional: `??` had nothing to narrow and no method resolved on the
    // result. A generic argument can end in `>` (`Vec<i32>?`), so strip the
    // suffix before the generic handling below rather than after.
    if let Some(inner) = s.strip_suffix('?') {
        if !inner.is_empty() {
            return Type::option(parse_stub_type(inner));
        }
    }

    // `*T` — a raw pointer. Without this it came back as a *name* that happened
    // to read "*u8", and a name prints exactly like the real pointer type — so
    // `s.as_ptr().offset(1)` failed with "no method `offset` found for type
    // `*u8`" while the type on screen looked perfectly correct (#696). Same
    // shape as the `T?` case above, and for the same reason.
    if let Some(inner) = s.strip_prefix('*') {
        if !inner.is_empty() {
            return Type::RawPtr(Box::new(parse_stub_type(inner)));
        }
    }

    // `any Trait` — a trait object. Without this it came back as the *name*
    // "any Reader", which prints exactly like the real type, so a module
    // function's `any Trait` parameter looked perfectly fine and nothing
    // recorded the TR5 coercion its argument needed. `io.copy(buf, out)` handed
    // over a raw struct pointer, and the first dispatch through it jumped to
    // address zero (#860). Same shape as the `*T` case above (#696).
    if let Some(trait_name) = s.strip_prefix("any ") {
        let trait_name = trait_name.trim();
        if !trait_name.is_empty() {
            return Type::TraitObject { trait_name: trait_name.to_string() };
        }
    }

    // `(A, B, ...)` — a tuple. Without this `char_indices() -> Iterator<(usize,
    // char)>` came back with two arguments, `(usize` and `char)`, because the
    // comma inside the parens read as the generic's own separator. `t.0` then
    // reported "no field `0` on type `(usize`" and a `for (i, c)` over it had
    // no type MIR could lower (#841).
    if s.starts_with('(') && s.ends_with(')') && s.len() > 2 {
        let inner = &s[1..s.len() - 1];
        let parts = split_top_level(inner);
        if parts.len() > 1 {
            return Type::Tuple(parts.iter().map(|p| parse_stub_type(p)).collect());
        }
    }

    // Handle other generics: `Name<T1, T2, ...>` (Vec, Map, Pool, Handle, ...)
    // Without this, `Vec<string>` returns as `UnresolvedNamed("Vec<string>")`,
    // which the method-lookup path doesn't unify against `Generic { Vec, [string] }`.
    if let Some(open) = s.find('<') {
        if s.ends_with('>') {
            let name = s[..open].trim();
            let inner = &s[open + 1..s.len() - 1];
            let args: Vec<Type> = if inner.is_empty() {
                Vec::new()
            } else {
                split_top_level(inner).iter().map(|p| parse_stub_type(p)).collect()
            };
            return Type::UnresolvedGeneric {
                name: name.to_string(),
                args: args.into_iter().map(|t| crate::types::GenericArg::Type(Box::new(t))).collect(),
            };
        }
    }

    match s {
        "" | "()" | "void" => Type::Unit,
        "none" => Type::None,
        "bool" => Type::Bool,
        "string" => Type::String,
        "char" => Type::Char,
        "i8" => Type::I8,
        "i16" => Type::I16,
        "i32" => Type::I32,
        "i64" => Type::I64,
        "i128" => Type::I128,
        "u8" => Type::U8,
        "u16" => Type::U16,
        "u32" => Type::U32,
        "u64" => Type::U64,
        "u128" => Type::U128,
        "usize" => Type::usize_ty(),
        "f32" => Type::F32,
        "f64" => Type::F64,
        "Never" => Type::Never,
        // Single uppercase letter = type variable (wildcard for module generics)
        _ if s.len() == 1 && s.as_bytes()[0].is_ascii_uppercase() => {
            Type::UnresolvedNamed("_Any".to_string())
        }
        _ => Type::UnresolvedNamed(s.to_string()),
    }
}

/// Split on top-level commas, respecting both `<…>` and `(…)`. A tuple inside
/// a generic argument list is the reason the parens count.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth: i32 = 0;
    let mut start = 0usize;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'<' | b'(' => depth += 1,
            b'>' | b')' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].trim());
    parts
}

/// Split `T or E` into `("T", "E")`, respecting nesting.
fn split_or_type(s: &str) -> Option<(&str, &str)> {
    let mut depth: i32 = 0;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'<' | b'(' => depth += 1,
            b'>' | b')' => depth -= 1,
            b' ' if depth == 0 && s[i..].starts_with(" or ") => {
                return Some((s[..i].trim(), s[i + 4..].trim()));
            }
            _ => {}
        }
    }
    None
}

/// Split `A, B` at the first top-level comma, respecting nesting.
fn split_comma(s: &str) -> Option<(&str, &str)> {
    let mut depth: i32 = 0;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'<' | b'(' => depth += 1,
            b'>' | b')' => depth -= 1,
            b',' if depth == 0 => {
                return Some((s[..i].trim(), s[i + 1..].trim()));
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_load_from_stubs() {
        let bm = BuiltinModules::new();
        assert!(bm.is_module("fs"));
        assert!(bm.is_module("net"));
        assert!(bm.is_module("json"));
        assert!(bm.is_module("cli"));
        assert!(bm.is_module("io"));
        assert!(bm.is_module("std"));
        assert!(!bm.is_module("random"));
    }

    #[test]
    fn fs_methods_present() {
        let bm = BuiltinModules::new();
        assert!(bm.get_method("fs", "read_text").is_some());
        assert!(bm.get_method("fs", "write_text").is_some());
        assert!(bm.get_method("fs", "exists").is_some());
        assert!(bm.get_method("fs", "open").is_some());
        assert!(bm.get_method("fs", "create").is_some());
        assert!(bm.get_method("fs", "append_text").is_some());
    }

    #[test]
    fn fs_read_text_signature() {
        let bm = BuiltinModules::new();
        let sig = bm.get_method("fs", "read_text").unwrap();
        assert_eq!(sig.params, vec![Type::String]);
        assert_eq!(sig.ret, Type::Result {
            ok: Box::new(Type::String),
            err: Box::new(Type::UnresolvedNamed("IoError".to_string())),
        });
    }

    #[test]
    fn fs_exists_returns_bool() {
        let bm = BuiltinModules::new();
        let sig = bm.get_method("fs", "exists").unwrap();
        assert_eq!(sig.ret, Type::Bool);
    }

    #[test]
    fn fs_copy_returns_void() {
        let bm = BuiltinModules::new();
        let sig = bm.get_method("fs", "copy").unwrap();
        assert_eq!(sig.params, vec![Type::String, Type::String]);
        assert_eq!(sig.ret, Type::Result {
            ok: Box::new(Type::Unit),
            err: Box::new(Type::UnresolvedNamed("IoError".to_string())),
        });
    }

    #[test]
    fn json_encode_has_wildcard_param() {
        let bm = BuiltinModules::new();
        let sig = bm.get_method("json", "encode").unwrap();
        assert_eq!(sig.params, vec![Type::UnresolvedNamed("_Any".to_string())]);
        assert_eq!(sig.ret, Type::String);
    }

    #[test]
    fn json_decode_has_generic_return() {
        let bm = BuiltinModules::new();
        let sig = bm.get_method("json", "decode").unwrap();
        assert_eq!(sig.params, vec![Type::String]);
        // Return type should be Result { ok: _Any (freshened), err: JsonError }
        match &sig.ret {
            Type::Result { ok, err } => {
                assert!(matches!(ok.as_ref(), Type::UnresolvedNamed(n) if n.starts_with('_')));
                assert_eq!(err.as_ref(), &Type::UnresolvedNamed("JsonError".to_string()));
            }
            other => panic!("Expected Result, got {:?}", other),
        }
    }

    #[test]
    fn std_exit_returns_never() {
        let bm = BuiltinModules::new();
        let sig = bm.get_method("std", "exit").unwrap();
        assert_eq!(sig.params, vec![Type::I64]);
        assert_eq!(sig.ret, Type::Never);
    }

    #[test]
    fn cli_args_returns_vec_string() {
        use crate::types::GenericArg;
        let bm = BuiltinModules::new();
        let sig = bm.get_method("cli", "args").unwrap();
        assert!(sig.params.is_empty());
        assert_eq!(sig.ret, Type::UnresolvedGeneric {
            name: "Vec".to_string(),
            args: vec![GenericArg::Type(Box::new(Type::String))],
        });
    }

    #[test]
    fn parse_primitives() {
        assert_eq!(parse_stub_type("string"), Type::String);
        assert_eq!(parse_stub_type("bool"), Type::Bool);
        assert_eq!(parse_stub_type("i64"), Type::I64);
        assert_eq!(parse_stub_type("u64"), Type::U64);
        assert_eq!(parse_stub_type("()"), Type::Unit);
        assert_eq!(parse_stub_type(""), Type::Unit);
        assert_eq!(parse_stub_type("Never"), Type::Never);
    }

    #[test]
    fn parse_result_type() {
        let ty = parse_stub_type("string or IoError");
        assert_eq!(ty, Type::Result {
            ok: Box::new(Type::String),
            err: Box::new(Type::UnresolvedNamed("IoError".to_string())),
        });
    }

    #[test]
    fn parse_generic_wildcard() {
        let ty = parse_stub_type("T");
        assert_eq!(ty, Type::UnresolvedNamed("_Any".to_string()));
    }

    #[test]
    fn parse_named_type() {
        let ty = parse_stub_type("File");
        assert_eq!(ty, Type::UnresolvedNamed("File".to_string()));
    }

    #[test]
    fn parse_generic_type() {
        use crate::types::GenericArg;
        let ty = parse_stub_type("Vec<string>");
        assert_eq!(ty, Type::UnresolvedGeneric {
            name: "Vec".to_string(),
            args: vec![GenericArg::Type(Box::new(Type::String))],
        });
    }

    #[test]
    fn split_or_respects_angle_brackets() {
        let result = split_or_type("Option<T> or Error");
        assert_eq!(result, Some(("Option<T>", "Error")));
    }

    #[test]
    fn parse_result_generic_form() {
        // Parser normalizes "string or IoError" → "Result<string, IoError>"
        let ty = parse_stub_type("Result<string, IoError>");
        assert_eq!(ty, Type::Result {
            ok: Box::new(Type::String),
            err: Box::new(Type::UnresolvedNamed("IoError".to_string())),
        });
    }

    #[test]
    fn parse_option_type() {
        let ty = parse_stub_type("Option<i64>");
        assert_eq!(ty, Type::option(Type::I64));
    }
}
