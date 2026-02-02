//! The interpreter implementation.

use rask_ast::decl::Decl;
use crate::env::Environment;
use crate::value::Value;

/// The tree-walk interpreter.
pub struct Interpreter {
    env: Environment,
}

impl Interpreter {
    /// Create a new interpreter.
    pub fn new() -> Self {
        Self {
            env: Environment::new(),
        }
    }

    /// Run a program (list of declarations).
    pub fn run(&mut self, _decls: &[Decl]) -> Result<Value, RuntimeError> {
        // TODO: Implement interpreter
        Ok(Value::Unit)
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

/// A runtime error.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("undefined variable: {0}")]
    UndefinedVariable(String),
    #[error("type error: {0}")]
    TypeError(String),
    #[error("division by zero")]
    DivisionByZero,
}
