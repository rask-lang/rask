//! Environment for variable bindings.

use std::collections::HashMap;
use crate::value::Value;

/// A scope in the environment.
#[derive(Debug, Default)]
struct Scope {
    bindings: HashMap<String, Value>,
}

/// The environment holding variable bindings.
#[derive(Debug, Default)]
pub struct Environment {
    scopes: Vec<Scope>,
}

impl Environment {
    /// Create a new empty environment.
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::default()],
        }
    }

    /// Push a new scope.
    pub fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    /// Pop the current scope.
    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    /// Define a variable in the current scope.
    pub fn define(&mut self, name: String, value: Value) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.bindings.insert(name, value);
        }
    }

    /// Look up a variable.
    pub fn get(&self, name: &str) -> Option<&Value> {
        for scope in self.scopes.iter().rev() {
            if let Some(value) = scope.bindings.get(name) {
                return Some(value);
            }
        }
        None
    }

    /// Assign to an existing variable.
    pub fn assign(&mut self, name: &str, value: Value) -> bool {
        for scope in self.scopes.iter_mut().rev() {
            if scope.bindings.contains_key(name) {
                scope.bindings.insert(name.to_string(), value);
                return true;
            }
        }
        false
    }
}
