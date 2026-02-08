// SPDX-License-Identifier: (MIT OR Apache-2.0)
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

pub struct Resolver {
    symbols: SymbolTable,
    scopes: ScopeTree,
    resolutions: HashMap<NodeId, SymbolId>,
    errors: Vec<ResolveError>,
    current_function: Option<SymbolId>,

    #[allow(dead_code)]
    current_package: Option<PackageId>,
    #[allow(dead_code)]
    package_bindings: HashMap<String, PackageId>,
    imported_symbols: HashSet<String>,
    lazy_imports: HashMap<String, Vec<String>>,
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
            ("Atomic", BuiltinTypeKind::Atomic),
            ("Shared", BuiltinTypeKind::Shared),
            ("Owned", BuiltinTypeKind::Owned),
            ("SpscRingBuffer", BuiltinTypeKind::SpscRingBuffer),
            ("f32x8", BuiltinTypeKind::F32x8),
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
        self.register_builtin_enum("Ordering", &["Less", "Equal", "Greater"]);

        let builtin_modules = [
            ("io", BuiltinModuleKind::Io),
            ("fs", BuiltinModuleKind::Fs),
            ("json", BuiltinModuleKind::Json),
            ("random", BuiltinModuleKind::Random),
            ("time", BuiltinModuleKind::Time),
            ("math", BuiltinModuleKind::Math),
            ("path", BuiltinModuleKind::Path),
            ("os", BuiltinModuleKind::Os),
            ("net", BuiltinModuleKind::Net),
            ("core", BuiltinModuleKind::Core),
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
            Some("*mut ()".to_string()),
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
            let pkg_name = pkg.name.clone();
            resolver.package_bindings.insert(pkg_name, pkg.id);
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
                DeclKind::Extern(extern_decl) => {
                    // Declare extern functions in symbol table
                    let sym_id = self.symbols.insert(
                        extern_decl.name.clone(),
                        SymbolKind::Function { params: vec![], ret_ty: extern_decl.ret_ty.clone() },
                        None,
                        decl.span,
                        false, // Extern functions are not pub-exported from module
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
            SymbolKind::Function { params: vec![], ret_ty: fn_decl.ret_ty.clone() },
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
        if let Err(e) = self.scopes.define(base, sym_id, span) {
            self.errors.push(e);
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
        if let Err(e) = self.scopes.define(base, sym_id, span) {
            self.errors.push(e);
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
            }

            self.imported_symbols.insert(binding_name.clone());

            if import_decl.is_lazy {
                self.lazy_imports.insert(binding_name, path.clone());
            }
        } else {
            let symbol_name = path.last().unwrap();
            let binding_name = import_decl.alias.as_ref().unwrap_or(symbol_name).clone();

            if self.scopes.lookup(&binding_name).is_some() {
                self.errors.push(ResolveError::shadows_import(binding_name.clone(), span));
                return;
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
                        self.resolve_function(method);
                    }
                }
                DeclKind::Enum(enum_decl) => {
                    for method in &enum_decl.methods {
                        self.resolve_function(method);
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
            }
        }
    }

    fn resolve_function(&mut self, fn_decl: &FnDecl) {
        let base = Self::base_name(&fn_decl.name);
        let fn_sym = self.scopes.lookup(base);
        self.current_function = fn_sym;

        let scope_kind = if let Some(sym_id) = fn_sym {
            ScopeKind::Function(sym_id)
        } else {
            ScopeKind::Function(SymbolId(u32::MAX))
        };
        self.scopes.push(scope_kind);

        let mut param_syms = Vec::new();
        for param in &fn_decl.params {
            let param_sym = self.symbols.insert(
                param.name.clone(),
                SymbolKind::Parameter { is_take: param.is_take },
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

        for stmt in &fn_decl.body {
            self.resolve_stmt(stmt);
        }

        self.scopes.pop();
        self.current_function = None;
    }

    fn resolve_impl(&mut self, impl_decl: &ImplDecl) {
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
            StmtKind::Ensure { body, catch } => {
                self.scopes.push(ScopeKind::Block);
                for s in body {
                    self.resolve_stmt(s);
                }
                self.scopes.pop();
                if let Some((name, handler)) = catch {
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
            ExprKind::Char(_) | ExprKind::Bool(_) => {}
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
                    self.resolve_expr(arg);
                }
            }
            ExprKind::MethodCall { object, args, .. } => {
                self.resolve_expr(object);
                for arg in args {
                    self.resolve_expr(arg);
                }
            }
            ExprKind::Field { object, field } => {
                if let ExprKind::Ident(name) = &object.kind {
                    if self.imported_symbols.contains(name) {
                        let _ = field;
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
                    // Qualified struct variant: Enum.Variant { ... }
                    let parts: Vec<&str> = name.splitn(2, '.').collect();
                    if let Some(sym_id) = self.scopes.lookup(parts[0]) {
                        self.resolutions.insert(expr.id, sym_id);
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
    fn test_qualified_package_import() {
        let decls = vec![make_import_decl(vec!["http"], None, false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Qualified package import should succeed");
    }

    #[test]
    fn test_symbol_import() {
        let decls = vec![make_import_decl(vec!["http", "Request"], None, false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Symbol import should succeed");
    }

    #[test]
    fn test_aliased_import() {
        let decls = vec![make_import_decl(vec!["http"], Some("h"), false, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Aliased import should succeed");
    }

    #[test]
    fn test_glob_import() {
        let decls = vec![make_import_decl(vec!["http"], None, true, false)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Glob import should succeed (with warning)");
    }

    #[test]
    fn test_lazy_import() {
        let decls = vec![make_import_decl(vec!["http"], None, false, true)];
        let result = Resolver::resolve(&decls);
        assert!(result.is_ok(), "Lazy import should succeed");
    }

    fn make_fn_decl(name: &str) -> Decl {
        Decl {
            id: NodeId(0),
            kind: DeclKind::Fn(FnDecl {
                name: name.to_string(),
                type_params: vec![],
                params: vec![],
                ret_ty: None,
                body: vec![],
                is_pub: false,
                is_comptime: false,
                is_unsafe: false,
                attrs: vec![],
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
}
