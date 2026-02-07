// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Scope tree for name resolution.

use std::collections::HashMap;
use crate::symbol::SymbolId;
use crate::error::ResolveError;
use rask_ast::Span;

/// Unique identifier for a scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScopeId(pub u32);

/// The kind of scope.
#[derive(Debug, Clone)]
pub enum ScopeKind {
    /// Global scope (top-level declarations).
    Global,
    /// Function scope.
    Function(SymbolId),
    /// Block scope (within a function).
    Block,
    /// Loop scope (for break/continue validation).
    Loop { label: Option<String> },
    /// Closure scope.
    Closure,
}

/// A scope in the scope tree.
#[derive(Debug)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    pub bindings: HashMap<String, SymbolId>,
}

/// Tree of scopes for name lookup.
#[derive(Debug)]
pub struct ScopeTree {
    scopes: Vec<Scope>,
    current: ScopeId,
}

impl ScopeTree {
    /// Create a new scope tree with a global scope.
    pub fn new() -> Self {
        let global = Scope {
            id: ScopeId(0),
            parent: None,
            kind: ScopeKind::Global,
            bindings: HashMap::new(),
        };
        Self {
            scopes: vec![global],
            current: ScopeId(0),
        }
    }

    /// Push a new scope.
    pub fn push(&mut self, kind: ScopeKind) -> ScopeId {
        let id = ScopeId(self.scopes.len() as u32);
        let scope = Scope {
            id,
            parent: Some(self.current),
            kind,
            bindings: HashMap::new(),
        };
        self.scopes.push(scope);
        self.current = id;
        id
    }

    /// Pop the current scope and return to parent.
    pub fn pop(&mut self) {
        if let Some(scope) = self.scopes.get(self.current.0 as usize) {
            if let Some(parent) = scope.parent {
                self.current = parent;
            }
        }
    }

    /// Get the current scope ID.
    #[allow(dead_code)]
    pub fn current(&self) -> ScopeId {
        self.current
    }

    /// Get a scope by ID.
    #[allow(dead_code)]
    pub fn get(&self, id: ScopeId) -> Option<&Scope> {
        self.scopes.get(id.0 as usize)
    }

    /// Look up a name in the current scope chain.
    pub fn lookup(&self, name: &str) -> Option<SymbolId> {
        let mut scope_id = Some(self.current);
        while let Some(id) = scope_id {
            if let Some(scope) = self.scopes.get(id.0 as usize) {
                if let Some(&symbol) = scope.bindings.get(name) {
                    return Some(symbol);
                }
                scope_id = scope.parent;
            } else {
                break;
            }
        }
        None
    }

    /// Define a name in the current scope.
    /// Shadowing is allowed - a new binding replaces the previous one.
    pub fn define(&mut self, name: String, symbol: SymbolId, _span: Span) -> Result<(), ResolveError> {
        let scope = &mut self.scopes[self.current.0 as usize];
        // Shadowing is allowed in Rask - just replace the existing binding
        scope.bindings.insert(name, symbol);
        Ok(())
    }

    /// Check if we're currently inside a loop.
    pub fn in_loop(&self) -> bool {
        self.find_loop_scope(None).is_some()
    }

    /// Check if a labeled loop exists in the scope chain.
    pub fn label_in_scope(&self, label: &str) -> bool {
        self.find_loop_scope(Some(label)).is_some()
    }

    /// Find a loop scope in the chain, optionally matching a label.
    fn find_loop_scope(&self, label: Option<&str>) -> Option<ScopeId> {
        let mut scope_id = Some(self.current);
        while let Some(id) = scope_id {
            if let Some(scope) = self.scopes.get(id.0 as usize) {
                if let ScopeKind::Loop { label: loop_label } = &scope.kind {
                    match (label, loop_label) {
                        (None, _) => return Some(id), // Any loop matches
                        (Some(l), Some(ll)) if l == ll => return Some(id),
                        _ => {}
                    }
                }
                // Stop at function boundaries for break/continue
                if matches!(scope.kind, ScopeKind::Function(_)) {
                    return None;
                }
                scope_id = scope.parent;
            } else {
                break;
            }
        }
        None
    }

    /// Check if we're inside a function.
    pub fn in_function(&self) -> bool {
        let mut scope_id = Some(self.current);
        while let Some(id) = scope_id {
            if let Some(scope) = self.scopes.get(id.0 as usize) {
                if matches!(scope.kind, ScopeKind::Function(_)) {
                    return true;
                }
                scope_id = scope.parent;
            } else {
                break;
            }
        }
        false
    }
}

impl Default for ScopeTree {
    fn default() -> Self {
        Self::new()
    }
}
