// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Type formatting for hover tooltips and completion details.

use rask_types::{GenericArg, Type};
use rask_check::TypeTable;

/// Formats types for human-readable display in hover tooltips.
pub struct TypeFormatter<'a> {
    types: &'a TypeTable,
}

impl<'a> TypeFormatter<'a> {
    pub fn new(types: &'a TypeTable) -> Self {
        Self { types }
    }

    /// The type as a diagnostic shows it: each name as the program wrote it,
    /// `libpkg.Cat` rather than the symbol `libpkg_Cat`. This had a renderer
    /// of its own, which didn't know written names.
    pub fn format(&self, ty: &Type) -> String {
        self.types.display_type_names(ty).to_string()
    }

    pub fn format_generic_arg(&self, arg: &GenericArg) -> String {
        match arg {
            GenericArg::Type(ty) => self.format(ty),
            GenericArg::ConstUsize(n) => n.to_string(),
        }
    }
}
