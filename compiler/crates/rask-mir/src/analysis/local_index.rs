// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! O(1) local lookup by ID — replaces repeated `.iter().find(|l| l.id == id)`.

use std::collections::HashMap;

use crate::{LocalId, MirType};
use crate::function::MirLocal;

/// Pre-built index for fast local lookups.
pub struct LocalIndex {
    types: HashMap<LocalId, MirType>,
}

impl LocalIndex {
    /// Build from a function's local list.
    pub fn new(locals: &[MirLocal]) -> Self {
        let types = locals.iter()
            .map(|l| (l.id, l.ty.clone()))
            .collect();
        Self { types }
    }

    /// Get the MIR type for a local. Panics if the local doesn't exist.
    pub fn ty(&self, id: LocalId) -> &MirType {
        self.types.get(&id)
            .unwrap_or_else(|| panic!("LocalIndex: unknown local {:?}", id))
    }

    /// Get the MIR type for a local, returning None if not found.
    pub fn try_ty(&self, id: LocalId) -> Option<&MirType> {
        self.types.get(&id)
    }
}
