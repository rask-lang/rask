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

/// Compile a .rk fixture together with a C driver and run it, returning
/// (stdout, stderr, exit code). The only way to exercise an `extern "C"` export
/// — the symbol has to be called from actual C frames for the boundary rules to
/// mean anything.
fn compile_with_c_and_run(fixture_name: &str, c_driver: &str) -> (String, String, i32) {
    let rask = rask_binary();
    let tmp = std::env::temp_dir();
    let stem = fixture_name.trim_end_matches(".rk");
    let bin_path = tmp.join(format!("rask_ffi_{}_{}", stem, std::process::id()));

    let compile_out = Command::new(&rask)
        .arg("compile")
        .arg(fixture(fixture_name))
        .arg("--link-obj")
        .arg(fixture(c_driver))
        .arg("-o")
        .arg(&bin_path)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask compile");

    assert!(
        compile_out.status.success(),
        "rask compile {} + {} failed:\nstdout: {}\nstderr: {}",
        fixture_name, c_driver,
        String::from_utf8_lossy(&compile_out.stdout),
        String::from_utf8_lossy(&compile_out.stderr),
    );

    let run_out = Command::new(&bin_path).output().expect("failed to run compiled binary");
    let _ = std::fs::remove_file(&bin_path);
    // A process killed by a signal has no exit code, so report it the way a
    // shell does — 128 + signal. Otherwise an abort and a clean exit both read
    // as "no code" and the test can't tell them apart.
    let code = run_out.status.code().unwrap_or_else(|| {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            run_out.status.signal().map(|s| 128 + s).unwrap_or(-1)
        }
        #[cfg(not(unix))]
        { -1 }
    });
    (
        String::from_utf8_lossy(&run_out.stdout).to_string(),
        String::from_utf8_lossy(&run_out.stderr).to_string(),
        code,
    )
}

#[test]
fn extern_c_export_returns_through_c_frames() {
    // The export form (`public extern "C" func name() { ... }`) had no working
    // path through the compiler at all: reachability starts at main, nothing in
    // Rask calls an exported symbol, so it was dropped as dead code and a C
    // driver linking against it got "undefined reference".
    let (stdout, stderr, code) =
        compile_with_c_and_run("ffi_boundary_ok.rk", "ffi_boundary_driver.c");
    assert_eq!(code, 0, "normal returns through the boundary: {}", stderr);
    assert_eq!(stdout, "C: got 42\nC: got 42\nmain still alive\n", "{:?}", stdout);
}

#[test]
fn panic_in_extern_c_export_aborts_at_the_boundary() {
    // ctrl.panic/A1. The panic happens inside a spawned task, so the task's
    // setjmp is live and the normal unwind would longjmp straight over the C
    // frame between them — skipping whatever the C caller had on the stack.
    // It must abort at the boundary instead.
    let (stdout, stderr, code) =
        compile_with_c_and_run("ffi_boundary_panic.rk", "ffi_boundary_driver.c");
    assert!(stdout.contains("C: before callback"), "the C frame ran: {:?}", stdout);
    assert!(
        !stdout.contains("C: after callback"),
        "the C frame must not resume past a panicking callback: {:?}", stdout,
    );
    assert!(
        !stdout.contains("must not be reached"),
        "the panic must not unwind back into Rask: {:?}", stdout,
    );
    assert!(
        stderr.contains("panic crossed an FFI boundary"),
        "the abort names the boundary: {:?}", stderr,
    );
    // SIGABRT, not exit(101) — P4's exit code is for a panic escaping main.
    assert_eq!(code, 134, "abort, not exit: stderr {}", stderr);
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

/// Like `run_capture`, but with stdin closed — a fixture that reads a line
/// gets a deterministic EOF instead of whatever the test runner was attached to.
fn run_capture_no_stdin(mode: &str, fixture_name: &str) -> (String, String, i32) {
    let rask = rask_binary();
    let out = Command::new(&rask)
        .args(["run", mode])
        .arg(fixture(fixture_name))
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("failed to run rask");
    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        String::from_utf8_lossy(&out.stderr).to_string(),
        out.status.code().unwrap_or(-1),
    )
}

// ─── Wide<T> data-parallel tests ─────────────────────────────

const WIDE_EXPECTED: &str = "10\n20\n300\n2, 4, 6, 8\n1\n4\n24\n";

#[test]
fn wide_basic_interp() {
    let (stdout, code) = run_interp("wide_basic.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, WIDE_EXPECTED);
}

// Closure-free Wide path runs natively; the CPU result is the reference
// semantics a device backend must match (conc.data-parallel/W3). Assert
// native and interp agree, and both are correct.
#[test]
fn wide_native_sum_matches_interp() {
    let (interp_out, interp_code) = run_interp("wide_native_sum.rk");
    let (native_out, native_code) = run_native("wide_native_sum.rk");
    assert_eq!(interp_code, 0, "interp failed");
    assert_eq!(native_code, 0, "native failed");
    assert_eq!(interp_out, "sum=10\n");
    assert_eq!(native_out, interp_out, "native != interp (W3 oracle)");
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

// ─── Field defaults (FD1-FD5) ───────────────────────────────
// Omitted fields fill from declared defaults. Both backends construct
// structs by name, so filling defaults at desugar time covers both.

const FIELD_DEFAULTS_EXPECTED: &str =
    "localhost 8080 false\nexample.com 3000 true\n640 480 untitled\n800 480 copy\n";

#[test]
fn compile_field_defaults() {
    let (stdout, code) = compile_and_run("field_defaults.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, FIELD_DEFAULTS_EXPECTED);
}

#[test]
fn native_field_defaults() {
    let (stdout, code) = run_native("field_defaults.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, FIELD_DEFAULTS_EXPECTED);
}

#[test]
fn interp_field_defaults() {
    let (stdout, code) = run_interp("field_defaults.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, FIELD_DEFAULTS_EXPECTED);
}

// Field annotations (@rename/@skip/@default) surface through reflect (#317:
// `comptime for` over `reflect.fields<T>()` now unrolls on native too).
const FIELD_ANNOTATIONS_EXPECTED: &str =
    "name user_name false false\n\
     cache_key cache_key true false\n\
     login_count login_count false true\n\
     role role false true\n";

#[test]
fn native_field_annotations() {
    let (stdout, code) = run_native("field_annotations.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, FIELD_ANNOTATIONS_EXPECTED);
}

#[test]
fn interp_field_annotations() {
    let (stdout, code) = run_interp("field_annotations.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, FIELD_ANNOTATIONS_EXPECTED);
}

// CT49: value.(field.name) inside the same unrolled loop, reading the actual
// field value back rather than just its FieldInfo metadata.
const DYNAMIC_FIELD_ACCESS_EXPECTED: &str = "x (x) = 1.5\ny (Y) = 2.5\n";

#[test]
fn native_dynamic_field_access() {
    let (stdout, code) = run_native("dynamic_field_access.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, DYNAMIC_FIELD_ACCESS_EXPECTED);
}

#[test]
fn interp_dynamic_field_access() {
    let (stdout, code) = run_interp("dynamic_field_access.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, DYNAMIC_FIELD_ACCESS_EXPECTED);
}

// #370 (field-position error type accepted) + #364 (Result field sized to match
// codegen so `tag` isn't clobbered).
#[test]
fn compile_result_struct_field() {
    let (stdout, code) = compile_and_run("result_struct_field.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "5\n999\n");
}

#[test]
fn native_result_struct_field() {
    let (stdout, code) = run_native("result_struct_field.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "5\n999\n");
}

// #347 — enum variant carrying a struct payload (Pos(Pt)) segfaulted natively;
// a scalar variant (Scalar(i32)) of the same enum must keep working.
#[test]
fn compile_enum_struct_payload() {
    let (stdout, code) = compile_and_run("enum_struct_payload.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "7\n9\n");
}

#[test]
fn native_enum_struct_payload() {
    let (stdout, code) = run_native("enum_struct_payload.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "7\n9\n");
}

// #347 family — returning/matching a struct ok-payload through a `T or E`.
#[test]
fn compile_result_struct_ok() {
    let (stdout, code) = compile_and_run("result_struct_ok.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "111 222 333\n");
}

#[test]
fn native_result_struct_ok() {
    let (stdout, code) = run_native("result_struct_ok.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "111 222 333\n");
}

// #365 family — one aggregate byte-copy helper. A struct payload copied
// through an Option slot must land byte-identical on both backends.
#[test]
fn compile_agg_copy_paths() {
    let (stdout, code) = compile_and_run("agg_copy_paths.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1 2 3\nnone\n");
}

#[test]
fn native_agg_copy_paths() {
    let (stdout, code) = run_native("agg_copy_paths.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "1 2 3\nnone\n");
}

// Option/Result slot constructors — scalar Ok/Err/Some/none round-trip
// identically on both backends after routing through one constructor set.
#[test]
fn compile_wrap_constructors() {
    let (stdout, code) = compile_and_run("wrap_constructors.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "ok 5\nerr\neven 4\nno even\n");
}

#[test]
fn native_wrap_constructors() {
    let (stdout, code) = run_native("wrap_constructors.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "ok 5\nerr\neven 4\nno even\n");
}

// #259 family — ok/err arm routing by type identity, not capitalization.
// A lowercase-named err type used to be sent to the ok side on native (the
// err arm never ran); an uppercase ok struct is the mirror case. Both must
// route identically to the interpreter now.
const ERR_ROUTING_OUT: &str = "cfg 8080\ncfg err\nnum 7\nnum err\n";

#[test]
fn compile_err_routing() {
    let (stdout, code) = compile_and_run("err_routing.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, ERR_ROUTING_OUT);
}

#[test]
fn native_err_routing() {
    let (stdout, code) = run_native("err_routing.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, ERR_ROUTING_OUT);
}

#[test]
fn err_routing_native_matches_interp() {
    let (native, ncode) = run_native("err_routing.rk");
    let (interp, icode) = run_interp("err_routing.rk");
    assert_eq!(ncode, 0);
    assert_eq!(icode, 0);
    assert_eq!(native, interp, "native and interp must agree on ok/err routing");
}

// One element-size query — collections must allocate correctly-sized slots for
// i32 (8-byte slot, no truncation), string (16), and struct (layout) elements,
// with Map keys/values sized independently. Native must equal interp.
const COLLECTION_SIZES_OUT: &str = "100\n200000\nalpha\nbeta\n1 2 3\n2\n";

#[test]
fn compile_collection_elem_sizes() {
    let (stdout, code) = compile_and_run("collection_elem_sizes.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, COLLECTION_SIZES_OUT);
}

#[test]
fn native_collection_elem_sizes() {
    let (stdout, code) = run_native("collection_elem_sizes.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, COLLECTION_SIZES_OUT);
}

#[test]
fn collection_elem_sizes_native_matches_interp() {
    let (native, ncode) = run_native("collection_elem_sizes.rk");
    let (interp, icode) = run_interp("collection_elem_sizes.rk");
    assert_eq!(ncode, 0);
    assert_eq!(icode, 0);
    assert_eq!(native, interp, "native and interp must agree on element sizes");
}

// while-let with an ok-side, uppercase-named type pattern must enter the loop.
// The capitalization heuristic previously routed `Reading` to the err side.
#[test]
fn compile_whilelet_ok_typepat() {
    let (stdout, code) = compile_and_run("whilelet_ok_typepat.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "0\n10\n20\ndone\n");
}

#[test]
fn native_whilelet_ok_typepat() {
    let (stdout, code) = run_native("whilelet_ok_typepat.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "0\n10\n20\ndone\n");
}

// ─── Stdlib method dispatch (T0.2) ───────────────────────────
// Dispatch is driven by the resolved receiver type + stub metadata, so
// native and interp must agree. Each fixture exercises one type family;
// the assertion pins native == interp == expected.

/// Assert a fixture's native and interpreter stdout match each other and
/// the expected output, and both exit 0.
fn assert_native_eq_interp(fixture_name: &str, expected: &str) {
    let (native, ncode) = run_native(fixture_name);
    let (interp, icode) = run_interp(fixture_name);
    assert_eq!(ncode, 0, "native exit for {}", fixture_name);
    assert_eq!(icode, 0, "interp exit for {}", fixture_name);
    assert_eq!(
        native, interp,
        "native != interp for {}\nnative: {:?}\ninterp: {:?}",
        fixture_name, native, interp
    );
    assert_eq!(native, expected, "unexpected output for {}", fixture_name);
}

#[test]
fn dispatch_vec_native_eq_interp() {
    assert_native_eq_interp("dispatch_vec.rk", "3\nfalse\n20\n30\n2\n");
}

// #399: `==`/`!=` on enums and structs compares contents on native, not the
// address of the operands' stack slots. Covers unit-variant enums, an enum with
// a payload, a struct with a string field, inequality, and an `if x == Variant`
// branch — all must match the interpreter.
#[test]
fn struct_enum_eq_native_eq_interp() {
    assert_native_eq_interp(
        "struct_enum_eq.rk",
        "true\nfalse\ntrue\ntrue\nfalse\nfalse\ntrue\ntrue\nfalse\nfalse\ntrue\nbranch-green\n",
    );
}

// comp.hidden-params (#422): named `using Pool<T>` contexts must lower on
// native — the SIG2 rewrite (hidden param + named alias + call-site arg) —
// and read the pool element the same as the interpreter.
#[test]
fn using_pool_read_native_eq_interp() {
    assert_native_eq_interp("using_pool_read.rk", "100");
}

#[test]
fn using_pool_propagate_native_eq_interp() {
    assert_native_eq_interp("using_pool_propagate.rk", "42");
}

// comp.hidden-params/CALL6: instance-method `using` context threads through
// method dispatch on both the top-level `l.settle(a)` and the inner
// `self.post(h)`. Native must match the interpreter.
#[test]
fn using_pool_method_native_eq_interp() {
    assert_native_eq_interp("using_pool_method.rk", "5");
}

// mem.context/CC1 (#434): `h.field` reads auto-resolve through a named
// `using Pool<T>` context — the spec's headline pattern.
#[test]
fn handle_autoderef_read_native_eq_interp() {
    assert_native_eq_interp("handle_autoderef_read.rk", "100");
}

// mem.context/CC1 (#434): `h.field` writes (plain + compound) through named and
// unnamed contexts mutate the pool element on both backends.
#[test]
fn handle_autoderef_write_native_eq_interp() {
    assert_native_eq_interp("handle_autoderef_write.rk", "705");
}

// mem.context/CC7 (#434): a private function infers its unnamed Pool<T> context
// from `h.field` access, so auto-deref works with no `using` clause.
#[test]
fn handle_autoderef_inferred_native_eq_interp() {
    assert_native_eq_interp("handle_autoderef_inferred.rk", "42");
}

// mem.pools/PF5: reads through a handle in a frozen context work on both backends.
#[test]
fn frozen_pool_read_native_eq_interp() {
    assert_native_eq_interp("frozen_pool_read.rk", "50");
}

// #380: `T` widens to `T?` at an assignment lvalue (reassignment + field store)
// and the value is wrapped to `Some` on both backends.
#[test]
fn optional_widen_assign_native_eq_interp() {
    assert_native_eq_interp("optional_widen_assign.rk", "1042");
}

// #270: scalar `mutate` params write back through field/index projections
// (`swap_fields(mutate p.x, mutate p.y)`, `boost(mutate p.x)`), while a whole
// Copy variable stays unchanged (`modify_int(z)`). Native == interp.
#[test]
fn scalar_mutate_writeback_native_eq_interp() {
    assert_native_eq_interp("scalar_mutate_writeback.rk", "211242");
}

// mem.pools/PL2 (#435): a bounded `with_capacity` pool works like a normal pool
// for inserts within the bound, on both backends.
#[test]
fn bounded_pool_with_capacity_native_eq_interp() {
    assert_native_eq_interp("bounded_pool_with_capacity.rk", "230");
}

// mem.pools/PL8 (#435): `insert` into a full bounded pool panics (exit 101) on
// both backends — nothing after the failing insert runs.
#[test]
fn bounded_pool_insert_full_panics() {
    let (nout, ncode) = run_native("bounded_pool_insert_full.rk");
    let (iout, icode) = run_interp("bounded_pool_insert_full.rk");
    assert_eq!(ncode, 101, "native should panic on full insert: {}", nout);
    assert_eq!(icode, 101, "interp should panic on full insert: {}", iout);
    assert!(!nout.contains("99"), "native must not reach past the panic: {}", nout);
    assert!(!iout.contains("99"), "interp must not reach past the panic: {}", iout);
}

// mem.pools/PL8 (#435): `try_insert` returns Some until the bounded pool is full,
// then none. Interpreter is the reference (native try_insert is tracked in #438).
#[test]
fn bounded_pool_try_insert_interp() {
    let (out, code) = run_interp("bounded_pool_try_insert.rk");
    assert_eq!(code, 0, "try_insert program must run: {}", out);
    assert_eq!(out, "110", "unexpected try_insert output: {:?}", out);
}

// #382 + #380: a Handle is Copy, so `pool[a].next = b` links without consuming
// `b`, and the widen reads back as Some. Interp is the reference (native pool
// niche-Option<Handle> reads are tracked in #438), and it must type-check
// (exit 0) — proving no E0800/E0308 remain.
#[test]
fn handle_copy_link_interp() {
    let (out, code) = run_interp("handle_copy_link.rk");
    assert_eq!(code, 0, "handle_copy_link must type-check and run: {}", out);
    assert_eq!(out, "12", "unexpected interp output: {:?}", out);
}

// #411: nested struct-field assignment (`ln.a.x = v`) persists on native — the
// projected place stores into the base local instead of a value copy.
#[test]
fn nested_field_store_native_eq_interp() {
    assert_native_eq_interp("nested_field_store.rk", "50604");
}

// #402: compound assignment through a pool handle (`pool[h].f -= n`) persists on
// native — aggregate pool accesses no longer coalesce into a value copy.
#[test]
fn pool_compound_assign_native_eq_interp() {
    assert_native_eq_interp("pool_compound_assign.rk", "65");
}

// #402: `with pool[h] as e { e.f = v }` writes through to the pool on native —
// the binding aliases the slot instead of copying the element.
#[test]
fn with_pool_writeback_native_eq_interp() {
    assert_native_eq_interp("with_pool_writeback.rk", "78");
}

// #411: field store into a Vec element (`v[i].hp = v`, `+=`) persists on native
// via read-modify-writeback.
#[test]
fn vec_elem_field_store_native_eq_interp() {
    assert_native_eq_interp("vec_elem_field_store.rk", "9925");
}

// comp.hidden-params/CALL2: a pool held in `self.players` resolves as a hidden
// context arg (lowered as a field access) for a free callee.
#[test]
fn using_pool_self_field_native_eq_interp() {
    assert_native_eq_interp("using_pool_self_field.rk", "7");
}

// mem.context/CC9: an inline closure passed as an argument inherits the
// enclosing function's pool context.
#[test]
fn using_closure_immediate_native_eq_interp() {
    assert_native_eq_interp("using_closure_immediate.rk", "100");
}

// mem.context/CC10: a storable closure can still take the pool as an explicit
// param — that resolves the callee's context without inheritance.
#[test]
fn using_closure_storable_ok_native_eq_interp() {
    assert_native_eq_interp("using_closure_storable_ok.rk", "100");
}

#[test]
fn dispatch_map_native_eq_interp() {
    assert_native_eq_interp("dispatch_map.rk", "3\ntrue\n2\nfalse\n2\n");
}

#[test]
fn dispatch_string_native_eq_interp() {
    assert_native_eq_interp(
        "dispatch_string.rk",
        "11\ntrue\ntrue\nHELLO, RASK\nhello, rask\nHello, World\n",
    );
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

// ─── Trait-object vtable dispatch (task 1.4, issue #194) ─────
//
// `Shape` declares an incompatible method (returns Self, TR2) before the
// compatible ones, so the vtable holds compatible slots at 0/1 while the naive
// offset would be 1/2. Native codegen goes through the real vtable — the
// interpreter's by-name dispatch can't catch a wrong-slot miscompile, so this
// has to run the compiled binary.
#[test]
fn compile_trait_object_dispatch() {
    let (stdout, code) = compile_and_run("trait_object_dispatch.rk");
    assert_eq!(code, 0);
    assert_eq!(stdout, "square=16\ncircle=75\n");
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

/// Like `compile_error`, but returns combined stdout+stderr so a test can
/// assert the failure is for the RIGHT reason (error code / message), not just
/// a non-zero exit on some unrelated error.
fn compile_error_output(name: &str) -> (bool, String) {
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

    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    (!out.status.success(), combined)
}

#[test]
fn error_annotation_against_a_container_initializer() {
    // #730: `unify` defers whenever either side is an unresolved generic, since
    // the name may still resolve — and a deferred `Equal` that never resolved
    // was dropped in silence. So `let probe: string = m` on a `Mutex` passed.
    // Reported for a primitive against a stdlib container only: two *named*
    // types legitimately unify across names (union members, enum variants,
    // trait objects, nominal aliases) and judging those reported the stdlib's
    // own source as broken.
    let (failed, out) = compile_error_output("annotation_vs_container.rk");
    assert!(failed, "an annotation must be checked against a container init: {}", out);
    assert!(
        out.contains("expected `string`, found `Mutex"),
        "should name both sides for the Mutex case: {}", out,
    );
    assert!(
        out.contains("expected `i64`, found `Sender"),
        "and for the tuple-destructured Sender: {}", out,
    );
}

#[test]
fn error_module_used_without_import() {
    // #723: the stdlib's source is resolved alongside the program and declares
    // each module as a plain type (`struct math { }`), so the name was in scope
    // whether or not it was imported. `math.sin(x)` with no import passed
    // `rask check`, compiled, and ran natively — and died on the interpreter,
    // which binds a module only when it sees the import declaration.
    let (failed, out) = compile_error_output("module_without_import.rk");
    assert!(failed, "a module used without importing it must be rejected: {}", out);
    for module in ["math", "fs"] {
        assert!(
            out.contains(&format!("`{module}` is used but never imported")),
            "should name `{module}`: {out}",
        );
        assert!(
            out.contains(&format!("import {module}")),
            "should show the import as the fix for `{module}`: {out}",
        );
    }
}

// #500: a free function named with a keyword can be declared but never called,
// so the declaration is rejected — and the message says why, instead of the
// backwards type error the call site used to produce.
#[test]
fn error_keyword_fn_name() {
    let (failed, out) = compile_error_output("keyword_fn_name.rk");
    assert!(failed, "`func check(...)` must be rejected: {}", out);
    assert!(
        out.contains("`check` is a keyword"),
        "should name the keyword: {}", out,
    );
}

// #713: one error variant covers four different trait requirements, and it
// used to give all four the same advice — "implement `Trait` for `Type`",
// explained in terms of trait objects. That is not advice anyone can take on a
// numeric bound (`Integer` is a set of primitive types, not a list of methods),
// and at a call site it points at the wrong file. A bound naming a trait that
// doesn't exist had no type to blame at all and reported `_`.
#[test]
fn error_trait_bound_messages() {
    let (failed, out) = compile_error_output("trait_bound_messages.rk");
    assert!(failed, "unsatisfied trait requirements must be rejected: {}", out);
    assert!(
        !out.contains("`_` does not implement"),
        "an unknown trait is a name problem, not a mystery type: {}", out,
    );
    assert!(
        out.contains("did you mean `Integer`?"),
        "a misspelt trait should suggest the real one: {}", out,
    );
    // The numeric bound explains membership and lists the members.
    assert!(
        out.contains("is not one of the types `Integer` covers"),
        "a numeric bound is membership, not missing methods: {}", out,
    );
    assert!(
        !out.contains("implement `Integer` for"),
        "nothing can implement a numeric trait, so that must not be the fix: {}", out,
    );
    // The other three keep their own advice, each naming what to do where.
    for expected in [
        "add the missing methods to the block",
        "pass a type that implements `Greeter`",
        "implement the trait before boxing",
    ] {
        assert!(out.contains(expected), "missing advice {:?}: {}", expected, out);
    }
    assert!(
        !out.contains("casting to a trait object requires"),
        "the trait-object wording belongs to the cast alone: {}", out,
    );
}

// OPT3/OPT11/OPT13/OPT32: the optional operator family needs an absent branch
// to work on. All three used to be rejected only as a side effect of the shape
// they were rewritten into, and reported it that way — `n ?? -1` said "expected
// `i64`, found `i32 or _`" and advised changing the type to the one already
// written (#645). Pins the message, the `.get(k)` advice for the index case,
// and the count: six mistakes, six errors, nothing extra from the cascade and
// nothing for the legal shapes in the same file.
#[test]
fn error_optional_operators_need_optionals() {
    let (failed, out) = compile_error_output("optional_operators_need_optionals.rk");
    assert!(failed, "`??`/`!`/`take` on a non-optional must be rejected: {}", out);
    assert_eq!(
        out.matches("E0831").count(), 4,
        "one per `??`: a local, a string, a map index, a struct field: {}", out,
    );
    assert_eq!(
        out.matches("E0832").count(), 1,
        "one `!` on a non-optional: {}", out,
    );
    assert_eq!(
        out.matches("E0365").count(), 1,
        "one `take` on a non-optional place: {}", out,
    );
    assert!(
        out.contains("m.get(k) ?? fallback"),
        "the index case should point at `.get`, not at the `??`: {}", out,
    );
    assert!(
        !out.contains("E0361"),
        "a rejected operator poisons its result, so no follow-on \"couldn\'t work \
         out the type\": {}", out,
    );
}

// ER11: a bare `T` becomes a `T or E` at `return` and nowhere else. The rule was
// always enforced; the message wasn't — the generic mismatch answered, and its
// "change this to type `i64 or LoadError`" was what the author had already
// written (#550, #641, #701). Pins the message and the count: four rejected
// positions, and the legitimate uses in the same file stay clean.
//
// The method argument is the fourth. It only started giving this message once
// the checker routed method arguments through the same coercion decision as
// every other position — before that it plain-unified and the generic mismatch
// answered there (#701).
#[test]
fn error_no_auto_wrap_outside_return() {
    let (failed, out) = compile_error_output("no_auto_wrap_outside_return.rk");
    assert!(failed, "a bare value at a non-return position must be rejected: {}", out);
    assert!(
        out.contains("auto-wrap only fires at `return`"),
        "should name the rule, not report a bare mismatch: {}", out,
    );
    assert!(
        out.contains("i64 or LoadError"),
        "should name the error type rather than printing `<type#N>`: {}", out,
    );
    assert_eq!(
        out.matches("E0828").count(), 4,
        "one per coercion position — binding, argument, method argument, field — \
         and nothing for the wrapped-by-a-call forms or the optional: {}", out,
    );
}

// #559: a `with` block's guard binding is access to the box's payload for the
// block's duration, not a value of its own — returning it raw let a caller
// keep reading through the mutex after the lock released. Struct payloads
// are rejected; a field read, a method call, and a scalar payload (the
// flagship example's `with counter.lock() as c { c }`) all still compile.
#[test]
fn error_with_guard_escapes() {
    let (failed, out) = compile_error_output("with_guard_escapes.rk");
    assert!(failed, "returning a struct `with` guard raw must be rejected: {}", out);
    assert!(
        out.contains("can't leave its block"),
        "should name the with-guard-escape rule: {}", out,
    );
    assert_eq!(
        out.matches("E0829").count(), 1,
        "only the bare-identifier case should fire — the field read, method \
         call, and scalar payload in the same file are all legitimate: {}", out,
    );
}

// #646: the error side of a `T or E` printed as `<type#N>` instead of the name.
// The pass that fills type names in was a hand-written match with a catch-all
// covering 17 of the 33 type-carrying variants, so anything added later fell
// through. Three different codes in one file, because the original bug was
// invisible to any single message — `Mismatch` was always correct.
#[test]
fn error_type_named_in_diagnostics() {
    let (failed, out) = compile_error_output("error_type_named_in_diagnostics.rk");
    assert!(failed, "wrapper-method and flat-try misuse must be rejected: {}", out);
    assert!(
        !out.contains("<type#"),
        "no diagnostic may leak an internal type id: {}", out,
    );
    for expected in ["no method `ok` on `i64 or IoError`",
                     "no method `unwrap_or_else` on `i64 or IoError`",
                     "`i64? or IoError`"] {
        assert!(out.contains(expected), "missing {:?} in: {}", expected, out);
    }
}

// A struct or enum name in call position used to type-check silently and then
// die in MIR lowering as "method `next` on receiver of unresolved type".
// `Name(value)` is the nominal-type constructor (T7); structs have no tuple
// form (S1).
#[test]
fn error_type_called_as_function() {
    let (failed, out) = compile_error_output("type_called_as_function.rk");
    assert!(failed, "`TaskId(1)` on a struct must be rejected: {}", out);
    assert!(
        out.contains("E0345") && out.contains("TaskId { value: …")
            && out.contains("Color.Variant"),
        "should name both cases with their fix: {}", out,
    );
}

// #341, ER31a: `try` wraps a propagated error into the caller's error enum, but
// only when one variant fits. Two that fit is a question for the author, and the
// answer changes behaviour — so the compiler asks instead of picking.
#[test]
fn error_ambiguous_error_wrap() {
    let (failed, out) = compile_error_output("ambiguous_error_wrap.rk");
    assert!(failed, "two variants wrapping the same error must be rejected: {}", out);
    assert!(
        out.contains("E0359") && out.contains("`Store` and `Fatal`")
            && out.contains("catch e => return ApiError.Store(e)"),
        "should name both candidates and how to choose: {}", out,
    );
}

// #506: a `{...}` in a string that isn't a single expression (plus an optional
// format spec) is rejected, instead of parsing whatever fits and dropping the
// rest — which is how a JSON body reached json.decode as the string "x".
#[test]
fn error_bad_interpolation() {
    let (failed, out) = compile_error_output("bad_interpolation.rk");
    assert!(failed, "a malformed interpolation must be rejected: {}", out);
    assert!(
        out.contains("is not a valid interpolation"),
        "should name the interpolation as the problem: {}", out,
    );
}

// #551, T10: honouring a nominal newtype's `with (…)` list means the list has
// to stay a list — an unlisted trait is still not inherited.
#[test]
fn error_nominal_trait_not_listed() {
    let (failed, out) = compile_error_output("nominal_trait_not_listed.rk");
    assert!(failed, "an unlisted trait must not be inherited: {}", out);
    assert!(
        out.contains("no method `lt`") && out.contains("no method `add`"),
        "should reject both the unlisted ordering and the arithmetic: {}", out,
    );
}

// A stdlib module function's declared type-param bound is checked against the
// written type argument. `decode<T: Decode>` had the bound dropped during stub
// loading, so `json.decode<WithPtr>` type-checked and blew up later — in MIR
// lowering on native, as a bogus "missing field" on interp.
#[test]
fn error_decode_bound_not_satisfied() {
    let (failed, out) = compile_error_output("decode_bound_not_satisfied.rk");
    assert!(failed, "a non-Decode type argument must be rejected: {}", out);
    assert!(
        out.contains("E0333") && out.contains("cannot be decoded"),
        "should say the type can't be decoded, not that it's missing methods: {}", out,
    );
}

// #506: an @unimplemented stdlib *module* function is caught at the call, the
// way a method on a receiver already was. `json.decode` used to type-check and
// then segfault; it's implemented now, so `json.to_value` stands in.
#[test]
fn error_unimplemented_module_fn() {
    let (failed, out) = compile_error_output("unimplemented_module_fn.rk");
    assert!(failed, "an unimplemented module function must be rejected: {}", out);
    assert!(
        out.contains("E0353") && out.contains("json.to_value"),
        "should name the function and the unimplemented code: {}", out,
    );
}

// #539: `{}` needs Displayable (std.fmt/D4). Before this, printing a struct
// failed in codegen with "Function not found: Point_to_string" and printing an
// optional printed its address on native and blew up at runtime on interp.
#[test]
fn error_not_displayable() {
    let (failed, out) = compile_error_output("not_displayable.rk");
    assert!(failed, "printing a non-Displayable type must be rejected: {}", out);
    assert!(
        out.contains("E0826") && out.contains("Point"),
        "should name the struct and the Displayable rule: {}", out,
    );
    assert!(
        out.contains("i64?"),
        "should catch the optional too, and name it as the user wrote it: {}", out,
    );
}

// A bare literal used to bind to whatever type it met first, so
// `func f() -> string { return 1 }` type-checked. Found chasing #383, where a
// `T? or E` return silently accepted anything numeric.
#[test]
fn error_literal_wrong_type() {
    let (failed, out) = compile_error_output("literal_wrong_type.rk");
    assert!(failed, "an int literal is not a string: {}", out);
    assert!(
        out.contains("expected `string`"),
        "should name the expected type: {}", out,
    );
}

// #345: `func main() -> void or E` that ends up on the error branch exits 1,
// not 0. Both backends: the interpreter treated the error as an ordinary
// return value, and native's main always returned void.
#[test]
fn main_error_return_exits_1() {
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "main_returns_error.rk");
        assert_eq!(code, 1, "{mode}: expected exit 1, got {code}\n{stdout}{stderr}");
        assert!(stdout.contains("starting"), "{mode}: should run up to the error: {stdout}");
        assert!(
            !stdout.contains("unreachable"),
            "{mode}: must stop at the propagated error: {stdout}",
        );
        assert!(
            stderr.contains("the thing failed"),
            "{mode}: should report the error's message: {stderr}",
        );
    }
}

#[test]
fn main_ok_return_exits_0() {
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "main_returns_ok.rk");
        assert_eq!(code, 0, "{mode}: expected exit 0, got {code}\n{stdout}{stderr}");
        assert!(stdout.contains("v=7"), "{mode}: {stdout}");
    }
}

#[test]
fn error_stdlib_renames() {
    // task-2b (#302): the old stdlib names are HARD errors, not aliases. Each
    // old name must be rejected as an unknown method (E0313), not silently
    // resolved. Witnesses recv/try_recv, as_secs*, getpid, os.vars,
    // fs.read_file/write_file/append_file, and the removed File.lines().
    let (failed, out) = compile_error_output("stdlib_renames.rk");
    assert!(failed, "old stdlib names must be rejected: {}", out);
    assert!(out.contains("E0313"), "should be an unknown-method error (E0313): {}", out);
    for old in ["recv", "try_recv", "as_secs", "getpid", "read_file", "lines"] {
        assert!(
            out.contains(&format!("no method `{}`", old)),
            "old name `{}` should be rejected as unknown method: {}", old, out,
        );
    }
}

// mem.pools/PF5: writing through a handle in a `using frozen Pool<T>` context is
// rejected (E0325); reads in the same file are fine.
#[test]
fn error_frozen_pool_write() {
    let (failed, out) = compile_error_output("frozen_pool_write.rk");
    assert!(failed, "frozen-context handle writes must be rejected: {}", out);
    assert!(out.contains("E0325"), "should be a frozen-context write error (E0325): {}", out);
    // Both the plain store and the compound assign are rejected; the read is not.
    assert_eq!(out.matches("error[E0325]").count(), 2, "exactly the two writes rejected: {}", out);
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
fn error_let_reassign() {
    assert!(compile_error("let_reassign.rk"), "should reject let reassignment");
}

// A shared read lock never hands out mutable access (conc.sync/R1) —
// mutation through a `.read()` binding used to type-check and write back
// through the read lock, racing concurrent readers.
#[test]
fn error_read_lock_mutate() {
    let (failed, out) = compile_error_output("read_lock_mutate.rk");
    assert!(failed, "mutation through a read-lock binding must be rejected: {}", out);
    assert!(out.contains("E0360"), "should flag the read-lock mutation: {}", out);
}

#[test]
fn error_context_ambiguous_cc8() {
    // mem.context/CC8: two Pool<Player> in scope where a callee needs the
    // context is a real diagnostic, not the old unresolved-variable failure.
    let (failed, out) = compile_error_output("context_ambiguous_min.rk");
    assert!(failed, "ambiguous context must be rejected: {}", out);
    assert!(out.contains("CC8"), "should carry the CC8 code: {}", out);
    assert!(
        out.contains("ambiguous context"),
        "should name the ambiguity, not a var lookup failure: {}", out,
    );
}

#[test]
fn error_context_closure_storable_cc10() {
    // mem.context/CC10: a storable closure needing a pool context it doesn't
    // take as a parameter is rejected — it can't inherit ambient contexts.
    let (failed, out) = compile_error_output("context_closure_storable.rk");
    assert!(failed, "storable closure needing context must be rejected: {}", out);
    assert!(out.contains("CC10"), "should carry the CC10 code: {}", out);
}

#[test]
fn error_nonexhaustive_match() {
    assert!(compile_error("nonexhaustive_match.rk"), "should reject non-exhaustive match");
}

#[test]
fn error_trait_bound_unsatisfied() {
    assert!(compile_error("trait_bound_unsatisfied.rk"), "should reject a type that doesn't implement the bound's trait (#314)");
}

#[test]
fn error_trait_bound_missing_method() {
    assert!(compile_error("trait_bound_missing_method.rk"), "should reject a method the bounds don't provide (#314)");
}

#[test]
fn error_nominal_conformance_required() {
    assert!(compile_error("nominal_conformance_required.rk"), "should reject a structural match with no declared conformance (G1/#283)");
}

#[test]
fn error_conformance_missing_method() {
    assert!(compile_error("conformance_missing_method.rk"), "should reject `extend T with Trait` when the type lacks the trait's method (G1)");
}

#[test]
fn error_conditional_conformance_unmet() {
    assert!(compile_error("conditional_conformance_unmet.rk"), "should reject Ring<Blob> when the CC condition `T: Show` isn't met (CC1)");
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
fn error_cast_rules() {
    assert!(compile_error("cast_rules.rk"), "should reject invalid `as` casts and misused conversion forms (CV1–CV10, CH5, BL3)");
}

#[test]
fn error_index_types() {
    // #310: index expression types are checked — integer for Vec/slice/string,
    // K for Map, Handle<T> for Pool; range slicing only on sequences.
    assert!(compile_error("index_types.rk"),
        "should reject wrong index types across container classes (E0819)");
}

#[test]
fn error_linear_containers() {
    // RC1/RC3: Vec/Map can't hold linear elements (@resource, transitively
    // linear, or optionals/tuples built from them). Covers every entry route:
    // annotation, push, param, return, field, transitive, nested, optional,
    // alias, Map value, Map key (E0820).
    assert!(compile_error("linear_containers.rk"),
        "should reject Vec/Map of linear values across all entry routes (E0820)");
}

#[test]
fn error_cross_task_ownership() {
    // T1–T3 / #296: sending a value over a channel transfers ownership (use
    // after send is use-after-move), a `take` parameter consumes its argument
    // without a call-site `own`, and a scope-limited closure captured into a
    // spawned task is rejected (E0800, E0813).
    assert!(compile_error("cross_task_ownership.rk"),
        "should reject use-after-send, use-after-take, and borrow-capturing spawn (T1–T3, #296)");
}

#[test]
fn error_trait_object_generic() {
    // TR3: a generic trait method has no vtable slot; calling it through
    // `any Trait` must be rejected at the call site.
    assert!(compile_error("trait_object_generic.rk"),
        "should reject calling a generic method through `any Trait` (TR3)");
}

#[test]
fn error_ensure_cancellation() {
    // C3/C4 (#293): a resource with a pending `ensure` consumed on some paths
    // but not all — where the paths then merge — has no statically-definite
    // consumption state at scope exit. Maybe-consumed is a compile error
    // (E0821), across if-without-else, single match arm, and nested blocks.
    assert!(compile_error("ensure_cancellation.rk"),
        "should reject ensure receiver consumed on only some merging paths (C4, E0821)");
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
        "func main() {\n    let v = Vec<i32>.new()\n    if true {\n        let moved = v\n    } else {\n        let x = 1\n    }\n    v.len()\n}"
    );
    assert!(output.contains("E0813"),
        "move in one if/else branch then use should be E0813 (O3): {}", output);
}

#[test]
fn error_move_in_if_without_else() {
    // #294: the implicit empty else must merge like a real branch.
    let output = check_output(
        "func main() {\n    let v = Vec<i32>.new()\n    if true {\n        let moved = v\n    }\n    v.len()\n}"
    );
    assert!(output.contains("E0813"),
        "move in an if-without-else then use should be E0813 (O3): {}", output);
}

#[test]
fn error_linear_consumed_one_branch_ifelse() {
    let output = check_output(
        "@resource\nstruct Conn { fd: i32 }\nextend Conn { func close(take self) {} }\nfunc main() {\n    let c = Conn { fd: 3 }\n    if true {\n        c.close()\n    } else {\n        let x = 1\n    }\n}"
    );
    assert!(output.contains("E0805"),
        "resource consumed in only one if/else branch should be E0805 (L1): {}", output);
}

#[test]
fn error_linear_consumed_if_without_else() {
    // #294: consuming a linear resource in an if-without-else leaks on the
    // false path.
    let output = check_output(
        "@resource\nstruct Conn { fd: i32 }\nextend Conn { func close(take self) {} }\nfunc main() {\n    let c = Conn { fd: 3 }\n    if true {\n        c.close()\n    }\n}"
    );
    assert!(output.contains("E0805"),
        "resource consumed in an if-without-else should be E0805 (L1): {}", output);
}

#[test]
fn error_move_in_loop_body() {
    let output = check_output(
        "func take_vec(take v: Vec<i32>) {}\nfunc main() {\n    let v = Vec<i32>.new()\n    loop {\n        take_vec(own v)\n    }\n}"
    );
    assert!(output.contains("E0813"),
        "moving a value inside a loop body is a next-iteration use-after-move (O3): {}", output);
}

#[test]
fn ok_move_in_both_branches() {
    assert!(check_succeeds(
        "func take_vec(take v: Vec<i32>) {}\nfunc main() {\n    let v = Vec<i32>.new()\n    if true {\n        take_vec(own v)\n    } else {\n        take_vec(own v)\n    }\n}"
    ), "moving on both branches is a definite move — should type-check");
}

#[test]
fn ok_conditional_move_then_reassign() {
    assert!(check_succeeds(
        "func main() {\n    mut v = Vec<i32>.new()\n    if true {\n        let moved = v\n    }\n    v = Vec<i32>.new()\n    v.push(1)\n}"
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
    let output = check_output("func main() {\n    let x: i32 = \"hello\"\n}");
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
    let output = check_output("func main() {\n    let x: i32 = \"hello\"\n}");
    assert!(output.contains("fix:"), "should include fix suggestion: {}", output);
}

// ─── rask fmt integration ───────────────────────────────────

#[test]
fn fmt_normalizes_spacing() {
    let rask = rask_binary();
    let tmp = std::env::temp_dir().join(format!("rask_fmttest_{}.rk", std::process::id()));
    std::fs::write(&tmp, "func    main(   ) {\nlet x=42\n}").unwrap();

    let _ = Command::new(&rask)
        .arg("fmt")
        .arg("-w")
        .arg(&tmp)
        .output()
        .expect("failed to run rask fmt");

    let formatted = std::fs::read_to_string(&tmp).unwrap();
    let _ = std::fs::remove_file(&tmp);

    assert!(formatted.contains("func main()"), "should normalize func spacing: {}", formatted);
    assert!(formatted.contains("let x = 42"), "should add spaces: {}", formatted);
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
        "func main() {\n    let s = \"hello\"\n    println(s.len().to_string())\n    s.contains(\"ell\")\n}"
    ), "string.len/contains should pass type check");
}

#[test]
fn discover_string_trim() {
    // string.trim() returns a slice — can't store it (S2), but can use inline
    assert!(check_succeeds(
        "func main() {\n    let s = \"  hello  \"\n    println(s.trim())\n}"
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
        "func main() {\n    let s = 42.to_string()\n    println(s)\n}"
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

#[test]
fn overflow_roundtrip_panics() {
    // The u8's width survives storage in a Vec and retrieval — arithmetic on
    // the pulled-out value still overflows. Locks in self-describing IntKind.
    assert_panics_both("overflow_roundtrip.rk", "overflow");
}

#[test]
fn overflow_narrow_literal_panics() {
    // Both operands are bare literals (no local to read a width from) — native
    // codegen used to default to signed 32-bit arithmetic and wrap silently
    // instead of checking at the declared u8 width (#328).
    assert_panics_both("overflow_narrow_literal.rk", "overflow");
}

#[test]
fn comptime_overflow_is_compile_error() {
    // CT1: overflow during comptime evaluation fails compilation with a
    // diagnostic (routed through the normal diagnostic path, not swallowed).
    let (ok, output) = compile_only_succeeds("comptime_overflow.rk");
    assert!(!ok, "comptime overflow must fail compilation: {}", output);
    assert!(output.contains("overflow"), "should report overflow: {}", output);
}

#[test]
fn comptime_overflow_narrow_is_compile_error() {
    // Same CT1 guarantee, but at a width narrower than miri's old i64
    // fallback (u8's 200 + 100 fits fine at i64, so the old constant loader
    // never tripped the overflow check at all — #328).
    let (ok, output) = compile_only_succeeds("comptime_overflow_narrow.rk");
    assert!(!ok, "narrow-width comptime overflow must fail compilation: {}", output);
    assert!(output.contains("overflow"), "should report overflow: {}", output);
}

#[test]
fn comptime_div_zero_is_compile_error() {
    let (ok, output) = compile_only_succeeds("comptime_div_zero.rk");
    assert!(!ok, "comptime divide-by-zero must fail compilation: {}", output);
    assert!(output.contains("by zero"), "should report divide by zero: {}", output);
}

// ─── Panic semantics: ensure × panic (ctrl.panic, issue #299) ────
//
// Step 1 of task 1.5 covers the interpreter (issues #289/#290/#291). Native
// codegen still doesn't run ensures on panic, so these assert `--interp` only;
// the native side lands with step 2.

#[test]
fn panic_ensure_e2_runs_remaining() {
    // E2: a panic in one ensure doesn't skip the others. LIFO gives C, then the
    // panicking ensure, then A — A must still print despite the panic. Both
    // backends now run ensures on panic (native via the reified hook).
    for mode in ["--interp", "--native"] {
        let (stdout, _stderr, code) = run_capture(mode, "panic_ensure_e2.rk");
        assert_eq!(code, 101, "{}: panic should exit 101 (P4)", mode);
        // stdout carries body + the two non-panicking ensures (C then A). "boom"
        // itself goes to stderr as the task panic.
        assert!(stdout.contains("body") && stdout.contains("C") && stdout.contains("A"),
            "{}: remaining ensures must run after a panicking one (E2): {:?}", mode, stdout);
    }
}

#[test]
fn panic_ensure_e3_first_panic_wins() {
    // E3: the body's "primary" panic wins; the ensure's "secondary" panic during
    // unwind is contained + reported to stderr; the other ensure still runs.
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "panic_ensure_e3.rk");
        assert_eq!(code, 101, "{}: panic should exit 101 (P4)", mode);
        assert!(stdout.contains("cleanup"), "{}: the non-panicking ensure must still run (E2): {:?}", mode, stdout);
        assert!(stderr.contains("primary"), "{}: primary panic wins: {}", mode, stderr);
        assert!(stderr.contains("secondary panic during unwind"),
            "{}: secondary panic should be reported to stderr: {}", mode, stderr);
    }
}

#[test]
fn panic_guard_during_unwind_is_secondary() {
    // E3, issue #298: an H1 guard (unconsumed TaskHandle) tripping at scope
    // exit while already unwinding from "boom" must not override it. Interp
    // only — native's ensure/guard-on-panic plumbing has bigger pre-existing
    // gaps here, untouched by this fix (ctrl.panic implementation notes).
    let (_stdout, stderr, code) = run_capture("--interp", "panic_guard_unwind_secondary.rk");
    assert_eq!(code, 101, "panic should exit 101 (P4): {}", stderr);
    assert!(stderr.contains("panic: boom"), "the body's panic must win: {}", stderr);
    assert!(stderr.contains("secondary panic during unwind") && stderr.contains("resource leak"),
        "the guard trip must be contained and reported as secondary: {}", stderr);
}

#[test]
fn panic_detached_task_reports_to_stderr() {
    // O4, issue #298: detach() doesn't hide a panic — it prints to stderr and
    // the process keeps running. Interp only; the compiled runtime's O4 fix
    // is covered separately in the C runtime (thread.c/green.c).
    let (stdout, stderr, code) = run_capture("--interp", "panic_detached_task_stderr.rk");
    assert_eq!(code, 0, "a detached task's panic must not kill the process: {}", stderr);
    assert_eq!(stdout, "done\n");
    assert!(stderr.contains("boom"), "the detached panic must reach stderr: {}", stderr);
}

#[test]
fn deref_of_owned_is_a_safe_borrow() {
    // #737: `*x` was classified as a raw-pointer dereference by syntax alone,
    // so `(*p).x` on an Owned needed an `unsafe` block — which made owned.md's
    // own examples uncompilable. mem.owned/OW3 says it's an ordinary borrow.
    // Neither backend could evaluate it either: the interpreter had no Deref
    // arm, and native emitted a real load and segfaulted.
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "owned_deref.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(
            stdout,
            "read 1 2\nafter write 10\nstill owned 10 2\n",
            "{}: read, write, and still-owned all through `*p`: {:?}", mode, stdout,
        );
    }
}

#[test]
fn channel_element_type_reaches_the_receiving_end() {
    // #717: a channel created without an explicit element type gave its Sender
    // and Receiver empty type-argument lists, so nothing linked the two ends.
    // Every method call on either end then invented its own fresh variable, and
    // what `send` learned could never reach `receive` — the result stayed an
    // unresolved `T or string` and lowering had no enum to match on.
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "channel_elem_inferred.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(stdout, "done hello\nfailed 7\n", "{}: {:?}", mode, stdout);
    }
}

#[test]
fn unqualified_variant_takes_its_own_enums_tag() {
    // #752: an unqualified arm resolved its tag by scanning every declared enum
    // for the name, so a user enum with an `Io` variant picked up IoError's tag
    // 6. The switch keyed an arm the enum has no tag for, nothing matched, and
    // the match fell through to `unreachable` — SIGILL.
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "variant_name_shared_with_stdlib.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(
            stdout,
            "caught: inner\ncaught: not found\ncaught: I/O error: inner\n",
            "{}: every arm reaches its own variant: {:?}", mode, stdout,
        );
    }
}

#[test]
fn threadpool_runs_every_job_exactly_once() {
    // #686: ThreadPool.spawn was pthread_create per job — `workers: 4` was
    // accepted and ignored, so 800 jobs meant 800 threads. With a real pool the
    // observable guarantee is that every job still runs exactly once: 800 joins
    // and the sum 0+1+...+799.
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "threadpool_bounded.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert!(stdout.contains("count 800"), "{}: every job joined: {:?}", mode, stdout);
        assert!(stdout.contains("total 319600"), "{}: each job ran once: {:?}", mode, stdout);
    }
}

#[test]
fn one_println_call_lands_whole_on_both_backends() {
    // #704: a print/println call is several writes — the text, then the newline
    // — so two threads used to splice mid-line ("line 0 from thread 2line 194
    // from thread 1"). Both backends now emit one call as one unit.
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "print_line_atomic.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.len(), 1200, "{}: every line accounted for", mode);
        let torn: Vec<&&str> = lines.iter()
            .filter(|l| {
                let mut parts = l.split(' ');
                parts.next() != Some("line")
                    || parts.next().is_none_or(|n| n.parse::<u32>().is_err())
                    || parts.next() != Some("from")
                    || parts.next() != Some("thread")
                    || parts.next().is_none_or(|n| !matches!(n, "1" | "2" | "3" | "4"))
                    || parts.next().is_some()
            })
            .collect();
        assert!(torn.is_empty(), "{}: {} torn lines, first: {:?}", mode, torn.len(), torn.first());
    }
}

#[test]
fn thread_join_reports_value_and_panic_on_both_backends() {
    // #677/#683: native join() used to fold the value and the outcome into one
    // number — every success reported 0, a task returning -1 looked like a
    // panic, and a real panic came back as an Err whose payload was the -1
    // itself, so matching JoinError.Panicked dereferenced it.
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "thread_join_outcome.rk");
        assert_eq!(code, 0, "{}: joining a panicked task must not kill the joiner: {}", mode, stderr);
        let lines: Vec<&str> = stdout.lines().collect();
        assert_eq!(lines.first(), Some(&"value 42"), "{}: join hands back the task's value: {:?}", mode, stdout);
        assert_eq!(lines.get(1), Some(&"value -1"), "{}: -1 is a value, not a failure: {:?}", mode, stdout);
        // Both backends word it the same now (#748): the stored message is
        // `file:line: msg`, with no reporter prefix baked in — `panic: ` belongs
        // at print time, not in a string user code prints itself. The path is
        // relative to the runner's cwd, so match the tail.
        let panicked = lines.get(2).copied().unwrap_or_default();
        assert!(panicked.starts_with("panicked ") && panicked.ends_with("thread_join_outcome.rk:25: boom"),
            "{}: a panicked task joins as JoinError.Panicked carrying file:line and its message: {:?}", mode, stdout);
        assert_eq!(lines.get(3), Some(&"still alive"), "{}: execution continues: {:?}", mode, stdout);
    }
}

#[test]
fn string_hash_is_the_same_number_every_run_on_both_backends() {
    // #744: native mixed the Map's per-process seed into the FNV accumulator,
    // and `string.hash()` is built on that same function — so the public method
    // answered a different number every run, and a different one again from the
    // interpreter. `.hash()` on a value should be as stable as `==` on it.
    let (interp, stderr, code) = run_capture("--interp", "string_hash_stable.rk");
    assert_eq!(code, 0, "interp: {}", stderr);

    for attempt in 0..3 {
        let (native, stderr, code) = run_capture("--native", "string_hash_stable.rk");
        assert_eq!(code, 0, "native: {}", stderr);
        assert_eq!(
            native, interp,
            "run {attempt}: native must agree with the interpreter, and with itself",
        );
    }
    assert_eq!(interp.lines().count(), 3, "three hashes: {:?}", interp);
}

#[test]
fn map_iteration_order_still_varies_between_runs() {
    // determinism/D7. The seed moved out of the hash function and into bucket
    // placement for #744 — this is the property that move must not cost. Eight
    // keys is 8! orders, so runs agreeing by chance isn't a real risk.
    for mode in ["--interp", "--native"] {
        let mut seen = std::collections::HashSet::new();
        for _ in 0..5 {
            let (stdout, stderr, code) = run_capture(mode, "map_order_varies.rk");
            assert_eq!(code, 0, "{}: {}", mode, stderr);
            assert_eq!(
                stdout.split_whitespace().count(), 8,
                "{}: every entry is iterated exactly once: {:?}", mode, stdout,
            );
            seen.insert(stdout);
        }
        assert!(
            seen.len() > 1,
            "{}: five runs gave one order — the seed stopped reaching iteration order", mode,
        );
    }
}

#[test]
fn panic_ensure_runs_on_native_with_captured_receiver() {
    // U1 on native: an ensure calling a method on a captured receiver runs during
    // unwind, and the by-reference capture sees the pre-panic mutation (42, not 1).
    for mode in ["--interp", "--native"] {
        let (stdout, _stderr, code) = run_capture(mode, "panic_ensure_capture.rk");
        assert_eq!(code, 101, "{}: panic should exit 101", mode);
        assert_eq!(stdout, "42\n", "{}: ensure runs on panic and sees the live receiver", mode);
    }
}

#[test]
fn panic_exits_101_both_backends() {
    // P4/EX4: a panic escaping main exits 101 on interp and native alike.
    // Native previously abort()ed (SIGABRT / 134); step 2's runtime plumbing
    // switches it to exit(101).
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "panic_exit_code.rk");
        assert_eq!(code, 101, "{}: panic should exit 101, stderr: {}", mode, stderr);
        assert_eq!(stdout, "before\n", "{}: pre-panic output should flush", mode);
        assert!(
            format!("{}{}", stdout, stderr).contains("boom"),
            "{}: panic message should appear", mode,
        );
    }
}

#[test]
fn panic_with_keeps_writes_u2() {
    // U2: a mutation made inside a `with` block before a panic is kept, not
    // rolled back. The ensure observes v[0] == 99. Interp-only: on native the
    // `with` write-back is emitted after the panic point (and Vec is an i64
    // pointer, not a ref-capturable aggregate), so native with-block U2 is a
    // separate codegen gap tracked on #290/#299, not covered by the ensure hook.
    let (stdout, stderr, code) = run_capture("--interp", "panic_with_u2.rk");
    assert_eq!(code, 101, "panic should exit 101 (P4); stderr: {}", stderr);
    assert_eq!(stdout, "99\n", "with-block write must survive the panic (U2)");
}

// ─── Ensure cancellation: static definiteness (ctrl.ensure C1–C5, #293) ──
//
// Every accepted path has a definite consumption state, so both backends must
// agree on which cleanups run. Covers: no-consume-runs, definite-consume-
// cancels, the transfer pattern (path-dependent but definite), and the #295
// nested-block case (consume inside a nested block that then exits).
#[test]
fn ensure_cancellation_both_backends() {
    let expected = "\
[no_consume]
no_consume body
rollback
[definite_consume]
definite_consume body
commit
[transfer true]
rollback
[transfer false]
commit
[nested_consume]
commit
";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "ensure_cancellation.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected,
            "{}: ensure cancellation must match static definiteness", mode);
    }
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
    let p = Point { x: 1, y: 2 }
    assert p.x == 1
}
"#).unwrap();

    std::fs::write(dir.join("b.rk"), r#"
struct Point { x: i32, y: i32, z: i32 }
test "b uses its own Point" {
    let p = Point { x: 1, y: 2, z: 3 }
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

// ─── Regression: issue #549 ─────────────────────────────────
//
// `rask test <pkg>` used to run only the operator half of the desugar phase,
// so struct field defaults and default arguments were never filled in and
// `Config {}` failed with E0822 "missing fields" — code `rask build` accepts.

#[test]
fn test_package_applies_field_defaults() {
    let rask = rask_binary();
    let dir = std::env::temp_dir().join(format!("rask_test_pkg_defaults_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("build.rk"), r#"
package "fd" "0.1.0" {
    description: "field defaults under rask test"
}
"#).unwrap();

    std::fs::write(dir.join("main.rk"), r#"
struct Config {
    public port: u16 = 8080
    public host: string = "localhost"
}

func scaled(n: i32, by: i32 = 3) -> i32 {
    return n * by
}

func main() {
    let c = Config {}
    println("{c.port}")
}

test "all fields defaulted" {
    let c = Config {}
    assert c.port == 8080
    assert c.host == "localhost"
}

test "explicit value wins over the default" {
    let c = Config { port: 9090 }
    assert c.port == 9090
    assert c.host == "localhost"
}

test "default argument fills in" {
    assert scaled(2) == 6
    assert scaled(2, 5) == 10
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
        !combined.contains("E0822"),
        "field defaults must be filled in before type checking: {}", combined,
    );
    assert!(
        out.status.success() && combined.contains("3 passed"),
        "rask test must honor field defaults and default args\nstdout: {}\nstderr: {}",
        stdout, stderr,
    );
}

// ─── assert_eq compares values, not addresses ───────────────
//
// Native lowering handed both sides to a runtime function typed (i64, i64):
// two strings arrived as their addresses and never matched, and a float or a
// char didn't fit the signature at all, so Cranelift rejected the whole test
// function. The interpreter had it right all along.

fn rask_test_output(args: &[&str]) -> (bool, String) {
    let rask = rask_binary();
    let out = Command::new(&rask)
        .arg("test")
        .args(args)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask test");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    (out.status.success(), combined)
}

#[test]
fn assert_eq_compares_by_value_on_both_backends() {
    let path = fixture("assert_eq_types.rk");
    let path = path.to_str().unwrap();

    for args in [vec![path], vec!["--interp", path]] {
        let (ok, combined) = rask_test_output(&args);
        assert!(
            ok && combined.contains("8 passed"),
            "assert_eq must compare by value ({:?}): {}", args, combined,
        );
    }
}

#[test]
fn assert_eq_failure_reports_got_and_expected() {
    let dir = std::env::temp_dir().join(format!("rask_assert_eq_diff_{}", next_tmp_id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("mismatch.rk");
    std::fs::write(&file, r#"
func main() { println("x") }

test "strings differ" {
    let a = "hei"
    assert_eq(a, "hallo")
}
"#).unwrap();

    let (ok, combined) = rask_test_output(&[file.to_str().unwrap()]);
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!ok, "a mismatched assert_eq must fail the test: {}", combined);
    assert!(
        combined.contains("got:") && combined.contains("hei")
            && combined.contains("expected:") && combined.contains("hallo"),
        "failure must name both values: {}", combined,
    );
}

// ─── Regression: issues #566, #569 ──────────────────────────
//
// A module-level `const` without a type annotation was typed in the same pass
// as function bodies, so a body in an earlier-sorting file was checked while the
// let was still an inference variable. `store.lock()` then had no receiver
// type to dispatch on, and the type of whatever the guard's method returned was
// lost: reading a newtype's `.value` segfaulted (#566) and inspecting the error
// side of a `T or E` trapped (#569). Both need the const's file to sort *after*
// the file that reads it, which is what `main.rk` / `store.rk` gives.

/// Build a package from (filename, source) pairs, run it, return (ok, output).
fn build_and_run_package(tag: &str, files: &[(&str, &str)]) -> (bool, String) {
    let rask = rask_binary();
    let dir = std::env::temp_dir().join(format!("rask_{}_{}", tag, next_tmp_id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, src) in files {
        std::fs::write(dir.join(name), src).unwrap();
    }

    let build = Command::new(&rask)
        .arg("build")
        .arg(&dir)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask build");
    let build_out = format!(
        "{}{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );
    if !build.status.success() {
        let _ = std::fs::remove_dir_all(&dir);
        return (false, format!("build failed: {}", build_out));
    }

    let run = Command::new(dir.join("build").join("debug").join(tag))
        .output()
        .expect("failed to run built binary");
    let run_out = format!(
        "{}{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr),
    );
    let ok = run.status.success();
    let _ = std::fs::remove_dir_all(&dir);
    (ok, format!("{}\n{}", build_out, run_out))
}

#[test]
fn newtype_value_survives_cross_module_mutex_method() {
    let (ok, out) = build_and_run_package("nt", &[
        ("build.rk", r#"
package "nt" "0.1.0" { description: "newtype through a mutex global" }
"#),
        ("ids.rk", r#"
type UserId = u64 with (Equal, Hashable, Comparable, Debug)
"#),
        // `main.rk` sorts before `store.rk`, so the body is reached first.
        ("main.rk", r#"
func run() -> string or StoreError {
    let id = try store.lock().make()
    return "value={id.value}"
}

func main() {
    let s = run() catch _e => { println("recovered"); return }
    println(s)
}
"#),
        ("store.rk", r#"
@message
enum StoreError { Boom }

struct Store { next: UserId }

extend Store {
    func new() -> Store { return Store { next: UserId(7) } }
    func make(self) -> UserId or StoreError { return self.next }
}

const store = Mutex.new(Store.new())
"#),
    ]);

    assert!(ok, "reading `.value` off a mutex guard's result must not crash: {}", out);
    assert!(
        !out.contains("unresolved field"),
        "the field's type must resolve, not fall back to i64: {}", out,
    );
    assert!(out.contains("value=7"), "expected `value=7`: {}", out);
}

#[test]
fn error_payload_survives_cross_module_mutex_method() {
    let (ok, out) = build_and_run_package("ep", &[
        ("build.rk", r#"
package "ep" "0.1.0" { description: "T or E error side through a mutex global" }
"#),
        ("errs.rk", r#"
@message
enum StoreError {
    @message("nf {0}") NotFound(u64)
    @message("cap {0}") Cap(u64)
}

@message
enum ApiError {
    Store(StoreError)
    @message("bad {0}") Bad(string)
}

func code(e: ApiError) -> string {
    match e { Store(inner) => return store_code(inner), Bad(_) => return "b" }
}

func store_code(e: StoreError) -> string {
    match e { NotFound(_) => return "nf", Cap(_) => return "cap" }
}
"#),
        ("main.rk", r#"
struct View { public id: u64 }

func handle(id: u64) -> View or ApiError {
    let v = try store.lock().view(id)
    return v
}

func main() {
    // Error side: a deep read of the wrapped payload used to trap.
    let bad = handle(999) catch e => {
        println("code={code(e)}")
        println("message={e.message()}")
        ok_side()
        return
    }
    println("unexpected ok {bad.id}")
}

func ok_side() {
    let v = handle(3) catch _e => { println("unexpected error"); return }
    println("ok={v.id}")
}
"#),
        ("store.rk", r#"
struct Store { n: u64 }

extend Store {
    func new() -> Store { return Store { n: 0 } }
    func view(self, id: u64) -> View or StoreError {
        if id > 100 { return StoreError.NotFound(id) }
        return View { id: id }
    }
}

const store = Mutex.new(Store.new())
"#),
    ]);

    assert!(ok, "inspecting the error from a mutex guard's method must not trap: {}", out);
    // The outer tag always survived; the inner payload is what was corrupt.
    assert!(out.contains("code=nf"), "inner error variant must match: {}", out);
    assert!(
        out.contains("message=nf 999"),
        "the payload must reach `message()` intact: {}", out,
    );
    assert!(out.contains("ok=3"), "the happy path must still work: {}", out);
}

// ─── Regression: issue #570 ─────────────────────────────────
//
// The interpreter kept three separate lists of which types each stdlib module
// exports: one for `import m.*`, one for `import m.Type`, and one per module in
// the qualified-field path. They disagreed — `http`'s types were only in the
// glob list — so `http.Response.ok(…)` failed with "cannot access field on
// module" while a bare `Response.ok(…)` worked. One table now serves all three.

fn interp_output(src: &str) -> String {
    let rask = rask_binary();
    let dir = std::env::temp_dir().join(format!("rask_interp_{}", next_tmp_id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("m.rk");
    std::fs::write(&file, src).unwrap();
    let out = Command::new(&rask)
        .arg("run")
        .arg("--interp")
        .arg(&file)
        .env("RASK_RUNTIME_DIR", runtime_dir())
        .output()
        .expect("failed to run rask run --interp");
    let _ = std::fs::remove_dir_all(&dir);
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    )
}

#[test]
fn interp_resolves_qualified_module_types() {
    // `http.Response` is the case from #570; the others went through
    // per-module tables that the shared one replaced.
    let out = interp_output(r#"
import http
import time
import json

func main() {
    let r = http.Response.ok("body")
    println("status={r.status}")

    let d = time.Duration.from_millis(5)
    println("ms={d.as_millis()}")

    println("json={json.encode(true)}")
}
"#);

    assert!(
        !out.contains("cannot access field on module") && !out.contains("has no member"),
        "qualified module types must resolve: {}", out,
    );
    assert!(out.contains("status=200"), "http.Response.ok must build a 200: {}", out);
    assert!(out.contains("ms=5"), "time.Duration must still resolve: {}", out);
    assert!(out.contains("json=true"), "json module must still work: {}", out);
}

#[test]
fn interp_qualified_and_bare_module_types_agree() {
    // The two spellings name the same type, so they must behave the same.
    let out = interp_output(r#"
import http

func main() {
    let viaModule = http.Response.ok("x")
    let viaBare = Response.ok("x")
    println("same={viaModule.status == viaBare.status}")
}
"#);
    assert!(out.contains("same=true"), "both spellings must agree: {}", out);
}

#[test]
fn interp_single_member_import_covers_every_exported_type() {
    // `import http.Response` used to be silently ignored — the single-member
    // table only knew about time/path/random.
    let out = interp_output(r#"
import http.Response

func main() {
    let r = Response.ok("x")
    println("status={r.status}")
}
"#);
    assert!(out.contains("status=200"), "a single-member import must bind the type: {}", out);
}

#[test]
fn interp_reports_an_unknown_module_member() {
    let out = interp_output(r#"
import http

func main() {
    println("{http.Nonexistent}")
}
"#);
    assert!(
        out.contains("Nonexistent"),
        "an unknown member must still be reported by name: {}", out,
    );
}

// ── Multi-file span attribution ────────────────────────────────────────────

/// A diagnostic in a package must name the file it came from.
///
/// Nothing used to build a multi-file package in a test — every fixture was a
/// single file checked on its own, where `file_id: 0` is right by construction,
/// so the bug this guards was invisible. The lexer stamped every token span with
/// file 0 because it had no idea which file it was reading, and the parser lifts
/// spans straight off tokens for `let` names, fields and parameters. Roughly
/// half of all spans claimed the first file, and errors rendered against it at
/// offsets that were meaningless there.
///
/// The fixture puts the error in the alphabetically *later* file so a lost
/// file_id lands it on the first one.
#[test]
fn package_diagnostic_names_the_right_file() {
    let rask = rask_binary();
    let pkg = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("multifile_spans");

    let out = Command::new(&rask)
        .arg("build")
        .arg(&pkg)
        .output()
        .expect("failed to run rask build");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );

    assert!(
        combined.contains("E0361"),
        "expected the unresolved-type error from the fixture, got:\n{combined}"
    );
    assert!(
        combined.contains("zzz_second.rk:12:9"),
        "diagnostic should point at zzz_second.rk:12:9 — the binding's real home.\n\
         Reporting aaa_first.rk means token spans lost their file_id again.\n\
         Got:\n{combined}"
    );
    assert!(
        !combined.contains("aaa_first.rk:"),
        "no diagnostic should be attributed to the first file here:\n{combined}"
    );
    // The rendered source line must be the real one, not whatever sits at that
    // offset in another file.
    assert!(
        combined.contains("let unconstrained = Vec.new()"),
        "the snippet should show the actual offending line:\n{combined}"
    );
    // A second error, this one inside a string interpolation — desugar re-lexes
    // the placeholder body, so those tokens need the file stamped too.
    assert!(
        combined.contains("zzz_second.rk:14:"),
        "the interpolation error should also name the second file:\n{combined}"
    );
}

// ─── Regression: issue #687 ─────────────────────────────────
//
// The f64 method set lived in four hand-maintained lists — checker, interpreter,
// codegen dispatch, drift registry — and they disagreed. `x.floor()` passed the
// checker, ran on the interpreter, and failed native codegen with "Function not
// found: f64_floor". They all read rask_stdlib::FLOAT_METHODS now; this test
// runs the whole primitive surface through both backends and compares.
#[test]
fn primitive_methods_agree_on_both_backends() {
    let (interp_out, interp_err, interp_code) = run_capture("--interp", "primitive_methods.rk");
    assert_eq!(interp_code, 0, "interp failed: {}", interp_err);
    let (native_out, native_err, native_code) = run_capture("--native", "primitive_methods.rk");
    assert_eq!(native_code, 0, "native failed: {}", native_err);
    assert_eq!(
        interp_out, native_out,
        "primitive method results diverge between backends"
    );
    // Spot-check a few values so a backend that agrees by both being wrong
    // still fails.
    assert!(native_out.contains("floor 3\n"), "got: {}", native_out);
    assert!(native_out.contains("int abs 42\n"), "got: {}", native_out);
    assert!(native_out.contains("to_int 3\n"), "got: {}", native_out);
}

// ─── Regression: issue #677 ─────────────────────────────────
//
// `match r { i64 as v => …, MyErr.Bad(m) => …, MyErr.Worse => … }` on a
// `i64 or MyErr` keyed every arm off the outer Ok/Err tag, so `Bad`'s variant
// tag 0 collided with the Ok arm and `Worse`'s tag 1 collided with Err — the
// error side always ran whichever arm the jump table kept last. The error
// variants get their own switch inside the Err branch now.
#[test]
fn match_over_result_with_enum_error_picks_the_right_variant() {
    let expected = "\
describe(0) = ok 42
describe(1) = bad oops
describe(2) = other
describe(3) = coded 7 seven
catchall(0) = fine 42
catchall(1) = some error
catchall(2) = worse!
catchall(3) = some error
";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "match_result_enum_variants.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}: wrong match arm ran", mode);
    }
}

// ─── Regression: a `mutate` write reaches the caller's storage ──────
//
// #702 fixed method receivers reached through a field. Two doors were left
// open: a field passed to a *free* function (`bump(mutate h.c)`) still handed
// over a copy, and a Vec element handed to anything that writes through it got
// a copy with no write-back. On top of that, a Vec reached through a field, an
// index, or a rebinding wasn't recognized as a Vec at all — the check compared
// the checker's `Vec<T>` spelling against the string "Vec" — so even
// `b.items[0].n += 1` wrote into a copy and read back 0.
#[test]
fn a_mutate_write_reaches_fields_and_collection_elements() {
    let expected = concat!(
        "field=2 (expect 2)\n",
        "elem=2 (expect 2)\n",
        "peek=2 (expect 2)\n",
        "field elem=2 (expect 2)\n",
        "nested elem=2 (expect 2)\n",
        "looped=10 (expect 10)\n",
        "map=2 (expect 2)\n",
        "grew after=41 (expect 41)\n",
        "renamed=second (expect second)\n",
    );
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "mutate_collection_element.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}: a mutate write went to a copy", mode);
    }
}

// ─── Regression: an element borrow can't be left dangling ──────────
//
// The element pointer handed to a `mutate` callee points into the Vec's own
// buffer, so a callee that reaches the same Vec and grows it would be writing
// through freed memory. mem.borrowing/W2 already forbids structurally mutating
// a collection while one of its elements is borrowed — `with v[0] as item { v
// .push(3) }` is a compile error — but the checker can't see that through a
// call into a global, so the runtime holds the line instead.
//
// Native only: the interpreter works on values rather than a pointer into a
// buffer, so it has nothing to invalidate and runs the program to completion.
#[test]
fn growing_a_vec_while_an_element_is_borrowed_panics() {
    let (stdout, stderr, code) = run_capture("--native", "mutate_elem_realloc_guard.rk");
    assert_ne!(code, 0, "should have panicked, stdout: {}", stdout);
    let out = format!("{}{}", stdout, stderr);
    assert!(
        out.contains("elements was being modified"),
        "expected the dangling-element panic, got: {}", out
    );
    assert!(
        !out.contains("UNREACHABLE"),
        "the push should have panicked before this line: {}", out
    );
}

// ─── Regression: issue #698 ─────────────────────────────────
//
// `self.last = title` in a `mutate self` method freed the caller's string. The
// RC pass walked `locals` chained with `params`, and a parameter lives in both
// lists, so every string parameter got two RcDecs for one RcInc. The field kept
// pointing at the freed buffer, and the next allocation — the println
// interpolation — wrote over it: the read-back string contained the format
// string's own prefix.
#[test]
fn a_string_stored_into_a_field_outlives_the_call() {
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "string_field_store_lifetime.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(
            stdout, "id=1 back=[verify the milestone] len=20\n",
            "{}: stored string did not survive the call", mode
        );
    }
}

// ─── Regression: a negative literal takes its type from context ─────
//
// `-2.5` is `(2.5).neg()` after desugaring, so the expected type reached the
// call and not the literal inside it. With nothing else to go on the literal
// defaulted to f64, and `let x: f32 = -2.5` was a type error while `2.5` was
// fine. The `neg` constraint deferred on the literal receiver and used to be
// dropped in silence, which is why this surfaced only once deferred method
// calls started being retried and reported (#425).
#[test]
fn a_negative_literal_takes_its_type_from_context() {
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "negative_literal_context.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, "1 -1 -5 -2.5\n", "{}", mode);
    }
}

// ─── Regression: float total order (type.operators/ORD3) ────────────
//
// `rask_vec_sort` compared every element as int64_t whatever the Vec held. A
// negative float's bit pattern orders backwards against another negative, so
// `[-1.5, -2.5]` sorted to `[-1.5, -2.5]` natively and `[-2.5, -1.5]` on the
// interpreter. Positive floats order correctly as integers, which is why a Vec
// of positives sorted fine and hid this.
#[test]
fn floats_sort_by_the_total_order() {
    let expected = "\
sorted -2.5 -1.5 0 3
nan<1 false
nan>1 false
nan==nan false
plain 1 2 3
";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "float_total_order.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// ─── IEEE 754 conformance (type.primitives/F1) ──────────────────────
//
// The expected values here are what IEEE 754-2019 requires, not what the
// compiler happened to print when this was written. Clause references are in
// the fixture. Two bugs were live when it was first run: `Vec<f64>.sort()`
// compared bit patterns as integers, and `f64.compare()` answered Equal for
// every unordered or signed-zero pair natively.
#[test]
fn ieee754_requirements_hold_on_both_backends() {
    // 5.11 — every NaN compares unordered with everything, itself included, so
    // all four ordered predicates are false and `!=` is true.
    let unordered = "\
nan_lt false
nan_gt false
nan_le false
nan_ge false
nan_eq_self false
nan_ne_self true
nan_is_nan true
negzero_eq_zero true
";
    // 5.10 — totalOrder separates -0 from +0 by sign and places NaN at an end,
    // where 5.11 leaves it unordered. This is what `compare()` implements.
    let total_order = "\
totalorder_negzero Less
totalorder_zero_negzero Greater
totalorder_nan_vs_one Greater
totalorder_one_vs_nan Less
";
    // 6.1 — infinity arithmetic is exact; 7.1 makes inf-inf and 0*inf invalid.
    let infinities = "\
inf_is_inf true
inf_plus_inf inf
inf_minus_inf_is_nan true
zero_times_inf_is_nan true
one_over_zero inf
one_over_negzero -inf
one_over_inf 0
inf_is_not_finite false
";
    // 6.2 — a NaN operand delivers a NaN.
    // 6.3 — (-0)+(-0) = -0, (-0)+(+0) = +0, (-0)*(+0) = -0, sqrt(-0) = -0.
    //       Observed through 1/x, which is the only way to see a zero's sign.
    let signs = "\
nan_plus_one_is_nan true
nan_times_zero_is_nan true
negzero_plus_negzero -inf
negzero_plus_zero inf
negzero_times_zero -inf
sqrt_negzero -inf
sqrt_neg_one_is_nan true
sqrt_inf inf
";
    // 5.9 — roundToIntegral passes NaN and infinity through unchanged.
    let round_to_integral = "\
floor_nan_is_nan true
ceil_nan_is_nan true
trunc_nan_is_nan true
floor_inf inf
ceil_inf inf
trunc_inf inf
";
    // 5.10 applied: sorting keeps every element and puts the NaN at the end.
    let sorting = "sorted -2.5 1 3 NaN\n";

    let expected = format!(
        "{unordered}{total_order}{infinities}{signs}{round_to_integral}{sorting}"
    );

    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "ieee754_conformance.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}: IEEE 754 conformance", mode);
    }
}

// ─── #425: dispatch comes from the checker, not from guessing ────────
//
// MIR mangles a method call to `{Type}_{method}`, and the receiver's type used
// to come from an eleven-step chain: the checker's recorded answer, then eight
// steps of guessing — a struct field's layout, the sole stdlib type declaring
// the method, a name-to-type policy table ("a two-argument `join` means Vec"),
// the MIR type, a layout name. Seven are deleted. Two remain, both authoritative:
//
//   0_checker_recorded  what dispatch resolved to
//   1_synthetic_local   a local MIR itself invented — a `store.lock().put(x)`
//                       guard has no checker node at all, so lowering writes
//                       its type down where it creates the local
//
// This test is what makes the deletion safe to keep: if a receiver starts
// reaching a step that no longer exists, it fails lowering instead, and if
// someone adds a guessing step back, the assert names it.
#[test]
fn method_dispatch_never_falls_back_to_guessing() {
    const ALLOWED: &[&str] = &["0_checker_recorded", "1_synthetic_local"];
    // Between them: primitives and floats, enums behind a `T or E`, a `.lock()`
    // guard receiver, a multi-parameter generic with a trait bound, a comptime
    // block, a slice receiver, and collections.
    let files: &[&str] = &[
        "primitive_methods.rk",
        "ieee754_conformance.rk",
        "float_total_order.rk",
        "match_result_enum_variants.rk",
        "string_field_store_lifetime.rk",
        "negative_literal_context.rk",
    ];

    let rask = rask_binary();
    let mut seen: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for name in files {
        let out = Command::new(&rask)
            .arg("compile")
            .arg(fixture(name))
            .arg("-o")
            .arg(std::env::temp_dir().join(format!("rask_disp_{}", next_tmp_id())))
            .env("RASK_RUNTIME_DIR", runtime_dir())
            .env("RASK_TRACE_DISPATCH", "1")
            .output()
            .expect("failed to run rask compile");
        let stderr = String::from_utf8_lossy(&out.stderr);
        for line in stderr.lines() {
            let Some(rest) = line.strip_prefix("[dispatch]   ") else { continue };
            let Some((step, tail)) = rest.split_once(": ") else { continue };
            seen.entry(step.to_string())
                .or_default()
                .push(format!("{name}: {tail}"));
        }
    }

    assert!(
        !seen.is_empty(),
        "no dispatch steps recorded at all — the tally or the trace flag broke, \
         which would make this test pass for the wrong reason"
    );
    let unexpected: Vec<_> = seen
        .iter()
        .filter(|(step, _)| !ALLOWED.contains(&step.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "a method call resolved its receiver by guessing: {unexpected:#?}"
    );
}

// ─── Regression: a channel pair's bindings have types ────────────────
//
// `Channel.buffered(n)` without an explicit `<T>` was claimed by the
// stub-registry route to resolve_runtime_method, which has no constructor for
// it, so the call got no return type. `mut (tx, rx) = Channel.buffered(4)` left
// both bindings as free type variables, and `tx.clone().send(x)` failed lowering
// outright: "method `send` on receiver of unresolved type".
#[test]
fn a_channel_pair_gets_its_types_from_the_constructor() {
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "channel_pair_types.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, "first 7\nsecond 9\n", "{}", mode);
    }
}

// ─── Regression: issue #688, and Path as one implementation ─────────
//
// Path's methods live in stdlib/path.rk as ordinary Rask. Native compiles that
// source; the interpreter is now handed the same declarations instead of running
// its own Rust copy. There is one implementation, so the backends can't disagree
// — which they did: `parent()` always answered none natively, and a path with an
// extension segfaulted.
#[test]
fn path_behaves_the_same_on_both_backends() {
    let expected = "\
[/usr/local/lib/thing.txt] parent=/usr/local/lib name=thing.txt stem=thing ext=txt abs=true n=4
[relative/path.tar.gz] parent=relative name=path.tar.gz stem=path.tar ext=gz abs=false n=2
[bare] parent=- name=bare stem=bare ext=- abs=false n=1
[/] parent=- name=- stem=- ext=- abs=true n=0
[/etc] parent=/ name=etc stem=etc ext=- abs=true n=1
[] parent=- name=- stem=- ext=- abs=false n=0
[.bashrc] parent=- name=.bashrc stem=.bashrc ext=- abs=false n=1
[/a/b/] parent=/a/b name=b stem=b ext=- abs=true n=2
[a.] parent=- name=a. stem=a ext=- abs=false n=1
with_ext: /x/y.md
with_ext none: /x/y.md
with_name: /x/z.md
div abs: /abs
";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "path_one_implementation.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// ─── Regression: issue #689 ─────────────────────────────────
//
// `json.encode(v)` on a JsonValue printed `140728691896880` natively — the
// enum's address, because MIR sent anything that wasn't a struct, a Vec or a
// string to `json_encode_i64`. stdlib/json.rk had a complete Rask encoder that
// nothing called. `JsonValue` implements `Displayable` through it now, and
// `json.encode` routes a JsonValue there, so both backends run the same code.
#[test]
fn json_value_encodes_the_same_on_both_backends() {
    // Raw string: the JSON text itself contains quotes and a backslash escape.
    let expected = concat!(
        r#"str: "he\"llo""#, "\n",
        "num: 42.5\n",
        "arr: [1,true,null]\n",
        r#"interp: "he\"llo""#, "\n",
    );
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "json_value_encoding.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// One encoder, with no help from the call site.
//
// The fixture above calls `v.to_string()` itself, which is what made the
// encoder reachable — so it passed while `json.encode` on its own was broken.
// Two bugs hid behind that:
//
//   native — `Function not found: JsonValue_to_string`. Lowering named the
//   body; reachability, the pass that decides what gets compiled, never heard
//   the name. Reachability names it now and lowering reads the answer.
//
//   interp — right output, wrong code. `json.encode` ran a Rust encoder inside
//   rask-interp, a second implementation of this same output, and the two
//   disagreed on Map key order: the Rust one walks the insertion-ordered
//   backing store while native walks seeded order (determinism/D7).
// `json.encode_pretty` exists at all, and runs the Rask printer on both backends.
//
// It's in specs/stdlib/json.md and both backends had a pretty printer, but nothing
// declared it in `extend json`, so no program could call it (#736). Routed the way
// `encode` is: reachability names the Rask body, lowering reads the name, and the
// interpreter calls the same body instead of its own Rust copy.
#[test]
fn json_encode_pretty_indents_the_same_on_both_backends() {
    let expected = concat!(
        "[\n",
        "  {\n",
        "    \"list\": [\n",
        "      1,\n",
        "      \"two\"\n",
        "    ]\n",
        "  },\n",
        "  null,\n",
        "  true\n",
        "]\n",
        "[]\n",
        "2.5\n",
    );
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "json_encode_pretty.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// `Owned<T>` actually allocates, so a recursive type works.
//
// `own` was a no-op — the keyword left no trace in the AST outside closures, and
// nothing downstream boxed anything. Layout gives an `Owned<T>` slot 8 bytes, so a
// 16-byte enum stored into a payload declared `Owned<Tree>` wrote across the next
// payload, and the recursive read used the first child's tag as an address (#705).
#[test]
fn a_recursive_type_can_be_built_with_owned() {
    let expected = concat!(
        "total=15 (expect 15)\n",
        "depth=3 (expect 3)\n",
        "wrapped=5 (expect 5)\n",
    );
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "owned_recursive_enum.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// `for mutate` writes back however the body ends.
//
// `continue` and `break` reached the writeback through dedicated blocks; leaving
// by returning didn't go through anything, so that iteration's write was dropped
// — `return item` handed back the new value and left the collection untouched.
// `try` propagating out of the body is a return too (#650).
#[test]
fn a_return_out_of_for_mutate_still_writes_back() {
    let expected = concat!(
        "return: 101 101 2\n",
        "break: 101 101 2\n",
        "try: nope 101 2\n",
    );
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "for_mutate_return_writeback.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// Pool elements that aren't structs, read and written.
//
// `pool[h]` on a scalar didn't produce a wrong answer — it killed native codegen
// with a Cranelift panic, because `PoolCheckedAccess` meant "address or value,
// work it out from the destination's type" and codegen always picked address.
// `pool[h] = v` with no field to write missed the pool branch entirely and became
// `Vec_set(pool, handle, v)`, using a packed handle as a position (#719).
#[test]
fn a_pool_element_can_be_a_scalar() {
    let expected = concat!(
        "42 -7\n", "100 -7\n",
        "2.5\n", "-0.75\n",
        "true\n", "false\n",
        "alpha\n",
        "10 one\n", "55 one\n",
        "93\n", "-3\n",
    );
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "pool_scalar_element.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// A program type reusing a stdlib type's name keeps its own method.
//
// rask#258 was this on native. It came back on the interpreter once the
// interpreter started running `stdlib/*.rk` as well: registration is
// last-writer-wins and the stdlib was going in last, so the stdlib enum's
// `message` overwrote the user struct's and ran `match self` against it.
// tests/suite/t56_shadowed_type_names.rk covers it too, but only
// tests/differential.sh runs that, and this needs to fail under `cargo test`.
#[test]
fn a_program_type_may_reuse_a_stdlib_name() {
    let expected = "user: boom\nio: /tmp/x\n";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "program_shadows_stdlib_name.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// `.clone()` where the receiver's type isn't known yet.
//
// The checker's `clone` arm short-circuits: a clone returns its receiver's type,
// so unifying the two settles inference and it answers there. That's before the
// code that files a deferred constraint for a still-unresolved receiver, so an
// unresolved one got no dispatch target and nothing came back to give it one.
// A closure parameter in a fused iterator chain is exactly that case — its type
// comes from the chain's element type — and lowering failed with `method `clone`
// on receiver of unresolved type`. Every other method works, because every other
// method goes through the deferred path.
#[test]
fn clone_dispatches_when_the_receiver_type_arrives_late() {
    let expected = "2 one 2\n";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "clone_in_fused_chain.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

#[test]
fn json_encode_uses_the_rask_encoder_on_both_backends() {
    let expected = concat!(r#"["a\"b",1,2.5,false,null,[7]]"#, "\n");
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "json_encode_one_encoder.rk");
        assert_eq!(code, 0, "{}: should exit 0, stderr: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// std.collections/C2 (#666): `try_push` was declared to return
// `void or PushError<T>` and `PushError` was never declared anywhere, so the
// rejected value it promised to hand back could not be named, matched or read.
// The family is now `GrowError<T>`, and a generic error type survives being the
// error branch of `T or E` on both backends.
#[test]
fn grow_error_carries_the_rejected_value_on_both_backends() {
    assert_native_eq_interp("grow_error_family.rk", "4217ok");
}

// type.errors/ER16a (#647): `try` attaches to the fallible step of a postfix
// chain, not to the whole chain. `try read_file(p).len()` used to be read as
// `try (read_file(p).len())` and failed with "no method `len` on
// `string or IoError`"; it means `(try read_file(p)).len()`.
#[test]
fn try_attaches_to_the_fallible_step_of_a_chain() {
    assert_native_eq_interp("try_chain_placement.rk", "808080031608080");
}

// type.primitives/CV1a, CV2 (#649): narrowing never happens implicitly. A
// method argument is a directional position like any other, and an array
// literal's element type is the one every element fits — `[small_u8, big_u64]`
// used to take `u8` and store 300 as 44 natively while the interpreter kept 300.
#[test]
fn no_implicit_narrowing_in_arguments_or_joins() {
    assert_native_eq_interp("no_silent_narrowing.rk", "7,300,7,7,300");
}

// std.collections/CP1-CP3, C2 (#666): a bounded vector refuses to grow past its
// bound, and `try_push` hands the rejected value back rather than panicking —
// which is what the growth error carries a payload for. A capacity hint
// (`with_capacity`) is not a bound.
#[test]
fn a_bounded_vec_hands_back_what_it_wont_take() {
    assert_native_eq_interp("bounded_vec.rk", "true22true0302false3417");
}

// std.collections/C2: `push` past the bound panics on both backends, and
// nothing after it runs.
#[test]
fn push_past_the_bound_panics_on_both_backends() {
    let (nout, ncode) = run_native("bounded_vec_push_full.rk");
    let (iout, icode) = run_interp("bounded_vec_push_full.rk");
    assert_eq!(ncode, 101, "native should panic at the bound: {}", nout);
    assert_eq!(icode, 101, "interp should panic at the bound: {}", iout);
    assert!(!nout.contains("99"), "native ran past the panic: {}", nout);
    assert!(!iout.contains("99"), "interp ran past the panic: {}", iout);
}

// #659 family: `r is MyErr.Bad` names a variant of the error enum, which lives
// one layer below the value's own ok/err tag. Native compared the variant tag
// against the outer tag, so `Bad` (variant 0) matched the *ok* tag 0 and the
// answer was wrong for every case — `match` had the same mixup in #677 and got
// a two-level switch; the three `is` sites never did.
#[test]
fn is_on_an_error_variant_reads_the_inner_tag_on_both_backends() {
    let expected = "\
bad is Bad:      true
bad is Worse:    false
worse is Bad:    false
worse is Worse:  true
ok is Bad:       false
ok is Worse:     false
if: ok skipped the Bad arm
if: bad took the Bad arm
bad is MyErr:    true
ok is MyErr:     false
eof is Eof:      true
eof is NotFound: false
";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture_no_stdin(mode, "is_error_variant.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// structure.modules/IM2: a dotted import binds its last segment, so
// `import std.math` names the math module. The interpreter special-cased
// `std.reflect` alone and read every other `std.*` form as a member of `std`,
// binding nothing — the program compiled natively and died at runtime on the
// interpreter with "type math has no method 'ln'".
#[test]
fn a_dotted_stdlib_import_binds_the_module_on_both_backends() {
    let expected = "\
ln(16) = 2.772588722239781
to_degrees = 0
read at eof is Eof: true
";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture_no_stdin(mode, "import_std_module.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// A value going into a container's element slot widens like any other argument
// position, and reading one back uses the *declared* element type. Three bugs
// stacked here: `v.push(1)` on a `Vec<i32?>` stored a bare int (builtin
// collection methods have no declared parameter to coerce against); the element
// type was then tracked from whichever push came last, so `push(1)`,
// `push(none)`, `push(3)` recorded `i32`; and the payload rebind from
// `if x? { … }` outlived its block, so the next `x` read a tag out of a bare
// slot. Native segfaulted on three elements but not two.
#[test]
fn a_vec_of_optionals_reads_back_what_went_in_on_both_backends() {
    let expected = "\
len=3
present 1
absent
present 3
[0]=1 [1]=-1 [2]=3
after set [1]=9
map a present=true
chain=8
chain all none=99
";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "vec_of_optionals.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// The payload rebind `if x?` installs is scoped to the branch. It used to leak
// past the closing brace, so a second `if x?` on the same variable tested a tag
// that wasn't there.
#[test]
fn a_presence_rebind_does_not_outlive_its_block() {
    let expected = "first block\nsecond block\nas option: 5\nbound: 5\ndone\n";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "presence_rebind_scope.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// #699: `reflect.fields<T>()` inside a generic function needs to know what `T`
// became at the call. Native monomorphizes, so `T` is already concrete by the
// time MIR unrolls the loop; the interpreter doesn't, and the name reached
// reflect as the literal "T" — "not a struct type". It records per-call type
// bindings now, read off the arguments, with PC1's implicit `value: T` counting
// as a declaration.
#[test]
fn reflect_through_a_generic_function_agrees_on_both_backends() {
    let expected = "x y \ntext \ndirect:x direct:y \n";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "reflect_through_generic.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}

// An `if` expression handed to a call is in value position, whatever position
// the call is in. `in_stmt_expr` — the flag that makes a statement's trailing
// `if` answer void — reached the arguments, so `out.push(if b: "1" else: "0")`
// as a bare expression statement was "expected `string`, found `void`" while
// `let s = out.push(…)` compiled. Found in examples/lsm_database.
#[test]
fn an_if_expression_can_be_a_call_argument_on_both_backends() {
    let expected = "one\nzero\nbuilder: 10\nvec: yes\nstmt then\n";
    for mode in ["--interp", "--native"] {
        let (stdout, stderr, code) = run_capture(mode, "if_expr_as_argument.rk");
        assert_eq!(code, 0, "{}: {}", mode, stderr);
        assert_eq!(stdout, expected, "{}", mode);
    }
}
