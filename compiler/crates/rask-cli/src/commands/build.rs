// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Build command.

use colored::Colorize;
use std::path::Path;
use std::process;

use crate::output;

pub fn cmd_build(path: &str) {
    use rask_resolve::PackageRegistry;

    let root = Path::new(path);
    if !root.exists() {
        eprintln!("{}: directory not found: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        eprintln!("{}: {} {} {} for single files", "hint".cyan(), output::command("rask"), output::command("typecheck"), output::arg("<file>"));
        process::exit(1);
    }

    println!("{} Discovering packages in {} {}\n", "===".dimmed(), output::file_path(path), "===".dimmed());

    let mut registry = PackageRegistry::new();
    match registry.discover(root) {
        Ok(_root_id) => {
            println!("Discovered {} package(s):\n", registry.len());

            for pkg in registry.packages() {
                let file_count = pkg.files.len();
                let decl_count: usize = pkg.files.iter().map(|f| f.decls.len()).sum();
                println!(
                    "  {} ({} file{}, {} declaration{})",
                    pkg.path_string(),
                    file_count,
                    if file_count == 1 { "" } else { "s" },
                    decl_count,
                    if decl_count == 1 { "" } else { "s" }
                );

                for file in &pkg.files {
                    println!("    - {}", file.path.display());
                }
            }
            println!();

            let mut total_errors = 0;
            for pkg in registry.packages() {
                println!("{} Compiling package: {} {}", "===".dimmed(), pkg.path_string().green(), "===".dimmed());

                let mut all_decls: Vec<_> = pkg.all_decls().cloned().collect();
                rask_desugar::desugar(&mut all_decls);

                match rask_resolve::resolve_package(&all_decls, &registry, pkg.id) {
                    Ok(resolved) => {
                        match rask_types::typecheck(resolved, &all_decls) {
                            Ok(typed) => {
                                let ownership_result = rask_ownership::check_ownership(&typed, &all_decls);
                                if !ownership_result.is_ok() {
                                    for error in &ownership_result.errors {
                                        eprintln!("error: {}", error.kind);
                                    }
                                    total_errors += ownership_result.errors.len();
                                } else {
                                    // Monomorphize
                                    match rask_mono::monomorphize(&typed, &all_decls) {
                                        Ok(mono) => {
                                            // Lower to MIR
                                            for mono_fn in &mono.functions {
                                                if let Err(e) = rask_mir::lower::MirLowerer::lower_function(&mono_fn.body) {
                                                    eprintln!("MIR lowering error in '{}': {:?}", mono_fn.name, e);
                                                    total_errors += 1;
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            eprintln!("monomorphization error: {:?}", e);
                                            total_errors += 1;
                                        }
                                    }
                                }
                            }
                            Err(errors) => {
                                for error in &errors {
                                    eprintln!("type error: {}", error);
                                }
                                total_errors += errors.len();
                            }
                        }
                    }
                    Err(errors) => {
                        for error in &errors {
                            eprintln!("resolve error: {}", error.kind);
                        }
                        total_errors += errors.len();
                    }
                }
            }

            println!();
            if total_errors == 0 {
                println!("{}", output::banner_ok(&format!("Build: {} package(s) compiled", registry.len())));
            } else {
                eprintln!("{}", output::banner_fail("Build", total_errors));
                process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }
}
