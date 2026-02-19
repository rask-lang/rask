// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Package cache — struct.packages/CA1-CA3.
//!
//! Manages downloaded registry packages at `~/.rask/cache/pkg/`.
//! Archives are extracted and verified by SHA-256 checksum before use.

use std::path::{Path, PathBuf};

use crate::lockfile::compute_checksum;

/// Package cache manager.
#[derive(Debug)]
pub struct PackageCache {
    /// Root cache directory (e.g., `~/.rask/cache/pkg/`).
    pub root: PathBuf,
}

/// Errors from cache operations.
#[derive(Debug)]
pub enum CacheError {
    Io(std::io::Error),
    ChecksumMismatch {
        expected: String,
        actual: String,
    },
    Extract(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::Io(err) => write!(f, "cache I/O error: {}", err),
            CacheError::ChecksumMismatch { expected, actual } => {
                write!(f, "checksum mismatch: expected {}, got {}", expected, actual)
            }
            CacheError::Extract(msg) => write!(f, "archive extraction failed: {}", msg),
        }
    }
}

impl std::error::Error for CacheError {}

impl From<std::io::Error> for CacheError {
    fn from(err: std::io::Error) -> Self {
        CacheError::Io(err)
    }
}

impl PackageCache {
    /// Initialize cache, respecting `RASK_CACHE` env override.
    ///
    /// Default location: `~/.rask/cache/pkg/`
    pub fn new() -> Self {
        let root = if let Ok(cache_dir) = std::env::var("RASK_CACHE") {
            PathBuf::from(cache_dir).join("pkg")
        } else if let Some(home) = home_dir() {
            home.join(".rask").join("cache").join("pkg")
        } else {
            PathBuf::from(".rask").join("cache").join("pkg")
        };
        PackageCache { root }
    }

    /// Create with an explicit root directory (for testing).
    pub fn with_root(root: PathBuf) -> Self {
        PackageCache { root }
    }

    /// Directory path for a cached package version.
    pub fn pkg_dir(&self, name: &str, version: &str) -> PathBuf {
        self.root.join(format!("{}-{}", name, version))
    }

    /// Check if a package version is already cached and verified.
    ///
    /// Returns the cache directory path if the package exists and its
    /// checksum matches `expected_checksum`.
    pub fn get(
        &self,
        name: &str,
        version: &str,
        expected_checksum: &str,
    ) -> Option<PathBuf> {
        let dir = self.pkg_dir(name, version);
        if !dir.is_dir() {
            return None;
        }

        let actual = compute_checksum(&dir);
        if actual == expected_checksum {
            Some(dir)
        } else {
            // Stale cache — checksum mismatch, remove it
            let _ = std::fs::remove_dir_all(&dir);
            None
        }
    }

    /// Store a downloaded archive: extract, verify checksum, return cache path.
    ///
    /// The archive is a `.tar.gz` file containing package source at the root.
    pub fn store(
        &self,
        name: &str,
        version: &str,
        archive: &Path,
        expected_checksum: &str,
    ) -> Result<PathBuf, CacheError> {
        let dest = self.pkg_dir(name, version);

        // Clean up any partial extraction
        if dest.exists() {
            std::fs::remove_dir_all(&dest)?;
        }
        std::fs::create_dir_all(&dest)?;

        // Extract tar.gz
        let file = std::fs::File::open(archive)?;
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&dest)
            .map_err(|e| CacheError::Extract(e.to_string()))?;

        // Verify checksum of extracted contents
        let actual = compute_checksum(&dest);
        if actual != expected_checksum {
            // Remove corrupted extraction
            let _ = std::fs::remove_dir_all(&dest);
            return Err(CacheError::ChecksumMismatch {
                expected: expected_checksum.to_string(),
                actual,
            });
        }

        Ok(dest)
    }

    /// Remove a specific cached package.
    pub fn remove(&self, name: &str, version: &str) -> Result<(), CacheError> {
        let dir = self.pkg_dir(name, version);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }

    /// Clear entire cache.
    pub fn clear(&self) -> Result<(), CacheError> {
        if self.root.exists() {
            std::fs::remove_dir_all(&self.root)?;
        }
        Ok(())
    }
}

/// Get the user's home directory.
fn home_dir() -> Option<PathBuf> {
    std::env::var("HOME").ok().map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_test_tarball(dir: &Path, files: &[(&str, &str)]) -> PathBuf {
        let archive_path = dir.join("test.tar.gz");
        let file = fs::File::create(&archive_path).unwrap();
        let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        let mut builder = tar::Builder::new(encoder);

        for (name, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, name, content.as_bytes()).unwrap();
        }

        builder.into_inner().unwrap().finish().unwrap();
        archive_path
    }

    #[test]
    fn test_cache_store_and_get() {
        let tmp = TempDir::new().unwrap();
        let cache = PackageCache::with_root(tmp.path().join("cache"));

        // Create a tarball with a .rk file
        let tarball = create_test_tarball(tmp.path(), &[
            ("main.rk", "func main() { }"),
        ]);

        // First, extract to compute the expected checksum
        let probe_dir = tmp.path().join("probe");
        fs::create_dir_all(&probe_dir).unwrap();
        let file = fs::File::open(&tarball).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);
        archive.unpack(&probe_dir).unwrap();
        let expected_checksum = compute_checksum(&probe_dir);

        // Store in cache
        let cached_path = cache.store("hello", "1.0.0", &tarball, &expected_checksum).unwrap();
        assert!(cached_path.is_dir());
        assert!(cached_path.join("main.rk").is_file());

        // Get from cache should return the path
        let got = cache.get("hello", "1.0.0", &expected_checksum);
        assert!(got.is_some());
        assert_eq!(got.unwrap(), cached_path);
    }

    #[test]
    fn test_cache_checksum_mismatch() {
        let tmp = TempDir::new().unwrap();
        let cache = PackageCache::with_root(tmp.path().join("cache"));

        let tarball = create_test_tarball(tmp.path(), &[
            ("main.rk", "func main() { }"),
        ]);

        let result = cache.store("hello", "1.0.0", &tarball, "sha256:wrong");
        assert!(result.is_err());
        match result.unwrap_err() {
            CacheError::ChecksumMismatch { .. } => {}
            other => panic!("expected ChecksumMismatch, got: {}", other),
        }

        // Directory should be cleaned up
        assert!(!cache.pkg_dir("hello", "1.0.0").exists());
    }

    #[test]
    fn test_cache_get_missing() {
        let tmp = TempDir::new().unwrap();
        let cache = PackageCache::with_root(tmp.path().join("cache"));
        assert!(cache.get("nonexistent", "1.0.0", "sha256:any").is_none());
    }

    #[test]
    fn test_cache_remove() {
        let tmp = TempDir::new().unwrap();
        let cache = PackageCache::with_root(tmp.path().join("cache"));

        let dir = cache.pkg_dir("hello", "1.0.0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("main.rk"), "func main() { }").unwrap();

        cache.remove("hello", "1.0.0").unwrap();
        assert!(!dir.exists());
    }

    #[test]
    fn test_cache_clear() {
        let tmp = TempDir::new().unwrap();
        let cache = PackageCache::with_root(tmp.path().join("cache"));

        let dir1 = cache.pkg_dir("a", "1.0.0");
        let dir2 = cache.pkg_dir("b", "2.0.0");
        fs::create_dir_all(&dir1).unwrap();
        fs::create_dir_all(&dir2).unwrap();

        cache.clear().unwrap();
        assert!(!cache.root.exists());
    }
}
