// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Pattern matching and value comparison.

use std::collections::HashMap;

use rask_ast::expr::{Expr, ExprKind, Pattern};

use crate::value::Value;

use super::Interpreter;

impl Interpreter {
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
                // Check if this ident is a known enum variant — match tag only
                if let Value::Enum { variant, .. } = value {
                    let is_known_variant = self.enums.values().any(|e| {
                        e.variants.iter().any(|v| v.name == *name)
                    });
                    if is_known_variant {
                        if variant == name {
                            return Some(HashMap::new());
                        } else {
                            return None;
                        }
                    }
                }
                // ER27: bare type name as match arm on a Result scrutinee —
                // match by payload type. Primitives (`i32`, `f64`, ...) and
                // user type names (`DivError`) match if the Ok/Err payload
                // has that runtime type.
                if let Value::Enum { name: sc_name, fields, .. } = value {
                    if sc_name == "Result" {
                        if let Some(inner) = fields.first() {
                            if runtime_type_matches(inner, name) {
                                return Some(HashMap::new());
                            }
                        }
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
                    (Value::Int(n), ExprKind::Int(s, _), ExprKind::Int(e, _)) => n >= s && n <= e,
                    _ => false,
                };
                if in_range { Some(HashMap::new()) } else { None }
            }

            // ER23/ER27: `TypeName [as name]` type pattern.
            // For Result scrutinees, match either the Ok branch (T side) or
            // Err branch (E side) by inspecting the payload's runtime type.
            Pattern::TypePat { ty_name, binding } => {
                let Value::Enum { name: sc_name, variant, fields, .. } = value else {
                    return None;
                };
                // Only Result (and Option, but TypePat is Result-scoped) match here.
                if sc_name != "Result" {
                    return None;
                }
                let inner = fields.first()?;
                if !runtime_type_matches(inner, ty_name) {
                    return None;
                }
                // Found a match. Must be Ok or Err — both bind the inner.
                let _ = variant;
                let mut bindings = HashMap::new();
                if let Some(n) = binding {
                    bindings.insert(n.clone(), inner.clone());
                }
                Some(bindings)
            }
        }
    }

    pub(super) fn values_equal(&self, value: &Value, lit_expr: &Expr) -> bool {
        match (&value, &lit_expr.kind) {
            (Value::Int(a), ExprKind::Int(b, _)) => *a == *b,
            (Value::Int128(a), ExprKind::Int(b, _)) => *a == *b as i128,
            (Value::Uint128(a), ExprKind::Int(b, _)) => *a == *b as u128,
            (Value::Float(a), ExprKind::Float(b, _)) => *a == *b,
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
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::String(a), Value::String(b)) => *a.lock().unwrap() == *b.lock().unwrap(),
            (Value::Enum { name: n1, variant: v1, fields: f1, .. },
             Value::Enum { name: n2, variant: v2, fields: f2, .. }) => {
                n1 == n2 && v1 == v2 && f1.len() == f2.len()
                    && f1.iter().zip(f2.iter()).all(|(a, b)| Self::value_eq(a, b))
            }
            (Value::Handle { pool_id: p1, index: i1, generation: g1 },
             Value::Handle { pool_id: p2, index: i2, generation: g2 }) => {
                p1 == p2 && i1 == i2 && g1 == g2
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
            Value::Int(n) => n.hash(&mut hasher),
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
            _ => 0u8.hash(&mut hasher),
        }
        hasher.finish()
    }

    /// Compare two runtime values for ordering.
    /// Returns None if the values are not comparable.
    pub(crate) fn value_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
        match (a, b) {
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Int128(a), Value::Int128(b)) => Some(a.cmp(b)),
            (Value::Uint128(a), Value::Uint128(b)) => Some(a.cmp(b)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::String(a), Value::String(b)) => {
                Some(a.lock().unwrap().cmp(&*b.lock().unwrap()))
            }
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)), // false < true
            (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
            // CO3: structs — lexicographic by field declaration order
            // (IndexMap preserves insertion order = declaration order)
            (Value::Struct(ref s1), Value::Struct(ref s2)) => {
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
        Value::Int(_) => matches!(
            ty_name,
            "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64"
                | "int" | "uint" | "isize" | "usize"
        ),
        Value::Float(_) => matches!(ty_name, "f32" | "f64"),
        Value::Enum { name, .. } => name == ty_name,
        Value::Struct(s) => {
            let guard = s.lock().unwrap();
            guard.name == ty_name
        }
        _ => false,
    }
}

