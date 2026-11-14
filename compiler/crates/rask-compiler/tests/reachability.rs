// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! What monomorphization reaches from `main`, on checked programs.
//!
//! Reachability follows the declaration the checker resolved each call to,
//! so these go through the whole front end rather than hand-built decls: a
//! call with no checker record behind it reaches nothing.

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rask_compiler::{compile_file, CfgConfig, CompilerConfig};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn reached(src: &str) -> Vec<String> {
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path: PathBuf = std::env::temp_dir().join(format!("rask_reach_{}_{}.rk", std::process::id(), n));
    std::fs::File::create(&path).and_then(|mut f| f.write_all(src.as_bytes())).expect("write tmp file");
    let output = compile_file(path.to_str().unwrap(), &CompilerConfig::new(CfgConfig::from_host("debug", vec![])));
    let _ = std::fs::remove_file(&path);
    let result = output.result.unwrap_or_else(|| panic!("expected the program to compile: {:?}", output.diagnostics));
    result.mono.functions.iter().map(|f| f.name.clone()).collect()
}

fn has(names: &[String], name: &str) -> bool {
    names.iter().any(|n| n == name)
}

#[test]
fn main_reaches_what_it_calls_and_nothing_else() {
    let names = reached(
        "func helper() -> i64 { return 1 }
         func unused() -> i64 { return 2 }
         func main() { println(\"{helper()}\") }",
    );
    assert!(has(&names, "helper"), "{names:?}");
    assert!(!has(&names, "unused"), "{names:?}");
}

#[test]
fn calls_are_followed_transitively_through_expressions_and_branches() {
    let names = reached(
        "func leaf() -> i64 { return 3 }
         func mid(n: i64) -> i64 {
             if n > 0 { return leaf() + 1 }
             return 0
         }
         func top() -> i64 { return mid(2) * 2 }
         func main() { println(\"{top()}\") }",
    );
    for f in ["top", "mid", "leaf"] {
        assert!(has(&names, f), "{f} not reached: {names:?}");
    }
}

#[test]
fn mutual_recursion_terminates() {
    let names = reached(
        "func is_even(n: i64) -> bool {
             if n == 0 { return true }
             return is_odd(n - 1)
         }
         func is_odd(n: i64) -> bool {
             if n == 0 { return false }
             return is_even(n - 1)
         }
         func main() { println(\"{is_even(4)}\") }",
    );
    assert!(has(&names, "is_even") && has(&names, "is_odd"), "{names:?}");
}

#[test]
fn a_method_body_is_walked_like_a_function() {
    let names = reached(
        "struct Point { x: i64 }
         func scale(n: i64) -> i64 { return n * 2 }
         extend Point {
             func doubled(self) -> i64 { return scale(self.x) }
         }
         func main() {
             let p = Point(x: 4)
             println(\"{p.doubled()}\")
         }",
    );
    assert!(has(&names, "Point_doubled") && has(&names, "scale"), "{names:?}");
}

#[test]
fn a_function_value_is_reached() {
    let names = reached(
        "func twice(n: i64) -> i64 { return n * 2 }
         func apply(f: func(i64) -> i64, n: i64) -> i64 { return f(n) }
         func main() { println(\"{apply(twice, 3)}\") }",
    );
    assert!(has(&names, "twice"), "{names:?}");
}
