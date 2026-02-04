//! Runtime values.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt;
use std::fs::File as StdFile;
use std::rc::Rc;

use rask_ast::expr::Expr;

/// Built-in function kinds (global functions without module prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    Print,
    Println,
    Panic,
}

/// Type constructor kinds (for static method calls like Vec.new()).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeConstructorKind {
    Vec,
    Map,
    String,
}

/// Module kinds for stdlib modules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleKind {
    Fs,   // fs.read_file, fs.write_file, etc.
    Io,   // io.read_line, io.print, etc.
    Cli,  // cli.args
    Std,  // std.exit
    Env,  // env.var, env.vars
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
    /// String (mutable, like Vec)
    String(Rc<RefCell<String>>),
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
    /// Vec (growable array) with interior mutability
    Vec(Rc<RefCell<Vec<Value>>>),
    /// Type constructor (for static method calls like Vec.new())
    TypeConstructor(TypeConstructorKind),
    /// Enum variant constructor (e.g., Option.Some before calling with args)
    EnumConstructor {
        enum_name: String,
        variant_name: String,
        field_count: usize,
    },
    /// Module (fs, io, cli, std, env)
    Module(ModuleKind),
    /// Open file handle (Option allows close to invalidate)
    File(Rc<RefCell<Option<StdFile>>>),
    /// Closure (captured environment + params + body)
    Closure {
        params: Vec<String>,
        body: Expr,
        captured_env: HashMap<String, Value>,
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
            Value::Vec(_) => "Vec",
            Value::TypeConstructor(_) => "type",
            Value::EnumConstructor { .. } => "enum constructor",
            Value::Module(_) => "module",
            Value::File(_) => "File",
            Value::Closure { .. } => "closure",
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
            Value::String(s) => write!(f, "{}", s.borrow()),
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
            Value::Vec(v) => {
                let vec = v.borrow();
                write!(f, "[")?;
                for (i, item) in vec.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, "]")
            }
            Value::TypeConstructor(kind) => match kind {
                TypeConstructorKind::Vec => write!(f, "Vec"),
                TypeConstructorKind::Map => write!(f, "Map"),
                TypeConstructorKind::String => write!(f, "string"),
            },
            Value::EnumConstructor {
                enum_name,
                variant_name,
                ..
            } => {
                write!(f, "{}.{}", enum_name, variant_name)
            }
            Value::Module(kind) => match kind {
                ModuleKind::Fs => write!(f, "<module fs>"),
                ModuleKind::Io => write!(f, "<module io>"),
                ModuleKind::Cli => write!(f, "<module cli>"),
                ModuleKind::Std => write!(f, "<module std>"),
                ModuleKind::Env => write!(f, "<module env>"),
            },
            Value::File(file) => {
                if file.borrow().is_some() {
                    write!(f, "<file>")
                } else {
                    write!(f, "<closed file>")
                }
            }
            Value::Closure { params, .. } => {
                write!(f, "<closure |{}|>", params.join(", "))
            }
        }
    }
}
