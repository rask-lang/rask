// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! BuildContext for build scripts — struct.build/BL1-BL3.
//!
//! When `func build(ctx: BuildContext)` runs in the interpreter,
//! method calls on `ctx` dispatch here. Mutable state (link flags,
//! extra objects) accumulates in `BuildState` and flows back to the
//! CLI after the build script finishes.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::value::Value;

/// Mutable state accumulated during build script execution.
/// The CLI reads this after the script finishes to configure linking.
#[derive(Debug, Clone)]
pub struct BuildState {
    pub package_name: String,
    pub package_version: String,
    pub package_dir: PathBuf,
    pub profile: String,
    pub target: String,
    pub host: String,
    pub gen_dir: PathBuf,
    pub out_dir: PathBuf,
    pub step_cache_dir: Option<PathBuf>,
    // Accumulated by build script methods
    pub link_libraries: Vec<String>,
    pub link_search_paths: Vec<String>,
    pub extra_objects: Vec<PathBuf>,
    pub declared_deps: Vec<PathBuf>,
    /// Tool version strings recorded for step cache keys.
    pub tool_versions: HashMap<String, String>,
}

impl BuildState {
    /// Create the `Value::Struct` that gets passed to `func build(ctx)`.
    pub fn to_value(&self) -> Value {
        let mut fields = HashMap::new();
        fields.insert("package_name".into(), Value::String(Arc::new(Mutex::new(self.package_name.clone()))));
        fields.insert("package_version".into(), Value::String(Arc::new(Mutex::new(self.package_version.clone()))));
        fields.insert("package_dir".into(), make_path(&self.package_dir));
        fields.insert("profile".into(), Value::String(Arc::new(Mutex::new(self.profile.clone()))));
        fields.insert("target".into(), Value::String(Arc::new(Mutex::new(self.target.clone()))));
        fields.insert("host".into(), Value::String(Arc::new(Mutex::new(self.host.clone()))));
        fields.insert("gen_dir".into(), make_path(&self.gen_dir));
        fields.insert("out_dir".into(), make_path(&self.out_dir));

        Value::Struct {
            name: "BuildContext".into(),
            fields,
            resource_id: None,
        }
    }
}

/// Create a Path struct value from a PathBuf.
fn make_path(p: &PathBuf) -> Value {
    let mut fields = HashMap::new();
    fields.insert(
        "value".into(),
        Value::String(Arc::new(Mutex::new(p.to_string_lossy().into_owned()))),
    );
    Value::Struct { name: "Path".into(), fields, resource_id: None }
}

/// Extract a string from a Value, handling both String and Path structs.
fn expect_string(val: &Value, context: &str) -> Result<String, String> {
    match val {
        Value::String(s) => Ok(s.lock().unwrap().clone()),
        Value::Struct { name, fields, .. } if name == "Path" => {
            match fields.get("value") {
                Some(Value::String(s)) => Ok(s.lock().unwrap().clone()),
                _ => Err(format!("{}: expected string or Path", context)),
            }
        }
        other => Err(format!("{}: expected string, got {}", context, other.type_name())),
    }
}

/// Extract a Vec<String> from a Value::Vec.
fn expect_string_vec(val: &Value, context: &str) -> Result<Vec<String>, String> {
    match val {
        Value::Vec(v) => {
            let items = v.lock().unwrap();
            items.iter().map(|item| expect_string(item, context)).collect()
        }
        _ => Err(format!("{}: expected Vec<string>", context)),
    }
}

/// Dispatch a method call on BuildContext.
/// `state` is the mutable BuildState on the Interpreter.
pub fn call_method(
    state: &mut BuildState,
    method: &str,
    args: Vec<Value>,
) -> Result<Value, String> {
    match method {
        "write_source" => {
            // write_source(name: string, code: string)
            if args.len() != 2 {
                return Err(format!("write_source expects 2 args, got {}", args.len()));
            }
            let name = expect_string(&args[0], "write_source name")?;
            let code = expect_string(&args[1], "write_source code")?;

            let path = state.gen_dir.join(&name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("write_source: create dir: {}", e))?;
            }
            std::fs::write(&path, &code)
                .map_err(|e| format!("write_source: {}", e))?;

            Ok(Value::Unit)
        }

        "write_file" => {
            // write_file(name: string, data: string)
            if args.len() != 2 {
                return Err(format!("write_file expects 2 args, got {}", args.len()));
            }
            let name = expect_string(&args[0], "write_file name")?;
            let data = expect_string(&args[1], "write_file data")?;

            let path = state.out_dir.join(&name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("write_file: create dir: {}", e))?;
            }
            std::fs::write(&path, data.as_bytes())
                .map_err(|e| format!("write_file: {}", e))?;

            Ok(Value::Unit)
        }

        "declare_dependency" => {
            // declare_dependency(path: string)
            if args.len() != 1 {
                return Err(format!("declare_dependency expects 1 arg, got {}", args.len()));
            }
            let path_str = expect_string(&args[0], "declare_dependency path")?;
            let dep_path = state.package_dir.join(&path_str);
            state.declared_deps.push(dep_path);
            Ok(Value::Unit)
        }

        "env" => {
            // env(name: string) -> string?
            if args.len() != 1 {
                return Err(format!("env expects 1 arg, got {}", args.len()));
            }
            let name = expect_string(&args[0], "env name")?;
            match std::env::var(&name) {
                Ok(val) => Ok(Value::Enum {
                    name: "Option".into(),
                    variant: "Some".into(),
                    fields: vec![Value::String(Arc::new(Mutex::new(val)))],
                }),
                Err(_) => Ok(Value::Enum {
                    name: "Option".into(),
                    variant: "None".into(),
                    fields: vec![],
                }),
            }
        }

        "warning" => {
            // warning(msg: string)
            if args.len() != 1 {
                return Err(format!("warning expects 1 arg, got {}", args.len()));
            }
            let msg = expect_string(&args[0], "warning msg")?;
            eprintln!("warning: {}", msg);
            Ok(Value::Unit)
        }

        "exec" => {
            // exec(program: string, args: [string]) -> () or Error
            if args.len() != 2 {
                return Err(format!("exec expects 2 args, got {}", args.len()));
            }
            let program = expect_string(&args[0], "exec program")?;
            let cmd_args = expect_string_vec(&args[1], "exec args")?;

            let status = std::process::Command::new(&program)
                .args(&cmd_args)
                .current_dir(&state.package_dir)
                .status()
                .map_err(|e| format!("exec: failed to run '{}': {}", program, e))?;

            if status.success() {
                Ok(Value::Unit)
            } else {
                Err(format!("exec: '{}' exited with status {}", program, status))
            }
        }

        "exec_output" => {
            // exec_output(program: string, args: [string]) -> string or Error
            if args.len() != 2 {
                return Err(format!("exec_output expects 2 args, got {}", args.len()));
            }
            let program = expect_string(&args[0], "exec_output program")?;
            let cmd_args = expect_string_vec(&args[1], "exec_output args")?;

            let output = std::process::Command::new(&program)
                .args(&cmd_args)
                .current_dir(&state.package_dir)
                .output()
                .map_err(|e| format!("exec_output: failed to run '{}': {}", program, e))?;

            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                Ok(Value::String(Arc::new(Mutex::new(stdout))))
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                Err(format!("exec_output: '{}' failed: {}", program, stderr))
            }
        }

        "find_program" => {
            // find_program(name: string) -> Path?
            if args.len() != 1 {
                return Err(format!("find_program expects 1 arg, got {}", args.len()));
            }
            let name = expect_string(&args[0], "find_program name")?;

            // Search PATH for the program
            if let Ok(path_env) = std::env::var("PATH") {
                for dir in std::env::split_paths(&path_env) {
                    let candidate = dir.join(&name);
                    if candidate.is_file() {
                        return Ok(Value::Enum {
                            name: "Option".into(),
                            variant: "Some".into(),
                            fields: vec![make_path(&candidate)],
                        });
                    }
                }
            }
            Ok(Value::Enum {
                name: "Option".into(),
                variant: "None".into(),
                fields: vec![],
            })
        }

        "is_cross_compiling" => {
            // is_cross_compiling() -> bool
            Ok(Value::Bool(state.target != state.host))
        }

        // === Phase 2: Native compilation methods ===

        "compile_c" => {
            // compile_c(sources: [string], flags: [string])
            if args.len() != 2 {
                return Err(format!("compile_c expects 2 args, got {}", args.len()));
            }
            let sources = expect_string_vec(&args[0], "compile_c sources")?;
            let flags = expect_string_vec(&args[1], "compile_c flags")?;

            for source in &sources {
                let src_path = state.package_dir.join(source);
                if !src_path.exists() {
                    return Err(format!("compile_c: source not found: {}", src_path.display()));
                }

                // Derive output name: foo.c -> foo.o in out_dir
                let stem = std::path::Path::new(source)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("out");
                let obj_path = state.out_dir.join(format!("{}.o", stem));

                let mut cmd = std::process::Command::new("cc");
                cmd.arg("-c")
                    .arg(&src_path)
                    .arg("-o")
                    .arg(&obj_path);
                for flag in &flags {
                    cmd.arg(flag);
                }
                cmd.current_dir(&state.package_dir);

                let status = cmd.status()
                    .map_err(|e| format!("compile_c: failed to run cc: {}", e))?;

                if !status.success() {
                    return Err(format!("compile_c: cc failed for '{}'", source));
                }

                state.extra_objects.push(obj_path);
            }

            Ok(Value::Unit)
        }

        "link_library" => {
            // link_library(name: string)
            if args.len() != 1 {
                return Err(format!("link_library expects 1 arg, got {}", args.len()));
            }
            let name = expect_string(&args[0], "link_library name")?;
            state.link_libraries.push(name);
            Ok(Value::Unit)
        }

        "link_search_path" => {
            // link_search_path(path: string)
            if args.len() != 1 {
                return Err(format!("link_search_path expects 1 arg, got {}", args.len()));
            }
            let path = expect_string(&args[0], "link_search_path path")?;
            state.link_search_paths.push(path);
            Ok(Value::Unit)
        }

        "pkg_config" => {
            // pkg_config(name: string) — query pkg-config for flags
            if args.len() != 1 {
                return Err(format!("pkg_config expects 1 arg, got {}", args.len()));
            }
            let name = expect_string(&args[0], "pkg_config name")?;

            let output = std::process::Command::new("pkg-config")
                .args(["--cflags", "--libs", &name])
                .output()
                .map_err(|e| format!("pkg_config: failed to run pkg-config: {}", e))?;

            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(format!("pkg_config: '{}' not found: {}", name, stderr));
            }

            let flags = String::from_utf8_lossy(&output.stdout);
            for flag in flags.split_whitespace() {
                if let Some(lib) = flag.strip_prefix("-l") {
                    state.link_libraries.push(lib.to_string());
                } else if let Some(path) = flag.strip_prefix("-L") {
                    state.link_search_paths.push(path.to_string());
                }
                // -I flags are for C include paths — not needed for Rask linking
            }

            Ok(Value::Unit)
        }

        "tool_version" => {
            // tool_version(program: string, flag: string) -> string
            if args.len() != 2 {
                return Err(format!("tool_version expects 2 args, got {}", args.len()));
            }
            let program = expect_string(&args[0], "tool_version program")?;
            let flag = expect_string(&args[1], "tool_version flag")?;

            let output = std::process::Command::new(&program)
                .arg(&flag)
                .output()
                .map_err(|e| format!("tool_version: failed to run '{}': {}", program, e))?;

            let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
            state.tool_versions.insert(program, version.clone());
            Ok(Value::String(Arc::new(Mutex::new(version))))
        }

        // "step" is handled in dispatch.rs (needs closure execution)
        "step" => Err("step() must be called on BuildContext (internal error)".into()),

        _ => Err(format!("BuildContext has no method '{}'", method)),
    }
}

/// Compute a content hash of input files for step caching.
/// Uses DefaultHasher (not cryptographic — fine for local cache invalidation).
pub fn hash_inputs(
    base_dir: &PathBuf,
    input_patterns: &[String],
    tool_versions: &HashMap<String, String>,
) -> Result<u64, String> {
    use std::collections::BTreeMap;
    use std::hash::{Hash, Hasher};
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();

    // Collect and sort all input files for deterministic hashing
    let mut files: BTreeMap<PathBuf, Vec<u8>> = BTreeMap::new();

    for pattern in input_patterns {
        let abs_pattern = if std::path::Path::new(pattern).is_absolute() {
            pattern.clone()
        } else {
            base_dir.join(pattern).to_string_lossy().into_owned()
        };

        // Simple glob: if pattern contains *, expand; otherwise treat as literal
        if abs_pattern.contains('*') {
            if let Ok(entries) = glob_files(&abs_pattern) {
                for path in entries {
                    if let Ok(content) = std::fs::read(&path) {
                        files.insert(path, content);
                    }
                }
            }
        } else {
            let path = PathBuf::from(&abs_pattern);
            if let Ok(content) = std::fs::read(&path) {
                files.insert(path, content);
            }
        }
    }

    for (path, content) in &files {
        path.hash(&mut hasher);
        content.hash(&mut hasher);
    }

    // Include tool versions in hash
    let sorted_versions: BTreeMap<_, _> = tool_versions.iter().collect();
    for (tool, version) in &sorted_versions {
        tool.hash(&mut hasher);
        version.hash(&mut hasher);
    }

    Ok(hasher.finish())
}

/// Load cached hash for a build step.
pub fn load_step_hash(cache_dir: &PathBuf, step_name: &str) -> Option<u64> {
    let hash_file = cache_dir.join(format!("{}.hash", step_name));
    std::fs::read_to_string(&hash_file).ok()?.trim().parse().ok()
}

/// Save hash for a build step.
pub fn save_step_hash(cache_dir: &PathBuf, step_name: &str, hash: u64) -> Result<(), String> {
    std::fs::create_dir_all(cache_dir)
        .map_err(|e| format!("failed to create step cache dir: {}", e))?;
    let hash_file = cache_dir.join(format!("{}.hash", step_name));
    std::fs::write(&hash_file, hash.to_string())
        .map_err(|e| format!("failed to write step hash: {}", e))
}

/// Simple glob expansion for input patterns.
fn glob_files(pattern: &str) -> Result<Vec<PathBuf>, String> {
    // Split into directory prefix and filename pattern
    let path = std::path::Path::new(pattern);
    let parent = path.parent().unwrap_or(std::path::Path::new("."));
    let file_pattern = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("*");

    let mut results = Vec::new();

    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if matches_glob(file_pattern, &name_str) && entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                results.push(entry.path());
            }
        }
    }

    Ok(results)
}

/// Simple glob matching (only supports * wildcard).
fn matches_glob(pattern: &str, name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(suffix) = pattern.strip_prefix('*') {
        return name.ends_with(suffix);
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return name.starts_with(prefix);
    }
    pattern == name
}
