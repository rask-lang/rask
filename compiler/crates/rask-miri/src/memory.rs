// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Virtual memory model for the MIR interpreter.
//!
//! Stack frames hold locals indexed by LocalId. No heap — comptime
//! doesn't allow pools, raw pointers, or runtime allocations.

use rask_mir::{LocalId, MirFunction, MirType};

use crate::{MiriError, MiriValue};

/// A single stack frame: one per function call.
pub struct StackFrame {
    /// Local variable slots, indexed by `LocalId.0`.
    /// `None` means uninitialized.
    pub locals: Vec<Option<MiriValue>>,
    /// Function name (for error messages).
    pub func_name: String,
    /// Types of each local (for type-aware operations).
    pub local_types: Vec<MirType>,
    /// Ensure cleanup stack: block IDs to execute on scope exit.
    pub cleanup_stack: Vec<rask_mir::BlockId>,
}

impl StackFrame {
    /// Create a frame for the given function, with all locals uninitialized.
    pub fn new(func: &MirFunction) -> Self {
        let local_count = func.locals.len();
        Self {
            locals: vec![None; local_count],
            func_name: func.name.clone(),
            local_types: func.locals.iter().map(|l| l.ty.clone()).collect(),
            cleanup_stack: Vec::new(),
        }
    }

    /// Read a local. Errors if uninitialized.
    pub fn get(&self, id: LocalId) -> Result<&MiriValue, MiriError> {
        let idx = id.0 as usize;
        self.locals
            .get(idx)
            .and_then(|slot| slot.as_ref())
            .ok_or(MiriError::UninitializedLocal(id))
    }

    /// Write a local.
    pub fn set(&mut self, id: LocalId, value: MiriValue) {
        let idx = id.0 as usize;
        if idx >= self.locals.len() {
            self.locals.resize(idx + 1, None);
        }
        self.locals[idx] = Some(value);
    }

    /// Get the type of a local.
    pub fn local_type(&self, id: LocalId) -> Option<&MirType> {
        self.local_types.get(id.0 as usize)
    }
}

/// Call stack: manages nested function calls.
pub struct CallStack {
    pub frames: Vec<StackFrame>,
    /// Maximum call depth (default 256, reasonable for comptime).
    pub max_depth: usize,
}

impl CallStack {
    pub fn new() -> Self {
        Self {
            frames: Vec::new(),
            max_depth: 256,
        }
    }

    pub fn push(&mut self, frame: StackFrame) -> Result<(), MiriError> {
        if self.frames.len() >= self.max_depth {
            return Err(MiriError::StackOverflow);
        }
        self.frames.push(frame);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<StackFrame> {
        self.frames.pop()
    }

    pub fn current(&self) -> Result<&StackFrame, MiriError> {
        self.frames.last().ok_or(MiriError::StackOverflow)
    }

    pub fn current_mut(&mut self) -> Result<&mut StackFrame, MiriError> {
        self.frames.last_mut().ok_or(MiriError::StackOverflow)
    }

    pub fn depth(&self) -> usize {
        self.frames.len()
    }
}
