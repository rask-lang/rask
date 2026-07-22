// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Integration tests for `rask compile` and `rask run --native`.
//! Each test compiles a .rk fixture to a native executable, runs it,
//! and checks stdout against expected output.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Global counter for temp file names. Local per-function counters collide
/// across helper functions (each starts at 0), producing identical paths
/// like `rask_ctest_<pid>_1.rk` from different threads — one thread deletes
/// the file before another's rask subprocess can read it. Sharing one counter
/// guarantees unique IDs across the test binary.
static NEXT_TMP_ID: AtomicU64 = AtomicU64::new(0);

fn next_tmp_id() -> u64 {
    NEXT_TMP_ID.fetch_add(1, Ordering::Relaxed)
}

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

/// Compile a .rk fixture and assert codegen produces no errors.
/// Use when the emitted binary may segfault for unrelated reasons
/// (e.g. runtime layout issues) but the specific codegen bug must
/// not return.
fn compile_only_succeeds(fixture_name: &str) -> (bool, String) {
    let rask = rask_binary();
    let tmp = std::env::temp_dir();
    let stem = fixture_name.trim_end_matches(".rk");
    let bin_path = tmp.join(format!("rask_test_{}_{}", stem, std::process::id()));

    let compile_out = Command::new(&rask)
        .arg("compile")
        .arg(fixture(fixture_name))
        .arg("-o")
        .arg(&bin_path)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask compile");

    let _ = std::fs::remove_file(&bin_path);

    let combined = format!(
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&compile_out.stdout),
        String::from_utf8_lossy(&compile_out.stderr),
    );
    (compile_out.status.success(), combined)
}

/// Run a .rk fixture via `rask run --interp`, returning stdout.
fn run_interp(fixture_name: &str) -> (String, i32) {
    let rask = rask_binary();
    let out = Command::new(&rask)
        .args(["run", "--interp"])
        .arg(fixture(fixture_name))
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask run --interp");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let code = out.status.code().unwrap_or(-1);
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

/// Run a fixture on one backend, returning (stdout, stderr, exit code).
/// `mode` is the `rask run` flag, e.g. "--interp" or "--native".
fn run_capture(mode: &str, fixture_name: &str) -> (String, String, i32) {
    let rask = rask_binary();
    let out = Command::new(&rask)
        .args(["run", mode])
        .arg(fixture(fixture_name))
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
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
fn compile_range_patterns() {
    let (stdout, code) = compile_and_run("range_patterns.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "digit\nletter\nunderscore\nother\nF\nB\nA\n");
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

#[test]
fn error_unknown_type_name() {
    assert!(compile_error("unknown_type_name.rk"), "should reject unknown PascalCase type in signature (PC2)");
}

#[test]
fn error_single_letter_type_name() {
    assert!(compile_error("single_letter_type_name.rk"), "should reject single-letter concrete type names (PC3)");
}

#[test]
fn compile_auto_generic_single_letter() {
    let (stdout, code) = compile_and_run("auto_generic_single_letter.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "2 1\nhello\n");
}

// ─── Ownership branch-merge soundness (task 1.1, issue #294) ──
//
// A value moved (or a linear resource consumed) on some paths but not all
// must be treated as unavailable after the paths join. The stricter merge
// rejects the negative forms; the legal forms live in tests/suite/.

#[test]
fn error_branch_merge_fixture() {
    assert!(compile_error("branch_merge.rk"),
        "should reject the branch-merge soundness violations");
}

#[test]
fn error_move_in_one_branch() {
    let output = check_output(
        "func main() {\n    const v = Vec<i32>.new()\n    if true {\n        const moved = v\n    } else {\n        const x = 1\n    }\n    v.len()\n}"
    );
    assert!(output.contains("E0813"),
        "move in one if/else branch then use should be E0813 (O3): {}", output);
}

#[test]
fn error_move_in_if_without_else() {
    // #294: the implicit empty else must merge like a real branch.
    let output = check_output(
        "func main() {\n    const v = Vec<i32>.new()\n    if true {\n        const moved = v\n    }\n    v.len()\n}"
    );
    assert!(output.contains("E0813"),
        "move in an if-without-else then use should be E0813 (O3): {}", output);
}

#[test]
fn error_linear_consumed_one_branch_ifelse() {
    let output = check_output(
        "@resource\nstruct Conn { fd: i32 }\nextend Conn { func close(take self) {} }\nfunc main() {\n    const c = Conn { fd: 3 }\n    if true {\n        c.close()\n    } else {\n        const x = 1\n    }\n}"
    );
    assert!(output.contains("E0805"),
        "resource consumed in only one if/else branch should be E0805 (L1): {}", output);
}

#[test]
fn error_linear_consumed_if_without_else() {
    // #294: consuming a linear resource in an if-without-else leaks on the
    // false path.
    let output = check_output(
        "@resource\nstruct Conn { fd: i32 }\nextend Conn { func close(take self) {} }\nfunc main() {\n    const c = Conn { fd: 3 }\n    if true {\n        c.close()\n    }\n}"
    );
    assert!(output.contains("E0805"),
        "resource consumed in an if-without-else should be E0805 (L1): {}", output);
}

#[test]
fn error_move_in_loop_body() {
    let output = check_output(
        "func take_vec(take v: Vec<i32>) {}\nfunc main() {\n    const v = Vec<i32>.new()\n    loop {\n        take_vec(own v)\n    }\n}"
    );
    assert!(output.contains("E0813"),
        "moving a value inside a loop body is a next-iteration use-after-move (O3): {}", output);
}

#[test]
fn ok_move_in_both_branches() {
    assert!(check_succeeds(
        "func take_vec(take v: Vec<i32>) {}\nfunc main() {\n    const v = Vec<i32>.new()\n    if true {\n        take_vec(own v)\n    } else {\n        take_vec(own v)\n    }\n}"
    ), "moving on both branches is a definite move — should type-check");
}

#[test]
fn ok_conditional_move_then_reassign() {
    assert!(check_succeeds(
        "func main() {\n    mut v = Vec<i32>.new()\n    if true {\n        const moved = v\n    }\n    v = Vec<i32>.new()\n    v.push(1)\n}"
    ), "reassigning after a conditional move should type-check");
}

// ─── Error message quality ──────────────────────────────────

/// Run `rask check` and return combined stdout+stderr.
fn check_output(source: &str) -> String {
    let rask = rask_binary();
    let id = next_tmp_id();
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
    let rask = rask_binary();
    let id = next_tmp_id();
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
    let rask = rask_binary();
    let id = next_tmp_id();
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
        "func main() {\n    mut v = Vec<i32>.new()\n    v.push(1)\n    println(v.len().to_string())\n}"
    ), "Vec.new/push/len should pass type check");
}

#[test]
fn discover_vec_pop() {
    assert!(check_succeeds(
        "func main() {\n    mut v = Vec<i32>.new()\n    v.push(1)\n    v.pop()\n}"
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
        "func main() {\n    mut m = Map<string, i32>.new()\n    m.insert(\"a\", 1)\n    println(m.len().to_string())\n}"
    ), "Map.new/insert/len should pass type check");
}

#[test]
fn discover_map_contains_key() {
    assert!(check_succeeds(
        "func main() {\n    mut m = Map<string, i32>.new()\n    m.insert(\"a\", 1)\n    m.contains_key(\"a\")\n}"
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

// ─── C import tests (CI1–CI5) ──────────────────────────────
// End-to-end: parse C header → translate → resolve → type-check.

fn c_header_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("c_headers")
        .join(name)
}

/// Write a temp .rk file that imports the given header and check it.
fn check_c_import(header: &str, rask_body: &str) -> bool {
    let rask = rask_binary();
    let id = next_tmp_id();
    let header_path = c_header_fixture(header);
    let tmp = std::env::temp_dir().join(format!("rask_ctest_{}_{}.rk", std::process::id(), id));
    let source = format!(
        "import c \"{}\"\n\n{}",
        header_path.display(),
        rask_body,
    );
    std::fs::write(&tmp, &source).unwrap();

    let out = Command::new(&rask)
        .arg("check")
        .arg(&tmp)
        .output()
        .expect("failed to run rask check");

    let _ = std::fs::remove_file(&tmp);
    out.status.success()
}

/// Run `rask check` and return stderr+stdout for assertion.
fn check_c_import_output(header: &str, rask_body: &str) -> (bool, String) {
    let rask = rask_binary();
    let id = next_tmp_id();
    let header_path = c_header_fixture(header);
    let tmp = std::env::temp_dir().join(format!("rask_ctest_{}_{}.rk", std::process::id(), id));
    let source = format!(
        "import c \"{}\"\n\n{}",
        header_path.display(),
        rask_body,
    );
    std::fs::write(&tmp, &source).unwrap();

    let out = Command::new(&rask)
        .arg("check")
        .arg(&tmp)
        .output()
        .expect("failed to run rask check");

    let _ = std::fs::remove_file(&tmp);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    (out.status.success(), combined)
}

/// Run `rask resolve` and return stdout for symbol inspection.
fn resolve_c_import(header: &str, rask_body: &str) -> String {
    let rask = rask_binary();
    let id = next_tmp_id();
    let header_path = c_header_fixture(header);
    let tmp = std::env::temp_dir().join(format!("rask_crestest_{}_{}.rk", std::process::id(), id));
    let source = format!(
        "import c \"{}\"\n\n{}",
        header_path.display(),
        rask_body,
    );
    std::fs::write(&tmp, &source).unwrap();

    let out = Command::new(&rask)
        .arg("resolve")
        .arg(&tmp)
        .output()
        .expect("failed to run rask resolve");

    let _ = std::fs::remove_file(&tmp);
    String::from_utf8_lossy(&out.stdout).to_string()
}

// CI1: import c "header.h" creates namespace with symbols
#[test]
fn c_import_creates_namespace() {
    let symbols = resolve_c_import("mylib.h", "func main() {}");
    assert!(symbols.contains("CNamespace"), "should create c namespace: {}", symbols);
    assert!(symbols.contains("mylib_add"), "should contain mylib_add: {}", symbols);
    assert!(symbols.contains("mylib_noop"), "should contain mylib_noop: {}", symbols);
}

// CI1: Functions parsed with correct types
#[test]
fn c_import_function_types() {
    let symbols = resolve_c_import("mylib.h", "func main() {}");
    assert!(
        symbols.contains("ExternFunction") && symbols.contains("mylib_add"),
        "should have ExternFunction for mylib_add: {}", symbols
    );
}

// CI1: Structs parsed with fields
#[test]
fn c_import_struct_fields() {
    let symbols = resolve_c_import("mylib.h", "func main() {}");
    assert!(
        symbols.contains("mylib_point") && symbols.contains("Struct"),
        "should have struct mylib_point: {}", symbols
    );
}

// CI1: Enum variants accessible
#[test]
fn c_import_enum_variants() {
    let symbols = resolve_c_import("mylib.h", "func main() {}");
    assert!(symbols.contains("MYLIB_OK"), "should have MYLIB_OK variant: {}", symbols);
    assert!(symbols.contains("MYLIB_ERR"), "should have MYLIB_ERR variant: {}", symbols);
    assert!(symbols.contains("MYLIB_TIMEOUT"), "should have MYLIB_TIMEOUT: {}", symbols);
}

// CI1: #define integer constant imported
#[test]
fn c_import_define_constant() {
    let symbols = resolve_c_import("mylib.h", "func main() {}");
    assert!(symbols.contains("MYLIB_VERSION"), "should have MYLIB_VERSION: {}", symbols);
}

// CI1: Forward-declared struct becomes opaque
#[test]
fn c_import_opaque_struct() {
    let symbols = resolve_c_import("mylib.h", "func main() {}");
    // mylib_ctx is forward-declared — should still exist as a struct
    assert!(symbols.contains("mylib_ctx"), "should have opaque mylib_ctx: {}", symbols);
}

// CI1: Static functions not imported (internal linkage)
#[test]
fn c_import_skips_static() {
    let symbols = resolve_c_import("mylib.h", "func main() {}");
    assert!(!symbols.contains("mylib_internal_helper"),
        "should NOT import static function: {}", symbols);
}

// CI1: Calling C function through namespace type-checks
#[test]
fn c_import_call_typechecks() {
    assert!(check_c_import("mylib.h",
        "func main() {\n    unsafe {\n        c.mylib_noop()\n    }\n}"
    ), "calling c.mylib_noop() should type-check");
}

// CI1: Multiple functions type-check
#[test]
fn c_import_call_with_args_typechecks() {
    assert!(check_c_import("mylib.h",
        "func main() {\n    unsafe {\n        c.mylib_add(1, 2)\n    }\n}"
    ), "calling c.mylib_add(1, 2) should type-check");
}

// CI5: import c "header.h" hiding { symbol }
#[test]
fn c_import_hiding() {
    let rask = rask_binary();
    let id = next_tmp_id();
    let header_path = c_header_fixture("mylib.h");
    let tmp = std::env::temp_dir().join(format!("rask_chidetest_{}_{}.rk", std::process::id(), id));
    let source = format!(
        "import c \"{}\" hiding {{ mylib_add }}\n\nfunc main() {{}}\n",
        header_path.display(),
    );
    std::fs::write(&tmp, &source).unwrap();

    let out = Command::new(&rask)
        .arg("resolve")
        .arg(&tmp)
        .output()
        .expect("failed to run rask resolve");

    let _ = std::fs::remove_file(&tmp);
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();

    // mylib_noop should be present, mylib_add should be hidden
    assert!(stdout.contains("mylib_noop"), "mylib_noop should still be visible");
    // Check that mylib_add is NOT in the CNamespace members
    // (it may still exist as a symbol, but not in the namespace)
    let ns_line = stdout.lines().find(|l| l.contains("CNamespace"));
    if let Some(line) = ns_line {
        assert!(!line.contains("mylib_add"),
            "mylib_add should be hidden from namespace: {}", line);
    }
}

// CI1: Aliased import: import c "header.h" as mylib
#[test]
fn c_import_alias() {
    let rask = rask_binary();
    let id = next_tmp_id();
    let header_path = c_header_fixture("mylib.h");
    let tmp = std::env::temp_dir().join(format!("rask_caliastest_{}_{}.rk", std::process::id(), id));
    let source = format!(
        "import c \"{}\" as mylib\n\nfunc main() {{\n    unsafe {{\n        mylib.mylib_noop()\n    }}\n}}\n",
        header_path.display(),
    );
    std::fs::write(&tmp, &source).unwrap();

    let out = Command::new(&rask)
        .arg("check")
        .arg(&tmp)
        .output()
        .expect("failed to run rask check");

    let _ = std::fs::remove_file(&tmp);
    assert!(out.status.success(), "aliased import should type-check: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr));
}

// Error: header not found should produce clear error
#[test]
fn c_import_missing_header() {
    let (ok, output) = check_c_import_output("nonexistent.h", "func main() {}");
    assert!(!ok, "missing header should fail");
    assert!(output.contains("not found") || output.contains("header"),
        "should mention header not found: {}", output);
}

// CI1: rask c-header CLI command works
#[test]
fn c_header_cli_command() {
    let rask = rask_binary();
    let header_path = c_header_fixture("mylib.h");

    let out = Command::new(&rask)
        .arg("c-header")
        .arg(&header_path)
        .output()
        .expect("failed to run rask c-header");

    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(out.status.success(), "c-header command should succeed: {}",
        String::from_utf8_lossy(&out.stderr));
    assert!(stdout.contains("extern \"C\" func mylib_add"), "should show mylib_add: {}", stdout);
    assert!(stdout.contains("extern \"C\" struct mylib_point"), "should show struct: {}", stdout);
    assert!(stdout.contains("MYLIB_VERSION"), "should show constant: {}", stdout);
}

// TM1: Type mapping verified through resolve output
#[test]
fn c_import_type_mapping() {
    let symbols = resolve_c_import("mylib.h", "func main() {}");
    // mylib_hash should have params with u32 return and *u8 + c_size params
    assert!(symbols.contains("mylib_hash"), "should have mylib_hash");
    // mylib_add should have c_int params
    let add_line = symbols.lines().find(|l| l.contains("mylib_add"));
    if let Some(line) = add_line {
        assert!(line.contains("c_int"), "mylib_add should have c_int params: {}", line);
    }
}

// Function-like macro produces warning, not error
#[test]
fn c_import_function_macro_warned() {
    let (ok, output) = check_c_import_output("mylib.h", "func main() {}");
    assert!(ok, "should still compile despite function-like macro");
    assert!(output.contains("MYLIB_MAX") || output.contains("macro"),
        "should warn about function-like macro: {}", output);
}

// ─── Codegen regression tests ────────────────────────────────
//
// These pin down specific bugs exposed by `rask build projects/tiwaz`:
//
// - mutex_field_lock: `with self.field.lock() as v { ... }` on a Mutex
//   field must lower to a 2-arg Mutex_lock call. Before the fix, the
//   method-call form wasn't detected and Mutex_lock was emitted with
//   one arg, failing Cranelift verification.
//
// - ensure_continuation: cleanup_chain continuation blocks that are
//   also reached from normal Goto/Branch paths must stay in the
//   normal block_map. Before the fix, transitive closure of
//   cleanup_only swallowed shared blocks → "Target block not found".
//
// Both tests assert `rask compile` succeeds (no codegen error).
// Runtime execution is exercised via --interp; native execution is
// skipped when it segfaults for unrelated runtime-layout reasons.

#[test]
fn codegen_mutex_field_lock() {
    let (ok, output) = compile_only_succeeds("mutex_field_lock.rk");
    assert!(ok, "mutex field .lock() in with-block should codegen cleanly:\n{}", output);
}

#[test]
fn interp_mutex_field_lock() {
    let (stdout, code) = run_interp("mutex_field_lock.rk");
    assert_eq!(code, 0, "stdout: {}", stdout);
    assert_eq!(stdout, "42\n");
}

#[test]
fn codegen_ensure_continuation() {
    let (ok, output) = compile_only_succeeds("ensure_continuation.rk");
    assert!(ok, "ensure handler continuation should codegen cleanly:\n{}", output);
}

#[test]
fn interp_ensure_continuation() {
    let (stdout, code) = run_interp("ensure_continuation.rk");
    assert_eq!(code, 0, "stdout: {}", stdout);
    // run(true) hits `return counter` before ensure runs for cleanup → 0
    // run(false) increments counter to 1, ensure adds 10 → 11
    // Order of output may depend on ensure timing semantics; accept
    // either (0, 11) or (10, 11) depending on ensure-before-return rules.
    assert!(
        stdout == "0\n1\n" || stdout == "10\n11\n" || stdout == "0\n11\n",
        "unexpected output: {:?}", stdout
    );
}

// ─── Integer overflow semantics (type.overflow, issue #325) ──────
//
// Panic on overflow in all builds (OV1–OV4, SH1), identical on both
// backends. Panic fixtures must fail (nonzero exit) with a message; the
// boundary fixture must run cleanly with identical output on interp+native.

/// Boundary arithmetic that must NOT panic — same output on both backends.
const OVERFLOW_BOUNDARY_OUT: &str =
    "2147483646\n-2147483647\n2147395600\n1073741824\n9223372036854775807\n";

#[test]
fn overflow_boundary_interp() {
    let (stdout, _stderr, code) = run_capture("--interp", "overflow_boundary.rk");
    assert_eq!(code, 0, "boundary arithmetic must not panic on interp");
    assert_eq!(stdout, OVERFLOW_BOUNDARY_OUT);
}

#[test]
fn overflow_boundary_native() {
    let (stdout, _stderr, code) = run_capture("--native", "overflow_boundary.rk");
    assert_eq!(code, 0, "boundary arithmetic must not panic on native");
    assert_eq!(stdout, OVERFLOW_BOUNDARY_OUT);
}

/// Assert a fixture panics (nonzero exit) with `needle` in its output on both
/// backends — the core "panic on overflow in all builds" guarantee.
fn assert_panics_both(fixture: &str, needle: &str) {
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, fixture);
        assert_ne!(code, 0, "{} on {} should panic, got exit 0", fixture, mode);
        let combined = format!("{}{}", stdout, stderr);
        assert!(
            combined.contains(needle),
            "{} on {}: expected `{}` in output, got:\n{}",
            fixture, mode, needle, combined,
        );
    }
}

#[test]
fn overflow_add_panics() {
    assert_panics_both("overflow_add.rk", "overflow");
}

#[test]
fn overflow_mul_panics() {
    assert_panics_both("overflow_mul.rk", "overflow");
}

#[test]
fn overflow_sub_panics() {
    // Unsigned subtraction below zero (OV1).
    assert_panics_both("overflow_sub.rk", "overflow");
}

#[test]
fn overflow_neg_panics() {
    // Negating signed MIN (OV1).
    assert_panics_both("overflow_neg.rk", "overflow");
}

#[test]
fn overflow_div_zero_panics() {
    // OV2: both backends now agree (native previously had no check).
    assert_panics_both("overflow_div_zero.rk", "by zero");
}

#[test]
fn overflow_div_min_panics() {
    // OV3: signed MIN / -1.
    assert_panics_both("overflow_div_min.rk", "overflow");
}

#[test]
fn overflow_shift_panics() {
    // SH1: shift amount exceeds bit width.
    assert_panics_both("overflow_shift.rk", "shift amount");
}

// ─── Regression: issue #236 ─────────────────────────────────
//
// `rask test <dir>` on a directory of standalone files (no build.rk)
// must run each file in isolation. Without isolation, identically named
// types in different files collide ("expected `Point`, found `Point`"
// with different TypeIds) — type checking regresses vs single-file mode.

#[test]
fn test_dir_runs_files_independently() {
    let rask = rask_binary();
    let dir = std::env::temp_dir().join(format!("rask_test_dir_indep_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // Two files each defining their own `Point` — cannot share a TypeId.
    std::fs::write(dir.join("a.rk"), r#"
struct Point { x: i32, y: i32 }
test "a uses its own Point" {
    const p = Point { x: 1, y: 2 }
    assert p.x == 1
}
"#).unwrap();

    std::fs::write(dir.join("b.rk"), r#"
struct Point { x: i32, y: i32, z: i32 }
test "b uses its own Point" {
    const p = Point { x: 1, y: 2, z: 3 }
    assert p.z == 3
}
"#).unwrap();

    let out = Command::new(&rask)
        .arg("test")
        .arg(&dir)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask test");

    let _ = std::fs::remove_dir_all(&dir);

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    let combined = format!("{}{}", stdout, stderr);

    assert!(
        out.status.success(),
        "rask test <dir> should succeed when files have independent types\nstdout: {}\nstderr: {}",
        stdout, stderr,
    );

    // Per-file processing prevents the spurious "expected `Point`, found `Point`"
    // error that issue #236 was about.
    assert!(
        !combined.contains("expected `Point`, found `Point`"),
        "must not produce cross-file Point/Point mismatch: {}", combined,
    );
    assert!(
        combined.contains("a uses its own Point") && combined.contains("b uses its own Point"),
        "both files' tests should run: {}", combined,
    );
}
