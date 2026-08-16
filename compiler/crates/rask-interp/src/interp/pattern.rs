// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Pattern matching and value comparison.

use std::collections::HashMap;
use std::sync::Arc;

use rask_ast::expr::{Expr, ExprKind, Pattern};

use crate::value::Value;

use super::Interpreter;

impl Interpreter {
    /// Does `name` name a real type (primitive, or a declared struct/enum)?
    /// Used to tell a type-discriminator pattern (`i32`, `DivError`) apart
    /// from a plain variable-binding pattern (`x`) on a Result scrutinee —
    /// only the former should fail the arm outright on a mismatch instead of
    /// falling through to bind.
    fn is_known_type_name(&self, name: &str) -> bool {
        let base = name.split('<').next().unwrap_or(name);
        // The wide set here on purpose: a match arm can name `string`, and the
        // interpreter still accepts the `int`/`uint` spellings.
        rask_ast::primitives::is_builtin_scalar_or_string(base)
            || self.enums.contains_key(base)
            || self.struct_decls.contains_key(base)
            || matches!(base, "Vec" | "Map")
    }

    pub(super) fn match_pattern(&self, pattern: &Pattern, value: &Value) -> Option<HashMap<String, Value>> {
        match pattern {
            Pattern::Wildcard => Some(HashMap::new()),

            Pattern::Ident(name) => {
                // Qualified name: "Message.Quit" → match enum "Message" variant "Quit"
                if let Some(dot) = name.find('.') {
                    let (pat_enum, pat_variant) = (&name[..dot], &name[dot + 1..]);
                    return match value {
                        Value::Enum { name: en, variant, .. }
                            if en == pat_enum && variant == pat_variant =>
                        {
                            Some(HashMap::new())
                        }
                        // ER28: for a Result scrutinee, descend into Ok/Err
                        // and retry against the payload (so arms like
                        // `IoError.NotFound` work against `r: T or IoError`).
                        Value::Enum { name: en, variant, fields, .. }
                            if en == "Result" && matches!(variant.as_str(), "Ok" | "Err") =>
                        {
                            if let Some(inner) = fields.first() {
                                return self.match_pattern(pattern, inner);
                            }
                            None
                        }
                        // Unit struct variant with no fields
                        Value::Struct(ref s) => {
                            let guard = s.lock().unwrap();
                            if guard.name == *name && guard.fields.is_empty() {
                                Some(HashMap::new())
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                }
                // A variant of the scrutinee's own enum — match on the tag.
                //
                // Scoped to that enum on purpose. Asking whether *any* declared
                // enum has a variant by this name makes the answer depend on what
                // else the program happens to declare: `is ParseError` against a
                // `T or ParseError` stopped matching once the stdlib's `JsonError`
                // (which has a `ParseError` variant) was in the table, because
                // the name looked like a variant and the arm compared it against
                // `Err`. The scrutinee's own enum is the only one that can
                // legitimately answer this.
                if let Value::Enum { name: sc_name, variant, .. } = value {
                    let is_own_variant = self
                        .enums
                        .get(sc_name)
                        .is_some_and(|e| e.variants.iter().any(|v| v.name == *name));
                    if is_own_variant {
                        if variant == name {
                            return Some(HashMap::new());
                        } else {
                            return None;
                        }
                    }
                }
                // ER27: bare type name as match arm on a Result or Option
                // scrutinee — match by payload type. Primitives (`i32`,
                // `f64`, ...) and user type names (`DivError`) match if the
                // Ok/Err/Some payload has that runtime type. A name that's a
                // *recognized* type (primitive, or a declared struct/enum)
                // settles the arm outright: match or don't, but never fall
                // through to the generic variable-binding case below, which
                // binds unconditionally and used to let e.g. an `i32` arm
                // catch an Err(DivError) payload just because "i32" looked
                // like an ordinary identifier (#391). Option needs the same
                // treatment as Result (#579) — without it, `x is i32` on a
                // `T?` fell through to the variable-binding case and matched
                // unconditionally, `none` included.
                if let Value::Enum { name: sc_name, fields, .. } = value {
                    if (sc_name == "Result" || sc_name == "Option") && self.is_known_type_name(name) {
                        return match fields.first() {
                            Some(inner) if runtime_type_matches(inner, name) => Some(HashMap::new()),
                            _ => None,
                        };
                    }
                }
                // Not a known variant — treat as variable binding
                let mut bindings = HashMap::new();
                bindings.insert(name.clone(), value.clone());
                Some(bindings)
            }

            Pattern::Literal(lit_expr) => {
                if self.values_equal(value, lit_expr) {
                    Some(HashMap::new())
                } else {
                    None
                }
            }

            Pattern::Constructor { name, fields } => {
                if let Value::Enum {
                    name: enum_name,
                    variant,
                    fields: enum_fields,
                    ..
                } = value
                {
                    // Handle qualified: "Message.Text" → enum "Message", variant "Text"
                    let matches = if let Some(dot) = name.find('.') {
                        let (pat_enum, pat_variant) = (&name[..dot], &name[dot + 1..]);
                        enum_name == pat_enum && variant == pat_variant
                    } else {
                        variant == name
                    };
                    if matches && fields.len() == enum_fields.len() {
                        let mut bindings = HashMap::new();
                        for (pat, val) in fields.iter().zip(enum_fields.iter()) {
                            if let Some(sub_bindings) = self.match_pattern(pat, val) {
                                bindings.extend(sub_bindings);
                            } else {
                                return None;
                            }
                        }
                        return Some(bindings);
                    }
                    // ER28: for a Result scrutinee, descend into Ok/Err and
                    // match against the inner value. Lets `match r { IoError.NotFound(p) => ... }`
                    // work when r: T or IoError.
                    if enum_name == "Result" && matches!(variant.as_str(), "Ok" | "Err") {
                        if let Some(inner) = enum_fields.first() {
                            return self.match_pattern(pattern, inner);
                        }
                    }
                }
                None
            }

            Pattern::Struct {
                name: pat_name,
                fields: pat_fields,
                rest: _,
            } => {
                if let Value::Struct(ref s) = value {
                    let guard = s.lock().unwrap();
                    if guard.name == *pat_name {
                        let mut bindings = HashMap::new();
                        for (field_name, field_pattern) in pat_fields {
                            if let Some(field_val) = guard.fields.get(field_name) {
                                if let Some(sub_bindings) =
                                    self.match_pattern(field_pattern, field_val)
                                {
                                    bindings.extend(sub_bindings);
                                } else {
                                    return None;
                                }
                            } else {
                                return None;
                            }
                        }
                        return Some(bindings);
                    }
                }
                None
            }

            Pattern::Tuple(patterns) => {
                if let Value::Vec(v) = value {
                    let vec = v.lock().unwrap();
                    if patterns.len() == vec.len() {
                        let mut bindings = HashMap::new();
                        for (pat, val) in patterns.iter().zip(vec.iter()) {
                            if let Some(sub_bindings) = self.match_pattern(pat, val) {
                                bindings.extend(sub_bindings);
                            } else {
                                return None;
                            }
                        }
                        return Some(bindings);
                    }
                }
                None
            }

            Pattern::Or(patterns) => {
                for pat in patterns {
                    if let Some(bindings) = self.match_pattern(pat, value) {
                        return Some(bindings);
                    }
                }
                None
            }

            Pattern::Range { start, end } => {
                // Bounds are literal chars or ints, checked by the parser.
                let in_range = match (value, &start.kind, &end.kind) {
                    (Value::Char(c), ExprKind::Char(s), ExprKind::Char(e)) => c >= s && c <= e,
                    (Value::Int(n, _), ExprKind::Int(s, _), ExprKind::Int(e, _)) => n >= s && n <= e,
                    _ => false,
                };
                if in_range { Some(HashMap::new()) } else { None }
            }

            // ER23/ER27: `TypeName [as name]` type pattern, and OPT15's
            // `none`. A flat `T? or E` wears two wrappers, so the walk goes
            // down layer by layer — the pattern names one leaf (OPT30).
            Pattern::TypePat { ty_name, binding } => {
                let mut current = value;
                loop {
                    let Value::Enum { name: sc_name, variant, fields, .. } = current else {
                        return None;
                    };
                    if sc_name != "Result" && sc_name != "Option" {
                        return None;
                    }
                    // The absent branch carries no payload, so it's the
                    // variant itself that answers, not an inner value.
                    if ty_name == "none" {
                        if variant == "None" {
                            return Some(HashMap::new());
                        }
                    }
                    let Some(inner) = fields.first() else { return None };
                    if runtime_type_matches(inner, ty_name) {
                        let mut bindings = HashMap::new();
                        if let Some(n) = binding {
                            bindings.insert(n.clone(), inner.clone());
                        }
                        return Some(bindings);
                    }
                    current = inner;
                }
            }
        }
    }

    pub(super) fn values_equal(&self, value: &Value, lit_expr: &Expr) -> bool {
        match (&value, &lit_expr.kind) {
            (Value::Int(a, _), ExprKind::Int(b, _)) => *a == *b,
            (Value::Int128(a), ExprKind::Int(b, _)) => *a == *b as i128,
            (Value::Uint128(a), ExprKind::Int(b, _)) => *a == *b as u128,
            (Value::Float(a, _), ExprKind::Float(b, _)) => *a == *b,
            (Value::Bool(a), ExprKind::Bool(b)) => *a == *b,
            (Value::Char(a), ExprKind::Char(b)) => *a == *b,
            (Value::String(a), ExprKind::String(b)) => *a.lock().unwrap() == *b,
            _ => false,
        }
    }

    /// Compare two runtime values for equality.
    pub(crate) fn value_eq(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Unit, Value::Unit) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a, _), Value::Int(b, _)) => a == b,
            (Value::Float(a, _), Value::Float(b, _)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            // Locking both sides hangs when they are the same buffer, and a
            // string compared with itself isn't hypothetical: `for mutate (k, v)
            // in map` writes each entry back under the key it just read out, so
            // probe and stored key share one Arc (#738). Same guard the struct
            // arm below already carries.
            (Value::String(a), Value::String(b)) => {
                Arc::ptr_eq(a, b) || *a.lock().unwrap() == *b.lock().unwrap()
            }
            (Value::Enum { name: n1, variant: v1, fields: f1, .. },
             Value::Enum { name: n2, variant: v2, fields: f2, .. }) => {
                n1 == n2 && v1 == v2 && f1.len() == f2.len()
                    && f1.iter().zip(f2.iter()).all(|(a, b)| Self::value_eq(a, b))
            }
            (Value::Handle { pool_id: p1, index: i1, generation: g1 },
             Value::Handle { pool_id: p2, index: i2, generation: g2 }) => {
                p1 == p2 && i1 == i2 && g1 == g2
            }
            // Field-wise, the same shape `value_hash` already uses. Without this
            // two structurally equal structs never compared equal, so a Map keyed
            // by a struct could be inserted into but never read: `m.insert(Id {
            // value: 1 }, …)` then `m.get(Id { value: 1 })` missed every time.
            //
            // This is the auto-derived equality. A type with its own `extend T
            // with Equal` isn't consulted — there's no interpreter to dispatch
            // through here — which is the same limitation the enum arm has.
            (Value::Struct(s1), Value::Struct(s2)) => {
                if Arc::ptr_eq(s1, s2) {
                    return true;
                }
                let a = s1.lock().unwrap();
                let b = s2.lock().unwrap();
                a.name == b.name
                    && a.fields.len() == b.fields.len()
                    && a.fields.iter().all(|(name, av)| {
                        b.fields.get(name).is_some_and(|bv| Self::value_eq(av, bv))
                    })
            }
            // A nominal newtype is its underlying value plus a name (T9), so
            // two of the same type compare by what they wrap. Without this a
            // `Map<UserId, …>` could be inserted into but never read back.
            (Value::Nominal { type_name: n1, inner: i1 },
             Value::Nominal { type_name: n2, inner: i2 }) => {
                n1 == n2 && Self::value_eq(i1, i2)
            }
            _ => false,
        }
    }

    /// Compute a hash for a runtime value (for auto-derived Hashable).
    pub(crate) fn value_hash(value: &Value) -> u64 {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        match value {
            Value::Unit => 0u8.hash(&mut hasher),
            Value::Bool(b) => b.hash(&mut hasher),
            Value::Int(n, _) => n.hash(&mut hasher),
            Value::Int128(n) => n.hash(&mut hasher),
            Value::Uint128(n) => n.hash(&mut hasher),
            Value::Char(c) => c.hash(&mut hasher),
            Value::String(s) => s.lock().unwrap().hash(&mut hasher),
            Value::Enum { name, variant, fields, .. } => {
                name.hash(&mut hasher);
                variant.hash(&mut hasher);
                for f in fields {
                    Self::value_hash(f).hash(&mut hasher);
                }
            }
            Value::Struct(ref s) => {
                let guard = s.lock().unwrap();
                guard.name.hash(&mut hasher);
                for (k, v) in &guard.fields {
                    k.hash(&mut hasher);
                    Self::value_hash(v).hash(&mut hasher);
                }
            }
            Value::Nominal { type_name, inner } => {
                type_name.hash(&mut hasher);
                Self::value_hash(inner).hash(&mut hasher);
            }
            _ => 0u8.hash(&mut hasher),
        }
        hasher.finish()
    }

    /// Compare two runtime values for ordering.
    /// Returns None if the values are not comparable.
    pub(crate) fn value_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
        match (a, b) {
            (Value::Int(a, _), Value::Int(b, _)) => Some(a.cmp(b)),
            (Value::Int128(a), Value::Int128(b)) => Some(a.cmp(b)),
            (Value::Uint128(a), Value::Uint128(b)) => Some(a.cmp(b)),
            // The float total order (type.operators/ORD3): NaN sorts last rather
            // than comparing Equal to everything, which left a NaN in a Vec
            // stopping the sort wherever it happened to sit. The comparison
            // *operators* stay IEEE — that's `call_float_method`, not this.
            (Value::Float(a, _), Value::Float(b, _)) => Some(a.total_cmp(b)),
            // `s <= s` hands the same Arc in twice; locking it a second time
            // deadlocks, so answer from identity first.
            (Value::String(a), Value::String(b)) => {
                if Arc::ptr_eq(a, b) {
                    return Some(std::cmp::Ordering::Equal);
                }
                Some(a.lock().unwrap().cmp(&*b.lock().unwrap()))
            }
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)), // false < true
            (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
            // CO3: structs — lexicographic by field declaration order
            // (IndexMap preserves insertion order = declaration order)
            (Value::Struct(ref s1), Value::Struct(ref s2)) => {
                if Arc::ptr_eq(s1, s2) {
                    return Some(std::cmp::Ordering::Equal);
                }
                let g1 = s1.lock().unwrap();
                let g2 = s2.lock().unwrap();
                for ((_, v1), (_, v2)) in g1.fields.iter().zip(g2.fields.iter()) {
                    match Self::value_cmp(v1, v2) {
                        Some(std::cmp::Ordering::Equal) => continue,
                        other => return other,
                    }
                }
                Some(std::cmp::Ordering::Equal)
            }
            // CO1: enums — variant order first, then payload
            (Value::Enum { variant_index: i1, variant: v1, fields: f1, .. },
             Value::Enum { variant_index: i2, variant: v2, fields: f2, .. }) => {
                if v1 != v2 {
                    return Some(i1.cmp(i2));
                }
                // Same variant — compare payloads lexicographically
                for (a, b) in f1.iter().zip(f2.iter()) {
                    match Self::value_cmp(a, b) {
                        Some(std::cmp::Ordering::Equal) => continue,
                        other => return other,
                    }
                }
                Some(std::cmp::Ordering::Equal)
            }
            _ => None,
        }
    }
}

/// Does the runtime `value` have type `ty_name`?
/// Handles primitives (`i32`, `f64`, `string`, `bool`, `char`) and named
/// enum/struct types. Used by ER27 match type patterns.
fn runtime_type_matches(value: &Value, ty_name: &str) -> bool {
    match value {
        Value::Bool(_) => ty_name == "bool",
        Value::Char(_) => ty_name == "char",
        Value::String(_) => ty_name == "string",
        // `Value::Int` holds the register-width integers; 128-bit ones are
        // `Int128`/`Uint128` and match their own arms.
        Value::Int(_, _) => {
            rask_ast::primitives::is_machine_integer(ty_name)
                || rask_ast::primitives::INT_ALIASES.contains(&ty_name)
        }
        Value::Float(_, _) => rask_ast::primitives::is_float(ty_name),
        Value::Enum { name, .. } => name == ty_name,
        Value::Struct(s) => {
            let guard = s.lock().unwrap();
            guard.name == ty_name
        }
        // Generic containers: compare the base name only (`Vec<i32>` ->
        // `Vec`) — the interpreter doesn't track element types at runtime,
        // so it can't verify `<i32>` matches. rask#217 generic type patterns.
        Value::Vec(_) => ty_name.split('<').next() == Some("Vec"),
        Value::Map(_) => ty_name.split('<').next() == Some("Map"),
        _ => false,
    }
}

