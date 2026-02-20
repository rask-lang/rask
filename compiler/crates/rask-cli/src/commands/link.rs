// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared linking utilities for compile and build commands.

use std::path::Path;
use std::process;

/// Runtime C sources that compile on all platforms.
const PORTABLE_SOURCES: &[&str] = &[
    "runtime.c",
    "args.c",
    "alloc.c",
    "panic.c",
    "vec.c",
    "map.c",
    "pool.c",
    "string.c",
    "random.c",
    "time.c",
    "atomic.c",
    "simd.c",
    "bench.c",
    "ptr.c",
];

/// Sources that require pthreads (Linux, macOS — not Windows/bare-metal).
const PTHREAD_SOURCES: &[&str] = &[
    "thread.c",
    "channel.c",
    "sync.c",
];

/// Linux-only: green scheduler + I/O backends (epoll, io_uring).
const LINUX_SOURCES: &[&str] = &[
    "green.c",
    "io_uring_engine.c",
    "io_epoll_engine.c",
];

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
    sources: Vec<&'static str>,
    link_flags: Vec<String>,
}

impl TargetConfig {
    fn for_target(target: Option<&str>) -> Result<Self, String> {
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
        let mut sources: Vec<&'static str> = PORTABLE_SOURCES.to_vec();
        match target_os {
            "linux" => {
                sources.extend_from_slice(PTHREAD_SOURCES);
                sources.extend_from_slice(LINUX_SOURCES);
            }
            "macos" => {
                sources.extend_from_slice(PTHREAD_SOURCES);
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

/// Link with extra libraries and object files.
pub fn link_executable_with(
    obj_path: &str,
    bin_path: &str,
    opts: &LinkOptions,
    release: bool,
    target: Option<&str>,
) -> Result<(), String> {
    let config = TargetConfig::for_target(target)?;
    let runtime_dir = find_runtime_dir()?;

    for src in &config.sources {
        if !runtime_dir.join(src).exists() {
            return Err(format!(
                "missing {} in {} — runtime is incomplete",
                src,
                runtime_dir.display()
            ));
        }
    }

    let mut cmd = process::Command::new(&config.cc);
    cmd.args(&config.cc_args);
    if release {
        cmd.arg("-O2");
    } else {
        cmd.arg("-DRASK_DEBUG");
    }
    for src in &config.sources {
        cmd.arg(runtime_dir.join(src));
    }
    cmd.arg(obj_path);
    for obj in &opts.objects {
        cmd.arg(obj);
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

    let status = cmd
        .status()
        .map_err(|e| format!("failed to run {}: {}", config.cc, e))?;

    // Always clean up the intermediate .o file
    let _ = std::fs::remove_file(obj_path);

    if !status.success() {
        return Err(format!("linker exited with status {}", status));
    }

    Ok(())
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
