// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Path construction for the interpreter.
//!
//! Path's *methods* live in `stdlib/path.rk` and are ordinary Rask — both
//! backends run that source, so there's one implementation. What used to be here
//! was a second one: 184 lines of Rust against 192 lines of C, for pure string
//! manipulation the two got different answers from (#688).
//!
//! What's left is the constructor, because `fs.current_dir()` and `fs.home_dir()`
//! build a Path from an OS call and need a value to hand back.

use crate::value::Value;
use indexmap::IndexMap;
use std::sync::{Arc, Mutex};

pub(crate) fn make_path_value(s: &str) -> Value {
    // Normalize separators to forward slash
    let normalized = s.replace('\\', "/");
    // Remove trailing slashes (except root)
    let trimmed = if normalized.len() > 1 {
        normalized.trim_end_matches('/')
    } else {
        &normalized
    };
    // Collapse double separators
    let mut result = String::with_capacity(trimmed.len());
    let mut prev_slash = false;
    for c in trimmed.chars() {
        if c == '/' {
            if !prev_slash || result.is_empty() {
                result.push(c);
            }
            prev_slash = true;
        } else {
            result.push(c);
            prev_slash = false;
        }
    }

    let mut fields = IndexMap::new();
    fields.insert(
        "value".to_string(),
        Value::String(Arc::new(Mutex::new(result))),
    );
    Value::new_struct(
        "Path".to_string(),
        fields,
        None,
    )
}
