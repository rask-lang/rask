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
                                    // Hidden parameter pass (comp.hidden-params/HP1)
                                    rask_hidden_params::desugar_hidden_params(&mut all_decls);

                                    // Monomorphize
                                    match rask_mono::monomorphize(&typed, &all_decls) {
                                        Ok(mono) => {
                                            // Lower to MIR
                                            let all_mono_decls: Vec<_> = mono.functions.iter().map(|f| f.body.clone()).collect();
                                            let mir_ctx = rask_mir::lower::MirContext {
                                                struct_layouts: &mono.struct_layouts,
                                                enum_layouts: &mono.enum_layouts,
                                                node_types: &typed.node_types,
                                            };

                                            let mut mir_functions = Vec::new();
                                            let mut mir_errors = 0;

                                            for mono_fn in &mono.functions {
                                                match rask_mir::lower::MirLowerer::lower_function(&mono_fn.body, &all_mono_decls, &mir_ctx) {
                                                    Ok(mir_fn) => mir_functions.push(mir_fn),
                                                    Err(e) => {
                                                        eprintln!("MIR lowering error in '{}': {:?}", mono_fn.name, e);
                                                        mir_errors += 1;
                                                    }
                                                }
                                            }

                                            total_errors += mir_errors;

                                            // Generate code if no MIR errors
                                            if mir_errors == 0 && !mir_functions.is_empty() {
                                                match rask_codegen::CodeGenerator::new() {
                                                    Ok(mut codegen) => {
                                                        // Declare runtime functions (print, exit, etc.)
                                                        if let Err(e) = codegen.declare_runtime_functions() {
                                                            eprintln!("codegen error: {}", e);
                                                            total_errors += 1;
                                                        }

                                                        // Declare all user functions
                                                        if total_errors == 0 {
                                                            if let Err(e) = codegen.declare_functions(&mono, &mir_functions) {
                                                                eprintln!("codegen error: {}", e);
                                                                total_errors += 1;
                                                            }
                                                        }

                                                        // Generate each function
                                                        if total_errors == 0 {
                                                            for mir_fn in &mir_functions {
                                                                if let Err(e) = codegen.gen_function(mir_fn) {
                                                                    eprintln!("codegen error in '{}': {}", mir_fn.name, e);
                                                                    total_errors += 1;
                                                                }
                                                            }
                                                        }

                                                        // Emit object file and link
                                                        if total_errors == 0 {
                                                            let obj_path = "output.o";
                                                            let bin_path = "output";
                                                            match codegen.emit_object(obj_path) {
                                                                Ok(_) => {
                                                                    println!("  {} {}", "Generated".green(), obj_path);
                                                                    // Link with C runtime to produce executable
                                                                    match link_executable(obj_path, bin_path) {
                                                                        Ok(_) => println!("  {} {}", "Linked".green(), bin_path),
                                                                        Err(e) => {
                                                                            eprintln!("link error: {}", e);
                                                                            total_errors += 1;
                                                                        }
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    eprintln!("failed to emit object file: {}", e);
                                                                    total_errors += 1;
                                                                }
                                                            }
                                                        }
                                                    }
                                                    Err(e) => {
                                                        eprintln!("failed to initialize codegen: {}", e);
                                                        total_errors += 1;
                                                    }
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

/// Find the runtime.c file, compile it, and link with the object file.
fn link_executable(obj_path: &str, bin_path: &str) -> Result<(), String> {
    // Find the runtime relative to the rask binary
    let runtime_path = find_runtime_c()?;

    // Compile runtime.c and link with the generated object file
    let status = process::Command::new("cc")
        .args([&runtime_path, obj_path, "-o", bin_path])
        .status()
        .map_err(|e| format!("failed to run cc: {}", e))?;

    if !status.success() {
        return Err(format!("linker exited with status {}", status));
    }

    // Clean up the intermediate .o file
    let _ = std::fs::remove_file(obj_path);

    Ok(())
}

/// Locate the C runtime file. Searches:
/// 1. Next to the rask binary: ../runtime/runtime.c
/// 2. RASK_RUNTIME_DIR environment variable
/// 3. Common development paths
fn find_runtime_c() -> Result<String, String> {
    // Check RASK_RUNTIME_DIR
    if let Ok(dir) = std::env::var("RASK_RUNTIME_DIR") {
        let p = Path::new(&dir).join("runtime.c");
        if p.exists() {
            return Ok(p.to_string_lossy().to_string());
        }
    }

    // Check relative to the current executable
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            // Development layout: target/release/rask → compiler/runtime/runtime.c
            // Walk up from the binary to find the compiler/runtime directory
            let mut dir = exe_dir.to_path_buf();
            for _ in 0..5 {
                let candidate = dir.join("compiler").join("runtime").join("runtime.c");
                if candidate.exists() {
                    return Ok(candidate.to_string_lossy().to_string());
                }
                let candidate = dir.join("runtime").join("runtime.c");
                if candidate.exists() {
                    return Ok(candidate.to_string_lossy().to_string());
                }
                if !dir.pop() {
                    break;
                }
            }
        }
    }

    Err("Could not find runtime.c — set RASK_RUNTIME_DIR to the directory containing it".to_string())
}
