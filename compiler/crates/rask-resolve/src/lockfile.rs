// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Lock file generation and verification — LK1-LK5, LK8.
//!
//! Pins exact dependency versions for reproducible builds.
//! SHA-256 checksums for integrity. Capabilities field per
//! package for permission tracking (PM1-PM8). Signing key
//! fingerprint per registry package for TOFU (LK8).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Sha256, Digest};

use super::package::{PackageId, PackageRegistry};

/// Current lock file format version.
const LOCKFILE_VERSION: u32 = 1;

/// A locked package entry.
#[derive(Debug, Clone)]
pub struct LockedPackage {
    pub name: String,
    pub version: String,
    pub source: String,
    pub checksum: String,
    /// Inferred capabilities: "net", "read", "write", "exec", "ffi".
    pub capabilities: Vec<String>,
    /// Ed25519 signing key fingerprint for registry packages (LK8).
    /// `None` for path/git deps or pre-signing packages (SG7).
    pub signing_key: Option<String>,
}

/// The full lock file.
#[derive(Debug, Default)]
pub struct LockFile {
    pub version: u32,
    pub packages: Vec<LockedPackage>,
}

impl LockFile {
    /// Generate a lock file from the current package registry.
    /// `root_dir` is the root package directory — used to compute relative source paths.
    pub fn generate(registry: &PackageRegistry, root_id: PackageId, root_dir: &Path) -> Self {
        Self::generate_workspace(registry, &[root_id], root_dir)
    }

    /// Generate with capability information from the build.
    pub fn generate_with_capabilities(
        registry: &PackageRegistry,
        root_id: PackageId,
        root_dir: &Path,
        caps: &BTreeMap<String, Vec<String>>,
    ) -> Self {
        Self::generate_workspace_with_capabilities(registry, &[root_id], root_dir, caps)
    }

    /// Generate lock file for a workspace with multiple root packages (WS2).
    /// All members' external deps go into a single lock file.
    pub fn generate_workspace(
        registry: &PackageRegistry,
        root_ids: &[PackageId],
        root_dir: &Path,
    ) -> Self {
        let mut packages = Vec::new();
        let mut seen = std::collections::HashSet::new();

        for pkg in registry.packages() {
            if root_ids.contains(&pkg.id) { continue; }
            if !pkg.is_external { continue; }
            if !seen.insert(pkg.name.clone()) { continue; }

            let version = pkg.manifest.as_ref()
                .map(|m| m.version.clone())
                .unwrap_or_else(|| "0.0.0".into());

            let source = if let Some(ref url) = pkg.registry_source {
                format!("registry+{}", url)
            } else {
                let rel_path = diff_paths(&pkg.root_dir, root_dir)
                    .unwrap_or_else(|| pkg.root_dir.clone());
                format!("path+{}", rel_path.display())
            };
            let checksum = compute_checksum(&pkg.root_dir);

            packages.push(LockedPackage {
                name: pkg.name.clone(),
                version,
                source,
                checksum,
                capabilities: Vec::new(),
                signing_key: None,
            });
        }

        packages.sort_by(|a, b| a.name.cmp(&b.name));
        LockFile { version: LOCKFILE_VERSION, packages }
    }

    /// Generate workspace lock file with capabilities.
    pub fn generate_workspace_with_capabilities(
        registry: &PackageRegistry,
        root_ids: &[PackageId],
        root_dir: &Path,
        caps: &BTreeMap<String, Vec<String>>,
    ) -> Self {
        let mut lock = Self::generate_workspace(registry, root_ids, root_dir);
        for pkg in &mut lock.packages {
            if let Some(pkg_caps) = caps.get(&pkg.name) {
                pkg.capabilities = pkg_caps.clone();
            }
        }
        lock
    }

    /// Load a lock file from disk. Errors name the line.
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read lock file: {}", e))?;
        parse(&content, &path.display().to_string())
    }

    /// Write the lock file to disk.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.render())
            .map_err(|e| format!("failed to write lock file: {}", e))
    }

    fn render(&self) -> String {
        let mut content = String::from("# rask.lock — auto-generated, do not edit\n");
        content.push_str(&format!("lockfile-version = {}\n\n", LOCKFILE_VERSION));

        for pkg in &self.packages {
            content.push_str("[[package]]\n");
            content.push_str(&format!("name = {}\n", quote(&pkg.name)));
            content.push_str(&format!("version = {}\n", quote(&pkg.version)));
            content.push_str(&format!("source = {}\n", quote(&pkg.source)));
            content.push_str(&format!("checksum = {}\n", quote(&pkg.checksum)));
            if let Some(ref key) = pkg.signing_key {
                content.push_str(&format!("signing-key = {}\n", quote(key)));
            }
            if !pkg.capabilities.is_empty() {
                let caps: Vec<String> = pkg.capabilities.iter().map(|c| quote(c)).collect();
                content.push_str(&format!("capabilities = [{}]\n", caps.join(", ")));
            }
            content.push('\n');
        }
        content
    }

    /// Verify that locked checksums match current state.
    pub fn verify(&self, registry: &PackageRegistry, root_id: PackageId) -> Result<(), String> {
        let locked: BTreeMap<&str, &LockedPackage> = self.packages.iter()
            .map(|p| (p.name.as_str(), p))
            .collect();

        for pkg in registry.packages() {
            if pkg.id == root_id { continue; }
            if !pkg.is_external { continue; }

            if let Some(locked_pkg) = locked.get(pkg.name.as_str()) {
                let current_checksum = compute_checksum(&pkg.root_dir);
                if current_checksum != locked_pkg.checksum {
                    return Err(format!(
                        "dependency '{}' has changed (checksum mismatch) — run `rask update`",
                        pkg.name,
                    ));
                }
            } else {
                return Err(format!(
                    "dependency '{}' not in rask.lock — run `rask update`",
                    pkg.name,
                ));
            }
        }

        Ok(())
    }

    /// Check if any package's capabilities changed versus what's locked.
    pub fn capabilities_changed(&self, caps: &BTreeMap<String, Vec<String>>) -> Vec<String> {
        let mut changed = Vec::new();
        for pkg in &self.packages {
            if let Some(new_caps) = caps.get(&pkg.name) {
                let mut old_sorted = pkg.capabilities.clone();
                let mut new_sorted = new_caps.clone();
                old_sorted.sort();
                new_sorted.sort();
                if old_sorted != new_sorted {
                    changed.push(pkg.name.clone());
                }
            }
        }
        changed
    }

    /// Check if any registry package's signing key changed versus what's locked (LK8).
    /// Returns package names where the key changed — callers should warn.
    pub fn signing_keys_changed(
        &self,
        new_keys: &BTreeMap<String, String>,
    ) -> Vec<(String, String, String)> {
        let mut changed = Vec::new();
        for pkg in &self.packages {
            if let Some(ref old_key) = pkg.signing_key {
                if let Some(new_key) = new_keys.get(&pkg.name) {
                    if old_key != new_key {
                        changed.push((
                            pkg.name.clone(),
                            old_key.clone(),
                            new_key.clone(),
                        ));
                    }
                }
            }
        }
        changed
    }

    /// True if there are no locked packages.
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }
}

/// Compute SHA-256 checksum of all .rk source files in a directory (recursive).
/// Uses paths relative to the package root for cross-machine reproducibility.
pub fn compute_checksum(dir: &Path) -> String {
    let mut hasher = Sha256::new();

    // Collect and sort source files for determinism
    let mut files: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();
    collect_rk_files_recursive(dir, dir, &mut files);

    for (rel_path, content) in &files {
        // Hash relative path (not absolute) for reproducibility across machines
        hasher.update(rel_path.to_string_lossy().as_bytes());
        hasher.update(content);
    }

    let result = hasher.finalize();
    format!("sha256:{:x}", result)
}

/// Recursively collect all .rk files, storing relative paths.
fn collect_rk_files_recursive(
    base: &Path,
    dir: &Path,
    files: &mut BTreeMap<PathBuf, Vec<u8>>,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();

        if path.is_dir() {
            // Skip build output and hidden directories
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.starts_with('.') || name == "build" || name == "vendor" {
                continue;
            }
            collect_rk_files_recursive(base, &path, files);
        } else if path.extension().map(|e| e == "rk").unwrap_or(false) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "build.rk" { continue; }

            if let Ok(content) = std::fs::read(&path) {
                // Store with relative path for cross-machine reproducibility
                let rel = path.strip_prefix(base).unwrap_or(&path).to_path_buf();
                files.insert(rel, content);
            }
        }
    }
}

const PACKAGE_KEYS: [&str; 6] = ["name", "version", "source", "checksum", "signing-key", "capabilities"];

/// A lock file entry while it's being read: each key with its line.
struct Entry {
    line: usize,
    fields: BTreeMap<String, (usize, Value)>,
}

enum Value {
    Str(String),
    List(Vec<String>),
}

/// Read rask.lock's text. `file` names it in errors.
fn parse(content: &str, file: &str) -> Result<LockFile, String> {
    let at = |n: usize, msg: String| format!("{}:{}: {}", file, n, msg);
    let mut entries: Vec<Entry> = Vec::new();
    let mut file_version: Option<u32> = None;

    for (i, raw) in content.lines().enumerate() {
        let n = i + 1;
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line == "[[package]]" {
            entries.push(Entry { line: n, fields: BTreeMap::new() });
            continue;
        }
        if line.starts_with('[') {
            return Err(at(n, format!("unknown table `{}`; rask.lock has only [[package]] entries", line)));
        }
        let Some((key, value)) = line.split_once('=') else {
            return Err(at(n, format!("expected `key = value`, found `{}`", line)));
        };
        let key = key.trim();
        let value = value.trim();
        let Some(entry) = entries.last_mut() else {
            if key != "lockfile-version" {
                return Err(at(n, format!("`{}` belongs inside a [[package]] entry", key)));
            }
            if file_version.is_some() {
                return Err(at(n, "`lockfile-version` is set twice".into()));
            }
            let v: u32 = value.parse()
                .map_err(|_| at(n, format!("`lockfile-version` is a whole number, found `{}`", value)))?;
            if v > LOCKFILE_VERSION {
                return Err(format!(
                    "rask.lock version {} is newer than supported ({}). Update your compiler",
                    v, LOCKFILE_VERSION,
                ));
            }
            if v != LOCKFILE_VERSION {
                return Err(at(n, format!(
                    "lockfile-version {} isn't supported (this toolchain reads {}); run `rask update`",
                    v, LOCKFILE_VERSION,
                )));
            }
            file_version = Some(v);
            continue;
        };
        if !PACKAGE_KEYS.contains(&key) {
            return Err(at(n, format!("unknown key `{}`; a [[package]] entry holds {}", key, PACKAGE_KEYS.join(", "))));
        }
        let parsed = if key == "capabilities" {
            parse_string_array(value).map(Value::List)
        } else {
            parse_string(value).map(Value::Str)
        };
        let parsed = parsed.ok_or_else(|| {
            let shape = if key == "capabilities" { "a list of quoted strings" } else { "a quoted string" };
            at(n, format!("`{}` must be {}, found `{}`", key, shape, value))
        })?;
        if let Some((first, _)) = entry.fields.insert(key.to_string(), (n, parsed)) {
            return Err(at(n, format!("`{}` is set twice in one entry (first on line {})", key, first)));
        }
    }

    if file_version.is_none() {
        return Err(format!("{}: no `lockfile-version` line; run `rask update` to regenerate it", file));
    }
    let packages = entries.into_iter()
        .map(|e| to_locked(e).map_err(|msg| at_entry(file, msg)))
        .collect::<Result<_, _>>()?;
    Ok(LockFile { version: LOCKFILE_VERSION, packages })
}

fn at_entry(file: &str, (line, msg): (usize, String)) -> String {
    format!("{}:{}: {}", file, line, msg)
}

impl Entry {
    fn string(&mut self, key: &str) -> Option<String> {
        match self.fields.remove(key) {
            Some((_, Value::Str(s))) => Some(s),
            _ => None,
        }
    }

    fn required(&mut self, key: &str) -> Result<String, (usize, String)> {
        self.string(key).ok_or_else(|| (self.line, format!("this [[package]] entry has no `{}`", key)))
    }
}

fn to_locked(mut e: Entry) -> Result<LockedPackage, (usize, String)> {
    Ok(LockedPackage {
        name: e.required("name")?,
        version: e.required("version")?,
        source: e.required("source")?,
        checksum: e.required("checksum")?,
        signing_key: e.string("signing-key"),
        capabilities: match e.fields.remove("capabilities") {
            Some((_, Value::List(l))) => l,
            _ => Vec::new(),
        },
    })
}

/// A TOML basic string, quotes and escapes included.
fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// The inside of a `quote`d string; None unless `s` is exactly one.
fn parse_string(s: &str) -> Option<String> {
    let (value, rest) = take_string(s)?;
    rest.trim().is_empty().then_some(value)
}

/// One quoted string at the start of `s`, and what follows it.
fn take_string(s: &str) -> Option<(String, &str)> {
    let inner = s.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = inner.char_indices();
    while let Some((i, c)) = chars.next() {
        match c {
            '"' => return Some((out, &inner[i + 1..])),
            '\\' => match chars.next()?.1 {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                _ => return None,
            },
            _ => out.push(c),
        }
    }
    None
}

/// Compute a relative path from `base` to `target`.
/// Falls back to returning `target` unchanged if both paths aren't absolute.
fn diff_paths(target: &Path, base: &Path) -> Option<PathBuf> {
    let target = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let base = std::fs::canonicalize(base).unwrap_or_else(|_| base.to_path_buf());

    let mut target_iter = target.components().peekable();
    let mut base_iter = base.components().peekable();

    // Skip common prefix
    while let (Some(a), Some(b)) = (target_iter.peek(), base_iter.peek()) {
        if a != b { break; }
        target_iter.next();
        base_iter.next();
    }

    // Go up for remaining base components
    let mut result = PathBuf::new();
    for _ in base_iter {
        result.push("..");
    }
    // Append remaining target components
    for component in target_iter {
        result.push(component);
    }

    Some(result)
}

/// `["a", "b"]`; None for anything else.
fn parse_string_array(s: &str) -> Option<Vec<String>> {
    let mut rest = s.trim().strip_prefix('[')?.trim_start();
    let mut out = Vec::new();
    loop {
        if let Some(after) = rest.strip_prefix(']') {
            return after.trim().is_empty().then_some(out);
        }
        let (item, after) = take_string(rest)?;
        out.push(item);
        let after = after.trim_start();
        rest = match after.strip_prefix(',') {
            Some(r) => r.trim_start(),
            None if after.starts_with(']') => after,
            None => return None,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rask_ast::decl::PackageDecl;
    use std::fs;
    use tempfile::TempDir;

    fn write_files(dir: &Path, files: &[(&str, &str)]) {
        for (name, text) in files {
            let path = dir.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
    }

    fn manifest(name: &str, version: &str) -> PackageDecl {
        PackageDecl {
            name: name.into(),
            version: version.into(),
            deps: Vec::new(),
            features: Vec::new(),
            metadata: Vec::new(),
            list_metadata: Vec::new(),
            profiles: Vec::new(),
        }
    }

    /// An app at `<tmp>/app` with a path dep `<tmp>/libs/json`, a registry
    /// dep cached at `<tmp>/cache/http`, and an internal subpackage.
    struct Fixture {
        tmp: TempDir,
        registry: PackageRegistry,
        root: PackageId,
    }

    impl Fixture {
        fn new() -> Self {
            let tmp = TempDir::new().unwrap();
            let app = tmp.path().join("app");
            let json = tmp.path().join("libs/json");
            let http = tmp.path().join("cache/http");
            write_files(&app, &[("main.rk", "func main() {}\n")]);
            write_files(&json, &[("lib.rk", "public func parse() {}\n")]);
            write_files(&http, &[("client.rk", "public func get() {}\n")]);

            let mut registry = PackageRegistry::new();
            let root = registry.add_package("app".into(), vec!["app".into()], app.clone());
            registry.add_package("util".into(), vec!["app".into(), "util".into()], app.join("util"));
            let json_id = registry.add_package("json".into(), vec!["json".into()], json);
            let pkg = registry.get_mut(json_id).unwrap();
            pkg.is_external = true;
            pkg.manifest = Some(manifest("json", "1.4.2"));
            registry
                .register_cached("http", "2.1.0", &http, "https://packages.rask-lang.dev")
                .unwrap();
            Fixture { tmp, registry, root }
        }

        fn app_dir(&self) -> PathBuf {
            self.tmp.path().join("app")
        }

        fn lock(&self) -> LockFile {
            LockFile::generate(&self.registry, self.root, &self.app_dir())
        }
    }

    fn locked(name: &str) -> LockedPackage {
        LockedPackage {
            name: name.into(),
            version: "1.0.0".into(),
            source: format!("path+../{}", name),
            checksum: "sha256:00".into(),
            capabilities: Vec::new(),
            signing_key: None,
        }
    }

    fn caps(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs.iter()
            .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
            .collect()
    }

    // ─── generate ───────────────────────────────────────────────────

    #[test]
    fn generate_pins_external_packages_only() {
        let f = Fixture::new();
        let lock = f.lock();
        assert_eq!(lock.version, LOCKFILE_VERSION);
        let names: Vec<&str> = lock.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["http", "json"], "sorted, without the root or its subpackage");

        let http = &lock.packages[0];
        assert_eq!(http.version, "2.1.0");
        assert_eq!(http.source, "registry+https://packages.rask-lang.dev");
        assert_eq!(http.checksum, compute_checksum(&f.tmp.path().join("cache/http")));

        let json = &lock.packages[1];
        assert_eq!(json.version, "1.4.2");
        assert_eq!(json.source, "path+../libs/json", "relative to the root package (LK7)");
        assert!(json.checksum.starts_with("sha256:") && json.checksum.len() == 7 + 64, "{}", json.checksum);
        assert!(json.capabilities.is_empty() && json.signing_key.is_none());
    }

    #[test]
    fn generate_without_a_manifest_version_writes_zero() {
        let mut f = Fixture::new();
        let id = f.registry.lookup_name("json").unwrap();
        f.registry.get_mut(id).unwrap().manifest = None;
        assert_eq!(f.lock().packages[1].version, "0.0.0");
    }

    #[test]
    fn generate_with_capabilities_fills_known_packages() {
        let f = Fixture::new();
        let lock = LockFile::generate_with_capabilities(
            &f.registry, f.root, &f.app_dir(), &caps(&[("http", &["net"]), ("nobody", &["ffi"])]),
        );
        assert_eq!(lock.packages[0].capabilities, ["net"]);
        assert!(lock.packages[1].capabilities.is_empty());
    }

    #[test]
    fn generate_workspace_skips_every_member_and_locks_each_name_once() {
        let mut f = Fixture::new();
        let member = f.registry.add_package("tool".into(), vec!["tool".into()], f.tmp.path().join("tool"));
        let dup = f.registry.add_package("json".into(), vec!["json2".into()], f.tmp.path().join("libs/json2"));
        f.registry.get_mut(dup).unwrap().is_external = true;
        let lock = LockFile::generate_workspace(&f.registry, &[f.root, member], f.tmp.path());
        let names: Vec<&str> = lock.packages.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["http", "json"]);
        assert_eq!(lock.packages[1].source, "path+libs/json", "the first json wins");
    }

    // ─── write → load ───────────────────────────────────────────────

    #[test]
    fn write_then_load_round_trips() {
        let f = Fixture::new();
        let mut lock = f.lock();
        lock.packages[0].signing_key = Some("ed25519:f9e8d7".into());
        lock.packages[0].capabilities = vec!["net".into(), "read".into()];
        lock.packages.push(LockedPackage {
            name: "odd".into(),
            source: "path+..\\weird \"dir\"".into(),
            ..locked("odd")
        });
        let path = f.tmp.path().join("rask.lock");
        lock.write(&path).unwrap();

        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# rask.lock — auto-generated, do not edit\nlockfile-version = 1\n\n[[package]]\nname = \"http\"\n"), "{text}");
        assert!(text.contains("signing-key = \"ed25519:f9e8d7\"\ncapabilities = [\"net\", \"read\"]\n"), "{text}");
        assert!(text.contains("source = \"path+..\\\\weird \\\"dir\\\"\"\n"), "escaped: {text}");

        let back = LockFile::load(&path).unwrap();
        assert_eq!(back.version, 1);
        assert_eq!(back.packages.len(), 3);
        for (a, b) in lock.packages.iter().zip(&back.packages) {
            assert_eq!(
                (&a.name, &a.version, &a.source, &a.checksum, &a.capabilities, &a.signing_key),
                (&b.name, &b.version, &b.source, &b.checksum, &b.capabilities, &b.signing_key),
            );
        }
        // Writing what was read changes nothing.
        assert_eq!(back.render(), text);
    }

    #[test]
    fn the_spec_example_loads() {
        let text = "# rask.lock — auto-generated, do not edit\nlockfile-version = 1\n\n[[package]]\nname = \"http-client\"\nversion = \"2.1.0\"\nsource = \"registry+https://packages.rask-lang.dev\"\nchecksum = \"sha256:a1b2c3d4...\"\nsigning-key = \"ed25519:f9e8d7c6...\"\ncapabilities = [\"net\"]\n\n[[package]]\nname = \"parser\"\nversion = \"1.3.0\"\nsource = \"path+../parser\"\nchecksum = \"sha256:e5f6g7h8...\"\n";
        let lock = parse(text, "rask.lock").unwrap();
        assert_eq!(lock.packages.len(), 2);
        assert_eq!(lock.packages[0].signing_key.as_deref(), Some("ed25519:f9e8d7c6..."));
        assert_eq!(lock.packages[0].capabilities, ["net"]);
        assert_eq!(lock.packages[1].signing_key, None);
        assert!(!lock.is_empty());
    }

    #[test]
    fn an_empty_lock_file_has_no_packages() {
        let lock = parse("lockfile-version = 1\n", "rask.lock").unwrap();
        assert!(lock.is_empty());
        assert!(LockFile::default().is_empty());
    }

    // ─── load errors ────────────────────────────────────────────────

    fn load_err(text: &str) -> String {
        match parse(text, "rask.lock") {
            Ok(lock) => panic!("loaded {} packages from:\n{text}", lock.packages.len()),
            Err(e) => e,
        }
    }

    const ENTRY: &str = "[[package]]\nname = \"a\"\nversion = \"1.0.0\"\nsource = \"path+a\"\nchecksum = \"sha256:00\"\n";

    #[test]
    fn load_names_the_line_of_a_malformed_entry() {
        let head = "lockfile-version = 1\n\n";
        let cases: &[(&str, &str)] = &[
            ("garbage line\n", "rask.lock:8: expected `key = value`, found `garbage line`"),
            ("color = \"red\"\n", "rask.lock:8: unknown key `color`"),
            ("name = \"b\"\n", "rask.lock:8: `name` is set twice in one entry (first on line 4)"),
            ("[package]\n", "rask.lock:8: unknown table `[package]`"),
        ];
        for (extra, want) in cases {
            let err = load_err(&format!("{head}{ENTRY}{extra}"));
            assert!(err.starts_with(want), "{extra:?}: {err}");
        }
    }

    #[test]
    fn load_checks_value_shapes() {
        let err = load_err(&format!("lockfile-version = 1\n{}", ENTRY.replace("\"1.0.0\"", "1.0.0")));
        assert_eq!(err, "rask.lock:4: `version` must be a quoted string, found `1.0.0`");
        let err = load_err(&format!("lockfile-version = 1\n{ENTRY}capabilities = \"net\"\n"));
        assert!(err.starts_with("rask.lock:7: `capabilities` must be a list of quoted strings"), "{err}");
        let err = load_err(&format!("lockfile-version = 1\n{ENTRY}capabilities = [\"net\" \"read\"]\n"));
        assert!(err.starts_with("rask.lock:7: `capabilities`"), "{err}");
        let err = load_err(&format!("lockfile-version = 1\n{}", ENTRY.replace("\"path+a\"", "\"path+a\" trailing")));
        assert!(err.starts_with("rask.lock:5: `source`"), "{err}");
    }

    #[test]
    fn load_names_a_missing_field_and_its_entry() {
        for field in ["name", "version", "source", "checksum"] {
            let entry: String = ENTRY.lines().filter(|l| !l.starts_with(field)).map(|l| format!("{l}\n")).collect();
            let err = load_err(&format!("lockfile-version = 1\n{ENTRY}\n{entry}"));
            assert_eq!(err, format!("rask.lock:8: this [[package]] entry has no `{field}`"));
        }
    }

    #[test]
    fn load_requires_a_supported_version() {
        assert_eq!(load_err(ENTRY), "rask.lock: no `lockfile-version` line; run `rask update` to regenerate it");
        let err = load_err("lockfile-version = 1\nname = \"a\"\n");
        assert_eq!(err, "rask.lock:2: `name` belongs inside a [[package]] entry");
        assert!(load_err("").contains("no `lockfile-version` line"));
        assert!(load_err(&format!("lockfile-version = 2\n{ENTRY}")).contains("newer than supported"));
        assert!(load_err(&format!("lockfile-version = 0\n{ENTRY}")).starts_with("rask.lock:1: lockfile-version 0 isn't supported"));
        assert!(load_err(&format!("lockfile-version = one\n{ENTRY}")).starts_with("rask.lock:1: `lockfile-version` is a whole number"));
        assert!(load_err(&format!("lockfile-version = 1\nlockfile-version = 1\n{ENTRY}")).contains("set twice"));
    }

    #[test]
    fn load_names_the_file_it_read() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("rask.lock");
        fs::write(&path, "lockfile-version = 1\nnope\n").unwrap();
        let err = LockFile::load(&path).unwrap_err();
        assert_eq!(err, format!("{}:2: expected `key = value`, found `nope`", path.display()));
        let err = LockFile::load(&tmp.path().join("missing.lock")).unwrap_err();
        assert!(err.starts_with("failed to read lock file"), "{err}");
    }

    // ─── verify ─────────────────────────────────────────────────────

    #[test]
    fn verify_accepts_a_fresh_lock() {
        let f = Fixture::new();
        f.lock().verify(&f.registry, f.root).unwrap();
    }

    #[test]
    fn verify_catches_a_changed_source_file() {
        let f = Fixture::new();
        let lock = f.lock();
        fs::write(f.tmp.path().join("libs/json/lib.rk"), "public func parse() { todo() }\n").unwrap();
        let err = lock.verify(&f.registry, f.root).unwrap_err();
        assert!(err.contains("'json' has changed") && err.contains("rask update"), "{err}");
    }

    #[test]
    fn verify_catches_a_tampered_checksum() {
        let f = Fixture::new();
        let mut lock = f.lock();
        lock.packages[0].checksum = "sha256:0000".into();
        let err = lock.verify(&f.registry, f.root).unwrap_err();
        assert!(err.contains("'http' has changed (checksum mismatch)"), "{err}");
    }

    #[test]
    fn verify_catches_a_package_the_lock_does_not_know() {
        let f = Fixture::new();
        let mut lock = f.lock();
        lock.packages.retain(|p| p.name != "json");
        let err = lock.verify(&f.registry, f.root).unwrap_err();
        assert_eq!(err, "dependency 'json' not in rask.lock — run `rask update`");
    }

    #[test]
    fn verify_ignores_files_the_checksum_does_not_cover() {
        let f = Fixture::new();
        let lock = f.lock();
        let json = f.tmp.path().join("libs/json");
        write_files(&json, &[
            ("README.md", "docs"),
            ("build.rk", "func build() {}"),
            ("build/out.rk", "x"),
            (".hidden/x.rk", "x"),
            ("vendor/dep/x.rk", "x"),
        ]);
        lock.verify(&f.registry, f.root).unwrap();
    }

    // ─── compute_checksum ───────────────────────────────────────────

    #[test]
    fn checksum_is_stable_and_independent_of_location() {
        let tmp = TempDir::new().unwrap();
        let a = tmp.path().join("a");
        let b = tmp.path().join("elsewhere/b");
        for dir in [&a, &b] {
            write_files(dir, &[("lib.rk", "func f() {}\n"), ("sub/more.rk", "func g() {}\n")]);
        }
        assert_eq!(compute_checksum(&a), compute_checksum(&a));
        assert_eq!(compute_checksum(&a), compute_checksum(&b));
    }

    #[test]
    fn checksum_follows_rk_content_and_paths() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("p");
        write_files(&dir, &[("lib.rk", "func f() {}\n")]);
        let base = compute_checksum(&dir);

        write_files(&dir, &[("notes.txt", "x"), ("rask.toml", "x"), ("build.rk", "x")]);
        assert_eq!(compute_checksum(&dir), base, "only .rk sources outside build.rk count");

        write_files(&dir, &[("lib.rk", "func f() { }\n")]);
        let edited = compute_checksum(&dir);
        assert_ne!(edited, base, "one byte of source changes it");

        write_files(&dir, &[("lib.rk", "func f() {}\n")]);
        assert_eq!(compute_checksum(&dir), base);

        fs::rename(dir.join("lib.rk"), dir.join("renamed.rk")).unwrap();
        assert_ne!(compute_checksum(&dir), base, "the file's path is part of it");
        fs::rename(dir.join("renamed.rk"), dir.join("lib.rk")).unwrap();

        write_files(&dir, &[("extra/new.rk", "")]);
        assert_ne!(compute_checksum(&dir), base, "an added file, even empty, changes it");
    }

    #[test]
    fn checksum_of_an_empty_or_missing_dir_is_the_empty_hash() {
        let tmp = TempDir::new().unwrap();
        let empty = "sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(compute_checksum(tmp.path()), empty);
        assert_eq!(compute_checksum(&tmp.path().join("missing")), empty);
    }

    // ─── capabilities_changed / signing_keys_changed ────────────────

    #[test]
    fn capabilities_changed_compares_as_sets() {
        let mut lock = LockFile::default();
        let mut a = locked("a");
        a.capabilities = vec!["read".into(), "net".into()];
        lock.packages = vec![a, locked("b"), locked("c")];
        let now = caps(&[("a", &["net", "read"]), ("b", &["ffi"]), ("new", &["exec"])]);
        assert_eq!(lock.capabilities_changed(&now), ["b"], "order doesn't matter; unknown and absent packages are skipped");
        let now = caps(&[("a", &["net"])]);
        assert_eq!(lock.capabilities_changed(&now), ["a"], "a dropped capability counts too");
    }

    #[test]
    fn signing_keys_changed_reports_old_and_new() {
        let mut lock = LockFile::default();
        let mut a = locked("a");
        a.signing_key = Some("ed25519:old".into());
        let mut b = locked("b");
        b.signing_key = Some("ed25519:same".into());
        lock.packages = vec![a, b, locked("unsigned")];
        let keys: BTreeMap<String, String> = [("a", "ed25519:new"), ("b", "ed25519:same"), ("unsigned", "ed25519:first")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(
            lock.signing_keys_changed(&keys),
            [("a".to_string(), "ed25519:old".to_string(), "ed25519:new".to_string())],
            "a first key for an unsigned package is not a change (LK8 adds it on the next fetch)",
        );
    }
}
