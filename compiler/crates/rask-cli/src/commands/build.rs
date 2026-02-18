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
    pub no_cache: bool,
    /// Bypass all caching (build script + compilation). Spec: struct.build/LC2.
    pub force: bool,
    /// Max parallel threads for dependency checking. Spec: struct.build/PP3.
    /// None = CPU count (default).
    pub jobs: Option<usize>,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            profile: "debug".to_string(),
            verbose: false,
            target: None,
            no_cache: false,
            force: false,
            jobs: None,
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
    use rask_ast::decl::DeclKind;
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

    // === LC1 Step 1-3: Parse build.rk, discover packages + deps ===
    let mut registry = PackageRegistry::new();
    let mut root_id = match registry.discover(&root) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    };

    // Lock file + capability checks (LK1-LK4, PM1-PM8)
    let lock_path = root.join("rask.lock");
    let has_external_deps = registry.packages().iter().any(|p| p.id != root_id && p.is_external);

    if has_external_deps {
        use std::collections::BTreeMap;

        // Verify existing lock file checksums (LK3)
        if lock_path.exists() {
            match rask_resolve::LockFile::load(&lock_path) {
                Ok(lockfile) => {
                    if let Err(e) = lockfile.verify(&registry, root_id) {
                        eprintln!("{}: {}", output::error_label(), e);
                        process::exit(1);
                    }
                }
                Err(e) => {
                    eprintln!("{}: {}", output::error_label(), e);
                    process::exit(1);
                }
            }
        }

        // Infer capabilities for each external dependency (PM1)
        let root_allows: BTreeMap<String, Vec<String>> = registry.get(root_id)
            .and_then(|pkg| pkg.manifest.as_ref())
            .map(|manifest| {
                manifest.deps.iter()
                    .map(|dep| (dep.name.clone(), dep.allow.clone()))
                    .collect()
            })
            .unwrap_or_default();

        let mut all_caps: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut cap_errors = 0;

        for pkg in registry.packages() {
            if pkg.id == root_id { continue; }
            if !pkg.is_external { continue; }

            let decls: Vec<_> = pkg.all_decls().cloned().collect();
            let inferred = rask_resolve::capabilities::infer_capabilities(&decls);
            all_caps.insert(pkg.name.clone(), inferred.clone());

            if inferred.is_empty() { continue; }

            // Check against allow list (PM3)
            let allowed = root_allows.get(&pkg.name)
                .cloned()
                .unwrap_or_default();

            let violations = rask_resolve::capabilities::check_capabilities(&inferred, &allowed);
            if !violations.is_empty() {
                for cap in &violations {
                    eprintln!(
                        "{}: dependency '{}' uses {} but is not allowed\n  add `allow: [\"{}\"]` to the dep declaration in build.rk",
                        output::error_label(),
                        pkg.name,
                        rask_resolve::capabilities::capability_description(cap),
                        cap,
                    );
                }
                cap_errors += violations.len();
            }
        }

        if cap_errors > 0 {
            eprintln!("\n{}", output::banner_fail("Capability check", cap_errors));
            process::exit(1);
        }

        // Generate or update lock file with capabilities (LK1-LK2)
        let lockfile = rask_resolve::LockFile::generate_with_capabilities(
            &registry, root_id, &root, &all_caps,
        );
        if !lock_path.exists() {
            if let Err(e) = lockfile.write(&lock_path) {
                eprintln!("warning: failed to write rask.lock: {}", e);
            } else if opts.verbose {
                println!("  {} rask.lock", "Generated".green().bold());
            }
        } else {
            // Check if capabilities changed, update if so
            if let Ok(existing) = rask_resolve::LockFile::load(&lock_path) {
                let changed = existing.capabilities_changed(&all_caps);
                if !changed.is_empty() {
                    if opts.verbose {
                        for name in &changed {
                            println!("  {} capabilities changed for '{}'", "Note:".yellow(), name);
                        }
                    }
                    // Re-write with updated capabilities
                    let _ = lockfile.write(&lock_path);
                }
            }
        }
    }

    // Binary name from build.rk package name or directory name (OD4)
    let bin_name = registry.get(root_id)
        .and_then(|pkg| pkg.manifest.as_ref().map(|m| m.name.clone()))
        .unwrap_or_else(|| binary_name(&root));

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

    if opts.verbose {
        println!("  Discovered {} package(s):", registry.len());
        for pkg in registry.packages() {
            let file_count = pkg.files.len();
            let decl_count: usize = pkg.files.iter().map(|f| f.decls.len()).sum();
            let dep_label = if pkg.is_external { " (dependency)" } else { "" };
            println!(
                "    {}{} ({} file{}, {} decl{})",
                pkg.path_string(),
                dep_label,
                file_count,
                if file_count == 1 { "" } else { "s" },
                decl_count,
                if decl_count == 1 { "" } else { "s" }
            );
        }
        println!();
    }

    // === LC1 Step 6: Run build script if func build() exists ===
    let build_decls: Vec<_> = registry.get(root_id)
        .map(|pkg| pkg.build_decls.clone())
        .unwrap_or_default();

    let has_build_fn = build_decls.iter().any(|d| {
        matches!(&d.kind, DeclKind::Fn(f) if f.name == "build")
    });

    let host_triple = format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS);
    let gen_dir = root.join(".rk-gen");
    let mut link_opts = super::link::LinkOptions::default();

    if has_build_fn {
        let build_cache_dir = root.join("build").join(".build-cache");

        // LC2: Check if build script can be skipped
        let build_rk_path = root.join("build.rk");
        let build_rk_content = fs::read_to_string(&build_rk_path).unwrap_or_default();
        let mut skip_build_script = false;

        if !opts.force {
            if let Some(cached) = super::cache::load_build_cache(&build_cache_dir) {
                let current_hash = super::cache::hash_build_inputs(&build_rk_content, &cached.deps);
                if current_hash == cached.hash {
                    // Cache hit — restore link state, skip execution
                    if opts.verbose {
                        println!("  {} build script (unchanged)", "Skipping".dimmed());
                    }
                    link_opts.libs = cached.libs;
                    link_opts.search_paths = cached.search_paths;
                    link_opts.objects = cached.objects.iter().map(String::from).collect();
                    skip_build_script = true;

                    // Re-discover to pick up any .rk-gen/ files from previous run
                    if gen_dir.exists() {
                        let mut fresh_registry = PackageRegistry::new();
                        match fresh_registry.discover(&root) {
                            Ok(new_id) => {
                                registry = fresh_registry;
                                root_id = new_id;
                            }
                            Err(e) => {
                                eprintln!("{}: re-discovery after cached build script: {}", output::error_label(), e);
                                process::exit(1);
                            }
                        }
                    }
                }
            }
        }

        if !skip_build_script {
            if opts.verbose {
                println!("  {} build.rk", "Running".yellow().bold());
            }

            // Create .rk-gen/ directory for generated files
            if let Err(e) = fs::create_dir_all(&gen_dir) {
                eprintln!("{}: failed to create .rk-gen directory: {}", output::error_label(), e);
                process::exit(1);
            }

            let pkg_version = registry.get(root_id)
                .and_then(|pkg| pkg.manifest.as_ref().map(|m| m.version.clone()))
                .unwrap_or_else(|| "0.0.0".into());

            let step_cache = root.join("build").join(".build-cache").join("steps");

            let build_state = rask_interp::BuildState {
                package_name: bin_name.clone(),
                package_version: pkg_version,
                package_dir: root.clone(),
                profile: opts.profile.clone(),
                target: opts.target.clone().unwrap_or_else(|| host_triple.clone()),
                host: host_triple.clone(),
                gen_dir: gen_dir.clone(),
                out_dir: out_dir.clone(),
                step_cache_dir: Some(step_cache),
                link_libraries: Vec::new(),
                link_search_paths: Vec::new(),
                extra_objects: Vec::new(),
                declared_deps: Vec::new(),
                tool_versions: std::collections::HashMap::new(),
            };

            let mut interp = rask_interp::Interpreter::new();
            match interp.run_build(&build_decls, build_state) {
                Ok(_) => {
                    if opts.verbose {
                        println!("  {} build script", "Finished".green().bold());
                    }

                    // Extract accumulated link state + save LC2 cache
                    if let Some(state) = interp.take_build_state() {
                        link_opts.libs = state.link_libraries.clone();
                        link_opts.search_paths = state.link_search_paths.clone();
                        let obj_strings: Vec<String> = state.extra_objects
                            .iter()
                            .map(|p| p.to_string_lossy().into_owned())
                            .collect();
                        link_opts.objects = obj_strings.clone();

                        // Persist build script cache (LC2)
                        let new_hash = super::cache::hash_build_inputs(&build_rk_content, &state.declared_deps);
                        if let Err(e) = super::cache::save_build_cache(
                            &build_cache_dir,
                            new_hash,
                            &state.declared_deps,
                            &state.link_libraries,
                            &state.link_search_paths,
                            &obj_strings,
                        ) {
                            if opts.verbose {
                                eprintln!("warning: failed to cache build script state: {}", e);
                            }
                        }
                    }

                    // Re-discover to pick up any .rk-gen/ files
                    let mut fresh_registry = PackageRegistry::new();
                    match fresh_registry.discover(&root) {
                        Ok(new_id) => {
                            registry = fresh_registry;
                            root_id = new_id;
                        }
                        Err(e) => {
                            eprintln!("{}: re-discovery after build script: {}", output::error_label(), e);
                            process::exit(1);
                        }
                    }
                }
                Err(diag) => {
                    eprintln!("{}: build script failed: {}", output::error_label(), diag.error);
                    process::exit(1);
                }
            }
        }
    }

    // === LC1 Step 7: Check all packages, codegen root only ===
    let mut total_errors = 0;

    // Check dependencies in parallel by dependency level (PP1-PP3)
    let dep_levels = toposort_levels(&registry, root_id);
    let max_jobs = opts.jobs.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    });

    if opts.verbose {
        println!("  {} {} job(s)", "Parallelism:".dimmed(), max_jobs);
    }

    for level in &dep_levels {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let level_errors = AtomicUsize::new(0);

        // Process packages in chunks of max_jobs (PP3)
        for chunk in level.chunks(max_jobs) {
            std::thread::scope(|s| {
                for &pkg_id in chunk {
                    let registry = &registry;
                    let level_errors = &level_errors;
                    let verbose = opts.verbose;

                    s.spawn(move || {
                        let pkg = match registry.get(pkg_id) {
                            Some(p) => p,
                            None => return,
                        };

                        if verbose {
                            println!("  {} {}", "Checking".dimmed(), pkg.path_string());
                        }

                        let mut all_decls: Vec<_> = pkg.all_decls().cloned().collect();
                        rask_desugar::desugar(&mut all_decls);

                        match rask_resolve::resolve_package(&all_decls, registry, pkg_id) {
                            Ok(resolved) => {
                                if let Err(errors) = rask_types::typecheck(resolved, &all_decls) {
                                    for error in &errors {
                                        eprintln!("type error: {}", error);
                                    }
                                    level_errors.fetch_add(errors.len(), Ordering::Relaxed);
                                }
                            }
                            Err(errors) => {
                                for error in &errors {
                                    eprintln!("resolve error: {}", error.kind);
                                }
                                level_errors.fetch_add(errors.len(), Ordering::Relaxed);
                            }
                        }
                    });
                }
            });
        }

        total_errors += level_errors.load(Ordering::Relaxed);
        if total_errors > 0 {
            break; // Don't check later levels if earlier ones failed
        }
    }

    // Compile root package (full pipeline, with compilation cache XC1-XC5)
    if total_errors == 0 {
        if let Some(root_pkg) = registry.get(root_id) {
            // Compute cache key from source files
            let source_files: Vec<_> = root_pkg.files.iter()
                .map(|f| (f.path.clone(), f.source.clone()))
                .collect();
            let source_hash = super::cache::hash_source_files(&source_files);
            let target_str = opts.target.as_deref().unwrap_or("native");
            let cache_key = super::cache::compute_cache_key(&source_hash, &opts.profile, target_str);
            let cache_dir = root.join("build").join(".cache");

            let obj_path = out_dir.join(format!("{}.o", bin_name));
            let bin_path = out_dir.join(&bin_name);
            let obj_str = obj_path.to_string_lossy().to_string();
            let bin_str = bin_path.to_string_lossy().to_string();

            // Check compilation cache (XC1-XC2); --force bypasses
            if !opts.no_cache && !opts.force {
                if let Some(cached_obj) = super::cache::lookup(&cache_dir, &cache_key) {
                    if opts.verbose {
                        println!("  {} (cache hit)", "Skipping codegen".dimmed());
                    }
                    if let Err(e) = std::fs::copy(&cached_obj, &obj_path) {
                        eprintln!("warning: cache copy failed: {}", e);
                        // Fall through to normal compilation
                    } else {
                        match super::link::link_executable_with(&obj_str, &bin_str, &link_opts) {
                            Ok(_) => {
                                // Success — skip to report
                                let elapsed = start.elapsed();
                                println!();
                                println!(
                                    "   {} {} ({}) [{:.2}s]",
                                    "Finished".green().bold(),
                                    bin_path.display(),
                                    opts.profile,
                                    elapsed.as_secs_f64()
                                );
                                return;
                            }
                            Err(e) => {
                                eprintln!("link error: {}", e);
                                total_errors += 1;
                            }
                        }
                    }
                }
            }

            if opts.verbose {
                println!("  {} {}", "Checking".dimmed(), root_pkg.path_string());
            }

            let mut all_decls: Vec<_> = root_pkg.all_decls().cloned().collect();
            rask_desugar::desugar(&mut all_decls);

            match rask_resolve::resolve_package(&all_decls, &registry, root_id) {
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
                                rask_hidden_params::desugar_hidden_params(&mut all_decls);

                                match rask_mono::monomorphize(&typed, &all_decls) {
                                    Ok(mono) => {
                                        let mut all_mono_decls: Vec<_> = mono.functions.iter().map(|f| {
                                            let mut decl = f.body.clone();
                                            if let rask_ast::decl::DeclKind::Fn(ref mut fn_decl) = decl.kind {
                                                fn_decl.name = f.name.clone();
                                            }
                                            decl
                                        }).collect();
                                        all_mono_decls.extend(all_decls.iter().filter(|d| matches!(&d.kind, rask_ast::decl::DeclKind::Extern(_))).cloned());
                                        let comptime_globals = std::collections::HashMap::new();
                                        let extern_funcs = super::codegen::collect_extern_func_names(&all_decls);
                                        let mir_ctx = rask_mir::lower::MirContext {
                                            struct_layouts: &mono.struct_layouts,
                                            enum_layouts: &mono.enum_layouts,
                                            node_types: &typed.node_types,
                                            comptime_globals: &comptime_globals,
                                            extern_funcs: &extern_funcs,
                                            line_map: None,
                                            source_file: None,
                                        };

                                        let mut mir_functions = Vec::new();
                                        let mut mir_errors = 0;

                                        for mono_fn in &mono.functions {
                                            match rask_mir::lower::MirLowerer::lower_function_named(&mono_fn.body, &all_mono_decls, &mir_ctx, Some(&mono_fn.name)) {
                                                Ok(mir_fns) => mir_functions.extend(mir_fns),
                                                Err(e) => {
                                                    eprintln!("MIR lowering error in '{}': {:?}", mono_fn.name, e);
                                                    mir_errors += 1;
                                                }
                                            }
                                        }

                                        total_errors += mir_errors;

                                        // Closure optimization: escape analysis + cross-function ownership + drops
                                        rask_mir::optimize_all_closures(&mut mir_functions);

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
                                                        match codegen.emit_object(&obj_str) {
                                                            Ok(_) => {
                                                                // Cache the compiled object (XC1)
                                                                if !opts.no_cache {
                                                                    let _ = super::cache::store(&cache_dir, &cache_key, &obj_path);
                                                                }
                                                                match super::link::link_executable_with(&obj_str, &bin_str, &link_opts) {
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
    }

    // === LC1 Step 8-9: Link + report ===
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

/// Regenerate rask.lock from current dependency state (LK3, PM6).
pub fn cmd_update(path: &str) {
    use rask_resolve::PackageRegistry;
    use std::collections::BTreeMap;

    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));

    if !root.is_dir() {
        eprintln!("{}: not a directory: {}", output::error_label(), output::file_path(path));
        process::exit(1);
    }

    let mut registry = PackageRegistry::new();
    let root_id = match registry.discover(&root) {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    };

    // Infer capabilities for all external deps (PM6)
    let mut all_caps: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for pkg in registry.packages() {
        if pkg.id == root_id { continue; }
        if !pkg.is_external { continue; }

        let decls: Vec<_> = pkg.all_decls().cloned().collect();
        let inferred = rask_resolve::capabilities::infer_capabilities(&decls);
        all_caps.insert(pkg.name.clone(), inferred);
    }

    // Detect capability changes from previous lock (PM6)
    let lock_path = root.join("rask.lock");
    if lock_path.exists() {
        if let Ok(old_lock) = rask_resolve::LockFile::load(&lock_path) {
            let changed = old_lock.capabilities_changed(&all_caps);
            for name in &changed {
                if let Some(new_caps) = all_caps.get(name) {
                    println!("  {} capabilities changed for '{}': [{}]",
                        "Warning:".yellow().bold(), name, new_caps.join(", "));
                }
            }
        }
    }

    let lockfile = rask_resolve::LockFile::generate_with_capabilities(
        &registry, root_id, &root, &all_caps,
    );

    if lockfile.is_empty() {
        if lock_path.exists() {
            let _ = fs::remove_file(&lock_path);
            println!("  {} rask.lock (no dependencies)", "Removed".green());
        } else {
            println!("  {} (no dependencies to lock)", "OK".green());
        }
    } else {
        match lockfile.write(&lock_path) {
            Ok(_) => {
                println!("  {} rask.lock ({} package{})", "Updated".green().bold(),
                    lockfile.packages.len(),
                    if lockfile.packages.len() == 1 { "" } else { "s" });
            }
            Err(e) => {
                eprintln!("{}: {}", output::error_label(), e);
                process::exit(1);
            }
        }
    }
}

/// Topological sort of dependency packages into parallel levels (Kahn's algorithm).
/// Returns levels where all packages in a level are independent of each other.
/// Root package is excluded (it compiles separately after all deps).
fn toposort_levels(
    registry: &rask_resolve::PackageRegistry,
    root_id: rask_resolve::PackageId,
) -> Vec<Vec<rask_resolve::PackageId>> {
    use std::collections::HashMap;

    // Build in-degree map: pkg depends on dep → pkg gets +1 in-degree
    let mut in_deg: HashMap<rask_resolve::PackageId, usize> = HashMap::new();
    for pkg in registry.packages() {
        if pkg.id == root_id { continue; }
        in_deg.entry(pkg.id).or_insert(0);
    }
    for pkg in registry.packages() {
        if pkg.id == root_id { continue; }
        for &dep_id in &pkg.imports {
            if dep_id != root_id && in_deg.contains_key(&dep_id) {
                *in_deg.get_mut(&pkg.id).unwrap() += 1;
            }
        }
    }

    let mut levels = Vec::new();

    loop {
        // Collect packages with no unresolved dependencies
        let level: Vec<_> = in_deg.iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();

        if level.is_empty() { break; }

        for &id in &level {
            in_deg.remove(&id);
        }

        // Decrement in-degree for packages that depended on this level
        let level_set: std::collections::HashSet<_> = level.iter().copied().collect();
        for pkg in registry.packages() {
            if let Some(deg) = in_deg.get_mut(&pkg.id) {
                let resolved = pkg.imports.iter().filter(|d| level_set.contains(d)).count();
                *deg -= resolved;
            }
        }

        levels.push(level);
    }

    levels
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
