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
pub mod capabilities;
pub mod semver;
pub mod features;
#[cfg(not(target_arch = "wasm32"))]
pub mod lockfile;
#[cfg(not(target_arch = "wasm32"))]
pub mod registry;
#[cfg(not(target_arch = "wasm32"))]
pub mod cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod tarball;
#[cfg(not(target_arch = "wasm32"))]
pub mod advisory;
#[cfg(not(target_arch = "wasm32"))]
pub mod signing;

pub use error::{ResolveError, ResolveErrorKind};
pub use scope::{Scope, ScopeId, ScopeKind};
pub use symbol::{Symbol, SymbolId, SymbolKind, SymbolTable, BuiltinFunctionKind};
pub use resolver::Resolver;
pub use package::{Package, PackageId, PackageRegistry, PackageError, SourceFile};
#[cfg(not(target_arch = "wasm32"))]
pub use lockfile::LockFile;

use rask_ast::decl::Decl;
use rask_ast::NodeId;
use std::collections::HashMap;

/// Names of all builtin stdlib modules available via `import`.
pub const BUILTIN_MODULE_NAMES: &[&str] = &[
    "io", "fs", "cli", "std", "json", "random", "time", "math",
    "path", "os", "net", "core", "async", "cfg", "http",
];

/// The result of name resolution.
#[derive(Debug, Default)]
pub struct ResolvedProgram {
    /// All symbols declared in the program.
    pub symbols: SymbolTable,
    /// Mapping from AST nodes (identifier usages) to their resolved symbols.
    pub resolutions: HashMap<NodeId, SymbolId>,
    /// Public type declarations from external packages, keyed by package name.
    /// The type checker registers these so cross-package types resolve.
    pub external_decls: HashMap<String, Vec<Decl>>,
}

/// Extern function signature extracted from C imports or explicit `extern "C"` decls.
#[derive(Debug, Clone)]
pub struct CImportExternFunc {
    pub name: String,
    pub params: Vec<String>,
    pub ret_ty: Option<String>,
}

impl ResolvedProgram {
    /// Extract all extern "C" function names and signatures from C import namespaces.
    /// Used by MIR lowering (extern_funcs set) and codegen (extern function declarations).
    pub fn c_import_extern_funcs(&self) -> Vec<CImportExternFunc> {
        let mut result = Vec::new();
        for sym in self.symbols.iter() {
            if let SymbolKind::CNamespace { members } = &sym.kind {
                for (_, &member_id) in members {
                    if let Some(member) = self.symbols.get(member_id) {
                        if let SymbolKind::ExternFunction { abi, params, ret_ty } = &member.kind {
                            if abi == "C" {
                                result.push(CImportExternFunc {
                                    name: member.name.clone(),
                                    params: params.clone(),
                                    ret_ty: ret_ty.clone(),
                                });
                            }
                        }
                    }
                }
            }
        }
        result
    }
}

/// Resolve all names in a list of declarations (single-file mode).
pub fn resolve(decls: &[Decl]) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve(decls)
}

/// Resolve with cfg values for dead branch elimination in `comptime if`.
pub fn resolve_with_cfg(
    decls: &[Decl],
    cfg_values: HashMap<String, String>,
) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve_with_cfg(decls, cfg_values)
}

/// Resolve stdlib definition files — skips E0209 builtin shadowing checks.
pub fn resolve_stdlib(decls: &[Decl]) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve_stdlib(decls)
}

/// Resolve all names in a package with access to other packages (multi-file mode).
pub fn resolve_package(
    decls: &[Decl],
    registry: &PackageRegistry,
    current_package: PackageId,
) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve_package(decls, registry, current_package)
}

/// Resolve a package with cfg values for dead branch elimination.
pub fn resolve_package_with_cfg(
    decls: &[Decl],
    registry: &PackageRegistry,
    current_package: PackageId,
    cfg_values: HashMap<String, String>,
) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve_package_with_cfg(decls, registry, current_package, cfg_values)
}

/// Resolve a package with separate stdlib declarations. Stdlib decls are
/// processed in stdlib_mode (bypasses builtin-shadowing checks).
pub fn resolve_package_with_stdlib(
    decls: &[Decl],
    registry: &PackageRegistry,
    current_package: PackageId,
    stdlib_decls: &[Decl],
) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve_package_with_stdlib(decls, registry, current_package, stdlib_decls)
}

/// Resolve a package with stdlib declarations and cfg values for
/// dead branch elimination in `comptime if`.
pub fn resolve_package_with_stdlib_and_cfg(
    decls: &[Decl],
    registry: &PackageRegistry,
    current_package: PackageId,
    stdlib_decls: &[Decl],
    cfg_values: HashMap<String, String>,
) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve_package_with_stdlib_and_cfg(decls, registry, current_package, stdlib_decls, cfg_values)
}
