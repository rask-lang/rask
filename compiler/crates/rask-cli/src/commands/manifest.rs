// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! `rask manifest` and `rask migrate` — struct.manifest.

use colored::Colorize;
use std::fs;
use std::path::Path;
use std::process;

use rask_resolve::package::MANIFEST_FILE;

use crate::output;

fn fail(msg: &str) -> ! {
    eprintln!("{}: {}", output::error_label(), msg);
    process::exit(1);
}

fn manifest_in(dir: &str) -> std::path::PathBuf {
    let path = Path::new(dir).join(MANIFEST_FILE);
    if !path.is_file() {
        fail(&format!("no {} in {}", MANIFEST_FILE, dir));
    }
    path
}

/// `rask manifest check [dir]`: load it and say what's wrong, every problem.
pub fn cmd_check(dir: &str) {
    let path = manifest_in(dir);
    match rask_manifest::load(&path) {
        Ok(pkg) => println!("  {} {} {} ({})", "OK".green(), pkg.name, pkg.version, path.display()),
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), path.display());
            for line in e.lines() {
                eprintln!("  {}", line);
            }
            process::exit(1);
        }
    }
}

/// `rask manifest json-schema`: the current shape, for editors.
pub fn cmd_json_schema() {
    match rask_manifest::json_schema() {
        Ok(s) => println!("{}", s),
        Err(e) => fail(&e),
    }
}

/// `rask migrate --to N [dir]`: rewrite rask.toml for schema version N.
pub fn cmd_migrate(dir: &str, to: u32) {
    let path = manifest_in(dir);
    let text = fs::read_to_string(&path).unwrap_or_else(|e| fail(&format!("reading {}: {}", path.display(), e)));
    let out = rask_manifest::migrate(&text, to).unwrap_or_else(|e| fail(&e));
    // Comments and layout don't survive a migration yet; say so before the
    // file changes rather than after.
    if text.lines().any(|l| l.trim_start().starts_with('#')) {
        eprintln!(
            "  {} the rewritten {} drops its comments (struct.manifest, migration isn't format-preserving yet)",
            "Note:".yellow(),
            MANIFEST_FILE
        );
    }
    if let Err(e) = fs::write(&path, &out) {
        fail(&format!("writing {}: {}", path.display(), e));
    }
    println!("  {} {} to rask-package@{}", "Migrated".green(), path.display(), to);
}
