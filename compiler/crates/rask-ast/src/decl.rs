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
    /// Top-level constant
    Const(ConstDecl),
}

/// A top-level constant declaration.
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Option<String>,
    pub init: crate::expr::Expr,
    pub is_pub: bool,
}

/// A function declaration.
#[derive(Debug, Clone)]
pub struct FnDecl {
    pub name: String,
    pub params: Vec<Param>,
    pub ret_ty: Option<String>,
    pub body: Vec<Stmt>,
    pub is_pub: bool,
    pub is_comptime: bool,
    pub is_unsafe: bool,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: String,
    pub is_take: bool,
    pub default: Option<Expr>,
}

/// A struct declaration.
#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
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
#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub path: Vec<String>,
    pub alias: Option<String>,
    pub is_glob: bool,
}
