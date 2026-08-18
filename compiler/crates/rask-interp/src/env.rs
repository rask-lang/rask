// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Environment for variable bindings.

use std::collections::HashMap;
use crate::value::Value;

/// A scope in the environment.
#[derive(Debug, Default)]
struct Scope {
    bindings: HashMap<String, Value>,
}

/// The environment holding variable bindings.
///
/// A Rask call pushes its scopes onto the same stack the caller is using, so
/// the stack is as deep as the recursion. Walking it per lookup made every name
/// cost O(depth) — and a name that *isn't* a variable, which is what a plain
/// function name looks like on the way to the function table, paid the full
/// walk every time. At 16,000 frames deep that was 7 seconds of hashing for a
/// program that does nothing (#799).
///
/// `defined_at` is the answer: name → the scopes that bind it, innermost last.
/// A lookup reads the last entry, a miss reads nothing, and neither depends on
/// how deep the stack is.
#[derive(Debug, Default)]
pub struct Environment {
    scopes: Vec<Scope>,
    /// Every bound name and the scope indices binding it, in scope order.
    defined_at: HashMap<String, Vec<usize>>,
}

impl Environment {
    /// Create a new empty environment.
    pub fn new() -> Self {
        Self {
            scopes: vec![Scope::default()],
            defined_at: HashMap::new(),
        }
    }

    /// Push a new scope.
    pub fn push_scope(&mut self) {
        self.scopes.push(Scope::default());
    }

    /// Pop the current scope.
    pub fn pop_scope(&mut self) {
        let Some(scope) = self.scopes.pop() else { return };
        let index = self.scopes.len();
        for name in scope.bindings.keys() {
            let Some(indices) = self.defined_at.get_mut(name) else { continue };
            // The popped scope is the innermost, so its entry is the last one.
            if indices.last() == Some(&index) {
                indices.pop();
            }
            if indices.is_empty() {
                self.defined_at.remove(name);
            }
        }
    }

    /// Define a variable in the current scope.
    pub fn define(&mut self, name: String, value: Value) {
        let index = self.scopes.len().saturating_sub(1);
        let Some(scope) = self.scopes.last_mut() else { return };
        // Redefining in the same scope replaces the value; the index already
        // has an entry for it and must not get a second one.
        if scope.bindings.insert(name.clone(), value).is_none() {
            self.defined_at.entry(name).or_default().push(index);
        }
    }

    /// Look up a variable.
    pub fn get(&self, name: &str) -> Option<&Value> {
        let index = *self.defined_at.get(name)?.last()?;
        self.scopes.get(index)?.bindings.get(name)
    }

    /// Assign to an existing variable.
    pub fn assign(&mut self, name: &str, value: Value) -> bool {
        let Some(index) = self.defined_at.get(name).and_then(|i| i.last().copied()) else {
            return false;
        };
        let Some(scope) = self.scopes.get_mut(index) else { return false };
        scope.bindings.insert(name.to_string(), value);
        true
    }

    /// Remove a variable from the environment (for `discard`).
    pub fn remove(&mut self, name: &str) {
        let Some(indices) = self.defined_at.get_mut(name) else { return };
        let Some(index) = indices.pop() else { return };
        if indices.is_empty() {
            self.defined_at.remove(name);
        }
        if let Some(scope) = self.scopes.get_mut(index) {
            scope.bindings.remove(name);
        }
    }

    /// Get a mutable reference to a variable (for in-place field assignment).
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Value> {
        let index = *self.defined_at.get(name)?.last()?;
        self.scopes.get_mut(index)?.bindings.get_mut(name)
    }

    /// Get the current scope depth.
    pub fn scope_depth(&self) -> usize {
        self.scopes.len()
    }

    /// Apply `f` to each in-scope value (innermost scope first), returning the
    /// first `Some`. Used for handle auto-deref, where the pool is located by the
    /// handle's pool id rather than by name — the closure may recurse into
    /// struct fields to reach a pool held in `self`.
    pub fn find_map<T, F: Fn(&Value) -> Option<T>>(&self, f: F) -> Option<T> {
        for scope in self.scopes.iter().rev() {
            for value in scope.bindings.values() {
                if let Some(found) = f(value) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Capture all visible variables (for closures).
    pub fn capture(&self) -> HashMap<String, Value> {
        let mut captured = HashMap::new();
        for scope in &self.scopes {
            for (name, value) in &scope.bindings {
                captured.insert(name.clone(), value.clone());
            }
        }
        captured
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(n: i64) -> Value {
        Value::Int(n, crate::value::IntKind::I64)
    }

    fn as_int(v: Option<&Value>) -> Option<i64> {
        match v {
            Some(Value::Int(n, _)) => Some(*n),
            _ => None,
        }
    }

    // The index has to answer what the scope walk answered: innermost wins, and
    // popping the scope that shadowed restores what it hid.
    #[test]
    fn inner_scope_shadows_and_pop_restores() {
        let mut env = Environment::new();
        env.define("x".into(), int(1));
        env.push_scope();
        env.define("x".into(), int(2));
        assert_eq!(as_int(env.get("x")), Some(2));
        env.pop_scope();
        assert_eq!(as_int(env.get("x")), Some(1));
    }

    #[test]
    fn redefining_in_one_scope_replaces_rather_than_stacks() {
        let mut env = Environment::new();
        env.define("x".into(), int(1));
        env.define("x".into(), int(2));
        assert_eq!(as_int(env.get("x")), Some(2));
        env.pop_scope();
        assert!(env.get("x").is_none(), "one define, one entry — not two");
    }

    #[test]
    fn assign_writes_the_innermost_binding() {
        let mut env = Environment::new();
        env.define("x".into(), int(1));
        env.push_scope();
        env.define("x".into(), int(2));
        assert!(env.assign("x", int(9)));
        assert_eq!(as_int(env.get("x")), Some(9));
        env.pop_scope();
        assert_eq!(as_int(env.get("x")), Some(1), "the outer one is untouched");
    }

    #[test]
    fn assign_to_an_unknown_name_reports_it() {
        let mut env = Environment::new();
        assert!(!env.assign("nope", int(1)));
    }

    #[test]
    fn remove_uncovers_the_shadowed_binding() {
        let mut env = Environment::new();
        env.define("x".into(), int(1));
        env.push_scope();
        env.define("x".into(), int(2));
        env.remove("x");
        assert_eq!(as_int(env.get("x")), Some(1));
        env.remove("x");
        assert!(env.get("x").is_none());
    }

    // What a plain function name looks like on the way to the function table.
    // This was the O(depth) case that made deep recursion quadratic (#799).
    #[test]
    fn a_miss_stays_a_miss_at_depth() {
        let mut env = Environment::new();
        for i in 0..500 {
            env.push_scope();
            env.define(format!("v{}", i), int(i));
        }
        assert!(env.get("not_a_variable").is_none());
        assert_eq!(as_int(env.get("v0")), Some(0));
        assert_eq!(as_int(env.get("v499")), Some(499));
    }

    #[test]
    fn get_mut_reaches_the_innermost_binding() {
        let mut env = Environment::new();
        env.define("x".into(), int(1));
        env.push_scope();
        env.define("x".into(), int(2));
        if let Some(v) = env.get_mut("x") {
            *v = int(7);
        }
        assert_eq!(as_int(env.get("x")), Some(7));
        env.pop_scope();
        assert_eq!(as_int(env.get("x")), Some(1));
    }
}
