// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared linking utilities for compile and build commands.

use std::path::Path;
use std::process;

/// Runtime C source files to compile and link.
const RUNTIME_SOURCES: &[&str] = &[
    "runtime.c",
    "args.c",
    "alloc.c",
    "panic.c",
    "thread.c",
    "channel.c",
    "sync.c",
    "vec.c",
    "map.c",
    "pool.c",
    "string.c",
    "green.c",
    "io_uring_engine.c",
    "io_epoll_engine.c",
    "random.c",
    "time.c",
    "atomic.c",
    "simd.c",
    "bench.c",
    "ptr.c",
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

/// Link with extra libraries and object files.
pub fn link_executable_with(obj_path: &str, bin_path: &str, opts: &LinkOptions) -> Result<(), String> {
    let runtime_dir = find_runtime_dir()?;

    for src in RUNTIME_SOURCES {
        if !runtime_dir.join(src).exists() {
            return Err(format!(
                "missing {} in {} — runtime is incomplete",
                src,
                runtime_dir.display()
            ));
        }
    }

    let mut cmd = process::Command::new("cc");
    for src in RUNTIME_SOURCES {
        cmd.arg(runtime_dir.join(src));
    }
    cmd.arg(obj_path);
    for obj in &opts.objects {
        cmd.arg(obj);
    }
    cmd.args(["-o", bin_path, "-no-pie", "-lpthread", "-lm"]);
    for path in &opts.search_paths {
        cmd.arg(format!("-L{}", path));
    }
    for lib in &opts.libs {
        cmd.arg(format!("-l{}", lib));
    }

    let status = cmd
        .status()
        .map_err(|e| format!("failed to run cc: {}", e))?;

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
