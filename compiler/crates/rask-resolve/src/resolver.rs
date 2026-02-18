// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The name resolver implementation.

use std::collections::{HashMap, HashSet};
use rask_ast::decl::{Decl, DeclKind, FnDecl, StructDecl, EnumDecl, TraitDecl, ImplDecl, ImportDecl, ExportDecl, TypeParam};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
use rask_ast::expr::{Expr, ExprKind, Pattern};
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
            ("spawn", BuiltinFunctionKind::Spawn, None),
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
            ("Owned", BuiltinTypeKind::Owned),
            ("f32x4", BuiltinTypeKind::Simd),
            ("f32x8", BuiltinTypeKind::Simd),
            ("f64x2", BuiltinTypeKind::Simd),
            ("f64x4", BuiltinTypeKind::Simd),
            ("i32x4", BuiltinTypeKind::Simd),
            ("i32x8", BuiltinTypeKind::Simd),
            ("Rng", BuiltinTypeKind::Rng),
            ("File", BuiltinTypeKind::File),
            // Atomic types
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

        let builtin_modules = [
            ("io", BuiltinModuleKind::Io),
            ("fs", BuiltinModuleKind::Fs),
            ("cli", BuiltinModuleKind::Cli),
            ("std", BuiltinModuleKind::Std),
            ("json", BuiltinModuleKind::Json),
            ("random", BuiltinModuleKind::Random),
            ("time", BuiltinModuleKind::Time),
            ("math", BuiltinModuleKind::Math),
            ("path", BuiltinModuleKind::Path),
            ("os", BuiltinModuleKind::Os),
            ("net", BuiltinModuleKind::Net),
            ("core", BuiltinModuleKind::Core),
            ("async", BuiltinModuleKind::Async),
        ];

        for (name, module) in builtin_modules {
            let sym_id = self.symbols.insert(
                name.to_string(),
                SymbolKind::BuiltinModule { module },
                None,
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(name.to_string(), sym_id, Span::new(0, 0));
        }

        // Register net module types (HttpRequest, HttpResponse, TcpListener, TcpConnection)
        for net_type in &["HttpRequest", "HttpResponse", "TcpListener", "TcpConnection"] {
            let sym_id = self.symbols.insert(
                net_type.to_string(),
                SymbolKind::Struct { fields: vec![] },
                None,
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(net_type.to_string(), sym_id, Span::new(0, 0));
        }

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
                ) || (matches!(sym.kind, SymbolKind::Enum { .. } | SymbolKind::EnumVariant { .. })
                    && sym.span == Span::new(0, 0));
            }
        }
        false
    }

    pub fn resolve(decls: &[Decl]) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();

        resolver.collect_declarations(decls);
        resolver.resolve_bodies(decls);

        if resolver.errors.is_empty() {
            Ok(ResolvedProgram {
                symbols: resolver.symbols,
                resolutions: resolver.resolutions,
            })
        } else {
            Err(resolver.errors)
        }
    }

    pub fn resolve_package(
        decls: &[Decl],
        registry: &crate::PackageRegistry,
        current_package: crate::PackageId,
    ) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();

        resolver.current_package = Some(current_package);

        for pkg in registry.packages() {
            resolver.package_bindings.insert(pkg.name.clone(), pkg.id);
        }

        // Collect public symbols from external packages
        for pkg in registry.packages() {
            if pkg.id != current_package {
                resolver.collect_package_exports(pkg);
            }
        }

        resolver.collect_declarations(decls);
        resolver.resolve_bodies(decls);

        if resolver.errors.is_empty() {
            Ok(ResolvedProgram {
                symbols: resolver.symbols,
                resolutions: resolver.resolutions,
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
                            field.is_pub,
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
                DeclKind::Test(_) | DeclKind::Benchmark(_) => {}
                DeclKind::Package(_) => {}
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
        if self.is_builtin_name(&base) {
            self.errors.push(ResolveError::shadows_builtin(base.clone(), span));
        }

        let sym_id = self.symbols.insert(
            base.clone(),
            SymbolKind::Function { params: vec![], ret_ty: fn_decl.ret_ty.clone(), context_clauses: fn_decl.context_clauses.clone() },
            None,
            span,
            is_pub,
        );
        if let Err(e) = self.scopes.define(base, sym_id, span) {
            self.errors.push(e);
        }
        sym_id
    }

    fn declare_struct(&mut self, struct_decl: &StructDecl, span: Span) {
        let base = Self::base_name(&struct_decl.name).to_string();
        if self.is_builtin_name(&base) {
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
                field.is_pub,
            );
            field_syms.push((field.name.clone(), field_sym));
        }

        if let Some(sym) = self.symbols.get_mut(sym_id) {
            sym.kind = SymbolKind::Struct { fields: field_syms };
        }
    }

    fn declare_enum(&mut self, enum_decl: &EnumDecl, span: Span) {
        let base = Self::base_name(&enum_decl.name).to_string();
        if self.is_builtin_name(&base) {
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
        if self.is_builtin_name(&trait_decl.name) {
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
                // Allow imports to replace builtins (e.g. `import async.spawn`
                // replaces the builtin `spawn`)
                let is_builtin = self.symbols.get(existing_id).map_or(false, |sym| {
                    matches!(
                        sym.kind,
                        SymbolKind::BuiltinFunction { .. }
                            | SymbolKind::BuiltinType { .. }
                            | SymbolKind::BuiltinModule { .. }
                    )
                });
                if !is_builtin {
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
                | "thread"
            );

            if is_stdlib_module {
                // For stdlib imports like `import async.spawn`, the binding may
                // already exist as a builtin. Accept it without redefinition.
                if self.scopes.lookup(&binding_name).is_none() {
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
                    self.scopes.push(ScopeKind::Block);
                    for stmt in &test_decl.body {
                        self.resolve_stmt(stmt);
                    }
                    self.scopes.pop();
                }
                DeclKind::Benchmark(bench_decl) => {
                    self.scopes.push(ScopeKind::Block);
                    for stmt in &bench_decl.body {
                        self.resolve_stmt(stmt);
                    }
                    self.scopes.pop();
                }
                DeclKind::Import(_) => {}
                DeclKind::Export(_) => {}
                DeclKind::Extern(_) => {}
                DeclKind::Package(_) => {}
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
            StmtKind::LetTuple { names, init } => {
                self.resolve_expr(init);
                for name in names {
                    let sym_id = self.symbols.insert(
                        name.clone(),
                        SymbolKind::Variable { mutable: true },
                        None,
                        stmt.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(name.clone(), sym_id, stmt.span) {
                        self.errors.push(e);
                    }
                }
            }
            StmtKind::ConstTuple { names, init } => {
                self.resolve_expr(init);
                for name in names {
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
                        self.errors.push(ResolveError::invalid_break(Some(lbl.clone()), stmt.span));
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
            StmtKind::For { label, binding, iter, body } => {
                self.resolve_expr(iter);
                self.scopes.push(ScopeKind::Loop { label: label.clone() });
                let names = match binding {
                    ForBinding::Single(name) => vec![name.clone()],
                    ForBinding::Tuple(names) => names.clone(),
                };
                for name in &names {
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
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
            }
        }
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
            ExprKind::Try(inner) => {
                self.resolve_expr(inner);
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
                for (source_expr, _) in bindings.iter() {
                    self.resolve_expr(source_expr);
                }
                self.scopes.push(ScopeKind::Block);
                for (_, binding_name) in bindings {
                    let sym_id = self.symbols.insert(
                        binding_name.clone(),
                        SymbolKind::Variable { mutable: true },
                        None,
                        expr.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(binding_name.clone(), sym_id, expr.span) {
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
                for stmt in body {
                    self.resolve_stmt(stmt);
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
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }
    }

    #[test]
    fn test_builtin_function_shadowing_error() {
        let decls = vec![make_fn_decl("println")];
        let result = Resolver::resolve(&decls);
        assert!(result.is_err(), "Shadowing built-in function should fail");
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
                is_comptime: false,
                is_unsafe: false,
                abi: None,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }
    }

    fn make_pub_struct_decl(name: &str) -> Decl {
        use rask_ast::decl::{Field, StructDecl};
        Decl {
            id: NodeId(200),
            kind: DeclKind::Struct(StructDecl {
                name: name.to_string(),
                type_params: vec![],
                fields: vec![
                    Field { name: "x".to_string(), name_span: Span::new(0, 0), ty: "i32".to_string(), is_pub: true },
                    Field { name: "y".to_string(), name_span: Span::new(0, 0), ty: "i32".to_string(), is_pub: true },
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
}
