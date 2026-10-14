// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Which C compiler a target uses, and where it finds its headers.
//!
//! One answer for both questions that ask it: the linker builds the runtime and
//! `compile_c()` sources with this compiler (`structure.build/XC3`), and
//! `import c` reads headers from this compiler's search list. When the two
//! disagreed, a struct was laid out from one `<stdint.h>` and passed by value to
//! code built against another, and the by-value ABI reads its field offsets from
//! that layout (#1102).

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// A C compiler and the arguments that point it at the target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CCompiler {
    pub program: String,
    pub args: Vec<String>,
}

/// Whether `arch`/`os` (in the target table's vocabulary) is this machine.
pub fn is_host(arch: &str, os: &str) -> bool {
    arch == std::env::consts::ARCH && os == std::env::consts::OS
}

/// The C compiler for a target.
///
/// 1. `CC`, always
/// 2. a native build: `cc`
/// 3. `zig cc`, the cross compiler that needs no per-target install
/// 4. a target-prefixed gcc (`aarch64-linux-gnu-gcc`)
/// 5. on macOS, clang's `-arch` between x86_64 and aarch64
pub fn c_compiler(arch: &str, os: &str) -> Result<CCompiler, String> {
    if let Ok(cc) = std::env::var("CC") {
        return Ok(CCompiler { program: cc, args: vec![] });
    }
    if is_host(arch, os) {
        return Ok(CCompiler { program: "cc".into(), args: vec![] });
    }
    if answers("zig", &["cc", "--version"]) {
        return Ok(CCompiler {
            program: "zig".into(),
            args: vec!["cc".into(), format!("--target={}", zig_target(arch, os))],
        });
    }
    let prefix = gcc_prefix(arch, os);
    if let Some(pfx) = &prefix {
        let gcc = format!("{}-gcc", pfx);
        if answers(&gcc, &["--version"]) {
            return Ok(CCompiler { program: gcc, args: vec![] });
        }
    }
    if std::env::consts::OS == "macos" && os == "macos" {
        return Ok(CCompiler {
            program: "clang".into(),
            args: vec!["-arch".into(), clang_arch(arch).into()],
        });
    }
    let mut msg = format!(
        "cross-compilation to {}-{} requires a C cross-compiler\n\nInstall one of:\n  - zig (recommended): https://ziglang.org/download/\n",
        arch, os,
    );
    if let Some(pfx) = &prefix {
        msg.push_str(&format!("  - {}-gcc\n", pfx));
    }
    msg.push_str("  - set CC=<your-cross-compiler>");
    Err(msg)
}

/// Where `import c "x.h"` looks once the importing file's directory and the
/// path as written have both missed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderSearch {
    pub dirs: Vec<PathBuf>,
    /// Why the target's system headers aren't among `dirs`, when they aren't.
    pub no_system_headers: Option<String>,
}

impl HeaderSearch {
    /// The search list for a target, computed once per target per process:
    /// each answer is a process spawn, and it can't change mid-compilation.
    ///
    /// `CPATH` and `C_INCLUDE_PATH` come first, because that is what they mean
    /// to the compiler itself. Then the target compiler's own system list.
    ///
    /// A cross build with no cross compiler gets no system directories at all.
    /// The host's `/usr/include` is the wrong header for another target, and a
    /// layout read from it is silently wrong; a missing header says why.
    pub fn for_target(arch: &str, os: &str) -> HeaderSearch {
        use std::collections::HashMap;
        use std::sync::{Mutex, OnceLock};
        static CACHE: OnceLock<Mutex<HashMap<(String, String), HeaderSearch>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let key = (arch.to_string(), os.to_string());
        if let Some(found) = cache.lock().unwrap().get(&key) {
            return found.clone();
        }
        let search = Self::compute(arch, os);
        cache.lock().unwrap().insert(key, search.clone());
        search
    }

    pub fn for_host() -> HeaderSearch {
        Self::for_target(std::env::consts::ARCH, std::env::consts::OS)
    }

    fn compute(arch: &str, os: &str) -> HeaderSearch {
        let mut search = HeaderSearch::default();
        for var in ["CPATH", "C_INCLUDE_PATH"] {
            if let Ok(val) = std::env::var(var) {
                for part in val.split(':').filter(|p| !p.is_empty()) {
                    search.push(PathBuf::from(part));
                }
            }
        }
        let native = is_host(arch, os);
        let asked = match c_compiler(arch, os) {
            Ok(cc) => system_include_dirs(&cc),
            Err(_) => Vec::new(),
        };
        if !asked.is_empty() {
            for d in asked {
                search.push(d);
            }
        } else if native {
            // The compiler couldn't be reached at all. On the host, the usual
            // places are still the host's headers.
            for d in ["/usr/include", "/usr/local/include",
                      "/usr/include/x86_64-linux-gnu", "/usr/include/aarch64-linux-gnu"] {
                search.push(PathBuf::from(d));
            }
        } else {
            search.no_system_headers = Some(format!(
                "cross-compiling to {}-{}, and no C compiler for that target answered \
                 for its system headers; the host's would describe the wrong machine \
                 (set CC to a cross compiler, or install zig)",
                arch, os,
            ));
        }
        search
    }

    fn push(&mut self, d: PathBuf) {
        if !self.dirs.contains(&d) {
            self.dirs.push(d);
        }
    }
}

/// The compiler's own system header list, from `-E -Wp,-v` on empty input.
/// Every compiler that matters prints it to stderr between two fixed lines.
/// Empty when the compiler isn't there or answers in a shape this doesn't read.
pub fn system_include_dirs(cc: &CCompiler) -> Vec<PathBuf> {
    // `-` reads the translation unit from stdin, so this needs no temp file and
    // no `/dev/null` (which isn't one on every host).
    let Ok(mut child) = Command::new(&cc.program)
        .args(&cc.args)
        .args(["-E", "-Wp,-v", "-xc", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
    else {
        return Vec::new();
    };
    drop(child.stdin.take());
    let Ok(out) = child.wait_with_output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stderr);
    let mut dirs = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        if line.starts_with("#include <...> search starts here:") {
            inside = true;
            continue;
        }
        if line.starts_with("End of search list.") {
            break;
        }
        if inside {
            let d = line.trim();
            // clang appends " (framework directory)" to framework entries, which
            // are not header directories in this sense.
            if !d.is_empty() && !d.ends_with("(framework directory)") {
                dirs.push(PathBuf::from(d));
            }
        }
    }
    dirs
}

fn answers(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn zig_target(arch: &str, os: &str) -> String {
    let zig_os = match os {
        "macos" => "macos",
        "linux" => "linux-gnu",
        _ => os,
    };
    format!("{}-{}", arch, zig_os)
}

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

fn clang_arch(arch: &str) -> &str {
    match arch {
        "aarch64" => "arm64",
        other => other,
    }
}
