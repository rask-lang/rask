// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Package cache — struct.packages/CA1-CA3.
//!
//! Manages downloaded registry packages at `~/.rask/cache/pkg/`.
//! Archives are extracted and verified by SHA-256 checksum before use.

use std::path::{Path, PathBuf};

use crate::lockfile::compute_checksum;

/// Where an archive is extracted before it's renamed into the cache. Inside
/// the cache root, so the rename stays on one filesystem.
const STAGING: &str = ".staging";

/// Tells apart two extractions in one process.
static STAGED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    ///
    /// Extracted beside the cache and renamed into place once it checks out,
    /// so a package's directory is either absent or complete. Extracted in
    /// place, a second `rask` sharing the cache found the half-written
    /// directory, read its checksum as a mismatch and deleted it under the
    /// first, which then failed its own check.
    pub fn store(
        &self,
        name: &str,
        version: &str,
        archive: &Path,
        expected_checksum: &str,
    ) -> Result<PathBuf, CacheError> {
        let dest = self.pkg_dir(name, version);
        let staging = self.root.join(STAGING).join(format!(
            "{name}-{version}-{}-{}",
            std::process::id(),
            STAGED.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let result = Self::extract_verified(archive, &staging, expected_checksum)
            .and_then(|()| Self::move_into_place(&staging, &dest, expected_checksum));
        let _ = std::fs::remove_dir_all(&staging);
        result.map(|()| dest)
    }

    fn extract_verified(archive: &Path, into: &Path, expected_checksum: &str) -> Result<(), CacheError> {
        let file = std::fs::File::open(archive)?;
        let decoder = flate2::read::GzDecoder::new(file);
        tar::Archive::new(decoder)
            .unpack(into)
            .map_err(|e| CacheError::Extract(e.to_string()))?;
        let actual = compute_checksum(into);
        if actual != expected_checksum {
            return Err(CacheError::ChecksumMismatch {
                expected: expected_checksum.to_string(),
                actual,
            });
        }
        Ok(())
    }

    /// Rename a verified extraction to `dest`. Another process may have put
    /// the same package there first; a complete copy of the same checksum is
    /// as good as ours.
    fn move_into_place(staged: &Path, dest: &Path, expected_checksum: &str) -> Result<(), CacheError> {
        if dest.is_dir() {
            if compute_checksum(dest) == expected_checksum {
                return Ok(());
            }
            std::fs::remove_dir_all(dest)?;
        }
        match std::fs::rename(staged, dest) {
            Ok(()) => Ok(()),
            Err(_) if dest.is_dir() && compute_checksum(dest) == expected_checksum => Ok(()),
            Err(e) => Err(e.into()),
        }
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

    /// Processes sharing a cache store and read the same package at once.
    /// Extracted in place, one deleted the other's half-written directory as
    /// a checksum mismatch, and the other's store then failed.
    #[test]
    fn concurrent_stores_of_one_package_all_succeed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("cache");
        let files: Vec<(String, String)> =
            (0..40).map(|i| (format!("f{i}.rk"), format!("func f{i}() {{ }}\n").repeat(200))).collect();
        let refs: Vec<(&str, &str)> = files.iter().map(|(n, c)| (n.as_str(), c.as_str())).collect();
        let tarball = create_test_tarball(tmp.path(), &refs);
        let probe = tmp.path().join("probe");
        fs::create_dir_all(&probe).unwrap();
        tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(&tarball).unwrap()))
            .unpack(&probe)
            .unwrap();
        let checksum = compute_checksum(&probe);

        let threads: Vec<_> = (0..8)
            .map(|_| {
                let (root, tarball, checksum) = (root.clone(), tarball.clone(), checksum.clone());
                std::thread::spawn(move || {
                    let cache = PackageCache::with_root(root);
                    for _ in 0..10 {
                        let got = match cache.get("busy", "1.0.0", &checksum) {
                            Some(dir) => dir,
                            None => cache.store("busy", "1.0.0", &tarball, &checksum).unwrap(),
                        };
                        assert!(got.join("f39.rk").is_file());
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let cache = PackageCache::with_root(root);
        assert!(cache.get("busy", "1.0.0", &checksum).is_some());
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
