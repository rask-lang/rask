// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Dependency management commands — struct.build/AD1-AD4, RM1-RM2.

use colored::Colorize;
use std::fs;
use std::path::Path;
use std::process;

use crate::output;

const MANIFEST: &str = "build.rk";

/// Add a dependency to build.rk (AD1-AD4).
pub fn cmd_add(
    name: &str,
    version: Option<&str>,
    dev: bool,
    feature: Option<&str>,
    local_path: Option<&str>,
) {
    let manifest_path = Path::new(MANIFEST);

    // Build the dep line
    let dep_line = if let Some(p) = local_path {
        format!("    dep \"{}\" {{ path: \"{}\" }}", name, p)
    } else {
        let ver = version.unwrap_or("\"*\"");
        // Normalize: if user passed bare version like 1.0, wrap in quotes
        if ver.starts_with('"') {
            format!("    dep \"{}\" {}", name, ver)
        } else {
            format!("    dep \"{}\" \"{}\"", name, ver)
        }
    };

    if !manifest_path.exists() {
        // Create a new build.rk with the dependency
        let dir_name = std::env::current_dir()
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().to_string()))
            .unwrap_or_else(|| "my-package".to_string());

        let scope_open = if dev {
            format!("\n    scope \"dev\" {{\n{}\n    }}", dep_line.trim())
        } else if let Some(feat) = feature {
            format!("\n    feature \"{}\" {{\n{}\n    }}", feat, dep_line.trim())
        } else {
            format!("\n{}", dep_line)
        };

        let content = format!(
            "package \"{}\" \"0.1.0\" {{{}\n}}\n",
            dir_name, scope_open
        );

        if let Err(e) = fs::write(manifest_path, &content) {
            eprintln!("{}: failed to create {}: {}", output::error_label(), MANIFEST, e);
            process::exit(1);
        }
        println!("  {} {}", "Created".green(), MANIFEST);
        println!("  {} {}", "Added".green(), dep_line.trim());
        return;
    }

    // Read existing manifest
    let content = match fs::read_to_string(manifest_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: failed to read {}: {}", output::error_label(), MANIFEST, e);
            process::exit(1);
        }
    };

    // Check for duplicates (AD3)
    let check_name = format!("dep \"{}\"", name);
    if content.contains(&check_name) {
        eprintln!("{}: \"{}\" already in {}. Use {} to change version.",
            output::error_label(), name, MANIFEST,
            "rask update".green()
        );
        process::exit(1);
    }

    // Find insertion point (AD2: preserve formatting)
    let new_content = if dev {
        insert_in_scope(&content, "dev", &dep_line)
    } else if let Some(feat) = feature {
        insert_in_feature(&content, feat, &dep_line)
    } else {
        insert_dep(&content, &dep_line)
    };

    if let Err(e) = fs::write(manifest_path, &new_content) {
        eprintln!("{}: failed to write {}: {}", output::error_label(), MANIFEST, e);
        process::exit(1);
    }

    let location = if dev {
        " to scope \"dev\"".to_string()
    } else if let Some(feat) = feature {
        format!(" to feature \"{}\"", feat)
    } else {
        String::new()
    };

    println!("  {} {}{}", "Added".green(), dep_line.trim(), location);
}

/// Remove a dependency from build.rk (RM1-RM2).
pub fn cmd_remove(name: &str) {
    let manifest_path = Path::new(MANIFEST);

    if !manifest_path.exists() {
        eprintln!("{}: no {} found", output::error_label(), MANIFEST);
        process::exit(1);
    }

    let content = match fs::read_to_string(manifest_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: failed to read {}: {}", output::error_label(), MANIFEST, e);
            process::exit(1);
        }
    };

    let pattern = format!("dep \"{}\"", name);
    if !content.contains(&pattern) {
        eprintln!("{}: \"{}\" not found in {}", output::error_label(), name, MANIFEST);
        process::exit(1);
    }

    // Remove lines containing the dep declaration
    let new_lines: Vec<&str> = content
        .lines()
        .filter(|line| !line.contains(&pattern))
        .collect();
    let new_content = new_lines.join("\n") + "\n";

    if let Err(e) = fs::write(manifest_path, &new_content) {
        eprintln!("{}: failed to write {}: {}", output::error_label(), MANIFEST, e);
        process::exit(1);
    }

    println!("  {} dep \"{}\"", "Removed".green(), name);
}

/// Insert a dep line into the package block, after existing deps.
fn insert_dep(content: &str, dep_line: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let mut result = Vec::new();
    let mut inserted = false;

    // Find the last dep line or the opening brace of the package block
    let mut last_dep_idx = None;
    let mut package_open_idx = None;

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("package ") && trimmed.ends_with('{') {
            package_open_idx = Some(i);
        }
        if trimmed.starts_with("dep ") {
            last_dep_idx = Some(i);
        }
    }

    let insert_after = last_dep_idx.or(package_open_idx);

    for (i, line) in lines.iter().enumerate() {
        result.push(*line);
        if !inserted {
            if let Some(idx) = insert_after {
                if i == idx {
                    result.push(dep_line);
                    inserted = true;
                }
            }
        }
    }

    if !inserted {
        // Fallback: insert before closing brace
        let len = result.len();
        if len > 0 {
            result.insert(len - 1, dep_line);
        }
    }

    result.join("\n") + "\n"
}

/// Insert a dep into a scope block, creating it if needed.
fn insert_in_scope(content: &str, scope_name: &str, dep_line: &str) -> String {
    let scope_pattern = format!("scope \"{}\"", scope_name);
    if content.contains(&scope_pattern) {
        // Insert into existing scope block
        let lines: Vec<&str> = content.lines().collect();
        let mut result = Vec::new();
        let mut in_scope = false;

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.contains(&scope_pattern) {
                in_scope = true;
            }
            if in_scope && trimmed == "}" {
                // Insert before closing brace of scope
                result.push(dep_line.as_ref());
                in_scope = false;
            }
            result.push(*line);
        }
        result.join("\n") + "\n"
    } else {
        // Create new scope block before closing brace of package
        let scope_block = format!(
            "\n    scope \"{}\" {{\n    {}\n    }}",
            scope_name,
            dep_line.trim()
        );
        insert_before_closing_brace(content, &scope_block)
    }
}

/// Insert a dep into a feature block, creating it if needed.
fn insert_in_feature(content: &str, feature_name: &str, dep_line: &str) -> String {
    let feat_pattern = format!("feature \"{}\"", feature_name);
    if content.contains(&feat_pattern) {
        let lines: Vec<&str> = content.lines().collect();
        let mut result = Vec::new();
        let mut in_feature = false;

        for line in &lines {
            let trimmed = line.trim();
            if trimmed.contains(&feat_pattern) {
                in_feature = true;
            }
            if in_feature && trimmed == "}" {
                result.push(dep_line.as_ref());
                in_feature = false;
            }
            result.push(*line);
        }
        result.join("\n") + "\n"
    } else {
        let feat_block = format!(
            "\n    feature \"{}\" {{\n    {}\n    }}",
            feature_name,
            dep_line.trim()
        );
        insert_before_closing_brace(content, &feat_block)
    }
}

/// Insert text before the last closing brace in the file.
fn insert_before_closing_brace(content: &str, text: &str) -> String {
    if let Some(pos) = content.rfind('}') {
        let mut result = content[..pos].to_string();
        result.push_str(text);
        result.push('\n');
        result.push_str(&content[pos..]);
        result
    } else {
        format!("{}\n{}\n", content, text)
    }
}
