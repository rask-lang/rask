// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Environment for variable bindings.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use crate::value::Value;

/// A variable's storage, shared by everything bound to that variable.
///
/// A binding is a *slot*, not a value. The distinction is invisible until a
/// closure captures the name: a scope-limited closure borrows the variable
/// (`rask_ast::ExprKind::Closure::is_own` — "without this flag the closure
/// borrows outer variables"), so it has to reach the same storage the definer
/// writes. Binding names to values instead made a capture a copy, and every
/// write through it landed on the copy (#1038).
pub type Slot = Arc<Mutex<Value>>;

/// Wrap a value in fresh storage.
pub fn slot(value: Value) -> Slot {
    Arc::new(Mutex::new(value))
}

/// A scope in the environment.
#[derive(Debug, Default)]
struct Scope {
    bindings: HashMap<String, Slot>,
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

    /// Define a variable in the current scope, in storage of its own.
    pub fn define(&mut self, name: String, value: Value) {
        self.define_slot(name, slot(value));
    }

    /// Bind a name to storage that already exists.
    ///
    /// This is what makes a capture a borrow: the closure's scope binds the
    /// definer's slot, so a write through either name is the same write.
    pub fn define_slot(&mut self, name: String, cell: Slot) {
        let index = self.scopes.len().saturating_sub(1);
        let Some(scope) = self.scopes.last_mut() else { return };
        // Redefining in the same scope replaces the binding; the index already
        // has an entry for it and must not get a second one.
        if scope.bindings.insert(name.clone(), cell).is_none() {
            self.defined_at.entry(name).or_default().push(index);
        }
    }

    /// Read a variable's current value.
    pub fn get(&self, name: &str) -> Option<Value> {
        Some(self.slot_of(name)?.lock().unwrap().clone())
    }

    /// The storage a name is bound to, for sharing it with a closure.
    pub fn slot_of(&self, name: &str) -> Option<&Slot> {
        let index = *self.defined_at.get(name)?.last()?;
        self.scopes.get(index)?.bindings.get(name)
    }

    /// Assign to an existing variable, in place.
    pub fn assign(&mut self, name: &str, value: Value) -> bool {
        let Some(cell) = self.slot_of(name) else { return false };
        *cell.lock().unwrap() = value;
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

    /// Apply `f` to a variable's value in place (for field assignment).
    ///
    /// The slot is locked for the length of `f`, so `f` must not reach back
    /// into the environment for the same name.
    pub fn with_mut<R>(&mut self, name: &str, f: impl FnOnce(&mut Value) -> R) -> Option<R> {
        let cell = self.slot_of(name)?.clone();
        let mut guard = cell.lock().unwrap();
        Some(f(&mut guard))
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
            for cell in scope.bindings.values() {
                let value = cell.lock().unwrap();
                if let Some(found) = f(&value) {
                    return Some(found);
                }
            }
        }
        None
    }

    /// Share every visible variable's storage — a scope-limited closure's
    /// captures. Writes through the closure land on the definer's variable,
    /// which is what "borrows outer variables" means.
    pub fn capture_shared(&self) -> HashMap<String, Slot> {
        let mut captured = HashMap::new();
        for scope in &self.scopes {
            for (name, cell) in &scope.bindings {
                captured.insert(name.clone(), Arc::clone(cell));
            }
        }
        captured
    }

    /// Copy every visible variable into storage of its own — an `own` closure's
    /// captures, and a spawned task's. Neither may alias the definer: `own`
    /// captures by move and outlives its creation scope, and a task that shared
    /// its parent's locals would be a data race.
    pub fn capture_snapshot(&self) -> HashMap<String, Slot> {
        let mut captured = HashMap::new();
        for scope in &self.scopes {
            for (name, cell) in &scope.bindings {
                captured.insert(name.clone(), slot(cell.lock().unwrap().clone()));
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

    fn as_int(v: Option<Value>) -> Option<i64> {
        match v {
            Some(Value::Int(n, _)) => Some(n),
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
    fn with_mut_reaches_the_innermost_binding() {
        let mut env = Environment::new();
        env.define("x".into(), int(1));
        env.push_scope();
        env.define("x".into(), int(2));
        env.with_mut("x", |v| *v = int(7));
        assert_eq!(as_int(env.get("x")), Some(7));
        env.pop_scope();
        assert_eq!(as_int(env.get("x")), Some(1));
    }

    // The capture that #1038 was about: a shared slot means a write through the
    // closure's name is a write to the definer's variable.
    #[test]
    fn a_shared_capture_writes_through_to_the_definer() {
        let mut env = Environment::new();
        env.define("a".into(), int(1));
        let captured = env.capture_shared();

        // What calling the closure does: a fresh scope binding the same storage.
        env.push_scope();
        for (name, cell) in &captured {
            env.define_slot(name.clone(), Arc::clone(cell));
        }
        env.assign("a", int(5));
        env.pop_scope();

        assert_eq!(as_int(env.get("a")), Some(5), "the definer sees the write");
    }

    // `own` and `spawn` must not alias — a moved-from local and a parent's
    // locals are both things the closure has no business writing.
    #[test]
    fn a_snapshot_capture_leaves_the_definer_alone() {
        let mut env = Environment::new();
        env.define("a".into(), int(1));
        let captured = env.capture_snapshot();

        env.push_scope();
        for (name, cell) in &captured {
            env.define_slot(name.clone(), Arc::clone(cell));
        }
        env.assign("a", int(5));
        env.pop_scope();

        assert_eq!(as_int(env.get("a")), Some(1), "the definer is untouched");
    }
}
