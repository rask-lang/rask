// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The name resolver implementation.

use std::collections::{HashMap, HashSet};
use rask_ast::decl::{Decl, DeclKind, FnDecl, StructDecl, EnumDecl, TraitDecl, ImplDecl, ImportDecl, ExportDecl, TypeParam, UnionDecl};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
use rask_ast::expr::{BinOp, Expr, ExprKind, Pattern, UnaryOp};
use rask_ast::{NodeId, Span};

use crate::error::ResolveError;
use crate::scope::{ScopeTree, ScopeKind};
use crate::symbol::{BuiltinModuleKind, SymbolTable, SymbolId, SymbolKind};
use crate::package::PackageId;
use crate::ResolvedProgram;

pub struct Resolver {
    symbols: SymbolTable,
    scopes: ScopeTree,
    resolutions: HashMap<NodeId, SymbolId>,
    errors: Vec<ResolveError>,
    current_function: Option<SymbolId>,

    current_package: Option<PackageId>,
    package_bindings: HashMap<String, PackageId>,
    imported_symbols: HashSet<String>,
    lazy_imports: HashMap<String, Vec<String>>,
    /// Maps struct/enum base names to their type params (for extend blocks)
    type_param_map: HashMap<String, Vec<TypeParam>>,
    /// Public symbols exported by each external package.
    package_exports: HashMap<PackageId, HashMap<String, SymbolId>>,
    /// When true, declarations can shadow builtin names without E0209.
    stdlib_mode: bool,
    /// Symbols defined during stdlib_mode — imports may override these.
    stdlib_symbols: HashSet<SymbolId>,
    /// Compile-time cfg values for dead branch elimination in `comptime if`.
    /// Maps field names (os, arch, env, profile) to their values.
    cfg_values: HashMap<String, String>,
}

impl Resolver {
    pub fn new() -> Self {
        let mut resolver = Self {
            symbols: SymbolTable::new(),
            scopes: ScopeTree::new(),
            resolutions: HashMap::new(),
            errors: Vec::new(),
            current_function: None,
            current_package: None,
            package_bindings: HashMap::new(),
            imported_symbols: HashSet::new(),
            lazy_imports: HashMap::new(),
            type_param_map: HashMap::new(),
            package_exports: HashMap::new(),
            stdlib_mode: false,
            stdlib_symbols: HashSet::new(),
            cfg_values: HashMap::new(),
        };

        resolver.register_builtins();
        resolver
    }

    fn register_builtins(&mut self) {
        use crate::symbol::{BuiltinFunctionKind, BuiltinTypeKind};

        let builtin_fns = [
            ("println", BuiltinFunctionKind::Println, None::<&str>),
            ("print", BuiltinFunctionKind::Print, None),
            ("panic", BuiltinFunctionKind::Panic, Some("!")),
            ("format", BuiltinFunctionKind::Format, None),
            ("todo", BuiltinFunctionKind::Todo, Some("!")),
            ("unreachable", BuiltinFunctionKind::Unreachable, Some("!")),
            ("min", BuiltinFunctionKind::Min, None),
            ("max", BuiltinFunctionKind::Max, None),
            ("clamp", BuiltinFunctionKind::Clamp, None),
        ];

        for (name, builtin, ret_ty) in builtin_fns {
            let sym_id = self.symbols.insert(
                name.to_string(),
                SymbolKind::BuiltinFunction { builtin },
                ret_ty.map(String::from),
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(name.to_string(), sym_id, Span::new(0, 0));
        }

        let builtin_types = [
            ("Vec", BuiltinTypeKind::Vec),
            ("Map", BuiltinTypeKind::Map),
            ("Set", BuiltinTypeKind::Set),
            ("string", BuiltinTypeKind::String),
            ("Error", BuiltinTypeKind::Error),
            ("Channel", BuiltinTypeKind::Channel),
            ("Pool", BuiltinTypeKind::Pool),
            ("Handle", BuiltinTypeKind::Handle),
            ("Atomic", BuiltinTypeKind::Atomic),
            ("AtomicBool", BuiltinTypeKind::Atomic),
            ("AtomicI8", BuiltinTypeKind::Atomic),
            ("AtomicU8", BuiltinTypeKind::Atomic),
            ("AtomicI16", BuiltinTypeKind::Atomic),
            ("AtomicU16", BuiltinTypeKind::Atomic),
            ("AtomicI32", BuiltinTypeKind::Atomic),
            ("AtomicU32", BuiltinTypeKind::Atomic),
            ("AtomicI64", BuiltinTypeKind::Atomic),
            ("AtomicU64", BuiltinTypeKind::Atomic),
            ("AtomicUsize", BuiltinTypeKind::Atomic),
            ("AtomicIsize", BuiltinTypeKind::Atomic),
            ("Shared", BuiltinTypeKind::Shared),
            ("Mutex", BuiltinTypeKind::Mutex),
            ("Owned", BuiltinTypeKind::Owned),
        ];

        for (name, builtin) in builtin_types {
            let sym_id = self.symbols.insert(
                name.to_string(),
                SymbolKind::BuiltinType { builtin },
                None,
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(name.to_string(), sym_id, Span::new(0, 0));
        }

        self.register_builtin_enum("Option", &["Some", "None"]);
        self.register_builtin_enum("Result", &["Ok", "Err"]);
        self.register_builtin_enum("Ordering", &[
            "Less", "Equal", "Greater",                          // comparison
            "Relaxed", "Acquire", "Release", "AcqRel", "SeqCst", // memory
        ]);
        // Domain-specific enums (Method, JsonValue, JsonError, HttpError)
        // are registered when their module is imported — see resolve_import().

        // Stdlib modules, domain types, and domain enums are NOT registered
        // in the global scope — they require explicit `import` statements.
        // See resolve_import() for how they enter scope.

        // Register null constant for unsafe pointer comparisons
        let null_sym = self.symbols.insert(
            "null".to_string(),
            SymbolKind::Variable { mutable: false },
            Some("*()".to_string()),
            Span::new(0, 0),
            true,
        );
        let _ = self.scopes.define("null".to_string(), null_sym, Span::new(0, 0));
    }

    fn register_builtin_enum(&mut self, name: &str, variants: &[&str]) {
        let enum_sym_id = self.symbols.insert(
            name.to_string(),
            SymbolKind::Enum { variants: vec![] },
            None,
            Span::new(0, 0),
            true,
        );
        let _ = self.scopes.define(name.to_string(), enum_sym_id, Span::new(0, 0));

        let mut variant_syms = Vec::new();
        for variant_name in variants {
            let variant_sym_id = self.symbols.insert(
                variant_name.to_string(),
                SymbolKind::EnumVariant { enum_id: enum_sym_id },
                None,
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(variant_name.to_string(), variant_sym_id, Span::new(0, 0));
            variant_syms.push((variant_name.to_string(), variant_sym_id));
        }

        if let Some(sym) = self.symbols.get_mut(enum_sym_id) {
            sym.kind = SymbolKind::Enum { variants: variant_syms };
        }
    }

    /// When a stdlib module is imported, register its companion types and enums
    /// into scope so users don't need separate imports for each type.
    fn register_module_companions(&mut self, module: BuiltinModuleKind, span: Span) {
        use crate::symbol::BuiltinTypeKind;

        use crate::symbol::BuiltinFunctionKind;

        // Companion functions that come into scope with the module
        let functions: &[(&str, BuiltinFunctionKind, Option<&str>)] = match module {
            BuiltinModuleKind::Async => &[("spawn", BuiltinFunctionKind::Spawn, None)],
            BuiltinModuleKind::Core => &[("transmute", BuiltinFunctionKind::Transmute, None)],
            _ => &[],
        };

        for (name, builtin, ret_ty) in functions {
            if self.scopes.lookup(name).is_some() {
                continue;
            }
            let sym_id = self.symbols.insert(
                name.to_string(),
                SymbolKind::BuiltinFunction { builtin: *builtin },
                ret_ty.map(|s| s.to_string()),
                span,
                false,
            );
            let _ = self.scopes.define(name.to_string(), sym_id, span);
        }

        let types: &[&str] = match module {
            BuiltinModuleKind::Net => &["TcpListener", "TcpConnection"],
            BuiltinModuleKind::Http => &[
                "Request", "Response", "Headers",
                "HttpServer", "Responder", "HttpClient",
            ],
            BuiltinModuleKind::Fs => &["File", "Metadata"],
            BuiltinModuleKind::Random => &["Rng"],
            BuiltinModuleKind::Path => &["Path"],
            BuiltinModuleKind::Cli => &["Args"],
            _ => &[],
        };

        let enums: &[(&str, &[&str])] = match module {
            BuiltinModuleKind::Http => &[
                ("Method", &["Get", "Head", "Post", "Put", "Delete", "Patch", "Options"]),
                ("HttpError", &[
                    "ConnectionFailed", "Timeout", "InvalidUrl", "InvalidResponse",
                    "TooManyRedirects", "Io",
                ]),
            ],
            BuiltinModuleKind::Json => &[
                ("JsonValue", &["Null", "Bool", "Number", "String", "Array", "Object"]),
                ("JsonError", &["ParseError", "TypeError", "MissingField"]),
            ],
            _ => &[],
        };

        let builtin_types: &[(&str, BuiltinTypeKind)] = match module {
            BuiltinModuleKind::Fs => &[("File", BuiltinTypeKind::File)],
            BuiltinModuleKind::Random => &[("Rng", BuiltinTypeKind::Rng)],
            BuiltinModuleKind::Math => &[
                ("f32x4", BuiltinTypeKind::Simd),
                ("f32x8", BuiltinTypeKind::Simd),
                ("f64x2", BuiltinTypeKind::Simd),
                ("f64x4", BuiltinTypeKind::Simd),
                ("i32x4", BuiltinTypeKind::Simd),
                ("i32x8", BuiltinTypeKind::Simd),
            ],
            _ => &[],
        };

        // Register plain struct-like types
        for type_name in types {
            if builtin_types.iter().any(|(n, _)| n == type_name) {
                continue; // handled below as BuiltinType
            }
            if self.scopes.lookup(type_name).is_some() {
                continue;
            }
            let sym_id = self.symbols.insert(
                type_name.to_string(),
                SymbolKind::Struct { fields: vec![] },
                None,
                span,
                false,
            );
            let _ = self.scopes.define(type_name.to_string(), sym_id, span);
        }

        // Register builtin types (File, Rng, SIMD)
        for (name, kind) in builtin_types {
            if self.scopes.lookup(name).is_some() {
                continue;
            }
            let sym_id = self.symbols.insert(
                name.to_string(),
                SymbolKind::BuiltinType { builtin: *kind },
                None,
                span,
                false,
            );
            let _ = self.scopes.define(name.to_string(), sym_id, span);
        }

        // Register enums
        for (enum_name, variants) in enums {
            if self.scopes.lookup(enum_name).is_some() {
                continue;
            }
            self.register_builtin_enum(enum_name, variants);
        }
    }

    /// Returns enum variants if the symbol is a known stdlib enum.
    fn stdlib_enum_variants(module: &str, symbol: &str) -> Option<&'static [&'static str]> {
        match (module, symbol) {
            ("http", "Method") => Some(&["Get", "Head", "Post", "Put", "Delete", "Patch", "Options"]),
            ("http", "HttpError") => Some(&[
                "ConnectionFailed", "Timeout", "InvalidUrl", "InvalidResponse",
                "TooManyRedirects", "Io",
            ]),
            ("json", "JsonValue") => Some(&["Null", "Bool", "Number", "String", "Array", "Object"]),
            ("json", "JsonError") => Some(&["ParseError", "TypeError", "MissingField"]),
            _ => None,
        }
    }

    /// Look up the correct SymbolKind for a selective stdlib import
    /// like `import http.HttpServer` or `import async.spawn`.
    fn resolve_stdlib_symbol(&self, module: &str, symbol: &str) -> SymbolKind {
        use crate::symbol::{BuiltinFunctionKind, BuiltinTypeKind};

        // Builtin functions
        match (module, symbol) {
            ("async", "spawn") => return SymbolKind::BuiltinFunction { builtin: BuiltinFunctionKind::Spawn },
            ("core", "transmute") => return SymbolKind::BuiltinFunction { builtin: BuiltinFunctionKind::Transmute },
            _ => {}
        }

        // Builtin types
        let builtin_type = match (module, symbol) {
            ("fs", "File") => Some(BuiltinTypeKind::File),
            ("random", "Rng") => Some(BuiltinTypeKind::Rng),
            ("math", "f32x4" | "f32x8" | "f64x2" | "f64x4" | "i32x4" | "i32x8") => Some(BuiltinTypeKind::Simd),
            _ => None,
        };
        if let Some(kind) = builtin_type {
            return SymbolKind::BuiltinType { builtin: kind };
        }

        // Struct-like types
        let is_struct = matches!((module, symbol),
            ("net", "TcpListener" | "TcpConnection")
            | ("http", "Request" | "Response" | "Headers" | "HttpServer" | "Responder" | "HttpClient")
            | ("path", "Path")
            | ("fs", "Metadata")
            | ("cli", "Args")
        );
        if is_struct {
            return SymbolKind::Struct { fields: vec![] };
        }

        // Fallback — treat as a variable binding
        SymbolKind::Variable { mutable: false }
    }

    fn is_primitive_type(name: &str) -> bool {
        matches!(name,
            "u8" | "u16" | "u32" | "u64" | "u128" | "usize" |
            "i8" | "i16" | "i32" | "i64" | "i128" | "isize" |
            "f32" | "f64" | "bool" | "char"
        )
    }

    fn is_builtin_name(&self, name: &str) -> bool {
        if let Some(sym_id) = self.scopes.lookup(name) {
            if let Some(sym) = self.symbols.get(sym_id) {
                return matches!(
                    sym.kind,
                    SymbolKind::BuiltinType { .. }
                        | SymbolKind::BuiltinFunction { .. }
                        | SymbolKind::BuiltinModule { .. }
                ) || (matches!(sym.kind, SymbolKind::Enum { .. })
                    && sym.span == Span::new(0, 0));
            }
        }
        false
    }

    /// Check if a name refers to a builtin type or enum (not a builtin function).
    /// User-defined functions can shadow builtin functions like `max`, `min`,
    /// but not builtin types like `Vec`, `Map`, `Option`.
    fn is_builtin_type_name(&self, name: &str) -> bool {
        if let Some(sym_id) = self.scopes.lookup(name) {
            if let Some(sym) = self.symbols.get(sym_id) {
                return matches!(
                    sym.kind,
                    SymbolKind::BuiltinType { .. }
                        | SymbolKind::BuiltinModule { .. }
                ) || (matches!(sym.kind, SymbolKind::Enum { .. })
                    && sym.span == Span::new(0, 0));
            }
        }
        false
    }

    fn resolve_inner(decls: &[Decl], stdlib_mode: bool) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();
        resolver.stdlib_mode = stdlib_mode;

        resolver.collect_declarations(decls);
        resolver.resolve_bodies(decls);

        if resolver.errors.is_empty() {
            Ok(ResolvedProgram {
                symbols: resolver.symbols,
                resolutions: resolver.resolutions,
                external_decls: HashMap::new(),
            })
        } else {
            Err(resolver.errors)
        }
    }

    pub fn resolve(decls: &[Decl]) -> Result<ResolvedProgram, Vec<ResolveError>> {
        Self::resolve_inner(decls, false)
    }

    /// Resolve with cfg values for dead branch elimination in `comptime if`.
    pub fn resolve_with_cfg(
        decls: &[Decl],
        cfg_values: HashMap<String, String>,
    ) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();
        resolver.cfg_values = cfg_values;
        resolver.collect_declarations(decls);
        resolver.resolve_bodies(decls);
        if resolver.errors.is_empty() {
            Ok(ResolvedProgram {
                symbols: resolver.symbols,
                resolutions: resolver.resolutions,
                external_decls: HashMap::new(),
            })
        } else {
            Err(resolver.errors)
        }
    }

    /// Resolve stdlib definition files — skips E0209 builtin shadowing checks.
    pub fn resolve_stdlib(decls: &[Decl]) -> Result<ResolvedProgram, Vec<ResolveError>> {
        Self::resolve_inner(decls, true)
    }

    pub fn resolve_package(
        decls: &[Decl],
        registry: &crate::PackageRegistry,
        current_package: crate::PackageId,
    ) -> Result<ResolvedProgram, Vec<ResolveError>> {
        Self::resolve_package_with_stdlib(decls, registry, current_package, &[])
    }

    pub fn resolve_package_with_cfg(
        decls: &[Decl],
        registry: &crate::PackageRegistry,
        current_package: crate::PackageId,
        cfg_values: HashMap<String, String>,
    ) -> Result<ResolvedProgram, Vec<ResolveError>> {
        Self::resolve_package_with_stdlib_and_cfg(decls, registry, current_package, &[], cfg_values)
    }

    /// Resolve a package with separate stdlib declarations processed in
    /// stdlib_mode (bypasses builtin-shadowing checks). Stdlib decls are
    /// collected and resolved first, then user decls on top.
    pub fn resolve_package_with_stdlib(
        decls: &[Decl],
        registry: &crate::PackageRegistry,
        current_package: crate::PackageId,
        stdlib_decls: &[Decl],
    ) -> Result<ResolvedProgram, Vec<ResolveError>> {
        Self::resolve_package_with_stdlib_and_cfg(decls, registry, current_package, stdlib_decls, HashMap::new())
    }

    /// Resolve a package with stdlib declarations and cfg values for
    /// dead branch elimination in `comptime if`.
    pub fn resolve_package_with_stdlib_and_cfg(
        decls: &[Decl],
        registry: &crate::PackageRegistry,
        current_package: crate::PackageId,
        stdlib_decls: &[Decl],
        cfg_values: HashMap<String, String>,
    ) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();
        resolver.cfg_values = cfg_values;

        resolver.current_package = Some(current_package);

        for pkg in registry.packages() {
            resolver.package_bindings.insert(pkg.name.clone(), pkg.id);
        }

        // Collect public symbols and type declarations from external packages
        let mut external_decls: HashMap<String, Vec<Decl>> = HashMap::new();
        for pkg in registry.packages() {
            if pkg.id != current_package {
                resolver.collect_package_exports(pkg);

                let public_type_decls: Vec<Decl> = pkg.all_decls()
                    .filter(|d| match &d.kind {
                        DeclKind::Struct(s) => s.is_pub,
                        DeclKind::Enum(e) => e.is_pub,
                        DeclKind::Trait(t) => t.is_pub,
                        DeclKind::TypeAlias(a) => a.is_pub,
                        _ => false,
                    })
                    .cloned()
                    .collect();
                if !public_type_decls.is_empty() {
                    external_decls.insert(pkg.name.clone(), public_type_decls);
                }
            }
        }

        // Collect stdlib declarations in stdlib_mode (skip shadow checks)
        if !stdlib_decls.is_empty() {
            resolver.stdlib_mode = true;
            resolver.collect_declarations(stdlib_decls);
            resolver.stdlib_mode = false;
        }

        // Collect and resolve user declarations
        resolver.collect_declarations(decls);

        // Resolve bodies for both stdlib and user decls
        if !stdlib_decls.is_empty() {
            resolver.stdlib_mode = true;
            resolver.resolve_bodies(stdlib_decls);
            resolver.stdlib_mode = false;
        }
        resolver.resolve_bodies(decls);

        if resolver.errors.is_empty() {
            Ok(ResolvedProgram {
                symbols: resolver.symbols,
                resolutions: resolver.resolutions,
                external_decls,
            })
        } else {
            Err(resolver.errors)
        }
    }

    // =========================================================================
    // Cross-Package Export Collection
    // =========================================================================

    /// Collect public symbols from an external package into `package_exports`.
    fn collect_package_exports(&mut self, pkg: &crate::Package) {
        let mut exports = HashMap::new();

        for decl in pkg.all_decls() {
            match &decl.kind {
                DeclKind::Fn(f) if f.is_pub => {
                    let base = Self::base_name(&f.name).to_string();
                    let sym_id = self.symbols.insert(
                        base.clone(),
                        SymbolKind::Function {
                            params: vec![],
                            ret_ty: f.ret_ty.clone(),
                            context_clauses: f.context_clauses.clone(),
                            is_unsafe: f.is_unsafe,
                        },
                        None,
                        Span::new(0, 0),
                        true,
                    );
                    exports.insert(base, sym_id);
                }
                DeclKind::Struct(s) if s.is_pub => {
                    let base = Self::base_name(&s.name).to_string();
                    let sym_id = self.symbols.insert(
                        base.clone(),
                        SymbolKind::Struct { fields: vec![] },
                        None,
                        Span::new(0, 0),
                        true,
                    );
                    let mut field_syms = Vec::new();
                    for field in &s.fields {
                        let field_sym = self.symbols.insert(
                            field.name.clone(),
                            SymbolKind::Field { parent: sym_id },
                            Some(field.ty.clone()),
                            Span::new(0, 0),
                            field.visibility.is_pub(),
                        );
                        field_syms.push((field.name.clone(), field_sym));
                    }
                    if let Some(sym) = self.symbols.get_mut(sym_id) {
                        sym.kind = SymbolKind::Struct { fields: field_syms };
                    }
                    exports.insert(base, sym_id);
                }
                DeclKind::Enum(e) if e.is_pub => {
                    let base = Self::base_name(&e.name).to_string();
                    let sym_id = self.symbols.insert(
                        base.clone(),
                        SymbolKind::Enum { variants: vec![] },
                        None,
                        Span::new(0, 0),
                        true,
                    );
                    let mut variant_syms = Vec::new();
                    for variant in &e.variants {
                        let v_sym = self.symbols.insert(
                            variant.name.clone(),
                            SymbolKind::EnumVariant { enum_id: sym_id },
                            None,
                            Span::new(0, 0),
                            true,
                        );
                        variant_syms.push((variant.name.clone(), v_sym));
                        exports.insert(variant.name.clone(), v_sym);
                    }
                    if let Some(sym) = self.symbols.get_mut(sym_id) {
                        sym.kind = SymbolKind::Enum { variants: variant_syms };
                    }
                    exports.insert(base, sym_id);
                }
                DeclKind::Trait(t) if t.is_pub => {
                    let sym_id = self.symbols.insert(
                        t.name.clone(),
                        SymbolKind::Trait {
                            methods: vec![],
                            super_traits: t.super_traits.clone(),
                        },
                        None,
                        Span::new(0, 0),
                        true,
                    );
                    exports.insert(t.name.clone(), sym_id);
                }
                DeclKind::Const(c) if c.is_pub => {
                    let sym_id = self.symbols.insert(
                        c.name.clone(),
                        SymbolKind::Variable { mutable: false },
                        c.ty.clone(),
                        Span::new(0, 0),
                        true,
                    );
                    exports.insert(c.name.clone(), sym_id);
                }
                _ => {}
            }
        }

        self.package_exports.insert(pkg.id, exports);
    }

    // =========================================================================
    // Pass 1: Declaration Collection
    // =========================================================================

    fn collect_declarations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(fn_decl) => {
                    self.declare_function(fn_decl, decl.span, fn_decl.is_pub);
                }
                DeclKind::Struct(struct_decl) => {
                    self.declare_struct(struct_decl, decl.span);
                }
                DeclKind::Enum(enum_decl) => {
                    self.declare_enum(enum_decl, decl.span);
                }
                DeclKind::Trait(trait_decl) => {
                    self.declare_trait(trait_decl, decl.span);
                }
                DeclKind::Impl(_) => {}
                DeclKind::Import(import_decl) => {
                    self.resolve_import(import_decl, decl.span);
                }
                DeclKind::Export(export_decl) => {
                    self.resolve_export(export_decl, decl.span);
                }
                DeclKind::Const(const_decl) => {
                    let sym_id = self.symbols.insert(
                        const_decl.name.clone(),
                        SymbolKind::Variable { mutable: false },
                        const_decl.ty.clone(),
                        decl.span,
                        const_decl.is_pub,
                    );
                    if let Err(e) = self.scopes.define(const_decl.name.clone(), sym_id, decl.span) {
                        self.errors.push(e);
                    }
                }
                DeclKind::TypeAlias(alias) => {
                    let sym_id = self.symbols.insert(
                        alias.name.clone(),
                        SymbolKind::TypeAlias { target: alias.target.clone() },
                        None,
                        decl.span,
                        alias.is_pub,
                    );
                    if let Err(e) = self.scopes.define(alias.name.clone(), sym_id, decl.span) {
                        self.errors.push(e);
                    }
                }
                DeclKind::Test(_) | DeclKind::Benchmark(_) => {}
                DeclKind::Package(_) => {}
                DeclKind::Union(union_decl) => {
                    self.declare_union(union_decl, decl.span);
                }
                DeclKind::Extern(extern_decl) => {
                    let param_types: Vec<String> = extern_decl.params.iter()
                        .map(|p| p.ty.clone())
                        .collect();
                    let sym_id = self.symbols.insert(
                        extern_decl.name.clone(),
                        SymbolKind::ExternFunction {
                            abi: extern_decl.abi.clone(),
                            params: param_types,
                            ret_ty: extern_decl.ret_ty.clone(),
                        },
                        None,
                        decl.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(extern_decl.name.clone(), sym_id, decl.span) {
                        self.errors.push(e);
                    }
                }
            }
        }
    }

    /// Strip generic params from function name: "foo<T: Trait>" → "foo"
    fn base_name(name: &str) -> &str {
        name.split('<').next().unwrap_or(name)
    }

    fn declare_function(&mut self, fn_decl: &FnDecl, span: Span, is_pub: bool) -> SymbolId {
        let base = Self::base_name(&fn_decl.name).to_string();
        // User functions can shadow builtin functions (max, min, etc.)
        // but not builtin types (Vec, Map, etc.)
        if !self.stdlib_mode && self.is_builtin_type_name(&base) {
            self.errors.push(ResolveError::shadows_builtin(base.clone(), span));
        }

        let sym_id = self.symbols.insert(
            base.clone(),
            SymbolKind::Function { params: vec![], ret_ty: fn_decl.ret_ty.clone(), context_clauses: fn_decl.context_clauses.clone(), is_unsafe: fn_decl.is_unsafe },
            None,
            span,
            is_pub,
        );
        if let Err(e) = self.scopes.define(base, sym_id, span) {
            self.errors.push(e);
        }
        if self.stdlib_mode {
            self.stdlib_symbols.insert(sym_id);
        }
        sym_id
    }

    fn declare_struct(&mut self, struct_decl: &StructDecl, span: Span) {
        let base = Self::base_name(&struct_decl.name).to_string();
        if !self.stdlib_mode && self.is_builtin_name(&base) {
            self.errors.push(ResolveError::shadows_builtin(base.clone(), span));
        }

        let sym_id = self.symbols.insert(
            base.clone(),
            SymbolKind::Struct { fields: vec![] },
            None,
            span,
            struct_decl.is_pub,
        );
        if let Err(e) = self.scopes.define(base.clone(), sym_id, span) {
            self.errors.push(e);
        }
        if self.stdlib_mode {
            self.stdlib_symbols.insert(sym_id);
        }

        // Store type params for extend block resolution
        if !struct_decl.type_params.is_empty() {
            self.type_param_map.insert(base, struct_decl.type_params.clone());
        }

        let mut field_syms = Vec::new();
        for field in &struct_decl.fields {
            let field_sym = self.symbols.insert(
                field.name.clone(),
                SymbolKind::Field { parent: sym_id },
                Some(field.ty.clone()),
                span,
                field.visibility.is_pub(),
            );
            field_syms.push((field.name.clone(), field_sym));
        }

        if let Some(sym) = self.symbols.get_mut(sym_id) {
            sym.kind = SymbolKind::Struct { fields: field_syms };
        }
    }

    fn declare_union(&mut self, union_decl: &UnionDecl, span: Span) {
        let sym_id = self.symbols.insert(
            union_decl.name.clone(),
            SymbolKind::Struct { fields: vec![] },
            None,
            span,
            union_decl.is_pub,
        );
        if let Err(e) = self.scopes.define(union_decl.name.clone(), sym_id, span) {
            self.errors.push(e);
        }

        let mut field_syms = Vec::new();
        for field in &union_decl.fields {
            let field_sym = self.symbols.insert(
                field.name.clone(),
                SymbolKind::Field { parent: sym_id },
                Some(field.ty.clone()),
                span,
                field.visibility.is_pub(),
            );
            field_syms.push((field.name.clone(), field_sym));
        }

        if let Some(sym) = self.symbols.get_mut(sym_id) {
            sym.kind = SymbolKind::Struct { fields: field_syms };
        }
    }

    fn declare_enum(&mut self, enum_decl: &EnumDecl, span: Span) {
        let base = Self::base_name(&enum_decl.name).to_string();
        if !self.stdlib_mode && self.is_builtin_name(&base) {
            self.errors.push(ResolveError::shadows_builtin(base.clone(), span));
        }

        let sym_id = self.symbols.insert(
            base.clone(),
            SymbolKind::Enum { variants: vec![] },
            None,
            span,
            enum_decl.is_pub,
        );
        if let Err(e) = self.scopes.define(base.clone(), sym_id, span) {
            self.errors.push(e);
        }
        if self.stdlib_mode {
            self.stdlib_symbols.insert(sym_id);
        }

        // Store type params for extend block resolution
        if !enum_decl.type_params.is_empty() {
            self.type_param_map.insert(base, enum_decl.type_params.clone());
        }

        let mut variant_syms = Vec::new();
        for variant in &enum_decl.variants {
            let variant_sym = self.symbols.insert(
                variant.name.clone(),
                SymbolKind::EnumVariant { enum_id: sym_id },
                None,
                span,
                enum_decl.is_pub,
            );
            if let Err(e) = self.scopes.define(variant.name.clone(), variant_sym, span) {
                self.errors.push(e);
            }
            variant_syms.push((variant.name.clone(), variant_sym));
        }

        if let Some(sym) = self.symbols.get_mut(sym_id) {
            sym.kind = SymbolKind::Enum { variants: variant_syms };
        }
    }

    fn declare_trait(&mut self, trait_decl: &TraitDecl, span: Span) {
        if !self.stdlib_mode && self.is_builtin_name(&trait_decl.name) {
            self.errors.push(ResolveError::shadows_builtin(trait_decl.name.clone(), span));
        }

        let sym_id = self.symbols.insert(
            trait_decl.name.clone(),
            SymbolKind::Trait {
                methods: vec![],
                super_traits: trait_decl.super_traits.clone(),
            },
            None,
            span,
            trait_decl.is_pub,
        );
        if let Err(e) = self.scopes.define(trait_decl.name.clone(), sym_id, span) {
            self.errors.push(e);
        }
    }

    // =========================================================================
    // Import Resolution
    // =========================================================================

    fn resolve_import(&mut self, import_decl: &ImportDecl, span: Span) {
        let path = &import_decl.path;

        if path.is_empty() {
            self.errors.push(ResolveError::unknown_package(vec![], span));
            return;
        }

        if import_decl.is_glob {
            eprintln!(
                "warning: glob import `import {}.*` - imports all public symbols",
                path.join(".")
            );
        }

        if path.len() == 1 {
            let pkg_name = &path[0];
            let binding_name = import_decl.alias.as_ref().unwrap_or(pkg_name).clone();

            let stdlib_module = match pkg_name.as_str() {
                "io" => Some(BuiltinModuleKind::Io),
                "fs" => Some(BuiltinModuleKind::Fs),
                "env" => Some(BuiltinModuleKind::Env),
                "cli" => Some(BuiltinModuleKind::Cli),
                "std" => Some(BuiltinModuleKind::Std),
                "json" => Some(BuiltinModuleKind::Json),
                "random" => Some(BuiltinModuleKind::Random),
                "time" => Some(BuiltinModuleKind::Time),
                "math" => Some(BuiltinModuleKind::Math),
                "path" => Some(BuiltinModuleKind::Path),
                "os" => Some(BuiltinModuleKind::Os),
                "net" => Some(BuiltinModuleKind::Net),
                "core" => Some(BuiltinModuleKind::Core),
                "async" => Some(BuiltinModuleKind::Async),
                "http" => Some(BuiltinModuleKind::Http),
                _ => None,
            };

            if let Some(module_kind) = stdlib_module {
                let sym_id = self.symbols.insert(
                    binding_name.clone(),
                    SymbolKind::BuiltinModule { module: module_kind },
                    None,
                    span,
                    false,
                );
                if let Err(e) = self.scopes.define(binding_name.clone(), sym_id, span) {
                    self.errors.push(e);
                }
                // IM1: qualified import — access as pkg.Name, no unqualified injection.
                // For glob imports (IM6), inject all companions unqualified.
                if import_decl.is_glob {
                    self.register_module_companions(module_kind, span);
                }
            } else if let Some(&pkg_id) = self.package_bindings.get(pkg_name) {
                // External package import — register as a package namespace
                if import_decl.is_glob {
                    // Glob import: bring all public symbols directly into scope
                    if let Some(exports) = self.package_exports.get(&pkg_id).cloned() {
                        for (name, sym_id) in &exports {
                            if let Err(e) = self.scopes.define(name.clone(), *sym_id, span) {
                                self.errors.push(e);
                            }
                            self.imported_symbols.insert(name.clone());
                        }
                    }
                    return;
                }
                let sym_id = self.symbols.insert(
                    binding_name.clone(),
                    SymbolKind::ExternalPackage { package_id: pkg_id },
                    None,
                    span,
                    false,
                );
                if let Err(e) = self.scopes.define(binding_name.clone(), sym_id, span) {
                    self.errors.push(e);
                }
            } else {
                self.errors.push(ResolveError::unknown_package(path.clone(), span));
                return;
            }

            self.imported_symbols.insert(binding_name.clone());

            if import_decl.is_lazy {
                self.lazy_imports.insert(binding_name, path.clone());
            }
        } else {
            // Multi-segment import: import pkg.Name or import stdlib.Name
            let pkg_name = &path[0];
            let symbol_name = path.last().unwrap();
            let binding_name = import_decl.alias.as_ref().unwrap_or(symbol_name).clone();

            if let Some(existing_id) = self.scopes.lookup(&binding_name) {
                // Allow imports to replace builtins and stdlib-defined symbols
                let is_builtin = self.symbols.get(existing_id).map_or(false, |sym| {
                    matches!(
                        sym.kind,
                        SymbolKind::BuiltinFunction { .. }
                            | SymbolKind::BuiltinType { .. }
                            | SymbolKind::BuiltinModule { .. }
                    )
                });
                let is_stdlib = self.stdlib_symbols.contains(&existing_id);
                if !is_builtin && !is_stdlib {
                    self.errors.push(ResolveError::shadows_import(binding_name.clone(), span));
                    return;
                }
            }

            // Try to resolve from external package exports
            if let Some(&pkg_id) = self.package_bindings.get(pkg_name) {
                if let Some(exports) = self.package_exports.get(&pkg_id) {
                    if let Some(&exported_sym) = exports.get(symbol_name) {
                        // Bind the actual exported symbol into scope
                        if let Err(e) = self.scopes.define(binding_name.clone(), exported_sym, span) {
                            self.errors.push(e);
                        }
                        self.imported_symbols.insert(binding_name.clone());
                        if import_decl.is_lazy {
                            self.lazy_imports.insert(binding_name, path.clone());
                        }
                        return;
                    }
                }
            }

            // Check if the package is a known stdlib module — if so, the imported
            // symbol is a stdlib function/type being selectively imported.
            let is_stdlib_module = matches!(pkg_name.as_str(),
                "io" | "fs" | "env" | "cli" | "std" | "json" | "random"
                | "time" | "math" | "path" | "os" | "net" | "core" | "async"
                | "thread" | "http"
            );

            if is_stdlib_module {
                // IM4: selective import — register with correct symbol kind.
                if self.scopes.lookup(&binding_name).is_none() {
                    // Enums need special handling (register variants too)
                    if let Some(variants) = Self::stdlib_enum_variants(pkg_name, symbol_name) {
                        self.register_builtin_enum(symbol_name, variants);
                    } else {
                        let kind = self.resolve_stdlib_symbol(pkg_name, symbol_name);
                        let sym_id = self.symbols.insert(
                            binding_name.clone(),
                            kind,
                            None,
                            span,
                            false,
                        );
                        if let Err(e) = self.scopes.define(binding_name.clone(), sym_id, span) {
                            self.errors.push(e);
                        }
                    }
                }
            } else {
                // Unknown package — create variable binding as fallback
                let sym_id = self.symbols.insert(
                    binding_name.clone(),
                    SymbolKind::Variable { mutable: false },
                    None,
                    span,
                    false,
                );
                if let Err(e) = self.scopes.define(binding_name.clone(), sym_id, span) {
                    self.errors.push(e);
                }
            }

            self.imported_symbols.insert(binding_name.clone());

            if import_decl.is_lazy {
                self.lazy_imports.insert(binding_name, path.clone());
            }
        }
    }

    fn resolve_export(&mut self, export_decl: &ExportDecl, span: Span) {
        for item in &export_decl.items {
            let path = &item.path;

            if path.is_empty() {
                self.errors.push(ResolveError::unknown_package(vec![], span));
                continue;
            }

            let export_name = item.alias.as_ref()
                .unwrap_or_else(|| path.last().unwrap());
            let _ = export_name;
        }
    }

    // =========================================================================
    // Pass 2: Body Resolution
    // =========================================================================

    fn resolve_bodies(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Fn(fn_decl) => {
                    self.resolve_function(fn_decl);
                }
                DeclKind::Struct(struct_decl) => {
                    for method in &struct_decl.methods {
                        self.resolve_function_with_type_params(method, &struct_decl.type_params);
                    }
                }
                DeclKind::Enum(enum_decl) => {
                    for method in &enum_decl.methods {
                        self.resolve_function_with_type_params(method, &enum_decl.type_params);
                    }
                }
                DeclKind::Trait(trait_decl) => {
                    for method in &trait_decl.methods {
                        if !method.body.is_empty() {
                            self.resolve_function(method);
                        }
                    }
                }
                DeclKind::Impl(impl_decl) => {
                    self.resolve_impl(impl_decl);
                }
                DeclKind::Const(const_decl) => {
                    self.resolve_expr(&const_decl.init);
                }
                DeclKind::Test(test_decl) => {
                    // Test blocks are function-like: allow return for early exit
                    self.scopes.push(ScopeKind::Function(SymbolId(u32::MAX)));
                    for stmt in &test_decl.body {
                        self.resolve_stmt(stmt);
                    }
                    self.scopes.pop();
                }
                DeclKind::Benchmark(bench_decl) => {
                    self.scopes.push(ScopeKind::Function(SymbolId(u32::MAX)));
                    for stmt in &bench_decl.body {
                        self.resolve_stmt(stmt);
                    }
                    self.scopes.pop();
                }
                DeclKind::Import(_) => {}
                DeclKind::Export(_) => {}
                DeclKind::Extern(_) => {}
                DeclKind::Package(_) => {}
                DeclKind::Union(_) => {}
                DeclKind::TypeAlias(_) => {}
            }
        }
    }

    fn resolve_function(&mut self, fn_decl: &FnDecl) {
        self.resolve_function_with_type_params(fn_decl, &[]);
    }

    fn resolve_function_with_type_params(&mut self, fn_decl: &FnDecl, outer_type_params: &[TypeParam]) {
        let base = Self::base_name(&fn_decl.name);
        let fn_sym = self.scopes.lookup(base);
        self.current_function = fn_sym;

        let scope_kind = if let Some(sym_id) = fn_sym {
            ScopeKind::Function(sym_id)
        } else {
            ScopeKind::Function(SymbolId(u32::MAX))
        };
        self.scopes.push(scope_kind);

        // Register comptime type params from outer context (struct/enum extend)
        for tp in outer_type_params {
            if tp.is_comptime {
                let sym_id = self.symbols.insert(
                    tp.name.clone(),
                    SymbolKind::Variable { mutable: false },
                    tp.comptime_type.clone(),
                    Span::new(0, 0),
                    false,
                );
                let _ = self.scopes.define(tp.name.clone(), sym_id, Span::new(0, 0));
            }
        }

        // Register comptime type params from function's own generics
        for tp in &fn_decl.type_params {
            if tp.is_comptime {
                let sym_id = self.symbols.insert(
                    tp.name.clone(),
                    SymbolKind::Variable { mutable: false },
                    tp.comptime_type.clone(),
                    Span::new(0, 0),
                    false,
                );
                let _ = self.scopes.define(tp.name.clone(), sym_id, Span::new(0, 0));
            }
        }

        let mut param_syms = Vec::new();
        for param in &fn_decl.params {
            let param_sym = self.symbols.insert(
                param.name.clone(),
                SymbolKind::Parameter {
                    is_take: param.is_take,
                    is_mutate: param.is_mutate,
                },
                Some(param.ty.clone()),
                Span::new(0, 0),
                false,
            );
            if let Err(e) = self.scopes.define(param.name.clone(), param_sym, Span::new(0, 0)) {
                self.errors.push(e);
            }
            param_syms.push(param_sym);

            if let Some(default) = &param.default {
                self.resolve_expr(default);
            }
        }

        if let Some(sym_id) = fn_sym {
            if let Some(sym) = self.symbols.get_mut(sym_id) {
                if let SymbolKind::Function { params, .. } = &mut sym.kind {
                    *params = param_syms;
                }
            }
        }

        // Register named context clauses as bindings
        for clause in &fn_decl.context_clauses {
            if let Some(name) = &clause.name {
                let ctx_sym = self.symbols.insert(
                    name.clone(),
                    SymbolKind::Variable { mutable: !clause.is_frozen },
                    Some(clause.ty.clone()),
                    Span::new(0, 0),
                    false,
                );
                if let Err(e) = self.scopes.define(name.clone(), ctx_sym, Span::new(0, 0)) {
                    self.errors.push(e);
                }
            }
        }

        for stmt in &fn_decl.body {
            self.resolve_stmt(stmt);
        }

        self.scopes.pop();
        self.current_function = None;
    }

    fn resolve_impl(&mut self, impl_decl: &ImplDecl) {
        // Look up type params from the target type's declaration
        let base = Self::base_name(&impl_decl.target_ty).to_string();
        let outer_params = self.type_param_map.get(&base).cloned().unwrap_or_default();
        for method in &impl_decl.methods {
            self.resolve_function_with_type_params(method, &outer_params);
        }
    }

    // =========================================================================
    // Statement Resolution
    // =========================================================================

    fn resolve_stmt(&mut self, stmt: &Stmt) {
        match &stmt.kind {
            StmtKind::Expr(expr) => {
                self.resolve_expr(expr);
            }
            StmtKind::Let { name, name_span, ty, init } => {
                self.resolve_expr(init);
                let sym_id = self.symbols.insert(
                    name.clone(),
                    SymbolKind::Variable { mutable: true },
                    ty.clone(),
                    *name_span,
                    false,
                );
                if let Err(e) = self.scopes.define(name.clone(), sym_id, stmt.span) {
                    self.errors.push(e);
                }
            }
            StmtKind::Const { name, name_span, ty, init } => {
                self.resolve_expr(init);
                let sym_id = self.symbols.insert(
                    name.clone(),
                    SymbolKind::Variable { mutable: false },
                    ty.clone(),
                    *name_span,
                    false,
                );
                if let Err(e) = self.scopes.define(name.clone(), sym_id, stmt.span) {
                    self.errors.push(e);
                }
            }
            StmtKind::LetTuple { patterns, init } => {
                self.resolve_expr(init);
                for name in rask_ast::stmt::tuple_pats_flat_names(patterns) {
                    let sym_id = self.symbols.insert(
                        name.to_string(),
                        SymbolKind::Variable { mutable: true },
                        None,
                        stmt.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(name.to_string(), sym_id, stmt.span) {
                        self.errors.push(e);
                    }
                }
            }
            StmtKind::ConstTuple { patterns, init } => {
                self.resolve_expr(init);
                for name in rask_ast::stmt::tuple_pats_flat_names(patterns) {
                    let sym_id = self.symbols.insert(
                        name.to_string(),
                        SymbolKind::Variable { mutable: false },
                        None,
                        stmt.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(name.to_string(), sym_id, stmt.span) {
                        self.errors.push(e);
                    }
                }
            }
            StmtKind::Assign { target, value } => {
                self.resolve_expr(target);
                self.resolve_expr(value);
            }
            StmtKind::Return(value) => {
                if !self.scopes.in_function() {
                    self.errors.push(ResolveError::invalid_return(stmt.span));
                }
                if let Some(v) = value {
                    self.resolve_expr(v);
                }
            }
            StmtKind::Break { label, value } => {
                if let Some(lbl) = label {
                    if !self.scopes.label_in_scope(lbl) {
                        // Ambiguity: `break ident` parsed with ident as label,
                        // but it's not a known label. If we're in a loop, this
                        // is likely `break value` — suppress the error and let
                        // the interpreter/type-checker treat the label as a value.
                        if !self.scopes.in_loop() {
                            self.errors.push(ResolveError::invalid_break(Some(lbl.clone()), stmt.span));
                        }
                        // else: silently allow — interpreter handles reinterpretation
                    }
                } else if !self.scopes.in_loop() {
                    self.errors.push(ResolveError::invalid_break(None, stmt.span));
                }
                if let Some(v) = value {
                    self.resolve_expr(v);
                }
            }
            StmtKind::Continue(label) => {
                if let Some(lbl) = label {
                    if !self.scopes.label_in_scope(lbl) {
                        self.errors.push(ResolveError::invalid_continue(Some(lbl.clone()), stmt.span));
                    }
                } else if !self.scopes.in_loop() {
                    self.errors.push(ResolveError::invalid_continue(None, stmt.span));
                }
            }
            StmtKind::While { cond, body } => {
                self.resolve_expr(cond);
                self.scopes.push(ScopeKind::Loop { label: None });
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
            }
            StmtKind::WhileLet { pattern, expr, body } => {
                self.resolve_expr(expr);
                self.scopes.push(ScopeKind::Loop { label: None });
                self.resolve_pattern(pattern);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
            }
            StmtKind::Loop { label, body } => {
                self.scopes.push(ScopeKind::Loop { label: label.clone() });
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
            }
            StmtKind::For { label, binding, mutate, iter, body } => {
                self.resolve_expr(iter);
                self.scopes.push(ScopeKind::Loop { label: label.clone() });
                let names = match binding {
                    ForBinding::Single(name) => vec![name.clone()],
                    ForBinding::Tuple(names) => names.clone(),
                };
                for name in &names {
                    let sym_id = self.symbols.insert(
                        name.clone(),
                        SymbolKind::Variable { mutable: *mutate },
                        None,
                        stmt.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(name.clone(), sym_id, stmt.span) {
                        self.errors.push(e);
                    }
                }
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
            }
            StmtKind::Ensure { body, else_handler } => {
                self.scopes.push(ScopeKind::Block);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
                if let Some((name, handler)) = else_handler {
                    self.scopes.push(ScopeKind::Block);
                    let sym_id = self.symbols.insert(
                        name.clone(),
                        SymbolKind::Variable { mutable: false },
                        None,
                        stmt.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(name.clone(), sym_id, stmt.span) {
                        self.errors.push(e);
                    }
                    for s in handler {
                        self.resolve_stmt(s);
                    }
                    self.scopes.pop();
                }
            }
            StmtKind::Comptime(body) => {
                self.scopes.push(ScopeKind::Block);
                if let Some(taken) = self.try_resolve_comptime_if(body) {
                    for s in taken {
                        self.resolve_stmt(s);
                    }
                } else {
                    for s in body {
                        self.resolve_stmt(s);
                    }
                }
                self.scopes.pop();
            }
            StmtKind::Discard { .. } => {
                // Name is resolved during type checking — nothing to do here
            }
        }
    }

    /// Try to evaluate a `comptime if cfg.field == "value"` condition statically.
    /// Returns the taken branch's statements if the pattern matches and
    /// the condition can be evaluated, or None to fall through to normal resolution.
    fn try_resolve_comptime_if<'b>(&self, stmts: &'b [Stmt]) -> Option<&'b [Stmt]> {
        if self.cfg_values.is_empty() || stmts.len() != 1 {
            return None;
        }
        let inner = match &stmts[0].kind {
            StmtKind::Expr(e) => e,
            _ => return None,
        };
        let (cond, then_branch, else_branch) = match &inner.kind {
            ExprKind::If { cond, then_branch, else_branch } => (cond, then_branch, else_branch),
            _ => return None,
        };

        let taken = self.eval_cfg_condition(cond)?;
        if taken {
            if let ExprKind::Block(block_stmts) = &then_branch.kind {
                Some(block_stmts)
            } else {
                None
            }
        } else if let Some(else_br) = else_branch {
            if let ExprKind::Block(block_stmts) = &else_br.kind {
                Some(block_stmts)
            } else {
                None
            }
        } else {
            Some(&[])
        }
    }

    /// Evaluate a cfg condition expression statically.
    /// Handles both pre-desugar (`Binary { Eq, .. }`) and post-desugar
    /// (`MethodCall { method: "eq", .. }`) forms, plus `!`, `&&`, `||`.
    fn eval_cfg_condition(&self, expr: &Expr) -> Option<bool> {
        match &expr.kind {
            // Pre-desugar: cfg.field == "value"
            ExprKind::Binary { op, left, right } => {
                match op {
                    BinOp::Eq | BinOp::Ne => {
                        let (field, value) = self.extract_cfg_comparison(left, right)?;
                        let cfg_val = self.cfg_values.get(field)?;
                        let result = cfg_val == value;
                        Some(if *op == BinOp::Eq { result } else { !result })
                    }
                    BinOp::And => {
                        let l = self.eval_cfg_condition(left)?;
                        let r = self.eval_cfg_condition(right)?;
                        Some(l && r)
                    }
                    BinOp::Or => {
                        let l = self.eval_cfg_condition(left)?;
                        let r = self.eval_cfg_condition(right)?;
                        Some(l || r)
                    }
                    _ => None,
                }
            }
            // Post-desugar: cfg.field.eq("value") — `==` desugars to `.eq()` method call
            ExprKind::MethodCall { object, method, args, .. } if method == "eq" => {
                let field = self.extract_cfg_field(object)?;
                let value = match args.first() {
                    Some(arg) => match &arg.expr.kind {
                        ExprKind::String(s) => s.as_str(),
                        _ => return None,
                    },
                    None => return None,
                };
                let cfg_val = self.cfg_values.get(field)?;
                Some(cfg_val == value)
            }
            // Post-desugar: !(cfg.field.eq("value")) — `!=` desugars to `!(.eq())`
            ExprKind::Unary { op: UnaryOp::Not, operand } => {
                Some(!self.eval_cfg_condition(operand)?)
            }
            _ => None,
        }
    }

    /// Extract (field_name, string_value) from `cfg.field == "value"` (pre-desugar).
    fn extract_cfg_comparison<'b>(&self, left: &'b Expr, right: &'b Expr) -> Option<(&'b str, &'b str)> {
        if let Some(field) = self.extract_cfg_field(left) {
            if let ExprKind::String(val) = &right.kind {
                return Some((field, val));
            }
        }
        if let Some(field) = self.extract_cfg_field(right) {
            if let ExprKind::String(val) = &left.kind {
                return Some((field, val));
            }
        }
        None
    }

    /// Extract the field name from a `cfg.field` expression.
    fn extract_cfg_field<'b>(&self, expr: &'b Expr) -> Option<&'b str> {
        if let ExprKind::Field { object, field } = &expr.kind {
            if let ExprKind::Ident(name) = &object.kind {
                if name == "cfg" {
                    return Some(field);
                }
            }
        }
        None
    }

    // =========================================================================
    // Expression Resolution
    // =========================================================================

    fn resolve_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_) |
            ExprKind::Char(_) | ExprKind::Bool(_) | ExprKind::Null => {}
            ExprKind::Ident(name) => {
                match self.scopes.lookup(name) {
                    Some(sym_id) => {
                        self.resolutions.insert(expr.id, sym_id);
                    }
                    None => {
                        // Try base type for generic constructors: Pool<Node> → Pool
                        let base_name = name.split('<').next().unwrap_or(name);
                        if base_name != name {
                            if let Some(sym_id) = self.scopes.lookup(base_name) {
                                self.resolutions.insert(expr.id, sym_id);
                                return;
                            }
                        }
                        self.errors.push(ResolveError::undefined(name.clone(), expr.span));
                    }
                }
            }
            ExprKind::Binary { left, right, .. } => {
                self.resolve_expr(left);
                self.resolve_expr(right);
            }
            ExprKind::Unary { operand, .. } => {
                self.resolve_expr(operand);
            }
            ExprKind::Call { func, args } => {
                self.resolve_expr(func);
                for arg in args {
                    self.resolve_expr(&arg.expr);
                }
            }
            ExprKind::MethodCall { object, method, args, .. } => {
                // Check for calls on external packages: lib.greet()
                if let ExprKind::Ident(name) = &object.kind {
                    if let Some(sym_id) = self.scopes.lookup(name) {
                        if let Some(sym) = self.symbols.get(sym_id) {
                            if let SymbolKind::ExternalPackage { package_id } = &sym.kind {
                                let pkg_id = *package_id;
                                self.resolutions.insert(object.id, sym_id);
                                if let Some(exports) = self.package_exports.get(&pkg_id) {
                                    if let Some(&method_sym) = exports.get(method) {
                                        self.resolutions.insert(expr.id, method_sym);
                                    }
                                }
                                for arg in args {
                                    self.resolve_expr(&arg.expr);
                                }
                                return;
                            }
                        }
                    }
                }
                self.resolve_expr(object);
                for arg in args {
                    self.resolve_expr(&arg.expr);
                }
            }
            ExprKind::Field { object, field } => {
                if let ExprKind::Ident(name) = &object.kind {
                    // Check for qualified access on external packages
                    if let Some(sym_id) = self.scopes.lookup(name) {
                        if let Some(sym) = self.symbols.get(sym_id) {
                            if let SymbolKind::ExternalPackage { package_id } = &sym.kind {
                                let pkg_id = *package_id;
                                self.resolutions.insert(object.id, sym_id);
                                if let Some(exports) = self.package_exports.get(&pkg_id) {
                                    if let Some(&field_sym) = exports.get(field) {
                                        self.resolutions.insert(expr.id, field_sym);
                                    }
                                    // No error for missing field — type checker handles it
                                }
                                return;
                            }
                        }
                    }

                    if self.imported_symbols.contains(name) {
                        // Resolve the imported identifier so the type checker
                        // has a proper NodeId → SymbolId mapping.
                        if let Some(sym_id) = self.scopes.lookup(name) {
                            self.resolutions.insert(object.id, sym_id);
                        }
                        return;
                    }
                    // Skip resolution for primitive type constants (u64.MAX, etc.)
                    if Self::is_primitive_type(name) {
                        return;
                    }
                }
                self.resolve_expr(object);
            }
            ExprKind::OptionalField { object, .. } => {
                self.resolve_expr(object);
            }
            ExprKind::Index { object, index } => {
                self.resolve_expr(object);
                self.resolve_expr(index);
            }
            ExprKind::Block(stmts) => {
                self.scopes.push(ScopeKind::Block);
                for stmt in stmts {
                    self.resolve_stmt(stmt);
                }
                self.scopes.pop();
            }
            ExprKind::If { cond, then_branch, else_branch } => {
                self.resolve_expr(cond);
                self.resolve_expr(then_branch);
                if let Some(else_br) = else_branch {
                    self.resolve_expr(else_br);
                }
            }
            ExprKind::IfLet { expr, pattern, then_branch, else_branch } => {
                self.resolve_expr(expr);
                self.scopes.push(ScopeKind::Block);
                self.resolve_pattern(pattern);
                self.resolve_expr(then_branch);
                self.scopes.pop();
                if let Some(else_br) = else_branch {
                    self.resolve_expr(else_br);
                }
            }
            ExprKind::Match { scrutinee, arms } => {
                self.resolve_expr(scrutinee);
                for arm in arms {
                    self.scopes.push(ScopeKind::Block);
                    self.resolve_pattern(&arm.pattern);
                    if let Some(guard) = &arm.guard {
                        self.resolve_expr(guard);
                    }
                    self.resolve_expr(&arm.body);
                    self.scopes.pop();
                }
            }
            ExprKind::Try { expr: inner, ref else_clause } => {
                self.resolve_expr(inner);
                if let Some(ec) = else_clause {
                    self.scopes.push(ScopeKind::Block);
                    let sym_id = self.symbols.insert(
                        ec.error_binding.clone(),
                        SymbolKind::Variable { mutable: false },
                        None,
                        Span::new(0, 0),
                        false,
                    );
                    if let Err(e) = self.scopes.define(ec.error_binding.clone(), sym_id, Span::new(0, 0)) {
                        self.errors.push(e);
                    }
                    self.resolve_expr(&ec.body);
                    self.scopes.pop();
                }
            }
            ExprKind::Unwrap { expr: inner, message: _ } => {
                self.resolve_expr(inner);
            }
            ExprKind::GuardPattern { expr, pattern, else_branch } => {
                self.resolve_expr(expr);
                self.resolve_pattern(pattern);
                self.resolve_expr(else_branch);
            }
            ExprKind::IsPattern { expr, pattern } => {
                self.resolve_expr(expr);
                self.resolve_pattern(pattern);
            }
            ExprKind::NullCoalesce { value, default } => {
                self.resolve_expr(value);
                self.resolve_expr(default);
            }
            ExprKind::Range { start, end, .. } => {
                if let Some(s) = start {
                    self.resolve_expr(s);
                }
                if let Some(e) = end {
                    self.resolve_expr(e);
                }
            }
            ExprKind::StructLit { name, fields, spread } => {
                if let Some(sym_id) = self.scopes.lookup(name) {
                    self.resolutions.insert(expr.id, sym_id);
                } else if name.contains('.') {
                    // Qualified name: Enum.Variant or pkg.Struct
                    let parts: Vec<&str> = name.splitn(2, '.').collect();
                    if let Some(sym_id) = self.scopes.lookup(parts[0]) {
                        // Check if this is a package-qualified struct literal
                        if let Some(sym) = self.symbols.get(sym_id) {
                            if let SymbolKind::ExternalPackage { package_id } = &sym.kind {
                                let pkg_id = *package_id;
                                if let Some(exports) = self.package_exports.get(&pkg_id) {
                                    if let Some(&struct_sym) = exports.get(parts[1]) {
                                        self.resolutions.insert(expr.id, struct_sym);
                                    }
                                }
                            } else {
                                self.resolutions.insert(expr.id, sym_id);
                            }
                        } else {
                            self.resolutions.insert(expr.id, sym_id);
                        }
                    } else {
                        self.errors.push(ResolveError::undefined(name.clone(), expr.span));
                    }
                } else {
                    // Try base type for generic: Box<T> → Box
                    let base_name = Self::base_name(name);
                    if base_name != name.as_str() {
                        if let Some(sym_id) = self.scopes.lookup(base_name) {
                            self.resolutions.insert(expr.id, sym_id);
                        } else {
                            self.errors.push(ResolveError::undefined(name.clone(), expr.span));
                        }
                    } else {
                        self.errors.push(ResolveError::undefined(name.clone(), expr.span));
                    }
                }
                for field in fields {
                    self.resolve_expr(&field.value);
                }
                if let Some(s) = spread {
                    self.resolve_expr(s);
                }
            }
            ExprKind::Array(elements) => {
                for elem in elements {
                    self.resolve_expr(elem);
                }
            }
            ExprKind::ArrayRepeat { value, count } => {
                self.resolve_expr(value);
                self.resolve_expr(count);
            }
            ExprKind::Tuple(elements) => {
                for elem in elements {
                    self.resolve_expr(elem);
                }
            }
            ExprKind::UsingBlock { args, body, .. } => {
                for arg in args {
                    self.resolve_expr(&arg.expr);
                }
                self.scopes.push(ScopeKind::Block);
                for stmt in body {
                    self.resolve_stmt(stmt);
                }
                self.scopes.pop();
            }
            ExprKind::WithAs { bindings, body } => {
                for binding in bindings.iter() {
                    self.resolve_expr(&binding.source);
                }
                self.scopes.push(ScopeKind::Block);
                for binding in bindings {
                    let sym_id = self.symbols.insert(
                        binding.name.clone(),
                        SymbolKind::Variable { mutable: binding.mutable },
                        None,
                        expr.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(binding.name.clone(), sym_id, expr.span) {
                        self.errors.push(e);
                    }
                }
                for stmt in body {
                    self.resolve_stmt(stmt);
                }
                self.scopes.pop();
            }
            ExprKind::Closure { params, body, .. } => {
                self.scopes.push(ScopeKind::Closure);
                for param in params {
                    let sym_id = self.symbols.insert(
                        param.name.clone(),
                        SymbolKind::Parameter {
                            is_take: false,
                            is_mutate: false,
                        },
                        param.ty.clone(),
                        expr.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(param.name.clone(), sym_id, expr.span) {
                        self.errors.push(e);
                    }
                }
                self.resolve_expr(body);
                self.scopes.pop();
            }
            ExprKind::Cast { expr: inner, .. } => {
                self.resolve_expr(inner);
            }
            ExprKind::Spawn { body } => {
                self.scopes.push(ScopeKind::Block);
                for stmt in body {
                    self.resolve_stmt(stmt);
                }
                self.scopes.pop();
            }
            ExprKind::Loop { label, body } => {
                self.scopes.push(ScopeKind::Loop { label: label.clone() });
                for stmt in body {
                    self.resolve_stmt(stmt);
                }
                self.scopes.pop();
            }
            ExprKind::BlockCall { body, .. } => {
                self.scopes.push(ScopeKind::Block);
                for stmt in body {
                    self.resolve_stmt(stmt);
                }
                self.scopes.pop();
            }
            ExprKind::Unsafe { body } => {
                self.scopes.push(ScopeKind::Block);
                for stmt in body {
                    self.resolve_stmt(stmt);
                }
                self.scopes.pop();
            }
            ExprKind::Comptime { body } => {
                self.scopes.push(ScopeKind::Block);
                if let Some(taken) = self.try_resolve_comptime_if(body) {
                    for s in taken {
                        self.resolve_stmt(s);
                    }
                } else {
                    for stmt in body {
                        self.resolve_stmt(stmt);
                    }
                }
                self.scopes.pop();
            }
            ExprKind::Assert { condition, message } | ExprKind::Check { condition, message } => {
                self.resolve_expr(condition);
                if let Some(msg) = message {
                    self.resolve_expr(msg);
                }
            }
            ExprKind::Select { arms, .. } => {
                for arm in arms {
                    match &arm.kind {
                        rask_ast::expr::SelectArmKind::Recv { channel, binding } => {
                            self.resolve_expr(channel);
                            // The binding is a new variable in the arm body scope
                            let sym_id = self.symbols.insert(
                                binding.clone(),
                                SymbolKind::Variable { mutable: false },
                                None,
                                arm.body.span,
                                false,
                            );
                            self.scopes.push(ScopeKind::Block);
                            if let Err(e) = self.scopes.define(binding.clone(), sym_id, arm.body.span) {
                                self.errors.push(e);
                            }
                            self.resolve_expr(&arm.body);
                            self.scopes.pop();
                        }
                        rask_ast::expr::SelectArmKind::Send { channel, value } => {
                            self.resolve_expr(channel);
                            self.resolve_expr(value);
                            self.resolve_expr(&arm.body);
                        }
                        rask_ast::expr::SelectArmKind::Default => {
                            self.resolve_expr(&arm.body);
                        }
                    }
                }
            }
        }
    }

    // =========================================================================
    // Pattern Resolution
    // =========================================================================

    fn resolve_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard => {}
            Pattern::Ident(name) => {
                if let Some(sym_id) = self.scopes.lookup(name) {
                    if let Some(sym) = self.symbols.get(sym_id) {
                        if matches!(sym.kind, SymbolKind::EnumVariant { .. }) {
                            return;
                        }
                    }
                }

                let sym_id = self.symbols.insert(
                    name.clone(),
                    SymbolKind::Variable { mutable: false },
                    None,
                    Span::new(0, 0),
                    false,
                );
                if let Err(e) = self.scopes.define(name.clone(), sym_id, Span::new(0, 0)) {
                    self.errors.push(e);
                }
            }
            Pattern::Literal(expr) => {
                self.resolve_expr(expr);
            }
            Pattern::Constructor { name, fields } => {
                if let Some(sym_id) = self.scopes.lookup(name) {
                    let _ = sym_id;
                }
                for field_pattern in fields {
                    self.resolve_pattern(field_pattern);
                }
            }
            Pattern::Struct { name, fields, .. } => {
                if let Some(_sym_id) = self.scopes.lookup(name) {}
                for (_, field_pattern) in fields {
                    self.resolve_pattern(field_pattern);
                }
            }
            Pattern::Tuple(patterns) => {
                for p in patterns {
                    self.resolve_pattern(p);
                }
            }
            Pattern::Or(patterns) => {
                for p in patterns {
                    self.resolve_pattern(p);
                }
            }
        }
    }
}

impl Default for Resolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::{Decl, DeclKind, ImportDecl};

    fn make_import_decl(path: Vec<&str>, alias: Option<&str>, is_glob: bool, is_lazy: bool) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Import(ImportDecl {
                path: path.into_iter().map(String::from).collect(),
                alias: alias.map(String::from),
                is_glob,
                is_lazy,
            }),
            span: Span::new(0, 10),
        }
    }

    #[test]
    fn test_stdlib_import() {
        let decls = vec![make_import_decl(vec!["io"], None, false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Stdlib import should succeed");
    }

    #[test]
    fn test_symbol_import() {
        let decls = vec![make_import_decl(vec!["io", "stdin"], None, false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Symbol import should succeed");
    }

    #[test]
    fn test_aliased_import() {
        let decls = vec![make_import_decl(vec!["io"], Some("h"), false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Aliased import should succeed");
    }

    #[test]
    fn test_glob_import() {
        let decls = vec![make_import_decl(vec!["io"], None, true, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Glob import should succeed (with warning)");
    }

    #[test]
    fn test_lazy_import() {
        let decls = vec![make_import_decl(vec!["fs"], None, false, true)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Lazy import should succeed");
    }

    #[test]
    fn test_unknown_package_import_fails() {
        let decls = vec![make_import_decl(vec!["nonexistent"], None, false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_err(), "Unknown package import should fail");
    }

    fn make_fn_decl(name: &str) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                context_clauses: vec![],
                body: vec![],
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: Span::new(0, 10),
            }),
            span: Span::new(0, 10),
        }
    }

    #[test]
    fn test_builtin_function_shadowing_allowed() {
        // User functions can shadow builtin functions (println, max, min, etc.)
        let decls = vec![make_fn_decl("println")];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Shadowing built-in function should be allowed");
    }

    #[test]
    fn test_builtin_type_shadowing_error() {
        use rask_ast::decl::StructDecl;
        let decls = vec![Decl {
            id: NodeId(0),
            kind: DeclKind::Struct(StructDecl {
                name: "Vec".to_string(),
                type_params: vec![],
                fields: vec![],
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }];
        let result = Resolver::resolve(&decls);
        assert!(result.is_err(), "Shadowing built-in type should fail");
    }

    #[test]
    fn test_prelude_enum_shadowing_error() {
        use rask_ast::decl::EnumDecl;
        let decls = vec![Decl {
            id: NodeId(0),
            kind: DeclKind::Enum(EnumDecl {
                name: "Option".to_string(),
                type_params: vec![],
                variants: vec![],
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }];
        let result = Resolver::resolve(&decls);
        assert!(result.is_err(), "Shadowing prelude enum should fail");
    }

    #[test]
    fn test_resolve_package_with_registry() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();

        let pkg_id = registry.add_package(
            "test_pkg".to_string(),
            vec!["test_pkg".to_string()],
            PathBuf::from("/test"),
        );

        let decls = vec![make_fn_decl("main")];
        let result = Resolver::resolve_package(&decls, &registry, pkg_id);
        assert!(result.is_ok(), "Package resolution should succeed");
    }

    #[test]
    fn test_resolve_package_bindings() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();
        let _http_pkg = registry.add_package(
            "http".to_string(),
            vec!["http".to_string()],
            PathBuf::from("/http"),
        );
        let main_pkg = registry.add_package(
            "main".to_string(),
            vec!["main".to_string()],
            PathBuf::from("/main"),
        );

        let decls = vec![
            make_import_decl(vec!["http"], None, false, false),
            make_fn_decl("main"),
        ];

        let result = Resolver::resolve_package(&decls, &registry, main_pkg);
        assert!(result.is_ok(), "Package with import should resolve");
    }

    fn make_pub_fn_decl(name: &str) -> Decl {
        Decl {
            id: NodeId(100),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                type_params: vec![],
                params: vec![],
                ret_ty: Some("string".to_string()),
                context_clauses: vec![],
                body: vec![],
                is_pub: true,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: Span::new(0, 10),
            }),
            span: Span::new(0, 10),
        }
    }

    fn make_pub_struct_decl(name: &str) -> Decl {
        use rask_ast::decl::{Field, FieldVisibility, StructDecl};
        Decl {
            id: NodeId(200),
            kind: DeclKind::Struct(StructDecl {
                name: name.to_string(),
                type_params: vec![],
                fields: vec![
                    Field { name: "x".to_string(), name_span: Span::new(0, 0), ty: "i32".to_string(), visibility: FieldVisibility::Public },
                    Field { name: "y".to_string(), name_span: Span::new(0, 0), ty: "i32".to_string(), visibility: FieldVisibility::Public },
                ],
                methods: vec![],
                is_pub: true,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }
    }

    #[test]
    fn test_cross_package_public_fn() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();

        // Library package with a public function
        let _lib_pkg = registry.add_package_with_decls(
            "lib".to_string(),
            vec!["lib".to_string()],
            PathBuf::from("/lib"),
            vec![make_pub_fn_decl("greet")],
        );

        // App package imports the lib
        let app_pkg = registry.add_package(
            "app".to_string(),
            vec!["app".to_string()],
            PathBuf::from("/app"),
        );

        let decls = vec![
            make_import_decl(vec!["lib"], None, false, false),
            make_fn_decl("main"),
        ];

        let result = Resolver::resolve_package(&decls, &registry, app_pkg);
        assert!(result.is_ok(), "Cross-package import should resolve: {:?}", result.err());

        // Verify the import created an ExternalPackage symbol
        let resolved = result.unwrap();
        let lib_sym = resolved.symbols.iter()
            .find(|s| s.name == "lib")
            .expect("lib symbol should exist");
        assert!(
            matches!(lib_sym.kind, SymbolKind::ExternalPackage { .. }),
            "lib should be ExternalPackage, got {:?}",
            lib_sym.kind
        );
    }

    #[test]
    fn test_cross_package_private_not_visible() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();

        // Library with a private function (make_fn_decl creates non-public)
        let _lib_pkg = registry.add_package_with_decls(
            "lib".to_string(),
            vec!["lib".to_string()],
            PathBuf::from("/lib"),
            vec![make_fn_decl("internal_helper")],
        );

        let app_pkg = registry.add_package(
            "app".to_string(),
            vec!["app".to_string()],
            PathBuf::from("/app"),
        );

        let decls = vec![
            // Try to import a specific private symbol
            make_import_decl(vec!["lib", "internal_helper"], None, false, false),
            make_fn_decl("main"),
        ];

        // This should still resolve (the import falls through to the fallback path)
        // but the symbol won't be the actual function — it'll be a dummy Variable
        let result = Resolver::resolve_package(&decls, &registry, app_pkg);
        assert!(result.is_ok(), "Import of non-public symbol should not error at resolve time");
    }

    #[test]
    fn test_cross_package_unqualified_import() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();

        let _lib_pkg = registry.add_package_with_decls(
            "lib".to_string(),
            vec!["lib".to_string()],
            PathBuf::from("/lib"),
            vec![make_pub_fn_decl("greet")],
        );

        let app_pkg = registry.add_package(
            "app".to_string(),
            vec!["app".to_string()],
            PathBuf::from("/app"),
        );

        let decls = vec![
            // import lib.greet — should put "greet" directly in scope
            make_import_decl(vec!["lib", "greet"], None, false, false),
            make_fn_decl("main"),
        ];

        let result = Resolver::resolve_package(&decls, &registry, app_pkg);
        assert!(result.is_ok(), "Unqualified import should resolve: {:?}", result.err());

        // Verify greet is in scope as a Function symbol (not a dummy Variable)
        let resolved = result.unwrap();
        let greet_sym = resolved.symbols.iter()
            .find(|s| s.name == "greet")
            .expect("greet symbol should exist in scope");
        assert!(
            matches!(greet_sym.kind, SymbolKind::Function { .. }),
            "greet should be Function, got {:?}",
            greet_sym.kind
        );
    }

    #[test]
    fn test_cross_package_struct() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();

        let _lib_pkg = registry.add_package_with_decls(
            "lib".to_string(),
            vec!["lib".to_string()],
            PathBuf::from("/lib"),
            vec![make_pub_struct_decl("Point")],
        );

        let app_pkg = registry.add_package(
            "app".to_string(),
            vec!["app".to_string()],
            PathBuf::from("/app"),
        );

        let decls = vec![
            make_import_decl(vec!["lib", "Point"], None, false, false),
            make_fn_decl("main"),
        ];

        let result = Resolver::resolve_package(&decls, &registry, app_pkg);
        assert!(result.is_ok(), "Struct import should resolve: {:?}", result.err());

        let resolved = result.unwrap();
        let point_sym = resolved.symbols.iter()
            .find(|s| s.name == "Point")
            .expect("Point symbol should exist");
        assert!(
            matches!(point_sym.kind, SymbolKind::Struct { .. }),
            "Point should be Struct, got {:?}",
            point_sym.kind
        );
    }

    #[test]
    fn test_single_file_resolve_unchanged() {
        // Verify that resolve() (not resolve_package) still works identically
        let decls = vec![
            make_import_decl(vec!["io"], None, false, false),
            make_fn_decl("main"),
        ];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Single-file resolve should still work");
    }

    #[test]
    fn test_resolve_stdlib_allows_builtin_function() {
        let decls = vec![make_fn_decl("println")];
        let result = Resolver::resolve_stdlib(&decls);
        assert!(result.is_ok(), "resolve_stdlib should allow redefining builtin functions");
    }

    #[test]
    fn test_resolve_stdlib_allows_builtin_type() {
        use rask_ast::decl::StructDecl;
        let decls = vec![Decl {
            id: NodeId(0),
            kind: DeclKind::Struct(StructDecl {
                name: "Vec".to_string(),
                type_params: vec![],
                fields: vec![],
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }];
        let result = Resolver::resolve_stdlib(&decls);
        assert!(result.is_ok(), "resolve_stdlib should allow redefining builtin types");
    }

    #[test]
    fn test_resolve_stdlib_allows_builtin_enum() {
        use rask_ast::decl::EnumDecl;
        let decls = vec![Decl {
            id: NodeId(0),
            kind: DeclKind::Enum(EnumDecl {
                name: "Option".to_string(),
                type_params: vec![],
                variants: vec![],
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }];
        let result = Resolver::resolve_stdlib(&decls);
        assert!(result.is_ok(), "resolve_stdlib should allow redefining builtin enums");
    }

    fn make_pub_enum_decl(name: &str, variants: &[&str]) -> Decl {
        use rask_ast::decl::{EnumDecl, Variant};
        Decl {
            id: NodeId(300),
            kind: DeclKind::Enum(EnumDecl {
                name: name.to_string(),
                type_params: vec![],
                variants: variants.iter().map(|v| Variant {
                    name: v.to_string(),
                    fields: vec![],
                    attrs: vec![],
                }).collect(),
                methods: vec![],
                is_pub: true,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }
    }

    #[test]
    fn test_external_decls_populated() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();

        let _lib_pkg = registry.add_package_with_decls(
            "lsm".to_string(),
            vec!["lsm".to_string()],
            PathBuf::from("/lsm"),
            vec![
                make_pub_struct_decl("Config"),
                make_pub_enum_decl("DbError", &["NotFound", "Corruption"]),
                make_fn_decl("internal_helper"), // private — should NOT appear
            ],
        );

        let app_pkg = registry.add_package(
            "app".to_string(),
            vec!["app".to_string()],
            PathBuf::from("/app"),
        );

        let decls = vec![
            make_import_decl(vec!["lsm"], None, false, false),
            make_fn_decl("main"),
        ];

        let result = Resolver::resolve_package(&decls, &registry, app_pkg);
        assert!(result.is_ok(), "Should resolve: {:?}", result.err());

        let resolved = result.unwrap();

        // external_decls should contain the public struct and enum
        let lsm_decls = resolved.external_decls.get("lsm")
            .expect("lsm should have external_decls");
        assert_eq!(lsm_decls.len(), 2, "Only public types (struct + enum), not private fn");

        let has_config = lsm_decls.iter().any(|d| matches!(&d.kind, DeclKind::Struct(s) if s.name == "Config"));
        let has_db_error = lsm_decls.iter().any(|d| matches!(&d.kind, DeclKind::Enum(e) if e.name == "DbError"));
        assert!(has_config, "Config struct should be in external_decls");
        assert!(has_db_error, "DbError enum should be in external_decls");
    }

    #[test]
    fn test_external_decls_empty_for_single_file() {
        let decls = vec![
            make_import_decl(vec!["io"], None, false, false),
            make_fn_decl("main"),
        ];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok());
        assert!(result.unwrap().external_decls.is_empty(),
            "Single-file resolve should have empty external_decls");
    }

    #[test]
    fn test_external_decls_excludes_private_types() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();

        // Package with only private (non-public) types
        let private_struct = Decl {
            id: NodeId(400),
            kind: DeclKind::Struct(rask_ast::decl::StructDecl {
                name: "InternalState".to_string(),
                type_params: vec![],
                fields: vec![],
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        };

        let _lib_pkg = registry.add_package_with_decls(
            "lib".to_string(),
            vec!["lib".to_string()],
            PathBuf::from("/lib"),
            vec![private_struct],
        );

        let app_pkg = registry.add_package(
            "app".to_string(),
            vec!["app".to_string()],
            PathBuf::from("/app"),
        );

        let decls = vec![
            make_import_decl(vec!["lib"], None, false, false),
            make_fn_decl("main"),
        ];

        let result = Resolver::resolve_package(&decls, &registry, app_pkg);
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert!(!resolved.external_decls.contains_key("lib"),
            "Package with only private types should not appear in external_decls");
    }

    #[test]
    fn test_imported_symbol_field_access_resolved() {
        // Regression: field access on an imported type (e.g., DbError.NotFound)
        // must insert a resolution for the object ident, even though it's in
        // imported_symbols. Without this, stale resolutions from other passes
        // (stdlib) can leak through.
        use crate::PackageRegistry;
        use std::path::PathBuf;
        use rask_ast::expr::{Expr, ExprKind};
        use rask_ast::stmt::{Stmt, StmtKind};

        let mut registry = PackageRegistry::new();

        let _lib_pkg = registry.add_package_with_decls(
            "lib".to_string(),
            vec!["lib".to_string()],
            PathBuf::from("/lib"),
            vec![make_pub_enum_decl("DbError", &["NotFound", "Corruption"])],
        );

        let app_pkg = registry.add_package(
            "app".to_string(),
            vec!["app".to_string()],
            PathBuf::from("/app"),
        );

        // Build: import lib; import lib.DbError; func main() { DbError.NotFound }
        let field_expr = Expr {
            id: NodeId(10),
            kind: ExprKind::Field {
                object: Box::new(Expr {
                    id: NodeId(11),
                    kind: ExprKind::Ident("DbError".to_string()),
                    span: Span::new(0, 7),
                }),
                field: "NotFound".to_string(),
            },
            span: Span::new(0, 16),
        };

        let main_decl = Decl {
            id: NodeId(12),
            kind: DeclKind::Fn(FnDecl {
                name: "main".to_string(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                context_clauses: vec![],
                body: vec![Stmt {
                    id: NodeId(13),
                    kind: StmtKind::Expr(field_expr),
                    span: Span::new(0, 16),
                }],
                is_pub: false,
                is_private: false,
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
                span: Span::new(0, 20),
            }),
            span: Span::new(0, 20),
        };

        let decls = vec![
            make_import_decl(vec!["lib"], None, false, false),
            make_import_decl(vec!["lib", "DbError"], None, false, false),
            main_decl,
        ];

        let result = Resolver::resolve_package(&decls, &registry, app_pkg);
        assert!(result.is_ok(), "Should resolve: {:?}", result.err());

        let resolved = result.unwrap();

        // The DbError ident (NodeId 11) must have a resolution pointing to the
        // exported Enum symbol, not be left unresolved.
        assert!(
            resolved.resolutions.contains_key(&NodeId(11)),
            "DbError ident in field access must be resolved"
        );

        let sym_id = resolved.resolutions[&NodeId(11)];
        let sym = resolved.symbols.get(sym_id).expect("symbol should exist");
        assert!(
            matches!(sym.kind, SymbolKind::Enum { .. }),
            "DbError should resolve to Enum, got {:?}",
            sym.kind
        );
    }
}
