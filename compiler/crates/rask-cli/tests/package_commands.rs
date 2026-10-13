// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! End-to-end tests for the package-managing commands: `rask add`,
//! `rask remove`, `rask init`, `rask manifest check`, `rask manifest
//! json-schema` and `rask migrate`. Each runs the built binary in its own
//! temporary directory.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// A fresh directory, removed on drop. The pid keeps parallel test binaries
/// (debug and release) apart.
struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rask_pkg_{}_{}", std::process::id(), id)).join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.0.join(name), text).unwrap();
    }

    fn read(&self, name: &str) -> String {
        fs::read_to_string(self.0.join(name)).unwrap()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if let Some(parent) = self.0.parent() {
            let _ = fs::remove_dir_all(parent);
        }
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit {}\n--- stdout\n{}--- stderr\n{}", self.code, self.stdout, self.stderr)
    }
}

fn rask(dir: &Path, args: &[&str]) -> Run {
    let out: Output = Command::new(env!("CARGO_BIN_EXE_rask"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("CLICOLOR_FORCE", "0")
        .output()
        .expect("failed to run rask");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn ok(dir: &Path, args: &[&str]) -> Run {
    let run = rask(dir, args);
    assert_eq!(run.code, 0, "rask {}: {:?}", args.join(" "), run);
    run
}

fn fails(dir: &Path, args: &[&str]) -> Run {
    let run = rask(dir, args);
    assert_eq!(run.code, 1, "rask {} should fail: {:?}", args.join(" "), run);
    run
}

/// `rask manifest check` passes on the directory's rask.toml.
fn assert_loads(dir: &Path) {
    let run = ok(dir, &["manifest", "check"]);
    assert!(run.stdout.contains("OK"), "{:?}", run);
}

const BASE: &str = "\
schema = \"rask-package@1\"   # schema line

# The package
[package]
name    = \"app\"
version = \"1.0.0\"

[dependencies]
# web
http = { registry = \"^2.0\", allow = [\"net\"] }  # pinned major
tls  = { git = \"https://example.com/tls\", rev = \"a1b2c3\" }
# end of deps

[features.ssl]
default = true
";

// ─── rask add ───────────────────────────────────────────────────────

#[test]
fn add_each_form_keeps_the_rest_of_the_file() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", BASE);
    let p = dir.path();

    let run = ok(p, &["add", "json", "^1.5"]);
    assert_eq!(run.stdout, "  Added json = { registry = \"^1.5\" } to [dependencies]\n");
    ok(p, &["add", "--dev", "mock-server", "^2.0"]);
    ok(p, &["add", "openssl", "^3.0", "--feature", "ssl"]);
    ok(p, &["add", "shared", "--path", "../my \"shared\""]);
    ok(p, &["add", "bench", "--dev", "--feature", "perf"]);
    ok(p, &["add", "my.pkg", "=1.0.0"]);

    let expected = "\
schema = \"rask-package@1\"   # schema line

# The package
[package]
name    = \"app\"
version = \"1.0.0\"

[dependencies]
# web
http = { registry = \"^2.0\", allow = [\"net\"] }  # pinned major
tls  = { git = \"https://example.com/tls\", rev = \"a1b2c3\" }
json = { registry = \"^1.5\" }
shared = { path = \"../my \\\"shared\\\"\" }
\"my.pkg\" = { registry = \"=1.0.0\" }
# end of deps

[features.ssl]
default = true

[dev-dependencies]
mock-server = { registry = \"^2.0\" }

[features.ssl.dependencies]
openssl = { registry = \"^3.0\" }

[features.perf.dev-dependencies]
bench = { registry = \"*\" }
";
    assert_eq!(dir.read("rask.toml"), expected);
    assert_loads(p);
}

#[test]
fn add_without_a_manifest_starts_one_named_after_the_directory() {
    let dir = TempDir::new("fresh-app");
    let run = ok(dir.path(), &["add", "http", "^2"]);
    assert!(run.stdout.contains("Created rask.toml") && run.stdout.contains("Added http"), "{:?}", run);
    let text = dir.read("rask.toml");
    assert!(text.starts_with("schema = \"rask-package@"), "{text}");
    assert!(text.contains("[package]\nname = \"fresh-app\"\nversion = \"0.1.0\"\n"), "{text}");
    assert!(text.ends_with("\n\n[dependencies]\nhttp = { registry = \"^2\" }\n"), "{text}");
    assert_loads(dir.path());
}

#[test]
fn add_refuses_a_duplicate_and_leaves_the_file() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", BASE);
    for args in [&["add", "http", "^3"][..], &["add", "tls"], &["add", "--dev", "http"]] {
        let run = fails(dir.path(), args);
        assert!(run.stderr.contains("\"") && run.stderr.contains("already in rask.toml"), "{:?}", run);
        assert!(run.stderr.contains("rask update"), "{:?}", run);
    }
    assert_eq!(dir.read("rask.toml"), BASE);
}

#[test]
fn add_refuses_an_edit_that_would_not_load_and_leaves_the_file() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", BASE);
    let run = fails(dir.path(), &["add", "json", "two"]);
    assert!(run.stderr.contains("wasn't written"), "{:?}", run);
    assert!(run.stderr.contains("dependencies.json.registry"), "names the bad key: {:?}", run);
    assert_eq!(dir.read("rask.toml"), BASE);

    let run = fails(dir.path(), &["add", "json", "1.0", "--path", "../json"]);
    assert!(run.stderr.contains("one source"), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), BASE);
}

#[test]
fn add_refuses_a_manifest_that_does_not_load() {
    let dir = TempDir::new("app");
    let broken = BASE.replace("version = \"1.0.0\"", "version = \"one\"");
    dir.write("rask.toml", &broken);
    let run = fails(dir.path(), &["add", "json", "^1"]);
    assert!(run.stderr.contains("doesn't load, so nothing was added") && run.stderr.contains("package.version"), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), broken);
}

#[test]
fn add_without_a_name_is_a_usage_error() {
    let dir = TempDir::new("app");
    let run = fails(dir.path(), &["add", "--dev"]);
    assert!(run.stderr.contains("missing package name"), "{:?}", run);
    assert!(!dir.path().join("rask.toml").exists());
}

// ─── rask remove ────────────────────────────────────────────────────

const EVERY_TABLE: &str = "\
schema = \"rask-package@1\"

[package]
name = \"app\"
version = \"1.0.0\"

[dependencies]
http = { registry = \"^2\" }  # web
json = { registry = \"^1\" }

[dev-dependencies]
mock = { path = \"../mock\" }

[build-dependencies]
gen = { git = \"https://example.com/gen\", branch = \"main\" }

[dependencies.tls]
git = \"https://example.com/tls\"
rev = \"a1b2c3\"

# Optional TLS
[features.ssl]
default = true

[features.ssl.dependencies]
openssl = { registry = \"^3\" }
";

#[test]
fn remove_from_each_table_keeps_everything_else() {
    let cases: &[(&str, &str)] = &[
        ("http", "http = { registry = \"^2\" }  # web\n"),
        ("mock", "mock = { path = \"../mock\" }\n"),
        ("gen", "gen = { git = \"https://example.com/gen\", branch = \"main\" }\n"),
        ("tls", "[dependencies.tls]\ngit = \"https://example.com/tls\"\nrev = \"a1b2c3\"\n"),
        ("openssl", "openssl = { registry = \"^3\" }\n"),
    ];
    for (name, lines) in cases {
        let dir = TempDir::new("app");
        dir.write("rask.toml", EVERY_TABLE);
        let run = ok(dir.path(), &["remove", name]);
        assert_eq!(run.stdout, format!("  Removed \"{name}\"\n"));
        assert_eq!(dir.read("rask.toml"), EVERY_TABLE.replacen(lines, "", 1), "removing {name}");
        assert_loads(dir.path());
    }
}

#[test]
fn remove_a_name_that_is_not_there() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", EVERY_TABLE);
    for name in ["nope", "ssl", "name"] {
        let run = fails(dir.path(), &["remove", name]);
        assert!(run.stderr.contains(&format!("\"{name}\" isn't a dependency in rask.toml")), "{:?}", run);
    }
    assert_eq!(dir.read("rask.toml"), EVERY_TABLE);
}

#[test]
fn remove_a_quoted_name() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", BASE);
    ok(dir.path(), &["add", "my.pkg", "^1"]);
    ok(dir.path(), &["remove", "my.pkg"]);
    assert_eq!(dir.read("rask.toml"), BASE);
}

#[test]
fn add_then_remove_is_a_round_trip() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", BASE);
    ok(dir.path(), &["add", "json", "^1"]);
    ok(dir.path(), &["add", "openssl", "--feature", "ssl"]);
    ok(dir.path(), &["remove", "json"]);
    ok(dir.path(), &["remove", "openssl"]);
    // The emptied table header stays; every original line is untouched.
    assert_eq!(dir.read("rask.toml"), format!("{BASE}\n[features.ssl.dependencies]\n"));
}

#[test]
fn remove_without_a_manifest() {
    let dir = TempDir::new("app");
    let run = fails(dir.path(), &["remove", "http"]);
    assert!(run.stderr.contains("no rask.toml here"), "{:?}", run);
}

// ─── rask init ──────────────────────────────────────────────────────

#[test]
fn init_writes_a_manifest_that_loads() {
    let dir = TempDir::new("work");
    // A .git above keeps init from running `git init` inside the temp dir.
    fs::create_dir(dir.path().join(".git")).unwrap();
    let run = ok(dir.path(), &["init", "hello"]);
    assert!(run.stdout.contains("Created rask.toml") && run.stdout.contains("Created main.rk"), "{:?}", run);
    let app = dir.path().join("hello");
    let text = fs::read_to_string(app.join("rask.toml")).unwrap();
    let schema_version = text
        .strip_prefix("schema = \"rask-package@")
        .and_then(|r| r.split('"').next())
        .unwrap_or_else(|| panic!("no schema line:\n{text}"));
    // The current version: one past it is refused.
    let next: u32 = schema_version.parse::<u32>().unwrap() + 1;
    let run = fails(&app, &["migrate", "--to", &next.to_string()]);
    assert!(run.stderr.contains(&format!("no @{next}")), "{:?}", run);
    assert!(text.contains("name = \"hello\"\nversion = \"0.1.0\""), "{text}");
    assert_loads(&app);
    assert!(app.join("main.rk").exists() && app.join(".gitignore").exists());
    assert!(!app.join(".git").exists(), "inside a repository already, so no nested one");
}

#[test]
fn init_names_the_package_after_the_last_path_component() {
    let dir = TempDir::new("work");
    fs::create_dir(dir.path().join(".git")).unwrap();
    ok(dir.path(), &["init", "nested/deeper/tool"]);
    let text = fs::read_to_string(dir.path().join("nested/deeper/tool/rask.toml")).unwrap();
    assert!(text.contains("name = \"tool\""), "{text}");
}

#[test]
fn init_does_not_nest_a_repository_from_a_subdirectory() {
    let dir = TempDir::new("work");
    fs::create_dir(dir.path().join(".git")).unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir(&sub).unwrap();
    ok(&sub, &["init", "app"]);
    assert!(!sub.join("app/.git").exists(), "a repository above the working directory counts");
}

#[test]
fn init_in_place_uses_the_directory_name() {
    let dir = TempDir::new("in-place");
    fs::create_dir(dir.path().join(".git")).unwrap();
    dir.write("main.rk", "func main() {}\n");
    let run = ok(dir.path(), &["init"]);
    assert!(run.stdout.contains("Skipped main.rk"), "{:?}", run);
    assert!(dir.read("rask.toml").contains("name = \"in-place\""));
    assert_eq!(dir.read("main.rk"), "func main() {}\n", "an existing main.rk is kept");
    assert_loads(dir.path());
}

#[test]
fn init_refuses_an_existing_directory_or_manifest() {
    let dir = TempDir::new("work");
    fs::create_dir(dir.path().join("taken")).unwrap();
    let run = fails(dir.path(), &["init", "taken"]);
    assert!(run.stderr.contains("directory 'taken' already exists"), "{:?}", run);
    assert!(!dir.path().join("taken/rask.toml").exists());

    dir.write("rask.toml", BASE);
    let run = fails(dir.path(), &["init"]);
    assert!(run.stderr.contains("rask.toml already exists"), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), BASE);
    assert!(!dir.path().join("main.rk").exists());
}

// ─── rask manifest check / json-schema ──────────────────────────────

#[test]
fn manifest_check_prints_every_problem() {
    let dir = TempDir::new("app");
    dir.write(
        "rask.toml",
        "schema = \"rask-package@1\"\n\n[package]\nname = \"x\"\nversion = \"one\"\nlicnse = \"MIT\"\n\n[dependencies]\nhttp = { registry = \"^2\", path = \"../h\" }\njson = { registry = \"two\" }\ntls = { git = \"u\", revv = \"a\" }\n",
    );
    let run = fails(dir.path(), &["manifest", "check"]);
    for problem in [
        "package.version: \"one\" is not a version",
        "package: unknown key `licnse`; did you mean `license`?",
        "dependencies.http: holds `registry`, `path`; pick exactly one of `registry`, `path`, `git`",
        "dependencies.json.registry: \"two\" is not a version range",
        "dependencies.tls: unknown key `revv`; did you mean `rev`?",
    ] {
        assert!(run.stderr.contains(problem), "missing `{problem}`: {:?}", run);
    }
    assert!(run.stdout.is_empty(), "{:?}", run);
}

#[test]
fn manifest_check_passes_a_good_manifest_in_another_directory() {
    let work = TempDir::new("work");
    let app = work.path().join("app");
    fs::create_dir(&app).unwrap();
    fs::write(app.join("rask.toml"), BASE).unwrap();
    let run = ok(work.path(), &["manifest", "check", "app"]);
    assert!(run.stdout.contains("OK app 1.0.0"), "{:?}", run);
}

#[test]
fn manifest_check_rejects_a_newer_schema_and_a_missing_file() {
    let dir = TempDir::new("app");
    let run = fails(dir.path(), &["manifest", "check"]);
    assert!(run.stderr.contains("no rask.toml in ."), "{:?}", run);
    dir.write("rask.toml", &BASE.replace("rask-package@1", "rask-package@999"));
    let run = fails(dir.path(), &["manifest", "check"]);
    assert!(run.stderr.contains("999"), "{:?}", run);
}

#[test]
fn manifest_json_schema_describes_the_current_shape() {
    let dir = TempDir::new("app");
    let run = ok(dir.path(), &["manifest", "json-schema"]);
    let s = run.stdout.trim();
    assert!(s.starts_with('{') && s.ends_with('}'), "{s}");
    assert!(s.contains("\"$schema\": \"http://json-schema.org/draft-07/schema#\""), "{s}");
    for key in ["package", "dependencies", "dev-dependencies", "build-dependencies", "features", "profiles"] {
        assert!(s.contains(&format!("\"{key}\": {{")), "no `{key}` in the schema");
    }
    assert_eq!(s.matches('{').count(), s.matches('}').count());
}

#[test]
fn manifest_without_a_subcommand_or_with_an_unknown_one() {
    let dir = TempDir::new("app");
    let run = rask(dir.path(), &["manifest", "frobnicate"]);
    assert_eq!(run.code, 2, "{:?}", run);
    assert!(run.stderr.contains("unknown `rask manifest` command `frobnicate`"), "{:?}", run);
}

// ─── rask migrate ───────────────────────────────────────────────────

#[test]
fn migrate_rewrites_and_the_result_loads() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", BASE);
    let run = ok(dir.path(), &["migrate", "--to", "1"]);
    assert!(run.stdout.contains("Migrated") && run.stdout.contains("rask-package@1"), "{:?}", run);
    assert!(run.stderr.contains("drops its comments"), "warns before losing comments: {:?}", run);
    let text = dir.read("rask.toml");
    assert!(text.starts_with("schema = \"rask-package@1\"\n"), "{text}");
    assert!(!text.contains('#'), "{text}");
    for kept in ["name = \"app\"", "version = \"1.0.0\"", "http = ", "tls = ", "rev = \"a1b2c3\"", "default = true"] {
        assert!(text.contains(kept), "lost `{kept}`:\n{text}");
    }
    assert_loads(dir.path());

    // Without comments there is nothing to warn about.
    let run = ok(dir.path(), &["migrate", "--to", "1"]);
    assert!(!run.stderr.contains("drops its comments"), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), text, "migrating to the version it's at is stable");
}

#[test]
fn migrate_refuses_a_version_that_does_not_exist() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", BASE);
    for to in ["0", "999"] {
        let run = fails(dir.path(), &["migrate", "--to", to]);
        assert!(run.stderr.contains(&format!("no @{to}")), "{:?}", run);
    }
    assert_eq!(dir.read("rask.toml"), BASE);
}

#[test]
fn migrate_argument_errors() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", BASE);
    let run = rask(dir.path(), &["migrate"]);
    assert_eq!(run.code, 2, "{:?}", run);
    assert!(run.stderr.contains("missing `--to <version>`"), "{:?}", run);
    let run = rask(dir.path(), &["migrate", "--to", "two"]);
    assert_eq!(run.code, 2, "{:?}", run);
    assert!(run.stderr.contains("`--to` takes a whole number, got `two`"), "{:?}", run);
    let run = fails(dir.path(), &["migrate", "--to", "1", "elsewhere"]);
    assert!(run.stderr.contains("no rask.toml in elsewhere"), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), BASE);
}

#[test]
fn migrate_refuses_a_manifest_that_does_not_load() {
    let dir = TempDir::new("app");
    let broken = BASE.replace("version = \"1.0.0\"", "version = \"one\"");
    dir.write("rask.toml", &broken);
    let run = fails(dir.path(), &["migrate", "--to", "1"]);
    assert!(run.stderr.contains("package.version"), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), broken);
}
