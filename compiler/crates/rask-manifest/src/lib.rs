// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Reads `rask.toml` (struct.manifest).
//!
//! The loader is Rask code the toolchain runs on its interpreter, in two
//! packages: `schema` (`libs/schema/`), the general library for TOML config
//! files checked against a versioned schema, and `manifest` (`loader/`),
//! the `rask-package` schema plus the `toolchain_*` entry points. This crate
//! checks the two once per thread, as a package and its dependency, calls
//! the entry points, and turns the JSON they return into `PackageDecl`.

use std::cell::OnceCell;
use std::path::{Path, PathBuf};

use rask_ast::decl::{Decl, DepDecl, FeatureDecl, FeatureOption, PackageDecl, ProfileDecl};
use serde_json::Value;

/// The `schema` library's sources, without its tests.
const LIBRARY: &[(&str, &str)] = &[
    ("toml.rk", include_str!("../../../../libs/schema/toml.rk")),
    ("schema.rk", include_str!("../../../../libs/schema/schema.rk")),
    ("check.rk", include_str!("../../../../libs/schema/check.rk")),
    ("replay.rk", include_str!("../../../../libs/schema/replay.rk")),
    ("jsonschema.rk", include_str!("../../../../libs/schema/jsonschema.rk")),
];

/// The `manifest` package's sources, without its tests.
const LOADER: &[(&str, &str)] = &[
    ("package_schema.rk", include_str!("../loader/package_schema.rk")),
    ("package.rk", include_str!("../loader/package.rk")),
    ("toolchain.rk", include_str!("../loader/toolchain.rk")),
];

/// The loader, checked and ready to run.
struct Loader {
    decls: Vec<Decl>,
    typed: rask_compiler::CheckResult,
}

thread_local! {
    static CHECKED: OnceCell<Result<Loader, String>> = const { OnceCell::new() };
}

fn sources(dir: &str, files: &[(&str, &str)], extra: &[(&str, &str)]) -> Vec<(PathBuf, String)> {
    files
        .iter()
        .chain(extra)
        .map(|(name, text)| (PathBuf::from(format!("<{dir}>/{name}")), text.to_string()))
        .collect()
}

/// Check `manifest` against its dependency `schema`, from the sources in
/// this binary. `*_extra` adds files, which is how the tests bring theirs.
fn check_packages(
    library_extra: &[(&str, &str)],
    loader_extra: &[(&str, &str)],
) -> Result<rask_compiler::CheckResult, String> {
    let mut registry = rask_resolve::PackageRegistry::new();
    let library = registry
        .add_source_package("schema", sources("libs/schema", LIBRARY, library_extra), vec![], true)
        .map_err(|e| e.to_string())?;
    let root = registry
        .add_source_package("manifest", sources("manifest loader", LOADER, loader_extra), vec![library], false)
        .map_err(|e| e.to_string())?;
    let all_decls: Vec<Decl> = registry.get(root).map(|p| p.all_decls().cloned().collect()).unwrap_or_default();
    let mut ctx = rask_compiler::PackageContext { registry, root_id: root, all_decls };
    let config = rask_compiler::CompilerConfig {
        cfg: rask_comptime::CfgConfig::from_host("debug", vec![]),
    };
    let out = rask_compiler::check_package(&mut ctx, &config);
    match out.result {
        Some(result) if !out.has_errors() => Ok(result),
        // The loader ships inside the toolchain, so this is a toolchain bug,
        // not a problem with anyone's manifest.
        _ => {
            let msgs: Vec<String> = out.diagnostics.iter().map(|d| located(d, &out.source_files)).collect();
            Err(format!("the manifest loader doesn't compile:\n{}", msgs.join("\n")))
        }
    }
}

/// `file:line: message` for a diagnostic against the embedded sources.
fn located(d: &rask_diagnostics::Diagnostic, files: &[(PathBuf, String)]) -> String {
    let Some(label) = d.labels.first() else { return d.message.clone() };
    let Some((path, text)) = files.get(label.span.file_id as usize) else { return d.message.clone() };
    let line = text[..label.span.start.min(text.len())].matches('\n').count() + 1;
    format!("{}:{}: {}", path.display(), line, d.message)
}

fn check_loader() -> Result<Loader, String> {
    let result = check_packages(&[], &[])?;
    Ok(Loader { decls: rask_compiler::program_decls(&result.decls), typed: result })
}

fn interpreter_for(result: &rask_compiler::CheckResult) -> rask_interp::Interpreter {
    let mut interp = rask_interp::Interpreter::new();
    interp.inject_cfg(&rask_comptime::CfgConfig::from_host("debug", vec![]));
    interp.adopt_checker_tables(&result.typed);
    if !result.package_names.is_empty() {
        interp.register_packages(&result.package_names);
    }
    interp
}

/// Call `name(args...)` in the loader.
fn call(name: &str, args: &[&str]) -> Result<String, String> {
    CHECKED.with(|cell| {
        let loader = cell.get_or_init(check_loader).as_ref().map_err(|e| e.clone())?;
        interpreter_for(&loader.typed)
            .call_string_function(&loader.decls, name, args)
            .map_err(|d| format!("the manifest loader failed in `{name}`: {d}"))
    })
}

/// `{"ok": x}` or `{"error": "..."}` from the loader.
fn ok_or_error(json: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| format!("the manifest loader returned bad JSON ({e}): {json}"))?;
    if let Some(msg) = v.get("error").and_then(Value::as_str) {
        return Err(msg.to_string());
    }
    v.get("ok").cloned().ok_or_else(|| format!("the manifest loader returned neither ok nor error: {json}"))
}

/// Read and check a manifest. The error is every problem found, one per line.
pub fn load(path: &Path) -> Result<PackageDecl, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("can't read it: {e}"))?;
    load_text(&text)
}

pub fn load_text(text: &str) -> Result<PackageDecl, String> {
    let ok = ok_or_error(&call("toolchain_load", &[text])?)?;
    package_from_json(&ok)
}

/// Rewrite a manifest for schema version `to` (struct.manifest/MG1).
pub fn migrate(text: &str, to: u32) -> Result<String, String> {
    let ok = ok_or_error(&call("toolchain_migrate", &[text, &to.to_string()])?)?;
    ok.as_str().map(str::to_string).ok_or_else(|| "the manifest loader returned a non-string migration".into())
}

/// Replay the schema's history against its shape (struct.manifest/SH3).
pub fn check_history() -> Result<(), String> {
    match call("toolchain_check_history", &[])? {
        s if s.is_empty() => Ok(()),
        s => Err(s),
    }
}

/// The current shape as JSON Schema, for editors.
pub fn json_schema() -> Result<String, String> {
    call("toolchain_json_schema", &[])
}

/// `rask-package@N` for the schema this toolchain ships.
pub fn schema_line() -> Result<String, String> {
    call("toolchain_schema_line", &[])
}

/// Make package discovery read `rask.toml` with this loader.
pub fn install() {
    rask_resolve::package::set_manifest_loader(load);
}

// ─── JSON to PackageDecl ────────────────────────────────────────────
// The loader emits exactly PackageDecl's fields; a missing one is a bug in
// toolchain.rk, so it's an error rather than a default.

fn field<'a>(v: &'a Value, key: &str) -> Result<&'a Value, String> {
    v.get(key).ok_or_else(|| format!("manifest loader output has no `{key}`"))
}

fn string(v: &Value, key: &str) -> Result<String, String> {
    field(v, key)?.as_str().map(str::to_string).ok_or_else(|| format!("`{key}` is not a string"))
}

fn opt_string(v: &Value, key: &str) -> Result<Option<String>, String> {
    match field(v, key)? {
        Value::Null => Ok(None),
        Value::String(s) => Ok(Some(s.clone())),
        _ => Err(format!("`{key}` is neither a string nor null")),
    }
}

fn boolean(v: &Value, key: &str) -> Result<bool, String> {
    field(v, key)?.as_bool().ok_or_else(|| format!("`{key}` is not a boolean"))
}

fn array<'a>(v: &'a Value, key: &str) -> Result<&'a Vec<Value>, String> {
    field(v, key)?.as_array().ok_or_else(|| format!("`{key}` is not an array"))
}

fn strings(v: &Value, key: &str) -> Result<Vec<String>, String> {
    array(v, key)?
        .iter()
        .map(|s| s.as_str().map(str::to_string).ok_or_else(|| format!("`{key}` holds a non-string")))
        .collect()
}

/// `[["k", v], ...]` with a string or list-of-strings `v`.
fn pairs<T>(v: &Value, key: &str, item: impl Fn(&Value) -> Option<T>) -> Result<Vec<(String, T)>, String> {
    array(v, key)?
        .iter()
        .map(|p| {
            let k = p.get(0).and_then(Value::as_str);
            let val = p.get(1).and_then(&item);
            match (k, val) {
                (Some(k), Some(val)) => Ok((k.to_string(), val)),
                _ => Err(format!("`{key}` holds a malformed pair")),
            }
        })
        .collect()
}

fn as_string(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string)
}

fn as_strings(v: &Value) -> Option<Vec<String>> {
    v.as_array()?.iter().map(as_string).collect()
}

fn dep_from_json(v: &Value) -> Result<DepDecl, String> {
    Ok(DepDecl {
        name: string(v, "name")?,
        version: opt_string(v, "version")?,
        path: opt_string(v, "path")?,
        git: opt_string(v, "git")?,
        branch: opt_string(v, "branch")?,
        rev: opt_string(v, "rev")?,
        with_features: strings(v, "with")?,
        target: opt_string(v, "target")?,
        allow: strings(v, "allow")?,
        exclusive_selections: pairs(v, "selections", as_string)?,
        scope: opt_string(v, "scope")?,
        feature: opt_string(v, "feature")?,
    })
}

fn deps_from_json(v: &Value, key: &str) -> Result<Vec<DepDecl>, String> {
    array(v, key)?.iter().map(dep_from_json).collect()
}

fn package_from_json(v: &Value) -> Result<PackageDecl, String> {
    let features = array(v, "features")?
        .iter()
        .map(|f| {
            Ok(FeatureDecl {
                name: string(f, "name")?,
                exclusive: boolean(f, "exclusive")?,
                deps: deps_from_json(f, "deps")?,
                options: array(f, "options")?
                    .iter()
                    .map(|o| Ok(FeatureOption { name: string(o, "name")?, deps: deps_from_json(o, "deps")? }))
                    .collect::<Result<_, String>>()?,
                default: opt_string(f, "default")?,
                on_by_default: boolean(f, "on_by_default")?,
                enables: strings(f, "enables")?,
            })
        })
        .collect::<Result<_, String>>()?;
    let profiles = array(v, "profiles")?
        .iter()
        .map(|p| Ok(ProfileDecl { name: string(p, "name")?, settings: pairs(p, "settings", as_string)? }))
        .collect::<Result<_, String>>()?;
    Ok(PackageDecl {
        name: string(v, "name")?,
        version: string(v, "version")?,
        deps: deps_from_json(v, "deps")?,
        features,
        metadata: pairs(v, "metadata", as_string)?,
        list_metadata: pairs(v, "list_metadata", as_strings)?,
        profiles,
    })
}

#[cfg(test)]
mod tests;
