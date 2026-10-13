// SPDX-License-Identifier: (MIT OR Apache-2.0)

use super::*;

const MY_API: &str = r#"schema = "rask-package@1"

[package]
name = "my-api"
version = "1.0.0"
license = "MIT"
vendor-dir = "vendor"

[dependencies]
http = { registry = "^2.0", allow = ["net"] }
tls  = { git = "https://github.com/x/tls", rev = "a1b2c3" }
srv  = { registry = "^2", select = { runtime = "tokio" } }

[dev-dependencies]
mock = { path = "../mock" }

[features.ssl.dependencies]
ring = { registry = "^0.17", feature = "crypto" }

[features.full]
enables = ["ssl"]
default = true

[exclusive-features.runtime]
default = "tokio"
options.tokio = { tokio = { registry = "^1.0" } }

[profiles.staging]
inherits = "release"
debug-info = true
"#;

#[test]
fn the_loader_compiles_and_its_history_replays() {
    check_history().unwrap();
    assert_eq!(schema_line().unwrap(), "rask-package@1");
}

#[test]
fn a_manifest_becomes_a_package_decl() {
    let p = load_text(MY_API).unwrap();
    assert_eq!(p.name, "my-api");
    assert_eq!(p.version, "1.0.0");
    assert_eq!(p.meta("license"), Some("MIT"));
    assert_eq!(p.meta("vendor_dir"), Some("vendor"));
    assert_eq!(p.meta("sandbox"), Some("false"));

    let http = p.deps.iter().find(|d| d.name == "http").unwrap();
    assert_eq!(http.version.as_deref(), Some("^2.0"));
    assert_eq!(http.allow, vec!["net"]);
    assert_eq!(http.scope, None);

    let tls = p.deps.iter().find(|d| d.name == "tls").unwrap();
    assert_eq!(tls.git.as_deref(), Some("https://github.com/x/tls"));
    assert_eq!(tls.rev.as_deref(), Some("a1b2c3"));
    assert_eq!(tls.version, None);

    let srv = p.deps.iter().find(|d| d.name == "srv").unwrap();
    assert_eq!(srv.exclusive_selections, vec![("runtime".to_string(), "tokio".to_string())]);

    let mock = p.deps.iter().find(|d| d.name == "mock").unwrap();
    assert_eq!(mock.scope.as_deref(), Some("dev"));
    assert_eq!(mock.path.as_deref(), Some("../mock"));

    let ssl = p.features.iter().find(|f| f.name == "ssl").unwrap();
    assert!(!ssl.exclusive);
    assert_eq!(ssl.deps[0].feature.as_deref(), Some("crypto"));
    let full = p.features.iter().find(|f| f.name == "full").unwrap();
    assert_eq!(full.enables, vec!["ssl"]);
    assert!(full.on_by_default);

    let runtime = p.features.iter().find(|f| f.name == "runtime").unwrap();
    assert!(runtime.exclusive);
    assert_eq!(runtime.default.as_deref(), Some("tokio"));
    assert_eq!(runtime.options[0].deps[0].name, "tokio");

    let staging = &p.profiles[0];
    assert_eq!(staging.name, "staging");
    assert!(staging.settings.contains(&("debug_info".to_string(), "true".to_string())));
}

#[test]
fn a_typo_is_an_error_with_a_suggestion() {
    let err = load_text("schema = \"rask-package@1\"\n[package]\nname = \"a\"\nversion = \"1.0.0\"\nlicence = \"MIT\"\n")
        .unwrap_err();
    assert!(err.contains("unknown key `licence`; did you mean `license`?"), "{err}");
}

#[test]
fn migrate_round_trips_the_current_version() {
    let out = migrate(MY_API, 1).unwrap();
    assert!(out.starts_with("schema = \"rask-package@1\""), "{out}");
    load_text(&out).unwrap();
}

#[test]
fn json_schema_names_the_tables() {
    let s = json_schema().unwrap();
    assert!(s.contains("\"dev-dependencies\""));
}

fn run_rask_tests(result: &rask_compiler::CheckResult) -> Vec<String> {
    let mut interp = interpreter_for(result);
    let results = interp.run_tests(&rask_compiler::program_decls(&result.decls), None);
    assert!(results.len() > 10, "only {} tests ran", results.len());
    results.iter().filter(|r| !r.passed).map(|r| r.name.clone()).collect()
}

/// The `manifest` package's own `test` blocks, on the interpreter that runs
/// it, with `schema` as its dependency.
#[test]
fn the_loaders_rask_tests_pass() {
    let result = check_packages(&[], &[("tests.rk", include_str!("../loader/tests.rk"))]).unwrap();
    let failed = run_rask_tests(&result);
    assert!(failed.is_empty(), "failed: {failed:?}");
}

/// The `schema` library's own `test` blocks, with the library as the root.
#[test]
fn the_schema_librarys_rask_tests_pass() {
    let mut registry = rask_resolve::PackageRegistry::new();
    let root = registry
        .add_source_package(
            "schema",
            sources("libs/schema", LIBRARY, &[("tests.rk", include_str!("../../../../libs/schema/tests.rk"))]),
            vec![],
            false,
        )
        .unwrap();
    let all_decls: Vec<Decl> = registry.get(root).unwrap().all_decls().cloned().collect();
    let mut ctx = rask_compiler::PackageContext { registry, root_id: root, all_decls };
    let config = rask_compiler::CompilerConfig { cfg: rask_comptime::CfgConfig::from_host("debug", vec![]) };
    let out = rask_compiler::check_package(&mut ctx, &config);
    assert!(!out.has_errors(), "{:?}", out.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let failed = run_rask_tests(&out.result.unwrap());
    assert!(failed.is_empty(), "failed: {failed:?}");
}

// ─── Package discovery through the loader ──────────────────────────

fn write_package(dir: &Path, files: &[(&str, &str)]) {
    std::fs::create_dir_all(dir).unwrap();
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
}

#[test]
fn discovery_reads_rask_toml() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("mydir");
    write_package(&pkg_dir, &[
        ("rask.toml", "schema = \"rask-package@1\"\n\n[package]\nname = \"myapp\"\nversion = \"1.0.0\"\n"),
        ("main.rk", "func main() { }"),
    ]);
    let mut registry = rask_resolve::PackageRegistry::new();
    let id = registry.discover(&pkg_dir).unwrap();
    let pkg = registry.get(id).unwrap();
    assert_eq!(pkg.name, "myapp");
    assert_eq!(pkg.manifest.as_ref().unwrap().version, "1.0.0");
    assert_eq!(pkg.files.len(), 1);
}

#[test]
fn discovery_follows_a_path_dependency() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    let app_dir = tmp.path().join("app");
    write_package(&app_dir, &[
        ("rask.toml", "schema = \"rask-package@1\"\n\n[package]\nname = \"app\"\nversion = \"1.0.0\"\n\n[dependencies]\nshared = { path = \"../shared\" }\n"),
        ("main.rk", "func main() { }"),
    ]);
    write_package(&tmp.path().join("shared"), &[("lib.rk", "public func greet() { }")]);
    let mut registry = rask_resolve::PackageRegistry::new();
    let id = registry.discover(&app_dir).unwrap();
    let pkg = registry.get(id).unwrap();
    assert_eq!(pkg.imports.len(), 1);
    let dep = registry.get(pkg.imports[0]).unwrap();
    assert_eq!(dep.name, "shared");
    assert!(dep.is_external);
}

#[test]
fn a_bad_manifest_fails_discovery_with_every_problem() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    let pkg_dir = tmp.path().join("bad");
    write_package(&pkg_dir, &[
        ("rask.toml", "schema = \"rask-package@1\"\n\n[package]\nname = \"bad\"\nversion = \"1\"\nlicence = \"MIT\"\n"),
        ("main.rk", "func main() { }"),
    ]);
    let err = rask_resolve::PackageRegistry::new().discover(&pkg_dir).unwrap_err().to_string();
    assert!(err.contains("\"1\" is not a version"), "{err}");
    assert!(err.contains("did you mean `license`"), "{err}");
}
