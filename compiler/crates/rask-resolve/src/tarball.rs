// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Reproducible tarball creation for package publishing — struct.build/PB6.
//!
//! Creates deterministic tar.gz archives: sorted file order,
//! zeroed timestamps, fixed permissions.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Sha256, Digest};

/// Info about a created tarball.
pub struct TarballInfo {
    /// Path to the tarball file.
    pub path: PathBuf,
    /// Total size in bytes.
    pub size: u64,
    /// Number of files included.
    pub file_count: usize,
    /// SHA-256 checksum of the tarball.
    pub checksum: String,
    /// Per-file breakdown: (relative_path, size).
    pub files: Vec<(String, u64)>,
}

/// 10 MB max package size (PB7).
pub const MAX_PUBLISH_SIZE: u64 = 10 * 1024 * 1024;

/// Create a reproducible tarball from a package directory (PB6).
///
/// Includes all .rk files, build.rk, README*, LICENSE* files.
/// File order is deterministic (sorted by relative path).
/// Timestamps, uid, gid are zeroed for reproducibility.
pub fn create_reproducible_tarball(
    root: &Path,
    dest: &Path,
) -> Result<TarballInfo, String> {
    // Collect publishable files
    let mut file_map: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    collect_publishable_files(root, root, &mut file_map)
        .map_err(|e| format!("failed to collect files: {}", e))?;

    if file_map.is_empty() {
        return Err("no publishable files found".to_string());
    }

    // Build per-file info
    let files: Vec<(String, u64)> = file_map.iter()
        .map(|(path, content)| (path.clone(), content.len() as u64))
        .collect();
    let file_count = files.len();

    // Create tar.gz with deterministic settings
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory: {}", e))?;
    }

    let file = std::fs::File::create(dest)
        .map_err(|e| format!("failed to create tarball: {}", e))?;
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);

    for (rel_path, content) in &file_map {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0); // PB6: no timestamps
        header.set_cksum();

        builder.append_data(&mut header, rel_path, content.as_slice())
            .map_err(|e| format!("failed to write {}: {}", rel_path, e))?;
    }

    builder.into_inner()
        .map_err(|e| format!("failed to finish tarball: {}", e))?
        .finish()
        .map_err(|e| format!("failed to compress: {}", e))?;

    // Compute checksum of the tarball itself
    let tarball_bytes = std::fs::read(dest)
        .map_err(|e| format!("failed to read tarball: {}", e))?;
    let size = tarball_bytes.len() as u64;
    let mut hasher = Sha256::new();
    hasher.update(&tarball_bytes);
    let checksum = format!("sha256:{:x}", hasher.finalize());

    Ok(TarballInfo {
        path: dest.to_path_buf(),
        size,
        file_count,
        checksum,
        files,
    })
}

/// Recursively collect publishable files into a sorted map.
fn collect_publishable_files(
    base: &Path,
    dir: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");

        if path.is_dir() {
            // Skip build output, vendor, hidden dirs
            if name.starts_with('.') || name == "build" || name == "vendor"
                || name.starts_with('_')
            {
                continue;
            }
            collect_publishable_files(base, &path, files)?;
        } else if is_publishable(name) {
            let content = std::fs::read(&path)?;
            let rel = path.strip_prefix(base)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            files.insert(rel, content);
        }
    }
    Ok(())
}

/// Check if a file should be included in the published tarball.
fn is_publishable(name: &str) -> bool {
    let lower = name.to_lowercase();
    // .rk source files (including build.rk)
    if name.ends_with(".rk") {
        return true;
    }
    // Standard project files
    if lower.starts_with("readme") || lower.starts_with("license")
        || lower.starts_with("changelog")
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_reproducible_tarball() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("pkg");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("main.rk"), "func main() { }").unwrap();
        std::fs::write(pkg.join("build.rk"), "package \"test\" \"1.0.0\"").unwrap();
        std::fs::write(pkg.join("README.md"), "# Test").unwrap();

        let dest1 = tmp.path().join("out1.tar.gz");
        let dest2 = tmp.path().join("out2.tar.gz");

        let info1 = create_reproducible_tarball(&pkg, &dest1).unwrap();
        let info2 = create_reproducible_tarball(&pkg, &dest2).unwrap();

        // Deterministic: same checksum both times
        assert_eq!(info1.checksum, info2.checksum);
        assert_eq!(info1.file_count, 3);
        assert_eq!(info1.size, info2.size);
    }

    #[test]
    fn test_empty_package() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("empty");
        std::fs::create_dir_all(&pkg).unwrap();

        let dest = tmp.path().join("out.tar.gz");
        let result = create_reproducible_tarball(&pkg, &dest);
        assert!(result.is_err());
    }

    #[test]
    fn test_skips_build_vendor_hidden() {
        let tmp = TempDir::new().unwrap();
        let pkg = tmp.path().join("pkg");
        std::fs::create_dir_all(pkg.join("build")).unwrap();
        std::fs::create_dir_all(pkg.join("vendor")).unwrap();
        std::fs::create_dir_all(pkg.join(".hidden")).unwrap();
        std::fs::write(pkg.join("build").join("output"), "binary").unwrap();
        std::fs::write(pkg.join("vendor").join("dep.rk"), "vendored").unwrap();
        std::fs::write(pkg.join(".hidden").join("secret"), "hidden").unwrap();
        std::fs::write(pkg.join("main.rk"), "func main() { }").unwrap();

        let dest = tmp.path().join("out.tar.gz");
        let info = create_reproducible_tarball(&pkg, &dest).unwrap();
        assert_eq!(info.file_count, 1); // only main.rk
    }
}
