// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared linking utilities for compile and build commands.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::process;

/// Sources that require pthreads (Linux, macOS — not Windows/bare-metal).
const PTHREAD_SOURCES: &[&str] = &[
    "thread.c",
    "threadpool.c",
    "channel.c",
    "sync.c",
];

/// Linux-only: green scheduler + I/O backends (epoll, io_uring).
const LINUX_SOURCES: &[&str] = &[
    "green.c",
    "io_uring_engine.c",
    "io_epoll_engine.c",
];

/// Every other `.c` in the runtime directory — the portable set, read from the
/// directory rather than listed here.
///
/// It used to be a 21-entry constant, which is a directory listing a human had
/// to keep in step. `unicode_text.c` was added to the tree and to that list in
/// the same change, so every `rask` binary built before it linked a `string.c`
/// that called into a file it had never heard of — three `undefined reference to
/// rask_canonical_decompose`-style lines, on programs that used no strings, with
/// nothing pointing at the compiler being stale (#1041).
///
/// The platform lists above stay written down: which sources need pthreads and
/// which are Linux-only is knowledge, not something a listing can tell you.
/// Sorted, so link order and the cache key don't depend on readdir order.
fn portable_sources(runtime_dir: &Path) -> Result<Vec<String>, String> {
    let entries = std::fs::read_dir(runtime_dir).map_err(|e| {
        format!("failed to read runtime dir {}: {}", runtime_dir.display(), e)
    })?;

    let mut sources: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.ends_with(".c"))
        .filter(|n| {
            !PTHREAD_SOURCES.contains(&n.as_str()) && !LINUX_SOURCES.contains(&n.as_str())
        })
        .collect();
    sources.sort();

    if sources.is_empty() {
        return Err(format!(
            "no runtime sources in {} — runtime is incomplete",
            runtime_dir.display()
        ));
    }
    Ok(sources)
}

/// Known target triples from the spec tier list.
const KNOWN_TARGETS: &[&str] = &[
    // Tier 1
    "x86_64-linux", "aarch64-linux",
    "x86_64-macos", "aarch64-macos",
    // Tier 2
    "x86_64-windows-msvc", "aarch64-windows-msvc",
    "wasm32-none",
    "x86_64-linux-musl", "aarch64-linux-musl",
    // Tier 3
    "riscv64-linux", "x86_64-freebsd", "arm-none",
];

/// Extra link-time inputs (libraries, object files, search paths).
#[derive(Default)]
pub struct LinkOptions {
    /// System libraries to link (-l flags, e.g. "m" for libm)
    pub libs: Vec<String>,
    /// Additional object files or C source files to link
    pub objects: Vec<String>,
    /// Library search paths (-L flags)
    pub search_paths: Vec<String>,
}

/// Platform-specific linking configuration derived from a target triple.
struct TargetConfig {
    cc: String,
    cc_args: Vec<String>,
    sources: Vec<String>,
    link_flags: Vec<String>,
}

impl TargetConfig {
    fn for_target(target: Option<&str>, runtime_dir: &Path) -> Result<Self, String> {
        let host_os = std::env::consts::OS;
        let host_arch = std::env::consts::ARCH;
        let host_triple = format!("{}-{}", host_arch, host_os);

        let target_triple = target.unwrap_or(&host_triple);
        let parts: Vec<&str> = target_triple.split('-').collect();
        let target_os = parts.get(1).copied().unwrap_or("unknown");
        let target_arch = parts.first().copied().unwrap_or("unknown");

        // Check runtime support for this OS
        match target_os {
            "linux" | "macos" => {}
            "windows" => return Err(format!(
                "cross-compilation to {} — Windows runtime not yet available",
                target_triple,
            )),
            "none" if target_arch == "wasm32" => return Err(format!(
                "cross-compilation to wasm32 — requires wasm-ld (not yet supported)",
            )),
            "none" => return Err(format!(
                "cross-compilation to {} — bare-metal runtime not yet available",
                target_triple,
            )),
            _ => return Err(format!(
                "cross-compilation to {} — runtime not available for OS '{}'",
                target_triple, target_os,
            )),
        }

        let is_native = target.is_none()
            || target_triple == host_triple
            || (target_os == host_os && target_arch == host_arch);

        // Resolve C compiler
        let (cc, cc_args) = resolve_cc(target_triple, target_os, target_arch, is_native)?;

        // Select runtime sources
        let mut sources = portable_sources(runtime_dir)?;
        match target_os {
            "linux" => {
                sources.extend(PTHREAD_SOURCES.iter().map(|s| s.to_string()));
                sources.extend(LINUX_SOURCES.iter().map(|s| s.to_string()));
            }
            "macos" => {
                sources.extend(PTHREAD_SOURCES.iter().map(|s| s.to_string()));
                // No green scheduler on macOS yet (needs kqueue backend)
            }
            _ => {}
        }

        // Platform-specific link flags
        let link_flags = match target_os {
            "linux" => vec!["-no-pie".into(), "-lpthread".into(), "-lm".into()],
            "macos" => vec!["-lpthread".into(), "-lm".into()],
            _ => vec![],
        };

        Ok(TargetConfig { cc, cc_args, sources, link_flags })
    }
}

/// Resolve the C compiler for a given target.
///
/// Resolution order:
/// 1. CC environment variable
/// 2. Native build → "cc"
/// 3. zig cc (universal cross-compiler)
/// 4. Platform-prefixed gcc (e.g. aarch64-linux-gnu-gcc)
/// 5. macOS clang with -arch flag (x86_64 ↔ aarch64)
fn resolve_cc(
    target: &str,
    target_os: &str,
    target_arch: &str,
    is_native: bool,
) -> Result<(String, Vec<String>), String> {
    // 1. CC env var always wins
    if let Ok(cc) = std::env::var("CC") {
        return Ok((cc, vec![]));
    }

    // 2. Native build
    if is_native {
        return Ok(("cc".into(), vec![]));
    }

    // 3. zig cc
    if probe_cc("zig", &["cc", "--version"]) {
        let zig_target = to_zig_target(target_arch, target_os);
        return Ok(("zig".into(), vec!["cc".into(), format!("--target={}", zig_target)]));
    }

    // 4. Prefixed gcc
    let prefix = gcc_prefix(target_arch, target_os);
    if let Some(pfx) = &prefix {
        let gcc = format!("{}-gcc", pfx);
        if probe_cc(&gcc, &["--version"]) {
            return Ok((gcc, vec![]));
        }
    }

    // 5. macOS clang cross between x86_64 and aarch64
    let host_os = std::env::consts::OS;
    if host_os == "macos" && target_os == "macos" {
        return Ok(("clang".into(), vec!["-arch".into(), clang_arch(target_arch).into()]));
    }

    // Nothing found
    let mut msg = format!(
        "cross-compilation to {} requires a C cross-compiler\n\nInstall one of:\n  - zig (recommended): https://ziglang.org/download/\n",
        target,
    );
    if let Some(pfx) = &prefix {
        msg.push_str(&format!("  - {}-gcc\n", pfx));
    }
    msg.push_str("  - set CC=<your-cross-compiler>");
    Err(msg)
}

/// Check if a compiler is available by running it.
fn probe_cc(cmd: &str, args: &[&str]) -> bool {
    process::Command::new(cmd)
        .args(args)
        .stdout(process::Stdio::null())
        .stderr(process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Map target to zig-style target triple.
fn to_zig_target(arch: &str, os: &str) -> String {
    let zig_os = match os {
        "macos" => "macos",
        "linux" => "linux-gnu",
        _ => os,
    };
    format!("{}-{}", arch, zig_os)
}

/// Map target to gcc cross-compiler prefix.
fn gcc_prefix(arch: &str, os: &str) -> Option<String> {
    match (arch, os) {
        ("aarch64", "linux") => Some("aarch64-linux-gnu".into()),
        ("x86_64", "linux") => Some("x86_64-linux-gnu".into()),
        ("aarch64", "windows") => Some("aarch64-w64-mingw32".into()),
        ("x86_64", "windows") => Some("x86_64-w64-mingw32".into()),
        ("riscv64", "linux") => Some("riscv64-linux-gnu".into()),
        ("arm", _) => Some("arm-none-eabi".into()),
        _ => None,
    }
}

/// Map arch name to clang -arch value.
fn clang_arch(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        other => other,
    }
}

/// Validate a target triple. Returns Ok if known or parseable.
pub fn validate_target(target: &str) -> Result<(), String> {
    if KNOWN_TARGETS.contains(&target) {
        return Ok(());
    }
    // Accept anything that looks like arch-os or arch-os-env
    let parts: Vec<&str> = target.split('-').collect();
    if parts.len() >= 2 && parts.len() <= 3 {
        return Ok(());
    }
    Err(format!(
        "unknown target '{}' — run `rask targets` to see available targets",
        target,
    ))
}

// ─── Runtime object cache ────────────────────────────────────────────────
//
// The runtime is 28 C files and they were recompiled from source on every
// `rask run`, `rask test` and `rask compile` — 1.8s of the 1.9s a hello-world
// takes, and ~10 minutes of a gate that gets through 373 files serially. The
// sources don't change between those invocations; only the Rask object does.
//
// So each source is compiled once into a cached `.o` and reused. Objects, not
// an archive: a static archive only contributes the members that resolve an
// undefined symbol, and passing every object explicitly keeps the link exactly
// as it was.

/// Compile flags for a profile — the ones that have to match between the
/// cached objects and the link, or the cache would hand back objects built
/// for a different build.
fn profile_cflags(release: bool) -> Vec<String> {
    if release {
        vec!["-O2".into()]
    } else {
        // -g preserves DWARF; -DRASK_DEBUG turns on the runtime's own checks.
        vec!["-DRASK_DEBUG".into(), "-g".into()]
    }
}

fn extra_cflags() -> Vec<String> {
    std::env::var("RASK_EXTRA_CFLAGS")
        .map(|e| e.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// Where cached runtime objects live.
///
/// Not the package's `build/.cache`: `rask run` on a loose file has no package,
/// and the runtime is the same for every package on the machine anyway.
fn runtime_cache_root() -> PathBuf {
    if let Ok(dir) = std::env::var("RASK_RUNTIME_CACHE") {
        return PathBuf::from(dir);
    }
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("rask").join("runtime");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home).join(".cache").join("rask").join("runtime");
    }
    std::env::temp_dir().join("rask-runtime-cache")
}

/// Identity of one set of runtime objects: which compiler, which flags, and
/// the sources themselves.
///
/// Sources are keyed by size and mtime rather than content — the same trade the
/// package cache makes for the compiler binary. Editing `runtime/*.c` therefore
/// invalidates this on its own, which is a change for the better: the old
/// advice was that editing a runtime source did nothing until you remembered to
/// rebuild the archive by hand.
fn runtime_cache_key(
    config: &TargetConfig,
    runtime_dir: &Path,
    release: bool,
) -> String {
    let mut hasher = DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut hasher);
    config.cc.hash(&mut hasher);
    config.cc_args.hash(&mut hasher);
    profile_cflags(release).hash(&mut hasher);
    extra_cflags().hash(&mut hasher);
    config.link_flags.hash(&mut hasher);

    for src in &config.sources {
        src.hash(&mut hasher);
        let path = runtime_dir.join(src);
        if let Ok(meta) = std::fs::metadata(&path) {
            meta.len().hash(&mut hasher);
            if let Ok(mtime) = meta.modified() {
                if let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH) {
                    d.as_nanos().hash(&mut hasher);
                }
            }
        }
    }
    format!("{:016x}", hasher.finish())
}

/// Compile the runtime once per (compiler, flags, sources) and reuse it.
///
/// Returns the object paths in `config.sources` order.
fn runtime_objects(
    config: &TargetConfig,
    runtime_dir: &Path,
    release: bool,
) -> Result<Vec<PathBuf>, String> {
    let dir = runtime_cache_root().join(runtime_cache_key(config, runtime_dir, release));
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create runtime cache dir {}: {}", dir.display(), e))?;

    let profile = profile_cflags(release);
    let extra = extra_cflags();
    let mut objects = Vec::with_capacity(config.sources.len());

    for src in &config.sources {
        let stem = src.trim_end_matches(".c");
        let obj = dir.join(format!("{}.o", stem));
        if obj.is_file() {
            objects.push(obj);
            continue;
        }

        // Compile to a private name and rename into place. `differential.sh`
        // runs many `rask` processes at once and they share this directory; a
        // rename is atomic, a half-written .o linked by a sibling is not.
        let tmp = dir.join(format!("{}.{}.tmp.o", stem, process::id()));
        let mut cmd = process::Command::new(&config.cc);
        cmd.args(&config.cc_args);
        cmd.args(&profile);
        // The link is -no-pie on Linux, so the objects must not be PIE either.
        if config.link_flags.iter().any(|f| f == "-no-pie") {
            cmd.arg("-fno-pie");
        }
        cmd.args(&extra);
        cmd.arg("-c").arg(runtime_dir.join(src));
        cmd.arg("-o").arg(&tmp);

        let status = cmd
            .status()
            .map_err(|e| format!("failed to run {}: {}", config.cc, e))?;
        if !status.success() {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("compiling runtime source {} failed", src));
        }
        // A rename losing the race with a sibling process is fine — both wrote
        // the same bytes from the same source with the same flags.
        if let Err(e) = std::fs::rename(&tmp, &obj) {
            let _ = std::fs::remove_file(&tmp);
            if !obj.is_file() {
                return Err(format!("failed to install {}: {}", obj.display(), e));
            }
        }
        objects.push(obj);
    }

    Ok(objects)
}

/// Link with extra libraries and object files.
pub fn link_executable_with(
    obj_path: &str,
    bin_path: &str,
    opts: &LinkOptions,
    release: bool,
    target: Option<&str>,
) -> Result<(), String> {
    let runtime_dir = find_runtime_dir()?;
    let config = TargetConfig::for_target(target, &runtime_dir)?;

    for src in &config.sources {
        if !runtime_dir.join(src).exists() {
            return Err(format!(
                "missing {} in {} — runtime is incomplete",
                src,
                runtime_dir.display()
            ));
        }
    }

    let runtime_objs = runtime_objects(&config, &runtime_dir, release)?;

    let mut cmd = process::Command::new(&config.cc);
    cmd.args(&config.cc_args);
    for flag in profile_cflags(release) {
        cmd.arg(flag);
    }
    cmd.arg(obj_path); // Rask .o first: keeps our DWARF section offsets valid
    for obj in &runtime_objs {
        cmd.arg(obj);
    }
    for obj in &opts.objects {
        cmd.arg(obj);
    }
    // Escape hatch for investigating a runtime-level bug: the runtime is
    // compiled here, so there was otherwise no way to get a sanitizer into a
    // Rask binary. `RASK_EXTRA_CFLAGS="-fsanitize=address -g"` is what #577's
    // heap corruption needs, and the same door serves -fsanitize=undefined,
    // coverage flags, or a one-off -D. It is part of the cache key, so the
    // runtime objects are built with it too rather than only the link.
    for flag in extra_cflags() {
        cmd.arg(flag);
    }
    cmd.args(["-o", bin_path]);
    for flag in &config.link_flags {
        cmd.arg(flag);
    }
    for path in &opts.search_paths {
        cmd.arg(format!("-L{}", path));
    }
    for lib in &opts.libs {
        cmd.arg(format!("-l{}", lib));
    }

    let out = cmd
        .output()
        .map_err(|e| format!("failed to run {}: {}", config.cc, e))?;

    // Always clean up the intermediate .o file
    let _ = std::fs::remove_file(obj_path);

    // Both streams go back out. Capturing was for the hint below, and dropping
    // either one silently would be the opposite of what it is for: a `cc`
    // front-end run with `-v`, or a wrapper like ccache, writes to stdout, and
    // that used to reach the terminal because this was `.status()`.
    if !out.stdout.is_empty() {
        print!("{}", String::from_utf8_lossy(&out.stdout));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    if !out.status.success() {
        let mut msg = format!("linker exited with status {}", out.status);
        if let Some(hint) = stale_runtime_hint(&stderr) {
            msg.push_str("\n\n");
            msg.push_str(&hint);
        }
        return Err(msg);
    }

    Ok(())
}

/// Explain an undefined `rask_*` symbol, which almost always means the binary
/// and the runtime tree disagree.
///
/// Raw `ld` output for this reads as a bug in the program being compiled: it
/// names `string.c` line numbers and Unicode internals for a program that never
/// touched a string, and says nothing about the compiler (#1041). The source
/// list is read from the runtime directory now, so the original cause is gone,
/// but a runtime whose sources and headers are out of step still lands here.
fn stale_runtime_hint(stderr: &str) -> Option<String> {
    let undefined_rask_symbol = stderr.lines().any(|l| {
        (l.contains("undefined reference to") || l.contains("undefined symbol"))
            && (l.contains("rask_") || l.contains("_rask_"))
    });
    if !undefined_rask_symbol {
        return None;
    }
    Some(
        "note: those are runtime symbols, so this is the compiler and the runtime \
         tree disagreeing — not a problem with the program being compiled.\n\
         \x20     Rebuild both:\n\
         \x20       cd compiler/runtime && make\n\
         \x20       cargo build --release -p rask-cli"
            .to_string(),
    )
}

/// Locate the runtime directory containing runtime.c and args.c.
/// Searches:
/// 1. RASK_RUNTIME_DIR environment variable
/// 2. Relative to the rask binary (walking up to find compiler/runtime/)
pub fn find_runtime_dir() -> Result<std::path::PathBuf, String> {
    if let Ok(dir) = std::env::var("RASK_RUNTIME_DIR") {
        let p = Path::new(&dir);
        if p.join("runtime.c").exists() {
            return Ok(p.to_path_buf());
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let mut dir = exe_dir.to_path_buf();
            for _ in 0..5 {
                let candidate = dir.join("compiler").join("runtime");
                if candidate.join("runtime.c").exists() {
                    return Ok(candidate);
                }
                let candidate = dir.join("runtime");
                if candidate.join("runtime.c").exists() {
                    return Ok(candidate);
                }
                if !dir.pop() {
                    break;
                }
            }
        }
    }

    Err("Could not find runtime directory — set RASK_RUNTIME_DIR to the directory containing runtime.c".to_string())
}
