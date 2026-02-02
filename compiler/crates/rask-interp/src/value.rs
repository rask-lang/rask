//! Runtime values.

use std::collections::HashMap;

/// A runtime value in the interpreter.
#[derive(Debug, Clone)]
pub enum Value {
    /// Unit value
    Unit,
    /// Boolean
    Bool(bool),
    /// Integer (using i64 for all integer types in interpreter)
    Int(i64),
    /// Float (using f64 for all float types in interpreter)
    Float(f64),
    /// Character
    Char(char),
    /// String
    String(String),
    /// Struct instance
    Struct {
        name: String,
        fields: HashMap<String, Value>,
    },
    /// Enum variant
    Enum {
        name: String,
        variant: String,
        fields: Vec<Value>,
    },
    /// Function
    Function {
        name: String,
        params: Vec<String>,
        // body stored elsewhere
    },
}
