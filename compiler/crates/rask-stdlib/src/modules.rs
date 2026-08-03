// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! What each stdlib module brings into scope.
//!
//! Read off the module's own `.rk` file: `http.rk` declares `Response`, so
//! `http` exports `Response`. Enum variants come from the declaration too, so
//! `Method.Get` doesn't need listing anywhere.
//!
//! This used to be five hand-written tables in three crates — two in the
//! resolver (one for `import m.Type`, one for `import m`), and three in the
//! interpreter. Nothing made them agree, and they'd drifted from the stdlib
//! itself: `import time.Timer` was rejected as "not part of this module" though
//! time.rk declares `Timer`, and the interpreter's `http` types were reachable
//! only through a glob import.

use std::collections::HashMap;
use std::sync::OnceLock;

use rask_ast::decl::DeclKind;

use crate::stubs::StubRegistry;

/// Names a module exports that its own file doesn't declare.
///
/// Two reasons a name lands here: the module re-exports a type declared
/// elsewhere (`fs.File` is declared in `io.rk`), or the type is provided by the
/// runtime/codegen with no `.rk` declaration at all. Anything else belongs in
/// the module's `.rk` file, not in this list.
const EXTRA_TYPES: &[(&str, &[&str])] = &[
    // Declared in io.rk; `fs` is the module you reach it through.
    ("fs", &["File"]),
    // Runtime/codegen types with no stub declaration at all.
    ("math", &["f32x4", "f32x8", "f64x2", "f64x4", "i32x4", "i32x8"]),
];

/// Names a module exports that aren't types — free functions that come into
/// scope with it, and submodules. Kept apart from the types because callers do
/// different things with them: these are valid in `import m.name` but must not
/// be registered as a struct.
const EXTRA_NAMES: &[(&str, &[&str])] = &[
    // `spawn(…)` reads as a language feature, not as `async.spawn(…)`.
    ("async", &["spawn", "join_all", "select_first", "cancelled"]),
    ("core", &["transmute"]),
    // `std` re-exports the reflection module; reflect.rk isn't in the stub set,
    // so nothing else knows the name.
    ("std", &["reflect", "exit"]),
];

/// What a module brings into scope.
#[derive(Debug, Default)]
pub struct ModuleExports {
    /// Struct-like types.
    pub types: Vec<String>,
    /// Enums with their variant names, in declaration order.
    pub enums: Vec<(String, Vec<String>)>,
    /// Free functions that come into scope with the module.
    pub functions: Vec<String>,
}

impl ModuleExports {
    /// Every exported name — types, enums, and functions.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.types
            .iter()
            .map(String::as_str)
            .chain(self.enums.iter().map(|(n, _)| n.as_str()))
            .chain(self.functions.iter().map(String::as_str))
    }

    /// True when `name` is an exported type or enum (not a function).
    pub fn exports_type(&self, name: &str) -> bool {
        self.types.iter().any(|t| t == name)
            || self.enums.iter().any(|(n, _)| n == name)
    }

    /// True when `name` is exported at all.
    pub fn exports(&self, name: &str) -> bool {
        self.names().any(|n| n == name)
    }
}

static TABLE: OnceLock<HashMap<String, ModuleExports>> = OnceLock::new();

fn table() -> &'static HashMap<String, ModuleExports> {
    TABLE.get_or_init(build)
}

fn build() -> HashMap<String, ModuleExports> {
    let mut out = derived();

    for (module, extra) in EXTRA_TYPES {
        let entry = out.entry((*module).to_string()).or_default();
        for name in *extra {
            if !entry.exports(name) {
                entry.types.push((*name).to_string());
            }
        }
    }

    for (module, names) in EXTRA_NAMES {
        let entry = out.entry((*module).to_string()).or_default();
        for name in *names {
            if !entry.exports(name) {
                entry.functions.push((*name).to_string());
            }
        }
    }

    for exports in out.values_mut() {
        exports.types.sort();
        exports.types.dedup();
        exports.enums.sort_by(|a, b| a.0.cmp(&b.0));
        exports.enums.dedup_by(|a, b| a.0 == b.0);
    }

    out
}

static ENUM_DECLS: OnceLock<HashMap<String, rask_ast::decl::EnumDecl>> = OnceLock::new();

/// Every enum the stdlib declares, by name — variants, payload types and all.
///
/// The interpreter needs these to evaluate `Method.Post`: it builds its enum
/// table from the program's own declarations, so a stdlib enum arrived as a name
/// with nothing behind it. Cached, because parsing the stub set per test run is
/// not free.
pub fn enum_decls() -> &'static HashMap<String, rask_ast::decl::EnumDecl> {
    ENUM_DECLS.get_or_init(|| {
        StubRegistry::all_type_decls()
            .into_iter()
            .filter_map(|decl| match decl.kind {
                DeclKind::Enum(e) => Some((base_name(&e.name), e)),
                _ => None,
            })
            .collect()
    })
}

/// Exports read straight off the stub sources, before the extras are added.
///
/// The name set comes from the registry, which attributes a type to the file it
/// was first seen in — that covers a type with only an `extend` block and no
/// `struct` of its own (`extend TcpListener` in net.rk). Variant lists come from
/// the parsed declarations, which the registry doesn't keep.
fn derived() -> HashMap<String, ModuleExports> {
    let registry = StubRegistry::load();

    let mut variants: HashMap<String, Vec<String>> = HashMap::new();
    for decl in StubRegistry::all_type_decls() {
        if let DeclKind::Enum(e) = &decl.kind {
            variants.insert(
                base_name(&e.name),
                e.variants.iter().map(|v| v.name.clone()).collect(),
            );
        }
    }

    let mut out: HashMap<String, ModuleExports> = HashMap::new();
    for name in registry.type_names() {
        let Some(module) = type_module(name) else { continue };
        // A module carries a same-named namespace struct (`struct http { }`) to
        // hang its qualified functions off. That's plumbing, not an export.
        if name == module {
            continue;
        }
        let entry = out.entry(module).or_default();
        match variants.get(name) {
            Some(vs) => entry.enums.push((name.to_string(), vs.clone())),
            None => entry.types.push(name.to_string()),
        }
    }
    out
}

/// `Response` → `http`, via the file the registry saw it declared in.
fn type_module(name: &str) -> Option<String> {
    StubRegistry::load()
        .get_type(name)?
        .source_file
        .strip_prefix("stdlib/")?
        .strip_suffix(".rk")
        .map(str::to_string)
}

fn base_name(name: &str) -> String {
    name.split('<').next().unwrap_or(name).to_string()
}

static EMPTY: OnceLock<ModuleExports> = OnceLock::new();

/// What `module` brings into scope. Empty for an unknown module.
pub fn exports(module: &str) -> &'static ModuleExports {
    table()
        .get(module)
        .unwrap_or_else(|| EMPTY.get_or_init(ModuleExports::default))
}

/// True when `module` exports a type or enum called `name`.
pub fn exports_type(module: &str, name: &str) -> bool {
    exports(module).exports_type(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn types_come_from_the_module_source() {
        let http = exports("http");
        for want in ["Request", "Response", "Headers", "HttpServer", "HttpClient", "Responder"] {
            assert!(http.exports_type(want), "http should export {}: {:?}", want, http);
        }
    }

    #[test]
    fn enum_variants_come_from_the_declaration() {
        let (_, variants) = exports("http")
            .enums
            .iter()
            .find(|(n, _)| n == "Method")
            .expect("http exports Method");
        for want in ["Get", "Head", "Post", "Put", "Delete", "Patch", "Options"] {
            assert!(variants.contains(&want.to_string()), "Method needs {}: {:?}", want, variants);
        }
    }

    #[test]
    fn a_module_does_not_export_its_own_namespace_struct() {
        assert!(!exports("http").exports("http"));
        assert!(!exports("json").exports("json"));
        assert!(!exports("os").exports("os"));
    }

    #[test]
    fn re_exports_and_runtime_types_are_covered() {
        assert!(exports_type("fs", "File"), "fs re-exports io.rk's File");
        assert!(exports_type("net", "TcpListener"), "runtime type, no stub decl");
        assert!(exports_type("math", "f32x4"));
    }

    #[test]
    fn module_functions_are_separate_from_types() {
        let a = exports("async");
        assert!(a.functions.iter().any(|f| f == "spawn"));
        assert!(!a.exports_type("spawn"), "spawn is a function, not a type");
        assert!(a.exports("spawn"));
    }

    /// Every extra has to be pulling its weight. Once a module's own `.rk` file
    /// declares a name, the entry is stale duplication — and the whole point of
    /// the list is that it holds only what can't be derived. This test is what
    /// keeps it that way; it caught `net`'s `TcpListener` on the first run.
    #[test]
    fn no_extra_export_is_already_derived() {
        let derived = derived();
        for (module, extra) in EXTRA_TYPES.iter().chain(EXTRA_NAMES.iter()) {
            for name in *extra {
                let already = derived
                    .get(*module)
                    .is_some_and(|e| e.exports(name));
                assert!(
                    !already,
                    "{}.rk already gives {} — drop it from EXTRA_EXPORTS",
                    module, name,
                );
            }
        }
    }

    /// The reverse: a type only reachable via `extend` in its module's file
    /// still counts as that module's export.
    #[test]
    fn an_extend_only_type_is_derived() {
        assert!(
            derived().get("net").is_some_and(|e| e.exports_type("TcpListener")),
            "net.rk has `extend TcpListener` and no struct — it still exports it",
        );
    }

    #[test]
    fn unknown_module_exports_nothing() {
        assert_eq!(exports("nope").names().count(), 0);
    }
}


