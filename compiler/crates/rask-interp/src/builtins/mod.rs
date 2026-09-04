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
mod iterators;
mod wide;

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

/// Methods the interpreter derives for every struct and enum. An `extend`
/// block that defines one of these replaces the derived version.
const DERIVABLE_METHODS: &[&str] = &[
    "eq", "ne", "lt", "le", "gt", "ge", "compare", "hash", "debug",
];

/// Values whose `.clone()` is just the value again.
///
/// A scalar, an optional, a result, a range, a handle: nothing here owns a heap
/// buffer or a reference count, so a copy is a copy. Their own method handlers
/// have no `clone` arm and answer `NoSuchMethod`, which is fine for hand-written
/// code and wrong for generic code — `T` is whatever the caller instantiated.
fn receiver_takes_generic_clone(receiver: &Value) -> bool {
    match receiver {
        Value::Unit
        | Value::Bool(_)
        | Value::Int(..)
        | Value::Int128(_)
        | Value::Uint128(_)
        | Value::Float(..)
        | Value::Char(_)
        | Value::Range { .. }
        | Value::Duration(_)
        | Value::Instant(_)
        | Value::Handle { .. }
        | Value::WeakHandle { .. } => true,
        Value::Enum { name, .. } => name == "Option" || name == "Result",
        _ => false,
    }
}

impl Interpreter {
    /// Look up a user-written method on a struct or enum value. Enum struct
    /// variants store their name as "Shape.Circle", so the type is also tried
    /// with the variant stripped. Stub registrations have empty bodies and
    /// don't count.
    fn user_method(&self, receiver: &Value, method: &str) -> Option<rask_ast::decl::FnDecl> {
        let type_name = match receiver {
            Value::Struct(s) => s.lock().unwrap().name.clone(),
            Value::Enum { name, .. } => name.clone(),
            Value::Nominal { type_name, .. } => type_name.clone(),
            _ => return None,
        };
        self.methods
            .get(&type_name)
            .and_then(|m| m.get(method).cloned())
            .or_else(|| {
                let base = type_name.split('.').next()?;
                self.methods.get(base).and_then(|m| m.get(method).cloned())
            })
            .filter(|f| !f.body.is_empty())
    }

    /// `__fmt(type, width, precision, align, fill)` — the desugared form of a
    /// `{x:spec}` placeholder. Display goes through the receiver's own
    /// `to_string()` so a user `Displayable` impl is honoured (std.fmt/D4).
    fn render_with_spec(&mut self, receiver: Value, args: &[Value]) -> Result<Value, RuntimeError> {
        let int_at = |i: usize| match args.get(i) {
            Some(Value::Int(n, _)) => *n,
            _ => 0,
        };
        let fill = match args.get(4) {
            Some(Value::Char(c)) => *c,
            _ => ' ',
        };
        let spec = rask_ast::fmt_spec::FormatSpec::decode(
            int_at(0), int_at(1), int_at(2), int_at(3), fill,
        );

        // Debug renders from the value itself and never reads `display`, so
        // asking for `to_string` first is not just wasted work — it fails on
        // the types Debug exists to cover. `{v:debug}` on a `Vec` died with
        // "no method to_string on type Vec" while native printed `[1, 2]`.
        let display = if matches!(spec.ty, rask_ast::fmt_spec::SpecType::Debug) {
            String::new()
        } else {
            match self.call_builtin_method(receiver.clone(), "to_string", vec![])? {
                Value::String(s) => s.lock().unwrap().clone(),
                other => format!("{}", other),
            }
        };
        let rendered = self.render_spec(&receiver, spec, display);
        Ok(Value::String(Arc::new(Mutex::new(rendered))))
    }

    /// Dispatch a method call on a built-in type.
    /// Returns the result, or falls back to user-defined methods.
    pub(crate) fn call_builtin_method(
        &mut self,
        receiver: Value,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        // ER16: .origin() on any value returns the error origin string.
        if method == "origin" {
            let origin_str = match &receiver {
                Value::Enum { origin, .. } => origin.as_ref()
                    .map(|o| o.to_string())
                    .unwrap_or_else(|| "<no origin>".to_string()),
                _ => "<no origin>".to_string(),
            };
            return Ok(Value::String(Arc::new(Mutex::new(origin_str))));
        }

        // `{x:spec}` and `format("{:spec}", x)` both desugar to this. The five
        // arguments are the spec's constants (std.fmt/CM5) — decode, render the
        // value under the spec's type token, then pad.
        if method == "__fmt" && args.len() == 5 {
            return self.render_with_spec(receiver, &args);
        }

        // A user `lt`/`compare`/`eq`/… body wins over the derived one (#400).
        // The derived arms below sit in front of the user-method lookup at the
        // bottom of this function, so without this an `extend` block that
        // defines its own ordering was never called — the interpreter answered
        // from the field-by-field default and disagreed with native.
        if matches!(receiver, Value::Struct(..) | Value::Enum { .. })
            && DERIVABLE_METHODS.contains(&method)
        {
            if let Some(method_fn) = self.user_method(&receiver, method) {
                let mut all_args = vec![receiver];
                all_args.extend(args);
                return self.call_function(&method_fn, all_args).map_err(|diag| diag.error);
            }
        }

        // A narrower receiver against a 128-bit argument widens, the same way
        // the 128-bit methods already widen a narrow argument. `0 - big` is
        // written with a plain literal on the left, and dispatching on the
        // receiver alone sent it to the 64-bit path, which then rejected the
        // argument: "expected int, got i128" for arithmetic the checker had
        // already accepted (#762).
        if let Value::Int(a, k) = &receiver {
            match args.first() {
                Some(Value::Int128(_)) => return self.call_int128_method(*a as i128, method, &args),
                Some(Value::Uint128(_)) if k.is_unsigned() || *a >= 0 => {
                    return self.call_uint128_method(*a as u128, method, &args)
                }
                _ => {}
            }
        }

        // `5 == a` with an optional on the right. Equality is symmetric, and
        // the optional side is the one that knows how to compare against a
        // bare payload — dispatching on the receiver alone sent this to the
        // scalar method, which refused the enum it was handed (#834).
        if matches!(method, "eq" | "ne")
            && args.len() == 1
            && !matches!(&receiver, Value::Enum { name, .. } if name == "Option")
        {
            let opt = match args.first() {
                Some(Value::Enum { name, variant, fields, .. }) if name == "Option" => {
                    Some((variant.clone(), fields.clone()))
                }
                _ => None,
            };
            if let Some((variant, fields)) = opt {
                let eq = self.call_option_method(&variant, &fields, "eq", vec![receiver])?;
                return Ok(match (method, eq) {
                    ("ne", Value::Bool(b)) => Value::Bool(!b),
                    (_, v) => v,
                });
            }
        }

        // `.clone()` on a value whose own method table has no clone arm.
        //
        // Generic code writes the call once and runs it at every instantiation:
        // `self.items[i].clone()` inside a `Bin<T>` is fine at `T = string` and
        // used to be a runtime error at `T = i64`, because the string handler
        // answers `clone` and the scalar ones don't. Native accepts all of them,
        // so the same source was an error on one backend and correct on the
        // other (#1020). Copying a value that is already a copy costs nothing.
        //
        // Listed the safe way round: a type left off this list keeps whatever
        // its own handler does, which is what happens today. Adding one here
        // that has real clone semantics — a `Shared` handing back another
        // reference, a `Vec` copying its elements — would take them away.
        if method == "clone" && args.is_empty() && receiver_takes_generic_clone(&receiver) {
            return Ok(receiver.deep_clone());
        }

        match &receiver {
            Value::Int(a, k) => return self.call_int_method(*a, *k, method, &args),
            Value::Int128(a) => return self.call_int128_method(*a, method, &args),
            Value::Uint128(a) => return self.call_uint128_method(*a, method, &args),
            Value::Float(a, ka) => return self.call_float_method(*a, *ka, method, &args),
            Value::Bool(a) => return self.call_bool_method(*a, method, &args),
            Value::Char(c) => return self.call_char_method(*c, method, &args),
            Value::String(s) => return self.call_string_method(s, method, args),
            Value::Vec(v) => return self.call_vec_method(v, method, args),
            Value::Wide(w) => return self.call_wide_method(w, method, args),
            Value::Map(m) => return self.call_map_method(m, method, args),
            Value::Pool(p) => return self.call_pool_method(p, method, args),
            Value::Rack(s) => return self.call_rack_method(s, method, args),
            Value::Link { rack_id, node } => {
                return self.call_link_method(*rack_id, node, method, args);
            }
            Value::Handle { pool_id, index, generation, .. } => {
                return self.call_handle_method(&receiver, *pool_id, *index, *generation, method, args);
            }
            Value::WeakHandle { pool_id, index, generation } => {
                return self.call_weak_handle_method(*pool_id, *index, *generation, method, args);
            }
            Value::TypeConstructor { kind, type_param } => {
                return self.call_type_constructor_method(kind, type_param.clone(), method, args);
            }
            Value::Enum { name, variant, fields, .. } if name == "Result" => {
                return self.call_result_method(variant, fields, method, args);
            }
            Value::Enum { name, variant, fields, .. } if name == "Option" => {
                return self.call_option_method(variant, fields, method, args);
            }
            Value::ThreadHandle(handle) => return self.call_thread_handle_method(handle, method),
            Value::TaskHandle(handle) => return self.call_task_handle_method(handle, method),
            Value::TaskGroup(tasks) => return self.call_task_group_method(tasks, method, args),
            Value::Sender(tx) => return self.call_sender_method(tx, method, args),
            Value::Receiver(rx) => return self.call_receiver_method(rx, method),
            Value::AtomicBool(atomic) => return self.call_atomic_bool_method(atomic, method, args),
            Value::AtomicUsize(atomic) => return self.call_atomic_usize_method(atomic, method, args),
            Value::AtomicU64(atomic) => return self.call_atomic_u64_method(atomic, method, args),
            Value::Shared(s) => return self.call_shared_method(&Arc::clone(s), method, args),
            Value::RaskMutex(m) => return self.call_mutex_method(&Arc::clone(m), method, args),
            Value::Rng(rng) => return self.call_rng_instance_method(&Arc::clone(rng), method, args),
            Value::StringBuilder(buf) => {
                return self.call_string_builder_method(&Arc::clone(buf), method, args)
            }
            Value::Iterator(iter) => return self.call_iterator_method(&Arc::clone(iter), method, args),
            // ctrl.ranges RV1 / SP1–SP4 — the two range adapters. Both hand
            // back a range, so they chain in either order.
            Value::Range { start, end, inclusive, step, rev } => {
                return match method {
                    "rev" => Ok(Value::Range {
                        start: *start, end: *end, inclusive: *inclusive,
                        step: *step, rev: !*rev,
                    }),
                    "step" => {
                        let n = match args.first() {
                            Some(Value::Int(n, _)) => *n,
                            _ => return Err(RuntimeError::TypeError(
                                "step() takes an integer stride".to_string(),
                            )),
                        };
                        if n == 0 {
                            return Err(RuntimeError::Panic(
                                "ctrl.ranges/SP3: step must be non-zero".to_string(),
                            ));
                        }
                        Ok(Value::Range {
                            start: *start, end: *end, inclusive: *inclusive,
                            step: n, rev: *rev,
                        })
                    }
                    _ => Err(RuntimeError::NoSuchMethod {
                        ty: "Range".to_string(),
                        method: method.to_string(),
                    }),
                };
            }
            #[cfg(not(target_arch = "wasm32"))]
            Value::TcpListener(l) => return self.call_tcp_listener_method(&Arc::clone(l), method, args),
            #[cfg(not(target_arch = "wasm32"))]
            Value::TcpConnection(c) => return self.call_tcp_stream_method(&Arc::clone(c), method, args),
            // TU9: a tuple's derived traits compare, order and hash element by
            // element. It was a `Value::Vec` until #1063, so `(1, 2) == (1, 2)`
            // used to land on `Vec.eq`; now it needs its own arm.
            Value::Tuple(..) if matches!(method, "eq" | "ne") => {
                let eq = args.first().is_some_and(|o| Self::value_eq(&receiver, o));
                return Ok(Value::Bool(if method == "eq" { eq } else { !eq }));
            }
            Value::Tuple(..) if method == "hash" => {
                return Ok(Value::int(Self::value_hash(&receiver) as i64));
            }
            Value::Tuple(..) if method == "compare" => {
                let ord = args.first()
                    .and_then(|o| Self::value_cmp(&receiver, o))
                    .unwrap_or(std::cmp::Ordering::Equal);
                return Ok(Self::ordering_value(ord));
            }
            Value::Enum { .. } if method == "eq" => {
                if let Some(other) = args.first() {
                    if let (Value::Enum { name: n1, variant: v1, fields: f1, .. },
                            Value::Enum { name: n2, variant: v2, fields: f2, .. }) = (&receiver, other) {
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
            Value::Struct(..) if method == "eq" => {
                if let Some(other) = args.first() {
                    if let (Value::Struct(ref s1), Value::Struct(ref s2)) = (&receiver, other) {
                        // `a == a` passes the same Arc twice — locking it again
                        // would deadlock the interpreter.
                        if Arc::ptr_eq(s1, s2) {
                            return Ok(Value::Bool(true));
                        }
                        let g1 = s1.lock().unwrap();
                        let g2 = s2.lock().unwrap();
                        if g1.name == g2.name && g1.fields.len() == g2.fields.len() {
                            let all_eq = g1.fields.iter()
                                .all(|(k, v1)| g2.fields.get(k).map_or(false, |v2| Self::value_eq(v1, v2)));
                            return Ok(Value::Bool(all_eq));
                        }
                        return Ok(Value::Bool(false));
                    }
                }
                return Ok(Value::Bool(false));
            }
            Value::Struct(..) if method == "ne" => {
                let eq_result = self.call_builtin_method(receiver, "eq", args)?;
                if let Value::Bool(b) = eq_result {
                    return Ok(Value::Bool(!b));
                }
                return Ok(Value::Bool(true));
            }
            Value::Struct(..) if method == "hash" => {
                return Ok(Value::int(Self::value_hash(&receiver) as i64));
            }
            Value::Enum { .. } if method == "hash" => {
                return Ok(Value::int(Self::value_hash(&receiver) as i64));
            }
            Value::Struct(..) if method == "compare" => {
                let ord = args.first()
                    .and_then(|other| Self::value_cmp(&receiver, other))
                    .unwrap_or(std::cmp::Ordering::Equal);
                return Ok(Self::ordering_value(ord));
            }
            Value::Enum { .. } if method == "compare" => {
                let ord = args.first()
                    .and_then(|other| Self::value_cmp(&receiver, other))
                    .unwrap_or(std::cmp::Ordering::Equal);
                return Ok(Self::ordering_value(ord));
            }
            // ORD1: lt/le/gt/ge derived from compare via value_cmp
            Value::Struct(..) | Value::Enum { .. }
                if matches!(method, "lt" | "le" | "gt" | "ge") =>
            {
                if let Some(other) = args.first() {
                    let ord = Self::value_cmp(&receiver, other);
                    let result = match (method, ord) {
                        ("lt", Some(std::cmp::Ordering::Less)) => true,
                        ("le", Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)) => true,
                        ("gt", Some(std::cmp::Ordering::Greater)) => true,
                        ("ge", Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)) => true,
                        _ => false,
                    };
                    return Ok(Value::Bool(result));
                }
                return Ok(Value::Bool(false));
            }
            // G2: debug for structs/enums — uses Display impl
            Value::Struct(..) | Value::Enum { .. } if method == "debug" => {
                return Ok(Value::String(Arc::new(Mutex::new(format!("{}", receiver)))));
            }
            Value::Struct(..) if method == "clone" => return Ok(receiver.deep_clone()),
            Value::Enum { .. } if method == "clone" => return Ok(receiver.deep_clone()),
            // E9: `.discriminant()` is the variant's *discriminant value*, and
            // E15 says `Variant = N` assigns that value — so on an enum with
            // explicit values it's N, not the position. This answered the
            // position, so `Opcode.ADD.discriminant()` was 1 here and 6
            // natively, while `Opcode.ADD as i64` was 6 here (E18 reads the
            // declared value) — the same enum giving two different numbers
            // through two spellings of the same question.
            Value::Enum { name, variant, variant_index, .. } if method == "discriminant" => {
                let disc = self
                    .enums
                    .get(name)
                    .and_then(|decl| decl.variants.iter().find(|v| &v.name == variant))
                    .and_then(|v| v.discriminant)
                    .unwrap_or(*variant_index as i128);
                return Ok(Value::int(disc as i64));
            }
            _ => {}
        }

        // Generic to_string fallback — Error→Displayable bridge (D5):
        // If the type has a user-defined message() method, use it for to_string().
        if method == "to_string" {
            let type_name = match &receiver {
                Value::Struct(ref s) => Some(s.lock().unwrap().name.clone()),
                Value::Enum { name, .. } => Some(name.clone()),
                _ => None,
            };
            if let Some(ref tn) = type_name {
                // Check for user-defined to_string first, then message (Error bridge)
                let base_name = tn.find('.').map_or(tn.as_str(), |pos| &tn[..pos]);
                let has_to_string = self.methods.get(tn)
                    .and_then(|m| m.get("to_string"))
                    .or_else(|| self.methods.get(base_name).and_then(|m| m.get("to_string")));
                if let Some(method_fn) = has_to_string.filter(|f| !f.body.is_empty()) {
                    let method_fn = method_fn.clone();
                    return self.call_function(&method_fn, vec![receiver]).map_err(|diag| diag.error);
                }
                let has_message = self.methods.get(tn)
                    .and_then(|m| m.get("message"))
                    .or_else(|| self.methods.get(base_name).and_then(|m| m.get("message")));
                if let Some(method_fn) = has_message.filter(|f| !f.body.is_empty()) {
                    let method_fn = method_fn.clone();
                    return self.call_function(&method_fn, vec![receiver]).map_err(|diag| diag.error);
                }
            }
            return Ok(Value::String(Arc::new(Mutex::new(format!("{}", receiver)))));
        }

        // Generic clone fallback (for types that don't have explicit clone)
        if method == "clone" {
            return Ok(receiver.clone());
        }

        // User-defined methods from extend blocks
        let type_name = match &receiver {
            Value::Struct(ref s) => s.lock().unwrap().name.clone(),
            Value::Enum { name, .. } => name.clone(),
            // A nominal newtype's methods are registered under its own name.
            // Falling back to `type_name()` looked them up under the literal
            // string "nominal" and never found any (#445).
            Value::Nominal { type_name, .. } => type_name.clone(),
            _ => receiver.type_name().to_string(),
        };

        // Enum struct variants store name as "Shape.Circle" — strip variant to find methods under "Shape"
        let resolved_method = self.methods.get(&type_name)
            .and_then(|m| m.get(method).cloned())
            .or_else(|| {
                type_name.find('.').and_then(|pos| {
                    self.methods.get(&type_name[..pos])
                        .and_then(|m| m.get(method).cloned())
                })
            });

        if let Some(method_fn) = resolved_method.filter(|f| !f.body.is_empty()) {
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
            return self.call_function(&method_fn, all_args).map_err(|diag| diag.error);
        }

        // `type Id = u64 with (Hashable)` delegates whatever it doesn't define
        // itself to the underlying value, so anything the newtype doesn't
        // answer is asked of what it wraps.
        if let Value::Nominal { inner, .. } = &receiver {
            // The arguments come down with it. Unwrapping only the receiver
            // left `a == b` on two `Id`s asking an int to compare itself to a
            // nominal, which is a type error the program didn't have (T12).
            let args = args
                .into_iter()
                .map(|a| match a {
                    Value::Nominal { inner, .. } => (*inner).clone(),
                    other => other,
                })
                .collect();
            return self.call_builtin_method((**inner).clone(), method, args);
        }

        Err(RuntimeError::NoSuchMethod {
            ty: type_name,
            method: method.to_string(),
        })
    }

    /// `Ordering` as a value. The variant index is its declaration order —
    /// Less(0), Equal(1), Greater(2) — which is what makes two `Ordering`s
    /// comparable to each other; every one of these used to be built with
    /// index 0.
    fn ordering_value(ord: std::cmp::Ordering) -> Value {
        let (variant, index) = match ord {
            std::cmp::Ordering::Less => ("Less", 0),
            std::cmp::Ordering::Equal => ("Equal", 1),
            std::cmp::Ordering::Greater => ("Greater", 2),
        };
        Value::Enum {
            name: "Ordering".to_string(),
            variant: variant.to_string(),
            fields: vec![],
            variant_index: index,
            origin: None,
        }
    }
}

/// FNV-1a over bytes — what `x.hash()` answers on every scalar type.
///
/// The same function the C runtime's `rask_hash_bytes` computes, so the two
/// backends agree on a value's hash and a value agrees with itself used as a Map
/// key (HA1, #813). Unseeded: a hash has to be as stable as `==`.
pub(crate) fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
