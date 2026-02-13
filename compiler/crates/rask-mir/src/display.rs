// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Display implementations for MIR types (debugging).

use crate::*;
use std::fmt;

impl fmt::Display for MirFunction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "func {} -> {:?} {{", self.name, self.ret_ty)?;
        for block in &self.blocks {
            writeln!(f, "  block{}:", block.id.0)?;
            for stmt in &block.statements {
                writeln!(f, "    {:?}", stmt)?;
            }
            writeln!(f, "    {:?}", block.terminator)?;
        }
        writeln!(f, "}}")
    }
}
