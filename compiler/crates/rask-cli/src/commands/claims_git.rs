// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Claims sources from git (type.claims/W3).
//!
//! A source is cloned once per URL and revision into the user's cache and
//! never fetched again. The first build pins what the checkout holds in
//! `rask.lock`; a checkout edited after that no longer matches and fails the
//! build, so the only way to change a source is to change its revision in
//! rask.toml, where review sees it.

use std::path::{Path, PathBuf};
use std::process::Command;

use rask_ast::decl::ClaimsSourceDecl;
use rask_resolve::lockfile::{compute_checksum, LockFile, LockedClaims};

/// The checkout of `source`, fetched if it isn't cached yet, after checking
/// it against the pin in the lock of the package at `root`.
pub fn checkout(root: &Path, source: &ClaimsSourceDecl) -> Result<PathBuf, String> {
    let url = source.git.as_deref().unwrap_or_default();
    let Some(rev) = source.rev.as_deref().or(source.branch.as_deref()) else {
        return Err(format!(
            "[claims] {}: a git claims source needs a `rev` (or a `branch`), so every build checks the same claims",
            source.name
        ));
    };
    let pinned_as = format!("git+{url}#{rev}");
    let dir = cache_dir().join(format!("{:016x}", fnv1a(&pinned_as)));
    if !dir.is_dir() {
        fetch(url, rev, &dir).map_err(|e| format!("[claims] {}: fetching {url} at {rev}: {e}", source.name))?;
    }
    let checksum = compute_checksum(&dir);

    let lock_path = root.join("rask.lock");
    let mut lock = if lock_path.exists() { LockFile::load(&lock_path)? } else { LockFile::default() };
    match lock.claims.iter().find(|c| c.name == source.name) {
        Some(pin) if pin.source == pinned_as && pin.checksum != checksum => {
            return Err(format!(
                "[claims] {}: the checkout at {} doesn't match what rask.lock pinned, so it changed after it \
                 was locked. Claims change through rask.toml, in review: point `rev` at the new revision. \
                 To fetch the pinned one again, delete that directory",
                source.name,
                dir.display()
            ));
        }
        Some(pin) if pin.source == pinned_as => {}
        _ => {
            lock.claims.retain(|c| c.name != source.name);
            lock.claims.push(LockedClaims { name: source.name.clone(), source: pinned_as, checksum });
            lock.claims.sort_by(|a, b| a.name.cmp(&b.name));
            if lock.version == 0 {
                lock.version = 1;
            }
            lock.write(&lock_path)?;
        }
    }
    Ok(dir)
}

/// `$RASK_CLAIMS_CACHE`, else `$XDG_CACHE_HOME/rask/claims`, else
/// `~/.cache/rask/claims`.
fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RASK_CLAIMS_CACHE") {
        return PathBuf::from(dir);
    }
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("rask").join("claims")
}

/// Clone `url` at `rev` into `dir`, through a temporary directory so a
/// failed fetch leaves nothing that looks cached.
fn fetch(url: &str, rev: &str, dir: &Path) -> Result<(), String> {
    let parent = dir.parent().ok_or("no cache directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let tmp = parent.join(format!(".fetch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let run = |args: &[&str]| -> Result<(), String> {
        let out = Command::new("git").args(args).output().map_err(|e| format!("running git: {e}"))?;
        if out.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    };
    let tmp_str = tmp.to_string_lossy();
    run(&["clone", "--quiet", "--no-checkout", url, &tmp_str])?;
    run(&["-C", &tmp_str, "checkout", "--quiet", rev])?;
    std::fs::rename(&tmp, dir).map_err(|e| e.to_string())
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}
