// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Vendoring — struct.build/VD1-VD5.
//!
//! `rask vendor` copies all registry dependencies into a local `vendor/`
//! directory for offline builds. Checksums are preserved for integrity.

use colored::Colorize;
use std::path::{Path, PathBuf};
use std::process;

use crate::output;

/// Copy all registry dependencies to vendor/ for offline builds.
pub fn cmd_vendor(path: &str, verbose: bool) {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));

    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    // Load lock file (VD1 requires resolved deps)
    let lock_path = root.join("rask.lock");
    if !lock_path.exists() {
        eprintln!("{}: no rask.lock found — run `rask fetch` first", output::error_label());
        process::exit(1);
    }

    let lockfile = match rask_resolve::LockFile::load(&lock_path) {
        Ok(lf) => lf,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    };

    if lockfile.packages.is_empty() {
        println!("  {} (no dependencies to vendor)", "OK".green());
        return;
    }

    // Collect registry packages from lock file
    let registry_pkgs: Vec<_> = lockfile.packages.iter()
        .filter(|p| p.source.starts_with("registry+"))
        .collect();

    if registry_pkgs.is_empty() {
        println!("  {} (no registry dependencies to vendor)", "OK".green());
        return;
    }

    let cache = rask_resolve::cache::PackageCache::new();
    let vendor_dir = root.join("vendor");

    // Create vendor directory
    if let Err(e) = std::fs::create_dir_all(&vendor_dir) {
        eprintln!("{}: failed to create vendor directory: {}", output::error_label(), e);
        process::exit(1);
    }

    // Write vendor/.gitignore
    let gitignore = vendor_dir.join(".gitignore");
    if !gitignore.exists() {
        let _ = std::fs::write(&gitignore, "# Vendored dependencies — check in for offline builds\n!*\n");
    }

    let mut vendored = 0;
    let mut errors = 0;

    for pkg in &registry_pkgs {
        let pkg_vendor_dir = vendor_dir.join(format!("{}-{}", pkg.name, pkg.version));

        // Check if already vendored with correct checksum (VD2)
        let checksum_file = pkg_vendor_dir.join(".checksum");
        if pkg_vendor_dir.is_dir() && checksum_file.is_file() {
            if let Ok(stored) = std::fs::read_to_string(&checksum_file) {
                if stored.trim() == pkg.checksum {
                    if verbose {
                        println!("  {} \"{}\" {} (up to date)", "Vendor".dimmed(), pkg.name, pkg.version);
                    }
                    vendored += 1;
                    continue;
                }
            }
        }

        // Find in cache
        let cache_dir = cache.pkg_dir(&pkg.name, &pkg.version);
        if !cache_dir.is_dir() {
            eprintln!("{}: \"{}\" {} not in cache — run `rask fetch` first",
                output::error_label(), pkg.name, pkg.version);
            errors += 1;
            continue;
        }

        // Remove old vendor dir if it exists
        if pkg_vendor_dir.exists() {
            if let Err(e) = std::fs::remove_dir_all(&pkg_vendor_dir) {
                eprintln!("{}: failed to clean {}: {}", output::error_label(), pkg_vendor_dir.display(), e);
                errors += 1;
                continue;
            }
        }

        // Copy from cache to vendor (VD1)
        if let Err(e) = copy_dir_recursive(&cache_dir, &pkg_vendor_dir) {
            eprintln!("{}: failed to vendor \"{}\" {}: {}",
                output::error_label(), pkg.name, pkg.version, e);
            errors += 1;
            continue;
        }

        // Write checksum file (VD2)
        if let Err(e) = std::fs::write(&checksum_file, &pkg.checksum) {
            eprintln!("{}: failed to write checksum for \"{}\" {}: {}",
                output::error_label(), pkg.name, pkg.version, e);
            errors += 1;
            continue;
        }

        if verbose {
            println!("  {} \"{}\" {}", "Vendored".green(), pkg.name, pkg.version);
        }
        vendored += 1;
    }

    if errors > 0 {
        eprintln!("\n{}", output::banner_fail("Vendor", errors));
        process::exit(1);
    }

    println!(
        "  {} {} package{} to vendor/",
        "Vendored".green().bold(),
        vendored,
        if vendored == 1 { "" } else { "s" },
    );

    // Check if vendor_dir is configured in build.rk (VD3)
    let build_rk = root.join("build.rk");
    if build_rk.is_file() {
        if let Ok(content) = std::fs::read_to_string(&build_rk) {
            if !content.contains("vendor_dir") {
                println!("  {} add `vendor_dir: \"vendor\"` to build.rk to use vendored deps",
                    "Hint:".cyan());
            }
        }
    }
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}
