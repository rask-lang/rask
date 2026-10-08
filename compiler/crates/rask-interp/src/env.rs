// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Environment for variable bindings.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use crate::value::{MapData, StructData, Value, VecData};

/// Storage a name is bound to: a variable of its own, or a place inside
/// another value.
///
/// A binding is a *slot*, not a value. The distinction is invisible until a
/// closure captures the name: a closure that stays in its frame borrows the
/// variable (`mem.closures/CM1`), so it has to reach the same storage the
/// definer writes. Binding names to values instead made a capture a copy, and
/// every write through it landed on the copy (#1038).
///
/// The other three are what a `mutate` parameter binds when the argument is a
/// field, an element or a map entry (`mutate b.n`, `mutate v[i]`). The
/// parameter is that place, the way native passes its address: copying it in
/// and back out at the return lost every write made after the return, by a
/// Sequence driven later (#1489). Containers already share their storage
/// through an `Arc`, so a place is the container plus where in it.
#[derive(Clone, Debug)]
pub enum Slot {
    Var(Arc<Mutex<Value>>),
    Field(Arc<Mutex<StructData>>, String),
    Elem(Arc<Mutex<VecData>>, usize),
    /// A map entry by position. A position stays put while the entry is
    /// borrowed: nothing can remove from the map until the borrow ends.
    Entry(Arc<Mutex<MapData>>, usize),
}

/// Wrap a value in fresh storage.
pub fn slot(value: Value) -> Slot {
    Slot::Var(Arc::new(Mutex::new(value)))
}

impl Slot {
    /// The value stored here. `None` only for an element or entry that is no
    /// longer there.
    pub fn get(&self) -> Option<Value> {
        match self {
            Slot::Var(cell) => Some(cell.lock().unwrap().clone()),
            Slot::Field(s, field) => s.lock().unwrap().fields.get(field).cloned(),
            Slot::Elem(v, i) => v.lock().unwrap().items.get(*i).cloned(),
            Slot::Entry(m, i) => m.lock().unwrap().get_index(*i).map(|(_, v)| v.clone()),
        }
    }

    /// Replace the value stored here. A link written into a field or an
    /// element records its backlink, as an assignment there does.
    pub fn set(&self, value: Value) -> bool {
        match self {
            Slot::Var(cell) => {
                *cell.lock().unwrap() = value;
                true
            }
            Slot::Field(s, field) => {
                let previous = s.lock().unwrap().fields.insert(field.clone(), value.clone());
                crate::rack::register_field(s, field, previous.as_ref(), &value);
                true
            }
            Slot::Elem(v, i) => {
                let mut vec = v.lock().unwrap();
                let Some(item) = vec.items.get_mut(*i) else { return false };
                *item = value.clone();
                drop(vec);
                crate::rack::register_element(v, &value);
                true
            }
            Slot::Entry(m, i) => match m.lock().unwrap().get_index_mut(*i) {
                Some((_, item)) => {
                    *item = value;
                    true
                }
                None => false,
            },
        }
    }
}

/// A scope in the environment.
#[derive(Debug, Default)]
struct Scope {
    bindings: HashMap<String, Slot>,
    /// Names bound to storage this frame borrows from its caller: a `mutate`
    /// parameter, or a closure's capture of one.
    lent: HashSet<String>,
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
        scope.lent.remove(&name);
        if scope.bindings.insert(name.clone(), cell).is_none() {
            self.defined_at.entry(name).or_default().push(index);
        }
    }

    /// Bind a name to storage the frame borrows rather than owns — the
    /// caller's variable behind a `mutate` parameter (`mem.closures/CM3`).
    /// A closure that outlives this frame still shares it instead of copying,
    /// so its writes reach the caller whenever it runs.
    pub fn define_lent(&mut self, name: String, cell: Slot) {
        self.define_slot(name.clone(), cell);
        if let Some(scope) = self.scopes.last_mut() {
            scope.lent.insert(name);
        }
    }

    /// Whether the innermost binding of `name` is borrowed storage.
    fn is_lent(&self, name: &str) -> bool {
        let Some(index) = self.defined_at.get(name).and_then(|ix| ix.last()) else {
            return false;
        };
        self.scopes.get(*index).is_some_and(|s| s.lent.contains(name))
    }

    /// Every visible name whose binding is borrowed storage.
    pub fn lent_names(&self) -> HashSet<String> {
        self.defined_at.keys().filter(|n| self.is_lent(n)).cloned().collect()
    }

    /// Read a variable's current value.
    pub fn get(&self, name: &str) -> Option<Value> {
        self.slot_of(name)?.get()
    }

    /// The storage a name is bound to, for sharing it with a closure.
    pub fn slot_of(&self, name: &str) -> Option<&Slot> {
        let index = *self.defined_at.get(name)?.last()?;
        self.scopes.get(index)?.bindings.get(name)
    }

    /// Assign to an existing variable, in place.
    pub fn assign(&mut self, name: &str, value: Value) -> bool {
        self.slot_of(name).is_some_and(|cell| cell.set(value))
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

    /// Get the current scope depth.
    pub fn scope_depth(&self) -> usize {
        self.scopes.len()
    }

    /// Share every visible variable's storage — a scope-limited closure's
    /// captures. Writes through the closure land on the definer's variable,
    /// which is what "borrows outer variables" means.
    pub fn capture_shared(&self) -> HashMap<String, Slot> {
        let mut captured = HashMap::new();
        for scope in &self.scopes {
            for (name, cell) in &scope.bindings {
                captured.insert(name.clone(), cell.clone());
            }
        }
        captured
    }

    /// Copy every visible variable into storage of its own — the captures of a
    /// closure that outlives its frame. It may not alias the definer's locals:
    /// it carries them (`mem.closures/CM2`), and the frame is going away.
    ///
    /// Borrowed storage is the exception. A `mutate` parameter is the caller's
    /// variable, so the closure borrows it like the frame did (CM3); copying it
    /// sent every write the closure made later to a copy nobody reads (#1324).
    pub fn capture_snapshot(&self) -> HashMap<String, Slot> {
        let mut captured = HashMap::new();
        for scope in &self.scopes {
            for (name, cell) in &scope.bindings {
                let cell = if scope.lent.contains(name) {
                    cell.clone()
                } else {
                    slot(cell.get().unwrap_or(Value::Unit))
                };
                captured.insert(name.clone(), cell);
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
            env.define_slot(name.clone(), cell.clone());
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
            env.define_slot(name.clone(), cell.clone());
        }
        env.assign("a", int(5));
        env.pop_scope();

        assert_eq!(as_int(env.get("a")), Some(1), "the definer is untouched");
    }

    // A `mutate` parameter's storage is the caller's, so even a carrying
    // closure reaches it (#1324).
    #[test]
    fn a_snapshot_shares_lent_storage() {
        let mut env = Environment::new();
        let caller = slot(int(1));
        env.define_lent("a".into(), caller.clone());
        env.define("b".into(), int(1));
        assert_eq!(env.lent_names(), HashSet::from(["a".to_string()]));
        let captured = env.capture_snapshot();
        captured["a"].set(int(5));
        captured["b"].set(int(5));
        assert_eq!(as_int(caller.get()), Some(5));
        assert_eq!(as_int(env.get("b")), Some(1));
    }

    #[test]
    fn rebinding_a_lent_name_owns_it_again() {
        let mut env = Environment::new();
        env.define_lent("a".into(), slot(int(1)));
        env.define("a".into(), int(2));
        assert!(env.lent_names().is_empty());
    }

    // `mutate b.n`: the parameter is the field, so a write through it, at any
    // time, is a write to the caller's struct (#1489).
    #[test]
    fn a_field_slot_reads_and_writes_the_struct() {
        let data = StructData {
            name: "Box2".into(),
            fields: [("n".to_string(), int(1))].into_iter().collect(),
            resource_id: None,
        };
        let s = Arc::new(Mutex::new(data));
        let mut env = Environment::new();
        env.define_lent("n".into(), Slot::Field(Arc::clone(&s), "n".into()));
        assert_eq!(as_int(env.get("n")), Some(1));
        assert!(env.assign("n", int(7)));
        assert_eq!(as_int(s.lock().unwrap().fields.get("n").cloned()), Some(7));
    }
}
