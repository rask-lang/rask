// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Name resolution for the Rask language.
//!
//! This crate resolves all identifiers in the AST to their declarations,
//! producing a mapping from AST NodeIds to SymbolIds.

mod error;
mod scope;
mod symbol;
mod resolver;
pub mod package;

pub use error::{ResolveError, ResolveErrorKind};
pub use scope::{Scope, ScopeId, ScopeKind};
pub use symbol::{Symbol, SymbolId, SymbolKind, SymbolTable};
pub use resolver::Resolver;
pub use package::{Package, PackageId, PackageRegistry, PackageError, SourceFile};

use rask_ast::decl::Decl;
use rask_ast::NodeId;
use std::collections::HashMap;

/// The result of name resolution.
#[derive(Debug, Default)]
pub struct ResolvedProgram {
    /// All symbols declared in the program.
    pub symbols: SymbolTable,
    /// Mapping from AST nodes (identifier usages) to their resolved symbols.
    pub resolutions: HashMap<NodeId, SymbolId>,
}

/// Resolve all names in a list of declarations (single-file mode).
pub fn resolve(decls: &[Decl]) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve(decls)
}

/// Resolve all names in a package with access to other packages (multi-file mode).
pub fn resolve_package(
    decls: &[Decl],
    registry: &PackageRegistry,
    current_package: PackageId,
) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve_package(decls, registry, current_package)
}
