// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Lock file generation and verification — LK1-LK5.
//!
//! Pins exact dependency versions for reproducible builds.
//! SHA-256 checksums for integrity. Capabilities field per
//! package for permission tracking (PM1-PM8).

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
        let mut packages = Vec::new();

        for pkg in registry.packages() {
            if pkg.id == root_id { continue; }
            if !pkg.is_external { continue; }

            let version = pkg.manifest.as_ref()
                .map(|m| m.version.clone())
                .unwrap_or_else(|| "0.0.0".into());

            // Registry packages use registry+URL, path deps use relative paths (LK3)
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
            });
        }

        // Sort for deterministic output (LK2)
        packages.sort_by(|a, b| a.name.cmp(&b.name));

        LockFile { version: LOCKFILE_VERSION, packages }
    }

    /// Generate with capability information from the build.
    pub fn generate_with_capabilities(
        registry: &PackageRegistry,
        root_id: PackageId,
        root_dir: &Path,
        caps: &BTreeMap<String, Vec<String>>,
    ) -> Self {
        let mut lock = Self::generate(registry, root_id, root_dir);
        for pkg in &mut lock.packages {
            if let Some(pkg_caps) = caps.get(&pkg.name) {
                pkg.capabilities = pkg_caps.clone();
            }
        }
        lock
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

    /// Load a lock file from disk.
    pub fn load(path: &Path) -> Result<Self, String> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read lock file: {}", e))?;

        let mut packages = Vec::new();
        let mut current: Option<BTreeMap<String, String>> = None;
        let mut file_version = 0u32;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line == "[[package]]" {
                if let Some(fields) = current.take() {
                    packages.push(fields_to_locked(&fields)?);
                }
                current = Some(BTreeMap::new());
                continue;
            }
            if let Some((key, value)) = parse_kv(line) {
                if current.is_none() && key == "lockfile-version" {
                    file_version = value.parse().unwrap_or(0);
                    if file_version > LOCKFILE_VERSION {
                        return Err(format!(
                            "rask.lock version {} is newer than supported ({}). Update your compiler",
                            file_version, LOCKFILE_VERSION,
                        ));
                    }
                    continue;
                }
                if let Some(ref mut fields) = current {
                    fields.insert(key, value);
                }
            }
        }

        if let Some(fields) = current.take() {
            packages.push(fields_to_locked(&fields)?);
        }

        Ok(LockFile { version: file_version, packages })
    }

    /// Write the lock file to disk.
    pub fn write(&self, path: &Path) -> Result<(), String> {
        let mut content = String::from("# rask.lock — auto-generated, do not edit\n");
        content.push_str(&format!("lockfile-version = {}\n\n", LOCKFILE_VERSION));

        for pkg in &self.packages {
            content.push_str("[[package]]\n");
            content.push_str(&format!("name = \"{}\"\n", pkg.name));
            content.push_str(&format!("version = \"{}\"\n", pkg.version));
            content.push_str(&format!("source = \"{}\"\n", pkg.source));
            content.push_str(&format!("checksum = \"{}\"\n", pkg.checksum));
            if !pkg.capabilities.is_empty() {
                let caps: Vec<String> = pkg.capabilities.iter()
                    .map(|c| format!("\"{}\"", c))
                    .collect();
                content.push_str(&format!("capabilities = [{}]\n", caps.join(", ")));
            }
            content.push('\n');
        }

        std::fs::write(path, &content)
            .map_err(|e| format!("failed to write lock file: {}", e))
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

/// Parse a "key = value" line, stripping quotes from value.
fn parse_kv(line: &str) -> Option<(String, String)> {
    let (key, rest) = line.split_once('=')?;
    let key = key.trim().to_string();
    let value = rest.trim().to_string();
    // Strip surrounding quotes if present
    let value = if value.starts_with('"') && value.ends_with('"') {
        value[1..value.len() - 1].to_string()
    } else {
        value
    };
    Some((key, value))
}

/// Convert a field map to a LockedPackage.
fn fields_to_locked(fields: &BTreeMap<String, String>) -> Result<LockedPackage, String> {
    let capabilities = fields.get("capabilities")
        .map(|v| parse_string_array(v))
        .unwrap_or_default();

    Ok(LockedPackage {
        name: fields.get("name").cloned().ok_or("missing 'name' in lock file entry")?,
        version: fields.get("version").cloned().ok_or("missing 'version' in lock file entry")?,
        source: fields.get("source").cloned().ok_or("missing 'source' in lock file entry")?,
        checksum: fields.get("checksum").cloned().ok_or("missing 'checksum' in lock file entry")?,
        capabilities,
    })
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

/// Parse a TOML-style string array: ["a", "b", "c"]
fn parse_string_array(s: &str) -> Vec<String> {
    let s = s.trim();
    if !s.starts_with('[') || !s.ends_with(']') {
        return Vec::new();
    }
    let inner = &s[1..s.len() - 1];
    inner.split(',')
        .map(|item| item.trim().trim_matches('"').to_string())
        .filter(|item| !item.is_empty())
        .collect()
}
