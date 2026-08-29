// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Methods on the string type.
//!
//! Layer: PURE — no OS access, can be compiled from Rask.

use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::ptr::RawPtr;
use crate::value::{FloatKind, IntKind, IteratorState, Value};

impl Interpreter {
    /// Handle string method calls.
    pub(crate) fn call_string_method(
        &self,
        s: &Arc<Mutex<String>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "len" => Ok(Value::int(s.lock().unwrap().len() as i64)),
            "is_empty" => Ok(Value::Bool(s.lock().unwrap().is_empty())),
            "clone" => Ok(Value::String(Arc::clone(s))),
            "starts_with" => {
                let prefix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().starts_with(&prefix)))
            }
            "ends_with" => {
                let suffix = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().ends_with(&suffix)))
            }
            "contains" => {
                let pattern = self.expect_string(&args, 0)?;
                Ok(Value::Bool(s.lock().unwrap().contains(&pattern)))
            }
            "push" | "push_char" => {
                let c = self.expect_char(&args, 0)?;
                s.lock().unwrap().push(c);
                Ok(Value::Unit)
            }
            "push_str" => {
                let other = self.expect_string(&args, 0)?;
                s.lock().unwrap().push_str(&other);
                Ok(Value::Unit)
            }
            // std.strings/V5. A view shares the source's buffer and holds a
            // count on it — here that count is the `Arc`, so the view is a
            // second handle on the same storage. Native reaches the same
            // semantics with a 16-byte copy plus a refcount bump (V1).
            "view" => Ok(Value::String(Arc::clone(s))),
            "trim" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim().to_string()))))
            }
            "trim_start" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim_start().to_string()))))
            }
            "trim_end" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().trim_end().to_string()))))
            }
            // FNV-1a over the bytes — the same function the native runtime
            // and string-keyed maps use, so both backends agree. Typed `u64`,
            // which is what the signature says: as a plain int it rendered the
            // top half of the range as a negative number (#813).
            "hash" => {
                let guard = s.lock().unwrap();
                let h = crate::builtins::fnv1a(guard.as_bytes());
                Ok(Value::Int(h as i64, crate::value::IntKind::U64))
            }
            "to_string" => Ok(Value::String(Arc::clone(s))),
            "debug" => {
                let val = s.lock().unwrap();
                Ok(Value::String(Arc::new(Mutex::new(format!("\"{}\"", val)))))
            }
            // What interpolation desugars to. There is no public `concat` —
            // interpolation is the one way to combine strings (std.strings).
            "__concat" => {
                let other = self.expect_string(&args, 0)?;
                let mut result = s.lock().unwrap().clone();
                result.push_str(&other);
                Ok(Value::String(Arc::new(Mutex::new(result))))
            }
            "to_owned" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().clone()))))
            }
            "to_uppercase" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().to_uppercase()))))
            }
            "to_lowercase" => {
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().to_lowercase()))))
            }
            "split" => {
                let delimiter = self.expect_string(&args, 0)?;
                let parts: Vec<Value> = s
                    .lock().unwrap()
                    .split(&delimiter)
                    .map(|p| Value::String(Arc::new(Mutex::new(p.to_string()))))
                    .collect();
                let state = IteratorState::PreComputed { items: parts, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "split_whitespace" => {
                let parts: Vec<Value> = s
                    .lock().unwrap()
                    .split_whitespace()
                    .map(|part| Value::String(Arc::new(Mutex::new(part.to_string()))))
                    .collect();
                let state = IteratorState::PreComputed { items: parts, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "chars" => {
                let chars: Vec<Value> = s.lock().unwrap().chars().map(Value::Char).collect();
                let state = IteratorState::PreComputed { items: chars, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "char_indices" => {
                let pairs: Vec<Value> = s.lock().unwrap().char_indices()
                    .map(|(i, c)| Value::vec(vec![Value::int(i as i64), Value::Char(c)]))
                    .collect();
                Ok(Value::vec(pairs))
            }
            "bytes" => {
                let bytes: Vec<Value> = s.lock().unwrap().bytes()
                    .map(|b| Value::int(b as i64))
                    .collect();
                Ok(Value::vec(bytes))
            }
            "lines" => {
                let lines: Vec<Value> = s
                    .lock().unwrap()
                    .lines()
                    .map(|l| Value::String(Arc::new(Mutex::new(l.to_string()))))
                    .collect();
                let state = IteratorState::PreComputed { items: lines, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            // No `limit:` yet — default arguments on methods are
            // rask-lang/rask#1028. An explicit limit still works positionally.
            "replace" => {
                let from = self.expect_string(&args, 0)?;
                let to = self.expect_string(&args, 1)?;
                let guard = s.lock().unwrap();
                let out = match args.get(2) {
                    Some(Value::Int(n, _)) => guard.replacen(&from, &to, *n as usize),
                    _ => guard.replace(&from, &to),
                };
                Ok(Value::String(Arc::new(Mutex::new(out))))
            }
            // Display columns (std.strings/U2) — `len` is bytes, `width`
            // aligns. Zero-width for combining marks, two for East Asian wide.
            "width" => Ok(Value::int(display_width(&s.lock().unwrap()) as i64)),
            "graphemes" => {
                let items: Vec<Value> = graphemes(&s.lock().unwrap())
                    .into_iter()
                    .map(|g| Value::String(Arc::new(Mutex::new(g))))
                    .collect();
                let state = IteratorState::PreComputed { items, index: 0 };
                Ok(Value::Iterator(Arc::new(Mutex::new(state))))
            }
            "truncate" => {
                let cols = self.expect_int(&args, 0)?;
                Ok(Value::String(Arc::new(Mutex::new(truncate_to_width(
                    &s.lock().unwrap(),
                    cols,
                )))))
            }
            // NFC (U5). ASCII is already normal, so it short-circuits (U4).
            "normalized" => {
                use unicode_normalization::UnicodeNormalization;
                let guard = s.lock().unwrap();
                let out = if guard.is_ascii() {
                    guard.clone()
                } else {
                    guard.nfc().collect::<String>()
                };
                Ok(Value::String(Arc::new(Mutex::new(out))))
            }
            "is_ascii" => Ok(Value::Bool(s.lock().unwrap().is_ascii())),
            // Internal float route for `parse<f32|f64>()`. Not a user-facing
            // method — the registry lists only `parse` (std.strings) — but the
            // interpreter rewrites a float-targeted parse to this name when the
            // target came from inference rather than an explicit type argument.
            "parse_float" => {
                let text = s.lock().unwrap().trim().to_string();
                match text.parse::<f64>() {
                    Ok(n) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![Value::Float(n, FloatKind::Untyped)],
                        variant_index: 0, origin: None,
                    }),
                    Err(_) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![parse_error(&text)],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            // Generic `parse<T>()`: the type checker injects T's name as arg 0.
            // Floats need a float parse — reading "3.5" as an integer just
            // fails (#480).
            "parse" => {
                let target = match args.first() {
                    Some(Value::String(t)) => t.lock().unwrap().clone(),
                    _ => "i64".to_string(),
                };
                let text = s.lock().unwrap().trim().to_string();
                let parsed = if matches!(target.as_str(), "f32" | "f64") {
                    text.parse::<f64>().ok().map(|f| {
                        let k = FloatKind::from_name(&target).unwrap_or(FloatKind::F64);
                        Value::Float(k.round(f), k)
                    })
                } else if let Some(k) = IntKind::from_name(&target) {
                    // The target's own width, both for the sign and for the
                    // range. Everything went through `i64`: u64::MAX exactly
                    // failed, `"-1".parse<u64>()` succeeded and printed as -1
                    // while native printed 18446744073709551615, and
                    // `"70000".parse<u8>()` succeeded here at 70000 while
                    // native truncated it to 112 (#837).
                    parse_at_width(&text, k)
                } else {
                    text.parse::<i64>().ok().map(Value::int)
                };
                match parsed {
                    Some(v) => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Ok".to_string(),
                        fields: vec![v],
                        variant_index: 0, origin: None,
                    }),
                    None => Ok(Value::Enum {
                        name: "Result".to_string(),
                        variant: "Err".to_string(),
                        fields: vec![parse_error(&text)],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            // Byte offset in, scalar out (std.strings/U1) — none when out of
            // range or when the offset lands inside a character.
            "char_at" => {
                let idx = self.expect_int(&args, 0)? as usize;
                let guard = s.lock().unwrap();
                let found = guard.is_char_boundary(idx).then(|| guard[idx..].chars().next()).flatten();
                Ok(match found {
                    Some(c) => Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::Char(c)],
                        variant_index: 0, origin: None,
                    },
                    None => Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    },
                })
            }
            "byte_at" => {
                let idx = self.expect_int(&args, 0)? as usize;
                match s.lock().unwrap().as_bytes().get(idx) {
                    Some(&b) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::int(b as i64)],
                        variant_index: 0, origin: None,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "index_of" => {
                let pattern = self.expect_string(&args, 0)?;
                match s.lock().unwrap().find(&pattern) {
                    Some(idx) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::int(idx as i64)],
                        variant_index: 0, origin: None,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "last_index_of" => {
                let pattern = self.expect_string(&args, 0)?;
                match s.lock().unwrap().rfind(&pattern) {
                    Some(idx) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::int(idx as i64)],
                        variant_index: 0, origin: None,
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                        variant_index: 0, origin: None,
                    }),
                }
            }
            "repeat" => {
                let n = self.expect_int(&args, 0)? as usize;
                Ok(Value::String(Arc::new(Mutex::new(s.lock().unwrap().repeat(n)))))
            }
            "reverse" => {
                let mut gs = graphemes(&s.lock().unwrap());
                gs.reverse();
                Ok(Value::String(Arc::new(Mutex::new(gs.concat()))))
            }
            "eq" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() == b))
            }
            "ne" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() != b))
            }
            "lt" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() < b))
            }
            "le" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() <= b))
            }
            "gt" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() > b))
            }
            "ge" => {
                let b = self.expect_string(&args, 0)?;
                Ok(Value::Bool(*s.lock().unwrap() >= b))
            }
            "compare" => {
                let b = self.expect_string(&args, 0)?;
                let ord = s.lock().unwrap().cmp(&b);
                Ok(Value::Enum {
                    name: "Ordering".to_string(),
                    variant: match ord {
                        std::cmp::Ordering::Less => "Less".to_string(),
                        std::cmp::Ordering::Equal => "Equal".to_string(),
                        std::cmp::Ordering::Greater => "Greater".to_string(),
                    },
                    fields: vec![],
                    variant_index: 0, origin: None,
                })
            }
            // C interop. The pointer keeps hold of this string's buffer, so
            // `*p` reads its first byte and `p.offset(n)` walks it — the same
            // answers native gives, which used to be a flat 0 here (#935).
            "as_c_str" | "as_ptr" => Ok(Value::RawPtr(RawPtr::bytes(s))),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "string".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// `StringBuilder.new()` / `.with_capacity(n)`.
    pub(crate) fn call_string_builder_type_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "new" => Ok(Value::StringBuilder(Arc::new(Mutex::new(String::new())))),
            "with_capacity" => {
                let n = match args.first() {
                    Some(v) => v.as_int().map_err(RuntimeError::TypeError)?,
                    None => return Err(RuntimeError::ArityMismatch { expected: 1, got: 0 }),
                };
                let cap = if n > 0 { n as usize } else { 0 };
                Ok(Value::StringBuilder(Arc::new(Mutex::new(String::with_capacity(cap)))))
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "StringBuilder".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// StringBuilder instance methods. `build` takes self, so the buffer is
    /// drained rather than copied — the builder is not usable afterwards.
    pub(crate) fn call_string_builder_method(
        &self,
        buf: &Arc<Mutex<String>>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "push" => {
                let s = match args.first() {
                    Some(Value::String(s)) => s.lock().unwrap().clone(),
                    Some(v) => v.to_string(),
                    None => return Err(RuntimeError::ArityMismatch { expected: 1, got: 0 }),
                };
                buf.lock().unwrap().push_str(&s);
                Ok(Value::Unit)
            }
            "push_char" => {
                let c = match args.first() {
                    Some(Value::Char(c)) => *c,
                    Some(v) => {
                        let cp = v.as_int().map_err(RuntimeError::TypeError)?;
                        char::from_u32(cp as u32).ok_or_else(|| {
                            RuntimeError::TypeError(format!("{} is not a Unicode scalar", cp))
                        })?
                    }
                    None => return Err(RuntimeError::ArityMismatch { expected: 1, got: 0 }),
                };
                buf.lock().unwrap().push(c);
                Ok(Value::Unit)
            }
            "build" => {
                let built = std::mem::take(&mut *buf.lock().unwrap());
                Ok(Value::String(Arc::new(Mutex::new(built))))
            }
            "len" => Ok(Value::int(buf.lock().unwrap().len() as i64)),
            "is_empty" => Ok(Value::Bool(buf.lock().unwrap().is_empty())),
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "StringBuilder".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

/// Parse into a specific integer width, refusing anything the width can't
/// hold. An unsigned target parses unsigned, so it reaches `u64::MAX` and
/// refuses a leading `-`.
fn parse_at_width(text: &str, kind: IntKind) -> Option<Value> {
    let bits = kind.bits().unwrap_or(64);
    if kind.is_unsigned() {
        let n = text.parse::<u64>().ok()?;
        if bits < 64 && n > (u64::MAX >> (64 - bits)) {
            return None;
        }
        Some(Value::Int(n as i64, kind))
    } else {
        let n = text.parse::<i64>().ok()?;
        if bits < 64 {
            let hi = (1i64 << (bits - 1)) - 1;
            if n > hi || n < -hi - 1 {
                return None;
            }
        }
        Some(Value::Int(n, kind))
    }
}

/// Build a `ParseError` variant for a failed `parse<T>()`.
/// Mirrors stdlib/string.rk's `ParseError` (Empty/Invalid/OutOfRange).
fn parse_error(text: &str) -> Value {
    let variant = if text.is_empty() {
        "Empty"
    } else if text.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '+' || c == '.') {
        "OutOfRange"
    } else {
        "Invalid"
    };
    Value::Enum {
        name: "ParseError".to_string(),
        variant: variant.to_string(),
        fields: vec![],
        variant_index: match variant { "Empty" => 0, "Invalid" => 1, _ => 2 },
        origin: None,
    }
}

// ─── Text units (std.strings/U1–U5) ────────────────────────────────────────
//
// Straight from the crates. The interpreter is the reference for what the
// answer should be, and the native runtime's tables are generated from these
// same crates by scripts/gen_unicode_text — so there is nothing here for a
// human to keep in step with anything.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// Display columns (U2). ASCII is the printable byte count (U4) — the common
/// case never touches a table.
pub(crate) fn display_width(s: &str) -> usize {
    if s.is_ascii() {
        return s.bytes().filter(|b| (0x20..0x7F).contains(b)).count();
    }
    s.width()
}

/// Grapheme clusters — what a reader calls characters.
pub(crate) fn graphemes(s: &str) -> Vec<String> {
    s.graphemes(true).map(str::to_string).collect()
}

/// Cut to at most `cols` display columns, never inside a grapheme (U2).
pub(crate) fn truncate_to_width(s: &str, cols: i64) -> String {
    if cols <= 0 {
        return String::new();
    }
    let cols = cols as usize;
    let mut used = 0;
    let mut out = String::new();
    for g in s.graphemes(true) {
        let w = display_width(g);
        if used + w > cols {
            break;
        }
        used += w;
        out.push_str(g);
    }
    out
}
