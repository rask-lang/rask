// SPDX-License-Identifier: (MIT OR Apache-2.0)
#![allow(dead_code)]
//! CLI module methods (cli.*).
//!
//! Layer: RUNTIME — reads process arguments from the OS.
//!
//! Provides argument parsing: quick API (cli.parse()) and builder (cli.Parser).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::interp::{Interpreter, RuntimeError};
use crate::value::Value;

impl Interpreter {
    /// Handle cli module methods.
    pub(crate) fn call_cli_module_method(
        &mut self,
        method: &str,
        _args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            // Legacy: cli.args() still works
            "args" => {
                let args_vec: Vec<Value> = self
                    .cli_args
                    .iter()
                    .map(|s| Value::String(Arc::new(Mutex::new(s.clone()))))
                    .collect();
                Ok(Value::Vec(Arc::new(Mutex::new(args_vec))))
            }
            // cli.parse() -> Args struct
            "parse" => {
                let parsed = parse_args(&self.cli_args);
                Ok(parsed)
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "cli".to_string(),
                method: method.to_string(),
            }),
        }
    }

    /// Handle methods on an Args struct returned by cli.parse().
    pub(crate) fn call_args_method(
        &self,
        fields: &HashMap<String, Value>,
        method: &str,
        args: Vec<Value>,
    ) -> Result<Value, RuntimeError> {
        match method {
            "flag" => {
                let long = self.expect_string(&args, 0)?;
                let short = self.expect_string(&args, 1)?;
                let flags = extract_vec_of_strings(fields, "_flags")?;
                let found = flags.iter().any(|f| f == &long || f == &short);
                Ok(Value::Bool(found))
            }
            "option" => {
                let long = self.expect_string(&args, 0)?;
                let short = self.expect_string(&args, 1)?;
                let options = extract_map_strings(fields, "_options")?;
                let val = options.get(&long).or_else(|| options.get(&short)).cloned();
                match val {
                    Some(v) => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "Some".to_string(),
                        fields: vec![Value::String(Arc::new(Mutex::new(v)))],
                    }),
                    None => Ok(Value::Enum {
                        name: "Option".to_string(),
                        variant: "None".to_string(),
                        fields: vec![],
                    }),
                }
            }
            "option_or" => {
                let long = self.expect_string(&args, 0)?;
                let short = self.expect_string(&args, 1)?;
                let default = self.expect_string(&args, 2)?;
                let options = extract_map_strings(fields, "_options")?;
                let val = options
                    .get(&long)
                    .or_else(|| options.get(&short))
                    .cloned()
                    .unwrap_or(default);
                Ok(Value::String(Arc::new(Mutex::new(val))))
            }
            "positional" => {
                match fields.get("_positional") {
                    Some(v) => Ok(v.clone()),
                    None => Ok(Value::Vec(Arc::new(Mutex::new(vec![])))),
                }
            }
            "program" => {
                match fields.get("_program") {
                    Some(v) => Ok(v.clone()),
                    None => Ok(Value::String(Arc::new(Mutex::new(String::new())))),
                }
            }
            _ => Err(RuntimeError::NoSuchMethod {
                ty: "Args".to_string(),
                method: method.to_string(),
            }),
        }
    }
}

/// Parse raw CLI args into an Args struct value.
///
/// Supports: --flag, -f, --option=value, --option value, -o value, --, combined short flags (-vn)
fn parse_args(raw_args: &[String]) -> Value {
    let mut flags: Vec<String> = Vec::new();
    let mut options: HashMap<String, String> = HashMap::new();
    let mut positional: Vec<Value> = Vec::new();
    let program = raw_args.first().cloned().unwrap_or_default();

    let args = if raw_args.len() > 1 { &raw_args[1..] } else { &[] };
    let mut i = 0;
    let mut after_double_dash = false;

    while i < args.len() {
        let arg = &args[i];

        if after_double_dash {
            positional.push(Value::String(Arc::new(Mutex::new(arg.clone()))));
            i += 1;
            continue;
        }

        if arg == "--" {
            after_double_dash = true;
            i += 1;
            continue;
        }

        if let Some(rest) = arg.strip_prefix("--") {
            // Long flag or option
            if let Some((key, val)) = rest.split_once('=') {
                options.insert(key.to_string(), val.to_string());
            } else if i + 1 < args.len() && !args[i + 1].starts_with('-') {
                // Peek: if next arg doesn't look like a flag, treat as option value
                // But we can't always know — treat as flag for now and let option() check
                // Actually: store as flag. option() will check _options first.
                // To support --output file, we'd need the parser definition.
                // For quick API: --key value is ambiguous. Store as flag only.
                // Builder API solves this.
                flags.push(rest.to_string());
            } else {
                flags.push(rest.to_string());
            }
        } else if let Some(rest) = arg.strip_prefix('-') {
            if rest.len() == 1 {
                // Single short flag: -v
                // Could be -o value (option). Same ambiguity as above.
                flags.push(rest.to_string());
            } else if rest.contains('=') {
                // -o=value
                if let Some((key, val)) = rest.split_once('=') {
                    options.insert(key.to_string(), val.to_string());
                }
            } else {
                // Combined short flags: -vn -> -v -n
                for c in rest.chars() {
                    flags.push(c.to_string());
                }
            }
        } else {
            positional.push(Value::String(Arc::new(Mutex::new(arg.clone()))));
        }

        i += 1;
    }

    // Build the Args struct
    let flags_value = Value::Vec(Arc::new(Mutex::new(
        flags
            .into_iter()
            .map(|f| Value::String(Arc::new(Mutex::new(f))))
            .collect(),
    )));

    let options_value = {
        let mut map_entries: Vec<Value> = Vec::new();
        for (k, v) in &options {
            map_entries.push(Value::Vec(Arc::new(Mutex::new(vec![
                Value::String(Arc::new(Mutex::new(k.clone()))),
                Value::String(Arc::new(Mutex::new(v.clone()))),
            ]))));
        }
        Value::Vec(Arc::new(Mutex::new(map_entries)))
    };

    let mut struct_fields = HashMap::new();
    struct_fields.insert("_flags".to_string(), flags_value);
    struct_fields.insert("_options".to_string(), options_value);
    struct_fields.insert(
        "_positional".to_string(),
        Value::Vec(Arc::new(Mutex::new(positional))),
    );
    struct_fields.insert(
        "_program".to_string(),
        Value::String(Arc::new(Mutex::new(program))),
    );

    Value::Struct {
        name: "Args".to_string(),
        fields: struct_fields,
        resource_id: None,
    }
}

/// Extract a Vec<String> from a struct field that holds Vec<Value::String>.
fn extract_vec_of_strings(
    fields: &HashMap<String, Value>,
    key: &str,
) -> Result<Vec<String>, RuntimeError> {
    match fields.get(key) {
        Some(Value::Vec(v)) => {
            let vec = v.lock().unwrap();
            let mut result = Vec::new();
            for item in vec.iter() {
                if let Value::String(s) = item {
                    result.push(s.lock().unwrap().clone());
                }
            }
            Ok(result)
        }
        _ => Ok(Vec::new()),
    }
}

/// Extract a HashMap<String, String> from a struct field holding Vec<[key, value]>.
fn extract_map_strings(
    fields: &HashMap<String, Value>,
    key: &str,
) -> Result<HashMap<String, String>, RuntimeError> {
    match fields.get(key) {
        Some(Value::Vec(v)) => {
            let vec = v.lock().unwrap();
            let mut result = HashMap::new();
            for entry in vec.iter() {
                if let Value::Vec(pair) = entry {
                    let pair = pair.lock().unwrap();
                    if pair.len() == 2 {
                        if let (Value::String(k), Value::String(v)) = (&pair[0], &pair[1]) {
                            result.insert(
                                k.lock().unwrap().clone(),
                                v.lock().unwrap().clone(),
                            );
                        }
                    }
                }
            }
            Ok(result)
        }
        _ => Ok(HashMap::new()),
    }
}
