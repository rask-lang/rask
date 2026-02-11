// SPDX-License-Identifier: (MIT OR Apache-2.0)
#![allow(dead_code)]
//! JSON module methods (json.*).
//!
//! Layer: PURE — custom recursive-descent parser, no OS access.
//!
//! Provides: json.parse(), json.stringify(), json.stringify_pretty(),
//! json.encode(struct), json.decode<T>(string).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rask_ast::decl::StructDecl;

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle json module methods.
    pub(crate) fn call_json_method(
        &self,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "parse" => {
                let input = self.expect_string(&args, 0)?;
                match parse_json(&input) {
                    Ok(value) => Ok(make_result_ok(value)),
                    Err(e) => Ok(make_result_err(&e)),
                }
            }
            "stringify" => {
                let value = args
                    .into_iter()
                    .next()
                    .ok_or(RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                let s = stringify_value(&value, false, 0);
                Ok(Value::String(Arc::new(Mutex::new(s))))
            }
            "stringify_pretty" => {
                let value = args
                    .into_iter()
                    .next()
                    .ok_or(RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                let s = stringify_value(&value, true, 0);
                Ok(Value::String(Arc::new(Mutex::new(s))))
            }
            "encode" => {
                // Encode a struct to JSON string
                let value = args
                    .into_iter()
                    .next()
                    .ok_or(RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                let json_val = value_to_json(&value)?;
                let s = stringify_value(&json_val, false, 0);
                Ok(Value::String(Arc::new(Mutex::new(s))))
            }
            "encode_pretty" => {
                let value = args
                    .into_iter()
                    .next()
                    .ok_or(RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                let json_val = value_to_json(&value)?;
                let s = stringify_value(&json_val, true, 0);
                Ok(Value::String(Arc::new(Mutex::new(s))))
            }
            "to_value" => {
                let value = args
                    .into_iter()
                    .next()
                    .ok_or(RuntimeError::ArityMismatch { expected: 1, got: 0 })?;
                value_to_json(&value)
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
                match parse_json(&input) {
                    Ok(json_val) => {
                        match json_to_typed(&json_val, &type_name, &self.struct_decls) {
                            Ok(value) => Ok(make_result_ok(value)),
                            Err(e) => Ok(make_result_err(&e)),
                        }
                    }
                    Err(e) => Ok(make_result_err(&e)),
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
                "expected '{}', found '{}' at position {}",
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
            Some(c) => Err(format!("unexpected character '{}' at position {}", c as char, self.pos)),
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
                        Some(c) => return Err(format!("invalid escape '\\{}'", c as char)),
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
                _ => return Err("invalid unicode escape".to_string()),
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
                return Err("expected digit".to_string());
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
            Err(format!("unexpected token at position {}", self.pos))
        }
    }

    fn parse_null(&mut self) -> Result<Value, String> {
        if self.input[self.pos..].starts_with(b"null") {
            self.pos += 4;
            Ok(make_json_null())
        } else {
            Err(format!("unexpected token at position {}", self.pos))
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
                _ => return Err(format!("expected ',' or ']' at position {}", self.pos)),
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
                _ => return Err(format!("expected ',' or '}}' at position {}", self.pos)),
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
            "trailing data at position {}",
            parser.pos
        ));
    }
    Ok(value)
}

// ─── JSON Stringification ───

/// Stringify a JsonValue (or any Value) into JSON.
fn stringify_value(value: &Value, pretty: bool, indent: usize) -> String {
    match value {
        Value::Enum { name, variant, fields } if name == "JsonValue" => {
            stringify_json_variant(variant, fields, pretty, indent)
        }
        // Also handle raw Rask values directly (for json.encode)
        Value::Unit => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(n) => n.to_string(),
        Value::Float(f) => {
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
        Value::Struct { fields, .. } => {
            if fields.is_empty() {
                return "{}".to_string();
            }
            let mut sorted_keys: Vec<&String> = fields.keys().collect();
            sorted_keys.sort();
            if pretty {
                let mut s = "{\n".to_string();
                for (i, key) in sorted_keys.iter().enumerate() {
                    let val = &fields[*key];
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
                            stringify_value(&fields[*k], false, 0)
                        )
                    })
                    .collect();
                format!("{{{}}}", pairs.join(","))
            }
        }
        Value::Enum { name, variant, fields } if name == "Option" => {
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
            if let Some(Value::Float(f)) = fields.first() {
                if f.is_nan() || f.is_infinite() {
                    "null".to_string()
                } else if *f == f.floor() && f.abs() < 1e15 {
                    format!("{:.0}", f)
                } else {
                    f.to_string()
                }
            } else if let Some(Value::Int(n)) = fields.first() {
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
fn value_to_json(value: &Value) -> Result<Value, RuntimeError> {
    match value {
        Value::Unit => Ok(make_json_null()),
        Value::Bool(b) => Ok(make_json_bool(*b)),
        Value::Int(n) => Ok(make_json_number(*n as f64)),
        Value::Float(f) => Ok(make_json_number(*f)),
        Value::String(s) => Ok(make_json_string(&s.lock().unwrap())),
        Value::Vec(v) => {
            let vec = v.lock().unwrap();
            let items: Result<Vec<Value>, RuntimeError> =
                vec.iter().map(|v| value_to_json(v)).collect();
            Ok(make_json_array(items?))
        }
        Value::Struct { fields, .. } => {
            let entries: Result<Vec<(String, Value)>, RuntimeError> = fields
                .iter()
                .filter(|(k, _)| !k.starts_with('_')) // Skip internal fields
                .map(|(k, v)| value_to_json(v).map(|jv| (k.clone(), jv)))
                .collect();
            Ok(make_json_object(entries?))
        }
        Value::Enum { name, variant, fields } if name == "Option" => {
            match variant.as_str() {
                "Some" if !fields.is_empty() => value_to_json(&fields[0]),
                _ => Ok(make_json_null()),
            }
        }
        Value::Enum { name, variant, fields } if name == "JsonValue" => {
            // Already a JsonValue, return as-is
            Ok(Value::Enum {
                name: name.clone(),
                variant: variant.clone(),
                fields: fields.clone(),
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
    }
}

fn make_json_bool(b: bool) -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Bool".to_string(),
        fields: vec![Value::Bool(b)],
    }
}

fn make_json_number(n: f64) -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Number".to_string(),
        fields: vec![Value::Float(n)],
    }
}

fn make_json_string(s: &str) -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "String".to_string(),
        fields: vec![Value::String(Arc::new(Mutex::new(s.to_string())))],
    }
}

fn make_json_array(items: Vec<Value>) -> Value {
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Array".to_string(),
        fields: vec![Value::Vec(Arc::new(Mutex::new(items)))],
    }
}

fn make_json_object(entries: Vec<(String, Value)>) -> Value {
    // Store as a struct with string keys (like a Map)
    let mut map = HashMap::new();
    for (k, v) in entries {
        map.insert(k, v);
    }
    Value::Enum {
        name: "JsonValue".to_string(),
        variant: "Object".to_string(),
        fields: vec![Value::Struct {
            name: "Map".to_string(),
            fields: map,
            resource_id: None,
        }],
    }
}

// ─── JSON Decode (typed deserialization) ───

/// Convert a parsed JsonValue into a typed Rask value based on struct declarations.
fn json_to_typed(
    json: &Value,
    type_name: &str,
    struct_decls: &HashMap<String, StructDecl>,
) -> Result<Value, String> {
    // Unwrap JsonValue enum wrapper if present
    let raw = unwrap_json_value(json);

    match type_name {
        "string" => extract_string(raw),
        "i32" | "i64" => extract_int(raw),
        "f32" | "f64" => extract_float(raw),
        "bool" => extract_bool(raw),
        _ => {
            // Look up struct declaration
            let decl = struct_decls
                .get(type_name)
                .ok_or_else(|| format!("unknown type: {}", type_name))?;
            let obj_fields = extract_object_fields(raw)?;
            let mut struct_fields = HashMap::new();

            for field in &decl.fields {
                if let Some(json_field) = obj_fields.get(&field.name) {
                    let value =
                        json_field_to_value(json_field, &field.ty, struct_decls)?;
                    struct_fields.insert(field.name.clone(), value);
                } else {
                    // Optional field → None
                    if field.ty.ends_with('?') {
                        struct_fields.insert(field.name.clone(), option_none());
                    } else {
                        return Err(format!(
                            "missing field '{}' in JSON for type '{}'",
                            field.name, type_name
                        ));
                    }
                }
            }

            Ok(Value::Struct {
                name: type_name.to_string(),
                fields: struct_fields,
                resource_id: None,
            })
        }
    }
}

/// Convert a single JSON value to a typed Rask value based on the field's declared type.
fn json_field_to_value(
    json: &Value,
    ty: &str,
    struct_decls: &HashMap<String, StructDecl>,
) -> Result<Value, String> {
    let raw = unwrap_json_value(json);

    // Handle optional types
    if let Some(inner_ty) = ty.strip_suffix('?') {
        if is_json_null(raw) {
            return Ok(option_none());
        }
        let inner = json_field_to_value(raw, inner_ty, struct_decls)?;
        return Ok(option_some(inner));
    }

    match ty {
        "string" => extract_string(raw),
        "i32" | "i64" | "int" => extract_int(raw),
        "f32" | "f64" | "float" => extract_float(raw),
        "bool" => extract_bool(raw),
        _ if ty.starts_with("Vec<") => {
            // Vec<T> — extract inner type
            let inner_ty = &ty[4..ty.len() - 1];
            let items = extract_array(raw)?;
            let converted: Result<Vec<Value>, String> = items
                .iter()
                .map(|item| json_field_to_value(item, inner_ty, struct_decls))
                .collect();
            Ok(Value::Vec(Arc::new(Mutex::new(converted?))))
        }
        _ => {
            // Nested struct
            json_to_typed(raw, ty, struct_decls)
        }
    }
}

/// Unwrap a JsonValue enum to its inner content for easier inspection.
fn unwrap_json_value(json: &Value) -> &Value {
    match json {
        Value::Enum {
            name,
            variant,
            fields,
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

fn extract_string(v: &Value) -> Result<Value, String> {
    match v {
        Value::String(_) => Ok(v.clone()),
        _ => Err(format!("expected string, found {}", v.type_name())),
    }
}

fn extract_int(v: &Value) -> Result<Value, String> {
    match v {
        Value::Int(n) => Ok(Value::Int(*n)),
        Value::Float(f) => Ok(Value::Int(*f as i64)),
        _ => Err(format!("expected number, found {}", v.type_name())),
    }
}

fn extract_float(v: &Value) -> Result<Value, String> {
    match v {
        Value::Float(f) => Ok(Value::Float(*f)),
        Value::Int(n) => Ok(Value::Float(*n as f64)),
        _ => Err(format!("expected number, found {}", v.type_name())),
    }
}

fn extract_bool(v: &Value) -> Result<Value, String> {
    match v {
        Value::Bool(b) => Ok(Value::Bool(*b)),
        _ => Err(format!("expected bool, found {}", v.type_name())),
    }
}

fn extract_array(v: &Value) -> Result<Vec<Value>, String> {
    match v {
        Value::Vec(vec) => Ok(vec.lock().unwrap().clone()),
        _ => Err(format!("expected array, found {}", v.type_name())),
    }
}

/// Extract fields from a JSON object (either a JsonValue.Object or a Map/Struct).
fn extract_object_fields(v: &Value) -> Result<HashMap<String, Value>, String> {
    match v {
        // JsonValue.Object wraps a Struct named "Map" with field→value pairs
        Value::Struct { fields, .. } => Ok(fields.clone()),
        // Direct Map (from parsed JSON)
        Value::Map(m) => {
            let map = m.lock().unwrap();
            let mut result = HashMap::new();
            for (k, v) in map.iter() {
                let key = match k {
                    Value::String(s) => s.lock().unwrap().clone(),
                    _ => continue,
                };
                result.insert(key, v.clone());
            }
            Ok(result)
        }
        _ => Err(format!("expected object, found {}", v.type_name())),
    }
}

// ─── Result / Option helpers ───

fn make_result_ok(value: Value) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Ok".to_string(),
        fields: vec![value],
    }
}

fn make_result_err(msg: &str) -> Value {
    Value::Enum {
        name: "Result".to_string(),
        variant: "Err".to_string(),
        fields: vec![Value::String(Arc::new(Mutex::new(msg.to_string())))],
    }
}

fn option_some(value: Value) -> Value {
    Value::Enum {
        name: "Option".to_string(),
        variant: "Some".to_string(),
        fields: vec![value],
    }
}

fn option_none() -> Value {
    Value::Enum {
        name: "Option".to_string(),
        variant: "None".to_string(),
        fields: vec![],
    }
}
