// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Linear resource tracking for the interpreter.
//!
//! Tracks resource lifetimes to enforce that `@resource` types (like File)
//! are consumed exactly once before scope exit.

use std::collections::HashMap;
use std::sync::OnceLock;

use rask_ast::Span;

/// `RASK_RUNTIME_CHECKS=1`, the switch compiled code reads for its extra checks.
pub fn runtime_checks_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("RASK_RUNTIME_CHECKS").is_ok_and(|v| v.starts_with('1'))
    })
}

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
    /// Where it was made, or failing that first bound — for the leak report.
    born: Option<Span>,
}

/// A linear value still live when its scope ended.
#[derive(Debug)]
pub struct Leaked {
    pub type_name: String,
    pub var_name: Option<String>,
    pub born: Option<Span>,
}

/// Tracks linear resource lifetimes across scopes.
#[derive(Debug)]
pub struct ResourceTracker {
    entries: HashMap<u64, ResourceEntry>,
    /// Map Arc pointer addresses to resource IDs (for Value::File).
    file_ids: HashMap<usize, u64>,
    /// Map Arc pointer addresses to resource IDs (for Handle).
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
            born: None,
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

    /// Set the variable name for a resource (for error messages). `at` is the
    /// binding, which stands in for the birthplace when nothing recorded one.
    pub fn set_var_name(&mut self, id: u64, name: String, at: Span) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.var_name = Some(name);
            entry.born.get_or_insert(at);
        }
    }

    /// Record where a resource was made.
    pub fn set_born(&mut self, id: u64, at: Span) {
        if let Some(entry) = self.entries.get_mut(&id) {
            entry.born = Some(at);
        }
    }

    /// Check if a resource has already been consumed.
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
    /// `outward_only` leaves an entry already owned further out alone.
    pub fn transfer_to_scope(&mut self, id: u64, new_scope_depth: usize, outward_only: bool) {
        if let Some(entry) = self.entries.get_mut(&id) {
            if !outward_only || new_scope_depth < entry.scope_depth {
                entry.scope_depth = new_scope_depth;
            }
        }
    }

    /// Register a Handle using its Arc pointer address (conc.async/H1).
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

    /// Forget every entry registered at this scope depth, returning the ones
    /// still live.
    ///
    /// An unconsumed linear value is a compile error (L1–L7, RC1–RC4), so the
    /// list should always be empty. Callers only look at it under
    /// `RASK_RUNTIME_CHECKS`, as a debugging aid (rask-lang/rask#1296).
    pub fn end_scope(&mut self, scope_depth: usize) -> Vec<Leaked> {
        let ended: Vec<u64> = self.entries.iter()
            .filter(|(_, e)| e.scope_depth == scope_depth)
            .map(|(&id, _)| id)
            .collect();
        let mut leaked = Vec::new();
        for id in &ended {
            self.file_ids.retain(|_, v| v != id);
            self.handle_ids.retain(|_, v| v != id);
            if let Some(e) = self.entries.remove(id) {
                if e.state == ResourceState::Live {
                    leaked.push(Leaked { type_name: e.type_name, var_name: e.var_name, born: e.born });
                }
            }
        }
        leaked
    }
}

#[cfg(test)]
mod tests {
    use crate::{Interpreter, RuntimeError};

    /// A `@resource` value made and never consumed. The checker rejects this
    /// (E0882), and nothing found gets one past it, so the parse goes straight
    /// to the interpreter to reach the runtime check at all.
    const LEAKS: &str = "\
@resource
struct Conn {
    id: i32
}

extend Conn {
    func close(take self) {
    }
}

func main() {
    let c = Conn { id: 1 }
}
";

    fn run(src: &str, checks: bool) -> Result<(), RuntimeError> {
        let lexed = rask_lexer::Lexer::new(src).tokenize();
        assert!(lexed.errors.is_empty(), "{:?}", lexed.errors);
        let parsed = rask_parser::Parser::new(lexed.tokens).parse();
        assert!(parsed.errors.is_empty(), "{:?}", parsed.errors);
        let (mut interp, _out) = Interpreter::with_captured_output();
        interp.set_source_info("leak.rk", src);
        interp.set_runtime_checks(checks);
        interp.run(&parsed.decls).map(|_| ()).map_err(|d| d.error)
    }

    #[test]
    fn leak_panics_with_runtime_checks() {
        match run(LEAKS, true) {
            Err(RuntimeError::Panic(msg)) => {
                assert!(msg.contains("Conn 'c'"), "names what leaked: {msg}");
                assert!(msg.contains("made at leak.rk:12"), "names where it was made: {msg}");
                assert!(msg.contains("ended at leak.rk:13"), "names where the scope ended: {msg}");
            }
            other => panic!("expected a leak panic, got {:?}", other),
        }
    }

    #[test]
    fn leak_is_silent_without_runtime_checks() {
        if let Err(e) = run(LEAKS, false) {
            panic!("the check is off, so nothing should fire: {e:?}");
        }
    }

    /// A panic leaves `c` unconsumed because it unwound past the close. That's
    /// not a leak, and the panic in flight is the one to report (ctrl.panic/E3).
    #[test]
    fn unwinding_scope_is_not_checked() {
        let src = LEAKS.replace(
            "    let c = Conn { id: 1 }\n",
            "    let c = Conn { id: 1 }\n    panic(\"boom\")\n",
        );
        match run(&src, true) {
            Err(RuntimeError::Panic(msg)) => {
                assert!(msg.contains("boom") && !msg.contains("leak"), "the first panic wins: {msg}");
            }
            other => panic!("expected the program's own panic, got {:?}", other),
        }
    }
}
