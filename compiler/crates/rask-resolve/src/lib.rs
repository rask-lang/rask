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
mod diagnostic;

pub use error::{ResolveError, ResolveErrorKind};
pub use scope::{Scope, ScopeId, ScopeKind};
pub use symbol::{Symbol, SymbolId, SymbolKind, SymbolTable, BuiltinFunctionKind};
pub use symbol::{is_builtin_type, is_reserved_builtin_fn, is_always_in_scope};
pub use resolver::Resolver;
pub use package::{ImportIndex, Package, PackageId, PackageRegistry, PackageError, SourceFile};
#[cfg(not(target_arch = "wasm32"))]
pub use lockfile::LockFile;

use rask_ast::decl::Decl;
use rask_ast::{NodeId, Span};
use std::collections::{HashMap, HashSet};

/// True when `name` names a builtin stdlib module.
///
/// Asks `BuiltinModuleKind` rather than consulting a second list of names — the
/// list this replaced had drifted, missing `env` and `thread`.
pub fn is_builtin_module(name: &str) -> bool {
    symbol::BuiltinModuleKind::from_name(name).is_some()
}

/// The result of name resolution.
#[derive(Debug, Default)]
pub struct ResolvedProgram {
    /// All symbols declared in the program.
    pub symbols: SymbolTable,
    /// Mapping from AST nodes (identifier usages) to their resolved symbols.
    pub resolutions: HashMap<NodeId, SymbolId>,
    /// The symbol each top-level function declaration got, by the
    /// declaration's id. Names aren't unique — the stdlib's stubs and bodies
    /// both declare `Wrapping` — so this is the only reliable way back.
    pub decl_symbols: HashMap<NodeId, SymbolId>,
    /// Public type declarations from external packages, keyed by package name.
    /// The type checker registers these so cross-package types resolve.
    pub external_decls: HashMap<String, Vec<Decl>>,
    /// Which package each source file belongs to, by file id.
    ///
    /// A span carries its file id and every package owns a set of them, so this
    /// answers "which package wrote this?" for any declaration — which is what
    /// `type.generics/XC1` needs and what the merged decl list drops. Empty
    /// outside a package build.
    pub file_packages: HashMap<u16, String>,
    /// XC4: every package each package can see, transitively, including itself.
    ///
    /// A conformance is visible to a package iff the package that declares it is
    /// in *that* package's dependency graph — not the build's. So `liba` keeps
    /// using its own `Labeled` for `Doc` even when the program linking it also
    /// pulls in `libb`'s. Empty outside a package build.
    pub package_deps: HashMap<String, HashSet<String>>,
    /// Names a `field: Type` pattern declared tentatively, by symbol.
    ///
    /// In `Cash(amount: i32)` the colon could mean a rename or a type test,
    /// and the resolver can't tell which: that depends on `amount`'s type.
    /// It declares `amount` so an arm body that uses it still resolves, and
    /// the checker, which does know, either binds `amount` itself or reports
    /// a use of a name nothing bound.
    pub field_type_pattern_names: HashMap<SymbolId, FieldTypePattern>,
    /// The program's `let`/`mut` bindings nothing reads, in source order
    /// (tool.warnings/W3). Warnings, so they ride on a successful resolve.
    pub unused_locals: Vec<UnusedLocal>,
    /// The program's imports nothing refers to (tool.warnings/W1).
    pub unused_imports: Vec<UnusedImport>,
}

/// A `let`/`mut` binding nothing reads.
#[derive(Debug, Clone)]
pub struct UnusedLocal {
    pub name: String,
    /// The name where it's bound.
    pub span: Span,
}

/// One of the program's imports, for the unused-import warning.
#[derive(Debug, Clone)]
pub(crate) struct ProgramImport {
    /// The name it binds: the alias, or the path's last segment.
    pub binding: String,
    pub path: Vec<String>,
    pub span: Span,
    /// The package the import is written in.
    pub package: Option<PackageId>,
}

/// An import whose name nothing in its package uses.
#[derive(Debug, Clone)]
pub struct UnusedImport {
    /// The name the import binds: the alias, or the path's last segment.
    pub name: String,
    /// The `import` line.
    pub span: Span,
}

/// `field: ty` in the pattern `pattern(…)`, where `ty` names a type.
#[derive(Debug, Clone)]
pub struct FieldTypePattern {
    pub pattern: String,
    pub field: String,
    pub ty: String,
}

/// What [`resolve`] works on.
#[derive(Clone, Copy)]
pub struct ResolveInput<'a> {
    /// The program: one file, or every file of a package merged.
    pub decls: &'a [Decl],
    /// The stdlib's own bodies, resolved alongside the program. They're
    /// compiled into every program, so their names have to bind for anything
    /// downstream to know what a call inside them refers to (#425). Empty
    /// when only the program's own names matter. To resolve a stdlib file by
    /// itself, put it here and leave `decls` empty.
    pub stdlib: &'a [Decl],
    /// For a package build. Other packages' exports and types come from here.
    pub package: Option<PackageBuild<'a>>,
}

/// What a package build tells resolve beyond the merged declarations.
#[derive(Clone, Copy)]
pub struct PackageBuild<'a> {
    /// Every package in the build.
    pub registry: &'a PackageRegistry,
    /// The one `decls` is.
    pub id: PackageId,
    /// Per dependency, by the name the program imports it as: each declared
    /// name against the name it was merged under (`Dog` → `Dog_libpkg`).
    /// References were rewritten to those names before resolve, so it's what
    /// a use of an import looks like here. Empty when nothing was renamed.
    pub renamed: &'a HashMap<String, HashMap<String, String>>,
}

/// Resolve every name in a program.
pub fn resolve(input: ResolveInput) -> Result<ResolvedProgram, Vec<ResolveError>> {
    Resolver::resolve(input)
}

