// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Shared linking utilities for compile and build commands.

use std::path::Path;
use std::process;

/// Find the runtime C files, compile them, and link with the object file.
pub fn link_executable(obj_path: &str, bin_path: &str) -> Result<(), String> {
    let runtime_dir = find_runtime_dir()?;
    let runtime_c = runtime_dir.join("runtime.c");
    let args_c = runtime_dir.join("args.c");

    if !args_c.exists() {
        return Err(format!(
            "missing args.c in {} — runtime is incomplete",
            runtime_dir.display()
        ));
    }

    let status = process::Command::new("cc")
        .arg(&runtime_c)
        .arg(&args_c)
        .arg(obj_path)
        .args(["-o", bin_path, "-no-pie"])
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
fn find_runtime_dir() -> Result<std::path::PathBuf, String> {
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
