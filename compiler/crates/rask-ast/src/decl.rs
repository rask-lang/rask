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
    pub body: Vec<Stmt>,
    pub is_pub: bool,
    pub is_comptime: bool,
    pub is_unsafe: bool,
    /// Attributes like `@entry`, `@inline`, etc.
    pub attrs: Vec<String>,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
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
}

/// A struct field.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
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
    pub methods: Vec<FnDecl>,
    pub is_pub: bool,
}

/// An impl block.
#[derive(Debug, Clone)]
pub struct ImplDecl {
    pub trait_name: Option<String>,
    pub target_ty: String,
    pub methods: Vec<FnDecl>,
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
