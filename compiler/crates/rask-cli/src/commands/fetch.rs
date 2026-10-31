// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dependency resolution and the lock file — struct.packages/LK1-LK8,
//! RG1-RG4, struct.build/SG4.
//!
//! [`resolve_and_lock`] is the one place rask.lock is written from scratch.
//! It sees every scope (dev and build deps too) and every feature, because
//! the lock pins what any build might need. `rask fetch`, `rask update`,
//! `rask add`, `rask remove` and a build with no lock yet all go through it.

use colored::Colorize;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::process;

use rask_ast::decl::{DepDecl, PackageDecl};
use rask_resolve::{LockFile, PackageId, PackageRegistry};

use crate::output;

/// Whether versions already in rask.lock hold (fetch) or are re-resolved to
/// the newest that fits (update).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pins {
    Keep,
    Ignore,
}

/// What a resolution found and wrote.
pub struct Resolved {
    pub registry: PackageRegistry,
    pub root_ids: Vec<PackageId>,
    /// None when nothing is external, and any old rask.lock was removed.
    pub lock: Option<LockFile>,
    /// Registry packages downloaded this time, not found in the cache.
    pub downloaded: usize,
    pub lock_changed: bool,
    /// Packages using a capability their entry doesn't allow.
    pub capability_warnings: usize,
}

/// `rask fetch`: download what's missing and bring rask.lock up to date.
pub fn cmd_fetch(path: &str, verbose: bool) {
    let root = project_root(path);
    if !root.join(rask_resolve::package::MANIFEST_FILE).exists() {
        println!("  {} (no rask.toml, nothing to fetch)", "OK".green());
        return;
    }
    let resolved = resolve_and_lock(&root, Pins::Keep, verbose).unwrap_or_else(|e| fail("Fetch", &e));
    report(&resolved, "Fetched");
}

/// `rask update`: re-resolve every registry package to the newest version
/// its constraints allow, and rewrite rask.lock.
pub fn cmd_update(path: &str) {
    let root = project_root(path);
    let resolved = resolve_and_lock(&root, Pins::Ignore, false).unwrap_or_else(|e| fail("Update", &e));
    report(&resolved, "Updated");
}

fn project_root(path: &str) -> PathBuf {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }
    root
}

fn fail(phase: &str, errors: &[String]) -> ! {
    for e in errors {
        eprintln!("{}: {}", output::error_label(), e);
    }
    eprintln!("\n{}", output::banner_fail(phase, errors.len()));
    process::exit(1);
}

fn report(r: &Resolved, verb: &str) {
    let Some(lock) = &r.lock else {
        println!("  {} (no external dependencies)", "OK".green());
        return;
    };
    let n = lock.packages.len();
    let downloaded = if r.downloaded > 0 { format!("{} downloaded, ", r.downloaded) } else { String::new() };
    println!(
        "  {} {} package{} ({}{})",
        verb.green().bold(),
        n,
        if n == 1 { "" } else { "s" },
        downloaded,
        if r.lock_changed { "lock file updated" } else { "up to date" },
    );
    if r.capability_warnings > 0 {
        println!(
            "  {} {} capability warning{}",
            "Note:".yellow(),
            r.capability_warnings,
            if r.capability_warnings == 1 { "" } else { "s" },
        );
    }
}

/// Every dependency a manifest declares, in any scope, feature or exclusive
/// option.
pub fn all_deps(m: &PackageDecl) -> impl Iterator<Item = &DepDecl> {
    m.deps.iter().chain(m.features.iter().flat_map(|f| {
        f.deps.iter().chain(f.options.iter().flat_map(|o| o.deps.iter()))
    }))
}

/// Discover every package in every scope, fetch the registry packages they
/// need, and write rask.lock. On failure nothing is written and every
/// problem comes back, one per string.
pub fn resolve_and_lock(root: &Path, pins: Pins, verbose: bool) -> Result<Resolved, Vec<String>> {
    let mut registry = PackageRegistry::new();
    registry.include_scope("dev");
    registry.include_scope("build");
    let root_ids = registry.discover_workspace(root).map_err(|e| vec![e.to_string()])?;
    let is_workspace = root_ids.len() > 1;
    let root_id = root_ids[0];
    let manifests: Vec<PackageDecl> = root_ids
        .iter()
        .filter_map(|id| registry.get(*id).and_then(|p| p.manifest.clone()))
        .collect();

    let lock_path = root.join("rask.lock");
    let existing_lock = if lock_path.exists() { LockFile::load(&lock_path).ok() } else { None };

    let mut errors = Vec::new();
    // Discovery passes over a path that isn't there, so it's caught here,
    // where the lock would otherwise quietly leave it out.
    for id in &root_ids {
        let Some(pkg) = registry.get(*id) else { continue };
        let Some(m) = &pkg.manifest else { continue };
        for d in all_deps(m) {
            if let Some(p) = &d.path {
                let dir = pkg.root_dir.join(p);
                if !dir.is_dir() {
                    errors.push(format!("dep \"{}\": path not found: {}", d.name, dir.display()));
                }
            }
        }
    }
    let mut downloaded = 0;
    // LK8: signing key fingerprints of the registry packages fetched now.
    let mut signing_keys: BTreeMap<String, String> = BTreeMap::new();

    // Direct registry deps first, then whatever their index entries name.
    let mut queue: Vec<(String, String)> = Vec::new();
    let mut queued: HashSet<String> = HashSet::new();
    for m in &manifests {
        for d in all_deps(m) {
            if let (Some(req), None, None) = (&d.version, &d.path, &d.git) {
                if queued.insert(d.name.clone()) {
                    queue.push((d.name.clone(), req.clone()));
                }
            }
        }
    }

    if !queue.is_empty() {
        let reg_config = rask_resolve::registry::RegistryConfig::from_env();
        let cache = rask_resolve::cache::PackageCache::new();
        // VD4: a vendored copy wins over the cache.
        let vendor_dir: Option<PathBuf> =
            manifests.iter().find_map(|m| m.meta("vendor_dir")).map(|v| root.join(v));
        if verbose {
            println!("  {} {}", "Registry".dimmed(), reg_config.url);
        }

        while let Some((name, req)) = queue.pop() {
            let constraint = match rask_resolve::semver::Constraint::parse(&req) {
                Ok(c) => c,
                Err(e) => {
                    errors.push(format!("dep \"{}\": {}", name, e));
                    continue;
                }
            };
            let index = match reg_config.fetch_index(&name) {
                Ok(i) => i,
                Err(e) => {
                    errors.push(format!("dep \"{}\": {}", name, e));
                    continue;
                }
            };
            let pinned = (pins == Pins::Keep)
                .then(|| existing_lock.as_ref())
                .flatten()
                .and_then(|lock| lock.packages.iter().find(|p| p.name == name && p.source.starts_with("registry+")))
                .and_then(|p| rask_resolve::semver::Version::parse(&p.version).ok())
                .filter(|v| constraint.matches(v));
            let version = match pinned {
                Some(v) => v,
                None => match resolve_from_index(&index, &[constraint], &name) {
                    Ok(v) => v,
                    Err(e) => {
                        errors.push(format!("dep \"{}\": {}", name, e));
                        continue;
                    }
                },
            };
            let version_str = version.to_string();
            let Some(meta) = index.versions.get(&version_str) else {
                errors.push(format!("dep \"{}\": version {} not in index", name, version_str));
                continue;
            };

            let vendored = vendor_dir.as_ref().map(|vd| vd.join(format!("{}-{}", name, version_str))).filter(|p| {
                std::fs::read_to_string(p.join(".checksum")).is_ok_and(|s| s.trim() == meta.checksum)
            });
            let dir = if let Some(p) = vendored {
                if verbose {
                    println!("  {} \"{}\" {} (vendored)", "Dep".dimmed(), name, version_str);
                }
                p
            } else if let Some(p) = cache.get(&name, &version_str, &meta.checksum) {
                if verbose {
                    println!("  {} \"{}\" {} (cached)", "Dep".dimmed(), name, version_str);
                }
                p
            } else {
                if verbose {
                    println!("  {} \"{}\" {} ...", "Fetching".cyan(), name, version_str);
                }
                let archive = std::env::temp_dir().join(format!("{}-{}.tar.gz", name, version_str));
                if let Err(e) = reg_config.download_archive(&name, &version_str, &archive) {
                    errors.push(format!("dep \"{}\": download failed: {}", name, e));
                    continue;
                }
                // SG4: a signed package is checked against its signature
                // before it reaches the cache.
                if let (Some(sig), Some(pubkey)) = (&meta.signature, &meta.signing_pubkey) {
                    let bytes = std::fs::read(&archive).unwrap_or_default();
                    if rask_resolve::signing::verify_signature(&bytes, sig, pubkey).is_err() {
                        let _ = std::fs::remove_file(&archive);
                        errors.push(format!(
                            "[struct.build/SG4] package '{}' {}: the signature doesn't match its key",
                            name, version_str
                        ));
                        continue;
                    }
                }
                let stored = cache.store(&name, &version_str, &archive, &meta.checksum);
                let _ = std::fs::remove_file(&archive);
                match stored {
                    Ok(p) => {
                        downloaded += 1;
                        p
                    }
                    Err(e) => {
                        errors.push(format!("dep \"{}\": {}", name, e));
                        continue;
                    }
                }
            };
            if let Some(pubkey) = &meta.signing_pubkey {
                if let Ok(fp) = rask_resolve::signing::fingerprint_from_pubkey_hex(pubkey) {
                    signing_keys.insert(name.clone(), fp);
                }
            }
            if let Err(e) = registry.register_cached(&name, &version_str, &dir, &reg_config.url) {
                errors.push(format!("dep \"{}\": failed to register: {}", name, e));
                continue;
            }
            for t in &meta.deps {
                if queued.insert(t.name.clone()) {
                    queue.push((t.name.clone(), t.version.clone()));
                }
            }
        }
    }
    if !errors.is_empty() {
        return Err(errors);
    }

    let external: Vec<_> = registry.packages().iter().filter(|p| !root_ids.contains(&p.id) && p.is_external).collect();
    if external.is_empty() {
        if lock_path.exists() {
            std::fs::remove_file(&lock_path).map_err(|e| vec![format!("removing rask.lock: {}", e)])?;
        }
        return Ok(Resolved { registry, root_ids, lock: None, downloaded, lock_changed: existing_lock.is_some(), capability_warnings: 0 });
    }

    // PM6: what each external package does, recorded in the lock.
    let mut all_caps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pkg in &external {
        let decls: Vec<_> = pkg.all_decls().cloned().collect();
        let inferred = rask_resolve::capabilities::infer_capabilities(&decls);
        if verbose && !inferred.is_empty() {
            println!("  {} '{}' uses: [{}]", "Capabilities".dimmed(), pkg.name, inferred.join(", "));
        }
        all_caps.insert(pkg.name.clone(), inferred);
    }
    let capability_warnings = warn_unallowed(&manifests, &all_caps);

    let mut lock = if is_workspace {
        LockFile::generate_workspace_with_capabilities(&registry, &root_ids, root, &all_caps)
    } else {
        LockFile::generate_with_capabilities(&registry, root_id, root, &all_caps)
    };
    // LK8: keys seen now, else the key the old lock pinned.
    for pkg in &mut lock.packages {
        pkg.signing_key = signing_keys.get(&pkg.name).cloned().or_else(|| {
            existing_lock.as_ref()?.packages.iter().find(|p| p.name == pkg.name)?.signing_key.clone()
        });
    }
    // Claims sources aren't dependencies: the registry doesn't see them, and
    // their pins carry over until the claims step rewrites them (type.claims/W3).
    if let Some(old) = &existing_lock {
        lock.claims = old.claims.clone();
    }
    if let Some(old) = &existing_lock {
        for (name, old_key, new_key) in old.signing_keys_changed(&signing_keys) {
            eprintln!("  {} signing key changed for '{}'", "Warning:".yellow(), name);
            eprintln!("    old: {}", old_key);
            eprintln!("    new: {}", new_key);
            eprintln!("    Verify this is an authorized key rotation.");
        }
        for name in old.capabilities_changed(&all_caps) {
            println!("  {} capabilities changed for '{}': [{}]", "Warning:".yellow().bold(), name, all_caps[&name].join(", "));
        }
    }

    let before = std::fs::read_to_string(&lock_path).ok();
    let lock_changed = before.as_deref() != Some(lock.render().as_str());
    if lock_changed {
        lock.write(&lock_path).map_err(|e| vec![e])?;
    }
    Ok(Resolved { registry, root_ids, lock: Some(lock), downloaded, lock_changed, capability_warnings })
}

/// PM1: a warning for each capability a package uses that no entry for it
/// allows. Returns how many.
fn warn_unallowed(manifests: &[PackageDecl], caps: &BTreeMap<String, Vec<String>>) -> usize {
    let mut allows: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for m in manifests {
        for d in all_deps(m) {
            allows.entry(d.name.as_str()).or_default().extend(d.allow.iter().cloned());
        }
    }
    let mut count = 0;
    for (name, used) in caps {
        let allowed = allows.get(name.as_str()).cloned().unwrap_or_default();
        for cap in rask_resolve::capabilities::check_capabilities(used, &allowed) {
            eprintln!(
                "  {} '{}' uses {} — add allow = [\"{}\"] to its entry in rask.toml",
                "Warning:".yellow(),
                name,
                rask_resolve::capabilities::capability_description(&cap),
                cap,
            );
            count += 1;
        }
    }
    count
}

/// The newest version, not yanked, that satisfies every constraint.
pub fn resolve_from_index(
    index: &rask_resolve::registry::PackageIndex,
    constraints: &[rask_resolve::semver::Constraint],
    pkg_name: &str,
) -> Result<rask_resolve::semver::Version, String> {
    let available: Vec<rask_resolve::semver::Version> = index
        .versions
        .iter()
        .filter(|(_, meta)| !meta.yanked)
        .filter_map(|(v, _)| rask_resolve::semver::Version::parse(v).ok())
        .collect();
    if available.is_empty() {
        return Err(format!("no versions available for '{}'", pkg_name));
    }
    rask_resolve::semver::resolve_version(constraints, &available)
}
