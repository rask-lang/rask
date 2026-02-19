// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Remote package registry client — struct.packages/RG1-RG4.
//!
//! Fetches package metadata and archives from a static registry
//! (e.g., GitHub Pages). The registry serves JSON indexes and
//! tarballs at well-known paths.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use serde::Deserialize;

/// Default registry URL.
const DEFAULT_REGISTRY: &str = "https://packages.rask-lang.dev";

/// Registry configuration.
#[derive(Debug, Clone)]
pub struct RegistryConfig {
    pub url: String,
}

/// Metadata for a single package version from the registry index.
#[derive(Debug, Clone, Deserialize)]
pub struct VersionMeta {
    pub checksum: String,
    #[serde(default)]
    pub deps: Vec<RegistryDep>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub yanked: bool,
}

/// A dependency entry from the registry index.
#[derive(Debug, Clone, Deserialize)]
pub struct RegistryDep {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub allow: Vec<String>,
}

/// The full index for a package.
#[derive(Debug, Clone, Deserialize)]
pub struct PackageIndex {
    pub name: String,
    pub versions: BTreeMap<String, VersionMeta>,
}

/// The root index listing all available packages.
#[derive(Debug, Clone, Deserialize)]
pub struct RootIndex {
    pub packages: Vec<String>,
}

/// Errors from registry operations.
#[derive(Debug)]
pub enum RegistryError {
    /// HTTP request failed.
    Http(String),
    /// Package not found (404).
    NotFound(String),
    /// JSON parse error.
    ParseError(String),
    /// I/O error writing to disk.
    Io(std::io::Error),
}

impl std::fmt::Display for RegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RegistryError::Http(msg) => write!(f, "registry HTTP error: {}", msg),
            RegistryError::NotFound(pkg) => write!(f, "package '{}' not found in registry", pkg),
            RegistryError::ParseError(msg) => write!(f, "registry parse error: {}", msg),
            RegistryError::Io(err) => write!(f, "registry I/O error: {}", err),
        }
    }
}

impl std::error::Error for RegistryError {}

impl From<std::io::Error> for RegistryError {
    fn from(err: std::io::Error) -> Self {
        RegistryError::Io(err)
    }
}

impl RegistryConfig {
    /// Load registry config from environment or use default.
    ///
    /// Reads `RASK_REGISTRY` env var, falls back to the default registry URL.
    pub fn from_env() -> Self {
        let url = std::env::var("RASK_REGISTRY")
            .unwrap_or_else(|_| DEFAULT_REGISTRY.to_string());
        // Strip trailing slash for consistent URL joining
        let url = url.trim_end_matches('/').to_string();
        RegistryConfig { url }
    }

    /// Create with an explicit URL (for testing).
    pub fn new(url: &str) -> Self {
        RegistryConfig {
            url: url.trim_end_matches('/').to_string(),
        }
    }

    /// Fetch the package index for a given package name.
    ///
    /// GET {url}/pkg/{name}/index.json
    pub fn fetch_index(&self, name: &str) -> Result<PackageIndex, RegistryError> {
        let url = format!("{}/pkg/{}/index.json", self.url, name);
        let response = reqwest::blocking::get(&url)
            .map_err(|e| RegistryError::Http(format!("{}: {}", url, e)))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RegistryError::NotFound(name.to_string()));
        }

        if !response.status().is_success() {
            return Err(RegistryError::Http(format!(
                "{}: HTTP {}", url, response.status()
            )));
        }

        let body = response.text()
            .map_err(|e| RegistryError::Http(format!("reading response: {}", e)))?;

        serde_json::from_str(&body)
            .map_err(|e| RegistryError::ParseError(format!("{}: {}", url, e)))
    }

    /// Download a package archive to a destination path.
    ///
    /// GET {url}/pkg/{name}/{version}.tar.gz
    pub fn download_archive(
        &self,
        name: &str,
        version: &str,
        dest: &Path,
    ) -> Result<(), RegistryError> {
        let url = format!("{}/pkg/{}/{}.tar.gz", self.url, name, version);
        let response = reqwest::blocking::get(&url)
            .map_err(|e| RegistryError::Http(format!("{}: {}", url, e)))?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(RegistryError::NotFound(
                format!("{} {}", name, version)
            ));
        }

        if !response.status().is_success() {
            return Err(RegistryError::Http(format!(
                "{}: HTTP {}", url, response.status()
            )));
        }

        let bytes = response.bytes()
            .map_err(|e| RegistryError::Http(format!("reading archive: {}", e)))?;

        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::File::create(dest)?;
        file.write_all(&bytes)?;

        Ok(())
    }

    /// Fetch the root index listing all packages.
    ///
    /// GET {url}/index.json
    pub fn fetch_root_index(&self) -> Result<RootIndex, RegistryError> {
        let url = format!("{}/index.json", self.url);
        let response = reqwest::blocking::get(&url)
            .map_err(|e| RegistryError::Http(format!("{}: {}", url, e)))?;

        if !response.status().is_success() {
            return Err(RegistryError::Http(format!(
                "{}: HTTP {}", url, response.status()
            )));
        }

        let body = response.text()
            .map_err(|e| RegistryError::Http(format!("reading response: {}", e)))?;

        serde_json::from_str(&body)
            .map_err(|e| RegistryError::ParseError(format!("{}: {}", url, e)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_package_index() {
        let json = r#"{
            "name": "http",
            "versions": {
                "2.0.0": {
                    "checksum": "sha256:abc123",
                    "deps": [
                        {"name": "net", "version": "^1.0", "allow": ["ffi"]}
                    ],
                    "capabilities": ["net"],
                    "yanked": false
                },
                "2.1.0": {
                    "checksum": "sha256:def456",
                    "deps": [
                        {"name": "net", "version": "^1.2"}
                    ],
                    "capabilities": ["net"],
                    "yanked": false
                }
            }
        }"#;

        let index: PackageIndex = serde_json::from_str(json).unwrap();
        assert_eq!(index.name, "http");
        assert_eq!(index.versions.len(), 2);

        let v200 = &index.versions["2.0.0"];
        assert_eq!(v200.checksum, "sha256:abc123");
        assert_eq!(v200.deps.len(), 1);
        assert_eq!(v200.deps[0].name, "net");
        assert_eq!(v200.deps[0].version, "^1.0");
        assert_eq!(v200.deps[0].allow, vec!["ffi"]);
        assert_eq!(v200.capabilities, vec!["net"]);
        assert!(!v200.yanked);

        let v210 = &index.versions["2.1.0"];
        assert_eq!(v210.checksum, "sha256:def456");
        assert!(v210.deps[0].allow.is_empty());
    }

    #[test]
    fn test_parse_package_index_minimal() {
        let json = r#"{
            "name": "hello",
            "versions": {
                "1.0.0": {
                    "checksum": "sha256:aaa"
                }
            }
        }"#;

        let index: PackageIndex = serde_json::from_str(json).unwrap();
        assert_eq!(index.name, "hello");
        let v = &index.versions["1.0.0"];
        assert!(v.deps.is_empty());
        assert!(v.capabilities.is_empty());
        assert!(!v.yanked);
    }

    #[test]
    fn test_parse_yanked_version() {
        let json = r#"{
            "name": "old",
            "versions": {
                "0.1.0": {
                    "checksum": "sha256:xxx",
                    "yanked": true
                }
            }
        }"#;

        let index: PackageIndex = serde_json::from_str(json).unwrap();
        assert!(index.versions["0.1.0"].yanked);
    }

    #[test]
    fn test_parse_root_index() {
        let json = r#"{"packages": ["http", "json", "net"]}"#;
        let root: RootIndex = serde_json::from_str(json).unwrap();
        assert_eq!(root.packages, vec!["http", "json", "net"]);
    }

    #[test]
    fn test_registry_config_default() {
        // Don't set env var — just test the struct
        let config = RegistryConfig::new("https://example.com/");
        assert_eq!(config.url, "https://example.com");
    }

    #[test]
    fn test_registry_config_strips_trailing_slash() {
        let config = RegistryConfig::new("https://example.com///");
        assert_eq!(config.url, "https://example.com");
    }
}
