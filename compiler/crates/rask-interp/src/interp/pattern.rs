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
                        // Unit struct variant: Value::Struct { name: "Shape.Circle" } with no fields
                        Value::Struct { name: sn, fields, .. } if sn == name && fields.is_empty() => {
                            Some(HashMap::new())
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
                }
                None
            }

            Pattern::Struct {
                name: pat_name,
                fields: pat_fields,
                rest: _,
            } => {
                if let Value::Struct { name, fields, .. } = value {
                    if name == pat_name {
                        let mut bindings = HashMap::new();
                        for (field_name, field_pattern) in pat_fields {
                            if let Some(field_val) = fields.get(field_name) {
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
            (Value::Enum { name: n1, variant: v1, fields: f1 },
             Value::Enum { name: n2, variant: v2, fields: f2 }) => {
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

    /// Compare two runtime values for ordering.
    /// Returns None if the values are not comparable.
    pub(crate) fn value_cmp(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
        match (a, b) {
            (Value::Int(a), Value::Int(b)) => Some(a.cmp(b)),
            (Value::Float(a), Value::Float(b)) => a.partial_cmp(b),
            (Value::String(a), Value::String(b)) => {
                Some(a.lock().unwrap().cmp(&*b.lock().unwrap()))
            }
            (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)), // false < true
            (Value::Char(a), Value::Char(b)) => Some(a.cmp(b)),
            _ => None, // Other types are not comparable
        }
    }
}

