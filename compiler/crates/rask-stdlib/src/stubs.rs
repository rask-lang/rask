// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Stub loader — parses .rk stub files to extract type/method metadata.
//!
//! Stub files in stdlib/ (net.rk, http.rk, etc.) are the single source of truth for builtin type APIs.
//! (force rebuild 2)

use rask_ast::decl::{Decl, DeclKind, FnDecl};
use rask_ast::Span;
use std::collections::HashMap;
use std::sync::OnceLock;

/// How many sources the stdlib contributes to the `file_id` space.
pub const STDLIB_FILE_COUNT: u16 = STUB_SOURCES.len() as u16;

/// First `file_id` the stdlib's own sources use.
///
/// Stubs are parsed separately from the user's package, so their ids can't come
/// from the same running counter — they'd collide, and a diagnostic raised
/// inside a stdlib body would resolve against the user's file.
///
/// The two spaces are disjoint by construction: the package counts up from 0,
/// the stdlib occupies the top of the range. Derived from the actual stub count
/// rather than picked, so the only ceiling is `file_id`'s own width — a package
/// would need ~65,500 files to reach it, and `parse_rk_files` reports that as an
/// error rather than letting the two spaces overlap.
pub const STDLIB_FILE_ID_BASE: u16 = u16::MAX - STDLIB_FILE_COUNT + 1;

/// `(name, source, file_id)` for every stub, so a caller can register them with
/// a `SourceMap` and have stdlib spans resolve against stdlib text.
pub fn stub_sources() -> impl Iterator<Item = (&'static str, &'static str, u16)> {
    STUB_SOURCES
        .iter()
        .enumerate()
        .map(|(i, (name, src))| (*name, *src, stub_file_id(i)))
}

/// The `file_id` for the nth stub source.
fn stub_file_id(index: usize) -> u16 {
    debug_assert!(index < STDLIB_FILE_COUNT as usize, "stub index out of range");
    STDLIB_FILE_ID_BASE + index as u16
}

/// Embedded stub file sources.
const STUB_SOURCES: &[(&str, &str)] = &[
    ("collections.rk", include_str!("../../../../stdlib/collections.rk")),
    ("memory.rk", include_str!("../../../../stdlib/memory.rk")),
    ("string.rk", include_str!("../../../../stdlib/string.rk")),
    ("option.rk", include_str!("../../../../stdlib/option.rk")),
    ("result.rk", include_str!("../../../../stdlib/result.rk")),
    ("io.rk", include_str!("../../../../stdlib/io.rk")),
    ("random.rk", include_str!("../../../../stdlib/random.rk")),
    // Ahead of builtins for the same reason collections.rk is: a method and a
    // free function share one name table, so whichever loads later wins the
    // bare name. With sequence.rk after builtins, giving `Sequence.min` a body
    // made `min(5.0, 3.0)` resolve to it and report "expected 1 argument,
    // found 2". `Vec.min` never had the problem only because collections.rk
    // already loads first (#1046).
    ("sequence.rk", include_str!("../../../../stdlib/sequence.rk")),
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
    ("num.rk", include_str!("../../../../stdlib/num.rk")),
    ("reflect.rk", include_str!("../../../../stdlib/reflect.rk")),
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
    /// Each parameter's declared mode, positionally matching `params`.
    ///
    /// `take` on a parameter is the declaration saying the callee keeps what it
    /// is handed. That is the fact MIR needs to decide whether the caller still
    /// owes a release on a string it passed — `v.push(s)` hands the reference
    /// over, `m.get(k)` only reads it — and before this it had to guess from a
    /// list of method names kept by hand in two passes.
    pub param_modes: Vec<StubParamMode>,
    pub ret_ty: String,
    pub doc: Option<String>,
    pub source_file: String,
    /// Byte offset span of the method name within the stub source.
    pub span: Span,
    /// Declared `@unimplemented` — the signature exists so the API can be
    /// designed and referenced, but nothing implements it on either backend.
    ///
    /// An empty body alone doesn't mean this: most stubs have empty bodies and
    /// are implemented natively in the runtime or the interpreter. This marks
    /// the ones that are genuinely holes, so calling one is an error the user
    /// sees at their call site rather than `Function not found: Vec_reserve`
    /// out of codegen.
    pub unimplemented: bool,
    /// Declared `@native` — the body isn't here, it's in the backends.
    ///
    /// This is the boundary of the language's blessed core, written down. An
    /// empty body used to mean four different things with nothing to tell them
    /// apart: implemented in the C runtime via the dispatch table, implemented
    /// as a bare symbol, resolved entirely at compile time, or not implemented
    /// at all. A reader of `stdlib/char.rk` couldn't tell which, and neither
    /// could a test — which is how `f64.floor()` shipped working on the
    /// interpreter and missing from codegen (#687).
    ///
    /// `Some(symbol)` names the implementation; `Some("")` means `@native` with
    /// no symbol given, for the ones resolved by another route.
    pub native: Option<std::string::String>,
    /// The stub has a Rask body — the implementation is right here, and both
    /// backends run it. The registry couldn't answer this before, which meant
    /// nothing could distinguish "written in Rask" from "declared only".
    pub has_body: bool,
    /// Declared `comptime func` — evaluated by the comptime engine, so the
    /// keyword already says where the body lives. `Vec.freeze` is one.
    pub is_comptime: bool,
    /// Trait bounds on the method's own type parameters: `decode<T: Decode>`
    /// gives `[("T", "Decode")]`. Carried because nothing else does — the
    /// checker builds module signatures from these stubs, and without the bound
    /// `json.decode<WithPtr>` type-checked clean and failed later, in MIR
    /// lowering on native and as a bogus "missing field" on interp.
    pub type_param_bounds: Vec<(String, String)>,
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

/// A stub parameter's declared mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StubParamMode {
    pub is_take: bool,
    pub is_mutate: bool,
    pub is_deleting: bool,
}

/// Top-level function extracted from stubs (println, print, etc.).
#[derive(Debug, Clone)]
pub struct FunctionStub {
    pub name: String,
    pub params: Vec<(String, String)>,
    /// Each parameter's declared mode, positionally matching `params`.
    ///
    /// Kept beside the name/type pairs rather than folded into them because the
    /// LSP reads those as a pair to render a signature, and only the resolver
    /// needs the modes — it registers a symbol per parameter, and a mode
    /// defaulted to `false` there would quietly reject a correct `mutate` at a
    /// call site.
    pub param_modes: Vec<StubParamMode>,
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
    ///
    /// Matches a bare `stdlib/string.rk` as well as an absolute path. The LSP
    /// always has an absolute one; `rask fmt stdlib/` does not, and requiring the
    /// leading slash is what left the formatter parsing the stubs without the
    /// allowances they need.
    pub fn is_stdlib_path(path: &str) -> bool {
        let path = path.replace('\\', "/");
        STUB_SOURCES.iter().any(|(name, _)| {
            let tail = format!("stdlib/{}", name);
            path == tail || path.ends_with(&format!("/{}", tail))
        })
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
                let parse_result = rask_parser::Parser::new(lex_result.tokens).allow_keyword_fn_names().parse();
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

        for (stub_index, (_filename, source)) in STUB_SOURCES.iter().enumerate() {
            let file_id = stub_file_id(stub_index);
            let lex_result = rask_lexer::Lexer::new_with_file_id(source, file_id).tokenize();
            if !lex_result.is_ok() {
                continue;
            }
            let mut parser =
                rask_parser::Parser::new_with_file_id(lex_result.tokens, next_id, file_id)
                    .allow_keyword_fn_names();
            let parse_result = parser.parse();
            next_id = parser.next_node_id();
            for decl in parse_result.decls {
                match &decl.kind {
                    DeclKind::Fn(_) | DeclKind::Impl(_) | DeclKind::Extern(_)
                    | DeclKind::Struct(_) | DeclKind::Enum(_) | DeclKind::Import(_)
                    | DeclKind::TypeAlias(_) | DeclKind::Trait(_) => {
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

        for (stub_index, (_filename, source)) in STUB_SOURCES.iter().enumerate() {
            let file_id = stub_file_id(stub_index);
            let lex_result = rask_lexer::Lexer::new_with_file_id(source, file_id).tokenize();
            if !lex_result.is_ok() {
                continue;
            }
            let mut parser =
                rask_parser::Parser::new_with_file_id(lex_result.tokens, next_id, file_id)
                    .allow_keyword_fn_names();
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
                        DeclKind::Trait(_) => true,
                        _ => false,
                    }
                } else {
                    // Files without function bodies still contribute struct/enum
                    // definitions — types must be visible for resolution even when
                    // their methods aren't implemented yet. Traits are the same:
                    // no body to strip, but the type checker still needs them to
                    // validate `extend T with Trait` conformance (#320).
                    matches!(&decl.kind, DeclKind::Struct(_) | DeclKind::Enum(_) | DeclKind::Trait(_))
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

        for (stub_index, (_filename, source)) in STUB_SOURCES.iter().enumerate() {
            let file_id = stub_file_id(stub_index);
            let lex_result = rask_lexer::Lexer::new_with_file_id(source, file_id).tokenize();
            if !lex_result.is_ok() {
                continue;
            }
            let mut parser =
                rask_parser::Parser::new_with_file_id(lex_result.tokens, next_id, file_id)
                    .allow_keyword_fn_names();
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

        for (stub_index, (_filename, source)) in STUB_SOURCES.iter().enumerate() {
            let file_id = stub_file_id(stub_index);
            let lex_result = rask_lexer::Lexer::new_with_file_id(source, file_id).tokenize();
            if !lex_result.is_ok() {
                continue;
            }
            let mut parser =
                rask_parser::Parser::new_with_file_id(lex_result.tokens, next_id, file_id)
                    .allow_keyword_fn_names();
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
                    param_modes: f.params.iter()
                        .filter(|p| p.name != "self")
                        .map(|p| StubParamMode {
                            is_take: p.is_take,
                            is_mutate: p.is_mutate,
                            is_deleting: p.is_deleting,
                        })
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
    let param_modes: Vec<StubParamMode> = f.params.iter()
        .filter(|p| p.name != "self")
        .map(|p| StubParamMode {
            is_take: p.is_take,
            is_mutate: p.is_mutate,
            is_deleting: p.is_deleting,
        })
        .collect();

    // Parser appends `<T: Bound>` to generic function names; strip for lookup.
    let bare_name = strip_type_params(&f.name);
    let span = find_func_name_span(source, &bare_name, parent_span);

    MethodStub {
        name: bare_name,
        takes_self,
        mutate_self,
        take_self,
        params,
        param_modes,
        ret_ty: f.ret_ty.clone().unwrap_or_default(),
        doc: f.doc.clone(),
        source_file: format!("stdlib/{}", filename),
        span,
        unimplemented: f.attrs.iter().any(|a| a == "unimplemented"),
        has_body: !f.body.is_empty(),
        is_comptime: f.is_comptime,
        native: f.attrs.iter().find_map(|a| {
            if a == "native" {
                return Some(std::string::String::new());
            }
            // `@native("rask_char_is_ascii")` reaches here as the attribute
            // text with its parens, which the parser preserved.
            a.strip_prefix("native(\"")
                .and_then(|rest| rest.strip_suffix("\")"))
                .map(|sym| sym.to_string())
        }),
        type_param_bounds: f.type_params.iter()
            .flat_map(|tp| tp.bounds.iter().map(move |b| (tp.name.clone(), b.clone())))
            .collect(),
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
    #[test]
    fn json_decode_carries_its_decode_bound() {
        let reg = super::StubRegistry::load();
        let m = reg.methods("json").iter()
            .find(|m| m.name == "decode")
            .expect("json.decode stub")
            .clone();
        assert_eq!(m.type_param_bounds, vec![("T".to_string(), "Decode".to_string())]);
    }

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
            "Vec", "Map", "Pool", "Handle", "string", "Option", "Result", "File", "Random",
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
        assert!(reg.has_method("fs", "read_text"));
        assert!(reg.has_method("fs", "write_text"));
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
            "read", "modify", "insert_if_missing", "modify_with_default",
            "keys", "values", "freeze",
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

    /// std.api/SD4: neither wrapper has methods — the operators are the whole
    /// API, and a stub reappearing here would put a second spelling back.
    #[test]
    fn the_wrappers_carry_no_methods() {
        let reg = StubRegistry::load();
        let cut = [
            "is_some", "is_none", "is_ok", "is_err",
            "unwrap", "unwrap_err", "unwrap_or", "unwrap_or_else",
            "map", "map_err", "and_then", "filter", "or", "ok", "to_option",
            "ok_or", "to_result",
        ];
        for method in &cut {
            assert!(!reg.has_method("Option", method), "Option grew a method: {}", method);
            assert!(!reg.has_method("Result", method), "Result grew a method: {}", method);
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
        assert!(reg.has_method("File", "read_bytes"), "File missing method: read_bytes");
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

#[cfg(test)]
mod boundary_tests {
    use super::*;

    /// Every `stdlib/*.rk` file is either compiled into STUB_SOURCES or listed
    /// here as deliberately left out.
    ///
    /// STUB_SOURCES is what the type checker knows the stdlib to be, and it's a
    /// hand-written list of `include_str!`s — a macro can't glob a directory, so
    /// nothing keeps it honest on its own. `reflect.rk` had been sitting on disk
    /// unlisted: `reflect.fields<T>()` had no return type, so `f.name` in a
    /// `comptime for` had no type either, and native couldn't dispatch a string
    /// method off it or infer what a `Vec` collecting it held (#931). Nothing
    /// failed loudly — the feature just half-worked.
    #[test]
    fn every_stdlib_file_is_listed_or_deliberately_left_out() {
        // Left out on purpose (#990). Both declare a trait the compiler already
        // provides — `Displayable` in fmt.rk, `Encode`/`Decode` in encoding.rk —
        // and a declared `to_string` then wins method lookup over the inherent
        // one, so `examples/package_manager.rk` stops building. Closing that
        // means deciding whether these files are documentation or the source of
        // truth; the entry here is so the answer stays a decision.
        const DELIBERATELY_ABSENT: &[&str] = &["encoding.rk", "fmt.rk"];

        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../stdlib");
        let mut on_disk: Vec<String> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("can't read {}: {}", dir, e))
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".rk"))
            .collect();
        on_disk.sort();

        let listed: std::collections::HashSet<&str> =
            STUB_SOURCES.iter().map(|(name, _)| *name).collect();

        let missing: Vec<&String> = on_disk
            .iter()
            .filter(|n| !listed.contains(n.as_str()) && !DELIBERATELY_ABSENT.contains(&n.as_str()))
            .collect();
        assert!(
            missing.is_empty(),
            "stdlib files the type checker never sees: {:?}\n\
             add them to STUB_SOURCES, or to DELIBERATELY_ABSENT with a reason",
            missing
        );

        let stale: Vec<&&str> = DELIBERATELY_ABSENT
            .iter()
            .filter(|n| !on_disk.iter().any(|d| d == *n) || listed.contains(**n))
            .collect();
        assert!(
            stale.is_empty(),
            "DELIBERATELY_ABSENT names files that are gone or now listed: {:?}",
            stale
        );
    }

    /// Every stdlib function says where its body lives.
    ///
    /// Four answers are legitimate: a Rask body right here, `comptime func`
    /// (the comptime engine evaluates it, so the keyword is the marker),
    /// `@native` (the backends implement it), or `@unimplemented` (nothing does,
    /// and calling it is an error the user sees at the call site). A fifth state
    /// — an empty body with no marker — used to be the majority, and it meant
    /// any of those with nothing to tell them apart. That's how `f64.floor()` came to work on
    /// the interpreter and be missing from codegen (#687), and how three
    /// implementations of `Path` came to disagree (#688).
    ///
    /// The list below is what remains unmarked. It shrinks; it must not grow.
    #[test]
    fn every_stdlib_function_says_where_its_body_lives() {
        let reg = StubRegistry::load();
        let mut unmarked: Vec<String> = Vec::new();
        for type_name in reg.type_names() {
            let Some(t) = reg.get_type(&type_name) else { continue };
            for m in &t.methods {
                if m.unimplemented || m.native.is_some() {
                    continue;
                }
                // A method with a Rask body is its own answer. The registry
                // doesn't carry bodies, so `has_body` stands in for it.
                if m.has_body || m.is_comptime {
                    continue;
                }
                unmarked.push(format!("{}.{}", type_name, m.name));
            }
        }
        unmarked.sort();
        assert!(
            unmarked.is_empty(),
            "{} stdlib functions have an empty body and no marker. Every one has to \
             say where its body lives — `@native(\"symbol\")`, `@unimplemented`, \
             `comptime func`, or a Rask body:\n  {}",
            unmarked.len(),
            unmarked.join("\n  ")
        );
    }
}
