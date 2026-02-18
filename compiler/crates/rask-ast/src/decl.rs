// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Declaration AST nodes.

use crate::{NodeId, Span};
use crate::stmt::Stmt;
use crate::expr::Expr;

/// A top-level declaration.
#[derive(Debug, Clone)]
pub struct Decl {
    pub id: NodeId,
    pub kind: DeclKind,
    pub span: Span,
}

/// The kind of declaration.
#[derive(Debug, Clone)]
pub enum DeclKind {
    /// Function declaration
    Fn(FnDecl),
    /// Struct declaration
    Struct(StructDecl),
    /// Enum declaration
    Enum(EnumDecl),
    /// Trait declaration
    Trait(TraitDecl),
    /// Impl block
    Impl(ImplDecl),
    /// Import declaration
    Import(ImportDecl),
    /// Export declaration (re-exports)
    Export(ExportDecl),
    /// Top-level constant
    Const(ConstDecl),
    /// Test block
    Test(TestDecl),
    /// Benchmark block
    Benchmark(BenchmarkDecl),
    /// External function declaration
    Extern(ExternDecl),
    /// Package block declaration (build.rk only)
    Package(PackageDecl),
}

/// A top-level constant declaration.
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Option<String>,
    pub init: crate::expr::Expr,
    pub is_pub: bool,
}

/// A test block declaration.
#[derive(Debug, Clone)]
pub struct TestDecl {
    pub name: String,
    pub body: Vec<Stmt>,
    pub is_comptime: bool,
}

/// A benchmark block declaration.
#[derive(Debug, Clone)]
pub struct BenchmarkDecl {
    pub name: String,
    pub body: Vec<Stmt>,
}

/// An external function declaration.
#[derive(Debug, Clone)]
pub struct ExternDecl {
    /// ABI string (e.g., "C", "system")
    pub abi: String,
    /// Function name
    pub name: String,
    /// Parameters
    pub params: Vec<Param>,
    /// Return type (None means void)
    pub ret_ty: Option<String>,
}

/// A function declaration.
#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub params: Vec<Param>,
    pub ret_ty: Option<String>,
    pub context_clauses: Vec<ContextClause>,
    pub body: Vec<Stmt>,
    pub is_pub: bool,
    pub is_comptime: bool,
    pub is_unsafe: bool,
    /// ABI for exported functions (e.g. `extern "C" func`)
    pub abi: Option<String>,
    /// Attributes like `@entry`, `@inline`, etc.
    pub attrs: Vec<String>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// A `using` context clause on a function signature.
#[derive(Debug, Clone)]
pub struct ContextClause {
    pub name: Option<String>,
    pub ty: String,
    pub is_frozen: bool,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub name_span: Span,
    pub ty: String,
    pub is_take: bool,
    pub is_mutate: bool,
    pub default: Option<Expr>,
}

/// A type parameter (for generics).
#[derive(Debug, Clone)]
pub struct TypeParam {
    pub name: String,
    /// True if this is a comptime parameter (e.g., `comptime N: usize`)
    pub is_comptime: bool,
    /// Type for comptime parameters (e.g., "usize" for `comptime N: usize`)
    pub comptime_type: Option<String>,
    /// Trait bounds (for regular type parameters)
    pub bounds: Vec<String>,
}

/// A struct declaration.
#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub fields: Vec<Field>,
    pub methods: Vec<FnDecl>,
    pub is_pub: bool,
    pub attrs: Vec<String>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// A struct field.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub name_span: Span,
    pub ty: String,
    pub is_pub: bool,
}

/// An enum declaration.
#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub variants: Vec<Variant>,
    pub methods: Vec<FnDecl>,
    pub is_pub: bool,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// An enum variant.
#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    pub fields: Vec<Field>,
}

/// A trait declaration.
#[derive(Debug, Clone)]
pub struct TraitDecl {
    pub name: String,
    /// Super-traits: `trait Display: ToString, Debug`
    pub super_traits: Vec<String>,
    pub methods: Vec<FnDecl>,
    pub is_pub: bool,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// An impl block.
#[derive(Debug, Clone)]
pub struct ImplDecl {
    pub trait_name: Option<String>,
    pub target_ty: String,
    pub methods: Vec<FnDecl>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// An import declaration.
///
/// Syntax:
/// - `import pkg` - qualified access: `pkg.Name`
/// - `import pkg as p` - aliased: `p.Name`
/// - `import pkg.Name` - unqualified: `Name` directly
/// - `import pkg.Name as N` - renamed: `N`
/// - `import lazy pkg` - lazy initialization
/// - `import pkg.*` - glob import (with warning)
#[derive(Debug, Clone)]
pub struct ImportDecl {
    /// The import path (e.g., ["http"] or ["http", "Request"])
    /// If len == 1: package import (qualified access)
    /// If len > 1: symbol import (unqualified access) unless is_glob
    pub path: Vec<String>,
    /// Optional alias: `import pkg as p` or `import pkg.Name as N`
    pub alias: Option<String>,
    /// Whether this is a glob import: `import pkg.*`
    pub is_glob: bool,
    /// Whether this is a lazy import: `import lazy pkg`
    pub is_lazy: bool,
}

/// An export declaration (re-exports for library facades).
///
/// Syntax:
/// - `export internal.Name` - re-export as `mylib.Name`
/// - `export internal.Name as Alias` - re-export with rename
/// - `export internal.Name, other.Thing` - multiple re-exports
#[derive(Debug, Clone)]
pub struct ExportDecl {
    /// Items to re-export
    pub items: Vec<ExportItem>,
}

/// An individual re-export item.
#[derive(Debug, Clone)]
pub struct ExportItem {
    /// Full path to the item: e.g., ["internal", "parser", "Parser"]
    pub path: Vec<String>,
    /// Optional rename: `export internal.Name as Alias`
    pub alias: Option<String>,
}

/// A package block declaration (struct.build/PK1-PK5).
///
/// Only valid in `build.rk`. Declares package metadata and dependencies.
///
/// ```rask
/// package "my-app" "1.0.0" {
///     dep "http" "^2.0"
///     dep "shared" { path: "../shared" }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct PackageDecl {
    pub name: String,
    pub version: String,
    pub deps: Vec<DepDecl>,
    pub features: Vec<FeatureDecl>,
    pub metadata: Vec<(String, String)>,
    pub profiles: Vec<ProfileDecl>,
}

/// A feature declaration inside a package block.
///
/// Additive: `feature "ssl" { dep "openssl" "^3.0" }`
/// Exclusive: `feature "runtime" exclusive { option "tokio" { dep "tokio" "^1.0" } }`
#[derive(Debug, Clone)]
pub struct FeatureDecl {
    pub name: String,
    pub exclusive: bool,
    /// Deps gated by this feature (additive features only).
    pub deps: Vec<DepDecl>,
    /// Options (exclusive features only).
    pub options: Vec<FeatureOption>,
    /// Default option name (exclusive features only, required).
    pub default: Option<String>,
}

/// An option inside an exclusive feature group.
#[derive(Debug, Clone)]
pub struct FeatureOption {
    pub name: String,
    pub deps: Vec<DepDecl>,
}

/// A build profile declaration inside a package block.
///
/// ```rask
/// profile "embedded" {
///     inherits: "release"
///     opt_level: "z"
///     panic: "abort"
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ProfileDecl {
    pub name: String,
    pub settings: Vec<(String, String)>,
}

/// A dependency declaration inside a package block.
#[derive(Debug, Clone)]
pub struct DepDecl {
    pub name: String,
    /// Version constraint (e.g., "^2.0"). None for path-only deps.
    pub version: Option<String>,
    /// Local path dependency.
    pub path: Option<String>,
    /// Git repository URL.
    pub git: Option<String>,
    /// Git branch.
    pub branch: Option<String>,
    /// Features to enable.
    pub with_features: Vec<String>,
    /// Target platform filter.
    pub target: Option<String>,
    /// Consented capabilities (PM3).
    pub allow: Vec<String>,
    /// Exclusive feature selections (FG5).
    pub exclusive_selections: Vec<(String, String)>,
}
