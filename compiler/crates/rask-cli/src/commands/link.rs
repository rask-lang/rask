// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared linking utilities for compile and build commands.

use std::path::Path;
use std::process;

/// Find the runtime.c file, compile it, and link with the object file.
pub fn link_executable(obj_path: &str, bin_path: &str) -> Result<(), String> {
    let runtime_path = find_runtime_c()?;

    let status = process::Command::new("cc")
        .args([&runtime_path, obj_path, "-o", bin_path, "-no-pie"])
        .status()
        .map_err(|e| format!("failed to run cc: {}", e))?;

    // Always clean up the intermediate .o file
    let _ = std::fs::remove_file(obj_path);

    if !status.success() {
        return Err(format!("linker exited with status {}", status));
    }

    Ok(())
}

/// Locate the C runtime file. Searches:
/// 1. RASK_RUNTIME_DIR environment variable
/// 2. Relative to the rask binary (walking up to find compiler/runtime/)
fn find_runtime_c() -> Result<String, String> {
    if let Ok(dir) = std::env::var("RASK_RUNTIME_DIR") {
        let p = Path::new(&dir).join("runtime.c");
        if p.exists() {
            return Ok(p.to_string_lossy().to_string());
        }
    }

    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let mut dir = exe_dir.to_path_buf();
            for _ in 0..5 {
                let candidate = dir.join("compiler").join("runtime").join("runtime.c");
                if candidate.exists() {
                    return Ok(candidate.to_string_lossy().to_string());
                }
                let candidate = dir.join("runtime").join("runtime.c");
                if candidate.exists() {
                    return Ok(candidate.to_string_lossy().to_string());
                }
                if !dir.pop() {
                    break;
                }
            }
        }
    }

    Err("Could not find runtime.c — set RASK_RUNTIME_DIR to the directory containing it".to_string())
}
