//! The name resolver implementation.

use std::collections::{HashMap, HashSet};
use rask_ast::decl::{Decl, DeclKind, FnDecl, StructDecl, EnumDecl, TraitDecl, ImplDecl, ImportDecl, ExportDecl};
use rask_ast::stmt::{Stmt, StmtKind};
use rask_ast::expr::{Expr, ExprKind, Pattern};
use rask_ast::{NodeId, Span};

use crate::error::ResolveError;
use crate::scope::{ScopeTree, ScopeKind};
use crate::symbol::{BuiltinModuleKind, SymbolTable, SymbolId, SymbolKind};
use crate::package::PackageId;
use crate::ResolvedProgram;

/// The name resolver.
pub struct Resolver {
    symbols: SymbolTable,
    scopes: ScopeTree,
    resolutions: HashMap<NodeId, SymbolId>,
    errors: Vec<ResolveError>,
    /// Currently resolving function (for return validation).
    current_function: Option<SymbolId>,

    // Package-aware fields
    /// Current package being resolved (None for single-file mode).
    #[allow(dead_code)]
    current_package: Option<PackageId>,
    /// Qualified package bindings: "http" -> PackageId.
    #[allow(dead_code)]
    package_bindings: HashMap<String, PackageId>,
    /// Symbols imported from other packages (to track shadowing).
    imported_symbols: HashSet<String>,
    /// Lazy imports - packages that should be loaded on first use.
    /// Maps binding name to package path for deferred loading.
    lazy_imports: HashMap<String, Vec<String>>,
}

impl Resolver {
    /// Create a new resolver for single-file mode.
    pub fn new() -> Self {
        let mut resolver = Self {
            symbols: SymbolTable::new(),
            scopes: ScopeTree::new(),
            resolutions: HashMap::new(),
            errors: Vec::new(),
            current_function: None,
            // Package fields empty for single-file mode
            current_package: None,
            package_bindings: HashMap::new(),
            imported_symbols: HashSet::new(),
            lazy_imports: HashMap::new(),
        };

        // Register built-in functions
        resolver.register_builtins();

        resolver
    }

    /// Register built-in functions like println.
    fn register_builtins(&mut self) {
        use crate::symbol::{BuiltinFunctionKind, BuiltinTypeKind};

        // Built-in functions
        let builtin_fns = [
            ("println", BuiltinFunctionKind::Println, None::<&str>),
            ("print", BuiltinFunctionKind::Print, None),
            ("panic", BuiltinFunctionKind::Panic, Some("!")),  // Never returns
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

        // Built-in types
        let builtin_types = [
            ("Vec", BuiltinTypeKind::Vec),
            ("Map", BuiltinTypeKind::Map),
            ("Set", BuiltinTypeKind::Set),
            ("string", BuiltinTypeKind::String),
            ("Error", BuiltinTypeKind::Error),
            ("Channel", BuiltinTypeKind::Channel),
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

        // Register prelude enums
        self.register_builtin_enum("Option", &["Some", "None"]);
        self.register_builtin_enum("Result", &["Ok", "Err"]);
    }

    /// Register a built-in enum with its variants.
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

    /// Check if a name is a built-in (function, type, module, or prelude enum).
    fn is_builtin_name(&self, name: &str) -> bool {
        // Check if the name is already defined and is a built-in
        if let Some(sym_id) = self.scopes.lookup(name) {
            if let Some(sym) = self.symbols.get(sym_id) {
                return matches!(
                    sym.kind,
                    SymbolKind::BuiltinType { .. }
                        | SymbolKind::BuiltinFunction { .. }
                        | SymbolKind::BuiltinModule { .. }
                ) || (matches!(sym.kind, SymbolKind::Enum { .. } | SymbolKind::EnumVariant { .. })
                    && sym.span == Span::new(0, 0)); // Built-in enums have span (0, 0)
            }
        }
        false
    }

    /// Resolve all names in declarations.
    pub fn resolve(decls: &[Decl]) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();

        // Pass 1: Collect all top-level declarations
        resolver.collect_declarations(decls);

        // Pass 2: Resolve all bodies
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

    /// Resolve all names in a package with access to other packages.
    pub fn resolve_package(
        decls: &[Decl],
        registry: &crate::PackageRegistry,
        current_package: crate::PackageId,
    ) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();

        // Set package context
        resolver.current_package = Some(current_package);

        // Pre-populate package bindings from registry
        for pkg in registry.packages() {
            let pkg_name = pkg.name.clone();
            resolver.package_bindings.insert(pkg_name, pkg.id);
        }

        // Pass 1: Collect all top-level declarations
        resolver.collect_declarations(decls);

        // Pass 2: Resolve all bodies
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
                DeclKind::Impl(_) => {
                    // Impl blocks don't declare new names, they add methods to existing types
                    // We'll handle them in pass 2
                }
                DeclKind::Import(import_decl) => {
                    self.resolve_import(import_decl, decl.span);
                }
                DeclKind::Export(export_decl) => {
                    self.resolve_export(export_decl, decl.span);
                }
                DeclKind::Const(const_decl) => {
                    // Top-level const
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
                DeclKind::Test(_) | DeclKind::Benchmark(_) => {
                    // Test and benchmark blocks don't declare names
                }
            }
        }
    }

    fn declare_function(&mut self, fn_decl: &FnDecl, span: Span, is_pub: bool) -> SymbolId {
        // Check for built-in shadowing
        if self.is_builtin_name(&fn_decl.name) {
            self.errors.push(ResolveError::shadows_builtin(fn_decl.name.clone(), span));
        }

        // Create function symbol (params filled in later)
        let sym_id = self.symbols.insert(
            fn_decl.name.clone(),
            SymbolKind::Function { params: vec![], ret_ty: fn_decl.ret_ty.clone() },
            None,
            span,
            is_pub,
        );
        if let Err(e) = self.scopes.define(fn_decl.name.clone(), sym_id, span) {
            self.errors.push(e);
        }
        sym_id
    }

    fn declare_struct(&mut self, struct_decl: &StructDecl, span: Span) {
        // Check for built-in shadowing
        if self.is_builtin_name(&struct_decl.name) {
            self.errors.push(ResolveError::shadows_builtin(struct_decl.name.clone(), span));
        }

        // Create struct symbol
        let sym_id = self.symbols.insert(
            struct_decl.name.clone(),
            SymbolKind::Struct { fields: vec![] },
            None,
            span,
            struct_decl.is_pub,
        );
        if let Err(e) = self.scopes.define(struct_decl.name.clone(), sym_id, span) {
            self.errors.push(e);
        }

        // Declare fields
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

        // Update struct with field info
        if let Some(sym) = self.symbols.get_mut(sym_id) {
            sym.kind = SymbolKind::Struct { fields: field_syms };
        }
    }

    fn declare_enum(&mut self, enum_decl: &EnumDecl, span: Span) {
        // Check for built-in shadowing
        if self.is_builtin_name(&enum_decl.name) {
            self.errors.push(ResolveError::shadows_builtin(enum_decl.name.clone(), span));
        }

        // Create enum symbol
        let sym_id = self.symbols.insert(
            enum_decl.name.clone(),
            SymbolKind::Enum { variants: vec![] },
            None,
            span,
            enum_decl.is_pub,
        );
        if let Err(e) = self.scopes.define(enum_decl.name.clone(), sym_id, span) {
            self.errors.push(e);
        }

        // Declare variants
        let mut variant_syms = Vec::new();
        for variant in &enum_decl.variants {
            let variant_sym = self.symbols.insert(
                variant.name.clone(),
                SymbolKind::EnumVariant { enum_id: sym_id },
                None,
                span,
                enum_decl.is_pub,
            );
            // Register variant in scope so it can be referenced by name
            if let Err(e) = self.scopes.define(variant.name.clone(), variant_sym, span) {
                self.errors.push(e);
            }
            variant_syms.push((variant.name.clone(), variant_sym));
        }

        // Update enum with variant info
        if let Some(sym) = self.symbols.get_mut(sym_id) {
            sym.kind = SymbolKind::Enum { variants: variant_syms };
        }
    }

    fn declare_trait(&mut self, trait_decl: &TraitDecl, span: Span) {
        // Check for built-in shadowing
        if self.is_builtin_name(&trait_decl.name) {
            self.errors.push(ResolveError::shadows_builtin(trait_decl.name.clone(), span));
        }

        let sym_id = self.symbols.insert(
            trait_decl.name.clone(),
            SymbolKind::Trait { methods: vec![] },
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

    /// Resolve an import declaration.
    ///
    /// Import types:
    /// - `import pkg` (path.len() == 1): Qualified package import
    /// - `import pkg.Name` (path.len() > 1): Symbol import (unqualified access)
    /// - `import pkg.*` (is_glob): Glob import (with warning)
    /// - `import pkg as p` or `import pkg.Name as N`: Aliased import
    fn resolve_import(&mut self, import_decl: &ImportDecl, span: Span) {
        let path = &import_decl.path;

        if path.is_empty() {
            // Invalid empty path - shouldn't happen from parser
            self.errors.push(ResolveError::unknown_package(vec![], span));
            return;
        }

        // In single-file mode without package registry, imports are no-ops
        // In the future, this will look up packages from the registry
        // For now, we record the import intent for later phases

        if import_decl.is_glob {
            // Glob import: import pkg.*
            // For now, just emit a warning (not an error per spec change)
            // TODO: When package registry is available, import all public symbols
            eprintln!(
                "warning: glob import `import {}.*` - imports all public symbols",
                path.join(".")
            );
        }

        if path.len() == 1 {
            // Qualified package import: `import pkg` or `import pkg as alias`
            let pkg_name = &path[0];
            let binding_name = import_decl.alias.as_ref().unwrap_or(pkg_name).clone();

            // Check if this is a known stdlib module
            let stdlib_module = match pkg_name.as_str() {
                "io" => Some(BuiltinModuleKind::Io),
                "fs" => Some(BuiltinModuleKind::Fs),
                "env" => Some(BuiltinModuleKind::Env),
                "cli" => Some(BuiltinModuleKind::Cli),
                "std" => Some(BuiltinModuleKind::Std),
                _ => None,
            };

            if let Some(module_kind) = stdlib_module {
                // Register stdlib module as a symbol in scope
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
            }

            // Record that this name is an imported package binding
            // This will be used later when resolving `pkg.Symbol` expressions
            self.imported_symbols.insert(binding_name.clone());

            // Track lazy imports for deferred loading
            if import_decl.is_lazy {
                self.lazy_imports.insert(binding_name, path.clone());
            }
        } else {
            // Symbol import: `import pkg.Name` or `import pkg.Name as Alias`
            // The last component is the symbol name
            let symbol_name = path.last().unwrap();
            let binding_name = import_decl.alias.as_ref().unwrap_or(symbol_name).clone();

            // Check for shadowing existing definitions
            if self.scopes.lookup(&binding_name).is_some() {
                self.errors.push(ResolveError::shadows_import(binding_name.clone(), span));
                return;
            }

            // Record that this symbol was imported
            // In the future, this will look up the symbol from the package
            // and register it in the current scope
            self.imported_symbols.insert(binding_name.clone());

            // Track lazy imports for deferred loading
            if import_decl.is_lazy {
                self.lazy_imports.insert(binding_name, path.clone());
            }
        }
    }

    /// Resolve an export (re-export) declaration.
    ///
    /// Export types:
    /// - `export internal.Name` - re-export as current package's public API
    /// - `export internal.Name as Alias` - re-export with rename
    /// - `export internal.Name, other.Thing` - multiple re-exports
    fn resolve_export(&mut self, export_decl: &ExportDecl, span: Span) {
        for item in &export_decl.items {
            let path = &item.path;

            if path.is_empty() {
                // Invalid empty path
                self.errors.push(ResolveError::unknown_package(vec![], span));
                continue;
            }

            // In single-file mode, exports are recorded for later processing
            // When we have package registry:
            // 1. Look up the symbol at path
            // 2. Verify it's accessible (public or same package)
            // 3. Add to current package's public exports with alias if present

            // For now, just validate the export path format
            // The actual symbol lookup requires package registry integration
            let export_name = item.alias.as_ref()
                .unwrap_or_else(|| path.last().unwrap());

            // Record this export for visibility tracking
            // In the future, this will be used to build the package's public API
            let _ = export_name; // Suppress unused warning for now
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
                    // Resolve method bodies
                    for method in &struct_decl.methods {
                        self.resolve_function(method);
                    }
                }
                DeclKind::Enum(enum_decl) => {
                    // Resolve method bodies
                    for method in &enum_decl.methods {
                        self.resolve_function(method);
                    }
                }
                DeclKind::Trait(trait_decl) => {
                    // Resolve default method implementations
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
                    // Resolve the initializer expression
                    self.resolve_expr(&const_decl.init);
                }
                DeclKind::Test(test_decl) => {
                    // Resolve test body
                    self.scopes.push(ScopeKind::Block);
                    for stmt in &test_decl.body {
                        self.resolve_stmt(stmt);
                    }
                    self.scopes.pop();
                }
                DeclKind::Benchmark(bench_decl) => {
                    // Resolve benchmark body
                    self.scopes.push(ScopeKind::Block);
                    for stmt in &bench_decl.body {
                        self.resolve_stmt(stmt);
                    }
                    self.scopes.pop();
                }
                DeclKind::Import(_) => {}
                DeclKind::Export(_) => {}
            }
        }
    }

    fn resolve_function(&mut self, fn_decl: &FnDecl) {
        // Look up the function symbol
        let fn_sym = self.scopes.lookup(&fn_decl.name);
        self.current_function = fn_sym;

        // Push function scope
        let scope_kind = if let Some(sym_id) = fn_sym {
            ScopeKind::Function(sym_id)
        } else {
            // Anonymous function or method - still need a scope
            ScopeKind::Function(SymbolId(u32::MAX))
        };
        self.scopes.push(scope_kind);

        // Declare parameters
        let mut param_syms = Vec::new();
        for param in &fn_decl.params {
            let param_sym = self.symbols.insert(
                param.name.clone(),
                SymbolKind::Parameter { is_take: param.is_take },
                Some(param.ty.clone()),
                Span::new(0, 0), // TODO: Get actual param span
                false,
            );
            if let Err(e) = self.scopes.define(param.name.clone(), param_sym, Span::new(0, 0)) {
                self.errors.push(e);
            }
            param_syms.push(param_sym);

            // Resolve default value if present
            if let Some(default) = &param.default {
                self.resolve_expr(default);
            }
        }

        // Update function symbol with parameter info
        if let Some(sym_id) = fn_sym {
            if let Some(sym) = self.symbols.get_mut(sym_id) {
                if let SymbolKind::Function { params, .. } = &mut sym.kind {
                    *params = param_syms;
                }
            }
        }

        // Resolve body
        for stmt in &fn_decl.body {
            self.resolve_stmt(stmt);
        }

        self.scopes.pop();
        self.current_function = None;
    }

    fn resolve_impl(&mut self, impl_decl: &ImplDecl) {
        // Resolve method bodies in impl blocks
        // Note: We don't push a new scope here - methods create their own
        for method in &impl_decl.methods {
            self.resolve_function(method);
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
            StmtKind::Let { name, ty, init } => {
                // Resolve init first (can't reference the new binding)
                self.resolve_expr(init);
                let sym_id = self.symbols.insert(
                    name.clone(),
                    SymbolKind::Variable { mutable: true },
                    ty.clone(),
                    stmt.span,
                    false,
                );
                if let Err(e) = self.scopes.define(name.clone(), sym_id, stmt.span) {
                    self.errors.push(e);
                }
            }
            StmtKind::Const { name, ty, init } => {
                self.resolve_expr(init);
                let sym_id = self.symbols.insert(
                    name.clone(),
                    SymbolKind::Variable { mutable: false },
                    ty.clone(),
                    stmt.span,
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
            StmtKind::Break(label) => {
                if let Some(lbl) = label {
                    if !self.scopes.label_in_scope(lbl) {
                        self.errors.push(ResolveError::invalid_break(Some(lbl.clone()), stmt.span));
                    }
                } else if !self.scopes.in_loop() {
                    self.errors.push(ResolveError::invalid_break(None, stmt.span));
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
            StmtKind::Deliver { label, value } => {
                if let Some(lbl) = label {
                    if !self.scopes.label_in_scope(lbl) {
                        self.errors.push(ResolveError::invalid_break(Some(lbl.clone()), stmt.span));
                    }
                } else if !self.scopes.in_loop() {
                    self.errors.push(ResolveError::invalid_break(None, stmt.span));
                }
                self.resolve_expr(value);
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
                let sym_id = self.symbols.insert(
                    binding.clone(),
                    SymbolKind::Variable { mutable: false },
                    None,
                    stmt.span,
                    false,
                );
                if let Err(e) = self.scopes.define(binding.clone(), sym_id, stmt.span) {
                    self.errors.push(e);
                }
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
            }
            StmtKind::Ensure(body) => {
                self.scopes.push(ScopeKind::Block);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
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
            ExprKind::Int(_) | ExprKind::Float(_) | ExprKind::String(_) |
            ExprKind::Char(_) | ExprKind::Bool(_) => {
                // Literals don't need resolution
            }
            ExprKind::Ident(name) => {
                match self.scopes.lookup(name) {
                    Some(sym_id) => {
                        self.resolutions.insert(expr.id, sym_id);
                    }
                    None => {
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
                    self.resolve_expr(arg);
                }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.resolve_expr(object);
                // Method name resolution is deferred to type checking
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Field { object, field } => {
                // Check if this is a qualified package access: pkg.Name
                if let ExprKind::Ident(name) = &object.kind {
                    if self.imported_symbols.contains(name) {
                        // This is a qualified package access like `http.Request`
                        // In the future, we'll look up the symbol from the package
                        // For now, just note that we recognize the pattern
                        // The actual symbol lookup requires the package registry
                        let _ = field; // Suppress unused warning
                        return;
                    }
                }
                // Regular field access - resolve the object
                self.resolve_expr(object);
                // Field resolution is deferred to type checking
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
                // Resolve struct type name
                if let Some(sym_id) = self.scopes.lookup(name) {
                    self.resolutions.insert(expr.id, sym_id);
                } else {
                    self.errors.push(ResolveError::undefined(name.clone(), expr.span));
                }
                // Resolve field values
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
            ExprKind::WithBlock { args, body, .. } => {
                for arg in args {
                    self.resolve_expr(arg);
                }
                self.scopes.push(ScopeKind::Block);
                for stmt in body {
                    self.resolve_stmt(stmt);
                }
                self.scopes.pop();
            }
            ExprKind::Closure { params, body } => {
                self.scopes.push(ScopeKind::Closure);
                for param in params {
                    let sym_id = self.symbols.insert(
                        param.name.clone(),
                        SymbolKind::Parameter { is_take: false },
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
        }
    }

    // =========================================================================
    // Pattern Resolution
    // =========================================================================

    fn resolve_pattern(&mut self, pattern: &Pattern) {
        match pattern {
            Pattern::Wildcard => {}
            Pattern::Ident(name) => {
                // First, check if this identifier refers to an existing enum variant
                if let Some(sym_id) = self.scopes.lookup(name) {
                    if let Some(sym) = self.symbols.get(sym_id) {
                        if matches!(sym.kind, SymbolKind::EnumVariant { .. }) {
                            // This is an enum variant reference, not a new binding
                            return;
                        }
                    }
                }

                // Not an enum variant - create a new variable binding
                let sym_id = self.symbols.insert(
                    name.clone(),
                    SymbolKind::Variable { mutable: false },
                    None,
                    Span::new(0, 0), // TODO: Get actual pattern span
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
                // Resolve constructor name (enum variant)
                // This might fail if the variant doesn't exist, but full checking is type-level
                if let Some(sym_id) = self.scopes.lookup(name) {
                    // Found - might be an enum variant
                    let _ = sym_id; // We don't record pattern resolutions currently
                }
                for field_pattern in fields {
                    self.resolve_pattern(field_pattern);
                }
            }
            Pattern::Struct { name, fields, .. } => {
                // Resolve struct name
                if let Some(_sym_id) = self.scopes.lookup(name) {
                    // Found struct
                }
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
                // All branches must bind the same names
                // For now, just resolve each
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
    fn test_qualified_package_import() {
        // import http
        let decls = vec![make_import_decl(vec!["http"], None, false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Qualified package import should succeed");
    }

    #[test]
    fn test_symbol_import() {
        // import http.Request
        let decls = vec![make_import_decl(vec!["http", "Request"], None, false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Symbol import should succeed");
    }

    #[test]
    fn test_aliased_import() {
        // import http as h
        let decls = vec![make_import_decl(vec!["http"], Some("h"), false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Aliased import should succeed");
    }

    #[test]
    fn test_glob_import() {
        // import http.*
        let decls = vec![make_import_decl(vec!["http"], None, true, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Glob import should succeed (with warning)");
    }

    #[test]
    fn test_lazy_import() {
        // import lazy http
        let decls = vec![make_import_decl(vec!["http"], None, false, true)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Lazy import should succeed");
    }

    // Helper to make a function declaration
    fn make_fn_decl(name: &str) -> Decl {
        use rask_ast::decl::FnDecl;
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                params: vec![],
                ret_ty: None,
                body: vec![],
                is_pub: false,
                is_comptime: false,
                is_unsafe: false,
            }),
            span: Span::new(0, 10),
        }
    }

    #[test]
    fn test_builtin_function_shadowing_error() {
        // func println() {} should error
        let decls = vec![make_fn_decl("println")];
        let result = Resolver::resolve(&decls);
        assert!(result.is_err(), "Shadowing built-in function should fail");
    }

    #[test]
    fn test_builtin_type_shadowing_error() {
        // struct Vec {} should error
        use rask_ast::decl::StructDecl;
        let decls = vec![Decl {
            id: NodeId(0),
            kind: DeclKind::Struct(StructDecl {
                name: "Vec".to_string(),
                fields: vec![],
                methods: vec![],
                is_pub: false,
                attrs: vec![],
            }),
            span: Span::new(0, 10),
        }];
        let result = Resolver::resolve(&decls);
        assert!(result.is_err(), "Shadowing built-in type should fail");
    }

    #[test]
    fn test_prelude_enum_shadowing_error() {
        // enum Option {} should error
        use rask_ast::decl::EnumDecl;
        let decls = vec![Decl {
            id: NodeId(0),
            kind: DeclKind::Enum(EnumDecl {
                name: "Option".to_string(),
                variants: vec![],
                methods: vec![],
                is_pub: false,
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

        // Create a minimal package registry
        let mut registry = PackageRegistry::new();

        // Create a mock package (normally this comes from discovery)
        let pkg_id = registry.add_package(
            "test_pkg".to_string(),
            vec!["test_pkg".to_string()],
            PathBuf::from("/test"),
        );

        // Test resolution with package context
        let decls = vec![make_fn_decl("main")];
        let result = Resolver::resolve_package(&decls, &registry, pkg_id);
        assert!(result.is_ok(), "Package resolution should succeed");
    }

    #[test]
    fn test_resolve_package_bindings() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        // Create registry with two packages
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

        // Import http package and use it
        let decls = vec![
            make_import_decl(vec!["http"], None, false, false),
            make_fn_decl("main"),
        ];

        let result = Resolver::resolve_package(&decls, &registry, main_pkg);
        assert!(result.is_ok(), "Package with import should resolve");
    }
}
