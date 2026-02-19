// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Project initialization — `rask init [name]`.

use colored::Colorize;
use std::fs;
use std::path::Path;
use std::process;

use crate::output;

/// Create a new Rask project in the given directory.
///
/// If `name` is None, uses the current directory (initializes in-place).
/// If `name` is Some, creates a new subdirectory.
pub fn cmd_init(name: Option<&str>) {
    let (project_dir, project_name) = match name {
        Some(n) => {
            let dir = Path::new(n);
            if dir.exists() {
                eprintln!("{}: directory '{}' already exists", output::error_label(), n);
                process::exit(1);
            }
            (dir.to_path_buf(), n.to_string())
        }
        None => {
            let dir = std::env::current_dir().unwrap_or_else(|e| {
                eprintln!("{}: {}", output::error_label(), e);
                process::exit(1);
            });
            let name = dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("my-project")
                .to_string();
            (dir, name)
        }
    };

    // Create project directory if needed
    if !project_dir.exists() {
        if let Err(e) = fs::create_dir_all(&project_dir) {
            eprintln!("{}: creating directory: {}", output::error_label(), e);
            process::exit(1);
        }
    }

    // Don't overwrite existing build.rk
    let build_rk = project_dir.join("build.rk");
    if build_rk.exists() {
        eprintln!("{}: {} already exists in {}",
            output::error_label(), "build.rk", project_dir.display());
        process::exit(1);
    }

    // Write build.rk
    let build_rk_content = format!(
        "package \"{}\" \"0.1.0\" {{\n    description: \"A Rask project\"\n    license: \"MIT\"\n}}\n",
        project_name
    );
    write_file(&build_rk, &build_rk_content);
    println!("  {} {}", "Created".green(), "build.rk");

    // Write main.rk if it doesn't exist
    let main_rk = project_dir.join("main.rk");
    if !main_rk.exists() {
        let main_content = format!(
            "// {}\n\nfunc main() {{\n    println(\"Hello from {}!\")\n}}\n",
            project_name, project_name,
        );
        write_file(&main_rk, &main_content);
        println!("  {} {}", "Created".green(), "main.rk");
    } else {
        println!("  {} {} (already exists)", "Skipped".dimmed(), "main.rk");
    }

    // Write .gitignore if not present
    let gitignore = project_dir.join(".gitignore");
    if !gitignore.exists() {
        let gitignore_content = "/build/\n.rk-gen/\n";
        write_file(&gitignore, gitignore_content);
        println!("  {} {}", "Created".green(), ".gitignore");
    }

    // Initialize git if not in a repo
    if !project_dir.join(".git").exists() {
        // Check if we're inside an existing repo
        let in_repo = find_parent_git(&project_dir);
        if !in_repo {
            let status = std::process::Command::new("git")
                .arg("init")
                .arg("-q")
                .current_dir(&project_dir)
                .status();
            if let Ok(s) = status {
                if s.success() {
                    println!("  {} git repository", "Initialized".green());
                }
            }
        }
    }

    println!();
    if name.is_some() {
        println!("  {} {}", "Project".green().bold(), project_dir.display());
        println!();
        println!("  Run with:");
        println!("    {} {}", "cd".dimmed(), project_name);
        println!("    {} {} {}", output::command("rask"), output::command("run"), "main.rk");
    } else {
        println!("  {} in {}", "Initialized".green().bold(), project_dir.display());
        println!();
        println!("  Run with:");
        println!("    {} {} {}", output::command("rask"), output::command("run"), "main.rk");
    }
}

fn write_file(path: &Path, content: &str) {
    if let Err(e) = fs::write(path, content) {
        eprintln!("{}: writing {}: {}", output::error_label(), path.display(), e);
        process::exit(1);
    }
}

/// Check if any parent directory has a .git directory.
fn find_parent_git(dir: &Path) -> bool {
    let mut current = dir.to_path_buf();
    loop {
        if current.join(".git").exists() {
            return true;
        }
        match current.parent() {
            Some(parent) if parent != current => {
                current = parent.to_path_buf();
            }
            _ => return false,
        }
    }
}
