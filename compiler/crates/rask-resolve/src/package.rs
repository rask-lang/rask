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
    /// Package name (from build.rk or directory name).
    pub name: String,
    /// Full package path (e.g., ["pkg", "sub"] for `pkg.sub`).
    pub path: Vec<String>,
    /// Root directory of the package.
    pub root_dir: PathBuf,
    /// Source files in this package.
    pub files: Vec<SourceFile>,
    /// Packages this package imports (populated during resolution).
    pub imports: Vec<PackageId>,
    /// Package metadata from build.rk (if present).
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
}

/// Error that can occur during package discovery.
#[derive(Debug)]
pub enum PackageError {
    /// I/O error while reading files or directories.
    Io(std::io::Error, PathBuf),
    /// Parse error in a source file.
    Parse {
        file: PathBuf,
        errors: Vec<String>,
    },
    /// Lex error in a source file.
    Lex {
        file: PathBuf,
        errors: Vec<String>,
    },
    /// Circular dependency between packages.
    CircularDependency(Vec<String>),
    /// Package not found.
    NotFound(Vec<String>),
    /// No .rk files found in directory.
    EmptyPackage(PathBuf),
}

impl std::fmt::Display for PackageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PackageError::Io(err, path) => {
                write!(f, "I/O error at {}: {}", path.display(), err)
            }
            PackageError::Parse { file, errors } => {
                write!(f, "Parse errors in {}:\n{}", file.display(), errors.join("\n"))
            }
            PackageError::Lex { file, errors } => {
                write!(f, "Lex errors in {}:\n{}", file.display(), errors.join("\n"))
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
        }
    }
}

impl std::error::Error for PackageError {}

// =========================================================================
// Shared helpers (no &self — pure functions on paths)
// =========================================================================

/// Collect .rk source file paths from a directory (excluding build.rk).
/// Also includes any .rk files from the .rk-gen/ subdirectory.
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
                if ext == "rk" && path.file_name().and_then(|n| n.to_str()) != Some("build.rk") {
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
/// Chains NodeIds across files to ensure uniqueness when decls are combined.
fn parse_rk_files(paths: Vec<PathBuf>) -> Result<Vec<SourceFile>, PackageError> {
    let mut source_files = Vec::new();
    let mut next_id: u32 = 0;
    for file_path in paths {
        let source = fs::read_to_string(&file_path)
            .map_err(|e| PackageError::Io(e, file_path.clone()))?;

        let mut lexer = rask_lexer::Lexer::new(&source);
        let lex_result = lexer.tokenize();
        if !lex_result.is_ok() {
            return Err(PackageError::Lex {
                file: file_path,
                errors: lex_result.errors.iter().map(|e| e.to_string()).collect(),
            });
        }

        let mut parser = rask_parser::Parser::new_with_start_id(lex_result.tokens, next_id);
        let parse_result = parser.parse();
        next_id = parser.next_node_id();
        if !parse_result.is_ok() {
            return Err(PackageError::Parse {
                file: file_path,
                errors: parse_result.errors.iter().map(|e| e.to_string()).collect(),
            });
        }

        source_files.push(SourceFile {
            path: file_path,
            source,
            decls: parse_result.decls,
        });
    }
    Ok(source_files)
}

/// Parse build.rk and extract the package block (PK3: independent parsing).
fn parse_build_rk(path: &Path) -> Result<(Option<PackageDecl>, Vec<Decl>), PackageError> {
    let source = fs::read_to_string(path)
        .map_err(|e| PackageError::Io(e, path.to_path_buf()))?;

    let mut lexer = rask_lexer::Lexer::new(&source);
    let lex_result = lexer.tokenize();
    if !lex_result.is_ok() {
        return Err(PackageError::Lex {
            file: path.to_path_buf(),
            errors: lex_result.errors.iter().map(|e| e.to_string()).collect(),
        });
    }

    let mut parser = rask_parser::Parser::new(lex_result.tokens);
    let parse_result = parser.parse();
    if !parse_result.is_ok() {
        return Err(PackageError::Parse {
            file: path.to_path_buf(),
            errors: parse_result.errors.iter().map(|e| e.to_string()).collect(),
        });
    }

    let mut manifest = None;
    for decl in &parse_result.decls {
        if let DeclKind::Package(pkg) = &decl.kind {
            manifest = Some(pkg.clone());
            break;
        }
    }

    Ok((manifest, parse_result.decls))
}

/// Extract path dependencies from a manifest.
fn path_deps(manifest: &Option<PackageDecl>) -> Vec<DepDecl> {
    manifest.as_ref()
        .map(|m| m.deps.iter().filter(|d| d.path.is_some()).cloned().collect())
        .unwrap_or_default()
}

// =========================================================================
// PackageRegistry
// =========================================================================

impl PackageRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Discover a package from a directory path.
    ///
    /// Recursively discovers nested sub-packages and path dependencies.
    pub fn discover(&mut self, root: &Path) -> Result<PackageId, PackageError> {
        self.discover_with_path(root, vec![])
    }

    /// Discover a workspace root, returning IDs for all member packages (WS1-WS3).
    ///
    /// If the root build.rk has `members: [...]`, discovers each member as a
    /// separate root package. Otherwise falls back to regular `discover()`.
    /// All members share a single lock file (WS2) and can reference each
    /// other via path dependencies (WS3).
    pub fn discover_workspace(&mut self, root: &Path) -> Result<Vec<PackageId>, PackageError> {
        let build_rk = root.join("build.rk");
        if !build_rk.is_file() {
            let id = self.discover(root)?;
            return Ok(vec![id]);
        }

        let (manifest, _) = parse_build_rk(&build_rk)?;
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

        // Parse build.rk if present (PK1)
        let build_rk_path = dir.join("build.rk");
        let (manifest, build_decls) = if build_rk_path.is_file() {
            parse_build_rk(&build_rk_path)?
        } else {
            (None, Vec::new())
        };

        // Name from build.rk or directory (display name only — path stays as directory)
        let pkg_name = manifest.as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| pkg_path.last().cloned().unwrap_or_else(|| "main".to_string()));

        let (files, subdirs) = collect_rk_files(dir)?;
        let source_files = parse_rk_files(files)?;
        let deps = path_deps(&manifest);

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

        // Parse build.rk if present
        let build_rk_path = canonical.join("build.rk");
        let (manifest, build_decls) = if build_rk_path.is_file() {
            parse_build_rk(&build_rk_path)?
        } else {
            (None, Vec::new())
        };

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
        let source_files = parse_rk_files(files)?;
        let deps = path_deps(&manifest);
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

        // Parse build.rk if present
        let build_rk_path = cache_dir.join("build.rk");
        let (manifest, build_decls) = if build_rk_path.is_file() {
            parse_build_rk(&build_rk_path)?
        } else {
            (None, Vec::new())
        };

        let pkg_name = manifest.as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| name.to_string());

        let (files, _subdirs) = collect_rk_files(cache_dir)?;
        let source_files = parse_rk_files(files)?;
        let pkg_path = vec![pkg_name.clone()];

        // Build a manifest with the correct version if build.rk doesn't have one
        let manifest = manifest.or_else(|| {
            Some(rask_ast::decl::PackageDecl {
                name: pkg_name.clone(),
                version: version.to_string(),
                deps: Vec::new(),
                features: Vec::new(),
                metadata: Vec::new(),
                list_metadata: Vec::new(),
                profiles: Vec::new(),
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
            files: vec![SourceFile { path: root_dir.join("lib.rk"), source: String::new(), decls }],
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

    #[test]
    fn test_discover_with_build_rk() {
        let tmp = TempDir::new().unwrap();
        let pkg_dir = tmp.path().join("mydir");

        create_test_package(&pkg_dir, &[
            ("build.rk", "package \"myapp\" \"1.0.0\" { }"),
            ("main.rk", "func main() { }"),
        ]);

        let mut registry = PackageRegistry::new();
        let id = registry.discover(&pkg_dir).unwrap();

        let pkg = registry.get(id).unwrap();
        assert_eq!(pkg.name, "myapp");
        assert!(pkg.manifest.is_some());
        assert_eq!(pkg.manifest.as_ref().unwrap().version, "1.0.0");
        // build.rk excluded from source files
        assert_eq!(pkg.files.len(), 1);
    }

    #[test]
    fn test_discover_path_dep() {
        let tmp = TempDir::new().unwrap();
        let app_dir = tmp.path().join("app");
        let lib_dir = tmp.path().join("shared");

        create_test_package(&app_dir, &[
            ("build.rk", "package \"app\" \"1.0.0\" {\n    dep \"shared\" { path: \"../shared\" }\n}"),
            ("main.rk", "func main() { }"),
        ]);
        create_test_package(&lib_dir, &[
            ("lib.rk", "public func greet() { }"),
        ]);

        let mut registry = PackageRegistry::new();
        let id = registry.discover(&app_dir).unwrap();

        let pkg = registry.get(id).unwrap();
        assert_eq!(pkg.imports.len(), 1);

        let dep_id = pkg.imports[0];
        let dep_pkg = registry.get(dep_id).unwrap();
        assert_eq!(dep_pkg.name, "shared");
        assert!(dep_pkg.is_external);
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
