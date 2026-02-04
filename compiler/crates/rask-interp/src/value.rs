//! Runtime values.

use std::collections::HashMap;
use std::fmt;

/// Built-in function kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Print,
    Println,
    Panic,
}

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
    /// Function reference
    Function {
        name: String,
    },
    /// Built-in function
    Builtin(BuiltinKind),
    /// Range value (for iteration)
    Range {
        start: i64,
        end: i64,
        inclusive: bool,
    },
}

impl Value {
    /// Get the type name for error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Unit => "()",
            Value::Bool(_) => "bool",
            Value::Int(_) => "i64",
            Value::Float(_) => "f64",
            Value::Char(_) => "char",
            Value::String(_) => "string",
            Value::Struct { .. } => "struct",
            Value::Enum { .. } => "enum",
            Value::Function { .. } => "func",
            Value::Builtin(_) => "builtin",
            Value::Range { .. } => "range",
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => write!(f, "()"),
            Value::Bool(b) => write!(f, "{}", b),
            Value::Int(n) => write!(f, "{}", n),
            Value::Float(n) => write!(f, "{}", n),
            Value::Char(c) => write!(f, "{}", c),
            Value::String(s) => write!(f, "{}", s),
            Value::Struct { name, fields } => {
                write!(f, "{} {{ ", name)?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}: {}", k, v)?;
                }
                write!(f, " }}")
            }
            Value::Enum { name, variant, fields } => {
                write!(f, "{}.{}", name, variant)?;
                if !fields.is_empty() {
                    write!(f, "(")?;
                    for (i, v) in fields.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", v)?;
                    }
                    write!(f, ")")?;
                }
                Ok(())
            }
            Value::Function { name } => write!(f, "<func {}>", name),
            Value::Builtin(kind) => write!(f, "<builtin {:?}>", kind),
            Value::Range { start, end, inclusive } => {
                if *inclusive {
                    write!(f, "{}..={}", start, end)
                } else {
                    write!(f, "{}..{}", start, end)
                }
            }
        }
    }
}
