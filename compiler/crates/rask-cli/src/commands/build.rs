// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Build command — struct.build/OD1-OD7, CL1-CL4.

use colored::Colorize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::time::Instant;

use rask_diagnostics::formatter::DiagnosticFormatter;
use rask_diagnostics::ToDiagnostic;
use rask_resolve::PackageError;

use crate::output;

/// Build options parsed from CLI flags.
pub struct BuildOptions {
    pub profile: String,
    pub verbose: bool,
    pub target: Option<String>,
    pub no_cache: bool,
    /// Bypass all caching (build script + compilation). Spec: struct.build/LC2.
    pub force: bool,
    /// F3: leave out the features the manifest marks `default: true`.
    pub no_default_features: bool,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            profile: "debug".to_string(),
            verbose: false,
            target: None,
            no_cache: false,
            force: false,
            no_default_features: false,
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

/// Format a single PackageError to stderr. Returns the error count.
fn format_package_error(e: &PackageError) -> usize {
    match e {
        PackageError::Parse { file, source, errors } => {
            let file_name = file.to_string_lossy();
            let fmt = DiagnosticFormatter::new(source).with_file_name(&file_name);
            for err in errors {
                eprintln!("{}", fmt.format(&err.to_diagnostic()));
            }
            errors.len()
        }
        PackageError::Lex { file, source, errors } => {
            let file_name = file.to_string_lossy();
            let fmt = DiagnosticFormatter::new(source).with_file_name(&file_name);
            for err in errors {
                eprintln!("{}", fmt.format(&err.to_diagnostic()));
            }
            errors.len()
        }
        PackageError::Multiple(errors) => {
            errors.iter().map(format_package_error).sum()
        }
        _ => {
            eprintln!("{}: {}", output::error_label(), e);
            1
        }
    }
}

/// Print a PackageError using the diagnostic formatter when possible.
/// Register each registry package rask.lock pins, from the cache.
fn register_locked(registry: &mut rask_resolve::PackageRegistry, lock: &rask_resolve::LockFile) -> Result<(), String> {
    let cache = rask_resolve::cache::PackageCache::new();
    for pkg in &lock.packages {
        let (Some(url), Some(sum)) = (pkg.source.strip_prefix("registry+"), &pkg.checksum) else { continue };
        if registry.lookup_name(&pkg.name).is_some() {
            continue;
        }
        let dir = cache
            .get(&pkg.name, &pkg.version, sum)
            .ok_or_else(|| format!("registry package '{}' {} not cached — run `rask fetch`", pkg.name, pkg.version))?;
        registry
            .register_cached(&pkg.name, &pkg.version, &dir, url)
            .map_err(|e| format!("registry package '{}' {}: {}", pkg.name, pkg.version, e))?;
    }
    Ok(())
}

/// LK4: an entry for a package nothing depends on any more means the lock
/// is out of date. This build may not see every package (a release build
/// skips dev deps), so anything it doesn't see is looked for again over
/// every scope before it counts as stale.
fn check_no_stale_entries(root: &Path, registry: &rask_resolve::PackageRegistry, lock: &rask_resolve::LockFile) -> Result<(), String> {
    let unseen: Vec<&str> = lock.packages.iter()
        .map(|p| p.name.as_str())
        .filter(|n| registry.lookup_name(n).is_none())
        .collect();
    let mut stale: Vec<&str> = Vec::new();
    if !unseen.is_empty() {
        let mut full = rask_resolve::PackageRegistry::new();
        full.include_scope("dev");
        full.include_scope("build");
        let root_ids = full.discover_workspace(root).map_err(|e| e.to_string())?;
        register_locked(&mut full, lock)?;
        let reachable = reachable_names(&full, &root_ids);
        stale = unseen.into_iter().filter(|n| !reachable.contains(*n)).collect();
    }
    match stale.as_slice() {
        [] => Ok(()),
        names => Err(format!(
            "rask.lock pins {}, which nothing depends on any more — run `rask update`",
            names.iter().map(|n| format!("'{}'", n)).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Every package name reachable from the roots through declared dependencies,
/// in any scope or feature.
fn reachable_names(registry: &rask_resolve::PackageRegistry, roots: &[rask_resolve::PackageId]) -> std::collections::HashSet<String> {
    let mut seen = std::collections::HashSet::new();
    let mut queue: Vec<rask_resolve::PackageId> = roots.to_vec();
    while let Some(id) = queue.pop() {
        let Some(m) = registry.get(id).and_then(|p| p.manifest.as_ref()) else { continue };
        for d in super::fetch::all_deps(m) {
            if seen.insert(d.name.clone()) {
                if let Some(next) = registry.lookup_name(&d.name) {
                    queue.push(next);
                }
            }
        }
    }
    seen
}

fn report_package_error(e: &PackageError) -> ! {
    let count = format_package_error(e);
    let phase = match e {
        PackageError::Lex { .. } => "Lex",
        _ => "Parse",
    };
    eprintln!("\n{}", output::banner_fail(phase, count));
    process::exit(1);
}

/// Compute the output binary path for a project directory.
/// Used by `rask run <dir>` to know what to execute after building.
pub fn project_binary_path(path: &str, profile: &str, target: Option<&str>) -> std::path::PathBuf {
    let root = Path::new(path).canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    let out_dir = output_dir(&root, profile, target);

    // Try to get name from build.rk manifest, fall back to directory name
    let mut registry = rask_resolve::PackageRegistry::new();
    let bin_name = match registry.discover_workspace(&root) {
        Ok(ids) => registry
            .get(ids[0])
            .and_then(|pkg| pkg.manifest.as_ref().map(|m| m.name.clone()))
            .unwrap_or_else(|| binary_name(&root)),
        Err(_) => binary_name(&root),
    };

    out_dir.join(&bin_name)
}

/// State produced by the shared build preparation pipeline.
/// Used by both `cmd_build` and `cmd_test_project`.
pub struct PreparedBuild {
    pub registry: rask_resolve::PackageRegistry,
    pub root_id: rask_resolve::PackageId,
    pub root: PathBuf,
    pub link_opts: super::link::LinkOptions,
    pub bin_name: String,
    pub out_dir: PathBuf,
    pub resolved_feature_names: Vec<String>,
    pub dep_errors: usize,
    pub opts: BuildOptions,
    pub start: Instant,
}

/// Run the shared build preparation pipeline: discover packages, validate
/// manifest, run build script, check dependencies. Exits on fatal errors.
pub fn prepare_build(path: &str, opts: BuildOptions) -> PreparedBuild {
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

    // Validate target triple (XT1-XT3)
    if let Some(ref t) = opts.target {
        if let Err(e) = super::link::validate_target(t) {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
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
    // A dev dependency is present exactly where `test` blocks are.
    // `std.testing/T1` strips them in release, so release is the build that
    // doesn't link one — which is the whole reason to write `[dev-dependencies]`.
    // Every other build compiles the test blocks and needs their imports to
    // resolve.
    let mut registry = PackageRegistry::new();
    let release = opts.profile == "release";
    if !release {
        registry.include_scope("dev");
    }
    let root_ids = match registry.discover_workspace(&root) {
        Ok(ids) => ids,
        Err(e) => report_package_error(&e),
    };
    if release {
        registry.strip_test_blocks();
    }
    let is_workspace = root_ids.len() > 1;
    let mut root_id = root_ids[0];

    // === Validate manifest: version constraints + features (VR1-VR3, F1-F6, FG1-FG6) ===
    let manifest = registry.get(root_id).and_then(|p| p.manifest.clone());
    let mut resolved_feature_names: Vec<String> = Vec::new();
    if let Some(ref manifest) = manifest {
        let mut validation_errors = 0;

        // Validate semver constraints on declared deps
        for dep in &manifest.deps {
            if let Some(ref version) = dep.version {
                if let Err(e) = rask_resolve::semver::Constraint::parse(version) {
                    eprintln!("{}: dep \"{}\": invalid version constraint \"{}\": {}",
                        output::error_label(), dep.name, version, e);
                    validation_errors += 1;
                }
            }
            // Validate path deps exist (D1)
            if let Some(ref dep_path) = dep.path {
                let resolved = root.join(dep_path);
                if !resolved.exists() {
                    eprintln!("{}: dep \"{}\": path not found: {}",
                        output::error_label(), dep.name, resolved.display());
                    validation_errors += 1;
                }
            }
            // Cannot have both path and git (D2)
            if dep.git.is_some() && dep.path.is_some() {
                eprintln!("{}: dep \"{}\": cannot have both 'path' and 'git'",
                    output::error_label(), dep.name);
                validation_errors += 1;
            }
        }

        // Check for duplicate deps (D3)
        let mut seen_deps = std::collections::HashSet::new();
        for dep in &manifest.deps {
            if !seen_deps.insert(&dep.name) {
                eprintln!("{}: duplicate dep \"{}\"", output::error_label(), dep.name);
                validation_errors += 1;
            }
        }

        // Resolve features (F1-F6, FG1-FG6)
        let dep_selections: Vec<(String, String, String)> = manifest.deps.iter()
            .flat_map(|dep| {
                dep.exclusive_selections.iter().map(move |(feat, opt)| {
                    (feat.clone(), opt.clone(), dep.name.clone())
                })
            })
            .collect();

        let features = rask_resolve::features::resolve_features(
            &manifest.features,
            &[],
            opts.no_default_features,
            &dep_selections,
        );

        for err in &features.errors {
            eprintln!("{}: {}", output::error_label(), err);
            validation_errors += 1;
        }
        resolved_feature_names = features.enabled.iter().cloned().collect();

        if opts.verbose && !features.enabled.is_empty() {
            let mut names: Vec<_> = features.enabled.iter().collect();
            names.sort();
            println!("  {} [{}]", "Features:".dimmed(), names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "));
        }
        if opts.verbose && !features.exclusive_selections.is_empty() {
            for (feat, sel) in &features.exclusive_selections {
                println!("  {} {} = \"{}\"", "Feature:".dimmed(), feat, sel);
            }
        }

        if validation_errors > 0 {
            eprintln!("\n{}", output::banner_fail("Validation", validation_errors));
            process::exit(1);
        }
    }

    // Lock file + capability checks (LK1-LK4, PM1-PM8)
    let lock_path = root.join("rask.lock");
    let declares_deps = root_ids.iter()
        .filter_map(|id| registry.get(*id).and_then(|p| p.manifest.as_ref()))
        .any(|m| super::fetch::all_deps(m).next().is_some());
    // LK2: a missing lock is written by the same resolution `rask fetch`
    // runs, over every scope. One written from this build's view would
    // leave out the dev deps a release build skips.
    if declares_deps && !lock_path.exists() {
        if let Err(errors) = super::fetch::resolve_and_lock(&root, super::fetch::Pins::Keep, opts.verbose) {
            for e in &errors {
                eprintln!("{}: {}", output::error_label(), e);
            }
            eprintln!("\n{}", output::banner_fail("Lock", errors.len()));
            process::exit(1);
        }
        if opts.verbose {
            println!("  {} rask.lock", "Generated".green().bold());
        }
    }
    let mut lockfile = if lock_path.exists() {
        match rask_resolve::LockFile::load(&lock_path) {
            Ok(lock) => Some(lock),
            Err(e) => {
                eprintln!("{}: {}", output::error_label(), e);
                process::exit(1);
            }
        }
    } else {
        None
    };
    if let Some(lock) = &lockfile {
        // RG1: registry packages come from the cache, at the versions the
        // lock pins.
        if let Err(e) = register_locked(&mut registry, lock) {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
        // LK3/LK4
        if let Err(e) = lock.verify(&registry, root_id) {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
        if let Err(e) = check_no_stale_entries(&root, &registry, lock) {
            eprintln!("{}: {}", output::error_label(), e);
            process::exit(1);
        }
    }

    let has_external_deps = registry.packages().iter().any(|p| p.id != root_id && p.is_external);
    if has_external_deps {
        use std::collections::BTreeMap;

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
                        "{}: dependency '{}' uses {} but is not allowed\n  add `allow = [\"{}\"]` to its entry in rask.toml",
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

        // PM6: the lock records what each package was seen to use. Updated
        // in place: regenerating from this build's view would drop the
        // entries it doesn't link.
        if let Some(lock) = &mut lockfile {
            let mut changed = false;
            for pkg in &mut lock.packages {
                if let Some(caps) = all_caps.get(&pkg.name) {
                    if *caps != pkg.capabilities {
                        if opts.verbose {
                            println!("  {} capabilities changed for '{}'", "Note:".yellow(), pkg.name);
                        }
                        pkg.capabilities = caps.clone();
                        changed = true;
                    }
                }
            }
            if changed {
                if let Err(e) = lock.write(&lock_path) {
                    eprintln!("warning: failed to update rask.lock: {}", e);
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
    // A build leaves its binary in `build/<profile>/`, so it's worth the debug
    // symbols that a `rask run` on a single file isn't.
    let mut link_opts = super::link::LinkOptions { keeps_binary: true, ..Default::default() };

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
                // Root package is unrestricted (struct.build/PM9)
                allowed_capabilities: None,
            };

            let mut interp = rask_interp::Interpreter::new();
            // The stdlib's Rask-written half comes along, as it does for
            // `rask run --interp`: without it `ctx.package_dir.as_string()`
            // was "no method `as_string` on type `Path`", since Path's
            // methods live in stdlib/path.rk.
            let script_decls = rask_compiler::program_decls(&build_decls, None);
            match interp.run_build(&script_decls, build_state) {
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

    // The root's check (`check_package`) merges every package's declarations
    // into one program and checks that, so a dependency's bodies are checked
    // there. A second, per-package pass used to run first, in parallel, and it
    // handed the checker only that package's own declarations — so a
    // dependency naming a type from *its* dependency failed with `unknown type
    // Doc` before the real check ever ran (#1295).
    let dep_errors = 0;

    PreparedBuild {
        registry,
        root_id,
        root,
        link_opts,
        bin_name,
        out_dir,
        resolved_feature_names,
        dep_errors,
        opts,
        start,
    }
}

pub fn cmd_build(path: &str, opts: BuildOptions) {
    let PreparedBuild {
        registry, root_id, root, link_opts, bin_name, out_dir,
        resolved_feature_names, dep_errors, opts, start,
    } = prepare_build(path, opts);

    let mut total_errors = dep_errors;

    let compile_label = if let Some(ref t) = opts.target {
        format!("{}, {}", opts.profile, t)
    } else {
        opts.profile.clone()
    };
    println!("{} {} ({})", "  Compiling".green().bold(), bin_name, compile_label);

    // Compile root package (full pipeline, with compilation cache XC1-XC5)
    if total_errors == 0 {
        // Extract data from registry before moving it into PackageContext.
        let (source_files, all_decls, pkg_path_string) = {
            let root_pkg = match registry.get(root_id) {
                Some(p) => p,
                None => {
                    eprintln!("{}: root package not found in registry", output::error_label());
                    process::exit(1);
                }
            };
            let all_decls: Vec<_> = root_pkg.all_decls().cloned().collect();
            let pkg_path_string = root_pkg.path_string();

            // Every file that goes into this binary, not just the root
            // package's. Two things read this list, and both were wrong for a
            // package with a subdirectory in it:
            //
            //   - the compilation cache keys on it. A package root holding no
            //     `.rk` files at all hashed nothing, so `rask build` reported
            //     "Finished" and ran the previous binary however the code under
            //     `src/` had changed. Even with files at the root, editing a
            //     sub-package left the key untouched — `helpers.answer()`
            //     going 99 → 7 rebuilt to a binary that still printed 99 (#1100).
            //
            //   - `show_diagnostic_multi` renders the snippet under a
            //     diagnostic from it, so an error pointing into a sub-package
            //     had no source to show.
            // Indexed by file id, not by iteration order: a span carries the
            // id, and the ids run across the whole registry.
            let mut source_files: Vec<(std::path::PathBuf, String)> = Vec::new();
            for pkg in registry.packages() {
                for f in &pkg.files {
                    let slot = f.file_id as usize;
                    if source_files.len() <= slot {
                        source_files
                            .resize(slot + 1, (std::path::PathBuf::new(), String::new()));
                    }
                    source_files[slot] = (f.path.clone(), f.source.clone());
                }
            }

            (source_files, all_decls, pkg_path_string)
        };

        let source_hash = super::cache::hash_source_files(&source_files);
        let target_str = opts.target.as_deref().unwrap_or("native");
        let compiler_fp = super::cache::compiler_fingerprint();
        let cache_key =
            super::cache::compute_cache_key(
                &source_hash, &opts.profile, target_str, &resolved_feature_names, compiler_fp,
            );
        let cache_dir = root.join("build").join(".cache");

        let obj_path = out_dir.join(format!("{}.o", bin_name));
        let bin_path = out_dir.join(&bin_name);
        let obj_str = obj_path.to_string_lossy().to_string();
        let bin_str = bin_path.to_string_lossy().to_string();

        // Check compilation cache (XC1-XC2); --force bypasses
        if !opts.no_cache && !opts.force {
            if let Some(cached_obj) = super::cache::lookup(&cache_dir, &cache_key) {
                if opts.verbose {
                    println!(
                        "  {} (cache hit, compiler {:016x})",
                        "Skipping codegen".dimmed(),
                        compiler_fp
                    );
                }
                if let Err(e) = std::fs::copy(&cached_obj, &obj_path) {
                    eprintln!("warning: cache copy failed: {}", e);
                } else {
                    let release = opts.profile == "release";
                    match super::link::link_executable_with(&obj_str, &bin_str, &link_opts, release, opts.target.as_deref()) {
                        Ok(_) => {
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
            println!("  {} {}", "Checking".dimmed(), pkg_path_string);
        }

        // Run the full pipeline through the shared driver. Moves `registry`
        // into PackageContext — not used after this point.
        let cfg = match super::link::build_cfg(
            opts.target.as_deref(), &opts.profile, resolved_feature_names.clone(),
        ) {
            Ok(cfg) => cfg,
            Err(e) => {
                eprintln!("{}: {}", output::error_label(), e);
                process::exit(1);
            }
        };
        let config = crate::compiler_config(cfg.clone());
        let mut pkg_ctx = rask_compiler::PackageContext {
            registry,
            root_id,
            all_decls,
        };
        let output = rask_compiler::compile_package(&mut pkg_ctx, &config);

        for d in &output.diagnostics {
            crate::show_diagnostic_multi(d, &source_files);
        }

        if let Some(result) = output.result {
            // Comptime globals were evaluated once in the pipeline; hard errors
            // (overflow, divide-by-zero) already failed it (result is None).
            let target = opts.target.as_deref();

            let build_mode = if opts.profile == "release" {
                rask_compiler::emit::BuildMode::Release
            } else {
                rask_compiler::emit::BuildMode::Debug
            };
            // A package is several files, and the back end takes one source
            // for locations. Giving it the right one per function needs the
            // source map #1568 plans, so a package build has none yet.
            let input = rask_compiler::emit::BackEnd::of(&result, None, None);
            match super::compile::emit_object(
                &input, &rask_compiler::emit::Entry::Main, target, build_mode, &obj_str,
            ) {
                Ok(()) => {
                    if !opts.no_cache {
                        let _ = super::cache::store(&cache_dir, &cache_key, &obj_path);
                    }
                    let release = opts.profile == "release";
                    if let Err(e) = super::link::link_executable_with(&obj_str, &bin_str, &link_opts, release, opts.target.as_deref()) {
                        eprintln!("link error: {}", e);
                        total_errors += 1;
                    }
                }
                Err(errors) => {
                    for e in &errors {
                        eprintln!("error: {}", e);
                    }
                    total_errors += errors.len();
                }
            }
        } else {
            total_errors += output.diagnostics.iter()
                .filter(|d| matches!(d.severity, rask_diagnostics::Severity::Error))
                .count();
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
///
/// From `rask_codegen::targets`, which is also what maps a name to a triple —
/// a list that advertised a target codegen would build for something else is
/// how `aarch64-macos` came to emit ELF (#1185).
pub fn cmd_targets() {
    use rask_codegen::targets::{Tier, TARGETS};
    println!("{}", "Available targets:".green().bold());
    println!();

    let tiers = [
        (Tier::One, "Tier 1".yellow().bold(), "tested, guaranteed"),
        (Tier::Two, "Tier 2".yellow(), "builds, best-effort"),
        (Tier::Three, "Tier 3".dimmed(), "community"),
    ];
    for (tier, heading, note) in tiers {
        println!("  {} ({}):", heading, note);
        for t in TARGETS.iter().filter(|t| t.tier == tier) {
            println!("    {:<22} {}", t.name, t.triple.dimmed());
        }
        println!();
    }

    // Detect and show host
    let host = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    println!("  {} {}-{}", "Host:".dimmed(), host, os);
}
