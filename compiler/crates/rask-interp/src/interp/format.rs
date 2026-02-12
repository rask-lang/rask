// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! String formatting and interpolation.

use crate::value::Value;

use super::{Interpreter, RuntimeError};

impl Interpreter {
    pub(super) fn format_string(&self, template: &str, args: &[Value]) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = template.chars().peekable();
        let mut arg_index = 0usize;

        while let Some(c) = chars.next() {
            if c == '{' {
                if chars.peek() == Some(&'{') {
                    chars.next();
                    result.push('{');
                    continue;
                }
                let mut spec_str = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next();
                        break;
                    }
                    spec_str.push(chars.next().unwrap());
                }
                let (arg_id, fmt_spec) = if let Some(colon_pos) = spec_str.find(':') {
                    let id_part = &spec_str[..colon_pos];
                    let spec_part = &spec_str[colon_pos + 1..];
                    (id_part.to_string(), Some(spec_part.to_string()))
                } else {
                    (spec_str, None)
                };

                let value = if arg_id.is_empty() {
                    if arg_index < args.len() {
                        let v = args[arg_index].clone();
                        arg_index += 1;
                        v
                    } else {
                        return Err(RuntimeError::TypeError(format!(
                            "format() not enough arguments (expected at least {})",
                            arg_index + 1
                        )));
                    }
                } else if let Ok(idx) = arg_id.parse::<usize>() {
                    if idx < args.len() {
                        args[idx].clone()
                    } else {
                        return Err(RuntimeError::TypeError(format!(
                            "format() argument index {} out of range (have {} args)",
                            idx,
                            args.len()
                        )));
                    }
                } else {
                    self.resolve_named_placeholder(&arg_id)?
                };

                match fmt_spec {
                    Some(spec) => {
                        let formatted = self.apply_format_spec(&value, &spec)?;
                        result.push_str(&formatted);
                    }
                    None => {
                        result.push_str(&format!("{}", value));
                    }
                }
            } else if c == '}' {
                if chars.peek() == Some(&'}') {
                    chars.next();
                    result.push('}');
                } else {
                    result.push('}');
                }
            } else {
                result.push(c);
            }
        }

        Ok(result)
    }

    /// Resolve a named placeholder like "name" or "obj.field" from the environment.
    fn resolve_named_placeholder(&self, name: &str) -> Result<Value, RuntimeError> {
        let parts: Vec<&str> = name.split('.').collect();
        if let Some(val) = self.env.get(parts[0]) {
            let mut current = val.clone();
            for &part in &parts[1..] {
                match current {
                    Value::Struct { fields, .. } => {
                        current = fields.get(part).cloned().unwrap_or(Value::Unit);
                    }
                    _ => {
                        return Err(RuntimeError::TypeError(format!(
                            "cannot access field '{}' on {}",
                            part,
                            current.type_name()
                        )));
                    }
                }
            }
            Ok(current)
        } else {
            Err(RuntimeError::UndefinedVariable(parts[0].to_string()))
        }
    }

    fn apply_format_spec(&self, value: &Value, spec: &str) -> Result<String, RuntimeError> {
        let mut fill = ' ';
        let mut align = None;
        let mut width = 0usize;
        let mut precision = None;
        let mut format_type = ' ';

        let spec_chars: Vec<char> = spec.chars().collect();
        let mut pos = 0;

        if spec_chars.len() >= 2 && matches!(spec_chars[1], '<' | '>' | '^') {
            fill = spec_chars[0];
            align = Some(spec_chars[1]);
            pos = 2;
        } else if !spec_chars.is_empty() && matches!(spec_chars[0], '<' | '>' | '^') {
            align = Some(spec_chars[0]);
            pos = 1;
        }

        let mut width_str = String::new();
        while pos < spec_chars.len() && spec_chars[pos].is_ascii_digit() {
            width_str.push(spec_chars[pos]);
            pos += 1;
        }
        if !width_str.is_empty() {
            width = width_str.parse().unwrap_or(0);
        }

        if pos < spec_chars.len() && spec_chars[pos] == '.' {
            pos += 1;
            let mut prec_str = String::new();
            while pos < spec_chars.len() && spec_chars[pos].is_ascii_digit() {
                prec_str.push(spec_chars[pos]);
                pos += 1;
            }
            precision = Some(prec_str.parse::<usize>().unwrap_or(0));
        }

        if pos < spec_chars.len() {
            format_type = spec_chars[pos];
        }

        let formatted = match format_type {
            '?' => {
                self.debug_format(value)
            }
            'x' => {
                match value {
                    Value::Int(n) => format!("{:x}", n),
                    _ => format!("{}", value),
                }
            }
            'X' => {
                match value {
                    Value::Int(n) => format!("{:X}", n),
                    _ => format!("{}", value),
                }
            }
            'b' => {
                match value {
                    Value::Int(n) => format!("{:b}", n),
                    _ => format!("{}", value),
                }
            }
            'o' => {
                match value {
                    Value::Int(n) => format!("{:o}", n),
                    _ => format!("{}", value),
                }
            }
            'e' => {
                match value {
                    Value::Float(n) => format!("{:e}", n),
                    Value::Int(n) => format!("{:e}", *n as f64),
                    _ => format!("{}", value),
                }
            }
            _ => {
                match precision {
                    Some(prec) => match value {
                        Value::Float(n) => format!("{:.prec$}", n, prec = prec),
                        _ => format!("{}", value),
                    },
                    None => format!("{}", value),
                }
            }
        };

        if width > 0 && formatted.len() < width {
            let padding = width - formatted.len();
            let effective_align = align.unwrap_or('>');
            match effective_align {
                '<' => {
                    let mut s = formatted;
                    for _ in 0..padding {
                        s.push(fill);
                    }
                    Ok(s)
                }
                '^' => {
                    let left_pad = padding / 2;
                    let right_pad = padding - left_pad;
                    let mut s = String::new();
                    for _ in 0..left_pad {
                        s.push(fill);
                    }
                    s.push_str(&formatted);
                    for _ in 0..right_pad {
                        s.push(fill);
                    }
                    Ok(s)
                }
                _ => {
                    let mut s = String::new();
                    for _ in 0..padding {
                        s.push(fill);
                    }
                    s.push_str(&formatted);
                    Ok(s)
                }
            }
        } else {
            Ok(formatted)
        }
    }

    fn debug_format(&self, value: &Value) -> String {
        match value {
            Value::String(s) => format!("\"{}\"", s.lock().unwrap()),
            Value::Char(c) => format!("'{}'", c),
            Value::Vec(v) => {
                let vec = v.lock().unwrap();
                let items: Vec<String> = vec.iter().map(|v| self.debug_format(v)).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Struct { name, fields, .. } => {
                let field_strs: Vec<String> = fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, self.debug_format(v)))
                    .collect();
                format!("{} {{ {} }}", name, field_strs.join(", "))
            }
            Value::Enum { name, variant, fields } => {
                if fields.is_empty() {
                    format!("{}.{}", name, variant)
                } else {
                    let field_strs: Vec<String> =
                        fields.iter().map(|v| self.debug_format(v)).collect();
                    format!("{}.{}({})", name, variant, field_strs.join(", "))
                }
            }
            _ => format!("{}", value),
        }
    }

    pub(super) fn interpolate_string(&mut self, s: &str) -> Result<String, RuntimeError> {
        let mut result = String::new();
        let mut chars = s.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '{' {
                if chars.peek() == Some(&'{') {
                    result.push('{');
                    result.push('{');
                    chars.next();
                    continue;
                }
                let mut expr_str = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next();
                        break;
                    }
                    expr_str.push(chars.next().unwrap());
                }
                if expr_str.is_empty() || expr_str.starts_with(':') {
                    result.push('{');
                    result.push_str(&expr_str);
                    result.push('}');
                    continue;
                }
                let (expr_part, fmt_spec) = if let Some(colon_pos) = expr_str.find(':') {
                    (&expr_str[..colon_pos], Some(&expr_str[colon_pos..]))
                } else {
                    (expr_str.as_str(), None)
                };
                let value = self.eval_interpolation_expr(expr_part)?;
                if let Some(spec) = fmt_spec {
                    result.push_str(&Self::format_value_with_spec(&value, spec));
                } else {
                    result.push_str(&format!("{}", value));
                }
            } else if c == '}' && chars.peek() == Some(&'}') {
                result.push('}');
                result.push('}');
                chars.next();
            } else {
                result.push(c);
            }
        }

        Ok(result)
    }

    /// Evaluate an expression inside string interpolation using the real parser.
    fn eval_interpolation_expr(&mut self, expr_str: &str) -> Result<Value, RuntimeError> {
        let expr_str = expr_str.trim();

        let lex_result = rask_lexer::Lexer::new(expr_str).tokenize();
        if !lex_result.errors.is_empty() {
            return Err(RuntimeError::TypeError(format!(
                "invalid interpolation expression: {}", expr_str
            )));
        }

        let mut parser = rask_parser::Parser::new(lex_result.tokens);
        let expr = parser.parse_expr().map_err(|e| {
            RuntimeError::TypeError(format!(
                "cannot parse interpolation '{}': {}", expr_str, e.message
            ))
        })?;

        self.eval_expr(&expr).map_err(|diag| diag.error)
    }

    /// Format a value with a format specifier like :.2, :.1, :b, :x, etc.
    fn format_value_with_spec(value: &Value, spec: &str) -> String {
        let spec = &spec[1..]; // strip leading ':'
        match value {
            Value::Float(f) => {
                if let Some(precision) = spec.strip_prefix('.') {
                    if let Ok(p) = precision.parse::<usize>() {
                        return format!("{:.*}", p, f);
                    }
                }
                format!("{}", f)
            }
            Value::Int(n) => {
                match spec {
                    "b" => format!("{:b}", n),
                    "x" => format!("{:x}", n),
                    "X" => format!("{:X}", n),
                    "o" => format!("{:o}", n),
                    _ => format!("{}", n),
                }
            }
            _ => format!("{}", value),
        }
    }
}

