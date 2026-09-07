// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Automated drift detection: verifies the interpreter handles every
//! method registered in rask-stdlib's implementation registry.
//!
//! One test loops over all registered types, constructs a dummy value,
//! and calls each method. If the interpreter returns NoSuchMethod,
//! the method is registered but not implemented — that's a bug.

use indexmap::IndexMap;
use std::sync::{mpsc, Arc, Mutex, RwLock};

use crate::interp::Interpreter;
use crate::value::{FloatKind, ModuleKind, PoolData, ThreadHandleInner, Value};

/// Construct a minimal dummy value for a given type name.
/// Only needs to route to the right dispatch — doesn't need valid data.
fn dummy_value(type_name: &str) -> Value {
    match type_name {
        "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" => Value::int(0),
        "i128" => Value::Int128(0),
        "u128" => Value::Uint128(0),
        "f64" => Value::Float(0.0, FloatKind::F64),
        "bool" => Value::Bool(false),
        "char" => Value::Char('a'),
        "string" => Value::String(Arc::new(Mutex::new(String::new()))),
        "Vec" => Value::vec(vec![]),
        "Map" => Value::Map(Arc::new(Mutex::new(Default::default()))),
        "Pool" => Value::Pool(Arc::new(Mutex::new(PoolData {
            pool_id: 0,
            slots: vec![],
            free_list: vec![],
            len: 0,
            type_param: None,
            capacity: None,
        }))),
        "Handle" => Value::Handle {
            pool_id: 0,
            index: 0,
            generation: 0,
        },
        "Rack" => {
            let rack = Arc::new(Mutex::new(crate::value::RackData::new()));
            crate::value::register_rack(&rack);
            Value::Rack(rack)
        }
        // A link needs a node to point at — that's the whole type.
        "Link" => {
            let rack = Arc::new(Mutex::new(crate::value::RackData::new()));
            crate::value::register_rack(&rack);
            let rack_id = rack.lock().unwrap().rack_id;
            let node = Arc::new(Mutex::new(crate::value::StructData {
                name: "Node".to_string(),
                fields: Default::default(),
                resource_id: None,
            }));
            rack.lock().unwrap().insert(Arc::clone(&node));
            Value::Link { rack_id, node }
        }
        "Result" => Value::Enum {
            name: "Result".to_string(),
            variant: "Ok".to_string(),
            fields: vec![Value::Unit],
            variant_index: 0, origin: None,
        },
        "Option" => Value::Enum {
            name: "Option".to_string(),
            variant: "Some".to_string(),
            fields: vec![Value::Unit],
            variant_index: 0, origin: None,
        },
        "File" => Value::File(Arc::new(Mutex::new(None))),
        "Metadata" => Value::new_struct(
            "Metadata".to_string(),
            IndexMap::new(),
            None,
        ),
        "TcpListener" => Value::TcpListener(Arc::new(Mutex::new(None))),
        "TcpConnection" => Value::TcpConnection(Arc::new(Mutex::new(None))),
        "JsonValue" => Value::Enum {
            name: "JsonValue".to_string(),
            variant: "Null".to_string(),
            fields: vec![],
            variant_index: 0, origin: None,
        },
        "Duration" => Value::Duration(0),
        "Instant" => Value::Instant(std::time::Instant::now()),
        "Path" => {
            let mut fields = IndexMap::new();
            fields.insert(
                "value".to_string(),
                Value::String(Arc::new(Mutex::new("/tmp".to_string()))),
            );
            Value::new_struct(
                "Path".to_string(),
                fields,
                None,
            )
        }
        "Args" => Value::new_struct(
            "Args".to_string(),
            IndexMap::new(),
            None,
        ),
        "ThreadHandle" => Value::ThreadHandle(Arc::new(ThreadHandleInner {
            handle: Mutex::new(None),
            receiver: Mutex::new(None),
            task_id: crate::value::next_task_id(),
        })),
        "TaskHandle" => Value::TaskHandle(Arc::new(ThreadHandleInner {
            handle: Mutex::new(None),
            receiver: Mutex::new(None),
            task_id: crate::value::next_task_id(),
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
        "Mutex" => Value::RaskMutex(Arc::new(Mutex::new(Value::Unit))),
        // Int rather than Unit: `Cell.get`/`replace` hand the payload back, and a
        // Unit payload makes them look unimplemented to this walk.
        "Cell" => Value::Cell(Arc::new(Mutex::new(Value::int(0)))),
        "Atomic" => Value::Atomic(Arc::new(Mutex::new(Value::int(0)))),
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
        // A type written in Rask is implemented once, in `stdlib/*.rk`, and both
        // backends run that source. A Rust implementation here would be a second
        // one — so for these the assertion is inverted.
        if rask_stdlib::registry::is_rask_implemented(type_name) {
            for &method in rask_stdlib::registry::type_method_names(type_name) {
                assert!(
                    !interp.has_method_dispatch(dummy.clone(), method),
                    "{type_name}.{method} is implemented in stdlib/*.rk, but the \
                     interpreter also has a Rust implementation — that's the two \
                     implementations this is meant to prevent"
                );
            }
            continue;
        }
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

// Every raw-pointer method has to answer. `PTR_METHODS` is the one list — the
// checker, MIR lowering and codegen all read it — and the interpreter simply
// had no entry at all, which is how `p.read()` came to be "no method on `i64`"
// (#935). Nothing checked, because the type walk above starts from
// `REGISTERED_TYPES` and a `*T` isn't a registered type.
//
// A method added to the table with a new `PtrSig` is caught by the compiler:
// `call_ptr_method` matches on the sig exhaustively. One added with an existing
// sig but an unhandled *name* is not, and this catches that.
#[test]
fn all_pointer_methods_implemented() {
    use crate::ptr::{call_ptr_method, RawPtr};
    use crate::interp::RuntimeError;

    // Points at a real buffer, so a read has something to read and the answer
    // is a value rather than an out-of-bounds panic.
    let p = RawPtr::bytes(&Arc::new(Mutex::new("ab".to_string())));

    for m in rask_stdlib::PTR_METHODS {
        // Enough arguments for the widest shape; the extras are ignored.
        let args = vec![Value::int(1), Value::RawPtr(p.clone())];
        let args = match m.sig {
            // `write` takes the value to store, not a count.
            rask_stdlib::PtrSig::Write => vec![Value::int(65)],
            // `eq`/`ne` compare against another pointer.
            rask_stdlib::PtrSig::Comparison => vec![Value::RawPtr(p.clone())],
            _ => args,
        };
        match call_ptr_method(&p, m.name, args) {
            Err(RuntimeError::NoSuchMethod { .. }) | Err(RuntimeError::Generic(_)) => panic!(
                "`{}` is in rask-stdlib's PTR_METHODS but the interpreter has no \
                 implementation — add one in rask-interp/src/ptr.rs",
                m.name
            ),
            _ => {}
        }
    }
}

// Every registered module method has to have *an* implementation the
// interpreter can reach. Two count: Rust dispatch in `stdlib/`, or a Rask body
// in `stdlib/*.rk`, which `call_module_method` falls back to. It used to demand
// Rust, which was right when Rust was the only answer — `json.parse` is Rask now
// and that's the whole point of moving it.
#[test]
fn all_registered_module_methods_implemented() {
    let stubs = rask_stdlib::StubRegistry::load();
    let mut interp = Interpreter::new();
    for &module in rask_stdlib::registry::REGISTERED_MODULES {
        let kind = module_kind(module);
        for &method in rask_stdlib::registry::module_method_names(module) {
            let in_rust = interp.has_module_dispatch(&kind, method);
            let in_rask = stubs
                .lookup_method(module, method)
                .map(|m| m.has_body)
                .unwrap_or(false);
            assert!(
                in_rust || in_rask,
                "{module}.{method} is registered in rask-stdlib with no implementation \
                 either side: no Rust dispatch in rask-interp, no Rask body in stdlib/{module}.rk"
            );
        }
    }
}
