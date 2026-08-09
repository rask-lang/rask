// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! String formatting and interpolation.

use rask_ast::fmt_spec::{pad, parse_spec, FormatSpec, SpecType};

use crate::value::{FloatKind, Value};

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
                let next = match &current {
                    Value::Struct(s) => {
                        s.lock().unwrap().fields.get(part).cloned().unwrap_or(Value::Unit)
                    }
                    _ => {
                        return Err(RuntimeError::TypeError(format!(
                            "cannot access field '{}' on {}",
                            part,
                            current.type_name()
                        )));
                    }
                };
                current = next;
            }
            Ok(current)
        } else {
            Err(RuntimeError::UndefinedVariable(parts[0].to_string()))
        }
    }

    /// A runtime template (`format(t, …)` where `t` isn't a literal) still
    /// parses its specs here. Static templates never reach this — the desugar
    /// pass turns them into `__fmt` calls (std.fmt/CM5).
    fn apply_format_spec(&self, value: &Value, spec: &str) -> Result<String, RuntimeError> {
        match parse_spec(spec) {
            Some(parsed) => Ok(self.render_spec(value, parsed, format!("{}", value))),
            None => Err(RuntimeError::TypeError(format!("`{}` is not a format spec", spec))),
        }
    }

    /// Render `value` under `spec`. `display` is what the value's own
    /// `to_string()` gives — passed in so a user `Displayable` impl can be
    /// consulted by the caller that has the interpreter mutably.
    pub(crate) fn render_spec(&self, value: &Value, spec: FormatSpec, display: String) -> String {
        let as_int = |v: &Value| match v {
            Value::Int(n, _) => Some(*n),
            Value::Char(c) => Some(*c as i64),
            Value::Bool(b) => Some(*b as i64),
            _ => None,
        };

        let base = match spec.ty {
            SpecType::Debug => self.debug_format(value),
            SpecType::Exp => match value {
                Value::Float(n, _) => format!("{:e}", n),
                Value::Int(n, _) => format!("{:e}", *n as f64),
                _ => display,
            },
            SpecType::Hex { upper } => match as_int(value) {
                Some(n) if upper => format!("{:X}", n),
                Some(n) => format!("{:x}", n),
                None => display,
            },
            SpecType::Binary => match as_int(value) {
                Some(n) => format!("{:b}", n),
                None => display,
            },
            SpecType::Octal => match as_int(value) {
                Some(n) => format!("{:o}", n),
                None => display,
            },
            SpecType::Display => match (spec.precision, value) {
                (Some(prec), Value::Float(n, _)) => format!("{:.prec$}", n, prec = prec),
                // Precision on a string truncates it — the one non-float use
                // the grammar allows.
                (Some(prec), Value::String(_)) => display.chars().take(prec).collect(),
                _ => display,
            },
        };

        let numeric = matches!(
            value,
            Value::Int(..) | Value::Float(_, _) | Value::Int128(_) | Value::Uint128(_)
        );
        pad(&base, spec.width, spec.effective_align(numeric), spec.fill)
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
            Value::Struct(ref s) => {
                let guard = s.lock().unwrap();
                let field_strs: Vec<String> = guard.fields
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, self.debug_format(v)))
                    .collect();
                format!("{} {{ {} }}", guard.name, field_strs.join(", "))
            }
            Value::Enum { name, variant, fields, .. } => {
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
                let mut closed = false;
                while let Some(&next) = chars.peek() {
                    if next == '}' {
                        chars.next();
                        closed = true;
                        break;
                    }
                    expr_str.push(chars.next().unwrap());
                }
                // No closing brace — pass the text through as-is. Re-emitting a
                // `}` here turned the one-character string `"{"` into `"{}"`,
                // so it compared unequal to the same literal on native.
                if !closed {
                    result.push('{');
                    result.push_str(&expr_str);
                    continue;
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
                    if spec == ":debug" {
                        result.push_str(&self.debug_format(&value));
                    } else {
                        result.push_str(&Self::format_value_with_spec(&value, spec));
                    }
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
    /// Render `value` under `spec` (which still carries its leading `:`).
    ///
    /// The set of specs handled here is the one `rask_ast::fmt_spec` defines —
    /// the parser rejects anything else before it gets this far, so the
    /// fall-through arms are for specs that don't apply to this value's type
    /// (`:x` on a string), not for unknown syntax.
    fn format_value_with_spec(value: &Value, spec: &str) -> String {
        debug_assert!(
            rask_ast::fmt_spec::is_valid_spec(&spec[1..]),
            "unvalidated format spec reached the formatter: {spec}",
        );
        let spec = &spec[1..]; // strip leading ':'
        match value {
            Value::Float(f, k) => {
                if let Some(precision) = spec.strip_prefix('.') {
                    if let Ok(p) = precision.parse::<usize>() {
                        return format!("{:.*}", p, f);
                    }
                }
                k.format(*f)
            }
            Value::Int(n, k) => {
                let unsigned = *n as u64;
                match spec {
                    "b" if k.is_unsigned() => format!("{:b}", unsigned),
                    "x" if k.is_unsigned() => format!("{:x}", unsigned),
                    "X" if k.is_unsigned() => format!("{:X}", unsigned),
                    "o" if k.is_unsigned() => format!("{:o}", unsigned),
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

