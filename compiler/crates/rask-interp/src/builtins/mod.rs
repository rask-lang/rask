// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Built-in type methods (always available, no import needed).
//!
//! Primitives, strings, collections, Result/Option, and threading types.

mod primitives;
mod strings;
mod collections;
mod enums;
mod threading;
mod shared;

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Dispatch a method call on a built-in type.
    /// Returns the result, or falls back to user-defined methods.
    pub(crate) fn call_builtin_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match &receiver {
            Value::Int(a) => return self.call_int_method(*a, method, &args),
            Value::Int128(a) => return self.call_int128_method(*a, method, &args),
            Value::Uint128(a) => return self.call_uint128_method(*a, method, &args),
            Value::Float(a) => return self.call_float_method(*a, method, &args),
            Value::Bool(a) => return self.call_bool_method(*a, method, &args),
            Value::Char(c) => return self.call_char_method(*c, method, &args),
            Value::String(s) => return self.call_string_method(s, method, args),
            Value::Vec(v) => return self.call_vec_method(v, method, args),
            Value::Map(m) => return self.call_map_method(m, method, args),
            Value::Pool(p) => return self.call_pool_method(p, method, args),
            Value::Handle { pool_id, index, generation, .. } => {
                return self.call_handle_method(&receiver, *pool_id, *index, *generation, method, args);
            }
            Value::TypeConstructor(kind) => return self.call_type_constructor_method(kind, method, args),
            Value::Enum { name, variant, fields } if name == "Result" => {
                return self.call_result_method(variant, fields, method, args);
            }
            Value::Enum { name, variant, fields } if name == "Option" => {
                return self.call_option_method(variant, fields, method, args);
            }
            Value::ThreadHandle(handle) => return self.call_thread_handle_method(handle, method),
            Value::Sender(tx) => return self.call_sender_method(tx, method, args),
            Value::Receiver(rx) => return self.call_receiver_method(rx, method),
            Value::AtomicBool(atomic) => return self.call_atomic_bool_method(atomic, method, args),
            Value::AtomicUsize(atomic) => return self.call_atomic_usize_method(atomic, method, args),
            Value::AtomicU64(atomic) => return self.call_atomic_u64_method(atomic, method, args),
            Value::Shared(s) => return self.call_shared_method(&Arc::clone(s), method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::TcpListener(l) => return self.call_tcp_listener_method(&Arc::clone(l), method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::TcpConnection(c) => return self.call_tcp_stream_method(&Arc::clone(c), method, args),
            Value::Enum { .. } if method == "eq" => {
                if let Some(other) = args.first() {
                    if let (Value::Enum { name: n1, variant: v1, fields: f1 },
                            Value::Enum { name: n2, variant: v2, fields: f2 }) = (&receiver, other) {
                        if n1 == n2 && v1 == v2 && f1.len() == f2.len() {
                            let all_eq = f1.iter().zip(f2.iter()).all(|(a, b)| Self::value_eq(a, b));
                            return Ok(Value::Bool(all_eq));
                        }
                        return Ok(Value::Bool(false));
                    }
                }
                return Ok(Value::Bool(false));
            }
            Value::Enum { .. } if method == "ne" => {
                let eq_result = self.call_builtin_method(receiver, "eq", args)?;
                if let Value::Bool(b) = eq_result {
                    return Ok(Value::Bool(!b));
                }
                return Ok(Value::Bool(true));
            }
            Value::Struct { .. } if method == "clone" => return Ok(receiver.deep_clone()),
            Value::Enum { .. } if method == "clone" => return Ok(receiver.deep_clone()),
            _ => {}
        }

        // Generic to_string fallback
        if method == "to_string" {
            return Ok(Value::String(Arc::new(Mutex::new(format!("{}", receiver)))));
        }

        // Generic clone fallback (for types that don't have explicit clone)
        if method == "clone" {
            return Ok(receiver.clone());
        }

        // User-defined methods from extend blocks
        let type_name = match &receiver {
            Value::Struct { name, .. } => name.clone(),
            Value::Enum { name, .. } => name.clone(),
            _ => receiver.type_name().to_string(),
        };

        if let Some(type_methods) = self.methods.get(&type_name) {
            if let Some(method_fn) = type_methods.get(method).cloned() {
                let consumes_self = method_fn.params.first()
                    .map(|p| p.name == "self" && p.is_take)
                    .unwrap_or(false);
                if consumes_self {
                    if let Some(id) = self.get_resource_id(&receiver) {
                        self.resource_tracker.mark_consumed(id)
                            .map_err(|msg| RuntimeError::Panic(msg))?;
                    }
                }
                let mut all_args = vec![receiver];
                all_args.extend(args);
                return self.call_function(&method_fn, all_args);
            }
        }

        Err(RuntimeError::NoSuchMethod {
            ty: type_name,
            method: method.to_string(),
        })
    }
}
