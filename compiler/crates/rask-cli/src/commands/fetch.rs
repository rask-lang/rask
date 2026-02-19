// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dependency fetching — struct.packages/LK2.
//!
//! `rask fetch` resolves dependencies, validates version constraints,
//! and updates the lock file. For path dependencies, this validates
//! the dependency graph. Registry download will be added later.

use colored::Colorize;
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

    // Discover packages
    let mut registry = PackageRegistry::new();
    let root_id = match registry.discover(&root) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    };

    let root_pkg = registry.get(root_id);
    let manifest = root_pkg.and_then(|p| p.manifest.as_ref());

    // Validate version constraints on declared deps
    let mut constraint_errors = 0;
    if let Some(manifest) = manifest {
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

    // Check capabilities against allow lists
    if let Some(manifest) = manifest {
        let allows: std::collections::HashMap<String, Vec<String>> = manifest.deps.iter()
            .map(|d| (d.name.clone(), d.allow.clone()))
            .collect();

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

    // Generate lock file
    let lock_path = root.join("rask.lock");
    let lockfile = rask_resolve::LockFile::generate_with_capabilities(
        &registry, root_id, &root, &all_caps,
    );

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

    println!(
        "  {} {} package{} ({})",
        "Fetched".green().bold(),
        external_count,
        if external_count == 1 { "" } else { "s" },
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
