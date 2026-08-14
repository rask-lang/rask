// SPDX-License-Identifier: (MIT OR Apache-2.0)
#![allow(dead_code)]
//! JSON module methods (json.*).
//!
//! Layer: PURE — custom recursive-descent parser, no OS access.
//!
//! Provides: json.parse(), json.stringify(), json.stringify_pretty(),
//! json.encode(struct), json.decode<T>(string).

use std::collections::HashMap;
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

use rask_ast::decl::{field_attrs, StructDecl};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::{FloatKind, MapData, MapKey, Value};

impl Interpreter {
    /// Handle json module methods.
    pub(crate) fn call_json_method(
        &mut self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        // A JsonValue already has a Rask encoder, and native runs it. Running
        // the Rust one below instead put two encoders behind one call, and they
        // disagreed: the Rust one walks the insertion-ordered backing store, so
        // interp printed object keys in insertion order while native printed
        // seeded order (determinism/D7). Same source both backends now; the
        // Rust path is for what has no Rask version — a struct, encoded by
        // reflection.
        if matches!(method, "encode" | "encode_pretty") {
            if let Some(Value::Enum { name, .. }) = args.first() {
                if name == "JsonValue" {
                    let recv = args[0].clone();
                    let body = if method == "encode_pretty" {
                        "to_string_pretty"
                    } else {
                        "to_string"
                    };
                    return self.call_rask_method("JsonValue", body, recv, vec![]);
                }
            }
        }

        // `json.parse`, `json.stringify` and `json.stringify_pretty` used to be
        // handled here. std.json has one verb pair and no parse/stringify family
        // (specs/stdlib/json.md), so nothing could call them from Rask; the
        // untyped path is `json.parse` in stdlib/json.rk, which both backends
        // run. What's left below needs a struct declaration to work from, which
        // is why it's still Rust.
        match method {
            "encode" => {
                // Encode a struct to JSON string
                let value = args
                    .into_iter()
                    .next()
                    .ok_or(RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                let json_val = value_to_json(&value, &self.struct_decls)?;
                let s = stringify_value(&json_val, false, 0);
                Ok(Value::String(Arc::new(Mutex::new(s))))
            }
            "encode_pretty" => {
                let value = args
                    .into_iter()
                    .next()
                    .ok_or(RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                let json_val = value_to_json(&value, &self.struct_decls)?;
                let s = stringify_value(&json_val, true, 0);
                Ok(Value::String(Arc::new(Mutex::new(s))))
            }
            "to_value" => {
                let value = args
                    .into_iter()
                    .next()
                    .ok_or(RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                value_to_json(&value, &self.struct_decls)
            }
            "decode" => {
                // decode(type_name, json_string) — type_name injected from type_args
                if args.len() < 2 {
                    return Err(RuntimeError::ArityMismatch {
                        expected: 2,
                        got: args.len(),
                    });
                }
                let type_name = self.expect_string(&args, 0)?;
                let input = self.expect_string(&args, 1)?;
                // The untyped path is a Rask function — `json.parse` in
                // stdlib/json.rk, which is also what native calls. Its
                // JsonParser is the grammar; the Rust one below is only still
                // here for the typed path, where the target's fields come from
                // struct declarations rather than from the text.
                if type_name == "JsonValue" {
                    let text = Value::String(Arc::new(Mutex::new(input)));
                    return self.call_rask_static("json", "parse", vec![text]);
                }
                let parsed = match parse_json(&input) {
                    Ok(v) => v,
                    Err(e) => return Ok(make_result_err(JsonErrKind::Parse, &e)),
                };
                match json_to_typed(&parsed, &type_name, "", &self.struct_decls) {
                    Ok(value) => Ok(make_result_ok(value)),
                    Err(e) => Ok(make_result_err(e.kind, &e.message)),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "json".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle methods on JsonValue enum instances.
    pub(crate) fn call_json_value_method(
        &self,
        variant: &str,
        fields: &[Value],
        method: &str,
    ) -> Result<Value, RuntimeError> {
        match method {
            "is_null" => Ok(Value::Bool(variant == "Null")),
            "as_bool" => match (variant, fields.first()) {
                ("Bool", Some(v)) => Ok(option_some(v.clone())),
                _ => Ok(option_none()),
            },
            "as_number" => match (variant, fields.first()) {
                ("Number", Some(v)) => Ok(option_some(v.clone())),
                _ => Ok(option_none()),
            },
            "as_string" => match (variant, fields.first()) {
                ("String", Some(v)) => Ok(option_some(v.clone())),
                _ => Ok(option_none()),
            },
            "as_array" => match (variant, fields.first()) {
                ("Array", Some(v)) => Ok(option_some(v.clone())),
                _ => Ok(option_none()),
            },
            "as_object" => match (variant, fields.first()) {
                ("Object", Some(v)) => Ok(option_some(v.clone())),
                _ => Ok(option_none()),
            },
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "JsonValue".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// `extend JsonError` from stdlib/json.rk. The interpreter builds these
    /// values itself rather than running the stdlib source, so the method has
    /// to live here too — keep the wording in step with the .rk file.
    pub(crate) fn call_json_error_method(
        &self,
        variant: &str,
        fields: &[Value],
        method: &str,
    ) -> Result<Value, RuntimeError> {
        if method != "message" {
            return Err(RuntimeError::NoSuchMethod {
                ty: "JsonError".to_string(),
                method: method.to_string(),
            });
        }
        let detail = match fields.first() {
            Some(Value::String(s)) => s.lock().unwrap().clone(),
            _ => String::new(),
        };
        let msg = match variant {
            "ParseError" => format!("parse error: {}", detail),
            "TypeError" => format!("type error: {}", detail),
            "MissingField" => format!("missing field: {}", detail),
            other => format!("{}: {}", other, detail),
        };
        Ok(Value::String(Arc::new(Mutex::new(msg))))
    }
}

// ─── JSON Parser (recursive descent) ───

struct JsonParser<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            input: input.as_bytes(),
            pos: 0,
        }
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() {
            match self.input[self.pos] {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                _ => break,
            }
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.get(self.pos).copied()
    }

    fn advance(&mut self) -> Option<u8> {
        let c = self.input.get(self.pos).copied()?;
        self.pos += 1;
        Some(c)
    }

    fn expect(&mut self, expected: u8) -> Result<(), String> {
        match self.advance() {
            Some(c) if c == expected => Ok(()),
            Some(c) => Err(format!(
                "expected '{}', found '{}' at byte {}",
                expected as char, c as char, self.pos - 1
            )),
            None => Err(format!("unexpected end of input, expected '{}'", expected as char)),
        }
    }

    fn parse_value(&mut self) -> Result<Value, String> {
        self.skip_whitespace();
        match self.peek() {
            Some(b'"') => self.parse_string_value(),
            Some(b'{') => self.parse_object(),
            Some(b'[') => self.parse_array(),
            Some(b't') | Some(b'f') => self.parse_bool(),
            Some(b'n') => self.parse_null(),
            Some(c) if c == b'-' || c.is_ascii_digit() => self.parse_number(),
            Some(c) => Err(format!("unexpected character '{}' at byte {}", c as char, self.pos)),
            None => Err("unexpected end of input".to_string()),
        }
    }

    fn parse_string_raw(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let mut s = String::new();
        loop {
            match self.advance() {
                Some(b'"') => return Ok(s),
                Some(b'\\') => {
                    match self.advance() {
                        Some(b'"') => s.push('"'),
                        Some(b'\\') => s.push('\\'),
                        Some(b'/') => s.push('/'),
                        Some(b'b') => s.push('\u{08}'),
                        Some(b'f') => s.push('\u{0C}'),
                        Some(b'n') => s.push('\n'),
                        Some(b'r') => s.push('\r'),
                        Some(b't') => s.push('\t'),
                        Some(b'u') => {
                            let hex = self.parse_hex4()?;
                            if let Some(c) = char::from_u32(hex) {
                                s.push(c);
                            } else {
                                s.push('\u{FFFD}');
                            }
                        }
                        Some(c) => return Err(format!("invalid escape '\\{}' at byte {}", c as char, self.pos)),
                        None => return Err("unexpected end of input in string escape".to_string()),
                    }
                }
                Some(c) => s.push(c as char),
                None => return Err("unexpected end of input in string".to_string()),
            }
        }
    }

    fn parse_hex4(&mut self) -> Result<u32, String> {
        let mut val = 0u32;
        for _ in 0..4 {
            match self.advance() {
                Some(c) if c.is_ascii_hexdigit() => {
                    val = val * 16
                        + match c {
                            b'0'..=b'9' => (c - b'0') as u32,
                            b'a'..=b'f' => (c - b'a' + 10) as u32,
                            b'A'..=b'F' => (c - b'A' + 10) as u32,
                            _ => unreachable!(),
                        };
                }
                _ => return Err(format!("bad \\u escape at byte {}", self.pos)),
            }
        }
        Ok(val)
    }

    fn parse_string_value(&mut self) -> Result<Value, String> {
        let s = self.parse_string_raw()?;
        Ok(make_json_string(&s))
    }

    fn parse_number(&mut self) -> Result<Value, String> {
        let start = self.pos;
        // Optional minus
        if self.peek() == Some(b'-') {
            self.advance();
        }
        // Integer part
        if self.peek() == Some(b'0') {
            self.advance();
        } else {
            if !self.peek().map_or(false, |c| c.is_ascii_digit()) {
                return Err(format!("expected a digit at byte {}", self.pos));
            }
            while self.peek().map_or(false, |c| c.is_ascii_digit()) {
                self.advance();
            }
        }
        // Fractional part
        if self.peek() == Some(b'.') {
            self.advance();
            while self.peek().map_or(false, |c| c.is_ascii_digit()) {
                self.advance();
            }
        }
        // Exponent
        if self.peek() == Some(b'e') || self.peek() == Some(b'E') {
            self.advance();
            if self.peek() == Some(b'+') || self.peek() == Some(b'-') {
                self.advance();
            }
            while self.peek().map_or(false, |c| c.is_ascii_digit()) {
                self.advance();
            }
        }
        let num_str = std::str::from_utf8(&self.input[start..self.pos])
            .map_err(|_| "invalid UTF-8 in number".to_string())?;
        let n: f64 = num_str
            .parse()
            .map_err(|_| format!("invalid number: {}", num_str))?;
        Ok(make_json_number(n))
    }

    fn parse_bool(&mut self) -> Result<Value, String> {
        if self.input[self.pos..].starts_with(b"true") {
            self.pos += 4;
            Ok(make_json_bool(true))
        } else if self.input[self.pos..].starts_with(b"false") {
            self.pos += 5;
            Ok(make_json_bool(false))
        } else {
            Err(format!("unexpected character '{}' at byte {}", self.peek().unwrap_or(b'?') as char, self.pos))
        }
    }

    fn parse_null(&mut self) -> Result<Value, String> {
        if self.input[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(make_json_null())
        } else {
            Err(format!("unexpected character '{}' at byte {}", self.peek().unwrap_or(b'?') as char, self.pos))
        }
    }

    fn parse_array(&mut self) -> Result<Value, String> {
        self.expect(b'[')?;
        self.skip_whitespace();
        let mut items: Vec<Value> = Vec::new();
        if self.peek() == Some(b']') {
            self.advance();
            return Ok(make_json_array(items));
        }
        loop {
            let val = self.parse_value()?;
            items.push(val);
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.advance();
                }
                Some(b']') => {
                    self.advance();
                    return Ok(make_json_array(items));
                }
                _ => return Err(format!("expected ',' or ']' at byte {}", self.pos)),
            }
        }
    }

    fn parse_object(&mut self) -> Result<Value, String> {
        self.expect(b'{')?;
        self.skip_whitespace();
        let mut entries: Vec<(String, Value)> = Vec::new();
        if self.peek() == Some(b'}') {
            self.advance();
            return Ok(make_json_object(entries));
        }
        loop {
            self.skip_whitespace();
            let key = self.parse_string_raw()?;
            self.skip_whitespace();
            self.expect(b':')?;
            let val = self.parse_value()?;
            entries.push((key, val));
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.advance();
                }
                Some(b'}') => {
                    self.advance();
                    return Ok(make_json_object(entries));
                }
                _ => return Err(format!("expected ',' or '}}' at byte {}", self.pos)),
            }
        }
    }
}

/// Parse a JSON string into a JsonValue enum.
fn parse_json(input: &str) -> Result<Value, String> {
    let mut parser = JsonParser::new(input);
    let value = parser.parse_value()?;
    parser.skip_whitespace();
    if parser.pos < parser.input.len() {
        return Err(format!(
            "trailing content after the JSON value at byte {}",
            parser.pos
        ));
    }
    Ok(value)
}

// ─── JSON Stringification ───

/// Stringify a JsonValue (or any Value) into JSON.
fn stringify_value(value: &Value, pretty: bool, indent: usize) -> String {
    match value {
        Value::Enum { name, variant, fields, .. } if name == "JsonValue" => {
            stringify_json_variant(variant, fields, pretty, indent)
        }
        // Also handle raw Rask values directly (for json.encode)
        Value::Unit => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n, _) => n.to_string(),
        Value::Float(f, _) => {
            if f.is_nan() {
                "null".to_string() // JSON has no NaN
            } else if f.is_infinite() {
                "null".to_string() // JSON has no Infinity
            } else if *f == f.floor() && f.abs() < 1e15 {
                format!("{:.0}", f) // Print integer-valued floats without decimal
            } else {
                f.to_string()
            }
        }
        Value::String(s) => {
            let s = s.lock().unwrap();
            escape_json_string(&s)
        }
        Value::Vec(v) => {
            let vec = v.lock().unwrap();
            if vec.is_empty() {
                return "[]".to_string();
            }
            if pretty {
                let mut s = "[\n".to_string();
                for (i, item) in vec.iter().enumerate() {
                    s.push_str(&"  ".repeat(indent + 1));
                    s.push_str(&stringify_value(item, true, indent + 1));
                    if i < vec.len() - 1 {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&"  ".repeat(indent));
                s.push(']');
                s
            } else {
                let items: Vec<String> = vec
                    .iter()
                    .map(|v| stringify_value(v, false, 0))
                    .collect();
                format!("[{}]", items.join(","))
            }
        }
        Value::Struct(ref s) => {
            let guard = s.lock().unwrap();
            if guard.fields.is_empty() {
                return "{}".to_string();
            }
            // Declaration order, not alphabetical. `fields` is an IndexMap, so
            // its own order is the order the struct declares — which is what
            // native emits, and what encoding.md's `comptime for` walk produces.
            // Sorting here made the same struct encode two different ways
            // depending on which backend ran it (#540).
            let sorted_keys: Vec<&String> = guard.fields.keys().collect();
            if pretty {
                let mut s = "{\n".to_string();
                for (i, key) in sorted_keys.iter().enumerate() {
                    let val = &guard.fields[*key];
                    s.push_str(&"  ".repeat(indent + 1));
                    s.push_str(&escape_json_string(key));
                    s.push_str(": ");
                    s.push_str(&stringify_value(val, true, indent + 1));
                    if i < sorted_keys.len() - 1 {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&"  ".repeat(indent));
                s.push('}');
                s
            } else {
                let pairs: Vec<String> = sorted_keys
                    .iter()
                    .map(|k| {
                        format!(
                            "{}:{}",
                            escape_json_string(k),
                            stringify_value(&guard.fields[*k], false, 0)
                        )
                    })
                    .collect();
                format!("{{{}}}", pairs.join(","))
            }
        }
        Value::Map(m) => {
            let map = m.lock().unwrap();
            if map.is_empty() {
                return "{}".to_string();
            }
            let key_of = |k: &MapKey| match &k.0 {
                Value::String(s) => escape_json_string(&s.lock().unwrap()),
                other => escape_json_string(&format!("{}", other)),
            };
            if pretty {
                let mut s = "{\n".to_string();
                for (i, (k, v)) in map.iter().enumerate() {
                    s.push_str(&"  ".repeat(indent + 1));
                    s.push_str(&key_of(k));
                    s.push_str(": ");
                    s.push_str(&stringify_value(v, true, indent + 1));
                    if i < map.len() - 1 {
                        s.push(',');
                    }
                    s.push('\n');
                }
                s.push_str(&"  ".repeat(indent));
                s.push('}');
                s
            } else {
                let pairs: Vec<String> = map
                    .iter()
                    .map(|(k, v)| format!("{}:{}", key_of(k), stringify_value(v, false, 0)))
                    .collect();
                format!("{{{}}}", pairs.join(","))
            }
        }
        Value::Enum { name, variant, fields, .. } if name == "Option" => {
            match variant.as_str() {
                "Some" => stringify_value(fields.first().unwrap_or(&Value::Unit), pretty, indent),
                "None" => "null".to_string(),
                _ => "null".to_string(),
            }
        }
        _ => "null".to_string(),
    }
}

fn stringify_json_variant(variant: &str, fields: &[Value], pretty: bool, indent: usize) -> String {
    match variant {
        "Null" => "null".to_string(),
        "Bool" => {
            if let Some(Value::Bool(b)) = fields.first() {
                b.to_string()
            } else {
                "false".to_string()
            }
        }
        "Number" => {
            if let Some(Value::Float(f, _)) = fields.first() {
                if f.is_nan() || f.is_infinite() {
                    "null".to_string()
                } else if *f == f.floor() && f.abs() < 1e15 {
                    format!("{:.0}", f)
                } else {
                    f.to_string()
                }
            } else if let Some(Value::Int(n, _)) = fields.first() {
                n.to_string()
            } else {
                "0".to_string()
            }
        }
        "String" => {
            if let Some(Value::String(s)) = fields.first() {
                escape_json_string(&s.lock().unwrap())
            } else {
                "\"\"".to_string()
            }
        }
        "Array" => {
            if let Some(arr) = fields.first() {
                stringify_value(arr, pretty, indent)
            } else {
                "[]".to_string()
            }
        }
        "Object" => {
            // Object wraps a Map (represented as Vec<[key, value]> or similar)
            if let Some(map) = fields.first() {
                stringify_value(map, pretty, indent)
            } else {
                "{}".to_string()
            }
        }
        _ => "null".to_string(),
    }
}

/// Escape a string for JSON output.
fn escape_json_string(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 2);
    result.push('"');
    for c in s.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            '\u{08}' => result.push_str("\\b"),
            '\u{0C}' => result.push_str("\\f"),
            c if (c as u32) < 0x20 => {
                result.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => result.push(c),
        }
    }
    result.push('"');
    result
}

// ─── Value Conversion ───

/// Convert a Rask Value (struct, vec, etc.) into a JsonValue enum value.
fn value_to_json(
    value: &Value,
    struct_decls: &HashMap<String, StructDecl>,
) -> Result<Value, RuntimeError> {
    match value {
        Value::Unit => Ok(make_json_null()),
        Value::Bool(b) => Ok(make_json_bool(*b)),
        Value::Int(n, _) => Ok(make_json_number(*n as f64)),
        Value::Float(f, _) => Ok(make_json_number(*f)),
        Value::String(s) => Ok(make_json_string(&s.lock().unwrap())),
        Value::Vec(v) => {
            let vec = v.lock().unwrap();
            let items: Result<Vec<Value>, RuntimeError> =
                vec.iter().map(|v| value_to_json(v, struct_decls)).collect();
            Ok(make_json_array(items?))
        }
        Value::Struct(ref s) => {
            let guard = s.lock().unwrap();
            // The declaration carries @rename/@skip; the value only knows field
            // names, so the key each field serializes under is looked up here.
            let decl = struct_decls.get(&guard.name);
            let mut entries = Vec::with_capacity(guard.fields.len());
            for (k, v) in guard.fields.iter() {
                if k.starts_with('_') {
                    continue; // internal field
                }
                let attrs = decl
                    .and_then(|d| d.fields.iter().find(|f| f.name == *k))
                    .map(|f| f.attrs.as_slice())
                    .unwrap_or(&[]);
                if field_attrs::is_skipped(attrs) {
                    continue;
                }
                entries.push((
                    field_attrs::serial_name(attrs, k),
                    value_to_json(v, struct_decls)?,
                ));
            }
            Ok(make_json_object(entries))
        }
        Value::Map(m) => {
            let map = m.lock().unwrap();
            let mut entries = Vec::with_capacity(map.len());
            for (k, v) in map.iter() {
                let key = match &k.0 {
                    Value::String(s) => s.lock().unwrap().clone(),
                    other => format!("{}", other),
                };
                entries.push((key, value_to_json(v, struct_decls)?));
            }
            Ok(make_json_object(entries))
        }
        Value::Enum { name, variant, fields, .. } if name == "Option" => {
            match variant.as_str() {
                "Some" if !fields.is_empty() => value_to_json(&fields[0], struct_decls),
                _ => Ok(make_json_null()),
            }
        }
        Value::Enum { name, variant, fields, variant_index, .. } if name == "JsonValue" => {
            // Already a JsonValue, return as-is
            Ok(Value::Enum {
                name: name.clone(),
                variant: variant.clone(),
                fields: fields.clone(),
                variant_index: *variant_index, origin: None,
            })
        }
        _ => Err(RuntimeError::TypeError(format!(
            "cannot convert {} to JSON",
            value.type_name()
        ))),
    }
}

// ─── JsonValue Constructors ───

fn make_json_null() -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Null".to_string(),
        fields: vec![],
        variant_index: 0, origin: None,
    }
}

fn make_json_bool(b: bool) -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Bool".to_string(),
        fields: vec![Value::Bool(b)],
        variant_index: 1, origin: None,
    }
}

fn make_json_number(n: f64) -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Number".to_string(),
        fields: vec![Value::Float(n, FloatKind::Untyped)],
        variant_index: 2, origin: None,
    }
}

fn make_json_string(s: &str) -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "String".to_string(),
        fields: vec![Value::String(Arc::new(Mutex::new(s.to_string())))],
        variant_index: 3, origin: None,
    }
}

fn make_json_array(items: Vec<Value>) -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Array".to_string(),
        fields: vec![Value::Vec(Arc::new(Mutex::new(items)))],
        variant_index: 4, origin: None,
    }
}

fn make_json_object(entries: Vec<(String, Value)>) -> Value {
    // A real Map, not a struct that looks like one. The struct stand-in meant
    // `value.as_object()` handed back something with no `get` on it.
    // Last value wins for a repeated key (J5) — `insert` already does that.
    let mut pairs = MapData::with_capacity(entries.len());
    for (k, v) in entries {
        pairs.insert(MapKey(Value::String(Arc::new(Mutex::new(k)))), v);
    }
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Object".to_string(),
        fields: vec![Value::Map(Arc::new(Mutex::new(pairs)))],
        variant_index: 5, origin: None,
    }
}

// ─── JSON Decode (typed deserialization) ───

/// Which JsonError variant a failure maps to.
#[derive(Clone, Copy, PartialEq)]
pub(crate) enum JsonErrKind {
    Parse,
    Type,
    Missing,
}

impl JsonErrKind {
    fn variant(self) -> (&'static str, u32) {
        match self {
            JsonErrKind::Parse => ("ParseError", 0),
            JsonErrKind::Type => ("TypeError", 1),
            JsonErrKind::Missing => ("MissingField", 2),
        }
    }
}

pub(crate) struct JsonErr {
    kind: JsonErrKind,
    message: String,
}

fn type_err(path: &str, want: &str, got: &Value) -> JsonErr {
    let where_ = if path.is_empty() { "the value".to_string() } else { path.to_string() };
    JsonErr {
        kind: JsonErrKind::Type,
        message: format!("{} should be {}, found {}", where_, want, json_kind_name(got)),
    }
}

fn json_kind_name(v: &Value) -> &'static str {
    match unwrap_json_value(v) {
        Value::Unit => "null",
        Value::Bool(_) => "a boolean",
        Value::Int(..) | Value::Float(_, _) => "a number",
        Value::String(_) => "a string",
        Value::Vec(_) => "an array",
        Value::Map(_) => "an object",
        Value::Struct(_) => "an object",
        _ if is_json_null(v) => "null",
        _ => "an unknown value",
    }
}

fn child_path(path: &str, name: &str) -> String {
    if path.is_empty() {
        format!("field {}", name)
    } else {
        format!("{}.{}", path, name)
    }
}

/// Convert a parsed JsonValue into a typed Rask value. `ty` is the target's
/// written type — the same strings struct declarations carry, so `Vec<Tag>`,
/// `Map<string, i64>` and `string?` all arrive here verbatim.
fn json_to_typed(
    json: &Value,
    ty: &str,
    path: &str,
    struct_decls: &HashMap<String, StructDecl>,
) -> Result<Value, JsonErr> {
    let ty = ty.trim();

    // `T?` — null or absent becomes none, anything else Some(T).
    if let Some(inner) = strip_optional(ty) {
        if is_json_null(json) {
            return Ok(option_none());
        }
        return Ok(option_some(json_to_typed(json, inner, path, struct_decls)?));
    }

    // The untyped tree, handed back as-is.
    if ty == "JsonValue" {
        return Ok(json.clone());
    }

    let raw = unwrap_json_value(json);

    if let Some(inner) = generic_arg(ty, "Vec") {
        let Value::Vec(items) = raw else {
            return Err(type_err(path, "a list", json));
        };
        let items = items.lock().unwrap().clone();
        let mut out = Vec::with_capacity(items.len());
        for (i, item) in items.iter().enumerate() {
            let base = if path.is_empty() { "the list" } else { path };
            out.push(json_to_typed(item, &inner, &format!("{}[{}]", base, i), struct_decls)?);
        }
        return Ok(Value::Vec(Arc::new(Mutex::new(out))));
    }

    if let Some(args) = generic_args(ty, "Map") {
        if args.len() == 2 {
            if args[0].trim() != "string" {
                return Err(JsonErr {
                    kind: JsonErrKind::Type,
                    message: format!(
                        "a Map decoded from JSON needs string keys, not `{}` — JSON object keys are always strings",
                        args[0].trim()
                    ),
                });
            }
            let entries = object_entries(raw).ok_or_else(|| type_err(path, "an object", json))?;
            let mut pairs = MapData::with_capacity(entries.len());
            for (k, v) in entries {
                let value = json_to_typed(&v, &args[1], &child_path(path, &k), struct_decls)?;
                pairs.insert(MapKey(Value::String(Arc::new(Mutex::new(k)))), value);
            }
            return Ok(Value::Map(Arc::new(Mutex::new(pairs))));
        }
    }

    match ty {
        "string" => extract_string(raw).map_err(|_| type_err(path, "string", json)),
        "bool" => extract_bool(raw).map_err(|_| type_err(path, "bool", json)),
        "f32" | "f64" | "float" => extract_float(raw).map_err(|_| type_err(path, ty, json)),
        _ if rask_ast::primitives::is_machine_integer(ty)
            || rask_ast::primitives::INT_ALIASES.contains(&ty) => {
            extract_int(raw, int_kind(ty)).map_err(|_| type_err(path, "an integer", json))
        }
        _ => {
            let decl = struct_decls.get(ty).ok_or_else(|| JsonErr {
                kind: JsonErrKind::Type,
                message: format!("`{}` isn't a type json.decode knows how to build", ty),
            })?;
            let entries = object_entries(raw).ok_or_else(|| type_err(path, "an object", json))?;
            let mut fields = IndexMap::new();
            for field in &decl.fields {
                // `@skip` fields aren't in the serialized form (E19) — they take
                // their `@default`, or the type's empty value.
                if field_attrs::is_skipped(&field.attrs) {
                    fields.insert(
                        field.name.clone(),
                        default_value(&field.attrs, &field.ty, struct_decls),
                    );
                    continue;
                }
                let key = field_attrs::serial_name(&field.attrs, &field.name);
                match entries.iter().find(|(k, _)| *k == key) {
                    Some((_, v)) => {
                        let value = json_to_typed(
                            v,
                            &field.ty,
                            &child_path(path, &key),
                            struct_decls,
                        )?;
                        fields.insert(field.name.clone(), value);
                    }
                    // A `T?` field takes `none`; anything else has to be there (J9).
                    None if strip_optional(&field.ty).is_some() => {
                        fields.insert(field.name.clone(), option_none());
                    }
                    // `@default` covers a missing key too (E20).
                    None if field_attrs::default_literal(&field.attrs).is_some() => {
                        fields.insert(
                            field.name.clone(),
                            default_value(&field.attrs, &field.ty, struct_decls),
                        );
                    }
                    None => {
                        return Err(JsonErr {
                            kind: JsonErrKind::Missing,
                            message: format!("field \"{}\" not found in the JSON object", key),
                        })
                    }
                }
            }
            // Keys the struct doesn't declare are skipped (J10).
            Ok(Value::new_struct(ty.to_string(), fields, None))
        }
    }
}

/// The value a field that isn't read from the input starts at: its
/// `@default(…)` literal when it has one, otherwise the type's empty value.
fn default_value(
    attrs: &[String],
    ty: &str,
    struct_decls: &HashMap<String, StructDecl>,
) -> Value {
    if let Some(literal) = field_attrs::default_literal(attrs) {
        if let Some(v) = literal_value(literal.trim(), ty) {
            return v;
        }
    }
    empty_value(ty, struct_decls)
}

fn literal_value(literal: &str, ty: &str) -> Option<Value> {
    let ty = ty.trim();
    match ty {
        "string" => field_attrs::string_literal(literal)
            .map(|s| Value::String(Arc::new(Mutex::new(s)))),
        "bool" => match literal {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
        "f32" | "f64" | "float" => literal.parse::<f64>().ok().map(|f| { let k = FloatKind::from_name(ty).unwrap_or(FloatKind::F64); Value::Float(k.round(f), k) }),
        _ => literal.parse::<i64>().ok().map(|n| Value::Int(n, int_kind(ty))),
    }
}

fn empty_value(ty: &str, struct_decls: &HashMap<String, StructDecl>) -> Value {
    let ty = ty.trim();
    if strip_optional(ty).is_some() {
        return option_none();
    }
    if generic_arg(ty, "Vec").is_some() {
        return Value::Vec(Arc::new(Mutex::new(Vec::new())));
    }
    if generic_args(ty, "Map").is_some() {
        return Value::Map(Arc::new(Mutex::new(MapData::new())));
    }
    match ty {
        "string" => Value::String(Arc::new(Mutex::new(String::new()))),
        "bool" => Value::Bool(false),
        "f32" | "f64" | "float" => Value::Float(0.0, FloatKind::Untyped),
        _ if rask_ast::primitives::is_machine_integer(ty)
            || rask_ast::primitives::INT_ALIASES.contains(&ty) => Value::Int(0, int_kind(ty)),
        _ => match struct_decls.get(ty) {
            Some(decl) => {
                let mut fields = IndexMap::new();
                for f in &decl.fields {
                    fields.insert(f.name.clone(), empty_value(&f.ty, struct_decls));
                }
                Value::new_struct(ty.to_string(), fields, None)
            }
            None => Value::Unit,
        },
    }
}

fn int_kind(ty: &str) -> crate::value::IntKind {
    use crate::value::IntKind;
    match ty {
        "i8" => IntKind::I8,
        "i16" => IntKind::I16,
        "i32" => IntKind::I32,
        "i64" | "isize" => IntKind::I64,
        "u8" => IntKind::U8,
        "u16" => IntKind::U16,
        "u32" => IntKind::U32,
        "u64" | "usize" => IntKind::U64,
        _ => IntKind::Untyped,
    }
}

/// `T?` → `T`. Only the trailing `?` counts; `Map<string, i64?>` keeps its own.
fn strip_optional(ty: &str) -> Option<&str> {
    ty.trim().strip_suffix('?').map(str::trim)
}

fn generic_arg(ty: &str, name: &str) -> Option<String> {
    generic_args(ty, name)?.into_iter().next()
}

/// The arguments of `Name<…>`, split on commas outside nested angle brackets.
fn generic_args(ty: &str, name: &str) -> Option<Vec<String>> {
    let ty = ty.trim();
    let rest = ty.strip_prefix(name)?.trim_start();
    let inner = rest.strip_prefix('<')?.strip_suffix('>')?;
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in inner.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                out.push(inner[start..i].trim().to_string());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(inner[start..].trim().to_string());
    Some(out)
}

/// Unwrap a JsonValue enum to its inner content for easier inspection.
fn unwrap_json_value(json: &Value) -> &Value {
    match json {
        Value::Enum {
            name,
            variant,
            fields,
            ..
        } if name == "JsonValue" => match variant.as_str() {
            "String" | "Number" | "Bool" | "Array" | "Object" => {
                fields.first().unwrap_or(json)
            }
            _ => json,
        },
        _ => json,
    }
}

fn is_json_null(v: &Value) -> bool {
    matches!(
        v,
        Value::Enum { name, variant, .. } if name == "JsonValue" && variant == "Null"
    ) || matches!(v, Value::Unit)
}

fn extract_string(v: &Value) -> Result<Value, ()> {
    match v {
        Value::String(_) => Ok(v.clone()),
        _ => Err(()),
    }
}

fn extract_int(v: &Value, kind: crate::value::IntKind) -> Result<Value, ()> {
    match v {
        Value::Int(n, _) => Ok(Value::Int(*n, kind)),
        Value::Float(f, _) => Ok(Value::Int(*f as i64, kind)),
        _ => Err(()),
    }
}

fn extract_float(v: &Value) -> Result<Value, ()> {
    match v {
        Value::Float(f, k) => Ok(Value::Float(*f, *k)),
        Value::Int(n, _) => Ok(Value::Float(*n as f64, FloatKind::Untyped)),
        _ => Err(()),
    }
}

fn extract_bool(v: &Value) -> Result<Value, ()> {
    match v {
        Value::Bool(b) => Ok(Value::Bool(*b)),
        _ => Err(()),
    }
}

/// Key/value pairs of a JSON object, in the order they were parsed.
fn object_entries(v: &Value) -> Option<Vec<(String, Value)>> {
    match v {
        Value::Map(m) => {
            let map = m.lock().unwrap();
            let mut out = Vec::with_capacity(map.len());
            for (k, val) in map.iter() {
                let Value::String(s) = &k.0 else { continue };
                out.push((s.lock().unwrap().clone(), val.clone()));
            }
            Some(out)
        }
        // A struct read as an object: `json.from_value` on an already-built value.
        Value::Struct(s) => {
            let guard = s.lock().unwrap();
            Some(guard.fields.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        }
        _ => None,
    }
}

// ─── Result / Option helpers ───

fn make_result_ok(value: Value) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![value],
        variant_index: 0, origin: None,
    }
}

fn make_result_err(kind: JsonErrKind, msg: &str) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![make_json_error(kind, msg)],
        variant_index: 1, origin: None,
    }
}

/// The `JsonError` a failing decode hands back (J3). A bare string used to go
/// here, so `e.message()` didn't resolve — the error had no type.
fn make_json_error(kind: JsonErrKind, msg: &str) -> Value {
    let (variant, index) = kind.variant();
    Value::Enum {
        name: "JsonError".to_string(),
        variant: variant.to_string(),
        fields: vec![Value::String(Arc::new(Mutex::new(msg.to_string())))],
        variant_index: index,
        origin: None,
    }
}

fn option_some(value: Value) -> Value {
    Value::Enum {
        name: "Option".to_string(),
        variant: "Some".to_string(),
        fields: vec![value],
        variant_index: 0, origin: None,
    }
}

fn option_none() -> Value {
    Value::Enum {
        name: "Option".to_string(),
        variant: "None".to_string(),
        fields: vec![],
        variant_index: 1, origin: None,
    }
}
