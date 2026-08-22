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
    /// Union declaration
    Union(UnionDecl),
    /// External function declaration
    Extern(ExternDecl),
    /// Package block declaration (build.rk only)
    Package(PackageDecl),
    /// Type alias declaration
    TypeAlias(TypeAliasDecl),
    /// C header import: `import c "header.h"`
    CImport(CImportDecl),
    /// User annotation declaration (type.annotations/AN1)
    Annotation(AnnotationDecl),
}

/// A user annotation declaration (type.annotations/AN1).
///
/// A restricted struct: const-representable fields with optional defaults, no
/// methods. Attachments (`@name(args)`) type-check like construction (AN3).
#[derive(Debug, Clone)]
pub struct AnnotationDecl {
    pub name: String,
    pub name_span: Span,
    pub fields: Vec<Field>,
    /// `on field, param` — empty means attachable anywhere (AN2).
    pub targets: Vec<AnnotationTarget>,
    pub is_pub: bool,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// What an annotation may attach to (type.annotations/AN2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnotationTarget {
    Struct,
    Enum,
    Variant,
    Field,
    Func,
    Param,
}

impl AnnotationTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Struct => "struct",
            Self::Enum => "enum",
            Self::Variant => "variant",
            Self::Field => "field",
            Self::Func => "func",
            Self::Param => "param",
        }
    }
}

/// A type declaration: nominal by default, transparent with `type alias`.
#[derive(Debug, Clone)]
pub struct TypeAliasDecl {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub target: String,
    pub is_pub: bool,
    /// True for `type alias X = Y` (transparent). False for `type X = Y` (nominal).
    pub is_transparent: bool,
    /// Traits inherited from underlying type: `type X = Y with (Equal, Hashable)`
    pub with_traits: Vec<String>,
}

/// A top-level constant declaration.
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Option<String>,
    pub init: crate::expr::Expr,
    pub is_pub: bool,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
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
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
    /// Byte offset of the `extern` keyword when this came from the block form,
    /// `extern "C" { func …; func … }`.
    ///
    /// The block flattens into one declaration per function, which is what every
    /// later pass wants, but it left the formatter with no way to print the
    /// braced form back — it reprinted each member as its own `extern "C" func`,
    /// and a comment written inside the braces had nowhere to go (#805). Two
    /// blocks in a row have different offsets, so this groups members without
    /// merging blocks that were written apart.
    pub block_start: Option<usize>,
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
    pub is_private: bool,
    pub is_comptime: bool,
    pub is_unsafe: bool,
    /// ABI for exported functions (e.g. `extern "C" func`)
    pub abi: Option<String>,
    /// Attributes like `@entry`, `@inline`, etc.
    pub attrs: Vec<String>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
    /// Span covering `func` keyword through closing `}`
    pub span: Span,
}

/// A `using` context clause on a function signature.
#[derive(Debug, Clone)]
pub struct ContextClause {
    pub name: Option<String>,
    pub ty: String,
    pub is_frozen: bool,
    /// The clause itself — `players: Pool<Player>` — so a diagnostic about one
    /// clause underlines it instead of the whole signature.
    pub span: Span,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub name_span: Span,
    pub ty: String,
    pub is_take: bool,
    pub is_mutate: bool,
    /// analysis.fourth-option: this parameter's `Store` may have nodes deleted
    /// from it that the callee picked itself. Separate from `is_mutate`, which
    /// covers inserting and writing — the two answer different questions for the
    /// caller ("can the contents change?" and "can my links die?").
    pub is_deleting: bool,
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

/// Field-level visibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldVisibility {
    /// `private field: T` — only extend blocks
    Private,
    /// (no keyword) — same package
    Package,
    /// `public field: T` — external
    Public,
}

impl FieldVisibility {
    pub fn is_pub(&self) -> bool {
        matches!(self, Self::Public)
    }
}

/// A struct field.
#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub name_span: Span,
    pub ty: String,
    pub visibility: FieldVisibility,
    /// Field annotations: `@rename("...")`, `@no_serialize`, `@default(expr)`.
    /// Stored verbatim (e.g. `rename("user_name")`), same shape as decl attrs.
    pub attrs: Vec<String>,
    /// Declared default (`port: i32 = 8080`). Compile-time constant only (FD1).
    /// Filled in at construction when the field is omitted.
    pub default: Option<Expr>,
}

/// Serialization annotations on a struct field (std.encoding/E18–E20).
///
/// Attributes arrive as raw text — `rename("user_name")`, `skip`,
/// `default("user")` — so reading them means a little parsing. Every format
/// (JSON today, TOML and MessagePack later) asks the same three questions, so
/// they're answered once here rather than in each encoder.
pub mod field_attrs {
    /// The key this field serializes under: `@rename("…")` when present,
    /// otherwise the field's own name (E18).
    pub fn serial_name(attrs: &[String], field_name: &str) -> String {
        for attr in attrs {
            if let Some(arg) = call_arg(attr, "rename") {
                if let Some(text) = string_literal(arg) {
                    return text;
                }
            }
        }
        field_name.to_string()
    }

    /// `@no_serialize` — left out of the serialized form entirely, in both
    /// directions (E19).
    pub fn is_skipped(attrs: &[String]) -> bool {
        attrs.iter().any(|a| a.trim() == "no_serialize")
    }

    /// The old spelling. `@skip` failed the guess test — skip from what? — and
    /// became `@no_serialize` (E19). Still recognized so the error can say so
    /// rather than silently serializing the field.
    pub fn uses_old_skip(attrs: &[String]) -> bool {
        attrs.iter().any(|a| a.trim() == "skip")
    }

    /// The `@default(…)` literal, verbatim, when the field has one (E20).
    pub fn default_literal(attrs: &[String]) -> Option<&str> {
        attrs.iter().find_map(|a| call_arg(a, "default"))
    }

    /// `@tag("field")` on an enum: internal tagging, the variant name goes in
    /// this field instead of being the object's own key (std.encoding/E24).
    pub fn tag_field(attrs: &[String]) -> Option<String> {
        attrs.iter().find_map(|a| call_arg(a, "tag").and_then(string_literal))
    }

    /// The argument text of `name(...)`, if this attribute is that call.
    fn call_arg<'a>(attr: &'a str, name: &str) -> Option<&'a str> {
        let rest = attr.trim().strip_prefix(name)?.trim_start();
        rest.strip_prefix('(')?.strip_suffix(')').map(str::trim)
    }

    /// The attachment's name — `validate(max:100)` → `validate`. The one
    /// definition all three consumers share (checker validation, native
    /// lowering of `has<A>()`, interp's FieldInfo.has), so what counts as
    /// "the annotation's name" can't drift between them (type.annotations).
    pub fn attachment_name(attr: &str) -> &str {
        let attr = attr.trim();
        attr.find('(').map(|i| &attr[..i]).unwrap_or(attr)
    }

    /// The contents of a `"…"` literal, with the usual escapes expanded.
    pub fn string_literal(text: &str) -> Option<String> {
        let inner = text.trim().strip_prefix('"')?.strip_suffix('"')?;
        let mut out = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('r') => out.push('\r'),
                Some(other) => out.push(other),
                None => break,
            }
        }
        Some(out)
    }
}

/// An enum declaration.
#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub variants: Vec<Variant>,
    pub methods: Vec<FnDecl>,
    pub is_pub: bool,
    /// Attributes like `@message`, `@derive(...)`, etc.
    pub attrs: Vec<String>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
    /// E14: Optional backing integer type (e.g., "u8", "i32")
    pub backing_type: Option<String>,
}

/// An enum variant.
#[derive(Debug, Clone)]
pub struct Variant {
    pub name: String,
    /// Where the variant's name sits in the source. The formatter needs it to
    /// keep a comment written beside a variant beside that variant.
    pub name_span: Span,
    pub fields: Vec<Field>,
    /// Attributes like `@message("template string")`.
    pub attrs: Vec<String>,
    /// E15: Optional explicit discriminant value
    pub discriminant: Option<i128>,
}

/// A union declaration.
#[derive(Debug, Clone)]
pub struct UnionDecl {
    pub name: String,
    pub fields: Vec<Field>,
    pub is_pub: bool,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// A trait declaration.
#[derive(Debug, Clone)]
pub struct TraitDecl {
    pub name: String,
    /// Super-traits: `trait Display: ToString, Debug`
    pub super_traits: Vec<String>,
    pub methods: Vec<FnDecl>,
    pub is_pub: bool,
    /// Whether this is an `unsafe trait`.
    pub is_unsafe: bool,
    /// `duck trait` — shape-matched (structural) instead of nominal (G1).
    pub is_duck: bool,
    /// Attributes (`@allow(...)`, …)
    pub attrs: Vec<String>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// An impl block (`extend T`, `extend T with Trait`, `extend T with A, B, C`).
#[derive(Debug, Clone)]
pub struct ImplDecl {
    /// Declared trait conformances (CD1). Empty for a plain `extend T` block.
    pub trait_names: Vec<String>,
    pub target_ty: String,
    pub methods: Vec<FnDecl>,
    /// Whether this is an `unsafe extend`.
    pub is_unsafe: bool,
    /// `scoped extend` — methods stay out of the type's inherent namespace (MN4).
    pub is_scoped: bool,
    /// CC1/CC2: `where` condition for conditional conformance on a generic
    /// target (`extend Ring<T> with Displayable where T: Displayable`). Each
    /// entry is a type param and its required trait bounds.
    pub where_bounds: Vec<TypeParam>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

impl ImplDecl {
    /// The first declared trait, if any (for single-trait consumers/formatting).
    pub fn trait_name(&self) -> Option<&String> {
        self.trait_names.first()
    }
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

/// A C header import declaration.
///
/// Syntax:
/// - `import c "header.h"` - auto-parse, access as `c.symbol`
/// - `import c "header.h" as name` - auto-parse, access as `name.symbol`
/// - `import c { "a.h", "b.h" }` - multiple headers, unified namespace
/// - `import c "header.h" hiding { symbol }` - suppress specific symbols
#[derive(Debug, Clone)]
pub struct CImportDecl {
    /// Header file paths (one or more).
    pub headers: Vec<String>,
    /// Namespace alias (default: "c").
    pub alias: String,
    /// Symbols to hide from auto-parsing.
    pub hiding: Vec<String>,
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
    /// List-valued metadata (e.g., `members: ["app", "lib"]`).
    pub list_metadata: Vec<(String, Vec<String>)>,
    pub profiles: Vec<ProfileDecl>,
}

impl PackageDecl {
    /// Workspace member directories, if this is a workspace root (WS1).
    pub fn members(&self) -> Option<&Vec<String>> {
        self.list_metadata.iter()
            .find(|(k, _)| k == "members")
            .map(|(_, v)| v)
    }

    /// Get a string metadata value by key.
    pub fn meta(&self, key: &str) -> Option<&str> {
        self.metadata.iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
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
