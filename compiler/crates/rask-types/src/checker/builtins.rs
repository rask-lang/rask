// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Builtin module method signatures (fs, net, json, etc.).

use std::collections::HashMap;

use super::type_defs::ModuleMethodSig;

use crate::types::Type;

/// Registry of builtin modules and their methods.
#[derive(Debug, Default)]
pub(super) struct BuiltinModules {
    pub(super) modules: HashMap<String, Vec<ModuleMethodSig>>,
}

impl BuiltinModules {
    pub fn new() -> Self {
        let mut modules = HashMap::new();

        // fs module
        let mut fs_methods = Vec::new();
        let io_error_ty = Type::UnresolvedNamed("IoError".to_string());

        // fs.open(path: string) -> File or IoError
        fs_methods.push(ModuleMethodSig {
            name: "open".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("File".to_string())),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.create(path: string) -> File or IoError
        fs_methods.push(ModuleMethodSig {
            name: "create".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("File".to_string())),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.read_file(path: string) -> string or IoError
        fs_methods.push(ModuleMethodSig {
            name: "read_file".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::String),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.write_file(path: string, content: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "write_file".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.exists(path: string) -> bool
        fs_methods.push(ModuleMethodSig {
            name: "exists".to_string(),
            params: vec![Type::String],
            ret: Type::Bool,
        });
        // fs.read_lines(path: string) -> Vec<string> or IoError
        fs_methods.push(ModuleMethodSig {
            name: "read_lines".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("Vec<string>".to_string())),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.canonicalize(path: string) -> string or IoError
        fs_methods.push(ModuleMethodSig {
            name: "canonicalize".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::String),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.copy(from: string, to: string) -> u64 or IoError
        fs_methods.push(ModuleMethodSig {
            name: "copy".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Result {
                ok: Box::new(Type::U64),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.rename(from: string, to: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "rename".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.remove(path: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "remove".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.create_dir(path: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "create_dir".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.create_dir_all(path: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "create_dir_all".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });
        // fs.append_file(path: string, content: string) -> () or IoError
        fs_methods.push(ModuleMethodSig {
            name: "append_file".to_string(),
            params: vec![Type::String, Type::String],
            ret: Type::Result {
                ok: Box::new(Type::Unit),
                err: Box::new(io_error_ty.clone()),
            },
        });

        modules.insert("fs".to_string(), fs_methods);

        // net module
        let error_ty = Type::UnresolvedNamed("Error".to_string());
        let mut net_methods = Vec::new();
        // net.tcp_listen(addr: string) -> TcpListener or Error
        net_methods.push(ModuleMethodSig {
            name: "tcp_listen".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("TcpListener".to_string())),
                err: Box::new(error_ty.clone()),
            },
        });
        modules.insert("net".to_string(), net_methods);

        // json module
        let mut json_methods = Vec::new();
        // json.encode(value) -> string (accepts any type)
        json_methods.push(ModuleMethodSig {
            name: "encode".to_string(),
            params: vec![Type::UnresolvedNamed("_Any".to_string())],
            ret: Type::String,
        });
        // json.decode(str: string) -> T or Error (generic, returns fresh var)
        json_methods.push(ModuleMethodSig {
            name: "decode".to_string(),
            params: vec![Type::String],
            ret: Type::Result {
                ok: Box::new(Type::UnresolvedNamed("_JsonDecodeResult".to_string())),
                err: Box::new(error_ty),
            },
        });
        modules.insert("json".to_string(), json_methods);

        Self { modules }
    }

    pub fn get_method(&self, module: &str, method: &str) -> Option<&ModuleMethodSig> {
        self.modules.get(module)?.iter().find(|m| m.name == method)
    }

    pub fn is_module(&self, name: &str) -> bool {
        self.modules.contains_key(name)
    }
}
