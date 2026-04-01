// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Integration tests for `rask compile` and `rask run --native`.
//! Each test compiles a .rk fixture to a native executable, runs it,
//! and checks stdout against expected output.

use std::path::{Path, PathBuf};
use std::process::Command;

fn rask_binary() -> PathBuf {
    // cargo test builds into target/debug or target/release
    let mut path = std::env::current_exe().unwrap();
    // Walk up from the test binary to the target dir
    path.pop(); // remove test binary name
    if path.ends_with("deps") {
        path.pop();
    }
    path.push("rask");
    path
}

fn runtime_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("runtime")
}

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

/// Compile a .rk file and run the resulting binary, returning stdout.
fn compile_and_run(fixture_name: &str) -> (String, i32) {
    let rask = rask_binary();
    let tmp = std::env::temp_dir();
    let stem = fixture_name.trim_end_matches(".rk");
    let bin_path = tmp.join(format!("rask_test_{}_{}", stem, std::process::id()));

    // Compile
    let compile_out = Command::new(&rask)
        .arg("compile")
        .arg(fixture(fixture_name))
        .arg("-o")
        .arg(&bin_path)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask compile");

    assert!(
        compile_out.status.success(),
        "rask compile {} failed:\nstdout: {}\nstderr: {}",
        fixture_name,
        String::from_utf8_lossy(&compile_out.stdout),
        String::from_utf8_lossy(&compile_out.stderr),
    );

    // Run the compiled binary
    let run_out = Command::new(&bin_path)
        .output()
        .expect("failed to run compiled binary");

    // Clean up
    let _ = std::fs::remove_file(&bin_path);

    let stdout = String::from_utf8_lossy(&run_out.stdout).to_string();
    let code = run_out.status.code().unwrap_or(-1);
    (stdout, code)
}

/// Compile via `rask run --native`, returning stdout.
fn run_native(fixture_name: &str) -> (String, i32) {
    let rask = rask_binary();

    let out = Command::new(&rask)
        .args(["run", "--native"])
        .arg(fixture(fixture_name))
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask run --native");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let code = out.status.code().unwrap_or(-1);
    (stdout, code)
}

// ─── rask compile tests ──────────────────────────────────────

#[test]
fn compile_hello() {
    let (stdout, code) = compile_and_run("hello.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "Hello, World!\n");
}

#[test]
fn compile_arithmetic() {
    let (stdout, code) = compile_and_run("arithmetic.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn compile_print_types() {
    let (stdout, code) = compile_and_run("print_types.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42 true hello\n");
}

#[test]
fn compile_multi_func() {
    let (stdout, code) = compile_and_run("multi_func.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "25\n");
}

#[test]
fn compile_exit_zero() {
    let (stdout, code) = compile_and_run("exit_zero.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "");
}

// ─── rask run --native tests ────────────────────────────────

#[test]
fn run_native_hello() {
    let (stdout, code) = run_native("hello.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "Hello, World!\n");
}

#[test]
fn run_native_arithmetic() {
    let (stdout, code) = run_native("arithmetic.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn run_native_multi_func() {
    let (stdout, code) = run_native("multi_func.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "25\n");
}

// ─── Copy semantics tests ─────────────────────────────────

#[test]
fn compile_copy_rebind() {
    let (stdout, code) = compile_and_run("copy_rebind.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42 42\n");
}

#[test]
fn run_native_copy_rebind() {
    let (stdout, code) = run_native("copy_rebind.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42 42\n");
}

// ─── Native codegen: structs, enums, closures, strings ──────

#[test]
fn compile_structs() {
    let (stdout, code) = compile_and_run("structs.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn compile_enums() {
    let (stdout, code) = compile_and_run("enums.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "75\n24\n");
}

#[test]
fn compile_closures() {
    let (stdout, code) = compile_and_run("closures.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "42\n");
}

#[test]
fn compile_strings() {
    let (stdout, code) = compile_and_run("strings.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "hello world\n5\n");
}

#[test]
fn compile_control_flow() {
    let (stdout, code) = compile_and_run("control_flow.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "55\n10\n");
}

#[test]
fn compile_vec_basic() {
    let (stdout, code) = compile_and_run("vec_basic.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "3\n");
}

// ─── Compile-error tests (should fail to compile) ────────────

fn compile_error(name: &str) -> bool {
    let rask = rask_binary();
    let error_fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("tests")
        .join("compile_errors")
        .join(name);

    let out = Command::new(&rask)
        .arg("check")
        .arg(&error_fixture)
        .output()
        .expect("failed to run rask check");

    // Should NOT succeed — return true if it correctly fails
    !out.status.success()
}

#[test]
fn error_type_mismatch_arg() {
    assert!(compile_error("type_mismatch_arg.rk"), "should reject type mismatch in argument");
}

#[test]
fn error_type_mismatch_return() {
    assert!(compile_error("type_mismatch_return.rk"), "should reject return type mismatch");
}

#[test]
fn error_undefined_variable() {
    assert!(compile_error("undefined_variable.rk"), "should reject undefined variable");
}

#[test]
fn error_wrong_arg_count() {
    assert!(compile_error("wrong_arg_count.rk"), "should reject wrong argument count");
}

#[test]
fn error_const_reassign() {
    assert!(compile_error("const_reassign.rk"), "should reject const reassignment");
}

#[test]
fn error_nonexhaustive_match() {
    assert!(compile_error("nonexhaustive_match.rk"), "should reject non-exhaustive match");
}

#[test]
fn error_missing_return() {
    assert!(compile_error("missing_return.rk"), "should reject missing return");
}

// ─── Error message quality ──────────────────────────────────

/// Run `rask check` and return combined stdout+stderr.
fn check_output(source: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let rask = rask_binary();
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rask_errtest_{}_{}.rk", std::process::id(), id));
    std::fs::write(&tmp, source).unwrap();

    let out = Command::new(&rask)
        .arg("check")
        .arg(&tmp)
        .output()
        .expect("failed to run rask check");

    let _ = std::fs::remove_file(&tmp);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    format!("{}{}", stdout, stderr)
}

#[test]
fn error_message_includes_line_number() {
    let output = check_output("func main() {\n    const x: i32 = \"hello\"\n}");
    assert!(output.contains("E0308"), "should include error code");
    assert!(output.contains(":2:"), "should include line number");
}

#[test]
fn error_message_shows_mismatched_types() {
    let output = check_output("func add(a: i32, b: i32) -> i32 { return a + b }\nfunc main() { add(1, \"x\") }");
    assert!(output.contains("mismatched"), "should mention mismatched types: {}", output);
}

#[test]
fn error_message_shows_undefined_symbol() {
    let output = check_output("func main() { println(x.to_string()) }");
    assert!(output.contains("undefined"), "should mention undefined: {}", output);
    assert!(output.contains("x"), "should mention the symbol name: {}", output);
}

#[test]
fn error_message_includes_fix_hint() {
    let output = check_output("func main() {\n    const x: i32 = \"hello\"\n}");
    assert!(output.contains("fix:"), "should include fix suggestion: {}", output);
}

// ─── rask fmt integration ───────────────────────────────────

#[test]
fn fmt_normalizes_spacing() {
    let rask = rask_binary();
    let tmp = std::env::temp_dir().join(format!("rask_fmttest_{}.rk", std::process::id()));
    std::fs::write(&tmp, "func    main(   ) {\nconst x=42\n}").unwrap();

    let _ = Command::new(&rask)
        .arg("fmt")
        .arg("-w")
        .arg(&tmp)
        .output()
        .expect("failed to run rask fmt");

    let formatted = std::fs::read_to_string(&tmp).unwrap();
    let _ = std::fs::remove_file(&tmp);

    assert!(formatted.contains("func main()"), "should normalize func spacing: {}", formatted);
    assert!(formatted.contains("const x = 42"), "should add spaces: {}", formatted);
}

// ─── rask lint integration ──────────────────────────────────

fn lint_output(source: &str) -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let rask = rask_binary();
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rask_linttest_{}_{}.rk", std::process::id(), id));
    std::fs::write(&tmp, source).unwrap();

    let out = Command::new(&rask)
        .arg("lint")
        .arg(&tmp)
        .output()
        .expect("failed to run rask lint");

    let _ = std::fs::remove_file(&tmp);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    format!("{}{}", stdout, stderr)
}

#[test]
fn lint_flags_camel_case_function() {
    let output = lint_output("func getData() -> i32 { return 1 }\nfunc main() {}");
    assert!(output.contains("snake_case") || output.contains("getData"),
        "should flag camelCase function: {}", output);
}

#[test]
fn lint_clean_code_passes() {
    let output = lint_output("func get_data() -> i32 { return 1 }\nfunc main() {}");
    assert!(output.contains("No lint issues") || !output.contains("warning"),
        "clean code should pass lint: {}", output);
}

// ─── rask api integration ───────────────────────────────────

#[test]
fn api_shows_vec_methods() {
    let rask = rask_binary();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("stdlib")
        .join("collections.rk");

    let out = Command::new(&rask)
        .arg("api")
        .arg(&stdlib)
        .output()
        .expect("failed to run rask api");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Vec"), "should show Vec type: {}", stdout);
    assert!(stdout.contains("push"), "should show push method: {}", stdout);
    assert!(stdout.contains("pop"), "should show pop method: {}", stdout);
    assert!(stdout.contains("len"), "should show len method: {}", stdout);
}

#[test]
fn api_shows_map_methods() {
    let rask = rask_binary();
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("..")
        .join("stdlib")
        .join("collections.rk");

    let out = Command::new(&rask)
        .arg("api")
        .arg(&stdlib)
        .output()
        .expect("failed to run rask api");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Map"), "should show Map type: {}", stdout);
    assert!(stdout.contains("insert"), "should show insert method: {}", stdout);
    assert!(stdout.contains("contains_key"), "should show contains_key method: {}", stdout);
}

// ─── Stdlib method discoverability via type checker ─────────
// Verify that calling stdlib methods actually passes type checking.
// This catches stubs that exist but aren't wired into the resolver.

fn check_succeeds(source: &str) -> bool {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let rask = rask_binary();
    let id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("rask_disctest_{}_{}.rk", std::process::id(), id));
    std::fs::write(&tmp, source).unwrap();

    let out = Command::new(&rask)
        .arg("check")
        .arg(&tmp)
        .output()
        .expect("failed to run rask check");

    let _ = std::fs::remove_file(&tmp);
    out.status.success()
}

#[test]
fn discover_vec_push_len() {
    assert!(check_succeeds(
        "func main() {\n    const v = Vec<i32>.new()\n    v.push(1)\n    println(v.len().to_string())\n}"
    ), "Vec.new/push/len should pass type check");
}

#[test]
fn discover_vec_pop() {
    assert!(check_succeeds(
        "func main() {\n    const v = Vec<i32>.new()\n    v.push(1)\n    v.pop()\n}"
    ), "Vec.pop should pass type check");
}

#[test]
fn discover_string_len_contains() {
    assert!(check_succeeds(
        "func main() {\n    const s = \"hello\"\n    println(s.len().to_string())\n    s.contains(\"ell\")\n}"
    ), "string.len/contains should pass type check");
}

#[test]
fn discover_string_trim() {
    // string.trim() returns a slice — can't store it (S2), but can use inline
    assert!(check_succeeds(
        "func main() {\n    const s = \"  hello  \"\n    println(s.trim())\n}"
    ), "string.trim should pass type check");
}

#[test]
fn discover_map_insert_len() {
    assert!(check_succeeds(
        "func main() {\n    const m = Map<string, i32>.new()\n    m.insert(\"a\", 1)\n    println(m.len().to_string())\n}"
    ), "Map.new/insert/len should pass type check");
}

#[test]
fn discover_map_contains_key() {
    assert!(check_succeeds(
        "func main() {\n    const m = Map<string, i32>.new()\n    m.insert(\"a\", 1)\n    m.contains_key(\"a\")\n}"
    ), "Map.contains_key should pass type check");
}

#[test]
fn discover_println_print() {
    assert!(check_succeeds(
        "func main() {\n    println(\"hello\")\n    print(\"world\")\n}"
    ), "println/print should pass type check");
}

#[test]
fn discover_to_string() {
    assert!(check_succeeds(
        "func main() {\n    const s = 42.to_string()\n    println(s)\n}"
    ), "i32.to_string should pass type check");
}
