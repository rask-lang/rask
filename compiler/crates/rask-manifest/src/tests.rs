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

/// Every `pattern` in the JSON Schema, keyed by its description.
fn patterns(v: &Value, out: &mut std::collections::BTreeMap<String, String>) {
    match v {
        Value::Object(m) => {
            if let (Some(Value::String(p)), Some(Value::String(d))) = (m.get("pattern"), m.get("description")) {
                out.insert(d.clone(), p.clone());
            }
            m.values().for_each(|x| patterns(x, out));
        }
        Value::Array(a) => a.iter().for_each(|x| patterns(x, out)),
        _ => {}
    }
}

/// An editor checks a version with the exported regular expression, the
/// loader with its own code. Both have to give the same answer.
#[test]
fn json_schema_patterns_agree_with_the_loader() {
    let mut found = std::collections::BTreeMap::new();
    patterns(&serde_json::from_str(&json_schema().unwrap()).unwrap(), &mut found);
    let regex = |prefix: &str| {
        let (_, p) = found.iter().find(|(d, _)| d.starts_with(prefix)).unwrap_or_else(|| panic!("no pattern for {prefix}: {found:?}"));
        regex::Regex::new(p).unwrap()
    };
    let version = regex("a version,");
    let range = regex("a version range");
    let manifest = |version: &str, range: &str| {
        format!("schema = \"rask-package@1\"\n[package]\nname = \"a\"\nversion = \"{version}\"\n[dependencies]\nb = {{ registry = \"{range}\" }}\n")
    };
    let versions = ["1.0.0", "10.20.30", "1.0.0-beta.3", "1.0.0-rc1", "1.0", "1", "1.0.0-", "1.0.0-beta..3", "1.0.0-be_ta", "v1.0.0", "1.0.0.0", ""];
    for v in versions {
        assert_eq!(version.is_match(v), load_text(&manifest(v, "*")).is_ok(), "version {v:?}");
    }
    let ranges = ["*", "1", "1.2", "1.2.3", "^1.2", "~1.2.3", "=1.2.3", ">=1.5", "=1.0.0-beta.1", "^1.0.0-beta.1", "1.0.0-beta.1", "<2.0", ">1", "<=1", "^", "two", "1.2.3.4", ""];
    for r in ranges {
        assert_eq!(range.is_match(r), load_text(&manifest("1.0.0", r)).is_ok(), "range {r:?}");
    }
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
            sources(
                "libs/schema",
                LIBRARY,
                &[
                    ("tests.rk", include_str!("../../../../libs/schema/tests.rk")),
                    ("toml_tests.rk", include_str!("../../../../libs/schema/toml_tests.rk")),
                ],
            ),
            vec![],
            false,
        )
        .unwrap();
    let all_decls: Vec<Decl> = registry.get(root).unwrap().all_decls().cloned().collect();
    let mut ctx = rask_compiler::PackageContext { registry, root_id: root, all_decls };
    let config = rask_compiler::CompilerConfig::new(rask_comptime::CfgConfig::from_host("debug", vec![]));
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

// ─── Every field and form, as PackageDecl carries it ───────────────

const HEAD: &str = "schema = \"rask-package@1\"\n[package]\nname = \"app\"\nversion = \"0.1.0\"\n";

fn dep<'a>(deps: &'a [DepDecl], name: &str) -> &'a DepDecl {
    deps.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("no dependency `{name}`"))
}

fn feature<'a>(p: &'a PackageDecl, name: &str) -> &'a FeatureDecl {
    p.features.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no feature `{name}`"))
}

fn strs(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn every_package_key_reaches_metadata() {
    let p = load_text(
        "schema = \"rask-package@1\"\n[package]\nname = \"ws\"\nversion = \"2.3.4\"\ndescription = \"A workspace\"\n\
         license = \"MIT OR Apache-2.0\"\nrepository = \"https://github.com/x/ws\"\nmembers = [\"app\", \"lib\"]\n\
         sandbox = true\nvendor-dir = \"third_party\"\n",
    )
    .unwrap();
    assert_eq!(p.name, "ws");
    assert_eq!(p.version, "2.3.4");
    assert_eq!(p.meta("description"), Some("A workspace"));
    assert_eq!(p.meta("license"), Some("MIT OR Apache-2.0"));
    assert_eq!(p.meta("repository"), Some("https://github.com/x/ws"));
    assert_eq!(p.meta("vendor_dir"), Some("third_party"));
    assert_eq!(p.meta("sandbox"), Some("true"));
    assert_eq!(p.members(), Some(&strs(&["app", "lib"])));
    assert_eq!(p.list_metadata, vec![("members".to_string(), strs(&["app", "lib"]))]);
}

#[test]
fn a_minimal_manifest_has_only_the_sandbox_default() {
    let p = load_text(HEAD).unwrap();
    assert_eq!(p.metadata, vec![("sandbox".to_string(), "false".to_string())]);
    assert!(p.list_metadata.is_empty());
    assert_eq!(p.members(), None);
    assert!(p.deps.is_empty());
    assert!(p.features.is_empty());
    assert!(p.profiles.is_empty());
}

#[test]
fn an_empty_members_list_is_not_a_workspace() {
    let p = load_text(&format!("{HEAD}members = []\n")).unwrap();
    assert_eq!(p.members(), None);
}

#[test]
fn every_dependency_table_gets_its_scope() {
    let p = load_text(&format!(
        "{HEAD}[dependencies]\na = {{ registry = \"^1\" }}\n[dev-dependencies]\nb = {{ registry = \"^1\" }}\n\
         [build-dependencies]\nc = {{ registry = \"^1\" }}\n[features.f.dependencies]\nd = {{ registry = \"^1\" }}\n\
         [features.f.dev-dependencies]\ne = {{ registry = \"^1\" }}\n[features.f.build-dependencies]\ng = {{ registry = \"^1\" }}\n\
         [exclusive-features.x]\ndefault = \"o\"\noptions.o = {{ h = {{ registry = \"^1\" }} }}\n"
    ))
    .unwrap();
    // The package's own deps, dev and build ones included, travel in one list.
    let top: Vec<_> = p.deps.iter().map(|d| (d.name.as_str(), d.scope.as_deref())).collect();
    assert_eq!(top, vec![("a", None), ("b", Some("dev")), ("c", Some("build"))]);

    let f = feature(&p, "f");
    assert!(!f.exclusive);
    let gated: Vec<_> = f.deps.iter().map(|d| (d.name.as_str(), d.scope.as_deref())).collect();
    assert_eq!(gated, vec![("d", None), ("e", Some("dev")), ("g", Some("build"))]);
    assert!(f.options.is_empty());
    assert_eq!(f.default, None);

    let x = feature(&p, "x");
    assert!(x.exclusive);
    assert!(x.deps.is_empty());
    assert_eq!(x.options.len(), 1);
    assert_eq!(x.options[0].name, "o");
    assert_eq!(x.options[0].deps[0].name, "h");
    assert_eq!(x.options[0].deps[0].scope, None);
}

#[test]
fn every_dependency_source_fills_its_own_fields() {
    let p = load_text(&format!(
        "{HEAD}[dependencies]\nreg = {{ registry = \"^2.0\" }}\nloc = {{ path = \"../loc\" }}\n\
         pinned = {{ git = \"https://g/p\", rev = \"a1b2c3\" }}\ntracking = {{ git = \"https://g/t\", branch = \"main\" }}\n\
         bare = {{ git = \"https://g/b\" }}\n"
    ))
    .unwrap();
    let reg = dep(&p.deps, "reg");
    assert_eq!(
        (reg.version.as_deref(), reg.path.as_deref(), reg.git.as_deref(), reg.rev.as_deref(), reg.branch.as_deref()),
        (Some("^2.0"), None, None, None, None)
    );
    let loc = dep(&p.deps, "loc");
    assert_eq!((loc.version.as_deref(), loc.path.as_deref(), loc.git.as_deref()), (None, Some("../loc"), None));
    let pinned = dep(&p.deps, "pinned");
    assert_eq!(
        (pinned.git.as_deref(), pinned.rev.as_deref(), pinned.branch.as_deref(), pinned.version.as_deref()),
        (Some("https://g/p"), Some("a1b2c3"), None, None)
    );
    let tracking = dep(&p.deps, "tracking");
    assert_eq!((tracking.git.as_deref(), tracking.rev.as_deref(), tracking.branch.as_deref()), (Some("https://g/t"), None, Some("main")));
    let bare = dep(&p.deps, "bare");
    assert_eq!((bare.git.as_deref(), bare.rev.as_deref(), bare.branch.as_deref()), (Some("https://g/b"), None, None));
}

#[test]
fn every_shared_dependency_key_comes_through() {
    let p = load_text(&format!(
        "{HEAD}[dependencies]\nfull = {{ registry = \"^1.0\", target = \"linux\", with = [\"rt\", \"net\"], \
         allow = [\"net\", \"read\", \"write\", \"exec\", \"ffi\", \"build_exec\"], \
         select = {{ runtime = \"tokio\", tls = \"rustls\" }}, feature = \"async\" }}\n\
         loc = {{ path = \"../loc\", target = \"macos\", with = [\"x\"], allow = [\"ffi\"] }}\nplain = {{ registry = \"*\" }}\n"
    ))
    .unwrap();
    let full = dep(&p.deps, "full");
    assert_eq!(full.target.as_deref(), Some("linux"));
    assert_eq!(full.with_features, strs(&["rt", "net"]));
    assert_eq!(full.allow, strs(&["net", "read", "write", "exec", "ffi", "build_exec"]));
    assert_eq!(
        full.exclusive_selections,
        vec![("runtime".to_string(), "tokio".to_string()), ("tls".to_string(), "rustls".to_string())]
    );
    assert_eq!(full.feature.as_deref(), Some("async"));

    let loc = dep(&p.deps, "loc");
    assert_eq!(loc.target.as_deref(), Some("macos"));
    assert_eq!(loc.with_features, strs(&["x"]));
    assert_eq!(loc.allow, strs(&["ffi"]));

    let plain = dep(&p.deps, "plain");
    assert_eq!(plain.version.as_deref(), Some("*"));
    assert_eq!(plain.target, None);
    assert!(plain.with_features.is_empty());
    assert!(plain.allow.is_empty());
    assert!(plain.exclusive_selections.is_empty());
    assert_eq!(plain.feature, None);
    assert_eq!(plain.scope, None);
}

#[test]
fn features_carry_default_enables_and_their_deps() {
    let p = load_text(&format!(
        "{HEAD}[features.ssl]\ndefault = true\nenables = [\"crypto\", \"logging\"]\n\
         [features.ssl.dependencies]\nopenssl = {{ registry = \"^3.0\", target = \"linux\" }}\n\
         [features.crypto]\n[features.logging]\ndefault = false\n"
    ))
    .unwrap();
    let names: Vec<_> = p.features.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, vec!["ssl", "crypto", "logging"]);
    let ssl = feature(&p, "ssl");
    assert!(ssl.on_by_default);
    assert_eq!(ssl.enables, strs(&["crypto", "logging"]));
    assert_eq!(dep(&ssl.deps, "openssl").target.as_deref(), Some("linux"));
    let crypto = feature(&p, "crypto");
    assert!(!crypto.on_by_default && crypto.enables.is_empty() && crypto.deps.is_empty() && !crypto.exclusive);
    assert!(!feature(&p, "logging").on_by_default);
    assert!(p.deps.is_empty(), "a feature's deps stay with the feature");
}

#[test]
fn exclusive_groups_carry_default_and_options() {
    // `tls` defines its option subtable before its own header, which TOML allows.
    let p = load_text(&format!(
        "{HEAD}[exclusive-features.runtime]\ndefault = \"tokio\"\n\
         options.tokio = {{ tokio = {{ registry = \"^1.0\", with = [\"full\"] }} }}\n\
         options.async-std = {{ async-std = {{ registry = \"^1.12\" }}, futures = {{ registry = \"^0.3\" }} }}\n\
         [exclusive-features.tls.options.none]\n[exclusive-features.tls]\ndefault = \"none\"\n"
    ))
    .unwrap();
    let runtime = feature(&p, "runtime");
    assert!(runtime.exclusive);
    assert!(!runtime.on_by_default);
    assert!(runtime.enables.is_empty());
    assert_eq!(runtime.default.as_deref(), Some("tokio"));
    let options: Vec<_> = runtime.options.iter().map(|o| (o.name.as_str(), o.deps.len())).collect();
    assert_eq!(options, vec![("tokio", 1), ("async-std", 2)]);
    assert_eq!(runtime.options[0].deps[0].with_features, strs(&["full"]));
    assert_eq!(runtime.options[1].deps[1].version.as_deref(), Some("^0.3"));
    let tls = feature(&p, "tls");
    assert_eq!(tls.default.as_deref(), Some("none"));
    assert_eq!(tls.options.len(), 1);
    assert!(tls.options[0].deps.is_empty());
}

#[test]
fn every_profile_key_becomes_a_setting() {
    let p = load_text(&format!(
        "{HEAD}[profiles.embedded]\ninherits = \"release\"\nopt-level = \"z\"\ndebug-info = false\n\
         overflow-checks = true\nlto = \"fat\"\ncodegen-units = 16\nstrip = true\npanic = \"abort\"\n[profiles.bare]\n"
    ))
    .unwrap();
    assert_eq!(p.profiles.len(), 2);
    let embedded = &p.profiles[0];
    assert_eq!(embedded.name, "embedded");
    let expect: Vec<(String, String)> = [
        ("inherits", "release"),
        ("opt_level", "z"),
        ("debug_info", "false"),
        ("overflow_checks", "true"),
        ("lto", "fat"),
        ("codegen_units", "16"),
        ("strip", "true"),
        ("panic", "abort"),
    ]
    .iter()
    .map(|(k, v)| (k.to_string(), v.to_string()))
    .collect();
    assert_eq!(embedded.settings, expect);
    assert_eq!(p.profiles[1].name, "bare");
    assert!(p.profiles[1].settings.is_empty());
}

#[test]
fn bad_values_are_refused_with_their_paths() {
    let cases: &[(&str, &str)] = &[
        ("version = \"1.0\"\n", "package.version: \"1.0\" is not a version"),
        ("sandbox = \"yes\"\n", "package.sandbox: expected `true` or `false`, found a string"),
        ("members = \"app\"\n", "package.members: expected an array, found a string"),
        ("[dependencies]\nx = { registry = \"two\" }\n", "dependencies.x.registry: \"two\" is not a version range"),
        ("[dependencies]\nx = { path = \"p\", rev = \"a\" }\n", "dependencies.x: unknown key `rev`"),
        ("[dependencies]\nx = { registry = \"^1\", allow = [\"disk\"] }\n", "dependencies.x.allow[0]: \"disk\" is not allowed here"),
        ("[dependencies]\nx = \"^1\"\n", "dependencies.x: expected a table holding one of"),
        ("[features.f]\ndefault = 1\n", "features.f.default: expected `true` or `false`, found an integer"),
        ("[exclusive-features.g]\noptions.a = {}\n", "exclusive-features.g: missing required key `default` (string)"),
        ("[profiles.p]\nopt-level = \"4\"\n", "profiles.p.opt-level: \"4\" is not allowed here"),
        ("[profiles.p]\ncodegen-units = \"16\"\n", "profiles.p.codegen-units: expected an integer, found a string"),
        ("[workspace]\n", "top level: unknown key `workspace`"),
    ];
    for (body, want) in cases {
        // `version` replaces HEAD's own, so it goes in a file of its own.
        let text = if body.starts_with("version") {
            format!("schema = \"rask-package@1\"\n[package]\nname = \"a\"\n{body}")
        } else {
            format!("{HEAD}{body}")
        };
        let err = load_text(&text).unwrap_err();
        assert!(err.contains(want), "for {body:?}: wanted {want:?}, got {err}");
    }
}

#[test]
fn every_problem_comes_back_one_per_line() {
    let err = load_text(&format!(
        "schema = \"rask-package@1\"\n[package]\nname = \"a\"\nversion = \"1\"\nlicence = \"MIT\"\n\
         [dependencies]\nx = {{ registry = \"^1\", path = \"p\" }}\n[profiles.p]\nlto = \"full\"\n"
    ))
    .unwrap_err();
    let lines: Vec<&str> = err.lines().collect();
    assert_eq!(lines.len(), 4, "{err}");
    assert!(lines.iter().any(|l| l.starts_with("package.version: ")), "{err}");
    assert!(lines.iter().any(|l| l.starts_with("package: unknown key `licence`; did you mean `license`?")), "{err}");
    assert!(lines.iter().any(|l| l.starts_with("dependencies.x: holds `registry`, `path`")), "{err}");
    assert!(lines.iter().any(|l| l.starts_with("profiles.p.lto: ")), "{err}");
}

#[test]
fn a_file_for_a_newer_schema_says_to_update() {
    let err = load_text("schema = \"rask-package@2\"\n[package]\nname = \"a\"\nversion = \"1.0.0\"\n").unwrap_err();
    assert!(err.contains("update the toolchain"), "{err}");
    let err = load_text("[package]\nname = \"a\"\nversion = \"1.0.0\"\n").unwrap_err();
    assert!(err.contains("missing `schema = "), "{err}");
}

// ─── Discovery: more shapes ────────────────────────────────────────

fn manifest(name: &str, rest: &str) -> String {
    format!("schema = \"rask-package@1\"\n\n[package]\nname = \"{name}\"\nversion = \"1.0.0\"\n{rest}")
}

#[test]
fn discovery_reads_a_workspace_and_its_members() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("ws");
    write_package(&root, &[("rask.toml", &manifest("ws", "members = [\"app\", \"lib\"]\n"))]);
    write_package(&root.join("app"), &[
        ("rask.toml", &manifest("app", "\n[dependencies]\nlib = { path = \"../lib\" }\n")),
        ("main.rk", "func main() { }"),
    ]);
    write_package(&root.join("lib"), &[
        ("rask.toml", &manifest("lib", "license = \"MIT\"\n")),
        ("lib.rk", "public func greet() { }"),
    ]);
    let mut registry = rask_resolve::PackageRegistry::new();
    let ids = registry.discover_workspace(&root).unwrap();
    let names: Vec<_> = ids.iter().map(|id| registry.get(*id).unwrap().name.clone()).collect();
    assert_eq!(names, vec!["app", "lib"]);
    let app = registry.get(ids[0]).unwrap();
    assert_eq!(app.imports.len(), 1);
    assert_eq!(registry.get(app.imports[0]).unwrap().name, "lib");
    let lib = registry.get(ids[1]).unwrap();
    assert_eq!(lib.manifest.as_ref().unwrap().meta("license"), Some("MIT"));
}

#[test]
fn a_missing_workspace_member_is_an_error() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("ws");
    write_package(&root, &[("rask.toml", &manifest("ws", "members = [\"ghost\"]\n"))]);
    let err = rask_resolve::PackageRegistry::new().discover_workspace(&root).unwrap_err().to_string();
    assert!(err.contains("workspace member 'ghost' not found"), "{err}");
}

#[test]
fn discovery_follows_a_chain_of_path_dependencies() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    write_package(&tmp.path().join("app"), &[
        ("rask.toml", &manifest("app", "\n[dependencies]\nmid = { path = \"../mid\" }\n")),
        ("main.rk", "func main() { }"),
    ]);
    write_package(&tmp.path().join("mid"), &[
        ("rask.toml", &manifest("mid", "\n[dependencies]\nleaf = { path = \"../leaf\" }\n")),
        ("lib.rk", "public func m() { }"),
    ]);
    write_package(&tmp.path().join("leaf"), &[
        ("rask.toml", &manifest("leaf", "")),
        ("lib.rk", "public func l() { }"),
    ]);
    let mut registry = rask_resolve::PackageRegistry::new();
    let id = registry.discover(&tmp.path().join("app")).unwrap();
    let app = registry.get(id).unwrap();
    let mid = registry.get(app.imports[0]).unwrap();
    assert_eq!(mid.name, "mid");
    assert!(mid.is_external);
    assert_eq!(mid.imports.len(), 1);
    let leaf = registry.get(mid.imports[0]).unwrap();
    assert_eq!(leaf.name, "leaf");
    assert_eq!(leaf.manifest.as_ref().unwrap().version, "1.0.0");
}

fn app_with_dev_dep(tmp: &Path) -> PathBuf {
    let app = tmp.join("app");
    write_package(&app, &[
        ("rask.toml", &manifest("app", "\n[dev-dependencies]\nmock = { path = \"../mock\" }\n")),
        ("main.rk", "func main() { }"),
    ]);
    write_package(&tmp.join("mock"), &[("lib.rk", "public func fake() { }")]);
    app
}

#[test]
fn a_dev_dependency_is_linked_only_when_dev_is_on() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    let app = app_with_dev_dep(tmp.path());

    let mut release = rask_resolve::PackageRegistry::new();
    let id = release.discover(&app).unwrap();
    assert!(release.get(id).unwrap().imports.is_empty());
    assert_eq!(release.unlinked_scopes().get("mock").map(String::as_str), Some("dev"));

    let mut test = rask_resolve::PackageRegistry::new();
    test.include_scope("dev");
    let id = test.discover(&app).unwrap();
    let pkg = test.get(id).unwrap();
    assert_eq!(pkg.imports.len(), 1);
    assert_eq!(test.get(pkg.imports[0]).unwrap().name, "mock");
    assert!(test.unlinked_scopes().is_empty());
}

#[test]
fn a_bad_manifest_in_a_path_dependency_fails_discovery() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    write_package(&tmp.path().join("app"), &[
        ("rask.toml", &manifest("app", "\n[dependencies]\nbroken = { path = \"../broken\" }\n")),
        ("main.rk", "func main() { }"),
    ]);
    write_package(&tmp.path().join("broken"), &[
        ("rask.toml", &manifest("broken", "\n[dependencies]\nx = { registry = \"^1\", revv = \"a\" }\n")),
        ("lib.rk", "public func b() { }"),
    ]);
    let err = rask_resolve::PackageRegistry::new().discover(&tmp.path().join("app")).unwrap_err().to_string();
    assert!(err.contains("broken"), "{err}");
    assert!(err.contains("dependencies.x: unknown key `revv`"), "{err}");
}

#[test]
fn a_bad_workspace_root_fails_before_any_member() {
    install();
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path().join("ws");
    write_package(&root, &[("rask.toml", &manifest("ws", "members = \"app\"\n"))]);
    let err = rask_resolve::PackageRegistry::new().discover_workspace(&root).unwrap_err().to_string();
    assert!(err.contains("package.members: expected an array, found a string"), "{err}");
}
