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
fn parse_stub_type(s: &str) -> Type {
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
        return Type::Option(Box::new(parse_stub_type(inner)));
    }

    match s {
        "" | "()" => Type::Unit,
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
        "usize" => Type::U64,
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

/// Split `T or E` into `("T", "E")`, respecting nested angle brackets.
fn split_or_type(s: &str) -> Option<(&str, &str)> {
    let mut depth: i32 = 0;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'<' => depth += 1,
            b'>' => depth -= 1,
            b' ' if depth == 0 && s[i..].starts_with(" or ") => {
                return Some((s[..i].trim(), s[i + 4..].trim()));
            }
            _ => {}
        }
    }
    None
}

/// Split `A, B` at the first top-level comma, respecting nested angle brackets.
fn split_comma(s: &str) -> Option<(&str, &str)> {
    let mut depth: i32 = 0;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b'<' => depth += 1,
            b'>' => depth -= 1,
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
        assert!(bm.get_method("fs", "read_file").is_some());
        assert!(bm.get_method("fs", "write_file").is_some());
        assert!(bm.get_method("fs", "exists").is_some());
        assert!(bm.get_method("fs", "open").is_some());
        assert!(bm.get_method("fs", "create").is_some());
        assert!(bm.get_method("fs", "append_file").is_some());
    }

    #[test]
    fn fs_read_file_signature() {
        let bm = BuiltinModules::new();
        let sig = bm.get_method("fs", "read_file").unwrap();
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
    fn fs_copy_returns_u64() {
        let bm = BuiltinModules::new();
        let sig = bm.get_method("fs", "copy").unwrap();
        assert_eq!(sig.params, vec![Type::String, Type::String]);
        assert_eq!(sig.ret, Type::Result {
            ok: Box::new(Type::U64),
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
        // Return type should be Result { ok: _Any (freshened), err: Error }
        match &sig.ret {
            Type::Result { ok, err } => {
                assert!(matches!(ok.as_ref(), Type::UnresolvedNamed(n) if n.starts_with('_')));
                assert_eq!(err.as_ref(), &Type::UnresolvedNamed("Error".to_string()));
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
        let bm = BuiltinModules::new();
        let sig = bm.get_method("cli", "args").unwrap();
        assert!(sig.params.is_empty());
        assert_eq!(sig.ret, Type::UnresolvedNamed("Vec<string>".to_string()));
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
        let ty = parse_stub_type("Vec<string>");
        assert_eq!(ty, Type::UnresolvedNamed("Vec<string>".to_string()));
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
        assert_eq!(ty, Type::Option(Box::new(Type::I64)));
    }
}
