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

// ─── A local registry ───────────────────────────────────────────────
// `rask add` asks a registry for versions and capabilities, and the lock
// update downloads what it names. Tests get a registry of their own: an
// HTTP server on a loopback port serving index.json files and tarballs from
// a directory, the way the real one serves them (struct.packages/RG1).

/// One published version: name, version, capabilities, deps as (name, range).
struct Published {
    name: &'static str,
    version: &'static str,
    capabilities: &'static [&'static str],
    deps: &'static [(&'static str, &'static str)],
}

const PUBLISHED: &[Published] = &[
    Published { name: "http", version: "2.0.0", capabilities: &["net"], deps: &[] },
    Published { name: "http", version: "2.1.0", capabilities: &["net"], deps: &[("json", "^1.0")] },
    Published { name: "http", version: "3.0.0-beta.1", capabilities: &["net"], deps: &[] },
    Published { name: "json", version: "1.0.0", capabilities: &[], deps: &[] },
    Published { name: "json", version: "1.5.2", capabilities: &[], deps: &[] },
    Published { name: "mock-server", version: "2.0.0", capabilities: &[], deps: &[] },
    Published { name: "openssl", version: "3.0.1", capabilities: &[], deps: &[] },
    Published { name: "bench", version: "0.4.1", capabilities: &[], deps: &[] },
    Published { name: "my.pkg", version: "1.0.0", capabilities: &[], deps: &[] },
    Published { name: "sockets", version: "1.2.0", capabilities: &["net", "ffi"], deps: &[] },
];

struct LocalRegistry {
    url: String,
    /// The cache every test shares. Packages are immutable once published, so
    /// sharing it is safe and saves the downloads.
    cache: PathBuf,
}

fn registry() -> &'static LocalRegistry {
    static REGISTRY: std::sync::OnceLock<LocalRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("rask_registry_{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let served = root.join("served");
        let mut indexes: std::collections::BTreeMap<&str, serde_json::Map<String, serde_json::Value>> = Default::default();
        for p in PUBLISHED {
            // The package's source, as its author would have it.
            let src = root.join("src").join(format!("{}-{}", p.name, p.version));
            fs::create_dir_all(&src).unwrap();
            let mut manifest = format!(
                "schema = \"rask-package@1\"\n\n[package]\nname = \"{}\"\nversion = \"{}\"\n",
                p.name, p.version
            );
            if !p.deps.is_empty() {
                manifest.push_str("\n[dependencies]\n");
                for (d, r) in p.deps {
                    manifest.push_str(&format!("{d} = {{ registry = \"{r}\" }}\n"));
                }
            }
            fs::write(src.join("rask.toml"), manifest).unwrap();
            fs::write(src.join("lib.rk"), "public func version() -> i64 {\n    return 1\n}\n").unwrap();

            let dir = served.join("pkg").join(p.name);
            fs::create_dir_all(&dir).unwrap();
            rask_resolve::tarball::create_reproducible_tarball(&src, &dir.join(format!("{}.tar.gz", p.version))).unwrap();
            let deps: Vec<serde_json::Value> =
                p.deps.iter().map(|(d, r)| serde_json::json!({ "name": d, "version": r })).collect();
            indexes.entry(p.name).or_default().insert(
                p.version.to_string(),
                serde_json::json!({
                    "checksum": rask_resolve::lockfile::compute_checksum(&src),
                    "deps": deps,
                    "capabilities": p.capabilities,
                }),
            );
        }
        for (name, versions) in indexes {
            let index = serde_json::json!({ "name": name, "versions": versions });
            fs::write(served.join("pkg").join(name).join("index.json"), index.to_string()).unwrap();
        }

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let served = served.clone();
                std::thread::spawn(move || serve(stream, &served));
            }
        });
        LocalRegistry { url, cache: root.join("cache") }
    })
}

/// Answer one GET with the file under `root`, or a 404.
fn serve(mut stream: std::net::TcpStream, root: &Path) {
    use std::io::{BufRead, BufReader, Write};
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut request = String::new();
    if reader.read_line(&mut request).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).is_err() || header == "\r\n" || header.is_empty() {
            break;
        }
    }
    let path = request.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/');
    let reply = match fs::read(root.join(path)) {
        Ok(body) => {
            let mut r = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
            r.extend(body);
            r
        }
        Err(_) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    };
    let _ = stream.write_all(&reply);
}

fn rask(dir: &Path, args: &[&str]) -> Run {
    let reg = registry();
    let out: Output = Command::new(env!("CARGO_BIN_EXE_rask"))
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .env("CLICOLOR_FORCE", "0")
        .env("RASK_REGISTRY", &reg.url)
        .env("RASK_CACHE", &reg.cache)
        // Bypasses any agent proxy for the loopback registry.
        .env("NO_PROXY", "127.0.0.1")
        .env("no_proxy", "127.0.0.1")
        .stdin(std::process::Stdio::null())
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

/// A minimal package at `dir`, so a path dependency on it resolves.
fn write_package(dir: &Path, name: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(dir.join("rask.toml"), format!("schema = \"rask-package@1\"\n\n[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n")).unwrap();
    fs::write(dir.join("lib.rk"), "public func f() -> i64 {\n    return 1\n}\n").unwrap();
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

    write_package(&p.join("../my \"shared\""), "shared");
    let run = ok(p, &["add", "json", "^1.5"]);
    assert!(run.stdout.starts_with("  Added json = { registry = \"^1.5\" } to [dependencies]\n  Updated rask.lock"), "{:?}", run);
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
bench = { registry = \"^0.4.1\" }
";
    assert_eq!(dir.read("rask.toml"), expected);
    assert_loads(p);
}

#[test]
fn add_without_a_manifest_starts_one_named_after_the_directory() {
    let dir = TempDir::new("fresh-app");
    let run = ok(dir.path(), &["add", "http", "^2", "--allow", "net"]);
    assert!(run.stdout.contains("Created rask.toml") && run.stdout.contains("Added http"), "{:?}", run);
    let text = dir.read("rask.toml");
    assert!(text.starts_with("schema = \"rask-package@"), "{text}");
    assert!(text.contains("[package]\nname = \"fresh-app\"\nversion = \"0.1.0\"\n"), "{text}");
    assert!(text.ends_with("\n\n[dependencies]\nhttp = { registry = \"^2\", allow = [\"net\"] }\n"), "{text}");
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
        write_package(&dir.path().join("../mock"), "mock");
        let run = ok(dir.path(), &["remove", name]);
        assert!(run.stdout.starts_with(&format!("  Removed \"{name}\"\n")), "{:?}", run);
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

// ─── Registry lookup, capabilities and the lock ─────────────────────

const PLAIN: &str = "schema = \"rask-package@1\"\n\n[package]\nname = \"app\"\nversion = \"1.0.0\"\n";

#[test]
fn add_without_a_version_writes_the_newest_stable_release() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", PLAIN);
    let run = ok(dir.path(), &["add", "json"]);
    assert!(run.stdout.contains("Added json = { registry = \"^1.5.2\" }"), "{:?}", run);
    // 3.0.0-beta.1 is newer, but a pre-release is only ever named exactly.
    let run = ok(dir.path(), &["add", "http", "--allow", "net"]);
    assert!(run.stdout.contains("Added http = { registry = \"^2.1.0\", allow = [\"net\"] }"), "{:?}", run);
    assert_loads(dir.path());
}

#[test]
fn add_asks_for_capabilities_and_without_a_terminal_wants_them_named() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", PLAIN);
    let run = fails(dir.path(), &["add", "sockets"]);
    assert!(run.stderr.contains("\"sockets\" uses net (") && run.stderr.contains("pass `--allow net,ffi` to grant it"), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), PLAIN);
    assert!(!dir.path().join("rask.lock").exists());

    let run = ok(dir.path(), &["add", "sockets", "--allow", "net,ffi"]);
    assert!(run.stdout.contains("sockets = { registry = \"^1.2.0\", allow = [\"net\", \"ffi\"] }"), "{:?}", run);
}

#[test]
fn add_and_remove_keep_the_lock_up_to_date() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", PLAIN);
    ok(dir.path(), &["add", "json", "^1.0"]);
    let lock = dir.read("rask.lock");
    assert!(lock.contains("name = \"json\"\nversion = \"1.5.2\""), "{lock}");
    assert!(lock.contains("checksum = \"sha256:"), "{lock}");

    let run = ok(dir.path(), &["remove", "json"]);
    assert!(run.stdout.contains("Removed rask.lock (no dependencies left)"), "{:?}", run);
    assert!(!dir.path().join("rask.lock").exists());
}

#[test]
fn remove_notes_a_dependency_something_else_still_needs() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", PLAIN);
    ok(dir.path(), &["add", "http", "^2", "--allow", "net"]);
    ok(dir.path(), &["add", "json", "^1"]);
    let run = ok(dir.path(), &["remove", "json"]);
    assert!(run.stdout.contains("Note: \"json\" stays in rask.lock: http still depends on it"), "{:?}", run);
    assert!(dir.read("rask.lock").contains("name = \"json\""));
}

#[test]
fn add_puts_the_manifest_back_when_the_lock_cannot_be_updated() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", PLAIN);
    let run = fails(dir.path(), &["add", "ghost", "--path", "../ghost"]);
    assert!(run.stderr.contains("couldn't update rask.lock, so rask.toml is unchanged"), "{:?}", run);
    assert!(run.stderr.contains("dep \"ghost\": path not found"), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), PLAIN);

    // Without a rask.toml to start from, there's none afterwards either.
    let fresh = TempDir::new("fresh");
    fails(fresh.path(), &["add", "ghost", "--path", "../ghost"]);
    assert!(!fresh.path().join("rask.toml").exists());
}

#[test]
fn add_names_a_package_the_registry_does_not_have() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", PLAIN);
    let run = fails(dir.path(), &["add", "nothing-here"]);
    assert!(run.stderr.contains("can't look up \"nothing-here\""), "{:?}", run);
    assert_eq!(dir.read("rask.toml"), PLAIN);
}

#[test]
fn update_keeps_registry_packages_and_dev_dependencies_in_the_lock() {
    let dir = TempDir::new("app");
    write_package(&dir.path().join("../mock"), "mock");
    dir.write("rask.toml", &format!("{PLAIN}\n[dependencies]\njson = {{ registry = \"^1\" }}\n\n[dev-dependencies]\nmock = {{ path = \"../mock\" }}\n"));
    ok(dir.path(), &["update"]);
    let lock = dir.read("rask.lock");
    assert!(lock.contains("name = \"json\""), "{lock}");
    assert!(lock.contains("name = \"mock\"\nversion = \"0.1.0\"\nsource = \"path+../mock\"\n"), "no checksum on a path entry: {lock}");
}

#[test]
fn a_release_build_writes_a_lock_with_the_dev_dependencies_it_skips() {
    let dir = TempDir::new("app");
    write_package(&dir.path().join("../mock"), "mock");
    dir.write("rask.toml", &format!("{PLAIN}\n[dev-dependencies]\nmock = {{ path = \"../mock\" }}\n"));
    dir.write("main.rk", "func main() {\n    println(\"hi\")\n}\n");
    ok(dir.path(), &["build", "--release"]);
    assert!(dir.read("rask.lock").contains("name = \"mock\""));
    // And the next release build, which doesn't link mock, still accepts it.
    ok(dir.path(), &["build", "--release"]);
}

#[test]
fn a_program_imports_a_registry_dependency() {
    let dir = TempDir::new("app");
    dir.write("rask.toml", PLAIN);
    dir.write("main.rk", "import json\n\nfunc main() {\n    println(\"json {json.version()}\")\n}\n");
    ok(dir.path(), &["add", "json", "^1"]);
    let run = ok(dir.path(), &["run", "."]);
    assert!(run.stdout.contains("json 1"), "{:?}", run);
}

#[test]
fn a_build_refuses_a_lock_entry_nothing_depends_on() {
    let dir = TempDir::new("app");
    write_package(&dir.path().join("../mock"), "mock");
    dir.write("rask.toml", &format!("{PLAIN}\n[dependencies]\nmock = {{ path = \"../mock\" }}\n"));
    dir.write("main.rk", "func main() {\n    println(\"hi\")\n}\n");
    ok(dir.path(), &["update"]);
    let lock = dir.read("rask.lock");
    dir.write("rask.lock", &format!("{lock}\n[[package]]\nname = \"gone\"\nversion = \"1.0.0\"\nsource = \"path+../gone\"\n"));
    let run = fails(dir.path(), &["build"]);
    assert!(run.stderr.contains("rask.lock pins 'gone', which nothing depends on any more — run `rask update`"), "{:?}", run);
    ok(dir.path(), &["update"]);
    ok(dir.path(), &["build"]);
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
