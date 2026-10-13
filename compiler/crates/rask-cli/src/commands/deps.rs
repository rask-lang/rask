// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dependency management commands — struct.build/AD1-AD5, RM1-RM2.
//!
//! Edits are line-based, so comments and layout stay as the author left them
//! (AD2). Each edited file goes through the manifest loader before it's
//! written: an edit that would leave the manifest invalid is refused, and the
//! file is untouched. After the edit, rask.lock is brought up to date with
//! the same resolution `rask fetch` runs; if that fails, rask.toml goes back
//! to what it was.

use colored::Colorize;
use std::fs;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process;

use rask_resolve::package::MANIFEST_FILE;

use super::fetch::{resolve_and_lock, Pins, Resolved};
use crate::output;

const DEP_TABLES: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];

/// Where a dependency comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    /// A version range, or None for "look up the newest" (AD1).
    Registry(Option<String>),
    Path(String),
    Git { url: String, rev: Option<String>, branch: Option<String> },
}

/// One `rask add`, parsed.
#[derive(Debug, Clone, PartialEq)]
pub struct AddRequest {
    pub name: String,
    pub source: Source,
    /// "dependencies", "dev-dependencies" or "build-dependencies".
    pub table: &'static str,
    pub feature: Option<String>,
    pub with: Vec<String>,
    /// None when no `--allow` was given: the registry's answer decides (AD5).
    pub allow: Option<Vec<String>>,
    pub target: Option<String>,
}

impl AddRequest {
    pub fn registry(name: &str, version: Option<&str>) -> Self {
        AddRequest {
            name: name.to_string(),
            source: Source::Registry(version.map(str::to_string)),
            table: "dependencies",
            feature: None,
            with: Vec::new(),
            allow: None,
            target: None,
        }
    }
}

const ADD_VALUE_FLAGS: [&str; 8] =
    ["--feature", "--path", "--git", "--rev", "--branch", "--with", "--allow", "--target"];

/// `rask add <name> [version] [flags]`, from the words after `add`.
pub fn parse_add_args(args: &[&str]) -> Result<AddRequest, String> {
    let mut positionals: Vec<&str> = Vec::new();
    let mut values: Vec<(&str, &str)> = Vec::new();
    let (mut dev, mut build) = (false, false);
    let mut i = 0;
    while i < args.len() {
        let a = args[i];
        if ADD_VALUE_FLAGS.contains(&a) {
            let v = args.get(i + 1).ok_or_else(|| format!("{} needs a value", a))?;
            values.push((a, v));
            i += 2;
            continue;
        }
        match a {
            "--dev" => dev = true,
            "--build" => build = true,
            _ if a.starts_with('-') => return Err(format!("unknown flag `{}`; see `rask add --help`", a)),
            _ => positionals.push(a),
        }
        i += 1;
    }
    let value = |flag: &str| values.iter().rev().find(|(f, _)| *f == flag).map(|(_, v)| v.to_string());
    let list = |flag: &str| {
        value(flag).map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>())
    };
    let name = match positionals.as_slice() {
        [] => return Err("missing package name".into()),
        [name] | [name, _] => name.to_string(),
        _ => return Err(format!("too many arguments: {}", positionals[2..].join(" "))),
    };
    let version = positionals.get(1).map(|v| v.trim_matches('"').to_string());
    let path = value("--path");
    let git = value("--git");
    let one_source = [version.is_some(), path.is_some(), git.is_some()];
    if one_source.iter().filter(|b| **b).count() > 1 {
        return Err("a dependency has one source: a version, `--path` or `--git`".into());
    }
    if git.is_none() && (value("--rev").is_some() || value("--branch").is_some()) {
        return Err("`--rev` and `--branch` go with `--git`".into());
    }
    let source = match (path, git) {
        (Some(p), _) => Source::Path(p),
        (_, Some(url)) => Source::Git { url, rev: value("--rev"), branch: value("--branch") },
        _ => Source::Registry(version),
    };
    let table = match (dev, build) {
        (true, true) => return Err("pick one of `--dev` and `--build`".into()),
        (true, false) => "dev-dependencies",
        (false, true) => "build-dependencies",
        (false, false) => "dependencies",
    };
    Ok(AddRequest {
        name,
        source,
        table,
        feature: value("--feature"),
        with: list("--with").unwrap_or_default(),
        allow: list("--allow"),
        target: value("--target"),
    })
}

/// Add a dependency to rask.toml (AD1-AD5).
pub fn cmd_add(args: &[&str]) {
    let mut req = parse_add_args(args).unwrap_or_else(|e| fail(&e));
    let manifest_path = Path::new(MANIFEST_FILE);
    let existing = manifest_path.exists().then(|| read_manifest(manifest_path));
    // Refuse a duplicate or a broken manifest before asking the registry.
    if let Some(text) = &existing {
        check_can_add(text, &req.name).unwrap_or_else(|e| fail(&e));
    }
    // A range the manifest wouldn't take is named by the loader, with its key,
    // before the registry is asked about it.
    plan_add(existing.as_deref(), "dir", &req).unwrap_or_else(|e| fail(&e));
    if let Source::Registry(version) = &req.source {
        let (range, capabilities) = look_up(&req.name, version.as_deref()).unwrap_or_else(|e| fail(&e));
        req.source = Source::Registry(Some(range));
        if req.allow.is_none() && !capabilities.is_empty() {
            req.allow = Some(grant(&req.name, &capabilities).unwrap_or_else(|e| fail(&e)));
        }
    }

    let dir_name = std::env::current_dir()
        .ok()
        .and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| "my-package".to_string());
    let added = plan_add(existing.as_deref(), &dir_name, &req).unwrap_or_else(|e| fail(&e));
    let resolved = write_and_lock(manifest_path, existing.as_deref(), &added.content);
    if existing.is_none() {
        println!("  {} {}", "Created".green(), MANIFEST_FILE);
    }
    println!("  {} {} to [{}]", "Added".green(), added.entry, added.table);
    report_lock(&resolved);
}

/// Remove a dependency from rask.toml (RM1-RM2).
pub fn cmd_remove(name: &str) {
    let manifest_path = Path::new(MANIFEST_FILE);
    if !manifest_path.exists() {
        fail(&format!("no {} here", MANIFEST_FILE));
    }
    let content = read_manifest(manifest_path);
    let new_content = plan_remove(&content, name).unwrap_or_else(|e| fail(&e));
    let resolved = write_and_lock(manifest_path, Some(&content), &new_content);
    println!("  {} \"{}\"", "Removed".green(), name);
    // RM1: still pinned means something else still needs it.
    let users = still_needed_by(&resolved, name);
    if !users.is_empty() {
        println!(
            "  {} \"{}\" stays in rask.lock: {} still depend{} on it",
            "Note:".yellow(),
            name,
            users.join(", "),
            if users.len() == 1 { "s" } else { "" },
        );
    }
    report_lock(&resolved);
}

/// Write `content`, then bring rask.lock up to date (AD4, RM2). If that
/// fails, rask.toml goes back to `before` and nothing has changed.
fn write_and_lock(manifest_path: &Path, before: Option<&str>, content: &str) -> Resolved {
    if let Err(e) = fs::write(manifest_path, content) {
        fail(&format!("writing {}: {}", MANIFEST_FILE, e));
    }
    let root = manifest_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    match resolve_and_lock(&root, Pins::Keep, false) {
        Ok(r) => r,
        Err(errors) => {
            let restored = match before {
                Some(text) => fs::write(manifest_path, text),
                None => fs::remove_file(manifest_path),
            };
            let note = match restored {
                Ok(()) => format!("{} is unchanged", MANIFEST_FILE),
                Err(e) => format!("putting {} back failed too: {}", MANIFEST_FILE, e),
            };
            fail(&format!("couldn't update rask.lock, so {}:\n{}", note, errors.join("\n")))
        }
    }
}

fn report_lock(r: &Resolved) {
    match (&r.lock, r.lock_changed) {
        (Some(lock), true) => println!("  {} rask.lock ({} package{})", "Updated".green(), lock.packages.len(),
            if lock.packages.len() == 1 { "" } else { "s" }),
        (None, true) => println!("  {} rask.lock (no dependencies left)", "Removed".green()),
        _ => {}
    }
}

/// The packages whose manifests still name `name` as a dependency.
fn still_needed_by(r: &Resolved, name: &str) -> Vec<String> {
    let mut users: Vec<String> = r
        .registry
        .packages()
        .iter()
        .filter(|p| p.manifest.as_ref().is_some_and(|m| super::fetch::all_deps(m).any(|d| d.name == name)))
        .map(|p| p.name.clone())
        .collect();
    users.sort();
    users.dedup();
    users
}

/// AD1: the range to write for a registry dependency, and what the version it
/// picks uses. Without a range, the newest stable version, as `^` of it.
fn look_up(name: &str, range: Option<&str>) -> Result<(String, Vec<String>), String> {
    let config = rask_resolve::registry::RegistryConfig::from_env();
    let index = config
        .fetch_index(name)
        .map_err(|e| format!("can't look up \"{}\" in {}: {}", name, config.url, e))?;
    let constraint = match range {
        Some(r) => rask_resolve::semver::Constraint::parse(r).map_err(|e| format!("\"{}\": {}", r, e))?,
        None => rask_resolve::semver::Constraint::Any,
    };
    let version = super::fetch::resolve_from_index(&index, &[constraint], name)?;
    let caps = index.versions.get(&version.to_string()).map(|m| m.capabilities.clone()).unwrap_or_default();
    Ok((range.map(str::to_string).unwrap_or_else(|| format!("^{}", version)), caps))
}

/// AD5: ask before granting what a package says it uses. Without a
/// terminal to ask on, `--allow` has to say it.
fn grant(name: &str, capabilities: &[String]) -> Result<Vec<String>, String> {
    let described: Vec<String> = capabilities
        .iter()
        .map(|c| format!("{} ({})", c, rask_resolve::capabilities::capability_description(c)))
        .collect();
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "\"{}\" uses {}; pass `--allow {}` to grant it",
            name,
            described.join(", "),
            capabilities.join(",")
        ));
    }
    print!("  \"{}\" uses {}. Allow? [y/N] ", name, described.join(", "));
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    let _ = std::io::stdin().lock().read_line(&mut answer);
    if matches!(answer.trim(), "y" | "Y" | "yes") {
        Ok(capabilities.to_vec())
    } else {
        Err(format!("nothing added; \"{}\" needs {} to work", name, capabilities.join(", ")))
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("{}: {}", output::error_label(), msg);
    process::exit(1);
}

fn read_manifest(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("reading {}: {}", MANIFEST_FILE, e)))
}

/// A checked edit, ready to write.
#[derive(Debug)]
struct Added {
    content: String,
    entry: String,
    table: String,
}

fn quoted_list(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|s| format!("\"{}\"", toml_escape(s))).collect();
    format!("[{}]", quoted.join(", "))
}

/// The inline table for one dependency entry.
fn entry_spec(req: &AddRequest) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &req.source {
        Source::Registry(v) => parts.push(format!("registry = \"{}\"", toml_escape(v.as_deref().unwrap_or("*")))),
        Source::Path(p) => parts.push(format!("path = \"{}\"", toml_escape(p))),
        Source::Git { url, rev, branch } => {
            parts.push(format!("git = \"{}\"", toml_escape(url)));
            if let Some(r) = rev {
                parts.push(format!("rev = \"{}\"", toml_escape(r)));
            }
            if let Some(b) = branch {
                parts.push(format!("branch = \"{}\"", toml_escape(b)));
            }
        }
    }
    if !req.with.is_empty() {
        parts.push(format!("with = {}", quoted_list(&req.with)));
    }
    if let Some(allow) = req.allow.as_ref().filter(|a| !a.is_empty()) {
        parts.push(format!("allow = {}", quoted_list(allow)));
    }
    if let Some(t) = &req.target {
        parts.push(format!("target = \"{}\"", toml_escape(t)));
    }
    format!("{{ {} }}", parts.join(", "))
}

/// AD3: one name, one dependency, across every table. Asked of the loader,
/// which has already parsed every spelling of the file.
fn check_can_add(text: &str, name: &str) -> Result<(), String> {
    let pkg = rask_manifest::load_text(text)
        .map_err(|e| format!("{} doesn't load, so nothing was added:\n{}", MANIFEST_FILE, e))?;
    if declares(&pkg, name) {
        return Err(format!(
            "\"{}\" is already in {}. Use `rask update` to move it to a newer version",
            name, MANIFEST_FILE
        ));
    }
    Ok(())
}

/// The manifest after adding the dependency. `existing` is None when there
/// is no rask.toml yet; one is started, named after `dir_name`.
fn plan_add(existing: Option<&str>, dir_name: &str, req: &AddRequest) -> Result<Added, String> {
    let name = req.name.as_str();
    let entry = format!("{} = {}", toml_key(name), entry_spec(req));
    let table: Vec<String> = match &req.feature {
        Some(f) => vec!["features".into(), f.clone(), req.table.into()],
        None => vec![req.table.into()],
    };
    let table_text = render_key(&table);

    let content = match existing {
        Some(text) => {
            check_can_add(text, name)?;
            text.to_string()
        }
        None => format!(
            "schema = \"{}\"\n\n[package]\nname = \"{}\"\nversion = \"0.1.0\"\n",
            rask_manifest::schema_line()?,
            toml_escape(dir_name)
        ),
    };

    let new_content = insert_entry(&content, &table, &entry);
    let pkg = rask_manifest::load_text(&new_content).map_err(|e| {
        format!("the edit would leave {} invalid, so it wasn't written:\n{}", MANIFEST_FILE, e)
    })?;
    if !declares(&pkg, name) {
        return Err(format!(
            "adding \"{}\" by line didn't declare it, so {} wasn't written",
            name, MANIFEST_FILE
        ));
    }
    Ok(Added { content: new_content, entry, table: table_text })
}

/// The manifest after removing `name` from every dependency table.
fn plan_remove(content: &str, name: &str) -> Result<String, String> {
    let before = rask_manifest::load_text(content)
        .map_err(|e| format!("{} doesn't load, so nothing was removed:\n{}", MANIFEST_FILE, e))?;
    let (new_content, removed) = remove_entry(content, name);
    if removed == 0 {
        if declares(&before, name) {
            return Err(format!(
                "\"{}\" is declared in a form `rask remove` can't edit line by line, such as an inline \
                 `dependencies = {{ ... }}` table; remove it by hand",
                name
            ));
        }
        return Err(format!("\"{}\" isn't a dependency in {}", name, MANIFEST_FILE));
    }
    let after = rask_manifest::load_text(&new_content).map_err(|e| {
        format!("the edit would leave {} invalid, so it wasn't written:\n{}", MANIFEST_FILE, e)
    })?;
    if declares(&after, name) {
        return Err(format!(
            "\"{}\" is still declared after removing its lines, so {} wasn't written; remove it by hand",
            name, MANIFEST_FILE
        ));
    }
    Ok(new_content)
}

fn declares(pkg: &rask_ast::decl::PackageDecl, name: &str) -> bool {
    let options = pkg.features.iter().flat_map(|f| f.options.iter()).flat_map(|o| o.deps.iter());
    pkg.deps
        .iter()
        .chain(pkg.features.iter().flat_map(|f| f.deps.iter()))
        .chain(options)
        .any(|d| d.name == name)
}

/// The inside of a TOML basic string.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A key, bare when TOML allows it.
fn toml_key(s: &str) -> String {
    let bare = !s.is_empty() && s.bytes().all(is_bare_key_byte);
    if bare { s.to_string() } else { format!("\"{}\"", toml_escape(s)) }
}

fn render_key(path: &[String]) -> String {
    path.iter().map(|s| toml_key(s)).collect::<Vec<_>>().join(".")
}

fn is_bare_key_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// One key segment at the start of `s`: bare, "basic" or 'literal'.
/// None for anything else, including escapes this editor doesn't decode.
fn parse_key_segment(s: &str) -> Option<(String, &str)> {
    if let Some(r) = s.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = r.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '"' => return Some((out, &r[i + 1..])),
                '\\' => match chars.next()?.1 {
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    _ => return None,
                },
                _ => out.push(c),
            }
        }
        None
    } else if let Some(r) = s.strip_prefix('\'') {
        let end = r.find('\'')?;
        Some((r[..end].to_string(), &r[end + 1..]))
    } else {
        let end = s.bytes().position(|b| !is_bare_key_byte(b)).unwrap_or(s.len());
        (end > 0).then(|| (s[..end].to_string(), &s[end..]))
    }
}

/// A dotted key at the start of `s`, and what follows it.
fn parse_key(s: &str) -> Option<(Vec<String>, &str)> {
    let mut segments = Vec::new();
    let mut rest = s.trim_start();
    loop {
        let (seg, after) = parse_key_segment(rest)?;
        segments.push(seg);
        let after = after.trim_start();
        match after.strip_prefix('.') {
            Some(r) => rest = r.trim_start(),
            None => return Some((segments, after)),
        }
    }
}

/// `[features.ssl.dependencies]` → `["features", "ssl", "dependencies"]`.
fn header_of(line: &str) -> Option<Vec<String>> {
    let inner = line.trim_start().strip_prefix('[')?;
    if inner.starts_with('[') {
        return None;
    }
    let (path, rest) = parse_key(inner)?;
    let rest = rest.strip_prefix(']')?.trim_start();
    (rest.is_empty() || rest.starts_with('#')).then_some(path)
}

/// Whether a line starts a new table, `[[array]]` headers included.
fn starts_table(line: &str) -> bool {
    header_of(line).is_some() || line.trim_start().starts_with("[[")
}

/// The key a `key = value` line sets.
fn key_of(line: &str) -> Option<Vec<String>> {
    let (path, rest) = parse_key(line)?;
    rest.starts_with('=').then_some(path)
}

/// The dependency a full key path belongs to, if it sits under a
/// dependency table.
fn dep_named_by(path: &[String]) -> Option<&str> {
    let is_table = |s: &String| DEP_TABLES.contains(&s.as_str());
    let at = match path {
        [t, ..] if is_table(t) => 1,
        [f, _, t, ..] if f == "features" && is_table(t) => 3,
        // `[exclusive-features.rt.options.tokio]` holds that option's deps.
        [e, _, o, _, ..] if e == "exclusive-features" && o == "options" => 4,
        _ => return None,
    };
    path.get(at).map(String::as_str)
}

fn line_ending(content: &str) -> &'static str {
    if content.contains("\r\n") { "\r\n" } else { "\n" }
}

/// Put `entry` at the end of `[table]`'s own lines, creating the table at
/// the end of the file if it isn't there.
fn insert_entry(content: &str, table: &[String], entry: &str) -> String {
    let nl = line_ending(content);
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = lines.iter().position(|l| header_of(l).as_deref() == Some(table)) else {
        let mut out = content.trim_end().to_string();
        out.push_str(&format!("{nl}{nl}[{}]{nl}{}{nl}", render_key(table), entry));
        return out;
    };
    let end = lines[start + 1..]
        .iter()
        .position(|l| starts_table(l))
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
    out.join(nl) + nl
}

/// Drop `name`'s lines from every dependency table, and a whole
/// `[dependencies.name]` table if it's written that way. Comments after the
/// dropped table's last key stay: they usually introduce the next table.
fn remove_entry(content: &str, name: &str) -> (String, usize) {
    let nl = line_ending(content);
    let mut out: Vec<&str> = Vec::new();
    let mut removed = 0;
    let mut header: Vec<String> = Vec::new();
    let mut skipping = false;
    // Blank and comment lines met inside a dropped table, kept if no key follows.
    let mut held: Vec<&str> = Vec::new();
    for line in content.lines() {
        if let Some(h) = header_of(line) {
            out.append(&mut held);
            skipping = dep_named_by(&h) == Some(name);
            header = h;
            if skipping {
                removed += 1;
                continue;
            }
        } else if starts_table(line) {
            out.append(&mut held);
            skipping = false;
            header.clear();
        } else if skipping {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') {
                held.push(line);
            } else {
                held.clear();
            }
            continue;
        } else if let Some(key) = key_of(line) {
            let full: Vec<String> = header.iter().cloned().chain(key).collect();
            if dep_named_by(&full) == Some(name) {
                removed += 1;
                continue;
            }
        }
        out.push(line);
    }
    out.append(&mut held);
    (out.join(nl) + nl, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "schema = \"rask-package@1\"\n\n[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[dependencies]\nhttp = { registry = \"^2\" }  # keep me\n\n# trailing comment\n[features.ssl]\ndefault = true\n";

    fn path(s: &str) -> Vec<String> {
        s.split('.').map(str::to_string).collect()
    }

    fn add(existing: &str, name: &str, version: Option<&str>) -> Result<Added, String> {
        plan_add(Some(existing), "dir", &AddRequest::registry(name, version))
    }

    /// `rask add <args>` planned against `existing`, without the registry.
    fn planned(existing: Option<&str>, args: &[&str]) -> Result<Added, String> {
        plan_add(existing, "dir", &parse_add_args(args)?)
    }

    fn deps_of(text: &str) -> Vec<rask_ast::decl::DepDecl> {
        let pkg = rask_manifest::load_text(text).unwrap_or_else(|e| panic!("doesn't load: {e}\n{text}"));
        pkg.deps.into_iter().chain(pkg.features.into_iter().flat_map(|f| f.deps)).collect()
    }

    fn dep<'a>(deps: &'a [rask_ast::decl::DepDecl], name: &str) -> &'a rask_ast::decl::DepDecl {
        deps.iter().find(|d| d.name == name).unwrap_or_else(|| panic!("no dep {name}"))
    }

    // ─── Line editing ───────────────────────────────────────────────

    #[test]
    fn insert_goes_after_the_last_key_and_keeps_comments() {
        let out = insert_entry(BASE, &path("dependencies"), "json = { registry = \"^1\" }");
        assert!(out.contains("http = { registry = \"^2\" }  # keep me\njson = { registry = \"^1\" }\n\n# trailing comment"), "{out}");
    }

    #[test]
    fn insert_creates_a_missing_table() {
        let out = insert_entry(BASE, &path("dev-dependencies"), "mock = { path = \"../mock\" }");
        assert!(out.ends_with("\n\n[dev-dependencies]\nmock = { path = \"../mock\" }\n"), "{out}");
    }

    #[test]
    fn insert_finds_a_header_however_it_is_spelled() {
        let text = "[ features . \"ssl\" . dependencies ]  # tls\nopenssl = { registry = \"^3\" }\n";
        let out = insert_entry(text, &path("features.ssl.dependencies"), "x = { path = \"x\" }");
        assert_eq!(out, "[ features . \"ssl\" . dependencies ]  # tls\nopenssl = { registry = \"^3\" }\nx = { path = \"x\" }\n");
    }

    #[test]
    fn insert_keeps_crlf_line_endings() {
        let text = "[dependencies]\r\nhttp = { registry = \"^2\" }\r\n\r\n[package]\r\n";
        let out = insert_entry(text, &path("dependencies"), "json = { registry = \"^1\" }");
        assert_eq!(out, "[dependencies]\r\nhttp = { registry = \"^2\" }\r\njson = { registry = \"^1\" }\r\n\r\n[package]\r\n");
        let out = insert_entry(text, &path("dev-dependencies"), "m = { path = \"m\" }");
        assert!(out.ends_with("[package]\r\n\r\n[dev-dependencies]\r\nm = { path = \"m\" }\r\n"), "{out:?}");
    }

    #[test]
    fn remove_takes_the_line_and_nothing_else() {
        let (out, n) = remove_entry(BASE, "http");
        assert_eq!(n, 1);
        assert_eq!(out, BASE.replace("http = { registry = \"^2\" }  # keep me\n", ""));
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

    #[test]
    fn remove_keeps_the_comment_that_introduces_the_next_table() {
        let text = "[dependencies.tls]\n# pinned\ngit = \"u\"\nrev = \"a\"\n\n# SSL support\n[features.ssl]\ndefault = true\n";
        let (out, n) = remove_entry(text, "tls");
        assert_eq!(n, 1);
        assert_eq!(out, "\n# SSL support\n[features.ssl]\ndefault = true\n");
    }

    #[test]
    fn remove_reads_quoted_and_dotted_keys() {
        let text = "[dependencies]\n\"my.pkg\" = { path = \"p\" }\nhttp.registry = \"^2\"\n\n[dependencies.'a.b']\npath = \"q\"\n";
        let (out, n) = remove_entry(text, "my.pkg");
        assert_eq!((out.as_str(), n), ("[dependencies]\nhttp.registry = \"^2\"\n\n[dependencies.'a.b']\npath = \"q\"\n", 1));
        let (out, n) = remove_entry(text, "http");
        assert_eq!(n, 1);
        assert!(!out.contains("http"), "{out}");
        let (out, n) = remove_entry(text, "a.b");
        assert_eq!(n, 1);
        assert_eq!(out, "[dependencies]\n\"my.pkg\" = { path = \"p\" }\nhttp.registry = \"^2\"\n\n");
        // `my` alone is not `my.pkg`.
        assert_eq!(remove_entry(text, "my").1, 0);
    }

    #[test]
    fn remove_reaches_dependency_tables_written_as_dotted_keys() {
        let text = "[features.ssl]\ndefault = true\ndependencies.openssl = { registry = \"^3\" }\n";
        let (out, n) = remove_entry(text, "openssl");
        assert_eq!((out.as_str(), n), ("[features.ssl]\ndefault = true\n", 1));
    }

    #[test]
    fn remove_keeps_crlf_line_endings() {
        let text = "[dependencies]\r\na = { path = \"a\" }\r\nb = { path = \"b\" }\r\n";
        assert_eq!(remove_entry(text, "a").0, "[dependencies]\r\nb = { path = \"b\" }\r\n");
    }

    #[test]
    fn a_multiline_array_row_is_not_a_header() {
        assert_eq!(header_of("  [\"a\", \"b\"],"), None);
        assert_eq!(header_of("[dependencies] # c"), Some(path("dependencies")));
        assert_eq!(header_of("[[bin]]"), None);
        assert!(starts_table("[[bin]]"));
    }

    // ─── rask add, through the loader ───────────────────────────────

    #[test]
    fn add_registry_version_lands_in_dependencies() {
        let added = add(BASE, "json", Some("^1.5")).unwrap();
        assert_eq!(added.entry, "json = { registry = \"^1.5\" }");
        assert_eq!(added.table, "dependencies");
        let deps = deps_of(&added.content);
        let json = dep(&deps, "json");
        assert_eq!(json.version.as_deref(), Some("^1.5"));
        assert_eq!(json.scope, None);
        assert_eq!(json.feature, None);
    }

    #[test]
    fn add_without_a_version_takes_any() {
        let added = add(BASE, "json", None).unwrap();
        assert_eq!(dep(&deps_of(&added.content), "json").version.as_deref(), Some("*"));
        // A quoted version from the shell loses its quotes.
        let added = planned(Some(BASE), &["json", "\"^1\""]).unwrap();
        assert_eq!(added.entry, "json = { registry = \"^1\" }");
    }

    #[test]
    fn add_path_dependency() {
        let added = planned(Some(BASE), &["shared", "--path", "../my \"shared\""]).unwrap();
        assert_eq!(dep(&deps_of(&added.content), "shared").path.as_deref(), Some("../my \"shared\""));
    }

    #[test]
    fn add_refuses_both_a_version_and_a_path() {
        let err = planned(Some(BASE), &["x", "1.0", "--path", "../x"]).unwrap_err();
        assert!(err.contains("one source"), "{err}");
    }

    #[test]
    fn add_dev_dependency_creates_the_table() {
        let added = planned(Some(BASE), &["mock", "^2.0", "--dev"]).unwrap();
        assert_eq!(added.table, "dev-dependencies");
        assert!(added.content.starts_with(BASE.trim_end()), "{}", added.content);
        assert_eq!(dep(&deps_of(&added.content), "mock").scope.as_deref(), Some("dev"));
    }

    #[test]
    fn add_to_a_feature() {
        let added = planned(Some(BASE), &["openssl", "^3", "--feature", "ssl"]).unwrap();
        assert_eq!(added.table, "features.ssl.dependencies");
        let pkg = rask_manifest::load_text(&added.content).unwrap();
        let ssl = pkg.features.iter().find(|f| f.name == "ssl").unwrap();
        assert!(ssl.on_by_default, "the existing [features.ssl] keeps its keys");
        assert_eq!(ssl.deps.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["openssl"]);

        let added = planned(Some(BASE), &["bench", "--dev", "--feature", "perf tools"]).unwrap();
        assert_eq!(added.table, "features.\"perf tools\".dev-dependencies");
        let pkg = rask_manifest::load_text(&added.content).unwrap();
        let f = pkg.features.iter().find(|f| f.name == "perf tools").unwrap();
        assert_eq!(f.deps[0].scope.as_deref(), Some("dev"));
    }

    #[test]
    fn add_next_to_a_table_form_dependency() {
        let text = "schema = \"rask-package@1\"\n\n[package]\nname = \"a\"\nversion = \"1.0.0\"\n\n[dependencies.tls]\ngit = \"https://x/tls\"\nbranch = \"main\"\n";
        let added = add(text, "json", Some("^1")).unwrap();
        let deps = deps_of(&added.content);
        assert_eq!(dep(&deps, "tls").branch.as_deref(), Some("main"));
        assert_eq!(dep(&deps, "json").version.as_deref(), Some("^1"));
    }

    #[test]
    fn add_keeps_every_other_line() {
        let text = "schema = \"rask-package@1\"   # schema\n\n# The package\n[package]\nname    = \"a\"\nversion = \"1.0.0\"\n\n[dependencies]\n# web\nhttp   = { registry = \"^2.0\", allow = [\"net\"] }\ntls    = { git = \"https://x/tls\", rev = \"a1b2c3\" }\n# end of deps\n\n[dev-dependencies]\nmock = { path = \"../mock\" }\n";
        let added = add(text, "json", Some("^1")).unwrap();
        let expected = text.replace(
            "rev = \"a1b2c3\" }\n",
            "rev = \"a1b2c3\" }\njson = { registry = \"^1\" }\n",
        );
        assert_eq!(added.content, expected);
    }

    #[test]
    fn add_refuses_a_name_already_declared_anywhere() {
        let text = "schema = \"rask-package@1\"\n\n[package]\nname = \"a\"\nversion = \"1.0.0\"\n\n[build-dependencies]\ngen = { git = \"u\", rev = \"r\" }\n\n[features.ssl.dev-dependencies]\nmock = { path = \"m\" }\n";
        for name in ["gen", "mock"] {
            let err = add(text, name, None).unwrap_err();
            // `rask update` takes a directory, not a package, so that's what it names.
            assert!(err.contains(&format!("\"{name}\" is already in rask.toml. Use `rask update`")), "{err}");
        }
        let err = add(BASE, "http", Some("^3")).unwrap_err();
        assert!(err.contains("already"), "{err}");
    }

    #[test]
    fn add_quotes_a_name_toml_would_misread() {
        let added = add(BASE, "my.pkg", Some("^1")).unwrap();
        assert_eq!(added.entry, "\"my.pkg\" = { registry = \"^1\" }");
        let deps = deps_of(&added.content);
        assert_eq!(dep(&deps, "my.pkg").version.as_deref(), Some("^1"));
        assert!(!deps.iter().any(|d| d.name == "my"), "a bare my.pkg would be a table `my`");
    }

    #[test]
    fn add_refuses_an_edit_that_would_not_load() {
        let err = add(BASE, "json", Some("two")).unwrap_err();
        assert!(err.contains("wouldn't be written") || err.contains("wasn't written"), "{err}");
        assert!(err.contains("dependencies.json.registry"), "names the problem: {err}");
    }

    #[test]
    fn add_refuses_a_manifest_that_already_does_not_load() {
        let broken = BASE.replace("version = \"0.1.0\"", "version = \"one\"");
        let err = add(&broken, "json", Some("^1")).unwrap_err();
        assert!(err.contains("doesn't load, so nothing was added") && err.contains("package.version"), "{err}");
    }

    #[test]
    fn add_starts_a_manifest_when_there_is_none() {
        let added = plan_add(None, "my \"app\"", &AddRequest::registry("http", Some("^2"))).unwrap();
        let pkg = rask_manifest::load_text(&added.content).unwrap();
        assert_eq!(pkg.name, "my \"app\"");
        assert_eq!(pkg.version, "0.1.0");
        let schema = rask_manifest::schema_line().unwrap();
        assert!(added.content.starts_with(&format!("schema = \"{schema}\"\n")), "{}", added.content);
        assert_eq!(dep(&deps_of(&added.content), "http").version.as_deref(), Some("^2"));
    }

    // ─── rask remove, through the loader ────────────────────────────

    const EVERY_TABLE: &str = "schema = \"rask-package@1\"\n\n[package]\nname = \"a\"\nversion = \"1.0.0\"\n\n[dependencies]\nhttp = { registry = \"^2\" }  # web\njson = { registry = \"^1\" }\n\n[dev-dependencies]\nmock = { path = \"../mock\" }\n\n[build-dependencies]\ngen = { git = \"https://x/gen\", branch = \"main\" }\n\n[dependencies.tls]\ngit = \"https://x/tls\"\nrev = \"a1b2c3\"\nwith = [\"ring\"]\n\n# optional TLS\n[features.ssl]\ndefault = true\n\n[features.ssl.dependencies]\nopenssl = { registry = \"^3\", allow = [\"ffi\"] }\n";

    #[test]
    fn remove_from_each_table() {
        for name in ["http", "json", "mock", "gen", "tls", "openssl"] {
            let out = plan_remove(EVERY_TABLE, name).unwrap_or_else(|e| panic!("{name}: {e}"));
            let left: Vec<String> = deps_of(&out).into_iter().map(|d| d.name).collect();
            assert!(!left.contains(&name.to_string()), "{name} still there:\n{out}");
            assert_eq!(left.len(), 5, "{name} took others with it:\n{out}");
            assert!(out.contains("# optional TLS"), "{name} dropped a comment:\n{out}");
            assert!(name == "http" || out.contains("http = { registry = \"^2\" }  # web"), "{out}");
        }
    }

    #[test]
    fn remove_a_name_that_is_not_there() {
        let err = plan_remove(EVERY_TABLE, "nope").unwrap_err();
        assert_eq!(err, "\"nope\" isn't a dependency in rask.toml");
        let err = plan_remove(EVERY_TABLE, "ssl").unwrap_err();
        assert!(err.contains("isn't a dependency"), "a feature is not a dependency: {err}");
    }

    #[test]
    fn remove_says_when_it_cannot_edit_the_form() {
        let text = "schema = \"rask-package@1\"\n\n[package]\nname = \"a\"\nversion = \"1.0.0\"\n\n[features.ssl]\ndependencies = { openssl = { registry = \"^3\" } }\n";
        let err = plan_remove(text, "openssl").unwrap_err();
        assert!(err.contains("can't edit line by line"), "{err}");
    }

    #[test]
    fn remove_refuses_a_manifest_that_does_not_load() {
        let broken = EVERY_TABLE.replace("version = \"1.0.0\"", "version = \"one\"");
        let err = plan_remove(&broken, "http").unwrap_err();
        assert!(err.contains("doesn't load, so nothing was removed") && err.contains("package.version"), "{err}");
    }

    const GROUPS: &str = "schema = \"rask-package@1\"\n\n[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[exclusive-features.rt]\ndefault = \"tokio\"\n\n[exclusive-features.rt.options.tokio]\ntokio = { registry = \"^1\" }  # the default\n\n[exclusive-features.rt.options.smol]\nsmol = { registry = \"^2\" }\n";

    #[test]
    fn add_refuses_a_name_an_exclusive_option_declares() {
        let err = add(GROUPS, "tokio", Some("^1")).unwrap_err();
        assert!(err.contains("\"tokio\" is already in rask.toml"), "{err}");
    }

    #[test]
    fn remove_reaches_a_dependency_in_an_exclusive_option() {
        let out = plan_remove(GROUPS, "smol").unwrap();
        assert!(!out.contains("smol = "), "{out}");
        assert!(out.contains("[exclusive-features.rt.options.smol]\n"), "the option itself stays: {out}");
        assert!(out.contains("tokio = { registry = \"^1\" }  # the default"), "{out}");
    }

    #[test]
    fn remove_says_so_for_an_option_written_inline() {
        let text = "schema = \"rask-package@1\"\n\n[package]\nname = \"a\"\nversion = \"0.1.0\"\n\n[exclusive-features.rt]\ndefault = \"tokio\"\noptions.tokio = { tokio = { registry = \"^1\" } }\n";
        let err = plan_remove(text, "tokio").unwrap_err();
        assert!(err.contains("can't edit line by line"), "{err}");
    }


    // ─── Arguments ──────────────────────────────────────────────────

    #[test]
    fn every_flag_reaches_the_entry() {
        let added = planned(Some(BASE), &["tls", "--git", "https://x/tls", "--rev", "a1b2", "--with", "rustls, alpn", "--allow", "net,ffi", "--target", "linux", "--build"]).unwrap();
        assert_eq!(added.table, "build-dependencies");
        assert_eq!(
            added.entry,
            "tls = { git = \"https://x/tls\", rev = \"a1b2\", with = [\"rustls\", \"alpn\"], allow = [\"net\", \"ffi\"], target = \"linux\" }"
        );
        let deps = deps_of(&added.content);
        let tls = dep(&deps, "tls");
        assert_eq!((tls.git.as_deref(), tls.rev.as_deref(), tls.scope.as_deref()), (Some("https://x/tls"), Some("a1b2"), Some("build")));
        assert_eq!(tls.with_features, ["rustls", "alpn"]);
        assert_eq!(tls.allow, ["net", "ffi"]);
        assert_eq!(tls.target.as_deref(), Some("linux"));
    }

    #[test]
    fn a_git_branch_is_written_too() {
        let added = planned(Some(BASE), &["tls", "--git", "https://x/tls", "--branch", "main"]).unwrap();
        assert_eq!(added.entry, "tls = { git = \"https://x/tls\", branch = \"main\" }");
    }

    #[test]
    fn argument_mistakes_are_named() {
        let err = |args: &[&str]| parse_add_args(args).unwrap_err();
        assert_eq!(err(&[]), "missing package name");
        assert!(err(&["a", "1", "2"]).contains("too many arguments: 2"));
        assert!(err(&["a", "--git", "u", "--path", "p"]).contains("one source"));
        assert!(err(&["a", "--rev", "x"]).contains("go with `--git`"));
        assert!(err(&["a", "--dev", "--build"]).contains("pick one"));
        assert!(err(&["a", "--feature"]).contains("--feature needs a value"));
        assert!(err(&["a", "--frobnicate"]).contains("unknown flag `--frobnicate`"));
    }

    #[test]
    fn flags_may_come_before_the_name() {
        let req = parse_add_args(&["--dev", "--feature", "x", "mock", "^2"]).unwrap();
        assert_eq!((req.name.as_str(), req.table, req.feature.as_deref()), ("mock", "dev-dependencies", Some("x")));
        assert_eq!(req.source, Source::Registry(Some("^2".into())));
    }

    #[test]
    fn an_empty_allow_writes_nothing() {
        let mut req = AddRequest::registry("json", Some("^1"));
        req.allow = Some(Vec::new());
        assert_eq!(entry_spec(&req), "{ registry = \"^1\" }");
    }

}
