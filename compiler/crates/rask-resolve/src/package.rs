// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Package discovery and management.
//!
//! A package in Rask is a directory containing `.rk` source files.
//! All files in a directory form one package. Nested directories
//! are separate packages (e.g., `pkg/sub/` is package `pkg.sub`).

use std::collections::HashMap;
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
}

/// A source file within a package.
#[derive(Debug)]
pub struct SourceFile {
    /// Path to the source file.
    pub path: PathBuf,
    /// Parsed declarations from this file.
    pub decls: Vec<Decl>,
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

impl PackageRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Discover a package from a directory path.
    ///
    /// This recursively discovers all nested packages as well.
    pub fn discover(&mut self, root: &Path) -> Result<PackageId, PackageError> {
        self.discover_with_path(root, vec![])
    }

    /// Discover a package with a given package path prefix.
    fn discover_with_path(
        &mut self,
        dir: &Path,
        path_prefix: Vec<String>,
    ) -> Result<PackageId, PackageError> {
        // Get directory name for package name
        let dir_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("main")
            .to_string();

        // Build full package path
        let mut pkg_path = path_prefix.clone();
        if !pkg_path.is_empty() || dir_name != "." {
            pkg_path.push(dir_name.clone());
        }

        // Check if already discovered
        if let Some(&id) = self.path_to_id.get(&pkg_path) {
            return Ok(id);
        }

        // Check for build.rk (PK1: single file, package root)
        let build_rk_path = dir.join("build.rk");
        let (manifest, build_decls) = if build_rk_path.is_file() {
            self.parse_build_rk(&build_rk_path)?
        } else {
            (None, Vec::new())
        };

        // Use package name from build.rk if available (PK4: optional)
        let pkg_name = manifest.as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| pkg_path.last().cloned().unwrap_or_else(|| "main".to_string()));

        // If build.rk provides a name, update the path to use it
        if manifest.is_some() && pkg_path.last().map(|s| s.as_str()) != Some(&pkg_name) {
            if let Some(last) = pkg_path.last_mut() {
                *last = pkg_name.clone();
            }
        }

        // Find all .rk files in this directory (excluding build.rk)
        let mut files = Vec::new();
        let mut subdirs = Vec::new();

        let entries = fs::read_dir(dir)
            .map_err(|e| PackageError::Io(e, dir.to_path_buf()))?;

        for entry in entries {
            let entry = entry.map_err(|e| PackageError::Io(e, dir.to_path_buf()))?;
            let path = entry.path();

            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "rk" {
                        // Skip build.rk — it's compiled separately (BL1)
                        if path.file_name().and_then(|n| n.to_str()) == Some("build.rk") {
                            continue;
                        }
                        files.push(path);
                    }
                }
            } else if path.is_dir() {
                // Skip hidden dirs, output dir, and underscore-prefixed dirs
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

        // Sort for deterministic ordering
        files.sort();
        subdirs.sort();

        // Parse all source files
        let mut source_files = Vec::new();
        for file_path in files {
            let source = fs::read_to_string(&file_path)
                .map_err(|e| PackageError::Io(e, file_path.clone()))?;

            // Lex
            let mut lexer = rask_lexer::Lexer::new(&source);
            let lex_result = lexer.tokenize();
            if !lex_result.is_ok() {
                return Err(PackageError::Lex {
                    file: file_path,
                    errors: lex_result.errors.iter().map(|e| e.to_string()).collect(),
                });
            }

            // Parse
            let mut parser = rask_parser::Parser::new(lex_result.tokens);
            let parse_result = parser.parse();
            if !parse_result.is_ok() {
                return Err(PackageError::Parse {
                    file: file_path,
                    errors: parse_result.errors.iter().map(|e| e.to_string()).collect(),
                });
            }

            source_files.push(SourceFile {
                path: file_path,
                decls: parse_result.decls,
            });
        }

        // Collect path deps before creating the package (need to resolve after registration)
        let path_deps: Vec<DepDecl> = manifest.as_ref()
            .map(|m| m.deps.iter().filter(|d| d.path.is_some()).cloned().collect())
            .unwrap_or_default();

        // Create the package
        let id = PackageId(self.packages.len() as u32);

        let package = Package {
            id,
            name: pkg_name.clone(),
            path: pkg_path.clone(),
            root_dir: dir.to_path_buf(),
            files: source_files,
            imports: Vec::new(),
            manifest,
            build_decls,
            is_external: false,
        };

        // Register the package
        self.packages.push(package);
        self.path_to_id.insert(pkg_path.clone(), id);

        // Only register by simple name if not ambiguous
        if !self.name_to_id.contains_key(&pkg_name) {
            self.name_to_id.insert(pkg_name, id);
        }

        // Recursively discover sub-packages
        for subdir in subdirs {
            self.discover_with_path(&subdir, pkg_path.clone())?;
        }

        // Discover path dependencies (struct.build/D2)
        for dep in path_deps {
            if let Some(dep_path) = &dep.path {
                let dep_dir = dir.join(dep_path);
                if dep_dir.is_dir() {
                    let dep_id = self.discover_dep(&dep_dir, &dep.name)?;
                    if let Some(pkg) = self.packages.get_mut(id.0 as usize) {
                        pkg.imports.push(dep_id);
                    }
                }
            }
        }

        Ok(id)
    }

    /// Parse build.rk and extract the package block (PK3: independent parsing).
    fn parse_build_rk(&self, path: &Path) -> Result<(Option<PackageDecl>, Vec<Decl>), PackageError> {
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

        // Extract the package block from declarations
        let mut manifest = None;
        for decl in &parse_result.decls {
            if let DeclKind::Package(pkg) = &decl.kind {
                manifest = Some(pkg.clone());
                break;
            }
        }

        Ok((manifest, parse_result.decls))
    }

    /// Discover an external dependency from a path.
    fn discover_dep(
        &mut self,
        dir: &Path,
        dep_name: &str,
    ) -> Result<PackageId, PackageError> {
        // Check if already discovered by name
        if let Some(&id) = self.name_to_id.get(dep_name) {
            return Ok(id);
        }

        let canonical = dir.canonicalize()
            .unwrap_or_else(|_| dir.to_path_buf());

        // Parse build.rk if present
        let build_rk_path = canonical.join("build.rk");
        let (manifest, build_decls) = if build_rk_path.is_file() {
            self.parse_build_rk(&build_rk_path)?
        } else {
            (None, Vec::new())
        };

        let pkg_name = manifest.as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| dep_name.to_string());

        // Find source files (excluding build.rk)
        let mut files = Vec::new();
        if let Ok(entries) = fs::read_dir(&canonical) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file()
                    && path.extension().map(|e| e == "rk").unwrap_or(false)
                    && path.file_name().and_then(|n| n.to_str()) != Some("build.rk")
                {
                    files.push(path);
                }
            }
        }

        // Include .rk-gen/ files
        let gen_dir = canonical.join(".rk-gen");
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

        let mut source_files = Vec::new();
        for file_path in files {
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

            let mut parser = rask_parser::Parser::new(lex_result.tokens);
            let parse_result = parser.parse();
            if !parse_result.is_ok() {
                return Err(PackageError::Parse {
                    file: file_path,
                    errors: parse_result.errors.iter().map(|e| e.to_string()).collect(),
                });
            }

            source_files.push(SourceFile {
                path: file_path,
                decls: parse_result.decls,
            });
        }

        // Collect path deps for recursive discovery
        let path_deps: Vec<DepDecl> = manifest.as_ref()
            .map(|m| m.deps.iter().filter(|d| d.path.is_some()).cloned().collect())
            .unwrap_or_default();

        let id = PackageId(self.packages.len() as u32);
        let pkg_path = vec![pkg_name.clone()];

        let package = Package {
            id,
            name: pkg_name.clone(),
            path: pkg_path.clone(),
            root_dir: canonical.clone(),
            files: source_files,
            imports: Vec::new(),
            manifest,
            build_decls,
            is_external: true,
        };

        self.packages.push(package);
        self.path_to_id.insert(pkg_path, id);
        if !self.name_to_id.contains_key(&pkg_name) {
            self.name_to_id.insert(pkg_name, id);
        }

        // Recursively discover path deps of this dependency
        for dep in path_deps {
            if let Some(dep_path) = &dep.path {
                let dep_dir = canonical.join(dep_path);
                if dep_dir.is_dir() {
                    let dep_id = self.discover_dep(&dep_dir, &dep.name)?;
                    if let Some(pkg) = self.packages.get_mut(id.0 as usize) {
                        pkg.imports.push(dep_id);
                    }
                }
            }
        }

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

    /// Add a package manually (for testing).
    #[cfg(test)]
    pub fn add_package(&mut self, name: String, path: Vec<String>, root_dir: std::path::PathBuf) -> PackageId {
        let id = PackageId(self.packages.len() as u32);
        let package = Package {
            id,
            name: name.clone(),
            path: path.clone(),
            root_dir,
            files: Vec::new(),
            imports: Vec::new(),
            manifest: None,
            build_decls: Vec::new(),
            is_external: false,
        };
        self.packages.push(package);
        self.path_to_id.insert(path, id);
        self.name_to_id.insert(name, id);
        id
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
}
