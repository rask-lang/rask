// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The first day with Rask, as a newcomer lives it: make a project, run it,
//! test it, check it, and write the small programs and the common mistakes
//! everyone writes in their first hour. Each test here is something that was
//! broken or misleading for that newcomer once; it fails if it comes back.
//!
//! Programs are written inline so the test reads as the story it guards.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

fn rask_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_rask"))
}

/// A fresh directory, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rask_first_day_{}_{}", std::process::id(), id));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        TempDir(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, text: &str) -> PathBuf {
        let path = self.0.join(name);
        fs::write(&path, text).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Run {
    code: i32,
    stdout: String,
    stderr: String,
}

impl Run {
    fn all(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }
}

impl std::fmt::Debug for Run {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "exit {}\n--- stdout\n{}--- stderr\n{}", self.code, self.stdout, self.stderr)
    }
}

fn finish(out: Output) -> Run {
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// `rask <args>` in `dir`.
fn rask_in(dir: &Path, args: &[&str]) -> Run {
    let out = Command::new(rask_binary())
        .args(args)
        .current_dir(dir)
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run rask");
    finish(out)
}

/// `rask run` on `source`, natively or on the interpreter.
fn run_program(source: &str, interp: bool) -> Run {
    let dir = TempDir::new();
    dir.write("main.rk", source);
    let mut args = vec!["run"];
    if interp {
        args.push("--interp");
    }
    args.push("main.rk");
    rask_in(dir.path(), &args)
}

/// `rask check` on `source`.
fn check_program(source: &str) -> Run {
    let dir = TempDir::new();
    dir.write("main.rk", source);
    rask_in(dir.path(), &["check", "main.rk"])
}

/// Both backends panic, and both say `needle`.
fn panics_on_both(source: &str, needle: &str) {
    for interp in [true, false] {
        let run = run_program(source, interp);
        let backend = if interp { "interpreter" } else { "native" };
        assert_eq!(run.code, 101, "{backend} should panic:\n{run:?}");
        assert!(run.all().contains(needle), "{backend} should say `{needle}`:\n{run:?}");
    }
}

/// Both backends run to completion and print `expected`.
fn prints_on_both(source: &str, expected: &str) {
    for interp in [true, false] {
        let run = run_program(source, interp);
        let backend = if interp { "interpreter" } else { "native" };
        assert_eq!(run.code, 0, "{backend} should succeed:\n{run:?}");
        assert_eq!(run.stdout, expected, "{backend} printed the wrong thing:\n{run:?}");
    }
}

// ─── The first collection: a fixed-size array ──────────────────────────────
//
// `[1, 2, 3]` is an array, and it is the first collection anyone writes.
// Natively an out-of-range index read or wrote past it (#1724): `v[5]`
// printed 0, `a[4] = 9` overwrote the neighbouring stack slot.

const OOB: &str = "index out of bounds: index is 4 but length is 3";

#[test]
fn reading_an_array_past_its_end_panics() {
    panics_on_both(
        "func main() {\n    let a = [1, 2, 3]\n    let i = 4\n    println(\"{a[i]}\")\n}\n",
        OOB,
    );
}

#[test]
fn reading_an_array_at_a_negative_index_panics() {
    panics_on_both(
        "func main() {\n    let a = [1, 2, 3]\n    let i = -1\n    println(\"{a[i]}\")\n}\n",
        "index is -1 but length is 3",
    );
}

#[test]
fn writing_an_array_past_its_end_panics() {
    panics_on_both(
        "func main() {\n    mut a = [1, 2, 3]\n    let i = 4\n    a[i] = 9\n    println(\"{a[0]}\")\n}\n",
        "index",
    );
    // `+=` reaches the same store.
    panics_on_both(
        "func main() {\n    mut a = [1, 2, 3]\n    let i = 4\n    a[i] += 1\n    println(\"{a[0]}\")\n}\n",
        "index",
    );
}

#[test]
fn lending_an_array_element_past_its_end_panics() {
    panics_on_both(
        "func bump(mutate x: i64) {\n    x += 1\n}\n\nfunc main() {\n    mut a = [1, 2, 3]\n    let i = 4\n    bump(mutate a[i])\n    println(\"{a[0]}\")\n}\n",
        OOB,
    );
    panics_on_both(
        "func main() {\n    mut a = [1, 2, 3]\n    let i = 4\n    with a[i] as x {\n        x = 5\n    }\n    println(\"{a[0]}\")\n}\n",
        "index",
    );
}

#[test]
fn a_field_of_an_array_element_past_its_end_panics() {
    panics_on_both(
        "struct Pt {\n    x: i64\n    y: i64\n    z: i64\n}\n\nfunc main() {\n    let a = [Pt(x: 1, y: 2, z: 3), Pt(x: 4, y: 5, z: 6)]\n    let i = 4\n    println(\"{a[i].x}\")\n}\n",
        "index is 4 but length is 2",
    );
}

#[test]
fn an_array_index_in_range_still_works() {
    prints_on_both(
        "func main() {\n    mut a = [1, 2, 3]\n    for i in 0..3 {\n        a[i] += 10\n    }\n    let j = 2\n    println(\"{a[0]} {a[1]} {a[j]}\")\n}\n",
        "11 12 13\n",
    );
}

// ─── Numbers ───────────────────────────────────────────────────────────────

/// `let a = 3` is an unsuffixed literal, so multiplied by a float it is a
/// float. The checker settled it by hash-map order when an integer literal and
/// a float literal met, and native printed `7` or `7.5` from run to run.
#[test]
fn an_integer_literal_name_times_a_float_is_a_float_every_time() {
    let source = "func main() {\n    let a = 3\n    let b = 2.5\n    println(\"{a * b} {b * a}\")\n}\n";
    for _ in 0..6 {
        prints_on_both(source, "7.5 7.5\n");
    }
}

// ─── Habits from other languages ───────────────────────────────────────────

/// `Point { x: 1, y: 2 }` is how Rust, Go and C# build a struct. It gave three
/// errors, the last saying top-level statements can't sit beside `main`.
/// Now it is one error per literal that shows the call to write instead.
#[test]
fn a_struct_built_with_braces_is_one_error_that_shows_the_call() {
    let run = check_program(
        "struct Point {\n    x: i64\n    y: i64\n}\n\nfunc main() {\n    let p = Point { x: 1, y: 2 }\n    println(\"{p.x}\")\n}\n",
    );
    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.all().contains("Point(x: …, y: …)"), "{run:?}");
    assert_eq!(run.all().matches("error[").count(), 1, "{run:?}");
}

#[test]
fn a_generic_struct_built_with_braces_is_the_same_one_error() {
    let run = check_program(
        "struct Pair<T> {\n    a: T\n    b: T\n}\n\nfunc main() {\n    let q = Pair<i64> { a: 1, b: 2 }\n    println(\"{q.a}\")\n}\n",
    );
    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.all().contains("Pair<i64>(a: …, b: …)"), "{run:?}");
    assert_eq!(run.all().matches("error[").count(), 1, "{run:?}");
}

/// `import env` passed check and ran on the interpreter, then failed natively
/// with "unresolved variable `env`": a second, unspecified copy of `os.env`
/// that only one backend had. It's gone, and the import names the real one.
#[test]
fn import_env_points_at_os_env() {
    let run = check_program("import env\n\nfunc main() {\n    println(\"hi\")\n}\n");
    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.all().contains("os.env(\"HOME\")"), "{run:?}");
}

#[test]
fn a_misspelled_stdlib_import_suggests_the_module() {
    let run = check_program("import strings\n\nfunc main() {\n    println(\"hi\")\n}\n");
    assert_eq!(run.code, 1, "{run:?}");
    assert!(run.all().contains("did you mean `string`?"), "{run:?}");
}

// ─── A project, start to finish ─────────────────────────────────────────────

/// `rask init`, then every everyday command with no path, the way the init
/// message and `cargo`/`go` habits suggest. `run` and `build` already took
/// the package you're in; `check`, `test` and `lint` said "missing file or
/// directory argument", and `rask check`'s usage line named `typecheck`.
#[test]
fn a_new_project_takes_every_everyday_command_with_no_path() {
    let dir = TempDir::new();
    let init = rask_in(dir.path(), &["init", "hello"]);
    assert_eq!(init.code, 0, "{init:?}");
    assert!(init.stdout.contains("rask run\n"), "init should say plain `rask run`:\n{init:?}");

    let project = dir.path().join("hello");
    fs::write(
        project.join("main.rk"),
        "func double(x: i64) -> i64 {\n    return x * 2\n}\n\nfunc main() {\n    println(\"{double(21)}\")\n}\n\ntest \"double\" {\n    assert double(2) == 4\n}\n",
    )
    .unwrap();

    let run = rask_in(&project, &["run"]);
    assert_eq!((run.code, run.stdout.as_str()), (0, "42\n"), "{run:?}");
    for args in [&["check"][..], &["test"], &["test", "--interp"], &["lint"], &["fmt", "--check"]] {
        let out = rask_in(&project, args);
        assert_eq!(out.code, 0, "`rask {}` in a project:\n{out:?}", args.join(" "));
        assert!(!out.all().contains("missing"), "`rask {}`:\n{out:?}", args.join(" "));
    }
    let test = rask_in(&project, &["test"]);
    assert!(test.all().contains("double"), "the test should have run:\n{test:?}");
}

/// `rask help` led with `compile`, `targets` and `bindc`, left `check` out
/// entirely, and filed `test`, `init` and `lint` under later headings. The
/// first section is now the everyday loop.
#[test]
fn help_leads_with_the_everyday_commands() {
    let dir = TempDir::new();
    let help = rask_in(dir.path(), &["help"]);
    assert_eq!(help.code, 0, "{help:?}");
    let everyday = help.stdout.split("\n\n").find(|s| s.starts_with("Everyday:")).expect("an Everyday section");
    for cmd in ["init", "run", "test", "check", "fmt", "lint"] {
        assert!(everyday.contains(&format!("\n  {cmd} ")), "`{cmd}` should be under Everyday:\n{everyday}");
    }
}

/// Build status moved to stderr so `rask run` prints only the program's
/// output. `rask targets` went with it, though its list is the answer.
#[test]
fn targets_lists_on_stdout() {
    let dir = TempDir::new();
    let run = rask_in(dir.path(), &["targets"]);
    assert_eq!(run.code, 0, "{run:?}");
    assert!(run.stdout.contains("Host:"), "{run:?}");
}

/// `rask lex big.rk | head` panicked with "failed printing to stdout:
/// Broken pipe" and exit 101 once `head` had its lines. It ends quietly now.
#[test]
fn piping_into_head_ends_quietly() {
    let dir = TempDir::new();
    let mut source = String::new();
    for i in 0..3000 {
        source.push_str(&format!("func f{i}(x: i64) -> i64 {{\n    return x + {i}\n}}\n\n"));
    }
    dir.write("big.rk", &source);
    let mut child = Command::new(rask_binary())
        .args(["lex", "big.rk"])
        .current_dir(dir.path())
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run rask");
    let mut first = [0u8; 64];
    child.stdout.take().unwrap().read_exact(&mut first).unwrap();
    // The read end is closed here; the rest of the output has nowhere to go.
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_ne!(out.status.code(), Some(101), "{stderr}");
}

// ─── Names that don't exist ────────────────────────────────────────────────

/// The fix line for a name that's wrong, on `rask check`.
fn fix_for(source: &str) -> String {
    let run = check_program(source);
    assert_eq!(run.code, 1, "{run:?}");
    run.all().lines().find(|l| l.contains("fix:")).unwrap_or_else(|| panic!("no fix line:\n{run:?}")).to_string()
}

/// Spelling distance alone sent these to the wrong place: `fs.read_file`
/// suggested `read_dir`, `fs.write_file` suggested `create_file`. The names
/// other languages use map to Rask's.
#[test]
fn names_from_other_languages_point_at_the_rask_name() {
    let cases = [
        ("import fs\n\nfunc main() {\n    let t = try fs.read_file(\"a.txt\")\n    println(t)\n}\n", "read_text"),
        ("import fs\n\nfunc main() {\n    try fs.write_file(\"a.txt\", \"hi\")\n}\n", "write_text"),
        ("import cli\n\nfunc main() {\n    let a = cli.args()\n    println(\"{a.len()}\")\n}\n", "os.args"),
        ("func main() {\n    let s = \"abc\"\n    println(\"{s.size()}\")\n}\n", "`len`"),
        ("func main() {\n    mut v: Vec<i64> = Vec.new()\n    v.add(1)\n    println(\"{v.len()}\")\n}\n", "`push`"),
    ];
    for (source, wanted) in cases {
        let fix = fix_for(source);
        assert!(fix.contains(wanted), "wanted `{wanted}` in {fix:?} for:\n{source}");
    }
}

#[test]
fn a_misspelled_local_suggests_the_local() {
    let fix = fix_for("func main() {\n    let count = 3\n    println(\"{cuont}\")\n}\n");
    assert!(fix.contains("did you mean `count`?"), "{fix}");
}

/// Names in scope come out of a hash map, so with two equally close the
/// suggestion changed from run to run. Ties go to the one that sorts first.
#[test]
fn a_tied_suggestion_is_the_same_every_run() {
    let source = "func main() {\n    let cut = 1\n    let cat = 2\n    let cet = 3\n    println(\"{cot} {cut} {cat} {cet}\")\n}\n";
    for _ in 0..4 {
        assert!(fix_for(source).contains("did you mean `cat`?"));
    }
}

/// A binding nothing reads is usually a typo. The spec has warned about it
/// since the start (tool.warnings/W3); nothing did.
#[test]
fn an_unused_variable_is_a_warning() {
    let run = check_program("func main() {\n    let total = 5\n    mut count = 0\n    count = 2\n    let _quiet = 1\n    println(\"hi\")\n}\n");
    assert_eq!(run.code, 0, "{run:?}");
    assert!(run.all().contains("unused variable `total`"), "{run:?}");
    assert!(run.all().contains("unused variable `count`"), "assigning isn't reading:\n{run:?}");
    assert!(!run.all().contains("_quiet"), "{run:?}");
    let allowed = check_program("@allow(unused_variable)\nfunc main() {\n    let total = 5\n}\n");
    assert!(!allowed.all().contains("W0901"), "{allowed:?}");
}

/// In a package build a dependency's names reach the resolver renamed
/// (`libpkg.twice` arrives as `twice_libpkg`), so the import's own name never
/// shows up. The first W1 called every package import unused. A use through
/// the module, an imported type and an alias all count; an import of a
/// package used for nothing still warns.
#[test]
fn a_package_import_used_under_its_merged_name_is_used() {
    let dir = TempDir::new();
    let toml = |name: &str, deps: &str| {
        format!(
            "schema = \"rask-package@1\"\n\n[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\
             description = \"x\"\nlicense = \"MIT OR Apache-2.0\"\n{deps}"
        )
    };
    for sub in ["app", "libpkg", "other"] {
        fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    dir.write(
        "app/rask.toml",
        &toml("app", "\n[dependencies]\nlibpkg = { path = \"../libpkg\" }\nother = { path = \"../other\" }\n"),
    );
    dir.write("libpkg/rask.toml", &toml("libpkg", ""));
    dir.write("other/rask.toml", &toml("other", ""));
    dir.write(
        "libpkg/defs.rk",
        "public struct Dog {\n    public age: i64\n}\n\npublic func twice(n: i64) -> i64 {\n    return n * 2\n}\n",
    );
    dir.write("other/defs.rk", "public func unused() -> i64 {\n    return 1\n}\n");
    dir.write(
        "app/main.rk",
        "import libpkg\nimport libpkg.Dog\nimport libpkg as lp\nimport other\n\n\
         func main() {\n    let d = Dog(age: 3)\n    println(\"{libpkg.twice(d.age)} {lp.twice(1)}\")\n}\n",
    );
    let run = rask_in(&dir.path().join("app"), &["check"]);
    assert_eq!(run.code, 0, "{run:?}");
    let all = run.all();
    let warned: Vec<&str> = all.lines().filter(|l| l.contains("W0201")).collect();
    assert_eq!(warned, ["warning[W0201]: unused import `other`"], "{run:?}");
}

/// A dependency's annotation, written `@libpkg.mark`, is a use of
/// `import libpkg`; one reached through its own import (`@flag`) is a use of
/// that, and so is a name in an annotation's arguments. And the dependency's
/// own dead code is its author's to hear about, not the program's.
#[test]
fn annotations_use_imports_and_a_dependency_keeps_its_warnings() {
    let run = check_with_libpkg(
        "public enum Level {\n    Low\n    High\n}\n\n\
         public annotation @mark {\n    level: Level = Level.Low\n}\n\npublic annotation @flag {\n    on: bool = false\n}\n\n\
         public func dead() -> i64 {\n    return 1\n    println(\"lib dead\")\n}\n",
        "import libpkg\nimport libpkg.flag\nimport libpkg.Level\n\n\
         struct Signup {\n    @libpkg.mark(level: Level.High)\n    @flag(on: true)\n    name: string\n}\n\n\
         func main() {\n    let s = Signup(name: \"x\")\n    println(\"{s.name} {libpkg.dead()}\")\n}\n",
    );
    assert_eq!(run.code, 0, "{run:?}");
    assert!(!run.all().contains("W0201"), "{run:?}");
    assert!(!run.all().contains("W0902"), "{run:?}");
}

/// `import libpkg` and `import libpkg.Dog` are two imports. Both reach the
/// resolver as the same merged name, `Dog_libpkg`, so using one must not
/// count for the other.
#[test]
fn a_package_and_its_member_are_separate_imports() {
    let lib = "public struct Dog {\n    public age: i64\n}\n\npublic func twice(n: i64) -> i64 {\n    return n * 2\n}\n";
    let imports = "import libpkg\nimport libpkg.Dog\n\n";
    let run = check_with_libpkg(lib, &format!("{imports}func main() {{\n    let d = Dog(age: 3)\n    println(\"{{d.age}}\")\n}}\n"));
    assert!(run.all().contains("unused import `libpkg`"), "{run:?}");
    assert!(!run.all().contains("unused import `Dog`"), "{run:?}");
    let run = check_with_libpkg(lib, &format!("{imports}func main() {{\n    println(\"{{libpkg.twice(2)}}\")\n}}\n"));
    assert!(run.all().contains("unused import `Dog`"), "{run:?}");
    assert!(!run.all().contains("unused import `libpkg`"), "{run:?}");
}

/// `rask check` on an `app` package that depends on a path package `libpkg`.
fn check_with_libpkg(lib: &str, main: &str) -> Run {
    let dir = TempDir::new();
    let toml = |name: &str, deps: &str| {
        format!(
            "schema = \"rask-package@1\"\n\n[package]\nname = \"{name}\"\nversion = \"0.1.0\"\n\
             description = \"x\"\nlicense = \"MIT OR Apache-2.0\"\n{deps}"
        )
    };
    for sub in ["app", "libpkg"] {
        fs::create_dir_all(dir.path().join(sub)).unwrap();
    }
    dir.write("app/rask.toml", &toml("app", "\n[dependencies]\nlibpkg = { path = \"../libpkg\" }\n"));
    dir.write("libpkg/rask.toml", &toml("libpkg", ""));
    dir.write("libpkg/defs.rk", lib);
    dir.write("app/main.rk", main);
    rask_in(&dir.path().join("app"), &["check"])
}

/// An import nothing uses is left over from an edit (tool.warnings/W1).
/// Every way a name gets used counts: a call through the module, a type in
/// a signature, a type argument, a type pattern, an `implements` block, a
/// bound on a type parameter.
#[test]
fn an_unused_import_is_a_warning() {
    let run = check_program("import time.Duration\nimport os\n\nfunc main() {\n    println(\"hi\")\n}\n");
    assert_eq!(run.code, 0, "{run:?}");
    assert!(run.all().contains("unused import `Duration`"), "{run:?}");
    assert!(run.all().contains("unused import `os`"), "{run:?}");

    let used = check_program(concat!(
        "import os\n",
        "import time.Duration\n",
        "import io\n",
        "import io.IoError\n",
        "import fs\n",
        "import std.reflect\n",
        "import time.Instant\n",
        "\n",
        "struct Pad {\n    n: usize\n}\n\n",
        "Pad implements io.Writer {\n",
        "    func write(self, data: Vec<u8>) -> usize or IoError {\n        return self.n\n    }\n",
        "    func write_bytes(self, data: Vec<u8>) -> void or IoError {\n        return\n    }\n",
        "    func write_text(self, data: string) -> void or IoError {\n        return\n    }\n",
        "    func flush(self) -> void or IoError {\n        return\n    }\n",
        "}\n\n",
        "func wait(d: Duration) {}\n\n",
        "func main() {\n",
        "    println(\"{os.args().len()}\")\n",
        "    let has = reflect.has_method<Instant>(\"elapsed\")\n",
        "    println(\"{has}\")\n",
        "    let r = fs.read_text(\"nope\")\n",
        "    if r is IoError as e {\n        println(e.message())\n    }\n",
        "}\n",
    ));
    assert!(!used.all().contains("W0201"), "{used:?}");
}

/// Code after a `return` is usually a debugging leftover (tool.warnings/W4).
#[test]
fn unreachable_code_is_a_warning() {
    let run = check_program(concat!(
        "func pick(x: i64) -> i64 {\n",
        "    if x > 0 {\n        return 1\n    } else {\n        return 2\n    }\n",
        "    println(\"after if\")\n",
        "}\n\n",
        "func main() {\n",
        "    for i in 0..3 {\n        continue\n        println(\"after continue\")\n    }\n",
        "    println(\"{pick(1)}\")\n",
        "    return\n",
        "    println(\"after return\")\n",
        "}\n",
    ));
    assert_eq!(run.code, 0, "{run:?}");
    assert_eq!(run.all().matches("W0902").count(), 3, "{run:?}");
    // In source order, nested lists included.
    let at = |s: &str| run.all().find(s).unwrap();
    assert!(at("after if") < at("after continue") && at("after continue") < at("after return"), "{run:?}");

    // A loop with a `break`, a panic inside an `if`, and a function that says
    // it's meant: nothing to say.
    let quiet = check_program(concat!(
        "@allow(unreachable_code)\nfunc meant() -> i64 {\n    return 1\n    println(\"x\")\n}\n\n",
        "func main() {\n",
        "    mut n = 0\n",
        "    loop {\n        n += 1\n        if n > 3 {\n            break\n        }\n    }\n",
        "    if n > 10 {\n        panic(\"big\")\n    }\n",
        "    println(\"{n} {meant()}\")\n",
        "}\n",
    ));
    assert!(!quiet.all().contains("W0902"), "{quiet:?}");
}

/// One mistake, one error. After `??` was rejected on a result, `n` had no
/// type, and `{n + 1}` (which is `n.add(1)`) was also reported as mutating
/// the `let` binding `n`, because `Set.add` mutates.
#[test]
fn a_rejected_fallback_does_not_also_report_mutation() {
    let run = check_program("func main() {\n    let n = \"42\".parse<i64>() ?? 0\n    println(\"{n + 1}\")\n}\n");
    assert_eq!(run.code, 1, "{run:?}");
    assert!(!run.all().contains("cannot mutate"), "{run:?}");
    assert_eq!(run.all().matches("error[").count(), 1, "{run:?}");
}

// ─── Files ─────────────────────────────────────────────────────────────────
//
// The first file anyone reads is one that isn't there yet. The error used to
// be "not found: No such file or directory (os error 2)", which doesn't say
// which file.

#[test]
fn a_missing_file_error_names_the_file() {
    let source = "import fs\n\nfunc main() -> void or fs.IoError {\n    let text = try fs.read_text(\"notes.txt\")\n    println(text)\n}\n";
    for interp in [true, false] {
        let run = run_program(source, interp);
        let backend = if interp { "interpreter" } else { "native" };
        assert_eq!(run.code, 1, "{backend}:\n{run:?}");
        assert!(run.stderr.contains("not found: notes.txt"), "{backend} should name the file:\n{run:?}");
    }
}

/// `os.args()[0]` on a native `rask run` was the temp file it compiled to,
/// `/tmp/rask_main_12345`; the interpreter gave `main.rk`.
#[test]
fn the_program_name_is_the_script_on_both_backends() {
    prints_on_both("import os\n\nfunc main() {\n    println(os.args()[0])\n}\n", "main.rk\n");
}
