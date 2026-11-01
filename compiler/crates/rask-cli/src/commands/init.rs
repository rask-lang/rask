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
pub fn cmd_init(name: Option<&str>, agents: bool) {
    let (project_dir, project_name) = match name {
        Some(n) => {
            let dir = Path::new(n);
            if dir.exists() {
                eprintln!("{}: directory '{}' already exists", output::error_label(), n);
                process::exit(1);
            }
            // The package is named after the directory, not the path to it.
            let name = dir
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| n.to_string());
            (dir.to_path_buf(), name)
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

    // Don't overwrite an existing manifest
    let manifest = project_dir.join(rask_resolve::package::MANIFEST_FILE);
    if manifest.exists() {
        eprintln!("{}: {} already exists in {}",
            output::error_label(), rask_resolve::package::MANIFEST_FILE, project_dir.display());
        process::exit(1);
    }

    // The schema line names the version this toolchain ships, so a new
    // manifest starts current (struct.manifest/MF1).
    let schema = rask_manifest::schema_line().unwrap_or_else(|e| {
        eprintln!("{}: {}", output::error_label(), e);
        process::exit(1);
    });
    write_file(&manifest, &manifest_text(&project_name, &schema));
    println!("  {} {}", "Created".green(), rask_resolve::package::MANIFEST_FILE);

    // Write main.rk if it doesn't exist
    let main_rk = project_dir.join("main.rk");
    if !main_rk.exists() {
        let main_content = main_text(&project_name);
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

    // Only on request: plenty of projects want nothing to do with agents.
    if agents {
        match super::agents::write_agents_md(&project_dir) {
            Ok(true) => println!("  {} {}", "Created".green(), "AGENTS.md section"),
            Ok(false) => println!("  {} {} (already has a Rask section)", "Skipped".dimmed(), "AGENTS.md"),
            Err(e) => {
                eprintln!("{}: writing AGENTS.md: {}", output::error_label(), e);
                process::exit(1);
            }
        }
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
        println!("    {} {}", "cd".dimmed(), project_dir.display());
        println!("    {} {} {}", output::command("rask"), output::command("run"), "main.rk");
    } else {
        println!("  {} in {}", "Initialized".green().bold(), project_dir.display());
        println!();
        println!("  Run with:");
        println!("    {} {} {}", output::command("rask"), output::command("run"), "main.rk");
    }
}

/// The starter `main.rk`. The name lands inside a Rask string, where `{`
/// starts an interpolation, so it's escaped like any other text there.
fn main_text(name: &str) -> String {
    let mut quoted = String::new();
    for c in name.chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '{' => quoted.push_str("{{"),
            '}' => quoted.push_str("}}"),
            c => quoted.push(c),
        }
    }
    format!("func main() {{\n    println(\"Hello from {quoted}!\")\n}}\n")
}

fn manifest_text(project_name: &str, schema: &str) -> String {
    format!(
        "schema = \"{}\"\n\n[package]\nname = \"{}\"\nversion = \"0.1.0\"\ndescription = \"A Rask project\"\nlicense = \"MIT\"\n",
        schema,
        toml_escape(project_name),
    )
}

/// The inside of a TOML basic string.
fn toml_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn write_file(path: &Path, content: &str) {
    if let Err(e) = fs::write(path, content) {
        eprintln!("{}: writing {}: {}", output::error_label(), path.display(), e);
        process::exit(1);
    }
}

/// Check if any parent directory has a .git directory.
fn find_parent_git(dir: &Path) -> bool {
    // A relative path runs out of parents at the working directory.
    let mut current = std::path::absolute(dir).unwrap_or_else(|_| dir.to_path_buf());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_new_manifest_loads_and_names_the_current_schema() {
        let schema = rask_manifest::schema_line().unwrap();
        assert!(schema.starts_with("rask-package@"), "{schema}");
        let text = manifest_text("hello", &schema);
        assert!(text.starts_with(&format!("schema = \"{schema}\"\n")), "{text}");
        let pkg = rask_manifest::load_text(&text).unwrap();
        assert_eq!((pkg.name.as_str(), pkg.version.as_str()), ("hello", "0.1.0"));
        assert!(pkg.deps.is_empty());
    }

    #[test]
    fn a_directory_name_toml_would_misread_is_escaped() {
        let schema = rask_manifest::schema_line().unwrap();
        let pkg = rask_manifest::load_text(&manifest_text("my \"app\" \\ x", &schema)).unwrap();
        assert_eq!(pkg.name, "my \"app\" \\ x");
    }

    #[test]
    fn the_starter_program_prints_any_name_as_written() {
        let name = "we{ird} \"app\" \\ x";
        let lexed = rask_lexer::Lexer::new(&main_text(name)).tokenize();
        assert!(lexed.is_ok(), "{:?}", lexed.errors);
        let mut parsed = rask_parser::Parser::new(lexed.tokens).parse();
        assert!(parsed.is_ok(), "{:?}", parsed.errors);
        rask_desugar::desugar_with_stdlib(&mut parsed.decls, rask_stdlib::StubRegistry::desugar_declarations());
        let (mut interp, out) = rask_interp::Interpreter::with_captured_output();
        interp.run(&parsed.decls).unwrap();
        assert_eq!(out.lock().unwrap().trim_end(), format!("Hello from {name}!"));
    }
}
