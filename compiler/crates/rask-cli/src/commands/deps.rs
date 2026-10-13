// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dependency management commands — struct.build/AD1-AD4, RM1-RM2.
//!
//! Edits are line-based, so comments and layout stay as the author left them
//! (AD2). Each edited file goes through the manifest loader before it's
//! written: an edit that would leave the manifest invalid is refused, and the
//! file is untouched.

use colored::Colorize;
use std::fs;
use std::path::Path;
use std::process;

use rask_resolve::package::MANIFEST_FILE;

use crate::output;

/// Add a dependency to rask.toml (AD1-AD4).
pub fn cmd_add(
    name: &str,
    version: Option<&str>,
    dev: bool,
    feature: Option<&str>,
    local_path: Option<&str>,
) {
    let manifest_path = Path::new(MANIFEST_FILE);
    let spec = match local_path {
        Some(p) => format!("{{ path = \"{}\" }}", toml_escape(p)),
        None => {
            let ver = version.unwrap_or("*").trim_matches('"');
            format!("{{ registry = \"{}\" }}", toml_escape(ver))
        }
    };
    let entry = format!("{} = {}", toml_key(name), spec);
    let table = match (dev, feature) {
        (true, Some(f)) => format!("features.{}.dev-dependencies", toml_key(f)),
        (true, None) => "dev-dependencies".to_string(),
        (false, Some(f)) => format!("features.{}.dependencies", toml_key(f)),
        (false, None) => "dependencies".to_string(),
    };

    let content = if manifest_path.exists() {
        read_manifest(manifest_path)
    } else {
        let dir_name = std::env::current_dir()
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "my-package".to_string());
        let schema = rask_manifest::schema_line().unwrap_or_else(|e| fail(&e));
        println!("  {} {}", "Created".green(), MANIFEST_FILE);
        format!(
            "schema = \"{}\"\n\n[package]\nname = \"{}\"\nversion = \"0.1.0\"\n",
            schema,
            toml_escape(&dir_name)
        )
    };

    // AD3: one name, one dependency, across every table. Asked of the
    // loader, which has already parsed every spelling of the file.
    if manifest_path.exists() {
        let pkg = rask_manifest::load_text(&content)
            .unwrap_or_else(|e| fail(&format!("{} doesn't load, so nothing was added:\n{}", MANIFEST_FILE, e)));
        let declared = pkg.deps.iter().chain(pkg.features.iter().flat_map(|f| f.deps.iter()));
        if declared.into_iter().any(|d| d.name == name) {
            eprintln!(
                "{}: \"{}\" is already in {}. Use {} to change its version.",
                output::error_label(), name, MANIFEST_FILE, "rask update".green()
            );
            process::exit(1);
        }
    }

    let new_content = insert_entry(&content, &table, &entry);
    write_checked(manifest_path, &new_content);
    println!("  {} {} to [{}]", "Added".green(), entry, table);
}

/// Remove a dependency from rask.toml (RM1-RM2).
pub fn cmd_remove(name: &str) {
    let manifest_path = Path::new(MANIFEST_FILE);
    if !manifest_path.exists() {
        fail(&format!("no {} here", MANIFEST_FILE));
    }
    let content = read_manifest(manifest_path);
    let (new_content, removed) = remove_entry(&content, name);
    if removed == 0 {
        fail(&format!("\"{}\" isn't a dependency in {}", name, MANIFEST_FILE));
    }
    write_checked(manifest_path, &new_content);
    println!("  {} \"{}\"", "Removed".green(), name);
}

fn fail(msg: &str) -> ! {
    eprintln!("{}: {}", output::error_label(), msg);
    process::exit(1);
}

fn read_manifest(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("reading {}: {}", MANIFEST_FILE, e)))
}

/// Write only an edit the loader accepts.
fn write_checked(path: &Path, content: &str) {
    if let Err(e) = rask_manifest::load_text(content) {
        fail(&format!(
            "the edit would leave {} invalid, so it wasn't written:\n{}",
            MANIFEST_FILE, e
        ));
    }
    if let Err(e) = fs::write(path, content) {
        fail(&format!("writing {}: {}", MANIFEST_FILE, e));
    }
}

/// The inside of a TOML basic string.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A key, bare when TOML allows it.
fn toml_key(s: &str) -> String {
    let bare = !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if bare { s.to_string() } else { format!("\"{}\"", toml_escape(s)) }
}

/// `[dependencies]` → `dependencies`, for a header line.
fn header_of(line: &str) -> Option<&str> {
    let t = line.trim();
    if t.starts_with("[[") {
        return None;
    }
    let inner = t.strip_prefix('[')?;
    let end = inner.find(']')?;
    Some(inner[..end].trim())
}

fn is_dep_table(header: &str) -> bool {
    header == "dependencies"
        || header == "dev-dependencies"
        || header == "build-dependencies"
        || (header.starts_with("features.")
            && (header.ends_with(".dependencies")
                || header.ends_with(".dev-dependencies")
                || header.ends_with(".build-dependencies")))
}

/// Put `entry` at the end of `[table]`'s own lines, creating the table at
/// the end of the file if it isn't there.
fn insert_entry(content: &str, table: &str, entry: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = lines.iter().position(|l| header_of(l) == Some(table)) else {
        let mut out = content.trim_end().to_string();
        out.push_str(&format!("\n\n[{}]\n{}\n", table, entry));
        return out;
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| header_of(l).is_some())
        .map_or(lines.len(), |i| start + 1 + i);
    // After the last key line, before any trailing blanks or comments.
    let mut at = start + 1;
    for (i, l) in lines.iter().enumerate().take(end).skip(start + 1) {
        let t = l.trim();
        if !t.is_empty() && !t.starts_with('#') {
            at = i + 1;
        }
    }
    let mut out: Vec<&str> = lines[..at].to_vec();
    out.push(entry);
    out.extend_from_slice(&lines[at..]);
    out.join("\n") + "\n"
}

/// Drop `name`'s line from every dependency table, and a whole
/// `[dependencies.name]` table if it's written that way.
fn remove_entry(content: &str, name: &str) -> (String, usize) {
    let quoted = format!("\"{}\"", toml_escape(name));
    let names_it = |line: &str| {
        let t = line.trim_start();
        let key = t.split('=').next().unwrap_or("").trim();
        !t.starts_with('#') && t.contains('=') && (key == name || key == quoted)
    };
    let mut out = Vec::new();
    let mut removed = 0;
    let mut table: Option<String> = None;
    let mut skipping_table = false;
    for line in content.lines() {
        if let Some(h) = header_of(line) {
            let h = h.to_string();
            skipping_table = h
                .rsplit_once('.')
                .is_some_and(|(parent, last)| is_dep_table(parent) && (last == name || last == quoted));
            if skipping_table {
                removed += 1;
            }
            table = Some(h);
            if skipping_table {
                continue;
            }
        } else if skipping_table {
            continue;
        } else if table.as_deref().is_some_and(is_dep_table) && names_it(line) {
            removed += 1;
            continue;
        }
        out.push(line);
    }
    (out.join("\n") + "\n", removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "schema = \"rask-package@1\"\n\n[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nhttp = { registry = \"^2\" }  # keep me\n\n# trailing comment\n[features.ssl]\ndefault = true\n";

    #[test]
    fn insert_goes_after_the_last_key_and_keeps_comments() {
        let out = insert_entry(BASE, "dependencies", "json = { registry = \"^1\" }");
        assert!(out.contains("http = { registry = \"^2\" }  # keep me\njson = { registry = \"^1\" }\n\n# trailing comment"), "{out}");
    }

    #[test]
    fn insert_creates_a_missing_table() {
        let out = insert_entry(BASE, "dev-dependencies", "mock = { path = \"../mock\" }");
        assert!(out.ends_with("\n\n[dev-dependencies]\nmock = { path = \"../mock\" }\n"), "{out}");
    }

    #[test]
    fn remove_takes_the_line_and_nothing_else() {
        let (out, n) = remove_entry(BASE, "http");
        assert_eq!(n, 1);
        assert!(!out.contains("http"));
        assert!(out.contains("# trailing comment"));
        let (_, none) = remove_entry(BASE, "name");
        assert_eq!(none, 0, "`name` under [package] is not a dependency");
    }

    #[test]
    fn remove_takes_a_table_form_dependency() {
        let text = "[dependencies.tls]\ngit = \"u\"\nrev = \"a\"\n\n[features.x]\n";
        let (out, n) = remove_entry(text, "tls");
        assert_eq!(n, 1);
        assert_eq!(out.trim(), "[features.x]");
    }
}
