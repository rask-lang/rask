// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Package discovery and management.
//!
//! A package in Rask is a directory containing `.rk` source files.
//! All files in a directory form one package. Nested directories
//! are separate packages (e.g., `pkg/sub/` is package `pkg.sub`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::fs;

use rask_ast::decl::{Decl, DeclKind, DepDecl, PackageDecl};

/// Unique identifier for a package.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PackageId(pub u32);

/// A discovered package.
#[derive(Debug)]
pub struct Package {
    /// Unique identifier for this package.
    pub id: PackageId,
    /// Package name (from rask.toml or directory name).
    pub name: String,
    /// Full package path (e.g., ["pkg", "sub"] for `pkg.sub`).
    pub path: Vec<String>,
    /// Root directory of the package.
    pub root_dir: PathBuf,
    /// Source files in this package.
    pub files: Vec<SourceFile>,
    /// Packages this package imports (populated during resolution).
    pub imports: Vec<PackageId>,
    /// Package metadata from rask.toml (if present).
    pub manifest: Option<PackageDecl>,
    /// Declarations from build.rk (for running func build()).
    pub build_decls: Vec<Decl>,
    /// Whether this is an external dependency (vs local sub-package).
    pub is_external: bool,
    /// Registry source URL (set for packages fetched from a remote registry).
    /// e.g., "https://packages.rask-lang.dev"
    pub registry_source: Option<String>,
}

/// A source file within a package.
#[derive(Debug)]
pub struct SourceFile {
    /// Path to the source file.
    pub path: PathBuf,
    /// Parsed declarations from this file.
    pub decls: Vec<Decl>,
    /// Original source text (for diagnostics).
    pub source: String,
    /// Which slot this file's spans point at. Unique across every package in
    /// the registry, so a diagnostic from a dependency renders against the
    /// dependency's source rather than whatever file the root happened to put
    /// in that slot.
    pub file_id: u16,
}

/// Registry of all discovered packages.
#[derive(Debug, Default)]
pub struct PackageRegistry {
    /// All packages, indexed by PackageId.
    packages: Vec<Package>,
    /// Map from package path to ID.
    path_to_id: HashMap<Vec<String>, PackageId>,
    /// Map from simple package name to ID (for single-segment imports).
    name_to_id: HashMap<String, PackageId>,
    /// Dependencies currently being discovered (cycle detection).
    discovering: HashSet<PathBuf>,
    /// Next unused node id, shared by every package.
    ///
    /// This used to restart at 0 for each package, and the consumer's
    /// declarations are merged with its dependencies' into one program — so
    /// two unrelated expressions carried the same id and the checker's record
    /// of one was read as the other's. `let v = libpkg.ones(3)` came out of
    /// MIR typed `i64` because a node in the library held that id first, and
    /// `v.len()` was emitted as `i64_len` (#1125).
    next_node_id: u32,
    /// Next unused file id, likewise shared.
    next_file_id: u16,
    /// Scopes this build links beyond the unscoped deps (`struct.build/D4`).
    /// Empty is a release build: unscoped deps only.
    ///
    /// Only the root's scoped deps are honoured. A dependency's own dev deps
    /// are its test runner's business — pulling them in would link half the
    /// ecosystem to run one test.
    root_scopes: Vec<String>,
    /// Root deps this build skipped, and the scope that would have linked
    /// them. An import of one gets to say *why* the package isn't here.
    unlinked_scopes: HashMap<String, String>,
}

/// Error that can occur during package discovery.
#[derive(Debug)]
pub enum PackageError {
    /// I/O error while reading files or directories.
    Io(std::io::Error, PathBuf),
    /// Parse error in a source file.
    Parse {
        file: PathBuf,
        source: String,
        errors: Vec<rask_parser::ParseError>,
    },
    /// Lex error in a source file.
    Lex {
        file: PathBuf,
        source: String,
        errors: Vec<rask_lexer::LexError>,
    },
    /// Circular dependency between packages.
    CircularDependency(Vec<String>),
    /// Package not found.
    NotFound(Vec<String>),
    /// No .rk files found in directory.
    EmptyPackage(PathBuf),
    /// More source files than the `file_id` space can name. Past this every
    /// diagnostic would be attributed to the wrong file.
    TooManyFiles { limit: u16 },
    /// `rask.toml` didn't load: every problem the manifest loader found,
    /// one per line.
    Manifest { file: PathBuf, message: String },
    /// Multiple errors across files.
    Multiple(Vec<PackageError>),
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageError::Io(err, path) => {
                write!(f, "I/O error at {}: {}", path.display(), err)
            }
            PackageError::Parse { file, errors, .. } => {
                let msgs: Vec<_> = errors.iter().map(|e| e.to_string()).collect();
                write!(f, "Parse errors in {}:\n{}", file.display(), msgs.join("\n"))
            }
            PackageError::Lex { file, errors, .. } => {
                let msgs: Vec<_> = errors.iter().map(|e| e.to_string()).collect();
                write!(f, "Lex errors in {}:\n{}", file.display(), msgs.join("\n"))
            }
            PackageError::CircularDependency(path) => {
                write!(f, "Circular dependency: {}", path.join(" -> "))
            }
            PackageError::NotFound(path) => {
                write!(f, "Package not found: {}", path.join("."))
            }
            PackageError::EmptyPackage(path) => {
                write!(f, "No .rk files found in {}", path.display())
            }
            PackageError::TooManyFiles { limit } => {
                write!(
                    f,
                    "package has more than {} source files — past that a file \
                     can't be named in a diagnostic without colliding with the \
                     stdlib's own sources. Split it into several packages.",
                    limit
                )
            }
            PackageError::Manifest { file, message } => {
                write!(f, "{}:\n{}", file.display(), message)
            }
            PackageError::Multiple(errors) => {
                for (i, e) in errors.iter().enumerate() {
                    if i > 0 { writeln!(f)?; }
                    write!(f, "{}", e)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for PackageError {}

// =========================================================================
// Shared helpers (no &self — pure functions on paths)
// =========================================================================

/// A package's own claims (type.claims/W1). Never one of its sources: the
/// build compiles it as a package of its own that imports this one.
pub const CLAIMS_FILE: &str = "claims.rk";

/// Collect .rk source file paths from a directory (excluding build.rk and
/// claims.rk). Also includes any .rk files from the .rk-gen/ subdirectory.
/// The source files of the package in `dir`, the same ones the registry
/// reads; its subpackages' aren't among them.
pub fn package_source_files(dir: &Path) -> Vec<PathBuf> {
    collect_rk_files(dir).map(|(files, _)| files).unwrap_or_default()
}

fn collect_rk_files(dir: &Path) -> Result<(Vec<PathBuf>, Vec<PathBuf>), PackageError> {
    let mut files = Vec::new();
    let mut subdirs = Vec::new();

    let entries = fs::read_dir(dir)
        .map_err(|e| PackageError::Io(e, dir.to_path_buf()))?;

    for entry in entries {
        let entry = entry.map_err(|e| PackageError::Io(e, dir.to_path_buf()))?;
        let path = entry.path();

        if path.is_file() {
            if let Some(ext) = path.extension() {
                let name = path.file_name().and_then(|n| n.to_str());
                if ext == "rk" && name != Some("build.rk") && name != Some(CLAIMS_FILE) {
                    files.push(path);
                }
            }
        } else if path.is_dir() {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !name.starts_with('.') && !name.starts_with('_') && name != "build" {
                subdirs.push(path);
            }
        }
    }

    // Include .rk-gen/ files (generated by build scripts)
    let gen_dir = dir.join(".rk-gen");
    if gen_dir.is_dir() {
        if let Ok(entries) = fs::read_dir(&gen_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().map(|e| e == "rk").unwrap_or(false) {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    subdirs.sort();

    Ok((files, subdirs))
}

/// Lex and parse a list of .rk file paths into SourceFiles.
///
/// Node ids and file ids come from the registry's own counters, so they stay
/// unique across every file of every package.
fn parse_rk_files(
    next_id: &mut u32,
    next_file_id: &mut u16,
    paths: Vec<PathBuf>,
) -> Result<Vec<SourceFile>, PackageError> {
    let mut sources = Vec::new();
    let mut read_errors = Vec::new();
    for file_path in paths {
        match fs::read_to_string(&file_path) {
            Ok(s) => sources.push((file_path, s)),
            Err(e) => read_errors.push(PackageError::Io(e, file_path)),
        }
    }
    let parsed = parse_rk_sources(next_id, next_file_id, sources);
    match (parsed, read_errors.len()) {
        (Ok(files), 0) => Ok(files),
        (Ok(_), 1) => Err(read_errors.pop().unwrap()),
        (Ok(_), _) => Err(PackageError::Multiple(read_errors)),
        (Err(e), 0) => Err(e),
        (Err(e), _) => {
            read_errors.push(e);
            Err(PackageError::Multiple(read_errors))
        }
    }
}

/// Lex and parse sources already in hand, each with the path it reports as.
fn parse_rk_sources(
    next_id: &mut u32,
    next_file_id: &mut u16,
    sources: Vec<(PathBuf, String)>,
) -> Result<Vec<SourceFile>, PackageError> {
    let mut source_files = Vec::new();
    let mut file_errors = Vec::new();
    for (file_path, source) in sources {

        // Same id the parser is about to use — token spans have to agree with
        // the composite spans the parser builds, or half the diagnostics in this
        // file point somewhere else.
        // The stdlib's sources hold the top of the `file_id` range. Running
        // into it would make a package file and a stdlib file share an id, and
        // every diagnostic from one would be rendered against the other — so
        // say so instead of wrapping into it silently.
        if *next_file_id >= rask_stdlib::stubs::STDLIB_FILE_ID_BASE {
            file_errors.push(PackageError::TooManyFiles {
                limit: rask_stdlib::stubs::STDLIB_FILE_ID_BASE,
            });
            break;
        }

        let file_id = *next_file_id;
        let mut lexer = rask_lexer::Lexer::new_with_file_id(&source, file_id);
        let lex_result = lexer.tokenize();
        if !lex_result.is_ok() {
            file_errors.push(PackageError::Lex {
                file: file_path,
                source,
                errors: lex_result.errors,
            });
            continue;
        }

        let mut parser =
            rask_parser::Parser::new_with_file_id(lex_result.tokens, *next_id, file_id);
        let parse_result = parser.parse();
        *next_id = parser.next_node_id();
        if !parse_result.is_ok() {
            file_errors.push(PackageError::Parse {
                file: file_path,
                source,
                errors: parse_result.errors,
            });
            continue;
        }

        source_files.push(SourceFile {
            path: file_path,
            source,
            decls: parse_result.decls,
            file_id,
        });
        *next_file_id += 1;
    }

    if !file_errors.is_empty() {
        if file_errors.len() == 1 {
            return Err(file_errors.into_iter().next().unwrap());
        }
        return Err(PackageError::Multiple(file_errors));
    }
    Ok(source_files)
}

/// The manifest file a package declares itself in (struct.manifest).
pub const MANIFEST_FILE: &str = "rask.toml";

/// Is `dir` the root of a package? A manifest makes it one, and so does a
/// build script on its own (struct.build/PK4: no manifest needed when there
/// are no dependencies).
pub fn is_package_root(dir: &Path) -> bool {
    dir.join(MANIFEST_FILE).is_file() || dir.join("build.rk").is_file()
}

/// Reads `rask.toml`. The loader is Rask code the toolchain runs on its
/// interpreter (rask-manifest), which sits above this crate, so the binary
/// hands it in at startup with `set_manifest_loader`.
pub type ManifestLoader = fn(&Path) -> Result<PackageDecl, String>;

static MANIFEST_LOADER: std::sync::OnceLock<ManifestLoader> = std::sync::OnceLock::new();

/// Install the function that reads `rask.toml`. The first call wins.
pub fn set_manifest_loader(loader: ManifestLoader) {
    let _ = MANIFEST_LOADER.set(loader);
}

fn load_manifest(path: &Path) -> Result<PackageDecl, PackageError> {
    let Some(loader) = MANIFEST_LOADER.get() else {
        return Err(PackageError::Manifest {
            file: path.to_path_buf(),
            message: "this tool was built without the manifest loader, so it can't read rask.toml \
                      (a binary installs it with rask_manifest::install())"
                .to_string(),
        });
    };
    loader(path).map_err(|message| PackageError::Manifest { file: path.to_path_buf(), message })
}

/// Read what a package directory declares: the manifest from `rask.toml`,
/// and the declarations of `build.rk` for running `func build()`. Either
/// file may be missing (PK4).
fn read_package_dir(dir: &Path) -> Result<(Option<PackageDecl>, Vec<Decl>), PackageError> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let manifest = if manifest_path.is_file() {
        Some(load_manifest(&manifest_path)?)
    } else {
        None
    };
    let build_rk = dir.join("build.rk");
    let build_decls = if build_rk.is_file() { parse_build_rk(&build_rk)? } else { Vec::new() };
    Ok((manifest, build_decls))
}

/// Parse build.rk, the build script.
fn parse_build_rk(path: &Path) -> Result<Vec<Decl>, PackageError> {
    let source = fs::read_to_string(path)
        .map_err(|e| PackageError::Io(e, path.to_path_buf()))?;

    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();
    if !lex_result.is_ok() {
        return Err(PackageError::Lex {
            file: path.to_path_buf(),
            source,
            errors: lex_result.errors,
        });
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();
    if !parse_result.is_ok() {
        return Err(PackageError::Parse {
            file: path.to_path_buf(),
            source,
            errors: parse_result.errors,
        });
    }
    Ok(parse_result.decls)
}

/// Path dependencies a build with these scopes active links.
///
/// A dep with no scope is linked by everything. A `[dev-dependencies]` dep is linked
/// only when `"dev"` is active, and likewise `"build"` — which is the whole
/// point of writing one (`struct.build/D4`).
fn path_deps(
    manifest: &Option<PackageDecl>,
    scopes: &[String],
    skipped: &mut HashMap<String, String>,
) -> Vec<DepDecl> {
    let Some(m) = manifest.as_ref() else { return Vec::new() };
    let mut kept = Vec::new();
    for dep in m.deps.iter().filter(|d| d.path.is_some()) {
        match &dep.scope {
            Some(scope) if !scopes.iter().any(|active| active == scope) => {
                skipped.insert(dep.name.clone(), scope.clone());
            }
            _ => kept.push(dep.clone()),
        }
    }
    kept
}

// =========================================================================
// PackageRegistry
// =========================================================================

impl PackageRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop every `test` and `benchmark` block from every package
    /// (`std.testing/T1`: stripped in release builds).
    ///
    /// Done here rather than at each consumer so nothing can see a program
    /// the build is not actually compiling — and because it's what makes
    /// `[dev-dependencies]` enforceable: with the test blocks gone, a release build
    /// has no honest reason to reach a dev dependency, so refusing to link
    /// one can't break working code.
    pub fn strip_test_blocks(&mut self) {
        // An import of an unlinked scoped dep goes with them. Its only honest
        // user was a test block, and leaving it behind would fail the build
        // on a line that is now dead. A *use* that survives the strip is a
        // different matter and still fails — as an undefined symbol that
        // names the scope.
        let unlinked: Vec<String> = self.unlinked_scopes.keys().cloned().collect();
        for pkg in &mut self.packages {
            for file in &mut pkg.files {
                file.decls.retain(|d| match &d.kind {
                    DeclKind::Test(_) | DeclKind::Benchmark(_) => false,
                    DeclKind::Import(i) => {
                        !i.path.first().is_some_and(|seg| unlinked.contains(seg))
                    }
                    _ => true,
                });
            }
        }
    }

    /// Root deps this build didn't link, and the scope that would have.
    pub fn unlinked_scopes(&self) -> &HashMap<String, String> {
        &self.unlinked_scopes
    }

    /// Also link the root's deps in this scope — `"dev"` wherever `test`
    /// blocks are compiled, `"build"` for the build script
    /// (`struct.build/D4`).
    pub fn include_scope(&mut self, scope: &str) -> &mut Self {
        if !self.root_scopes.iter().any(|s| s == scope) {
            self.root_scopes.push(scope.to_string());
        }
        self
    }

    /// Discover a package from a directory path.
    ///
    /// Recursively discovers nested sub-packages and path dependencies.
    pub fn discover(&mut self, root: &Path) -> Result<PackageId, PackageError> {
        self.discover_with_path(root, vec![])
    }

    /// Discover a workspace root, returning IDs for all member packages (WS1-WS3).
    ///
    /// If the root rask.toml has `members = [...]`, discovers each member as a
    /// separate root package. Otherwise falls back to regular `discover()`.
    /// All members share a single lock file (WS2) and can reference each
    /// other via path dependencies (WS3).
    pub fn discover_workspace(&mut self, root: &Path) -> Result<Vec<PackageId>, PackageError> {
        let manifest_path = root.join(MANIFEST_FILE);
        if !manifest_path.is_file() {
            let id = self.discover(root)?;
            return Ok(vec![id]);
        }

        let manifest = Some(load_manifest(&manifest_path)?);
        let members = manifest.as_ref()
            .and_then(|m| m.members())
            .cloned()
            .unwrap_or_default();

        if members.is_empty() {
            let id = self.discover(root)?;
            return Ok(vec![id]);
        }

        let mut ids = Vec::new();
        for member in &members {
            let member_dir = root.join(member);
            if !member_dir.is_dir() {
                return Err(PackageError::Io(
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("workspace member '{}' not found", member),
                    ),
                    member_dir,
                ));
            }
            let id = self.discover(&member_dir)?;
            ids.push(id);
        }

        Ok(ids)
    }

    /// Register a package and return its ID. Handles name_to_id dedup.
    fn register_package(&mut self, mut package: Package, pkg_path: Vec<String>) -> PackageId {
        let id = PackageId(self.packages.len() as u32);
        package.id = id;
        let name = package.name.clone();
        self.packages.push(package);
        self.path_to_id.insert(pkg_path, id);
        if !self.name_to_id.contains_key(&name) {
            self.name_to_id.insert(name, id);
        }
        id
    }

    /// Resolve path dependencies for a package and record them as imports.
    fn resolve_path_deps(
        &mut self,
        pkg_id: PackageId,
        deps: Vec<DepDecl>,
        base_dir: &Path,
    ) -> Result<(), PackageError> {
        for dep in deps {
            if let Some(ref dep_path) = dep.path {
                let dep_dir = base_dir.join(dep_path);
                if dep_dir.is_dir() {
                    let dep_id = self.discover_dep(&dep_dir, &dep.name)?;
                    if let Some(pkg) = self.packages.get_mut(pkg_id.0 as usize) {
                        pkg.imports.push(dep_id);
                    }
                }
            }
        }
        Ok(())
    }

    /// Discover a local package with a given package path prefix.
    fn discover_with_path(
        &mut self,
        dir: &Path,
        path_prefix: Vec<String>,
    ) -> Result<PackageId, PackageError> {
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("main")
            .to_string();

        let mut pkg_path = path_prefix.clone();
        if !pkg_path.is_empty() || dir_name != "." {
            pkg_path.push(dir_name.clone());
        }

        if let Some(&id) = self.path_to_id.get(&pkg_path) {
            return Ok(id);
        }

        // The manifest and the build script, either of which may be missing
        let (manifest, build_decls) = read_package_dir(dir)?;

        // Name from rask.toml or directory (display name only — path stays as directory)
        let pkg_name = manifest.as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| pkg_path.last().cloned().unwrap_or_else(|| "main".to_string()));

        let (files, subdirs) = collect_rk_files(dir)?;
        let source_files =
            parse_rk_files(&mut self.next_node_id, &mut self.next_file_id, files)?;
        let mut skipped = HashMap::new();
        let deps = path_deps(&manifest, &self.root_scopes, &mut skipped);
        self.unlinked_scopes.extend(skipped);

        let package = Package {
            id: PackageId(0), // placeholder, set by register_package
            name: pkg_name,
            path: pkg_path.clone(),
            root_dir: dir.to_path_buf(),
            files: source_files,
            imports: Vec::new(),
            manifest,
            build_decls,
            is_external: false,
            registry_source: None,
        };

        let id = self.register_package(package, pkg_path.clone());

        // Recurse into sub-packages
        for subdir in subdirs {
            self.discover_with_path(&subdir, pkg_path.clone())?;
        }

        // Discover path dependencies (struct.build/D2)
        self.resolve_path_deps(id, deps, dir)?;

        Ok(id)
    }

    /// Discover an external dependency from a path.
    fn discover_dep(
        &mut self,
        dir: &Path,
        dep_name: &str,
    ) -> Result<PackageId, PackageError> {
        // Check if already discovered under either the dep name or the manifest name
        if let Some(&id) = self.name_to_id.get(dep_name) {
            return Ok(id);
        }

        let canonical = dir.canonicalize()
            .unwrap_or_else(|_| dir.to_path_buf());

        // Cycle detection
        if !self.discovering.insert(canonical.clone()) {
            return Err(PackageError::CircularDependency(
                self.discovering.iter().map(|p| p.display().to_string()).collect()
            ));
        }

        // The manifest and the build script, either of which may be missing
        let (manifest, build_decls) = read_package_dir(&canonical)?;

        let pkg_name = manifest.as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| dep_name.to_string());

        // Check again with the resolved name (build.rk may declare a different name)
        if pkg_name != dep_name {
            if let Some(&id) = self.name_to_id.get(&pkg_name) {
                self.discovering.remove(&canonical);
                return Ok(id);
            }
        }

        let (files, _subdirs) = collect_rk_files(&canonical)?;
        let source_files =
            parse_rk_files(&mut self.next_node_id, &mut self.next_file_id, files)?;
        let deps = path_deps(&manifest, &[], &mut HashMap::new());
        let pkg_path = vec![pkg_name.clone()];

        let package = Package {
            id: PackageId(0),
            name: pkg_name,
            path: pkg_path.clone(),
            root_dir: canonical.clone(),
            files: source_files,
            imports: Vec::new(),
            manifest,
            build_decls,
            is_external: true,
            registry_source: None,
        };

        let id = self.register_package(package, pkg_path);
        self.resolve_path_deps(id, deps, &canonical)?;
        self.discovering.remove(&canonical);

        Ok(id)
    }

    /// Get a package by ID.
    /// The next unused node id, for declarations made outside any package's
    /// files: a C header's translation (struct.c-interop/CI1).
    pub fn next_node_id_mut(&mut self) -> &mut u32 {
        &mut self.next_node_id
    }

    pub fn get(&self, id: PackageId) -> Option<&Package> {
        self.packages.get(id.0 as usize)
    }

    /// Get a mutable package by ID.
    pub fn get_mut(&mut self, id: PackageId) -> Option<&mut Package> {
        self.packages.get_mut(id.0 as usize)
    }

    /// Look up package by full path (e.g., ["http", "client"]).
    pub fn lookup_path(&self, path: &[String]) -> Option<PackageId> {
        self.path_to_id.get(path).copied()
    }

    /// Look up package by simple name (e.g., "http").
    pub fn lookup_name(&self, name: &str) -> Option<PackageId> {
        self.name_to_id.get(name).copied()
    }

    /// Get all packages.
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// Get the number of packages.
    pub fn len(&self) -> usize {
        self.packages.len()
    }

    /// Check if empty.
    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// Register a package from the local cache (downloaded from a registry).
    ///
    /// Parses .rk files from `cache_dir`, creates a Package with
    /// `is_external: true` and the given `registry_url` as source.
    pub fn register_cached(
        &mut self,
        name: &str,
        version: &str,
        cache_dir: &Path,
        registry_url: &str,
    ) -> Result<PackageId, PackageError> {
        // Already registered?
        if let Some(&id) = self.name_to_id.get(name) {
            return Ok(id);
        }

        // The manifest and the build script, either of which may be missing
        let (manifest, build_decls) = read_package_dir(cache_dir)?;

        let pkg_name = manifest.as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| name.to_string());

        let (files, _subdirs) = collect_rk_files(cache_dir)?;
        let source_files =
            parse_rk_files(&mut self.next_node_id, &mut self.next_file_id, files)?;
        let pkg_path = vec![pkg_name.clone()];

        // Build a manifest with the correct version if the package has none
        let manifest = manifest.or_else(|| {
            Some(rask_ast::decl::PackageDecl {
                name: pkg_name.clone(),
                version: version.to_string(),
                deps: Vec::new(),
                features: Vec::new(),
                metadata: Vec::new(),
                list_metadata: Vec::new(),
                profiles: Vec::new(),
                claims: Vec::new(),
            })
        });

        let package = Package {
            id: PackageId(0),
            name: pkg_name,
            path: pkg_path.clone(),
            root_dir: cache_dir.to_path_buf(),
            files: source_files,
            imports: Vec::new(),
            manifest,
            build_decls,
            is_external: true,
            registry_source: Some(registry_url.to_string()),
        };

        Ok(self.register_package(package, pkg_path))
    }

    /// Add a package manually (for testing).
    /// Register a package whose sources the caller holds in memory, importing
    /// `imports` (packages already registered). How the toolchain checks Rask
    /// it ships inside its own binary, like the manifest loader and the
    /// schema library under it. An external package gets its declarations
    /// qualified, as a fetched dependency does.
    pub fn add_source_package(
        &mut self,
        name: &str,
        sources: Vec<(PathBuf, String)>,
        imports: Vec<PackageId>,
        is_external: bool,
    ) -> Result<PackageId, PackageError> {
        let files = parse_rk_sources(&mut self.next_node_id, &mut self.next_file_id, sources)?;
        let package = Package {
            id: PackageId(0),
            name: name.to_string(),
            path: vec![name.to_string()],
            root_dir: PathBuf::from(format!("<{name}>")),
            files,
            imports,
            manifest: None,
            build_decls: Vec::new(),
            is_external,
            registry_source: None,
        };
        Ok(self.register_package(package, vec![name.to_string()]))
    }

    #[cfg(test)]
    pub fn add_package(&mut self, name: String, path: Vec<String>, root_dir: std::path::PathBuf) -> PackageId {
        let package = Package {
            id: PackageId(0),
            name: name.clone(),
            path: path.clone(),
            root_dir,
            files: Vec::new(),
            imports: Vec::new(),
            manifest: None,
            build_decls: Vec::new(),
            is_external: false,
            registry_source: None,
        };
        self.register_package(package, path)
    }

    #[cfg(test)]
    pub fn add_package_with_decls(
        &mut self,
        name: String,
        path: Vec<String>,
        root_dir: std::path::PathBuf,
        decls: Vec<Decl>,
    ) -> PackageId {
        let package = Package {
            id: PackageId(0),
            name: name.clone(),
            path: path.clone(),
            root_dir: root_dir.clone(),
            files: vec![SourceFile {
                path: root_dir.join("lib.rk"),
                source: String::new(),
                decls,
                file_id: {
                    let id = self.next_file_id;
                    self.next_file_id += 1;
                    id
                },
            }],
            imports: Vec::new(),
            manifest: None,
            build_decls: Vec::new(),
            is_external: false,
            registry_source: None,
        };
        self.register_package(package, path)
    }

    /// Get all declarations from a package (flattened from all files).
    pub fn all_decls(&self, id: PackageId) -> Vec<&Decl> {
        self.get(id)
            .map(|pkg| pkg.files.iter().flat_map(|f| &f.decls).collect())
            .unwrap_or_default()
    }
}

impl Package {
    /// Get all declarations in this package (from all files).
    pub fn all_decls(&self) -> impl Iterator<Item = &Decl> {
        self.files.iter().flat_map(|f| &f.decls)
    }

    /// Get the full package path as a dot-separated string.
    pub fn path_string(&self) -> String {
        self.path.join(".")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_package(dir: &Path, files: &[(&str, &str)]) {
        fs::create_dir_all(dir).unwrap();
        for (name, content) in files {
            let path = dir.join(name);
            let mut file = File::create(path).unwrap();
            file.write_all(content.as_bytes()).unwrap();
        }
    }

    #[test]
    fn test_discover_single_package() {
        let tmp = TempDir::new().unwrap();
        let pkg_dir = tmp.path().join("mypackage");

        create_test_package(&pkg_dir, &[
            ("main.rk", "func main() { }"),
            ("util.rk", "func helper() { }"),
        ]);

        let mut registry = PackageRegistry::new();
        let id = registry.discover(&pkg_dir).unwrap();

        let pkg = registry.get(id).unwrap();
        assert_eq!(pkg.name, "mypackage");
        assert_eq!(pkg.files.len(), 2);
    }

    #[test]
    fn test_discover_nested_packages() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("project");
        let sub = root.join("sub");

        create_test_package(&root, &[
            ("main.rk", "func main() { }"),
        ]);
        create_test_package(&sub, &[
            ("helper.rk", "func help() { }"),
        ]);

        let mut registry = PackageRegistry::new();
        registry.discover(&root).unwrap();

        assert_eq!(registry.len(), 2);

        // Check root package
        let root_id = registry.lookup_name("project").unwrap();
        let root_pkg = registry.get(root_id).unwrap();
        assert_eq!(root_pkg.path, vec!["project"]);

        // Check sub package
        let sub_id = registry.lookup_path(&["project".to_string(), "sub".to_string()]).unwrap();
        let sub_pkg = registry.get(sub_id).unwrap();
        assert_eq!(sub_pkg.path, vec!["project", "sub"]);
    }

    /// Reading rask.toml takes the loader the binary installs
    /// (rask_manifest::install). Without it the error says so instead of
    /// treating the package as having no manifest. Discovery through the
    /// real loader is tested in rask-manifest.
    #[test]
    fn a_manifest_without_the_loader_is_an_error() {
        let tmp = TempDir::new().unwrap();
        let pkg_dir = tmp.path().join("mydir");
        create_test_package(&pkg_dir, &[
            ("rask.toml", "schema = \"rask-package@1\"\n"),
            ("main.rk", "func main() { }"),
        ]);
        let err = PackageRegistry::new().discover(&pkg_dir).unwrap_err();
        assert!(err.to_string().contains("manifest loader"), "{err}");
    }

    #[test]
    fn test_build_dir_skipped() {
        let tmp = TempDir::new().unwrap();
        let pkg_dir = tmp.path().join("mypackage");

        create_test_package(&pkg_dir, &[
            ("main.rk", "func main() { }"),
        ]);
        // Create build output directory — should be skipped
        fs::create_dir_all(pkg_dir.join("build").join("debug")).unwrap();

        let mut registry = PackageRegistry::new();
        registry.discover(&pkg_dir).unwrap();

        // Only the root package, no build/ sub-packages
        assert_eq!(registry.len(), 1);
    }
}
