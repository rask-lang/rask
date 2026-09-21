// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Linear resource tracking for the interpreter.
//!
//! Tracks resource lifetimes to enforce that `@resource` types (like File)
//! are consumed exactly once before scope exit.

use std::collections::HashMap;

/// State of a tracked resource.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResourceState {
    /// Resource is alive and unconsumed.
    Live,
    /// Resource was consumed (closed, passed with `take self`, etc.).
    Consumed,
}

/// A tracked resource entry.
#[derive(Debug)]
pub struct ResourceEntry {
    type_name: String,
    var_name: Option<String>,
    state: ResourceState,
    scope_depth: usize,
    /// Handed to a pool, so `mem.resources/R5` is what reports it rather than
    /// the ordinary "this binding was never consumed" message. A pooled value
    /// has no binding to name — the report used to read `Conn '?'`.
    pooled: bool,
}

/// Tracks linear resource lifetimes across scopes.
#[derive(Debug)]
pub struct ResourceTracker {
    entries: HashMap<u64, ResourceEntry>,
    /// Map Arc pointer addresses to resource IDs (for Value::File).
    file_ids: HashMap<usize, u64>,
    /// Map Arc pointer addresses to resource IDs (for TaskHandle/ThreadHandle).
    handle_ids: HashMap<usize, u64>,
    next_id: u64,
}

impl ResourceTracker {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            file_ids: HashMap::new(),
            handle_ids: HashMap::new(),
            next_id: 1,
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Register a new resource (for @resource structs). Returns the assigned ID.
    pub fn register(&mut self, type_name: &str, scope_depth: usize) -> u64 {
        let id = self.alloc_id();
        self.entries.insert(id, ResourceEntry {
            type_name: type_name.to_string(),
            var_name: None,
            state: ResourceState::Live,
            scope_depth,
            pooled: false,
        });
        id
    }

    /// Register a File resource using its Arc pointer address. Returns the assigned ID.
    pub fn register_file(&mut self, ptr: usize, scope_depth: usize) -> u64 {
        let id = self.register("File", scope_depth);
        self.file_ids.insert(ptr, id);
        id
    }

    /// Look up the resource ID for a File by its Arc pointer address.
    pub fn lookup_file_id(&self, ptr: usize) -> Option<u64> {
        self.file_ids.get(&ptr).copied()
    }

    /// Set the variable name for a resource (for error messages).
    pub fn set_var_name(&mut self, id: u64, name: String) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.var_name = Some(name);
        }
    }

    /// Check if a resource has already been consumed.
    /// R5: the value went into a pool, so the pool is what owes it now.
    pub fn mark_pooled(&mut self, id: u64) {
        if let Some(e) = self.entries.get_mut(&id) {
            e.pooled = true;
        }
    }

    pub fn is_consumed(&self, id: u64) -> bool {
        self.entries.get(&id)
            .map(|e| e.state == ResourceState::Consumed)
            .unwrap_or(false)
    }

    /// Mark a resource as consumed. Returns Err if already consumed.
    pub fn mark_consumed(&mut self, id: u64) -> Result<(), String> {
        if let Some(entry) = self.entries.get_mut(&id) {
            if entry.state == ResourceState::Consumed {
                let var = entry.var_name.as_deref().unwrap_or("unknown");
                return Err(format!(
                    "resource already consumed: {} '{}'",
                    entry.type_name, var
                ));
            }
            entry.state = ResourceState::Consumed;
            Ok(())
        } else {
            // Unknown resource ID — not tracked, ignore
            Ok(())
        }
    }

    /// Hand a consumed resource back to a new owner. A `take self` method
    /// receives the resource and may pass it on to another one — that is one
    /// move per owner, not two consumptions of the same value, so the callee's
    /// frame sees it live again (mem.linear/L2).
    pub fn revive(&mut self, id: u64) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.state = ResourceState::Live;
        }
    }

    /// Nothing is being tracked, so nothing needs walking.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Transfer a resource to a different scope depth (for returns/moves).
    pub fn transfer_to_scope(&mut self, id: u64, new_scope_depth: usize) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.scope_depth = new_scope_depth;
        }
    }

    /// Register a TaskHandle/ThreadHandle using its Arc pointer address (conc.async/H1).
    pub fn register_handle(&mut self, ptr: usize, type_name: &str, scope_depth: usize) -> u64 {
        let id = self.register(type_name, scope_depth);
        self.handle_ids.insert(ptr, id);
        id
    }

    /// Look up the resource ID for a handle by its Arc pointer address.
    pub fn lookup_handle_id(&self, ptr: usize) -> Option<u64> {
        self.handle_ids.get(&ptr).copied()
    }

    /// Re-point a file pointer at an id this tracker was handed.
    pub fn register_file_id(&mut self, ptr: usize, id: u64) {
        self.file_ids.insert(ptr, id);
    }

    /// Re-point a handle pointer at an id this tracker was handed.
    pub fn register_handle_id(&mut self, ptr: usize, id: u64) {
        self.handle_ids.insert(ptr, id);
    }

    /// Take a resource's whole entry out of this tracker, keeping its id.
    ///
    /// For handing one across a task boundary. A task runs on its own
    /// `Interpreter`, so it has its own tracker; without this the parent went
    /// on owing a resource the task had already closed, and reported a leak
    /// for a program that was correct (#882). Moving the entry rather than
    /// re-registering keeps the id, which is what the value itself carries —
    /// a fresh id would make the task's `close()` look up nothing and succeed
    /// silently.
    pub fn take_entry(&mut self, id: u64) -> Option<ResourceEntry> {
        let entry = self.entries.remove(&id)?;
        self.file_ids.retain(|_, v| *v != id);
        self.handle_ids.retain(|_, v| *v != id);
        Some(entry)
    }

    /// Put an entry taken from another tracker in at the given scope depth.
    pub fn insert_entry(&mut self, id: u64, mut entry: ResourceEntry, scope_depth: usize) {
        entry.scope_depth = scope_depth;
        self.entries.insert(id, entry);
        self.next_id = self.next_id.max(id + 1);
    }

    /// Check for unconsumed resources at the given scope depth.
    /// Returns Err listing leaked resources, or Ok if all consumed.
    /// Removes all entries at this scope depth regardless.
    pub fn check_scope_exit(&mut self, scope_depth: usize) -> Result<(), String> {
        let mut leaked: Vec<String> = Vec::new();
        let mut to_remove: Vec<u64> = Vec::new();

        // R5's are counted per element type rather than listed: a pooled value
        // has no binding to name, and the pool is what is being reported.
        let mut pooled: std::collections::BTreeMap<String, usize> =
            std::collections::BTreeMap::new();
        for (&id, entry) in &self.entries {
            if entry.scope_depth == scope_depth {
                if entry.state == ResourceState::Live {
                    if entry.pooled {
                        *pooled.entry(entry.type_name.clone()).or_insert(0) += 1;
                    } else {
                        let var = entry.var_name.as_deref().unwrap_or("?");
                        leaked.push(format!("{} '{}'", entry.type_name, var));
                    }
                }
                to_remove.push(id);
            }
        }

        // Clean up entries at this scope depth
        for id in &to_remove {
            // Also clean up file_ids and handle_ids
            self.file_ids.retain(|_, v| v != id);
            self.handle_ids.retain(|_, v| v != id);
            self.entries.remove(id);
        }

        // Keep this wording in step with the runtime's (`rask_pool_free` in
        // runtime/pool.c) — the differential harness compares the two backends'
        // output verbatim.
        if let Some((ty, n)) = pooled.into_iter().next() {
            return Err(format!(
                "Pool<{}> has {} unconsumed resource element{} at scope exit.\n\
                 Resources must be explicitly consumed (use take_all() before scope ends).",
                ty,
                n,
                if n == 1 { "" } else { "s" }
            ));
        }
        if leaked.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "resource leak: {} not consumed before scope exit",
                leaked.join(", ")
            ))
        }
    }
}
