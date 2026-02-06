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
struct ResourceEntry {
    type_name: String,
    var_name: Option<String>,
    state: ResourceState,
    scope_depth: usize,
}

/// Tracks linear resource lifetimes across scopes.
#[derive(Debug)]
pub struct ResourceTracker {
    entries: HashMap<u64, ResourceEntry>,
    /// Map Arc pointer addresses to resource IDs (for Value::File).
    file_ids: HashMap<usize, u64>,
    next_id: u64,
}

impl ResourceTracker {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            file_ids: HashMap::new(),
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

    /// Transfer a resource to a different scope depth (for returns/moves).
    pub fn transfer_to_scope(&mut self, id: u64, new_scope_depth: usize) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.scope_depth = new_scope_depth;
        }
    }

    /// Check for unconsumed resources at the given scope depth.
    /// Returns Err listing leaked resources, or Ok if all consumed.
    /// Removes all entries at this scope depth regardless.
    pub fn check_scope_exit(&mut self, scope_depth: usize) -> Result<(), String> {
        let mut leaked: Vec<String> = Vec::new();
        let mut to_remove: Vec<u64> = Vec::new();

        for (&id, entry) in &self.entries {
            if entry.scope_depth == scope_depth {
                if entry.state == ResourceState::Live {
                    let var = entry.var_name.as_deref().unwrap_or("?");
                    leaked.push(format!("{} '{}'", entry.type_name, var));
                }
                to_remove.push(id);
            }
        }

        // Clean up entries at this scope depth
        for id in &to_remove {
            // Also clean up file_ids
            self.file_ids.retain(|_, v| v != id);
            self.entries.remove(id);
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
