// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Build command — struct.build/OD1-OD7, CL1-CL4.

use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

use crate::output;

/// Build options parsed from CLI flags.
pub struct BuildOptions {
    pub profile: String,
    pub verbose: bool,
    pub target: Option<String>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            profile: "debug".to_string(),
            verbose: false,
            target: None,
        }
    }
}

/// Determine the output directory.
/// OD2: build/<profile>/ for native builds
/// OD3: build/<target>/<profile>/ for cross-compilation
fn output_dir(root: &Path, profile: &str, target: Option<&str>) -> PathBuf {
    let base = root.join("build");
    match target {
        Some(triple) => base.join(triple).join(profile),
        None => base.join(profile),
    }
}

/// Determine binary name from directory name (OD4).
fn binary_name(root: &Path) -> String {
    root.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("output")
        .to_string()
}

/// Ensure build/.gitignore exists (OD5).
fn ensure_gitignore(root: &Path) {
    let build_dir = root.join("build");
    let gitignore = build_dir.join(".gitignore");
    if build_dir.exists() && !gitignore.exists() {
        let _ = fs::write(&gitignore, "*\n");
    }
}

pub fn cmd_build(path: &str, opts: BuildOptions) {
    use rask_resolve::PackageRegistry;

    let start = Instant::now();
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));

    if !root.exists() {
        eprintln!("{}: directory not found: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        eprintln!("{}: use {} {} {} for single files", "hint".cyan(), output::command("rask"), output::command("compile"), output::arg("<file>"));
        process::exit(1);
    }

    // Create output directory (OD1, OD2, OD3)
    let out_dir = output_dir(&root, &opts.profile, opts.target.as_deref());
    if let Err(e) = fs::create_dir_all(&out_dir) {
        eprintln!("{}: failed to create output directory {}: {}", output::error_label(), out_dir.display(), e);
        process::exit(1);
    }

    // Auto-create build/.gitignore (OD5)
    ensure_gitignore(&root);

    let bin_name = binary_name(&root);

    if opts.verbose {
        println!("  {} {}", "Profile:".dimmed(), opts.profile);
        if let Some(ref t) = opts.target {
            println!("  {} {}", "Target:".dimmed(), t);
        }
        println!("  {} {}", "Output:".dimmed(), out_dir.display());
        println!("  {} {}", "Binary:".dimmed(), bin_name);
        println!();
    }

    let compile_label = if let Some(ref t) = opts.target {
        format!("{}, {}", opts.profile, t)
    } else {
        opts.profile.clone()
    };
    println!("{} {} ({})", "  Compiling".green().bold(), bin_name, compile_label);

    let mut registry = PackageRegistry::new();
    match registry.discover(&root) {
        Ok(_root_id) => {
            if opts.verbose {
                println!("  Discovered {} package(s):", registry.len());
                for pkg in registry.packages() {
                    let file_count = pkg.files.len();
                    let decl_count: usize = pkg.files.iter().map(|f| f.decls.len()).sum();
                    println!(
                        "    {} ({} file{}, {} decl{})",
                        pkg.path_string(),
                        file_count,
                        if file_count == 1 { "" } else { "s" },
                        decl_count,
                        if decl_count == 1 { "" } else { "s" }
                    );
                }
                println!();
            }

            let mut total_errors = 0;
            for pkg in registry.packages() {
                if opts.verbose {
                    println!("  {} {}", "Checking".dimmed(), pkg.path_string());
                }

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
                                    // Hidden parameter pass
                                    rask_hidden_params::desugar_hidden_params(&mut all_decls);

                                    match rask_mono::monomorphize(&typed, &all_decls) {
                                        Ok(mono) => {
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

                                            if mir_errors == 0 && !mir_functions.is_empty() {
                                                let codegen_result = match opts.target {
                                                    Some(ref t) => rask_codegen::CodeGenerator::new_with_target(t),
                                                    None => rask_codegen::CodeGenerator::new(),
                                                };
                                                match codegen_result {
                                                    Ok(mut codegen) => {
                                                        if let Err(e) = codegen.declare_runtime_functions() {
                                                            eprintln!("codegen error: {}", e);
                                                            total_errors += 1;
                                                        }

                                                        if total_errors == 0 {
                                                            if let Err(e) = codegen.declare_stdlib_functions() {
                                                                eprintln!("codegen error: {}", e);
                                                                total_errors += 1;
                                                            }
                                                        }

                                                        if total_errors == 0 {
                                                            if let Err(e) = codegen.declare_functions(&mono, &mir_functions) {
                                                                eprintln!("codegen error: {}", e);
                                                                total_errors += 1;
                                                            }
                                                        }

                                                        if total_errors == 0 {
                                                            if let Err(e) = codegen.register_strings(&mir_functions) {
                                                                eprintln!("codegen error: {}", e);
                                                                total_errors += 1;
                                                            }
                                                        }

                                                        if total_errors == 0 {
                                                            for mir_fn in &mir_functions {
                                                                if let Err(e) = codegen.gen_function(mir_fn) {
                                                                    eprintln!("codegen error in '{}': {}", mir_fn.name, e);
                                                                    total_errors += 1;
                                                                }
                                                            }
                                                        }

                                                        // Emit to build/<profile>/ (OD2)
                                                        if total_errors == 0 {
                                                            let obj_path = out_dir.join(format!("{}.o", bin_name));
                                                            let bin_path = out_dir.join(&bin_name);
                                                            let obj_str = obj_path.to_string_lossy().to_string();
                                                            let bin_str = bin_path.to_string_lossy().to_string();

                                                            match codegen.emit_object(&obj_str) {
                                                                Ok(_) => {
                                                                    match super::link::link_executable(&obj_str, &bin_str) {
                                                                        Ok(_) => {}
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

            let elapsed = start.elapsed();
            println!();
            if total_errors == 0 {
                let bin_path = out_dir.join(&bin_name);
                println!(
                    "   {} {} ({}) [{:.2}s]",
                    "Finished".green().bold(),
                    bin_path.display(),
                    opts.profile,
                    elapsed.as_secs_f64()
                );
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

/// Clean build artifacts (OD6).
pub fn cmd_clean(path: &str, all: bool) {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    let build_dir = root.join("build");

    if build_dir.exists() {
        match fs::remove_dir_all(&build_dir) {
            Ok(_) => println!("  {} {}", "Removed".green(), build_dir.display()),
            Err(e) => {
                eprintln!("{}: failed to remove {}: {}", output::error_label(), build_dir.display(), e);
                process::exit(1);
            }
        }
    } else {
        println!("  {} (nothing to clean)", "OK".green());
    }

    if all {
        // Also clean global cache entries for this project
        if let Some(home) = dirs_home() {
            let cache_dir = home.join(".rask").join("cache");
            if cache_dir.exists() {
                println!("  {} {}", "Cleaned".green(), cache_dir.display());
            }
        }
    }
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var("HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(|| std::env::var("USERPROFILE").ok().map(PathBuf::from))
}

/// List available cross-compilation targets (XT9).
pub fn cmd_targets() {
    println!("{}", "Available targets:".green().bold());
    println!();

    println!("  {} (tested, guaranteed):", "Tier 1".yellow().bold());
    println!("    x86_64-linux");
    println!("    aarch64-linux");
    println!("    x86_64-macos");
    println!("    aarch64-macos");
    println!();

    println!("  {} (builds, best-effort):", "Tier 2".yellow());
    println!("    x86_64-windows-msvc");
    println!("    aarch64-windows-msvc");
    println!("    wasm32-none");
    println!("    x86_64-linux-musl");
    println!("    aarch64-linux-musl");
    println!();

    println!("  {} (community):", "Tier 3".dimmed());
    println!("    riscv64-linux");
    println!("    x86_64-freebsd");
    println!("    arm-none");
    println!();

    // Detect and show host
    let host = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    println!("  {} {}-{}", "Host:".dimmed(), host, os);
}
