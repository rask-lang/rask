// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dependency auditing — struct.build/AU1-AU5.
//!
//! `rask audit` checks locked dependency versions against an advisory
//! database for known vulnerabilities.

use colored::Colorize;
use std::path::{Path, PathBuf};
use std::process;

use crate::output;

/// Check dependencies for known vulnerabilities.
pub fn cmd_audit(path: &str, ignore: Vec<String>, db_path: Option<&str>, verbose: bool) {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));

    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    // AU2: Lock file based — read exact versions from rask.lock
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
        println!("  {} no dependencies to audit", "OK".green());
        return;
    }

    // Load advisory database (AU1, AU5)
    let db = if let Some(local_path) = db_path {
        // AU5: Offline mode
        match rask_resolve::advisory::AdvisoryDb::load_file(local_path) {
            Ok(db) => db,
            Err(e) => {
                eprintln!("{}: {}", output::error_label(), e);
                process::exit(1);
            }
        }
    } else {
        // AU1: Fetch from advisory server
        let url = std::env::var("RASK_ADVISORY_URL")
            .unwrap_or_else(|_| rask_resolve::advisory::DEFAULT_ADVISORY_URL.to_string());

        if verbose {
            println!("  {} {}...", "Fetching advisories from".dimmed(), url);
        }

        match rask_resolve::advisory::AdvisoryDb::fetch(&url) {
            Ok(db) => db,
            Err(e) => {
                eprintln!("{}: {}", output::error_label(), e);
                eprintln!("{}: use --db <path> for offline auditing", "hint".cyan());
                process::exit(1);
            }
        }
    };

    if verbose {
        println!("  {} {} advisories (updated: {})",
            "Loaded".dimmed(), db.advisories.len(), db.updated);
    }

    // Check each locked package
    let mut findings: Vec<(&rask_resolve::lockfile::LockedPackage, &rask_resolve::advisory::Advisory)> = Vec::new();
    let mut ignored_count = 0;

    for pkg in &lockfile.packages {
        let advisories = db.lookup(&pkg.name, &pkg.version);
        for advisory in advisories {
            if ignore.contains(&advisory.id) {
                ignored_count += 1;
                if verbose {
                    println!("  {} {} (ignored)", "Skip".dimmed(), advisory.id);
                }
                continue;
            }
            findings.push((pkg, advisory));
        }
    }

    // Report results
    if findings.is_empty() {
        println!("  {} {} package{} audited, no vulnerabilities found",
            "OK".green(),
            lockfile.packages.len(),
            if lockfile.packages.len() == 1 { "" } else { "s" },
        );
        if ignored_count > 0 {
            println!("  {} {} {} ignored",
                "Note:".yellow(),
                ignored_count,
                if ignored_count == 1 { "advisory" } else { "advisories" },
            );
        }
        return;
    }

    // Print findings
    eprintln!();
    for (pkg, advisory) in &findings {
        let severity_colored = match advisory.severity.as_str() {
            "critical" => advisory.severity.red().bold().to_string(),
            "high" => advisory.severity.red().to_string(),
            "medium" => advisory.severity.yellow().to_string(),
            _ => advisory.severity.dimmed().to_string(),
        };

        eprintln!("  {} {} {} — {} [{}]",
            "VULN".red().bold(),
            pkg.name, pkg.version,
            advisory.title,
            severity_colored,
        );
        eprintln!("    ID: {}", advisory.id);
        eprintln!("    Affected: {}", advisory.affected);
        if let Some(ref url) = advisory.url {
            eprintln!("    Details: {}", url);
        }
        eprintln!();
    }

    eprintln!("{} {} found in {} package{}",
        findings.len(),
        if findings.len() == 1 { "vulnerability" } else { "vulnerabilities" },
        lockfile.packages.len(),
        if lockfile.packages.len() == 1 { "" } else { "s" },
    );

    if ignored_count > 0 {
        eprintln!("{} {} ignored",
            ignored_count,
            if ignored_count == 1 { "advisory" } else { "advisories" },
        );
    }

    // AU3: Non-zero exit code
    process::exit(1);
}
