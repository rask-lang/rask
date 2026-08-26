// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The name resolver implementation.

use std::collections::{HashMap, HashSet};
use rask_ast::decl::{Decl, DeclKind, FnDecl, StructDecl, EnumDecl, TraitDecl, ImplDecl, ImportDecl, ExportDecl, CImportDecl, TypeParam, UnionDecl};
use rask_ast::stmt::{ForBinding, Stmt, StmtKind};
use rask_ast::expr::{BinOp, Expr, ExprKind, Pattern, UnaryOp};
use rask_ast::{NodeId, Span};

use crate::error::ResolveError;
use crate::scope::{ScopeTree, ScopeKind};
use crate::symbol::{BuiltinModuleKind, SymbolTable, SymbolId, SymbolKind};
use crate::package::PackageId;
use crate::ResolvedProgram;

/// Levenshtein distance, for "did you mean" on a misspelled import.
fn edit_distance(a: &str, b: &str) -> usize {
    let b_len = b.chars().count();
    if a.is_empty() {
        return b_len;
    }
    if b.is_empty() {
        return a.chars().count();
    }
    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr = vec![0; b_len + 1];
    for (i, a_ch) in a.chars().enumerate() {
        curr[0] = i + 1;
        for (j, b_ch) in b.chars().enumerate() {
            let cost = if a_ch == b_ch { 0 } else { 1 };
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b_len]
}

pub struct Resolver {
    symbols: SymbolTable,
    scopes: ScopeTree,
    resolutions: HashMap<NodeId, SymbolId>,
    errors: Vec<ResolveError>,
    current_function: Option<SymbolId>,

    current_package: Option<PackageId>,
    package_bindings: HashMap<String, PackageId>,
    imported_symbols: HashSet<String>,
    /// The subset of `imported_symbols` an import brought in as a *type* — a
    /// module name, a companion type, an enum's own name, a selective type
    /// import.
    ///
    /// IM8 asks this rather than `imported_symbols`, which also holds enum
    /// variant names. A variant's name is `JsonError.ParseError`; a program
    /// declaring its own `ParseError` isn't shadowing anything a reader could
    /// confuse it with, and two suite files exist to assert exactly that a match
    /// resolves against the scrutinee's own enum. Both failed IM8 otherwise, and
    /// the message named the wrong module for it — `string` also has a
    /// `ParseError`, and that's the one the owner lookup found.
    imported_type_names: HashSet<String>,
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
            imported_type_names: HashSet::new(),
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
        use crate::symbol::BuiltinTypeKind;

        for entry in crate::symbol::BUILTIN_FUNCTIONS {
            let sym_id = self.symbols.insert(
                entry.name.to_string(),
                SymbolKind::BuiltinFunction { builtin: entry.kind },
                entry.ret_ty.map(String::from),
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(entry.name.to_string(), sym_id, Span::new(0, 0));
        }

        // BI1's set only. A builtin type that belongs to a module comes into
        // scope with the import, in `register_module_companions`.
        for entry in crate::symbol::BUILTIN_TYPES.iter().filter(|t| t.module.is_none()) {
            let sym_id = self.symbols.insert(
                entry.name.to_string(),
                SymbolKind::BuiltinType { builtin: entry.kind },
                None,
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(entry.name.to_string(), sym_id, Span::new(0, 0));
        }

        // Primitive types — always in scope so stdlib stubs and user code
        // can reference them in casts (`as u16`) and type annotations.
        let primitives = [
            "u8", "u16", "u32", "u64", "u128", "usize",
            "i8", "i16", "i32", "i64", "i128", "isize",
            "f32", "f64", "bool", "char",
        ];
        for name in primitives {
            let sym_id = self.symbols.insert(
                name.to_string(),
                SymbolKind::BuiltinType { builtin: BuiltinTypeKind::Primitive },
                None,
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(name.to_string(), sym_id, Span::new(0, 0));
        }

        // `StringBuilder` used to be registered here too, on the grounds that
        // it's the recommended idiom for building strings in loops
        // (canonical-patterns) and so "can't hide behind an import". No spec
        // says that, and IM1 doesn't make an exception for it — BI1's set is
        // closed. It needs `import string.StringBuilder` like any other stdlib
        // type, which is what `strings.Builder` costs in Go.

        self.register_builtin_enum("Option", &["Some", "None"]);
        self.register_builtin_enum("Result", &["Ok", "Err"]);
        // Comparison results and atomic memory orderings share one enum.
        self.register_builtin_enum("Ordering", rask_stdlib::ORDERING_VARIANTS);
        // Domain-specific enums (Method, JsonValue, JsonError, HttpError)
        // are registered when their module is imported — see resolve_import().

        // Stdlib modules, domain types, and domain enums are NOT registered
        // in the global scope — they require explicit `import` statements.
        // See resolve_import() for how they enter scope.

        // Top-level stdlib stub functions (e.g. async.rk's `spawn`,
        // `cancelled`, `join_all`, `select_first`) are auto-registered.
        // The pipeline sometimes runs the resolver without stdlib_decls
        // (single-file `rask check`), and these names are spec-required to
        // be in scope under their context (`spawn` under `using Multitasking`,
        // for instance — checked separately via context-clause analysis).
        // Skip names already claimed by hardcoded builtins above so println,
        // print, format, etc. keep their BuiltinFunction symbol kind.
        let stub_reg = rask_stdlib::StubRegistry::load();
        for f in stub_reg.functions() {
            if self.scopes.lookup(&f.name).is_some() {
                continue;
            }
            let ret_ty = if f.ret_ty.is_empty() { None } else { Some(f.ret_ty.clone()) };
            let sym_id = self.symbols.insert(
                f.name.clone(),
                SymbolKind::Function {
                    params: vec![],
                    ret_ty,
                    context_clauses: vec![],
                    is_unsafe: false,
                },
                None,
                Span::new(0, 0),
                true,
            );
            let _ = self.scopes.define(f.name.clone(), sym_id, Span::new(0, 0));
            // These come from the stubs, so a stdlib file importing one of them
            // (`import async.spawn` in http.rk) is replacing its own symbol, not
            // shadowing a user import. Without this, `rask test` — the one entry
            // point that resolves stdlib bodies — reported `spawn` shadowing an
            // import that doesn't exist (#507).
            self.stdlib_symbols.insert(sym_id);
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

    /// When a stdlib module is imported, register its companion types and enums
    /// into scope so users don't need separate imports for each type.
    fn register_module_companions(&mut self, module: BuiltinModuleKind, span: Span) {
        use crate::symbol::BuiltinTypeKind;

        use crate::symbol::BuiltinFunctionKind;

        // Only the program's own imports reserve a name. The stdlib's files are
        // resolved into the same scope, and io.rk imports `sync.Shared` for its
        // own use — recording that as the program's import satisfied IM1 for
        // every program, which is the shape of #780 and how `net.tcp_listen(…)`
        // came to work with no import while every other module needed one.
        let program_import = !self.stdlib_mode;

        // Companion functions that come into scope with the module
        let functions: &[(&str, BuiltinFunctionKind, Option<&str>)] = match module {
            BuiltinModuleKind::ASYNC => &[("spawn", BuiltinFunctionKind::Spawn, None)],
            BuiltinModuleKind::CORE => &[("transmute", BuiltinFunctionKind::Transmute, None)],
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

        // The module's own `.rk` file says what it exports, types and enum
        // variants alike — see `rask_stdlib::modules`. `stdlib_module_exports`
        // reads the same table, so the two can no longer disagree about what a
        // module has; this list used to be the shorter of the two (`os` was
        // missing `Stdio` and `Signal`, `io` was missing `IoError`).
        let exports = rask_stdlib::modules::exports(module.name());
        let types = &exports.types;
        let enums = &exports.enums;

        // The compiler-provided types this module owns — the box family for
        // `memory` and `sync`, `File`, `Random`, the SIMD vectors. Read off
        // `BUILTIN_TYPES`, which is also what BI3 asks, so "is this name mine to
        // declare" and "which import brings it in" can't answer differently.
        let builtin_types: Vec<(&'static str, BuiltinTypeKind)> =
            crate::symbol::module_builtin_types(module.name())
                .map(|t| (t.name, t.kind))
                .collect();

        // Structs with known public fields
        let struct_fields: &[(&str, &[&str])] = match module {
            BuiltinModuleKind::OS => &[
                ("Output", &["status", "stdout", "stderr"]),
            ],
            _ => &[],
        };

        // Register plain struct-like types
        for type_name in types {
            // IM8's other order: the program declared this name before writing
            // the import. Registering nothing left the program's own type in
            // scope and said nothing about it, which is how a user
            // `struct Timer` came to sit under stdlib/time.rk's — and natively
            // it then took the stdlib's zero-field layout and segfaulted (#975).
            let shadowed_by_local = self.declares_locally(type_name);

            // Record the name as imported whether or not something already
            // occupies it. `imported_symbols` is how the program says it asked
            // for this name, and the stdlib's own declarations are collected
            // before any import runs — so every module type was already in scope
            // by then, the `continue` below fired, and the record was never made.
            // `import time` then left `Duration` looking un-imported.
            if program_import {
                self.imported_symbols.insert(type_name.to_string());
                self.imported_type_names.insert(type_name.to_string());
            }

            if shadowed_by_local {
                self.errors.push(ResolveError::shadows_import(
                    type_name.to_string(),
                    Some(module.name().to_string()),
                    span,
                ));
                continue;
            }
            if builtin_types.iter().any(|(n, _)| n == type_name) {
                continue; // handled below as BuiltinType
            }
            if self.scopes.lookup(type_name).is_some() {
                continue;
            }
            let field_names = struct_fields.iter()
                .find(|(n, _)| *n == *type_name)
                .map(|(_, f)| *f)
                .unwrap_or(&[]);
            let fields: Vec<(String, crate::symbol::SymbolId)> = field_names.iter()
                .map(|f| {
                    let field_sym = self.symbols.insert(
                        f.to_string(),
                        SymbolKind::Variable { mutable: false },
                        None,
                        span,
                        false,
                    );
                    (f.to_string(), field_sym)
                })
                .collect();
            let sym_id = self.symbols.insert(
                type_name.to_string(),
                SymbolKind::Struct { fields },
                None,
                span,
                false,
            );
            let _ = self.scopes.define(type_name.to_string(), sym_id, span);
        }

        // The module's compiler-provided types — the box family, File, Random,
        // the SIMD vectors.
        for (name, kind) in &builtin_types {
            let shadowed_by_local = self.declares_locally(name);
            if program_import {
                self.imported_symbols.insert((*name).to_string());
                self.imported_type_names.insert((*name).to_string());
            }
            if shadowed_by_local {
                self.errors.push(ResolveError::shadows_import(
                    (*name).to_string(),
                    Some(module.name().to_string()),
                    span,
                ));
                continue;
            }
            if self.scopes.lookup(name).is_some() {
                continue;
            }
            let sym_id = self.symbols.insert(
                (*name).to_string(),
                SymbolKind::BuiltinType { builtin: *kind },
                None,
                span,
                false,
            );
            let _ = self.scopes.define((*name).to_string(), sym_id, span);
        }

        // Register enums
        for (enum_name, variants) in enums {
            let shadowed_by_local = self.declares_locally(enum_name);
            if program_import {
                self.imported_symbols.insert(enum_name.clone());
                self.imported_type_names.insert(enum_name.clone());
                for v in variants {
                    self.imported_symbols.insert(v.clone());
                }
            }
            if shadowed_by_local {
                self.errors.push(ResolveError::shadows_import(
                    enum_name.clone(),
                    Some(module.name().to_string()),
                    span,
                ));
                continue;
            }
            if self.scopes.lookup(enum_name).is_some() {
                continue;
            }
            let variant_refs: Vec<&str> = variants.iter().map(String::as_str).collect();
            self.register_builtin_enum(enum_name, &variant_refs);
        }
    }

    /// Everything a stdlib module offers under a selective import — its types,
    /// its enums, and any function that comes into scope with it. Module
    /// functions (`fs.read_text`) are added separately from the stdlib registry.
    ///
    /// Read off the module's own `.rk` file by `rask_stdlib::modules`, which is
    /// also what `register_module_companions` registers from. They have to agree,
    /// and now they can't disagree.
    fn stdlib_module_exports(module: &str) -> Vec<&'static str> {
        rask_stdlib::modules::exports(module).names().collect()
    }

    /// Closest export by name, for the "did you mean" on a bad import. Only
    /// offers a suggestion when the names are actually close — a wrong guess
    /// is worse than none.
    fn nearest_export(module: &str, symbol: &str) -> Option<String> {
        let lower = symbol.to_lowercase();
        let mut best: Option<(usize, &str)> = None;
        let stub_fns: Vec<&str> = rask_stdlib::StubRegistry::load()
            .methods(module)
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        let exports = Self::stdlib_module_exports(module);
        let candidates = exports.iter().copied().chain(stub_fns.iter().copied());
        for cand in candidates {
            // A renamed symbol often keeps its start (`Rng` → `Random`), which
            // edit distance alone scores badly on short names.
            let shares_start = {
                let c = cand.to_lowercase();
                lower.len() >= 2 && (c.starts_with(&lower) || lower.starts_with(&c))
            };
            let d = if shares_start { 0 } else { edit_distance(symbol, cand) };
            if best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, cand));
            }
        }
        best.filter(|(d, _)| *d <= symbol.chars().count() / 2 + 1)
            .map(|(_, name)| name.to_string())
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
        use crate::symbol::BuiltinFunctionKind;

        // Builtin functions
        match (module, symbol) {
            ("async", "spawn") => return SymbolKind::BuiltinFunction { builtin: BuiltinFunctionKind::Spawn },
            ("core", "transmute") => return SymbolKind::BuiltinFunction { builtin: BuiltinFunctionKind::Transmute },
            _ => {}
        }

        if let Some(kind) = crate::symbol::module_builtin_type(module, symbol) {
            return SymbolKind::BuiltinType { builtin: kind };
        }

        // Is it a type at all? This was a hardcoded list of five modules —
        // net, http, path, fs, cli — and a type in any of the other twenty-odd
        // fell through to the variable binding below. `import time.Duration as
        // Span` bound `Span` as a *variable*, which is why the alias had no
        // methods and still passed in a type position (#923). The module's own
        // `.rk` file answers it.
        if rask_stdlib::modules::exports_type(module, symbol) {
            return SymbolKind::Struct { fields: vec![] };
        }

        // Fallback — treat as a variable binding
        SymbolKind::Variable { mutable: false }
    }

    /// The spec's primitive set — no `string`, no `int`/`uint` aliases.
    fn is_primitive_type(name: &str) -> bool {
        rask_ast::primitives::is_scalar(name)
    }

    /// BI3/BF3: is `name` reserved, so a program's own declaration of it is an
    /// error?
    ///
    /// `is_reserved_name` answers by name — BI1's types, BF1's functions, the
    /// prelude enums — instead of asking what the scope currently holds. That's
    /// the fix for BI3 leaking on exactly the names the stdlib also declares:
    /// the stdlib's own `public struct Vec<T> { }` replaced the builtin binding,
    /// and the `stdlib_symbols` bail-out below then said `Vec` wasn't a builtin.
    /// So `struct Vec { … }` compiled while `struct Set { … }` was refused, the
    /// difference being only which names collections.rk happens to declare
    /// (#977). Same for `Map`, `Pool`, `Handle`, `Rack`, `Link`, `Mutex`,
    /// `Option` and `Result`.
    ///
    /// The scope lookup that follows covers the two kinds of name that aren't in
    /// a table: an imported module, and a module's own enum registered by
    /// `register_module_companions`.
    ///
    /// This used to be two functions, `is_builtin_name` for a struct and
    /// `is_builtin_type_name` for a function, differing in whether a builtin
    /// *function*'s name was reserved. BF1's `reserved` flag is that distinction
    /// now, and it's the accurate version: a program may declare `max`, which
    /// isn't in BF1, and may not declare `println`, which is — whether the
    /// declaration is a struct or a function.
    fn is_reserved_name(&self, name: &str) -> bool {
        if crate::symbol::is_always_in_scope(name) {
            return true;
        }
        if let Some(sym_id) = self.scopes.lookup(name) {
            // A module the *stdlib* imported for its own use isn't reserved for
            // the program. `stdlib/http.rk` imports `net`, and that binding
            // lands in the shared scope — so without this, `let net = …` was
            // rejected while `let fs = …` was fine, for no reason a reader
            // could see (#780). A name needs its own import here, which is
            // IM1's rule and what E0210 reports.
            if self.stdlib_symbols.contains(&sym_id) {
                return false;
            }
            if let Some(sym) = self.symbols.get(sym_id) {
                return matches!(sym.kind, SymbolKind::BuiltinModule { .. })
                    || (matches!(sym.kind, SymbolKind::Enum { .. })
                        && sym.span == Span::new(0, 0));
            }
        }
        false
    }

    fn resolve_inner(decls: &[Decl], stdlib_mode: bool) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();
        resolver.stdlib_mode = stdlib_mode;

        resolver.collect_declarations(decls);
        resolver.check_annotations(decls);
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
        resolver.check_annotations(decls);
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

    /// Resolve the program with stdlib bodies alongside it.
    ///
    /// The single-file mirror of `resolve_package_with_stdlib_and_cfg`. Needed
    /// because the stdlib's own bodies are compiled into every program, so they
    /// have to be resolved — and then type-checked — for anything downstream to
    /// know what a call inside them refers to.
    ///
    /// Stdlib decls go in under `stdlib_mode`: they *define* `Result`, `Option`
    /// and `spawn`, so the builtin-shadowing check (E0209) would reject the
    /// definitions of the very builtins it's protecting.
    pub fn resolve_with_stdlib_and_cfg(
        decls: &[Decl],
        stdlib_decls: &[Decl],
        cfg_values: HashMap<String, String>,
    ) -> Result<ResolvedProgram, Vec<ResolveError>> {
        let mut resolver = Resolver::new();
        resolver.cfg_values = cfg_values;

        if !stdlib_decls.is_empty() {
            resolver.stdlib_mode = true;
            resolver.collect_declarations(stdlib_decls);
            resolver.stdlib_mode = false;
        }
        resolver.collect_declarations(decls);
        resolver.check_annotations(decls);

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
                external_decls: HashMap::new(),
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
                        // A reader in another package needs the declaration,
                        // not just the attachment: `has<A>()` matches the
                        // attachment text and works without it, but
                        // `get<A>().max` has to know what `max` is declared as
                        // (type.annotations/AN6).
                        DeclKind::Annotation(a) => a.is_pub,
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
        resolver.check_annotations(decls);

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
                    self.check_shadows_import(&const_decl.name, decl.span);
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
                    self.check_shadows_import(&alias.name, decl.span);
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
                // Annotations register as struct symbols so the name resolves
                // — `has<A>()` names it as a type argument, and a runtime
                // construction attempt reaches the checker's tailored
                // rejection instead of dying here as "undefined symbol".
                // Attachment validation (AN2-AN5) reads the AST directly.
                DeclKind::Annotation(ann) => {
                    let sym_id = self.symbols.insert(
                        ann.name.clone(),
                        SymbolKind::Struct { fields: vec![] },
                        None,
                        decl.span,
                        ann.is_pub,
                    );
                    if let Err(e) = self.scopes.define(ann.name.clone(), sym_id, decl.span) {
                        self.errors.push(e);
                    }
                }
                DeclKind::CImport(c_import) => {
                    self.resolve_c_import(c_import, decl.span);
                }
                DeclKind::Union(union_decl) => {
                    self.declare_union(union_decl, decl.span);
                }
                DeclKind::Extern(extern_decl) => {
                    let param_types: Vec<String> = extern_decl.params.iter()
                        .map(|p| p.ty.clone())
                        .collect();

                    // One C symbol, one signature. Ordinary bindings shadow, so
                    // a second `extern "C" func strlen` just replaced the first
                    // — and when the first was the stdlib's, std.fs's own calls
                    // started type-checking against the user's return type. The
                    // errors surfaced with fs.rk's offsets against the user's
                    // file, pointing at a line and column that didn't exist
                    // there. Agreeing redeclarations stay legal: a program
                    // shouldn't have to know std.fs already declared `strlen`.
                    if let Some(prev_id) = self.scopes.lookup(&extern_decl.name) {
                        if let Some(prev) = self.symbols.get(prev_id) {
                            if let SymbolKind::ExternFunction { abi, params, ret_ty } = &prev.kind {
                                let same = *abi == extern_decl.abi
                                    && *params == param_types
                                    && *ret_ty == extern_decl.ret_ty;
                                if !same {
                                    self.errors.push(ResolveError::conflicting_extern(
                                        extern_decl.name.clone(),
                                        render_extern_sig(abi, params, ret_ty),
                                        render_extern_sig(
                                            &extern_decl.abi,
                                            &param_types,
                                            &extern_decl.ret_ty,
                                        ),
                                        prev.span,
                                        decl.span,
                                    ));
                                }
                                // Either way the existing binding stands, so the
                                // declaration that's already in use keeps working.
                                continue;
                            }
                        }
                    }

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

    /// IM8: a declaration may not take a name an import already bound.
    ///
    /// Only the import-then-declaration order reaches here. The other order —
    /// declaration first, import after — is caught where the import binds its
    /// name, which has reported it for years; this is the half that was missing,
    /// and it's the half that mattered, because it's the order people write.
    ///
    /// `imported_symbols` holds what the *program* imported, so the stdlib's own
    /// `import net` inside http.rk doesn't reserve `net` against a program
    /// (#780).
    fn check_shadows_import(&mut self, name: &str, span: Span) {
        if self.stdlib_mode || !self.imported_type_names.contains(name) {
            return;
        }
        self.errors.push(ResolveError::shadows_import(
            name.to_string(),
            Self::owning_module(name),
            span,
        ));
    }

    /// The stdlib module that exports `name`, for the message. `None` when the
    /// name came from a package rather than the stdlib.
    fn owning_module(name: &str) -> Option<String> {
        rask_stdlib::modules::module_names()
            .iter()
            .find(|m| rask_stdlib::modules::exports(m).exports(name))
            .map(|m| m.to_string())
    }

    /// Is `name` bound to a type the *program* declared — as opposed to a
    /// builtin, something the stdlib declared, or an earlier import?
    fn declares_locally(&self, name: &str) -> bool {
        if self.stdlib_mode || self.imported_symbols.contains(name) {
            return false;
        }
        let Some(sym_id) = self.scopes.lookup(name) else { return false };
        if self.stdlib_symbols.contains(&sym_id) {
            return false;
        }
        self.symbols.get(sym_id).is_some_and(|sym| {
            matches!(
                sym.kind,
                SymbolKind::Struct { .. }
                    | SymbolKind::Enum { .. }
                    | SymbolKind::Trait { .. }
                    | SymbolKind::TypeAlias { .. }
            ) && sym.span != Span::new(0, 0)
        })
    }

    fn declare_function(&mut self, fn_decl: &FnDecl, span: Span, is_pub: bool) -> SymbolId {
        let base = Self::base_name(&fn_decl.name).to_string();
        if !self.stdlib_mode && self.is_reserved_name(&base) {
            self.errors.push(ResolveError::shadows_builtin(base.clone(), span));
        }
        self.check_shadows_import(&base, span);

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
        if !self.stdlib_mode && self.is_reserved_name(&base) {
            self.errors.push(ResolveError::shadows_builtin(base.clone(), span));
        }
        self.check_shadows_import(&base, span);

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
        let union_base = Self::base_name(&union_decl.name).to_string();
        if !self.stdlib_mode && self.is_reserved_name(&union_base) {
            self.errors.push(ResolveError::shadows_builtin(union_base.clone(), span));
        }
        self.check_shadows_import(&union_base, span);

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
        if !self.stdlib_mode && self.is_reserved_name(&base) {
            self.errors.push(ResolveError::shadows_builtin(base.clone(), span));
        }
        self.check_shadows_import(&base, span);

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
            // Don't register user-defined variants in the enclosing scope.
            // Access via qualified syntax: Enum.Variant. Only builtin
            // variants (Ok, Err, Some, None) are registered at top level
            // by register_builtin_enum.
            variant_syms.push((variant.name.clone(), variant_sym));
        }

        if let Some(sym) = self.symbols.get_mut(sym_id) {
            sym.kind = SymbolKind::Enum { variants: variant_syms };
        }
    }

    fn declare_trait(&mut self, trait_decl: &TraitDecl, span: Span) {
        if !self.stdlib_mode && self.is_reserved_name(&trait_decl.name) {
            self.errors.push(ResolveError::shadows_builtin(trait_decl.name.clone(), span));
        }
        let trait_base = Self::base_name(&trait_decl.name).to_string();
        self.check_shadows_import(&trait_base, span);

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

            let stdlib_module = BuiltinModuleKind::from_name(pkg_name.as_str());

            if let Some(module_kind) = stdlib_module {
                let sym_id = self.symbols.insert(
                    binding_name.clone(),
                    SymbolKind::BuiltinModule { module: module_kind },
                    None,
                    span,
                    false,
                );
                // Stdlib decls share one scope with user code, so an import
                // *inside* the stdlib landed in the user's scope too — and
                // `stdlib/http.rk` imports `net` and `json`. That's why those
                // two, alone among the modules, worked with no import and were
                // reserved words a program couldn't name a local after (#780).
                // Marking the binding stdlib-owned puts them back under IM1:
                // `stdlib_module_needs_import` reports the missing import, and
                // `is_builtin_name` lets a user bind the name.
                if self.stdlib_mode {
                    self.stdlib_symbols.insert(sym_id);
                }
                if let Err(e) = self.scopes.define(binding_name.clone(), sym_id, span) {
                    self.errors.push(e);
                }
                // Stdlib modules always register companion types/enums into scope.
                // Module functions are accessed qualified (os.env), but types
                // (Command, File, Signal) are used unqualified per convention.
                self.register_module_companions(module_kind, span);
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

            // Not in stdlib_mode: `imported_symbols` records what the *program*
            // imported, and `stdlib/http.rk`'s own `import net` is not that.
            // Recording it satisfied the import requirement for every user
            // program, so `net.tcp_listen(…)` compiled with no import while
            // every other module needed one (#780).
            if !self.stdlib_mode {
                self.imported_symbols.insert(binding_name.clone());
                self.imported_type_names.insert(binding_name.clone());
            }

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
                    // A variant of a compiler-registered enum counts too. The
                    // atomic orderings are variants of `Ordering`, which the
                    // resolver puts in scope itself, so `import sync.Relaxed`
                    // met a name that was already there and was reported as
                    // shadowing an import that doesn't exist. Span (0,0) is how
                    // the rest of the resolver tells a registered builtin from
                    // a declaration with real source behind it.
                    || (matches!(
                            sym.kind,
                            SymbolKind::Enum { .. } | SymbolKind::EnumVariant { .. }
                        ) && sym.span == Span::new(0, 0))
                });
                let is_stdlib = self.stdlib_symbols.contains(&existing_id);
                let is_imported = self.imported_symbols.contains(&binding_name);
                // The stdlib's files are collected into one flat scope, so
                // `import async.spawn` in http.rk meets async.rk's own `spawn`
                // as if they were the same file. That's an artifact of the
                // flattening, not the ambiguity this error is about — and the
                // order depends on which file happens to be listed first.
                if !is_builtin && !is_stdlib && !is_imported && !self.stdlib_mode {
                    self.errors.push(ResolveError::shadows_import(
                        binding_name.clone(),
                        Self::owning_module(&binding_name),
                        span,
                    ));
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
            // Same question as the bare `import pkg` above, so the same answer.
            // This used to be its own `matches!` over module names, and the two
            // disagreed: `num` was here but not in the enum, `memory` and `sync`
            // in neither — so `import sync.Shared` fell through to the
            // unknown-package branch and bound `Shared` as a plain variable
            // (#977).
            if crate::is_builtin_module(pkg_name) {
                // A name the module doesn't have used to sail through and fail
                // much later — `import random.Rng` (renamed to `Random`) got a
                // plain variable binding, type-checked clean, and only broke at
                // codegen with "Function not found: Rng_from_seed" (#395).
                // The function set comes from the stub sources themselves, not
                // from a hand-maintained list — `io.stdin` is declared in
                // stdlib/io.rk but missing from the registry's IO_METHODS, and
                // a check stricter than the actual API is worse than no check.
                let known = Self::stdlib_module_exports(pkg_name).contains(&symbol_name.as_str())
                    || rask_stdlib::mir_metadata::type_has_method(pkg_name, symbol_name)
                    || Self::stdlib_enum_variants(pkg_name, symbol_name).is_some()
                    // `import std.reflect` names a submodule, not a symbol.
                    || rask_stdlib::mir_metadata::stdlib_module_names()
                        .contains(symbol_name.as_str())
                    || crate::is_builtin_module(symbol_name);
                if !known && !self.stdlib_mode {
                    self.errors.push(ResolveError::no_such_stdlib_export(
                        pkg_name.clone(),
                        symbol_name.clone(),
                        Self::nearest_export(pkg_name, symbol_name),
                        span,
                    ));
                    return;
                }
                // IM2: `import std.io` names a module, not a symbol inside one, so
                // it has to bind the namespace exactly like a bare `import io`.
                // Without this the name kept whatever the stdlib had already put
                // in scope and the module's companion types never registered — the
                // program then compiled natively and died on the interpreter.
                if let Some(module_kind) = BuiltinModuleKind::from_name(symbol_name.as_str()) {
                    let sym_id = self.symbols.insert(
                        binding_name.clone(),
                        SymbolKind::BuiltinModule { module: module_kind },
                        None,
                        span,
                        false,
                    );
                    if self.stdlib_mode {
                        self.stdlib_symbols.insert(sym_id);
                    }
                    if let Err(e) = self.scopes.define(binding_name.clone(), sym_id, span) {
                        self.errors.push(e);
                    }
                    self.register_module_companions(module_kind, span);
                } else if self.scopes.lookup(&binding_name).is_none() {
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
                        // A binding the *stdlib* made for its own use is
                        // stdlib-owned, and IM1 only asks about those. Without
                        // this, io.rk's `import sync.Shared` left a plain symbol
                        // in the shared scope that looked like a program
                        // declaration, so `Shared` needed no import in any
                        // program — the same leak as #780, one layer down.
                        if self.stdlib_mode {
                            self.stdlib_symbols.insert(sym_id);
                        }
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

            // Not in stdlib_mode: `imported_symbols` records what the *program*
            // imported, and `stdlib/http.rk`'s own `import net` is not that.
            // Recording it satisfied the import requirement for every user
            // program, so `net.tcp_listen(…)` compiled with no import while
            // every other module needed one (#780).
            if !self.stdlib_mode {
                self.imported_symbols.insert(binding_name.clone());
                // A type or a module namespace is a name IM8 protects. A
                // function (`import async.spawn`) or an enum variant is not.
                let is_type = rask_stdlib::modules::exports_type(pkg_name, symbol_name)
                    || crate::symbol::module_builtin_type(pkg_name, symbol_name).is_some()
                    || crate::is_builtin_module(symbol_name);
                if is_type {
                    self.imported_type_names.insert(binding_name.clone());
                }
            }

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
    // C Import Resolution (CI1)
    // =========================================================================

    fn resolve_c_import(&mut self, c_import: &CImportDecl, span: Span) {
        use rask_c_parse::translate;

        let mut all_decls = Vec::new();

        for header_path in &c_import.headers {
            let source = match self.read_c_header(header_path) {
                Ok(s) => s,
                Err(msg) => {
                    self.errors.push(ResolveError::c_header_not_found(
                        header_path.clone(), msg, span,
                    ));
                    continue;
                }
            };

            match rask_c_parse::parse_c_header(&source) {
                Ok(result) => {
                    for w in &result.warnings {
                        eprintln!("warning: {}: {}", header_path, w.message);
                    }
                    let translated = translate::translate(&result, &c_import.hiding);
                    for w in &translated.warnings {
                        eprintln!("warning: c-import: {}", w);
                    }
                    all_decls.extend(translated.decls);
                }
                Err(e) => {
                    self.errors.push(ResolveError::c_parse_error(
                        header_path.clone(),
                        e.to_string(),
                        span,
                    ));
                }
            }
        }

        // Create symbols for each translated declaration
        let mut members = HashMap::new();

        for decl in &all_decls {
            match decl {
                translate::RaskCDecl::Function(f) => {
                    let param_types: Vec<String> = f.params.iter()
                        .map(|p| p.ty.clone())
                        .collect();
                    let sym_id = self.symbols.insert(
                        f.name.clone(),
                        SymbolKind::ExternFunction {
                            abi: "C".to_string(),
                            params: param_types,
                            ret_ty: if f.ret_ty.is_empty() { None } else { Some(f.ret_ty.clone()) },
                        },
                        None,
                        span,
                        false,
                    );
                    members.insert(f.name.clone(), sym_id);
                }
                translate::RaskCDecl::Struct(s) => {
                    if s.is_opaque {
                        let sym_id = self.symbols.insert(
                            s.name.clone(),
                            SymbolKind::Struct { fields: vec![] },
                            None,
                            span,
                            false,
                        );
                        members.insert(s.name.clone(), sym_id);
                    } else {
                        let mut field_syms = Vec::new();
                        let struct_sym_id = self.symbols.insert(
                            s.name.clone(),
                            SymbolKind::Struct { fields: vec![] },
                            None,
                            span,
                            false,
                        );
                        for f in &s.fields {
                            let f_sym = self.symbols.insert(
                                f.name.clone(),
                                SymbolKind::Field { parent: struct_sym_id },
                                Some(f.ty.clone()),
                                span,
                                false,
                            );
                            field_syms.push((f.name.clone(), f_sym));
                        }
                        if let Some(sym) = self.symbols.get_mut(struct_sym_id) {
                            sym.kind = SymbolKind::Struct { fields: field_syms };
                        }
                        members.insert(s.name.clone(), struct_sym_id);
                    }
                }
                translate::RaskCDecl::Union(s) => {
                    let mut field_syms = Vec::new();
                    let union_sym_id = self.symbols.insert(
                        s.name.clone(),
                        SymbolKind::Struct { fields: vec![] },
                        None,
                        span,
                        false,
                    );
                    for f in &s.fields {
                        let f_sym = self.symbols.insert(
                            f.name.clone(),
                            SymbolKind::Field { parent: union_sym_id },
                            Some(f.ty.clone()),
                            span,
                            false,
                        );
                        field_syms.push((f.name.clone(), f_sym));
                    }
                    if let Some(sym) = self.symbols.get_mut(union_sym_id) {
                        sym.kind = SymbolKind::Struct { fields: field_syms };
                    }
                    members.insert(s.name.clone(), union_sym_id);
                }
                translate::RaskCDecl::Enum(e) => {
                    let enum_sym_id = self.symbols.insert(
                        e.name.clone(),
                        SymbolKind::Enum { variants: vec![] },
                        None,
                        span,
                        false,
                    );
                    let mut variant_syms = Vec::new();
                    for (vname, _value) in &e.variants {
                        let v_sym = self.symbols.insert(
                            vname.clone(),
                            SymbolKind::EnumVariant { enum_id: enum_sym_id },
                            None,
                            span,
                            false,
                        );
                        variant_syms.push((vname.clone(), v_sym));
                        // C enum constants are also accessible as top-level names
                        members.insert(vname.clone(), v_sym);
                    }
                    if let Some(sym) = self.symbols.get_mut(enum_sym_id) {
                        sym.kind = SymbolKind::Enum { variants: variant_syms };
                    }
                    members.insert(e.name.clone(), enum_sym_id);
                }
                translate::RaskCDecl::Const(c) => {
                    let sym_id = self.symbols.insert(
                        c.name.clone(),
                        SymbolKind::Variable { mutable: false },
                        Some(c.ty.clone()),
                        span,
                        false,
                    );
                    members.insert(c.name.clone(), sym_id);
                }
                translate::RaskCDecl::TypeAlias(a) => {
                    let sym_id = self.symbols.insert(
                        a.name.clone(),
                        SymbolKind::TypeAlias { target: a.target.clone() },
                        None,
                        span,
                        false,
                    );
                    members.insert(a.name.clone(), sym_id);
                }
            }
        }

        // Register the namespace under the alias
        let ns_sym = self.symbols.insert(
            c_import.alias.clone(),
            SymbolKind::CNamespace { members },
            None,
            span,
            false,
        );
        if let Err(e) = self.scopes.define(c_import.alias.clone(), ns_sym, span) {
            self.errors.push(e);
        }
    }

    /// Read a C header file, searching standard include paths.
    fn read_c_header(&self, path: &str) -> Result<String, String> {
        // Try relative to current directory first
        if let Ok(contents) = std::fs::read_to_string(path) {
            return Ok(contents);
        }

        // Search standard include paths
        let search_paths = [
            "/usr/include",
            "/usr/local/include",
            "/usr/include/x86_64-linux-gnu",
            "/usr/include/aarch64-linux-gnu",
        ];

        for base in &search_paths {
            let full = format!("{}/{}", base, path);
            if let Ok(contents) = std::fs::read_to_string(&full) {
                return Ok(contents);
            }
        }

        Err(format!("header not found in search paths: {}", path))
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
                DeclKind::Package(_) | DeclKind::CImport(_) => {}
                DeclKind::Union(_) => {}
                DeclKind::TypeAlias(_) => {}
                DeclKind::Annotation(ann) => {
                    // Field defaults are expressions; resolve so they can name consts.
                    for field in &ann.fields {
                        if let Some(default) = &field.default {
                            self.resolve_expr(default);
                        }
                    }
                }
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
                    is_deleting: param.is_deleting,
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
            StmtKind::Mut { name, name_span, ty, init } => {
                self.resolve_expr(init);
                if !self.stdlib_mode && self.is_reserved_name(name) {
                    self.errors.push(ResolveError::shadows_builtin(name.clone(), *name_span));
                }
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
            StmtKind::Let { name, name_span, ty, init } => {
                self.resolve_expr(init);
                if !self.stdlib_mode && self.is_reserved_name(name) {
                    self.errors.push(ResolveError::shadows_builtin(name.clone(), *name_span));
                }
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
            StmtKind::MutTuple { patterns, init } => {
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
            StmtKind::LetTuple { patterns, init } => {
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
            StmtKind::LetStruct { pattern, init, is_mut } => {
                self.resolve_expr(init);
                for name in rask_ast::stmt::pattern_binding_names(pattern) {
                    let sym_id = self.symbols.insert(
                        name.clone(),
                        SymbolKind::Variable { mutable: *is_mut },
                        None,
                        stmt.span,
                        false,
                    );
                    if let Err(e) = self.scopes.define(name, sym_id, stmt.span) {
                        self.errors.push(e);
                    }
                }
            }
            StmtKind::Assign { target, value, .. } => {
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
                match value {
                    // `break x` reads either way, and the parser picks the value
                    // reading when no enclosing loop is labelled `x`. If `x`
                    // isn't a variable either, say which two things it failed to
                    // be — "undefined symbol, add an import" sends a misspelled
                    // label off in the wrong direction entirely.
                    Some(v) => match &v.kind {
                        ExprKind::Ident(name) if self.scopes.lookup(name).is_none() => {
                            self.errors.push(ResolveError::unknown_break_target(
                                name.clone(),
                                self.scopes.labels_in_scope(),
                                v.span,
                            ));
                        }
                        _ => self.resolve_expr(v),
                    },
                    None => {}
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
            StmtKind::While { label, cond, body } => {
                self.resolve_expr(cond);
                self.scopes.push(ScopeKind::Loop { label: label.clone() });
                // OPT19 on a loop: `while expr? as v` binds the payload for the
                // body, re-read each iteration. Same binder the `If` arm defines,
                // scoped to the loop instead of a then-branch.
                if let ExprKind::IsPresent { binding: Some(name), .. } = &cond.kind {
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
            StmtKind::WhileLet { label, pattern, expr, body } => {
                self.resolve_expr(expr);
                self.scopes.push(ScopeKind::Loop { label: label.clone() });
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
            StmtKind::ComptimeFor { binding, iter, body } => {
                self.resolve_expr(iter);
                self.scopes.push(ScopeKind::Block);
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
            ExprKind::If { cond, then_branch, else_branch, .. } => (cond, then_branch, else_branch),
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

    /// IM1: the module `name` would have to be imported from, or `None` when the
    /// program may use it as it stands.
    ///
    /// A stdlib name is in scope where the program asked for it and nowhere
    /// else. The stdlib's own source is resolved alongside the program into one
    /// scope, so every module (`struct math { }`) and every type it declares
    /// landed in the program's namespace whether it imported them or not:
    /// `math.sin(x)` compiled and ran natively while the interpreter, which
    /// binds a module only when it sees the import, rejected the same program at
    /// runtime (#723). This started as the module half of that. The types were
    /// left out on purpose — "only module names, so `Vec` and the others that are
    /// genuinely always in scope are untouched" — which let all 65 of the
    /// stdlib's public types resolve bare, `Duration.seconds(1)` with no import
    /// anywhere (#977).
    ///
    /// Three sources of owned names, because no one of them has all of them:
    ///
    /// - module names, from the stub sources;
    /// - types the stdlib declares in a `.rk` file, from the stub registry;
    /// - types the compiler provides on a module's behalf with no declaration
    ///   anywhere — `Heap`, the atomics, the SIMD vectors. Those aren't in the
    ///   registry's type names, so asking the stubs alone reported them as an
    ///   undeclared type instead of a missing import.
    ///
    /// A name the program declares itself is its own, imported or not, and
    /// `is_always_in_scope` is the exception BI1 names.
    fn needs_import_from(&self, name: &str) -> Option<String> {
        if self.stdlib_mode
            || self.imported_symbols.contains(name)
            || crate::symbol::is_always_in_scope(name)
        {
            return None;
        }
        // Bound to something the program declared → not the stdlib's name here.
        if let Some(sym_id) = self.scopes.lookup(name) {
            if !self.stdlib_symbols.contains(&sym_id) {
                return None;
            }
        }
        let owned = rask_stdlib::mir_metadata::stdlib_module_names().contains(name)
            || rask_stdlib::mir_metadata::stdlib_type_names().contains(name)
            || crate::symbol::BUILTIN_TYPES
                .iter()
                .any(|t| t.name == name && t.module.is_some());
        if !owned {
            return None;
        }
        // A module imports as itself; a type imports out of the module that
        // exports it.
        Some(Self::owning_module(name).unwrap_or_else(|| name.to_string()))
    }

    /// The identifier-shaped pieces of a type string. `Vec<Duration>?` gives
    /// `Vec` and `Duration`, `*u8` gives `u8`, `T or Error` gives `T`, `or` and
    /// `Error`.
    ///
    /// Deliberately not a parser. The only question asked of these is whether
    /// one is a stdlib type name, so splitting on everything that can't be part
    /// of an identifier is enough — and it can't be wrong about a type spelling
    /// it hasn't been taught, which a parser would be.
    fn type_idents(ty: &str) -> impl Iterator<Item = &str> {
        ty.split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
    }

    /// IM1 for a type annotation.
    ///
    /// Annotations are strings by the time the resolver sees them — `d: Duration`
    /// on a field, `-> Instant` on a signature — and nothing in this pass looked
    /// at them. So however well IM1 was enforced in expression position, every
    /// stdlib type still reached a program through its annotations: `struct S { d:
    /// Duration }` compiled with no import (#977).
    fn check_type_annotation(&mut self, ty: &str, span: Span) {
        if self.stdlib_mode {
            return;
        }
        let flagged: Vec<(String, String)> = Self::type_idents(ty)
            .filter_map(|name| {
                self.needs_import_from(name).map(|m| (name.to_string(), m))
            })
            .collect();
        for (name, module) in flagged {
            self.errors.push(ResolveError::module_not_imported(name, Some(module), span));
        }
    }

    /// IM1 over every type annotation in the program.
    ///
    /// A pass of its own, run once `collect_declarations` has seen all the
    /// imports. Checking a field where it's declared would make the answer depend
    /// on whether the `import` line happened to come first in the file.
    fn check_annotations(&mut self, decls: &[Decl]) {
        for decl in decls {
            match &decl.kind {
                DeclKind::Struct(s) => {
                    for f in &s.fields {
                        self.check_type_annotation(&f.ty, decl.span);
                    }
                }
                DeclKind::Union(u) => {
                    for f in &u.fields {
                        self.check_type_annotation(&f.ty, decl.span);
                    }
                }
                DeclKind::Enum(e) => {
                    for v in &e.variants {
                        for ty in &v.fields {
                            self.check_type_annotation(&ty.ty, decl.span);
                        }
                    }
                }
                DeclKind::Fn(f) => self.check_fn_annotations(f, decl.span),
                DeclKind::Trait(t) => {
                    for m in &t.methods {
                        self.check_fn_annotations(m, decl.span);
                    }
                }
                DeclKind::Impl(i) => {
                    self.check_type_annotation(&i.target_ty, decl.span);
                    for m in &i.methods {
                        self.check_fn_annotations(m, decl.span);
                    }
                }
                DeclKind::Const(c) => {
                    if let Some(ty) = &c.ty {
                        self.check_type_annotation(ty, decl.span);
                    }
                }
                DeclKind::TypeAlias(a) => self.check_type_annotation(&a.target, decl.span),
                _ => {}
            }
        }
    }

    fn check_fn_annotations(&mut self, fn_decl: &FnDecl, span: Span) {
        for p in &fn_decl.params {
            self.check_type_annotation(&p.ty, span);
        }
        if let Some(ret) = &fn_decl.ret_ty {
            self.check_type_annotation(ret, span);
        }
    }

    fn resolve_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::Int(_, _) | ExprKind::Float(_, _) | ExprKind::String(_) |
            ExprKind::StringInterp(_) | ExprKind::Char(_) | ExprKind::Bool(_) | ExprKind::Null | ExprKind::None => {}
            ExprKind::Ident(name) => {
                match self.scopes.lookup(name) {
                    Some(sym_id) => {
                        if let Some(module) = self.needs_import_from(name) {
                            self.errors.push(ResolveError::module_not_imported(
                                name.clone(), Some(module), expr.span,
                            ));
                        }
                        self.resolutions.insert(expr.id, sym_id);
                    }
                    None => {
                        // Try base type for generic constructors: Pool<Node> → Pool
                        let base_name = name.split('<').next().unwrap_or(name);
                        if base_name != name {
                            if let Some(sym_id) = self.scopes.lookup(base_name) {
                                // The generic spelling needs its import as much
                                // as the bare one: `Pool<Node>.new()` reached
                                // here instead of the branch above and slipped
                                // past IM1 entirely.
                                if let Some(module) = self.needs_import_from(base_name) {
                                    self.errors.push(ResolveError::module_not_imported(
                                        base_name.to_string(),
                                        Some(module),
                                        expr.span,
                                    ));
                                }
                                self.resolutions.insert(expr.id, sym_id);
                                return;
                            }
                        }
                        // A name a module owns is a missing import, not a
                        // missing declaration. `Heap` and the atomics have no
                        // declaration anywhere, so once they stopped being
                        // always-in-scope they read as "unknown type `Heap`"
                        // with a fix suggesting the program declare one.
                        let base = name.split('<').next().unwrap_or(name);
                        if let Some(module) = self.needs_import_from(base) {
                            self.errors.push(ResolveError::module_not_imported(
                                base.to_string(), Some(module), expr.span,
                            ));
                            return;
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
                            if let SymbolKind::CNamespace { members } = &sym.kind {
                                let members = members.clone();
                                self.resolutions.insert(object.id, sym_id);
                                if let Some(&method_sym) = members.get(method) {
                                    self.resolutions.insert(expr.id, method_sym);
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
                            if let SymbolKind::CNamespace { members } = &sym.kind {
                                let members = members.clone();
                                self.resolutions.insert(object.id, sym_id);
                                if let Some(&field_sym) = members.get(field) {
                                    self.resolutions.insert(expr.id, field_sym);
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
            ExprKind::DynamicField { object, field_expr } => {
                self.resolve_expr(object);
                self.resolve_expr(field_expr);
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
            ExprKind::If { cond, then_branch, else_branch, else_binding } => {
                self.resolve_expr(cond);
                // OPT20/ER20: `if expr? as v` binds v in the then-branch.
                // ER21: else-branch also binds v (to the error) for Result.
                let presence_binding = match &cond.kind {
                    ExprKind::IsPresent { binding: Some(name), .. } => Some(name.clone()),
                    _ => None,
                };
                if let Some(ref name) = presence_binding {
                    self.scopes.push(ScopeKind::Block);
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
                    self.resolve_expr(then_branch);
                    self.scopes.pop();
                } else {
                    self.resolve_expr(then_branch);
                }
                if let Some(else_br) = else_branch {
                    // ER22: explicit `else as e` takes precedence over the cond's
                    // `as v` for the else-branch name.
                    let else_name = else_binding.clone().or_else(|| presence_binding.clone());
                    if let Some(ref name) = else_name {
                        self.scopes.push(ScopeKind::Block);
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
                        self.resolve_expr(else_br);
                        self.scopes.pop();
                    } else {
                        self.resolve_expr(else_br);
                    }
                }
            }
            ExprKind::IfLet { expr, pattern, then_branch, else_branch, else_binding } => {
                self.resolve_expr(expr);
                self.scopes.push(ScopeKind::Block);
                self.resolve_pattern(pattern);
                self.resolve_expr(then_branch);
                self.scopes.pop();
                if let Some(else_br) = else_branch {
                    // ER22: `else as e` scopes its binding to the else branch.
                    self.scopes.push(ScopeKind::Block);
                    if let Some(name) = else_binding {
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
                    self.resolve_expr(else_br);
                    self.scopes.pop();
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
            ExprKind::Try { expr: inner } | ExprKind::Take { place: inner } => {
                self.resolve_expr(inner);
            }
            ExprKind::Catch { value, ref clause } => {
                self.resolve_expr(value);
                // The binder scopes to the handler body only. `_` binds nothing.
                self.scopes.push(ScopeKind::Block);
                if !clause.is_discard() {
                    let sym_id = self.symbols.insert(
                        clause.binder.clone(),
                        SymbolKind::Variable { mutable: false },
                        None,
                        Span::new(0, 0),
                        false,
                    );
                    if let Err(e) = self.scopes.define(clause.binder.clone(), sym_id, Span::new(0, 0)) {
                        self.errors.push(e);
                    }
                }
                self.resolve_expr(&clause.body);
                self.scopes.pop();
            }
            ExprKind::IsPresent { expr: inner, .. } => {
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
                            } else if let SymbolKind::CNamespace { members } = &sym.kind {
                                if let Some(&member_sym) = members.get(parts[1]) {
                                    self.resolutions.insert(expr.id, member_sym);
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
                        SymbolKind::Variable { mutable: true },
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
                                    is_deleting: false,
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
            ExprKind::Cast { expr: inner, .. } | ExprKind::Convert { expr: inner, .. } => {
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
            Pattern::Range { .. } => {}
            Pattern::TypePat { binding, .. } => {
                if let Some(name) = binding {
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
            }
        }
    }
}

/// An extern signature as the user wrote it, for the conflict message.
fn render_extern_sig(abi: &str, params: &[String], ret_ty: &Option<String>) -> String {
    let params = params.join(", ");
    match ret_ty.as_deref().filter(|r| !r.is_empty() && *r != "()") {
        Some(ret) => format!("extern \"{}\" func({}) -> {}", abi, params, ret),
        None => format!("extern \"{}\" func({})", abi, params),
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
    use crate::error::ResolveErrorKind;
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
        // BF1 names eight compiler-known functions and BF3 reserves those. The
        // rest of what `register_builtins` puts in scope — `min`, `max`,
        // `clamp`, the test builtins — are ordinary generic functions, and a
        // program declaring its own has always been allowed.
        for name in ["min", "max", "clamp", "drop", "assert_eq"] {
            let decls = vec![make_fn_decl(name)];
            assert!(
                Resolver::resolve(&decls).is_ok(),
                "`{}` is not in BF1, so a program may declare its own",
                name
            );
        }
    }

    #[test]
    fn test_bf1_function_shadowing_error() {
        // BF3. These used to be accepted: `declare_function` asked
        // `is_builtin_type_name`, which by design said nothing about builtin
        // *functions*, so `func println(x: i64)` compiled and was then never
        // called — the compiler generates code for `println` per call site
        // (BF2), so the declaration had nothing to hook onto (#977).
        for name in [
            "println", "print", "format", "panic",
            "todo", "unreachable", "spawn", "transmute",
        ] {
            let decls = vec![make_fn_decl(name)];
            let err = Resolver::resolve(&decls)
                .expect_err(&format!("`{}` is in BF1 and must be rejected", name));
            assert!(
                err.iter().any(|e| matches!(
                    &e.kind,
                    ResolveErrorKind::ShadowsBuiltin { name: n } if n == name
                )),
                "`{}` should report shadowing a builtin, got {:?}",
                name,
                err
            );
        }
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
                backing_type: None,
            }),
            span: Span::new(0, 10),
        }];
        let result = Resolver::resolve(&decls);
        assert!(result.is_err(), "Shadowing prelude enum should fail");
    }

    fn make_struct_decl(name: &str) -> Decl {
        use rask_ast::decl::StructDecl;
        Decl {
            id: NodeId(0),
            kind: DeclKind::Struct(StructDecl {
                name: name.to_string(),
                type_params: vec![],
                fields: vec![],
                methods: vec![],
                is_pub: false,
                attrs: vec![],
                doc: None,
            }),
            span: Span::new(0, 10),
        }
    }

    /// BI3 over the whole builtin set, not just the names the stdlib happens not
    /// to declare.
    ///
    /// `Vec`, `Map`, `Pool`, `Handle`, `Rack`, `Link`, `Mutex`, `Option` and
    /// `Result` were all accepted before: the stdlib declares each of them in a
    /// `.rk` file of its own, that binding replaced the builtin one in the shared
    /// scope, and the check asked the scope rather than the builtin table. So
    /// `struct Set` was refused and `struct Vec` was not, for no reason a reader
    /// could see (#977).
    #[test]
    fn test_bi1_type_shadowing_error_covers_names_the_stdlib_also_declares() {
        for name in [
            "Vec", "Map", "Set", "string", "Error", "Channel",
            "Option", "Result", "Ordering", "i32", "f64",
        ] {
            let decls = vec![make_struct_decl(name)];
            assert!(
                Resolver::resolve(&decls).is_err(),
                "`{}` is always in scope, so a program may not declare it",
                name
            );
        }
    }

    /// The other side of the same rule: a stdlib type that isn't in BI1's set is
    /// an ordinary name, and a program may have it. `Handle` and `Pool` were
    /// refused here until the box family moved out of the always-in-scope table
    /// and into `memory` — being a closed compiler type (mem.boxes/BX1–BX4) is
    /// about who may define one, not about who can see the name unasked.
    #[test]
    fn test_a_stdlib_type_name_outside_bi1_is_declarable() {
        for name in [
            "Duration", "Timer", "Instant", "StringBuilder", "Budget9",
            "Pool", "Handle", "Rack", "Link", "Mutex", "Shared", "Heap",
        ] {
            let decls = vec![make_struct_decl(name)];
            assert!(
                Resolver::resolve(&decls).is_ok(),
                "`{}` is not in BI1's set, so a program that hasn't imported it \
                 may declare its own",
                name
            );
        }
    }

    /// IM8, in the order people write: the import first, the declaration after.
    /// Only the reverse order was checked, where the import meets a name that's
    /// already bound.
    #[test]
    fn test_declaration_after_import_shadows_it() {
        let decls = vec![
            make_import_decl(vec!["time", "Duration"], None, false, false),
            make_struct_decl("Duration"),
        ];
        let err = Resolver::resolve(&decls).expect_err("IM8: the second `Duration` is an error");
        assert!(
            err.iter().any(|e| matches!(
                &e.kind,
                ResolveErrorKind::ShadowsImport { name, module }
                    if name == "Duration" && module.as_deref() == Some("time")
            )),
            "should report shadowing `time`'s Duration, got {:?}",
            err
        );
    }

    /// An import records the name it bound, so IM1 can tell "the program asked
    /// for this" from "it was in scope anyway". The stdlib's declarations are
    /// collected before any import runs, so every module type was already in
    /// scope when the import got there — the registration loop hit its
    /// already-bound `continue` and never made the record, leaving `Duration`
    /// looking un-imported right after `import time`.
    #[test]
    fn test_a_module_import_records_the_names_it_brings() {
        let decls = vec![make_import_decl(vec!["time"], None, false, false)];
        let mut resolver = Resolver::new();
        resolver.collect_declarations(&decls);
        for name in ["Duration", "Instant", "Timer"] {
            assert!(
                resolver.imported_symbols.contains(name),
                "`import time` should record `{}`: {:?}",
                name,
                resolver.imported_symbols
            );
        }
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
                    Field { name: "x".to_string(), name_span: Span::new(0, 0), ty: "i32".to_string(), visibility: FieldVisibility::Public, attrs: vec![], default: None },
                    Field { name: "y".to_string(), name_span: Span::new(0, 0), ty: "i32".to_string(), visibility: FieldVisibility::Public, attrs: vec![], default: None },
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
                backing_type: None,
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
                    name_span: Span::new(0, 0),
                    fields: vec![],
                    attrs: vec![],
                    discriminant: None,
                }).collect(),
                methods: vec![],
                is_pub: true,
                attrs: vec![],
                doc: None,
                backing_type: None,
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

    fn make_pub_annotation_decl(name: &str, is_pub: bool) -> Decl {
        use rask_ast::decl::{AnnotationDecl, Field, FieldVisibility};
        Decl {
            id: NodeId(240),
            kind: DeclKind::Annotation(AnnotationDecl {
                name: name.to_string(),
                name_span: Span::new(0, 0),
                fields: vec![Field {
                    name: "max".to_string(),
                    name_span: Span::new(0, 0),
                    ty: "i64".to_string(),
                    visibility: FieldVisibility::Public,
                    attrs: vec![],
                    default: None,
                }],
                is_pub,
                doc: None,
            }),
            span: Span::new(0, 10),
        }
    }

    /// A `public annotation` has to cross the package boundary. Without it the
    /// importer's checker doesn't know the annotation exists: an attachment
    /// written there isn't validated at all — `@validate(bogus: 1)` was accepted
    /// with a field that doesn't exist — and `get<A>().max` has no declaration
    /// to read `max`'s type from (type.annotations/AN6).
    #[test]
    fn test_public_annotation_is_exported() {
        use crate::PackageRegistry;
        use std::path::PathBuf;

        let mut registry = PackageRegistry::new();
        let _lib = registry.add_package_with_decls(
            "liba".to_string(),
            vec!["liba".to_string()],
            PathBuf::from("/liba"),
            vec![
                make_pub_annotation_decl("validate", true),
                make_pub_annotation_decl("internal_only", false),
            ],
        );
        let app = registry.add_package(
            "app".to_string(),
            vec!["app".to_string()],
            PathBuf::from("/app"),
        );

        let decls = vec![
            make_import_decl(vec!["liba"], None, false, false),
            make_fn_decl("main"),
        ];
        let resolved = Resolver::resolve_package(&decls, &registry, app)
            .expect("should resolve");

        let ext = resolved.external_decls.get("liba").expect("liba exports");
        let exported: Vec<&str> = ext
            .iter()
            .filter_map(|d| match &d.kind {
                DeclKind::Annotation(a) => Some(a.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(exported, vec!["validate"], "public only, not the private one");
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
