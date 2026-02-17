// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Automated drift detection: verifies the interpreter handles every
//! method registered in rask-stdlib's implementation registry.
//!
//! One test loops over all registered types, constructs a dummy value,
//! and calls each method. If the interpreter returns NoSuchMethod,
//! the method is registered but not implemented — that's a bug.

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Mutex, RwLock};

use crate::interp::Interpreter;
use crate::value::{ModuleKind, PoolData, ThreadHandleInner, Value};

/// Construct a minimal dummy value for a given type name.
/// Only needs to route to the right dispatch — doesn't need valid data.
fn dummy_value(type_name: &str) -> Value {
    match type_name {
        "i64" => Value::Int(0),
        "i128" => Value::Int128(0),
        "u128" => Value::Uint128(0),
        "f64" => Value::Float(0.0),
        "bool" => Value::Bool(false),
        "char" => Value::Char('a'),
        "string" => Value::String(Arc::new(Mutex::new(String::new()))),
        "Vec" => Value::Vec(Arc::new(Mutex::new(vec![]))),
        "Map" => Value::Map(Arc::new(Mutex::new(vec![]))),
        "Pool" => Value::Pool(Arc::new(Mutex::new(PoolData {
            pool_id: 0,
            slots: vec![],
            free_list: vec![],
            len: 0,
            type_param: None,
        }))),
        "Handle" => Value::Handle {
            pool_id: 0,
            index: 0,
            generation: 0,
        },
        "Result" => Value::Enum {
            name: "Result".to_string(),
            variant: "Ok".to_string(),
            fields: vec![Value::Unit],
        },
        "Option" => Value::Enum {
            name: "Option".to_string(),
            variant: "Some".to_string(),
            fields: vec![Value::Unit],
        },
        "File" => Value::File(Arc::new(Mutex::new(None))),
        "Metadata" => Value::Struct {
            name: "Metadata".to_string(),
            fields: HashMap::new(),
            resource_id: None,
        },
        "TcpListener" => Value::TcpListener(Arc::new(Mutex::new(None))),
        "TcpConnection" => Value::TcpConnection(Arc::new(Mutex::new(None))),
        "JsonValue" => Value::Enum {
            name: "JsonValue".to_string(),
            variant: "Null".to_string(),
            fields: vec![],
        },
        "Duration" => Value::Duration(0),
        "Instant" => Value::Instant(std::time::Instant::now()),
        "Path" => {
            let mut fields = HashMap::new();
            fields.insert(
                "value".to_string(),
                Value::String(Arc::new(Mutex::new("/tmp".to_string()))),
            );
            Value::Struct {
                name: "Path".to_string(),
                fields,
                resource_id: None,
            }
        }
        "Args" => Value::Struct {
            name: "Args".to_string(),
            fields: HashMap::new(),
            resource_id: None,
        },
        "ThreadHandle" => Value::ThreadHandle(Arc::new(ThreadHandleInner {
            handle: Mutex::new(None),
            receiver: Mutex::new(None),
        })),
        "Sender" => {
            let (tx, _rx) = mpsc::sync_channel(1);
            Value::Sender(Arc::new(Mutex::new(tx)))
        }
        "Receiver" => {
            let (_tx, rx) = mpsc::sync_channel(1);
            Value::Receiver(Arc::new(Mutex::new(rx)))
        }
        "Shared" => Value::Shared(Arc::new(RwLock::new(Value::Unit))),
        "AtomicBool" => {
            Value::AtomicBool(Arc::new(std::sync::atomic::AtomicBool::new(false)))
        }
        "AtomicUsize" => {
            Value::AtomicUsize(Arc::new(std::sync::atomic::AtomicUsize::new(0)))
        }
        "AtomicU64" => {
            Value::AtomicU64(Arc::new(std::sync::atomic::AtomicU64::new(0)))
        }
        _ => panic!("no dummy value for type '{type_name}'"),
    }
}

/// Map module name to ModuleKind.
fn module_kind(name: &str) -> ModuleKind {
    match name {
        "fs" => ModuleKind::Fs,
        "net" => ModuleKind::Net,
        "json" => ModuleKind::Json,
        "time" => ModuleKind::Time,
        "math" => ModuleKind::Math,
        "random" => ModuleKind::Random,
        "os" => ModuleKind::Os,
        "io" => ModuleKind::Io,
        "cli" => ModuleKind::Cli,
        _ => panic!("unknown module '{name}'"),
    }
}

#[test]
fn all_registered_type_methods_implemented() {
    use rask_stdlib::registry::{is_codegen_only_type, codegen_only_methods};

    let mut interp = Interpreter::new();
    for &type_name in rask_stdlib::registry::REGISTERED_TYPES {
        // Skip types that only exist for native codegen
        if is_codegen_only_type(type_name) {
            continue;
        }
        let dummy = dummy_value(type_name);
        let skip = codegen_only_methods(type_name);
        for &method in rask_stdlib::registry::type_method_names(type_name) {
            if skip.contains(&method) {
                continue;
            }
            assert!(
                interp.has_method_dispatch(dummy.clone(), method),
                "{type_name}.{method} registered in rask-stdlib but interpreter returns NoSuchMethod"
            );
        }
    }
}

#[test]
fn all_registered_module_methods_implemented() {
    let mut interp = Interpreter::new();
    for &module in rask_stdlib::registry::REGISTERED_MODULES {
        let kind = module_kind(module);
        for &method in rask_stdlib::registry::module_method_names(module) {
            assert!(
                interp.has_module_dispatch(&kind, method),
                "{module}.{method} registered in rask-stdlib but interpreter returns NoSuchMethod"
            );
        }
    }
}
