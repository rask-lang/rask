// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Publishing — struct.build/PB1-PB7.
//!
//! `rask publish` validates metadata, builds a reproducible tarball,
//! and uploads it to the registry.

use colored::Colorize;
use std::path::{Path, PathBuf};
use std::process;

use crate::output;

/// Publish a package to the registry.
pub fn cmd_publish(path: &str, dry_run: bool, verbose: bool) {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));

    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    let build_rk = root.join("build.rk");
    if !build_rk.exists() {
        eprintln!("{}: no build.rk found — cannot publish without a package manifest", output::error_label());
        process::exit(1);
    }

    // Discover package
    let mut registry = rask_resolve::PackageRegistry::new();
    let root_id = match registry.discover(&root) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    };

    let manifest = match registry.get(root_id).and_then(|p| p.manifest.clone()) {
        Some(m) => m,
        None => {
            eprintln!("{}: build.rk has no package block", output::error_label());
            process::exit(1);
        }
    };

    let mut errors = 0;

    // PB2: Required metadata — description and license
    if manifest.meta("description").is_none() {
        eprintln!("{}: missing `description` metadata in build.rk (PB2)", output::error_label());
        errors += 1;
    }
    if manifest.meta("license").is_none() {
        eprintln!("{}: missing `license` metadata in build.rk (PB2)", output::error_label());
        errors += 1;
    }

    // PB5: No path dependencies
    for dep in &manifest.deps {
        if dep.path.is_some() {
            eprintln!("{}: dep \"{}\" uses a path — packages with path deps cannot be published (PB5)",
                output::error_label(), dep.name);
            errors += 1;
        }
    }

    if errors > 0 {
        eprintln!("\n{}", output::banner_fail("Publish", errors));
        process::exit(1);
    }

    // PB1: Pre-checks — run build (type-check + validation)
    if verbose {
        println!("  {} pre-checks...", "Running".dimmed());
    }
    let rask_exe = std::env::current_exe().unwrap_or_else(|_| "rask".into());
    let check_status = std::process::Command::new(&rask_exe)
        .args(["build", &root.to_string_lossy()])
        .status();

    match check_status {
        Ok(s) if s.success() => {
            if verbose {
                println!("  {} pre-checks passed", "OK".green());
            }
        }
        Ok(s) => {
            eprintln!("{}: pre-checks failed (exit {})", output::error_label(),
                s.code().unwrap_or(-1));
            process::exit(1);
        }
        Err(e) => {
            eprintln!("{}: failed to run pre-checks: {}", output::error_label(), e);
            process::exit(1);
        }
    }

    // PB6: Build reproducible tarball
    let tmp_dir = std::env::temp_dir();
    let tarball_path = tmp_dir.join(format!("{}-{}.tar.gz", manifest.name, manifest.version));

    let tarball_info = match rask_resolve::tarball::create_reproducible_tarball(&root, &tarball_path) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("{}: failed to create tarball: {}", output::error_label(), e);
            process::exit(1);
        }
    };

    // PB7: Size limit (10 MB)
    if tarball_info.size > rask_resolve::tarball::MAX_PUBLISH_SIZE {
        eprintln!("{}: package too large: {} bytes (max {} bytes)",
            output::error_label(),
            tarball_info.size,
            rask_resolve::tarball::MAX_PUBLISH_SIZE);
        eprintln!();
        eprintln!("File breakdown:");
        for (file_path, size) in &tarball_info.files {
            eprintln!("  {:>8} bytes  {}", size, file_path);
        }
        let _ = std::fs::remove_file(&tarball_path);
        process::exit(1);
    }

    // PB3: Dry run — print summary and exit
    if dry_run {
        println!("  {} (dry run)", "Publish".yellow().bold());
        println!("  Package: {} {}", manifest.name, manifest.version);
        println!("  Files:   {}", tarball_info.file_count);
        println!("  Size:    {} bytes", tarball_info.size);
        println!("  Checksum: {}", tarball_info.checksum);
        if verbose {
            println!();
            for (file_path, size) in &tarball_info.files {
                println!("  {:>8} bytes  {}", size, file_path);
            }
        }
        let _ = std::fs::remove_file(&tarball_path);
        return;
    }

    // PB4: Authentication
    let token = match load_auth_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            let _ = std::fs::remove_file(&tarball_path);
            process::exit(1);
        }
    };

    // Upload to registry
    let reg_config = rask_resolve::registry::RegistryConfig::from_env();
    if verbose {
        println!("  {} to {}...", "Publishing".cyan(), reg_config.url);
    }

    match reg_config.publish(&manifest.name, &manifest.version, &tarball_path, &token) {
        Ok(()) => {
            println!("  {} {} {}",
                "Published".green().bold(), manifest.name, manifest.version);
        }
        Err(e) => {
            eprintln!("{}: upload failed: {}", output::error_label(), e);
            let _ = std::fs::remove_file(&tarball_path);
            process::exit(1);
        }
    }

    let _ = std::fs::remove_file(&tarball_path);
}

/// Load auth token from RASK_REGISTRY_TOKEN env or ~/.rask/credentials (PB4).
fn load_auth_token() -> Result<String, String> {
    if let Ok(token) = std::env::var("RASK_REGISTRY_TOKEN") {
        if !token.is_empty() {
            return Ok(token);
        }
    }

    let credentials_path = home_dir()
        .ok_or_else(|| "cannot determine home directory".to_string())?
        .join(".rask")
        .join("credentials");

    let content = std::fs::read_to_string(&credentials_path)
        .map_err(|_| format!(
            "no auth token: set RASK_REGISTRY_TOKEN or create {}",
            credentials_path.display()
        ))?;

    // Parse simple format: token = "..."
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim();
            let value = value.trim().trim_matches('"');
            if key == "token" && !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }

    Err(format!(
        "no 'token' found in {}",
        credentials_path.display()
    ))
}

fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Yank a published version — hides from new resolution.
///
/// Already-locked versions in existing projects are unaffected.
pub fn cmd_yank(pkg_name: &str, version: &str) {
    let token = match load_auth_token() {
        Ok(t) => t,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    };

    let reg_config = rask_resolve::registry::RegistryConfig::from_env();

    match reg_config.yank(pkg_name, version, &token) {
        Ok(()) => {
            println!("  {} {} {}", "Yanked".green().bold(), pkg_name, version);
            println!("  Existing lock files are unaffected.");
        }
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }
}
