// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dependency tracking and staleness detection for spec files.
//!
//! Parses `<!-- depends: ... -->` and `<!-- implemented-by: ... -->` headers
//! from spec markdown, then compares git timestamps to detect stale specs.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Dependencies declared by a spec file.
#[derive(Debug, Clone)]
pub struct SpecDeps {
    /// Path to the spec file (relative to project root)
    pub path: PathBuf,
    /// Other spec files this spec depends on (paths relative to specs/)
    pub depends: Vec<String>,
    /// Compiler crates that implement this spec (paths relative to project root)
    pub implemented_by: Vec<String>,
}

/// A staleness warning for a spec file.
#[derive(Debug)]
pub struct StalenessWarning {
    /// The spec that may be stale
    pub spec: PathBuf,
    /// The dependency that was modified more recently
    pub dependency: String,
    /// Short git hash of the spec's last commit
    pub spec_commit: String,
    /// Short git hash of the dependency's last commit
    pub dep_commit: String,
    /// "depends on" or "implemented by"
    pub direction: &'static str,
}

/// Extract dependency headers from the first 20 lines of a markdown file.
pub fn extract_deps(path: &Path, markdown: &str) -> SpecDeps {
    let mut depends = Vec::new();
    let mut implemented_by = Vec::new();

    for line in markdown.lines().take(20) {
        let trimmed = line.trim();

        if let Some(inner) = strip_comment(trimmed, "depends:") {
            depends.extend(parse_csv(&inner));
        } else if let Some(inner) = strip_comment(trimmed, "implemented-by:") {
            implemented_by.extend(parse_csv(&inner));
        }
    }

    SpecDeps {
        path: path.to_path_buf(),
        depends,
        implemented_by,
    }
}

/// Strip an HTML comment with a known prefix, returning the value part.
/// `<!-- depends: a.md, b.md -->` with prefix "depends:" → Some("a.md, b.md")
fn strip_comment(line: &str, prefix: &str) -> Option<String> {
    let inner = line.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    let value = inner.strip_prefix(prefix)?.trim();
    Some(value.to_string())
}

/// Split a comma-separated string, trimming each entry.
fn parse_csv(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Check all spec dependencies for staleness using git timestamps.
/// Returns warnings for any spec whose dependency was modified more recently.
pub fn check_staleness(all_deps: &[SpecDeps], root: &Path) -> Vec<StalenessWarning> {
    let mut warnings = Vec::new();

    for deps in all_deps {
        if deps.depends.is_empty() && deps.implemented_by.is_empty() {
            continue;
        }

        let spec_path = deps.path.to_string_lossy().to_string();
        let spec_info = match git_last_modified(root, &spec_path) {
            Some(info) => info,
            None => continue,
        };

        // Check spec-to-spec dependencies
        for dep in &deps.depends {
            // depends paths are relative to specs/, resolve to full path
            let dep_path = format!("specs/{}", dep);
            if let Some(dep_info) = git_last_modified(root, &dep_path) {
                if dep_info.timestamp > spec_info.timestamp {
                    warnings.push(StalenessWarning {
                        spec: deps.path.clone(),
                        dependency: dep_path,
                        spec_commit: spec_info.hash.clone(),
                        dep_commit: dep_info.hash.clone(),
                        direction: "depends on",
                    });
                }
            }
        }

        // Check spec-to-compiler dependencies (bidirectional)
        for crate_path in &deps.implemented_by {
            if let Some(crate_info) = git_last_modified(root, crate_path) {
                if crate_info.timestamp > spec_info.timestamp {
                    warnings.push(StalenessWarning {
                        spec: deps.path.clone(),
                        dependency: crate_path.clone(),
                        spec_commit: spec_info.hash.clone(),
                        dep_commit: crate_info.hash.clone(),
                        direction: "implemented by",
                    });
                }
            }
        }
    }

    warnings
}

struct GitInfo {
    timestamp: u64,
    hash: String,
}

/// Get the last modification timestamp and short hash for a path via git.
/// Returns None if git is unavailable or the path has no commits.
fn git_last_modified(root: &Path, file: &str) -> Option<GitInfo> {
    let output = Command::new("git")
        .args(["log", "-1", "--format=%ct %h", "--", file])
        .current_dir(root)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut parts = trimmed.splitn(2, ' ');
    let timestamp = parts.next()?.parse::<u64>().ok()?;
    let hash = parts.next()?.to_string();

    Some(GitInfo { timestamp, hash })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_deps_both_headers() {
        let md = "\
<!-- depends: memory/ownership.md, types/error-types.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->
# Some Spec
Content here.
";
        let deps = extract_deps(Path::new("specs/test.md"), md);
        assert_eq!(deps.depends, vec!["memory/ownership.md", "types/error-types.md"]);
        assert_eq!(deps.implemented_by, vec!["compiler/crates/rask-types/"]);
    }

    #[test]
    fn test_extract_deps_none() {
        let md = "# Just a title\n\nNo dependency headers.";
        let deps = extract_deps(Path::new("specs/test.md"), md);
        assert!(deps.depends.is_empty());
        assert!(deps.implemented_by.is_empty());
    }

    #[test]
    fn test_extract_deps_only_depends() {
        let md = "<!-- depends: control/ensure.md -->\n# Spec";
        let deps = extract_deps(Path::new("specs/test.md"), md);
        assert_eq!(deps.depends, vec!["control/ensure.md"]);
        assert!(deps.implemented_by.is_empty());
    }

    #[test]
    fn test_parse_csv() {
        assert_eq!(parse_csv("a, b, c"), vec!["a", "b", "c"]);
        assert_eq!(parse_csv("single"), vec!["single"]);
        assert_eq!(parse_csv("  spaced , items  "), vec!["spaced", "items"]);
        assert!(parse_csv("").is_empty());
    }
}
