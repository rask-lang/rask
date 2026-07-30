// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Compilation cache — XC1-XC5.
//!
//! Caches compiled .o files keyed by source content + profile + target + which
//! compiler produced them. Skips codegen entirely when the cache hits.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Identity of the `rask` binary that is running.
///
/// Nothing in a package's source says which compiler compiled it, so a key made
/// only of source served objects built by a *previous* binary: rebuild the
/// compiler, rebuild the package, and the old codegen gets relinked. A fix could
/// look like it did nothing, and a miscompile could survive the change that fixed
/// it — silently, which is the dangerous part.
///
/// Path + size + mtime of the running executable, which cargo changes on every
/// rebuild. That also covers the stdlib: `stdlib/*.rk` is `include_str!`'d into
/// the binary, so editing it changes the binary. Hashing the whole executable
/// would be stricter but costs more than the build it is protecting — a package
/// build is ~1s and the binary is tens of megabytes.
pub fn compiler_fingerprint() -> u64 {
    static FINGERPRINT: OnceLock<u64> = OnceLock::new();
    *FINGERPRINT.get_or_init(|| {
        let mut hasher = DefaultHasher::new();
        env!("CARGO_PKG_VERSION").hash(&mut hasher);
        if let Ok(exe) = std::env::current_exe() {
            exe.hash(&mut hasher);
            if let Ok(meta) = std::fs::metadata(&exe) {
                meta.len().hash(&mut hasher);
                if let Ok(mtime) = meta.modified() {
                    if let Ok(since_epoch) = mtime.duration_since(std::time::UNIX_EPOCH) {
                        since_epoch.as_nanos().hash(&mut hasher);
                    }
                }
            }
        }
        hasher.finish()
    })
}

/// Compute a cache key from source declarations, profile, target, and compiler.
/// Not cryptographic — just needs deterministic collision resistance for local use.
///
/// `compiler` keeps two compilers' objects apart rather than invalidating on
/// every switch, so alternating between a release and a debug build (or a
/// worktree) still hits its own entries.
pub fn compute_cache_key(
    source_hash_inputs: &[u8],
    profile: &str,
    target: &str,
    compiler: u64,
) -> String {
    let mut hasher = DefaultHasher::new();
    source_hash_inputs.hash(&mut hasher);
    profile.hash(&mut hasher);
    target.hash(&mut hasher);
    compiler.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Hash all source files in a package for cache keying.
pub fn hash_source_files(files: &[(PathBuf, String)]) -> Vec<u8> {
    use std::collections::BTreeMap;

    // Sort by path for deterministic hashing
    let sorted: BTreeMap<&PathBuf, &String> = files.iter().map(|(p, s)| (p, s)).collect();

    let mut hasher = DefaultHasher::new();
    for (path, source) in &sorted {
        path.hash(&mut hasher);
        source.hash(&mut hasher);
    }
    hasher.finish().to_le_bytes().to_vec()
}

/// Look up a cached object file.
pub fn lookup(cache_dir: &Path, key: &str) -> Option<PathBuf> {
    let cached = cache_dir.join(format!("{}.o", key));
    if cached.is_file() {
        Some(cached)
    } else {
        None
    }
}

/// Store an object file in the cache.
pub fn store(cache_dir: &Path, key: &str, obj_path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| format!("failed to create cache dir: {}", e))?;

    let cached = cache_dir.join(format!("{}.o", key));
    std::fs::copy(obj_path, &cached)
        .map_err(|e| format!("failed to cache object file: {}", e))?;

    // LRU eviction if cache is too large
    if let Err(e) = evict_if_needed(cache_dir) {
        eprintln!("warning: cache eviction failed: {}", e);
    }

    Ok(())
}

/// Evict oldest cache entries if total size exceeds limit.
fn evict_if_needed(cache_dir: &Path) -> Result<(), String> {
    let max_bytes: u64 = std::env::var("RASK_CACHE_SIZE")
        .ok()
        .and_then(|s| parse_size(&s))
        .unwrap_or(2 * 1024 * 1024 * 1024); // 2 GB default

    let mut entries: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total_size: u64 = 0;

    let dir = std::fs::read_dir(cache_dir)
        .map_err(|e| format!("failed to read cache dir: {}", e))?;

    for entry in dir.flatten() {
        let path = entry.path();
        if path.extension().map(|e| e == "o").unwrap_or(false) {
            if let Ok(meta) = entry.metadata() {
                let size = meta.len();
                let modified = meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                total_size += size;
                entries.push((path, size, modified));
            }
        }
    }

    if total_size <= max_bytes {
        return Ok(());
    }

    // Sort by modification time (oldest first)
    entries.sort_by_key(|(_, _, t)| *t);

    // Remove oldest until under limit
    for (path, size, _) in &entries {
        if total_size <= max_bytes {
            break;
        }
        if let Ok(()) = std::fs::remove_file(path) {
            total_size -= size;
        }
    }

    Ok(())
}

/// Parse human-readable size strings (e.g., "2G", "500M", "1024K").
fn parse_size(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(num) = s.strip_suffix('G').or_else(|| s.strip_suffix('g')) {
        num.trim().parse::<u64>().ok().map(|n| n * 1024 * 1024 * 1024)
    } else if let Some(num) = s.strip_suffix('M').or_else(|| s.strip_suffix('m')) {
        num.trim().parse::<u64>().ok().map(|n| n * 1024 * 1024)
    } else if let Some(num) = s.strip_suffix('K').or_else(|| s.strip_suffix('k')) {
        num.trim().parse::<u64>().ok().map(|n| n * 1024)
    } else {
        s.parse().ok()
    }
}

// === Build script caching (LC2) ===

/// Cached state from a previous build script run.
pub struct BuildScriptCache {
    pub hash: u64,
    pub deps: Vec<PathBuf>,
    pub libs: Vec<String>,
    pub search_paths: Vec<String>,
    pub objects: Vec<String>,
}

/// Hash build.rk content + content of declared dependency files.
///
/// The compiler's identity counts here too: a build script is compiled Rask, so
/// a new compiler can make the same `build.rk` behave differently.
pub fn hash_build_inputs(build_rk_content: &str, dep_paths: &[PathBuf]) -> u64 {
    let mut hasher = DefaultHasher::new();
    compiler_fingerprint().hash(&mut hasher);
    build_rk_content.hash(&mut hasher);
    // Sort dep paths for deterministic ordering
    let mut sorted: Vec<_> = dep_paths.to_vec();
    sorted.sort();
    for path in &sorted {
        path.hash(&mut hasher);
        if let Ok(content) = std::fs::read(path) {
            content.hash(&mut hasher);
        } else {
            // Missing dep file — hash a sentinel so cache invalidates
            0xDEADu64.hash(&mut hasher);
        }
    }
    hasher.finish()
}

/// Load cached build script state from `build/.build-cache/`.
pub fn load_build_cache(cache_dir: &Path) -> Option<BuildScriptCache> {
    let hash_file = cache_dir.join("build-script.hash");
    let deps_file = cache_dir.join("build-script.deps");
    let link_file = cache_dir.join("build-script.link");

    let hash: u64 = std::fs::read_to_string(&hash_file).ok()?.trim().parse().ok()?;

    let deps: Vec<PathBuf> = std::fs::read_to_string(&deps_file).ok()
        .map(|s| s.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect())
        .unwrap_or_default();

    let link_content = std::fs::read_to_string(&link_file).ok().unwrap_or_default();
    let (libs, search_paths, objects) = parse_link_state(&link_content);

    Some(BuildScriptCache { hash, deps, libs, search_paths, objects })
}

/// Save build script cache state after a successful run.
pub fn save_build_cache(
    cache_dir: &Path,
    hash: u64,
    deps: &[PathBuf],
    libs: &[String],
    search_paths: &[String],
    objects: &[String],
) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| format!("failed to create build cache dir: {}", e))?;

    std::fs::write(cache_dir.join("build-script.hash"), hash.to_string())
        .map_err(|e| format!("failed to write build-script.hash: {}", e))?;

    let deps_content: String = deps.iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(cache_dir.join("build-script.deps"), deps_content)
        .map_err(|e| format!("failed to write build-script.deps: {}", e))?;

    let link_content = format_link_state(libs, search_paths, objects);
    std::fs::write(cache_dir.join("build-script.link"), link_content)
        .map_err(|e| format!("failed to write build-script.link: {}", e))?;

    Ok(())
}

/// Serialize link state as simple line-oriented format.
/// Each section is a header line followed by values, separated by blank lines.
fn format_link_state(libs: &[String], search_paths: &[String], objects: &[String]) -> String {
    let mut out = String::new();
    out.push_str("[libs]\n");
    for lib in libs { out.push_str(lib); out.push('\n'); }
    out.push_str("[search_paths]\n");
    for sp in search_paths { out.push_str(sp); out.push('\n'); }
    out.push_str("[objects]\n");
    for obj in objects { out.push_str(obj); out.push('\n'); }
    out
}

/// Parse link state from the simple line-oriented format.
fn parse_link_state(content: &str) -> (Vec<String>, Vec<String>, Vec<String>) {
    let mut libs = Vec::new();
    let mut search_paths = Vec::new();
    let mut objects = Vec::new();
    let mut section = "";

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() { continue; }
        match line {
            "[libs]" => section = "libs",
            "[search_paths]" => section = "search_paths",
            "[objects]" => section = "objects",
            _ => match section {
                "libs" => libs.push(line.to_string()),
                "search_paths" => search_paths.push(line.to_string()),
                "objects" => objects.push(line.to_string()),
                _ => {}
            },
        }
    }

    (libs, search_paths, objects)
}

#[cfg(test)]
mod tests {
    use super::*;

    // The whole point of the compiler component: the same source compiled by a
    // different binary must not hit the same cache entry. Without it a rebuilt
    // compiler relinked the previous binary's objects and a codegen fix looked
    // like it did nothing.
    #[test]
    fn cache_key_separates_compilers() {
        let src = b"same source";
        let a = compute_cache_key(src, "debug", "native", 0x1111);
        let b = compute_cache_key(src, "debug", "native", 0x2222);
        assert_ne!(a, b);
    }

    #[test]
    fn cache_key_is_stable_for_the_same_inputs() {
        let src = b"same source";
        let a = compute_cache_key(src, "debug", "native", 0x1111);
        let b = compute_cache_key(src, "debug", "native", 0x1111);
        assert_eq!(a, b);
    }

    #[test]
    fn cache_key_still_separates_source_profile_and_target() {
        let fp = 0x1111;
        let base = compute_cache_key(b"a", "debug", "native", fp);
        assert_ne!(base, compute_cache_key(b"b", "debug", "native", fp));
        assert_ne!(base, compute_cache_key(b"a", "release", "native", fp));
        assert_ne!(base, compute_cache_key(b"a", "debug", "wasm32", fp));
    }

    // Two calls in one process describe one binary, so the fingerprint has to be
    // the same both times or every build would miss its own cache.
    #[test]
    fn compiler_fingerprint_is_stable_within_a_process() {
        assert_eq!(compiler_fingerprint(), compiler_fingerprint());
    }

    #[test]
    fn build_script_hash_includes_the_compiler() {
        // Can't vary the fingerprint inside one process, so check the weaker
        // property the caller depends on: the compiler is mixed in at all, so
        // this hash isn't just the content hash.
        let mut content_only = DefaultHasher::new();
        "build".hash(&mut content_only);
        assert_ne!(hash_build_inputs("build", &[]), content_only.finish());
    }
}
