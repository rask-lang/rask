// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Stub loader — parses .rk stub files to extract type/method metadata.
//!
//! Stub files in stdlib/ (net.rk, http.rk, etc.) are the single source of truth for builtin type APIs.
//! (force rebuild 2)

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::Span;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Embedded stub file sources.
const STUB_SOURCES: &[(&str, &str)] = &[
    ("collections.rk", include_str!("../../../../stdlib/collections.rk")),
    ("memory.rk", include_str!("../../../../stdlib/memory.rk")),
    ("string.rk", include_str!("../../../../stdlib/string.rk")),
    ("option.rk", include_str!("../../../../stdlib/option.rk")),
    ("result.rk", include_str!("../../../../stdlib/result.rk")),
    ("io.rk", include_str!("../../../../stdlib/io.rk")),
    ("random.rk", include_str!("../../../../stdlib/random.rk")),
    ("builtins.rk", include_str!("../../../../stdlib/builtins.rk")),
    ("fs.rk", include_str!("../../../../stdlib/fs.rk")),
    ("net.rk", include_str!("../../../../stdlib/net.rk")),
    ("json.rk", include_str!("../../../../stdlib/json.rk")),
    ("cli.rk", include_str!("../../../../stdlib/cli.rk")),
    ("std.rk", include_str!("../../../../stdlib/std.rk")),
    ("http.rk", include_str!("../../../../stdlib/http.rk")),
    ("async.rk", include_str!("../../../../stdlib/async.rk")),
    ("thread.rk", include_str!("../../../../stdlib/thread.rk")),
    ("sync.rk", include_str!("../../../../stdlib/sync.rk")),
    ("time.rk", include_str!("../../../../stdlib/time.rk")),
    ("os.rk", include_str!("../../../../stdlib/os.rk")),
    ("path.rk", include_str!("../../../../stdlib/path.rk")),
    ("math.rk", include_str!("../../../../stdlib/math.rk")),
    ("char.rk", include_str!("../../../../stdlib/char.rk")),
    ("error_context.rk", include_str!("../../../../stdlib/error_context.rk")),
    ("bits.rk", include_str!("../../../../stdlib/bits.rk")),
    ("sequence.rk", include_str!("../../../../stdlib/sequence.rk")),
];

/// A method extracted from a stub file.
#[derive(Debug, Clone)]
pub struct MethodStub {
    pub name: String,
    pub takes_self: bool,
    /// True if declared `mutate self` — method mutates the receiver.
    pub mutate_self: bool,
    /// True if declared `take self` — method consumes the receiver.
    pub take_self: bool,
    pub params: Vec<(String, String)>, // (name, type)
    pub ret_ty: String,
    pub doc: Option<String>,
    pub source_file: String,
    /// Byte offset span of the method name within the stub source.
    pub span: Span,
}

/// A type extracted from a stub file.
#[derive(Debug, Clone)]
pub struct TypeStub {
    pub name: String,
    pub doc: Option<String>,
    pub methods: Vec<MethodStub>,
    pub source_file: String,
    /// Byte offset span of the type name within the stub source.
    pub span: Span,
}

/// Top-level function extracted from stubs (println, print, etc.).
#[derive(Debug, Clone)]
pub struct FunctionStub {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub ret_ty: String,
    pub doc: Option<String>,
    pub source_file: String,
    /// Byte offset span of the function name within the stub source.
    pub span: Span,
}

/// Registry of all stub data, lazily loaded.
pub struct StubRegistry {
    types: HashMap<String, TypeStub>,
    functions: Vec<FunctionStub>,
    sources: HashMap<String, &'static str>,
}

static REGISTRY: OnceLock<StubRegistry> = OnceLock::new();

impl StubRegistry {
    /// Check if a file path points to a stdlib stub file.
    pub fn is_stdlib_path(path: &str) -> bool {
        STUB_SOURCES.iter().any(|(name, _)| path.ends_with(&format!("/stdlib/{}", name)))
    }

    /// Get the global stub registry (lazily initialized).
    pub fn load() -> &'static StubRegistry {
        REGISTRY.get_or_init(|| {
            let mut registry = StubRegistry {
                types: HashMap::new(),
                functions: Vec::new(),
                sources: HashMap::new(),
            };

            for (filename, source) in STUB_SOURCES {
                registry.sources.insert(filename.to_string(), source);
                let lex_result = rask_lexer::Lexer::new(source).tokenize();
                if !lex_result.is_ok() {
                    continue;
                }
                let parse_result = rask_parser::Parser::new(lex_result.tokens).parse();
                for decl in &parse_result.decls {
                    registry.process_decl(decl, filename, source);
                }
            }

            registry
        })
    }

    /// Return declarations from stdlib .rk files that have compilable function
    /// bodies. Includes struct/enum definitions so the resolver can find types
    /// referenced by impl blocks and function bodies.
    /// All stdlib declarations with full method signatures preserved.
    /// Used by the type checker which needs to see every method name and
    /// its parameter/return types, even when the body is empty.
    pub fn typecheck_decls() -> Vec<Decl> {
        let mut decls = Vec::new();
        let mut next_id: u32 = 1_000_000;

        for (_filename, source) in STUB_SOURCES {
            let lex_result = rask_lexer::Lexer::new(source).tokenize();
            if !lex_result.is_ok() {
                continue;
            }
            let mut parser = rask_parser::Parser::new_with_start_id(lex_result.tokens, next_id);
            let parse_result = parser.parse();
            next_id = parser.next_node_id();
            for decl in parse_result.decls {
                match &decl.kind {
                    DeclKind::Fn(_) | DeclKind::Impl(_) | DeclKind::Extern(_)
                    | DeclKind::Struct(_) | DeclKind::Enum(_) | DeclKind::Import(_)
                    | DeclKind::TypeAlias(_) => {
                        decls.push(decl);
                    }
                    _ => {}
                }
            }
        }

        decls
    }

    /// Stdlib declarations with empty-body methods stripped.
    /// Used by the monomorphizer, interpreter, and codegen which need
    /// real implementations, not stub signatures.
    pub fn compilable_decls() -> Vec<Decl> {
        let mut decls = Vec::new();
        // Start NodeIds high to avoid collision with user code NodeIds.
        let mut next_id: u32 = 1_000_000;

        for (_filename, source) in STUB_SOURCES {
            let lex_result = rask_lexer::Lexer::new(source).tokenize();
            if !lex_result.is_ok() {
                continue;
            }
            let mut parser = rask_parser::Parser::new_with_start_id(lex_result.tokens, next_id);
            let parse_result = parser.parse();
            next_id = parser.next_node_id();
            let has_fn_body = parse_result.decls.iter().any(|d| match &d.kind {
                DeclKind::Fn(f) => !f.body.is_empty(),
                DeclKind::Impl(i) => i.methods.iter().any(|m| !m.body.is_empty()),
                _ => false,
            });
            for mut decl in parse_result.decls {
                let dominated = if has_fn_body {
                    match &decl.kind {
                        DeclKind::Fn(f) => !f.body.is_empty(),
                        DeclKind::Impl(i) => i.methods.iter().any(|m| !m.body.is_empty()),
                        DeclKind::Extern(_) => true,
                        DeclKind::Struct(_) | DeclKind::Enum(_) => true,
                        DeclKind::Import(_) => true,
                        DeclKind::TypeAlias(_) => true,
                        _ => false,
                    }
                } else {
                    // Files without function bodies still contribute struct/enum
                    // definitions — types must be visible for resolution even when
                    // their methods aren't implemented yet.
                    matches!(&decl.kind, DeclKind::Struct(_) | DeclKind::Enum(_))
                };
                if dominated {
                    // Strip empty-body methods from Impl blocks so they
                    // don't reach the monomorphizer or interpreter as
                    // no-op stubs that shadow C runtime implementations.
                    if let DeclKind::Impl(ref mut i) = decl.kind {
                        i.methods.retain(|m| !m.body.is_empty());
                    }
                    decls.push(decl);
                }
            }
        }

        rask_desugar::desugar(&mut decls);
        decls
    }

    /// Return struct/enum definitions from stdlib files that have compilable
    /// function bodies. Injected into the monomorphizer for layout computation.
    pub fn compilable_struct_defs() -> Vec<Decl> {
        let mut decls = Vec::new();
        let mut next_id: u32 = 2_000_000;

        for (_filename, source) in STUB_SOURCES {
            let lex_result = rask_lexer::Lexer::new(source).tokenize();
            if !lex_result.is_ok() {
                continue;
            }
            let mut parser = rask_parser::Parser::new_with_start_id(lex_result.tokens, next_id);
            let parse_result = parser.parse();
            next_id = parser.next_node_id();
            let has_fn_body = parse_result.decls.iter().any(|d| match &d.kind {
                DeclKind::Fn(f) => !f.body.is_empty(),
                DeclKind::Impl(i) => i.methods.iter().any(|m| !m.body.is_empty()),
                _ => false,
            });
            if has_fn_body {
                for decl in parse_result.decls {
                    if matches!(&decl.kind, DeclKind::Struct(_) | DeclKind::Enum(_)) {
                        decls.push(decl);
                    }
                }
            }
        }

        decls
    }

    /// Return struct and enum declarations from ALL stdlib files (not just those
    /// with function bodies). Used to register type definitions (fields, variants)
    /// that the type checker needs for field access and pattern matching.
    pub fn all_type_decls() -> Vec<Decl> {
        let mut decls = Vec::new();
        let mut next_id: u32 = 3_000_000;

        for (_filename, source) in STUB_SOURCES {
            let lex_result = rask_lexer::Lexer::new(source).tokenize();
            if !lex_result.is_ok() {
                continue;
            }
            let mut parser = rask_parser::Parser::new_with_start_id(lex_result.tokens, next_id);
            let parse_result = parser.parse();
            next_id = parser.next_node_id();
            for decl in parse_result.decls {
                if matches!(&decl.kind, DeclKind::Struct(_) | DeclKind::Enum(_) | DeclKind::Impl(_)) {
                    decls.push(decl);
                }
            }
        }

        decls
    }

    fn process_decl(&mut self, decl: &rask_ast::decl::Decl, filename: &str, source: &str) {
        let decl_span = decl.span;
        match &decl.kind {
            DeclKind::Struct(s) => {
                let base_name = strip_type_params(&s.name);
                let name_span = find_name_span(source, &base_name, "struct", decl_span);
                let entry = self.types.entry(base_name.clone()).or_insert_with(|| TypeStub {
                    name: base_name,
                    doc: s.doc.clone(),
                    methods: Vec::new(),
                    source_file: format!("stdlib/{}", filename),
                    span: name_span,
                });
                for m in &s.methods {
                    entry.methods.push(fn_to_method_stub(m, filename, source, decl_span));
                }
            }
            DeclKind::Enum(e) => {
                let base_name = strip_type_params(&e.name);
                let name_span = find_name_span(source, &base_name, "enum", decl_span);
                let entry = self.types.entry(base_name.clone()).or_insert_with(|| TypeStub {
                    name: base_name,
                    doc: e.doc.clone(),
                    methods: Vec::new(),
                    source_file: format!("stdlib/{}", filename),
                    span: name_span,
                });
                for m in &e.methods {
                    entry.methods.push(fn_to_method_stub(m, filename, source, decl_span));
                }
            }
            DeclKind::Impl(i) => {
                let base_name = strip_type_params(&i.target_ty);
                let entry = self.types.entry(base_name.clone()).or_insert_with(|| TypeStub {
                    name: base_name.clone(),
                    doc: None,
                    methods: Vec::new(),
                    source_file: format!("stdlib/{}", filename),
                    span: find_name_span(source, &base_name, "extend", decl_span),
                });
                for m in &i.methods {
                    entry.methods.push(fn_to_method_stub(m, filename, source, decl_span));
                }
            }
            DeclKind::Fn(f) => {
                let name_span = find_func_name_span(source, &f.name, decl_span);
                self.functions.push(FunctionStub {
                    name: f.name.clone(),
                    params: f.params.iter()
                        .filter(|p| p.name != "self")
                        .map(|p| (p.name.clone(), p.ty.clone()))
                        .collect(),
                    ret_ty: f.ret_ty.clone().unwrap_or_default(),
                    doc: f.doc.clone(),
                    source_file: format!("stdlib/{}", filename),
                    span: name_span,
                });
            }
            _ => {}
        }
    }

    /// Get methods for a type.
    pub fn methods(&self, type_name: &str) -> &[MethodStub] {
        self.types.get(type_name)
            .map(|t| t.methods.as_slice())
            .unwrap_or(&[])
    }

    /// Look up a specific method on a type.
    pub fn lookup_method(&self, type_name: &str, method_name: &str) -> Option<&MethodStub> {
        self.methods(type_name).iter().find(|m| m.name == method_name)
    }

    /// Check if a method exists on a type.
    pub fn has_method(&self, type_name: &str, method_name: &str) -> bool {
        self.lookup_method(type_name, method_name).is_some()
    }

    /// Get type stub by name.
    pub fn get_type(&self, type_name: &str) -> Option<&TypeStub> {
        self.types.get(type_name)
    }

    /// Get all registered type names.
    pub fn type_names(&self) -> impl Iterator<Item = &str> {
        self.types.keys().map(|s| s.as_str())
    }

    /// Get all top-level function stubs.
    pub fn functions(&self) -> &[FunctionStub] {
        &self.functions
    }

    /// Get the source text for a stub file by filename (e.g. "collections.rk").
    pub fn source(&self, filename: &str) -> Option<&str> {
        self.sources.get(filename).copied()
    }

    /// Convert a byte offset within a stub file to 0-based (line, col).
    pub fn offset_to_lsp_position(&self, source_file: &str, offset: usize) -> Option<(u32, u32)> {
        let filename = source_file.strip_prefix("stdlib/")?;
        let source = self.sources.get(filename)?;
        let line_map = rask_ast::LineMap::new(source);
        let (line, col) = line_map.offset_to_line_col(offset);
        // LineMap returns 1-based, LSP wants 0-based
        Some((line - 1, col - 1))
    }
}

/// Convert a FnDecl to a MethodStub with span.
fn fn_to_method_stub(f: &FnDecl, filename: &str, source: &str, parent_span: Span) -> MethodStub {
    let self_param = f.params.iter().find(|p| p.name == "self");
    let takes_self = self_param.is_some();
    let mutate_self = self_param.map_or(false, |p| p.is_mutate);
    let take_self = self_param.map_or(false, |p| p.is_take);
    let params: Vec<(String, String)> = f.params.iter()
        .filter(|p| p.name != "self")
        .map(|p| (p.name.clone(), p.ty.clone()))
        .collect();

    let span = find_func_name_span(source, &f.name, parent_span);

    MethodStub {
        name: f.name.clone(),
        takes_self,
        mutate_self,
        take_self,
        params,
        ret_ty: f.ret_ty.clone().unwrap_or_default(),
        doc: f.doc.clone(),
        source_file: format!("stdlib/{}", filename),
        span,
    }
}

/// Find the span of a type name after a keyword (struct/enum/extend) within a decl range.
fn find_name_span(source: &str, name: &str, keyword: &str, within: Span) -> Span {
    let start = within.start;
    let end = within.end.min(source.len());
    let text = &source[start..end];
    let pattern = format!("{} {}", keyword, name);
    if let Some(pos) = text.find(&pattern) {
        let name_start = start + pos + keyword.len() + 1;
        let name_end = name_start + name.len();
        Span::new(name_start, name_end)
    } else {
        within
    }
}

/// Find the span of a function name (`func name(`) within a decl range.
fn find_func_name_span(source: &str, name: &str, within: Span) -> Span {
    let start = within.start;
    let end = within.end.min(source.len());
    let text = &source[start..end];
    let pattern1 = format!("func {}(", name);
    let pattern2 = format!("func {}", name);
    let pos = text.find(&pattern1).or_else(|| text.find(&pattern2));
    if let Some(pos) = pos {
        // Point to the name, not the `func` keyword
        let name_start = start + pos + 5; // "func " is 5 chars
        let name_end = name_start + name.len();
        Span::new(name_start, name_end)
    } else {
        within
    }
}

/// Check if a declaration has a non-empty function body worth compiling.
/// Returns true for:
/// - Top-level functions with non-empty bodies
/// - Struct/enum declarations (type definitions needed by compilable functions)
/// - Impl/extend blocks where at least one method has a non-empty body
/// - Extern declarations (needed for C interop in stdlib)

/// Strip type parameters from a name: "Vec<T>" → "Vec", "Map<K, V>" → "Map"
fn strip_type_params(name: &str) -> String {
    if let Some(idx) = name.find('<') {
        name[..idx].to_string()
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stubs_load_without_panic() {
        let reg = StubRegistry::load();
        assert!(reg.types.len() > 0, "No types loaded");
    }

    #[test]
    fn vec_methods_present() {
        let reg = StubRegistry::load();
        let methods = reg.methods("Vec");
        assert!(methods.len() > 10, "Expected many Vec methods, got {}", methods.len());
        assert!(reg.has_method("Vec", "push"));
        assert!(reg.has_method("Vec", "pop"));
        assert!(reg.has_method("Vec", "len"));
        assert!(reg.has_method("Vec", "new"));
    }

    #[test]
    fn method_takes_self() {
        let reg = StubRegistry::load();
        let new = reg.lookup_method("Vec", "new").unwrap();
        assert!(!new.takes_self, "Vec.new() should not take self");
        let push = reg.lookup_method("Vec", "push").unwrap();
        assert!(push.takes_self, "Vec.push() should take self");
    }

    #[test]
    fn method_has_doc() {
        let reg = StubRegistry::load();
        let push = reg.lookup_method("Vec", "push").unwrap();
        assert!(push.doc.is_some(), "Vec.push() should have a doc comment");
    }

    #[test]
    fn option_methods_present() {
        let reg = StubRegistry::load();
        assert!(reg.has_method("Option", "is_some"));
        assert!(reg.has_method("Option", "unwrap"));
        assert!(reg.has_method("Option", "map"));
        assert!(reg.has_method("Option", "or"));
    }

    #[test]
    fn string_methods_present() {
        let reg = StubRegistry::load();
        assert!(reg.has_method("string", "len"));
        assert!(reg.has_method("string", "contains"));
        assert!(reg.has_method("string", "trim"));
    }

    #[test]
    fn top_level_functions() {
        let reg = StubRegistry::load();
        let fns = reg.functions();
        let names: Vec<&str> = fns.iter().map(|f| f.name.as_str()).collect();
        assert!(names.contains(&"println"), "Missing println: {:?}", names);
        assert!(names.contains(&"print"), "Missing print: {:?}", names);
    }

    #[test]
    fn type_has_doc() {
        let reg = StubRegistry::load();
        let vec_type = reg.get_type("Vec").unwrap();
        assert!(vec_type.doc.is_some(), "Vec should have a doc comment");
    }

    #[test]
    fn all_types_loaded() {
        let reg = StubRegistry::load();
        let expected = [
            "Vec", "Map", "Pool", "Handle", "string", "Option", "Result", "File", "Rng",
            "fs", "net", "json", "cli", "io", "std", "http",
            "JsonValue", "JsonError", "JsonParser",
            "Headers", "Request", "Response", "HttpServer", "Responder", "HttpClient",
            "Method", "HttpError",
        ];
        for name in &expected {
            assert!(reg.get_type(name).is_some(), "Missing type: {}", name);
        }
    }

    #[test]
    fn method_spans_are_precise() {
        let reg = StubRegistry::load();
        let push = reg.lookup_method("Vec", "push").unwrap();
        assert!(push.span.start > 0, "Method span should be non-zero");
        assert!(push.span.end > push.span.start, "Method span should have positive length");
        let source = reg.source("collections.rk").unwrap();
        let name_text = &source[push.span.start..push.span.end];
        assert_eq!(name_text, "push");
    }

    #[test]
    fn type_spans_are_precise() {
        let reg = StubRegistry::load();
        let vec_type = reg.get_type("Vec").unwrap();
        assert!(vec_type.span.start > 0);
        let source = reg.source("collections.rk").unwrap();
        let name_text = &source[vec_type.span.start..vec_type.span.end];
        assert_eq!(name_text, "Vec");
    }

    #[test]
    fn function_spans_are_precise() {
        let reg = StubRegistry::load();
        let println_fn = reg.functions().iter().find(|f| f.name == "println").unwrap();
        assert!(println_fn.span.start > 0);
        let source = reg.source("builtins.rk").unwrap();
        let name_text = &source[println_fn.span.start..println_fn.span.end];
        assert_eq!(name_text, "println");
    }

    #[test]
    fn disambiguates_same_name_methods() {
        let reg = StubRegistry::load();
        // Both Vec and Map have `new` — spans should point to different locations
        let vec_new = reg.lookup_method("Vec", "new").unwrap();
        let map_new = reg.lookup_method("Map", "new").unwrap();
        assert_ne!(vec_new.span.start, map_new.span.start,
            "Vec.new and Map.new should have different spans");

        let source = reg.source("collections.rk").unwrap();
        assert_eq!(&source[vec_new.span.start..vec_new.span.end], "new");
        assert_eq!(&source[map_new.span.start..map_new.span.end], "new");
    }

    #[test]
    fn fs_module_methods() {
        let reg = StubRegistry::load();
        assert!(reg.has_method("fs", "read_file"));
        assert!(reg.has_method("fs", "write_file"));
        assert!(reg.has_method("fs", "exists"));
        assert!(reg.has_method("fs", "open"));
        assert!(reg.has_method("fs", "create"));
    }

    #[test]
    fn module_types_loaded() {
        let reg = StubRegistry::load();
        for module in &["fs", "net", "json", "cli", "io", "std"] {
            let ts = reg.get_type(module);
            assert!(ts.is_some(), "Missing module type: {}", module);
        }
    }

    #[test]
    fn offset_to_position_works() {
        let reg = StubRegistry::load();
        // First line, first char should be (0, 0)
        let pos = reg.offset_to_lsp_position("stdlib/builtins.rk", 0);
        assert_eq!(pos, Some((0, 0)));
    }

    // ─── Stdlib discoverability: full API surface ──────────────

    #[test]
    fn vec_full_api() {
        let reg = StubRegistry::load();
        let expected = [
            "new", "with_capacity", "fixed", "len", "is_empty", "capacity",
            "is_bounded", "remaining", "allocated",
            "push", "try_push", "pop", "clear", "insert", "remove",
            "reserve", "try_reserve", "get", "get_clone",
        ];
        for method in &expected {
            assert!(reg.has_method("Vec", method), "Vec missing method: {}", method);
        }
    }

    #[test]
    fn map_full_api() {
        let reg = StubRegistry::load();
        let expected = [
            "new", "with_capacity", "len", "is_empty", "capacity", "is_bounded",
            "insert", "remove", "clear", "get", "get_clone", "contains_key",
            "read", "modify", "ensure", "ensure_modify",
            "iter", "keys", "values", "freeze",
        ];
        for method in &expected {
            assert!(reg.has_method("Map", method), "Map missing method: {}", method);
        }
    }

    #[test]
    fn pool_full_api() {
        let reg = StubRegistry::load();
        let expected = [
            "new", "with_capacity", "remove", "get", "len",
            "is_empty", "clear",
        ];
        for method in &expected {
            assert!(reg.has_method("Pool", method), "Pool missing method: {}", method);
        }
    }

    #[test]
    fn pool_insert_discoverable() {
        let reg = StubRegistry::load();
        assert!(reg.has_method("Pool", "insert"), "Pool missing method: insert");
    }

    #[test]
    fn string_full_api() {
        let reg = StubRegistry::load();
        let expected = [
            "len", "is_empty", "contains", "starts_with", "ends_with",
            "trim", "split", "replace", "chars",
        ];
        for method in &expected {
            assert!(reg.has_method("string", method), "string missing method: {}", method);
        }
    }

    #[test]
    fn string_case_methods_discoverable() {
        let reg = StubRegistry::load();
        assert!(reg.has_method("string", "to_uppercase"), "string missing to_uppercase");
        assert!(reg.has_method("string", "to_lowercase"), "string missing to_lowercase");
    }

    #[test]
    fn option_full_api() {
        let reg = StubRegistry::load();
        let expected = [
            "is_some", "is_none", "unwrap", "unwrap_or",
            "map", "and_then", "or",
        ];
        for method in &expected {
            assert!(reg.has_method("Option", method), "Option missing method: {}", method);
        }
    }

    #[test]
    fn option_filter_discoverable() {
        let reg = StubRegistry::load();
        assert!(reg.has_method("Option", "filter"), "Option missing filter");
    }

    #[test]
    fn result_full_api() {
        let reg = StubRegistry::load();
        let expected = [
            "is_ok", "is_err", "unwrap", "unwrap_err", "unwrap_or",
            "map", "map_err",
        ];
        for method in &expected {
            assert!(reg.has_method("Result", method), "Result missing method: {}", method);
        }
    }

    #[test]
    fn file_full_api() {
        let reg = StubRegistry::load();
        let expected = [
            "write", "close",
        ];
        for method in &expected {
            assert!(reg.has_method("File", method), "File missing method: {}", method);
        }
    }

    #[test]
    fn file_read_discoverable() {
        let reg = StubRegistry::load();
        assert!(reg.has_method("File", "read_all"), "File missing method: read_all");
        assert!(reg.has_method("File", "read_text"), "File missing method: read_text");
    }

    #[test]
    fn builtin_functions_present() {
        let reg = StubRegistry::load();
        let fns = reg.functions();
        let names: Vec<&str> = fns.iter().map(|f| f.name.as_str()).collect();
        let expected = ["println", "print", "panic"];
        for name in &expected {
            assert!(names.contains(name), "Missing builtin function: {}", name);
        }
    }

    #[test]
    fn stderr_and_assert_builtins() {
        let reg = StubRegistry::load();
        let fns = reg.functions();
        let names: Vec<&str> = fns.iter().map(|f| f.name.as_str()).collect();
        // eprintln/eprint: same convenience as println/print but for stderr
        assert!(names.contains(&"eprintln"), "Missing eprintln");
        assert!(names.contains(&"eprint"), "Missing eprint");
        // assert: runtime assertion (panics on false), distinct from test-only assert
        assert!(names.contains(&"assert"), "Missing assert");
    }

    #[test]
    fn method_signatures_have_return_types() {
        let reg = StubRegistry::load();
        // These methods must declare return types — not empty string
        let checks = [
            ("Vec", "len"), ("Vec", "pop"), ("Vec", "get"),
            ("Map", "len"), ("Map", "get"), ("Map", "contains_key"),
            ("string", "len"), ("string", "contains"),
            ("Option", "unwrap"), ("Option", "is_some"),
        ];
        for (ty, method) in &checks {
            let m = reg.lookup_method(ty, method)
                .unwrap_or_else(|| panic!("{}.{} not found", ty, method));
            assert!(!m.ret_ty.is_empty(), "{}.{}() has empty return type", ty, method);
        }
    }

    #[test]
    fn self_receiver_consistency() {
        let reg = StubRegistry::load();
        // Static methods should NOT take self
        let statics = [("Vec", "new"), ("Map", "new"), ("Pool", "new")];
        for (ty, method) in &statics {
            let m = reg.lookup_method(ty, method).unwrap();
            assert!(!m.takes_self, "{}.{} should be static (no self)", ty, method);
        }
        // Instance methods should take self
        let instances = [
            ("Vec", "push"), ("Vec", "len"), ("Vec", "pop"),
            ("Map", "insert"), ("Map", "len"), ("Map", "get"),
            ("string", "len"), ("string", "contains"),
        ];
        for (ty, method) in &instances {
            let m = reg.lookup_method(ty, method).unwrap();
            assert!(m.takes_self, "{}.{} should take self", ty, method);
        }
    }
}
