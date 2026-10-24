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

impl Decl {
    /// The name this declaration introduces at module level. `None` for one
    /// that introduces none: an `extend` block, an import, a test.
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            DeclKind::Fn(f) => Some(&f.name),
            DeclKind::Struct(s) => Some(&s.name),
            DeclKind::Enum(e) => Some(&e.name),
            DeclKind::Interface(t) => Some(&t.name),
            DeclKind::Const(c) => Some(&c.name),
            DeclKind::TypeAlias(a) => Some(&a.name),
            DeclKind::Annotation(a) => Some(&a.name),
            DeclKind::Union(u) => Some(&u.name),
            _ => None,
        }
    }

    /// `name`, to write.
    pub fn name_mut(&mut self) -> Option<&mut String> {
        match &mut self.kind {
            DeclKind::Fn(f) => Some(&mut f.name),
            DeclKind::Struct(s) => Some(&mut s.name),
            DeclKind::Enum(e) => Some(&mut e.name),
            DeclKind::Interface(t) => Some(&mut t.name),
            DeclKind::Const(c) => Some(&mut c.name),
            DeclKind::TypeAlias(a) => Some(&mut a.name),
            DeclKind::Annotation(a) => Some(&mut a.name),
            DeclKind::Union(u) => Some(&mut u.name),
            _ => None,
        }
    }
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
    /// Interface declaration
    Interface(InterfaceDecl),
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
    pub is_pub: bool,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// A type declaration: nominal by default, transparent with `type alias`.
#[derive(Debug, Clone)]
pub struct TypeAliasDecl {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub target: crate::ty::TypeExpr,
    pub is_pub: bool,
    /// True for `type alias X = Y` (transparent). False for `type X = Y` (nominal).
    pub is_transparent: bool,
    /// Interfaces inherited from underlying type: `type X = Y implements Equal, Hashable`
    pub with_interfaces: Vec<crate::ty::TypeExpr>,
}

/// A top-level constant declaration.
#[derive(Debug, Clone)]
pub struct ConstDecl {
    pub name: String,
    pub ty: Option<crate::ty::TypeExpr>,
    pub init: crate::expr::Expr,
    pub is_pub: bool,
    /// Attributes (`@comptime_quota(50000)`), stored verbatim.
    pub attrs: Vec<crate::attr::Attr>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

impl ConstDecl {
    /// The text inside `@comptime_quota(...)`, if the const carries it (CT35).
    ///
    /// Returned unparsed so the fold site can complain about `@comptime_quota(lots)`
    /// instead of ignoring it — an attribute that silently does nothing is worse
    /// than one that doesn't exist.
    pub fn comptime_quota_arg(&self) -> Option<&crate::attr::AttrValue> {
        crate::attr::find(&self.attrs, "comptime_quota")?.positional()
    }
}

/// A test block declaration.
#[derive(Debug, Clone)]
pub struct TestDecl {
    pub name: String,
    pub body: Vec<Stmt>,
    pub is_comptime: bool,
    /// `@allow(name)` and friends, same as `FnDecl::attrs`. A test body is
    /// where a warning most often fires on purpose.
    pub attrs: Vec<crate::attr::Attr>,
}

/// A benchmark block declaration.
#[derive(Debug, Clone)]
pub struct BenchmarkDecl {
    pub name: String,
    pub body: Vec<Stmt>,
    pub attrs: Vec<crate::attr::Attr>,
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
    pub ret_ty: Option<crate::ty::TypeExpr>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
    /// Attributes written before `extern`. In the block form they cover every
    /// member: `@memory_only extern "C" { … }`.
    pub attrs: Vec<crate::attr::Attr>,
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
    pub ret_ty: Option<crate::ty::TypeExpr>,
    pub body: Vec<Stmt>,
    pub is_pub: bool,
    pub is_private: bool,
    pub is_comptime: bool,
    pub is_unsafe: bool,
    /// ABI for exported functions (e.g. `extern "C" func`)
    pub abi: Option<String>,
    /// Attributes like `@entry`, `@inline`, etc.
    pub attrs: Vec<crate::attr::Attr>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
    /// Span covering `func` keyword through closing `}`
    pub span: Span,
    /// Where the declaration's text begins: its first attribute or modifier,
    /// else `span.start`. Only the formatter needs it — the blank line and the
    /// comments before a method belong before its `@attr` or `public`.
    pub decl_start: usize,
}

impl FnDecl {
    /// Declared without a Rask body: the backend implements it (`@native`,
    /// `@builtin`) or nothing does yet (`@unimplemented`). An empty `{}`
    /// without one of those is a real body that does nothing.
    pub fn body_lives_elsewhere(&self) -> bool {
        self.attrs.iter().any(|a| a.is("native") || a.is("builtin") || a.is("unimplemented"))
    }

    /// The effects the stdlib declares on a function it can't show in a body:
    /// `@grows`, `@shrinks`, `@blocks`, `@prints`, `@ambient`.
    pub fn declared_effects(&self) -> DeclaredEffects {
        let has = |name| self.attrs.iter().any(|a| a.is(name));
        DeclaredEffects {
            grows: has("grows"),
            shrinks: has("shrinks"),
            blocks: has("blocks"),
            prints: has("prints"),
            ambient: has("ambient"),
        }
    }
}

/// What a stdlib function does that its body can't show, as its declaration
/// says. Read by the checker, ownership and effects.
///
/// `grows`/`shrinks` (comp.advanced/EF1): the method changes how many elements
/// its receiver holds, or moves them to new storage. Either one invalidates a
/// `with` binding or a `for mutate` loop over the receiver (mem.borrowing/W2,
/// ctrl.loops/LP14), and neither is possible on a fixed array. It was five
/// name lists once, and the with-block rule's list didn't know `remove_where`
/// (#1560).
///
/// `blocks`/`ambient` (comp.effects/FX1): on a native, the bottom of a call
/// chain, where no body says what happens. Every other function's are
/// inferred from what it calls.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeclaredEffects {
    /// Adds elements, or may move them to a bigger buffer (`push`, `reserve`).
    pub grows: bool,
    /// Removes elements, or may move them to a smaller buffer (`pop`, `shrink`).
    pub shrinks: bool,
    /// Can park the thread until something outside it happens: a syscall,
    /// `sleep`, a channel op, a `join`.
    pub blocks: bool,
    /// Writes to stdout or stderr. Blocks only when whatever reads them stops
    /// reading, so the blocking-in-a-loop warning leaves it out by default.
    pub prints: bool,
    /// Reads or changes state nobody passed in and the program doesn't own:
    /// the clock, environment variables, the process, the random source, the
    /// task runtime. Its result isn't fixed by its arguments, and it doesn't
    /// wait.
    pub ambient: bool,
}

impl DeclaredEffects {
    /// Changes the number of elements in the receiver, or where they live.
    pub fn changes_size(self) -> bool {
        self.grows || self.shrinks
    }
}

/// What the checker resolved a method call to, for the passes after it:
/// `"Type.method"` with the receiver's declared type (`Handle.join` for
/// `t.join()` on a `Handle<i64>`), and the method's declared effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodTarget {
    pub name: String,
    pub effect: DeclaredEffects,
}

/// A function parameter.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub name_span: Span,
    /// `None` when the type is left to inference (GC1).
    pub ty: Option<crate::ty::TypeExpr>,
    pub is_take: bool,
    pub is_mutate: bool,
    /// analysis.fourth-option: this parameter's `Store` may have nodes deleted
    /// from it that the callee picked itself. Separate from `is_mutate`, which
    /// covers inserting and writing — the two answer different questions for the
    /// caller ("can the contents change?" and "can my links die?").
    pub is_deleting: bool,
    pub default: Option<Expr>,
}

/// One interface a type parameter or associated type is bound by: `Printable`
/// in `T: Printable + Debug`.
#[derive(Debug, Clone)]
pub struct Bound {
    pub ty: crate::ty::TypeExpr,
    /// Where it's written. A bound the compiler inferred points at the
    /// declaration it was added to.
    pub span: Span,
}

/// A type parameter (for generics).
#[derive(Debug, Clone)]
pub struct TypeParam {
    pub name: String,
    /// True if this is a comptime parameter (e.g., `comptime N: usize`)
    pub is_comptime: bool,
    /// Type for comptime parameters (e.g., "usize" for `comptime N: usize`)
    pub comptime_type: Option<crate::ty::TypeExpr>,
    /// Interface bounds (for regular type parameters)
    pub bounds: Vec<Bound>,
    /// GT4: `interface Mul<Rhs = Self>` — what the parameter means when a bound or
    /// conformance header writes the interface bare. Only interfaces declare these.
    pub default: Option<crate::ty::TypeExpr>,
}

impl TypeParam {
    /// The bounds' types, without where they were written.
    pub fn bound_types(&self) -> Vec<crate::ty::TypeExpr> {
        self.bounds.iter().map(|b| b.ty.clone()).collect()
    }
}

/// GT1/AT1: an interface member the conformance supplies, not a method.
///
/// `type Out`, `type Out: Comparable`, `type Out = Self`.
#[derive(Debug, Clone)]
pub struct AssocTypeDecl {
    pub name: String,
    /// AT5: every conformance's binding must satisfy these.
    pub bounds: Vec<Bound>,
    /// AT4: what a conformance that omits the binding gets.
    pub default: Option<crate::ty::TypeExpr>,
    pub span: Span,
}

/// AT2: `type Out = Meters` inside an `T implements Interface` block.
#[derive(Debug, Clone)]
pub struct AssocTypeBinding {
    pub name: String,
    pub ty: crate::ty::TypeExpr,
    pub span: Span,
}

/// A struct declaration.
#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    pub type_params: Vec<TypeParam>,
    pub fields: Vec<Field>,
    pub methods: Vec<FnDecl>,
    pub is_pub: bool,
    pub attrs: Vec<crate::attr::Attr>,
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
    pub ty: crate::ty::TypeExpr,
    /// Where the type is written. Errors about the type point here.
    pub ty_span: Span,
    pub visibility: FieldVisibility,
    /// Field annotations: `@rename("...")`, `@no_serialize`, `@default(expr)`.
    /// Stored verbatim (e.g. `rename("user_name")`), same shape as decl attrs.
    pub attrs: Vec<crate::attr::Attr>,
    /// Declared default (`port: i32 = 8080`). Compile-time constant only (FD1).
    /// Filled in at construction when the field is omitted.
    pub default: Option<Expr>,
    /// Doc comment (`/// ...`) written above the field.
    pub doc: Option<String>,
}

/// Serialization annotations on a struct field (std.encoding/E18–E20).
///
/// Every format (JSON today, TOML and MessagePack later) asks the same
/// questions of a field's attributes, so they're answered once here rather
/// than in each encoder.
pub mod field_attrs {
    use crate::attr::{self, Attr};

    /// The key this field serializes under: `@rename("…")` when present,
    /// otherwise the field's own name (E18).
    pub fn serial_name(attrs: &[Attr], field_name: &str) -> String {
        attr::find(attrs, "rename")
            .and_then(|a| a.positional())
            .and_then(|v| v.string())
            .unwrap_or_else(|| field_name.to_string())
    }

    /// `@no_serialize` — left out of the serialized form entirely, in both
    /// directions (E19).
    pub fn is_skipped(attrs: &[Attr]) -> bool {
        attr::has(attrs, "no_serialize")
    }

    /// The old spelling. `@skip` failed the guess test — skip from what? — and
    /// became `@no_serialize` (E19). Still recognized so the error can say so
    /// rather than silently serializing the field.
    pub fn uses_old_skip(attrs: &[Attr]) -> bool {
        attr::has(attrs, "skip")
    }

    /// The `@default(…)` value, when the field has one (E20).
    pub fn default_value(attrs: &[Attr]) -> Option<&crate::attr::AttrValue> {
        attr::find(attrs, "default")?.positional()
    }

    /// What a field the input doesn't supply is built from: the `@default(…)`
    /// override first, then the declared default (type.structs/FD6).
    pub fn decode_default(field: &super::Field) -> Option<&crate::expr::Expr> {
        match default_value(&field.attrs) {
            Some(value) => value.expr(),
            None => field.default.as_ref(),
        }
    }

    /// The static method desugar writes for a field's decode default. Both
    /// decoders call it for a field the input leaves out. Its body is the
    /// default as a `comptime` block, so what it hands back was computed
    /// while compiling (type.structs/FD1, std.encoding/E21).
    pub fn decode_default_method(field_name: &str) -> String {
        format!("__decode_default_{field_name}")
    }

    /// `@tag("field")` on an enum: internal tagging, the variant name goes in
    /// this field instead of being the object's own key (std.encoding/E24).
    pub fn tag_field(attrs: &[Attr]) -> Option<String> {
        attr::find(attrs, "tag")?.positional()?.string()
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
    pub attrs: Vec<crate::attr::Attr>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
    /// E14: Optional backing integer type (e.g., "u8", "i32")
    pub backing_type: Option<crate::ty::TypeExpr>,
}

/// The name a positional variant field goes by: `_0`, `_1`, … No written
/// field can be called that, since a field name starts with a letter.
pub fn positional_field_name(index: usize) -> String {
    format!("_{index}")
}

/// Whether `name` is one `positional_field_name` made.
pub fn is_positional_field_name(name: &str) -> bool {
    name.strip_prefix('_').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
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
    pub attrs: Vec<crate::attr::Attr>,
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

/// An interface declaration.
#[derive(Debug, Clone)]
pub struct InterfaceDecl {
    pub name: String,
    /// GT1: `interface Scale<Rhs>` — bound by the conformance header, substituted
    /// through every required signature before it's checked.
    pub type_params: Vec<TypeParam>,
    /// Super-interfaces: `interface Display: ToString, Debug`
    pub super_interfaces: Vec<crate::ty::TypeExpr>,
    pub methods: Vec<FnDecl>,
    /// AT1: associated types the conformance supplies.
    pub assoc_types: Vec<AssocTypeDecl>,
    pub is_pub: bool,
    /// Whether this is an `unsafe interface`.
    pub is_unsafe: bool,
    /// `duck interface` — shape-matched (structural) instead of nominal (G1).
    pub is_duck: bool,
    /// Attributes (`@allow(...)`, …)
    pub attrs: Vec<crate::attr::Attr>,
    /// Doc comment (`/// ...`)
    pub doc: Option<String>,
}

/// An extend block (`extend T`, `T implements I`).
#[derive(Debug, Clone)]
pub struct ImplDecl {
    /// The one interface this block conforms to (CD1). None for a plain
    /// `extend T` block.
    pub interface: Option<crate::ty::TypeExpr>,
    pub target_ty: crate::ty::TypeExpr,
    pub methods: Vec<FnDecl>,
    /// Whether this is an `unsafe extend`.
    pub is_unsafe: bool,
    /// `public` in front of the block. Parsed and printed back; the checker
    /// treats every conformance as declared, public or not.
    pub is_pub: bool,
    /// CC1/CC2: `where` condition for conditional conformance on a generic
    /// target (`Ring<T> implements Displayable where T: Displayable`). Each
    /// entry is a type param and its required interface bounds.
    pub where_bounds: Vec<TypeParam>,
    /// AT2: `type Out = Meters` lines in the block.
    pub assoc_bindings: Vec<AssocTypeBinding>,
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

/// A package's manifest, as `rask.toml` declares it (struct.manifest,
/// struct.build/PK1-PK4). Filled in by the manifest loader (rask-manifest),
/// not the parser.
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

/// A feature from the manifest's `[features]` or `[exclusive-features]`.
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
    /// F3: `default: true` — an additive feature that is on unless the build
    /// says `--no-default-features`.
    pub on_by_default: bool,
    /// Other features this one turns on (F6).
    pub enables: Vec<String>,
}

/// An option inside an exclusive feature group.
#[derive(Debug, Clone)]
pub struct FeatureOption {
    pub name: String,
    pub deps: Vec<DepDecl>,
}

/// A build profile from the manifest's `[profiles]`.
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

/// A dependency from one of the manifest's dependency tables.
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
    /// Git revision.
    pub rev: Option<String>,
    /// Features to enable.
    pub with_features: Vec<String>,
    /// Target platform filter.
    pub target: Option<String>,
    /// Consented capabilities (PM3).
    pub allow: Vec<String>,
    /// Exclusive feature selections (FG5).
    pub exclusive_selections: Vec<(String, String)>,
    /// Dependency scope (struct.build/D4): `"dev"`, `"build"`, or `None` for
    /// an ordinary dependency that every build links.
    pub scope: Option<String>,
    /// A further feature of ours this dep is gated on (F5).
    pub feature: Option<String>,
}
