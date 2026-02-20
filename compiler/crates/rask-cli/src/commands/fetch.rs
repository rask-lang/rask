// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dependency fetching — struct.packages/LK2, RG1-RG4.
//!
//! `rask fetch` resolves dependencies, validates version constraints,
//! and updates the lock file. For path dependencies, validates the
//! dependency graph. For registry dependencies, downloads and caches
//! packages from the remote registry.

use colored::Colorize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process;

use crate::output;

/// Fetch and validate all dependencies, updating rask.lock.
pub fn cmd_fetch(path: &str, verbose: bool) {
    use rask_resolve::PackageRegistry;
    use std::collections::BTreeMap;

    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));

    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    let build_rk = root.join("build.rk");
    if !build_rk.exists() {
        println!("  {} (no build.rk, nothing to fetch)", "OK".green());
        return;
    }

    // Discover packages (workspace-aware)
    let mut registry = PackageRegistry::new();
    let root_ids = match registry.discover_workspace(&root) {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    };
    let is_workspace = root_ids.len() > 1;
    let root_id = root_ids[0];

    // Collect manifests from all workspace members
    let manifests: Vec<_> = root_ids.iter()
        .filter_map(|id| registry.get(*id).and_then(|p| p.manifest.clone()))
        .collect();

    // Validate version constraints on declared deps (all workspace members)
    let mut constraint_errors = 0;
    for manifest in &manifests {
        for dep in &manifest.deps {
            if let Some(ref version) = dep.version {
                if let Err(e) = rask_resolve::semver::Constraint::parse(version) {
                    eprintln!("{}: dep \"{}\": invalid version constraint \"{}\": {}",
                        output::error_label(), dep.name, version, e);
                    constraint_errors += 1;
                } else if verbose {
                    let desc = rask_resolve::semver::validate_constraint(version)
                        .unwrap_or_default();
                    println!("  {} \"{}\" {} ({})", "Dep".dimmed(), dep.name, version, desc);
                }
            } else if dep.path.is_some() {
                if verbose {
                    let p = dep.path.as_deref().unwrap_or("?");
                    println!("  {} \"{}\" (path: {})", "Dep".dimmed(), dep.name, p);
                }
            }

            // Validate path deps exist
            if let Some(ref dep_path) = dep.path {
                let resolved = root.join(dep_path);
                if !resolved.exists() {
                    eprintln!("{}: dep \"{}\": path not found: {}",
                        output::error_label(), dep.name, resolved.display());
                    constraint_errors += 1;
                } else if !resolved.is_dir() {
                    eprintln!("{}: dep \"{}\": path is not a directory: {}",
                        output::error_label(), dep.name, resolved.display());
                    constraint_errors += 1;
                }
            }

            // Validate git deps have required fields
            if dep.git.is_some() && dep.path.is_some() {
                eprintln!("{}: dep \"{}\": cannot have both 'path' and 'git'",
                    output::error_label(), dep.name);
                constraint_errors += 1;
            }
        }

        // Check for duplicate deps (D3)
        let mut seen_deps: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for dep in &manifest.deps {
            if !seen_deps.insert(&dep.name) {
                eprintln!("{}: duplicate dep \"{}\" in build.rk",
                    output::error_label(), dep.name);
                constraint_errors += 1;
            }
        }

        // Validate feature declarations
        for feat in &manifest.features {
            if feat.exclusive && feat.default.is_none() {
                eprintln!("{}: exclusive feature \"{}\" requires a default option (FG2)",
                    output::error_label(), feat.name);
                constraint_errors += 1;
            }
            if feat.exclusive {
                for opt in &feat.options {
                    if opt.name.is_empty() {
                        eprintln!("{}: empty option name in exclusive feature \"{}\"",
                            output::error_label(), feat.name);
                        constraint_errors += 1;
                    }
                }
                // Validate default is a valid option
                if let Some(ref default) = feat.default {
                    if !feat.options.iter().any(|o| o.name == *default) {
                        eprintln!("{}: default \"{}\" is not a valid option for feature \"{}\"",
                            output::error_label(), default, feat.name);
                        constraint_errors += 1;
                    }
                }
            }
        }

        // Validate profiles
        for profile in &manifest.profiles {
            if profile.name == "debug" || profile.name == "release" {
                // Built-in profiles can be customized
                continue;
            }
            // Custom profiles should have inherits
            let has_inherits = profile.settings.iter()
                .any(|(k, _)| k == "inherits");
            if !has_inherits {
                eprintln!("warning: custom profile \"{}\" has no 'inherits' — defaults to debug settings",
                    profile.name);
            }
        }
    }

    if constraint_errors > 0 {
        eprintln!("\n{}", output::banner_fail("Fetch", constraint_errors));
        process::exit(1);
    }

    // =========================================================================
    // Resolve registry dependencies (RG1)
    // =========================================================================

    // Collect registry deps from all workspace members.
    let mut registry_deps: Vec<(String, String)> = Vec::new();
    let mut seen_reg_deps = HashSet::new();
    for m in &manifests {
        for d in &m.deps {
            if d.version.is_some() && d.path.is_none() && d.git.is_none() {
                if seen_reg_deps.insert(d.name.clone()) {
                    registry_deps.push((d.name.clone(), d.version.clone().unwrap()));
                }
            }
        }
    }

    let mut registry_fetched = 0;

    if !registry_deps.is_empty() {
        // Load existing lock file for pinned versions
        let lock_path = root.join("rask.lock");
        let existing_lock = if lock_path.exists() {
            rask_resolve::LockFile::load(&lock_path).ok()
        } else {
            None
        };

        let reg_config = rask_resolve::registry::RegistryConfig::from_env();
        let cache = rask_resolve::cache::PackageCache::new();

        // VD4: Check vendor_dir for offline-first resolution
        let vendor_dir: Option<PathBuf> = manifests.iter()
            .find_map(|m| m.meta("vendor_dir"))
            .map(|v| root.join(v));

        if verbose {
            println!("  {} {}", "Registry".dimmed(), reg_config.url);
        }

        let mut fetch_errors = 0;
        let mut resolved_packages: Vec<(String, String)> = Vec::new();
        let mut visited: HashSet<String> = HashSet::new();

        // Build work queue starting from direct registry deps
        let mut queue: Vec<(String, String)> = registry_deps.clone();

        while let Some((dep_name, constraint_str)) = queue.pop() {
            if visited.contains(&dep_name) {
                continue;
            }
            visited.insert(dep_name.clone());

            // Check lock file for a pinned version
            let pinned_version = existing_lock.as_ref().and_then(|lock| {
                lock.packages.iter()
                    .find(|p| p.name == dep_name && p.source.starts_with("registry+"))
                    .map(|p| p.version.clone())
            });

            let constraint = match rask_resolve::semver::Constraint::parse(&constraint_str) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("{}: dep \"{}\": {}", output::error_label(), dep_name, e);
                    fetch_errors += 1;
                    continue;
                }
            };

            // Fetch package index from registry
            let index = match reg_config.fetch_index(&dep_name) {
                Ok(idx) => idx,
                Err(e) => {
                    eprintln!("{}: dep \"{}\": {}", output::error_label(), dep_name, e);
                    fetch_errors += 1;
                    continue;
                }
            };

            // Resolve version: prefer pinned, otherwise pick newest compatible
            let resolved_version = if let Some(ref pinned) = pinned_version {
                // Verify pinned version still satisfies the constraint
                match rask_resolve::semver::Version::parse(pinned) {
                    Ok(v) if constraint.matches(&v) => v,
                    _ => {
                        // Pinned version no longer satisfies — re-resolve
                        match resolve_from_index(&index, &[constraint], &dep_name) {
                            Ok(v) => v,
                            Err(e) => {
                                eprintln!("{}: dep \"{}\": {}", output::error_label(), dep_name, e);
                                fetch_errors += 1;
                                continue;
                            }
                        }
                    }
                }
            } else {
                match resolve_from_index(&index, &[constraint], &dep_name) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("{}: dep \"{}\": {}", output::error_label(), dep_name, e);
                        fetch_errors += 1;
                        continue;
                    }
                }
            };

            let version_str = resolved_version.to_string();
            let meta = match index.versions.get(&version_str) {
                Some(m) => m,
                None => {
                    eprintln!("{}: dep \"{}\": version {} not in index",
                        output::error_label(), dep_name, version_str);
                    fetch_errors += 1;
                    continue;
                }
            };

            // VD4: Vendor dir takes priority over registry cache
            if let Some(ref vd) = vendor_dir {
                let vendor_pkg = vd.join(format!("{}-{}", dep_name, version_str));
                let checksum_file = vendor_pkg.join(".checksum");
                if vendor_pkg.is_dir() && checksum_file.is_file() {
                    if let Ok(stored) = std::fs::read_to_string(&checksum_file) {
                        if stored.trim() == meta.checksum {
                            if verbose {
                                println!("  {} \"{}\" {} (vendored)", "Dep".dimmed(), dep_name, version_str);
                            }
                            if let Err(e) = registry.register_cached(
                                &dep_name, &version_str, &vendor_pkg, &reg_config.url
                            ) {
                                eprintln!("{}: dep \"{}\": failed to register: {}",
                                    output::error_label(), dep_name, e);
                                fetch_errors += 1;
                            } else {
                                resolved_packages.push((dep_name.clone(), version_str));
                                for transitive in &meta.deps {
                                    if !visited.contains(&transitive.name) {
                                        queue.push((transitive.name.clone(), transitive.version.clone()));
                                    }
                                }
                            }
                            continue;
                        }
                    }
                }
            }

            // Check cache, download if needed
            let cached_path = match cache.get(&dep_name, &version_str, &meta.checksum) {
                Some(path) => {
                    if verbose {
                        println!("  {} \"{}\" {} (cached)", "Dep".dimmed(), dep_name, version_str);
                    }
                    path
                }
                None => {
                    if verbose {
                        println!("  {} \"{}\" {} ...", "Fetching".cyan(), dep_name, version_str);
                    }

                    // Download to temp file
                    let tmp_dir = std::env::temp_dir();
                    let tmp_archive = tmp_dir.join(format!("{}-{}.tar.gz", dep_name, version_str));

                    if let Err(e) = reg_config.download_archive(&dep_name, &version_str, &tmp_archive) {
                        eprintln!("{}: dep \"{}\": download failed: {}",
                            output::error_label(), dep_name, e);
                        fetch_errors += 1;
                        continue;
                    }

                    // Extract and verify checksum
                    match cache.store(&dep_name, &version_str, &tmp_archive, &meta.checksum) {
                        Ok(path) => {
                            // Clean up temp file
                            let _ = std::fs::remove_file(&tmp_archive);
                            registry_fetched += 1;
                            path
                        }
                        Err(e) => {
                            let _ = std::fs::remove_file(&tmp_archive);
                            eprintln!("{}: dep \"{}\": {}", output::error_label(), dep_name, e);
                            fetch_errors += 1;
                            continue;
                        }
                    }
                }
            };

            // Register in package registry
            if let Err(e) = registry.register_cached(
                &dep_name, &version_str, &cached_path, &reg_config.url
            ) {
                eprintln!("{}: dep \"{}\": failed to register: {}",
                    output::error_label(), dep_name, e);
                fetch_errors += 1;
                continue;
            }

            resolved_packages.push((dep_name.clone(), version_str));

            // Queue transitive registry deps from index metadata
            for transitive in &meta.deps {
                if !visited.contains(&transitive.name) {
                    queue.push((transitive.name.clone(), transitive.version.clone()));
                }
            }
        }

        if fetch_errors > 0 {
            eprintln!("\n{}", output::banner_fail("Fetch", fetch_errors));
            process::exit(1);
        }
    }

    // Count packages
    let external_count = registry.packages().iter()
        .filter(|p| p.id != root_id && p.is_external)
        .count();

    if external_count == 0 {
        println!("  {} (no external dependencies)", "OK".green());
        // Remove stale lock file if exists
        let lock_path = root.join("rask.lock");
        if lock_path.exists() {
            let _ = std::fs::remove_file(&lock_path);
            println!("  {} rask.lock (no longer needed)", "Removed".green());
        }
        return;
    }

    // Infer capabilities
    let mut all_caps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut cap_warnings = 0;
    for pkg in registry.packages() {
        if pkg.id == root_id { continue; }
        if !pkg.is_external { continue; }

        let decls: Vec<_> = pkg.all_decls().cloned().collect();
        let inferred = rask_resolve::capabilities::infer_capabilities(&decls);

        if verbose && !inferred.is_empty() {
            println!("  {} '{}' uses: [{}]",
                "Capabilities".dimmed(), pkg.name, inferred.join(", "));
        }

        all_caps.insert(pkg.name.clone(), inferred);
    }

    // Check capabilities against allow lists (all workspace members)
    {
        let mut allows: std::collections::HashMap<String, Vec<String>> = std::collections::HashMap::new();
        for m in &manifests {
            for d in &m.deps {
                allows.entry(d.name.clone()).or_default().extend(d.allow.clone());
            }
        }
        for (pkg_name, caps) in &all_caps {
            if caps.is_empty() { continue; }
            let allowed = allows.get(pkg_name).cloned().unwrap_or_default();
            let violations = rask_resolve::capabilities::check_capabilities(caps, &allowed);
            for cap in &violations {
                eprintln!(
                    "  {} '{}' uses {} — add allow: [\"{}\"] to build.rk",
                    "Warning:".yellow(), pkg_name,
                    rask_resolve::capabilities::capability_description(cap),
                    cap,
                );
                cap_warnings += 1;
            }
        }
    }

    // Generate lock file (WS2: single lock at workspace root)
    let lock_path = root.join("rask.lock");
    let lockfile = if is_workspace {
        rask_resolve::LockFile::generate_workspace_with_capabilities(
            &registry, &root_ids, &root, &all_caps,
        )
    } else {
        rask_resolve::LockFile::generate_with_capabilities(
            &registry, root_id, &root, &all_caps,
        )
    };

    // Check if lock file changed
    let changed = if lock_path.exists() {
        let content = std::fs::read_to_string(&lock_path).unwrap_or_default();
        // Quick check: generate and compare
        let temp_lock = lockfile.write(&lock_path);
        if let Ok(()) = temp_lock {
            let new = std::fs::read_to_string(&lock_path).unwrap_or_default();
            let did_change = new != content;
            if !did_change {
                // Restore original
                let _ = std::fs::write(&lock_path, &content);
            }
            did_change
        } else {
            true
        }
    } else {
        if let Err(e) = lockfile.write(&lock_path) {
            eprintln!("{}: writing rask.lock: {}", output::error_label(), e);
            process::exit(1);
        }
        true
    };

    let fetched_msg = if registry_fetched > 0 {
        format!("{} downloaded, ", registry_fetched)
    } else {
        String::new()
    };

    println!(
        "  {} {} package{} ({}{})",
        "Fetched".green().bold(),
        external_count,
        if external_count == 1 { "" } else { "s" },
        fetched_msg,
        if changed { "lock file updated" } else { "up to date" },
    );

    if cap_warnings > 0 {
        println!("  {} {} capability warning{}",
            "Note:".yellow(),
            cap_warnings,
            if cap_warnings == 1 { "" } else { "s" },
        );
    }
}

/// Resolve the newest non-yanked version from a registry index that satisfies constraints.
fn resolve_from_index(
    index: &rask_resolve::registry::PackageIndex,
    constraints: &[rask_resolve::semver::Constraint],
    pkg_name: &str,
) -> Result<rask_resolve::semver::Version, String> {
    let available: Vec<rask_resolve::semver::Version> = index.versions.iter()
        .filter(|(_, meta)| !meta.yanked)
        .filter_map(|(v, _)| rask_resolve::semver::Version::parse(v).ok())
        .collect();

    if available.is_empty() {
        return Err(format!("no versions available for '{}'", pkg_name));
    }

    rask_resolve::semver::resolve_version(constraints, &available)
}
