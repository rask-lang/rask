// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Builtin module method signatures, derived from stdlib stub files.

use std::collections::HashMap;

use super::type_defs::ModuleMethodSig;

use crate::types::Type;
use rask_ast::ty::TypeExpr;

/// Modules with type-checked signatures.
const TYPED_MODULES: &[&str] = &["fs", "net", "json", "cli", "io", "std"];

/// Registry of builtin modules and their methods.
#[derive(Debug, Default, Clone)]
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
                    params: m.params.iter().map(|(_, ty)| stub_type(ty)).collect(),
                    ret: stub_type(&m.ret_ty),
                    type_param_bounds: m.type_param_bounds.iter()
                        .map(|(n, b)| (n.clone(), b.clone()))
                        .collect(),
                    // The declared type is still intact here, before
                    // `stub_type` erases a type parameter into `_Any`.
                    param_type_params: m.params.iter()
                        .map(|(_, ty)| {
                            let t = ty.bare_name()?;
                            m.type_param_bounds.iter()
                                .find(|(n, _)| n == t)
                                .map(|(n, _)| n.clone())
                        })
                        .collect(),
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

/// A stub signature's type, read without a type table: names stay unresolved
/// for the checker to look up, and a single uppercase letter is a wildcard.
pub(super) fn stub_type(ty: &TypeExpr) -> Type {
    let all = |ts: &[TypeExpr]| ts.iter().map(stub_type).collect::<Vec<_>>();
    match ty {
        TypeExpr::Unit => Type::Unit,
        TypeExpr::NoneType => Type::None,
        TypeExpr::Func { params, ret } => Type::Fn { params: all(params), ret: Box::new(stub_type(ret)) },
        TypeExpr::Result { ok, err } => Type::Result {
            ok: Box::new(stub_type(ok)),
            err: Box::new(stub_type(err)),
        },
        TypeExpr::Optional(inner) => Type::option(stub_type(inner)),
        TypeExpr::RawPtr(inner) => Type::RawPtr(Box::new(stub_type(inner))),
        // Which declaration it means is the stdlib's, settled where the
        // signature is used (`TypeTable::as_stdlib_reads`): no table exists yet.
        TypeExpr::Any(inner) => Type::InterfaceObject { interface_name: inner.to_string(), decl: None },
        TypeExpr::Tuple(elems) => Type::Tuple(all(elems)),
        TypeExpr::Named { path, args } if !args.is_empty() => {
            let name = path.join(".");
            match (name.as_str(), args.as_slice()) {
                ("Option", [inner]) => Type::option(stub_type(inner)),
                ("Result", [ok, err]) => Type::Result {
                    ok: Box::new(stub_type(ok)),
                    err: Box::new(stub_type(err)),
                },
                _ => Type::UnresolvedGeneric {
                    name,
                    args: all(args)
                        .into_iter()
                        .map(|t| crate::types::GenericArg::Type(Box::new(t)))
                        .collect(),
                },
            }
        }
        TypeExpr::Named { path, .. } => stub_name(&path.join(".")),
        _ => Type::UnresolvedNamed(ty.to_string()),
    }
}

fn stub_name(s: &str) -> Type {
    match s {
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
        "isize" => Type::isize_ty(),
        "f32" => Type::F32,
        "f64" => Type::F64,
        "Never" => Type::Never,
        // The C scalar names, per struct.c-interop/TM1. See `c_type_spelling`.
        _ if rask_ast::primitives::c_type_spelling(s).is_some() => {
            stub_name(rask_ast::primitives::c_type_spelling(s).unwrap())
        }
        // Single uppercase letter = type variable (wildcard for module generics)
        _ if s.len() == 1 && s.as_bytes()[0].is_ascii_uppercase() => {
            Type::UnresolvedNamed("_Any".to_string())
        }
        _ => Type::UnresolvedNamed(s.to_string()),
    }
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
        assert!(bm.get_method("fs", "create_file").is_some());
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

    /// A `func(...)` parameter has to come back as a real function type. As a
    /// *name* it prints exactly like one, so nothing ties the argument to it —
    /// the shape that left `spawn(f: func() -> T) -> Handle<T>` with an
    /// unresolved T (#882).
    #[test]
    fn a_function_parameter_is_a_function_type() {
        let f = TypeExpr::Func { params: vec![], ret: Box::new(TypeExpr::named("T")) };
        assert_eq!(
            stub_type(&f),
            Type::Fn { params: Vec::new(), ret: Box::new(Type::UnresolvedNamed("_Any".to_string())) },
        );
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
    fn a_generic_stays_unresolved_with_its_arguments() {
        use crate::types::GenericArg;
        let ty = stub_type(&TypeExpr::generic("Vec", vec![TypeExpr::named("string")]));
        assert_eq!(ty, Type::UnresolvedGeneric {
            name: "Vec".to_string(),
            args: vec![GenericArg::Type(Box::new(Type::String))],
        });
    }
}
