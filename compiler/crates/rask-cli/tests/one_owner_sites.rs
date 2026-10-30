// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Each fact has one owner, and the places that restate one only go down.
//!
//! v0.7's gate (ROADMAP.md). A fact an earlier pass already knew — which
//! function a call names, what a stdlib method takes, how wide a value is —
//! is worked out again downstream from a name, a spelling or a hand list, and
//! the copies drift (rask-lang/rask#1570). Each ratchet below counts one kind
//! of restatement and pins today's number. A fix lowers the number; a new
//! copy fails the build.
//!
//! When a ratchet reaches its target, delete the measurement and keep the
//! structural guard that made the sites impossible (a Cargo dependency, a
//! type that can't hold the old answer).

use std::fs;
use std::path::{Path, PathBuf};

/// Where in the tree. Paths are relative to `compiler/crates`.
struct Ratchet {
    name: &'static str,
    /// Today's count. Only ever lower it.
    today: usize,
    /// Where it has to get to for v0.7 to ship.
    target: usize,
    /// What a new site means, for the failure message.
    why: &'static str,
    count: fn(&Path) -> Vec<String>,
}

const RATCHETS: &[Ratchet] = &[
    Ratchet {
        name: "name sniffs in the back half",
        today: 29,
        target: 0,
        why: "MIR or codegen asked a callee's or type's spelling what it is. The \
              checker resolved that call; carry its identity on the call \
              (rask-lang/rask#1564, #1566)",
        count: name_sniffs,
    },
    Ratchet {
        name: "method names the checker answers by hand",
        today: 26,
        target: 0,
        why: "a method answered by matching its name as a string literal in the \
              checker instead of reading its declaration in stdlib/*.rk, or the \
              interface it belongs to (rask-lang/rask#1561, #1565)",
        count: checker_method_arms,
    },
    Ratchet {
        name: "`_Any` wildcards in the checker",
        today: 4,
        target: 0,
        why: "a module function typed through a wildcard that skips argument \
              checks (rask-lang/rask#1561)",
        count: any_wildcards,
    },
    Ratchet {
        name: "MIR metadata rows keyed by spelling",
        today: 89,
        target: 0,
        why: "a row in INTERNAL_SPELLINGS, TRANSFERS_OUT or BORROWS_ITS_CALLBACK: a \
              fact about a call stated by its name instead of on its \
              declaration (rask-lang/rask#1564, #1565)",
        count: mir_metadata_rows,
    },
    Ratchet {
        name: "hand-written codegen dispatch entries",
        today: 238,
        target: 0,
        why: "an `internal_entries` row in rask-codegen/src/dispatch.rs: a runtime \
              call whose shape lowering didn't state (rask-lang/rask#1555, #1565)",
        count: codegen_internal_entries,
    },
    Ratchet {
        name: "pass entry point variants",
        today: 14,
        target: 7,
        why: "a second public entry to a pass (`_with_stdlib`, `_package`, …). \
              One entry per pass, taking an input struct, called only by the \
              driver (rask-lang/rask#1568)",
        count: pass_entry_points,
    },
    Ratchet {
        name: "interpreters run without the driver's setup",
        today: 4,
        target: 0,
        why: "a tool built its own interpreter instead of asking the driver for \
              one; the copies skip steps the driver does (rask-lang/rask#1568)",
        count: interpreter_setups,
    },
    Ratchet {
        name: "size or alignment computed outside the layout model",
        today: 5,
        target: 0,
        why: "a second answer to how wide a value is; one layout crate answers \
              everyone (rask-lang/rask#1567)",
        count: size_models,
    },
    Ratchet {
        name: "readers of the pointer-width global",
        today: 15,
        target: 0,
        why: "the target's pointer width read from a process global instead of \
              the config the driver hands down (rask-lang/rask#1568)",
        count: pointer_bits_readers,
    },
];

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    if dir.is_file() {
        out.push(dir.to_path_buf());
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Non-test source lines under `dirs`, as (relative path, line number, code).
fn source_lines(crates: &Path, dirs: &[&str], skip: &[&str]) -> Vec<(String, usize, String)> {
    let mut files = Vec::new();
    for dir in dirs {
        rust_sources(&crates.join(dir), &mut files);
    }
    let mut out = Vec::new();
    for path in files {
        let rel = path.strip_prefix(crates).unwrap().to_string_lossy().replace('\\', "/");
        if rel.ends_with("/tests.rs") || rel.contains("/tests/") || skip.iter().any(|s| rel.ends_with(s)) {
            continue;
        }
        let src = fs::read_to_string(&path).unwrap();
        let body = src.split("#[cfg(test)]\nmod tests").next().unwrap_or(&src);
        for (i, line) in body.lines().enumerate() {
            let code = line.split("//").next().unwrap_or(line).to_string();
            out.push((rel.clone(), i + 1, code));
        }
    }
    out
}

fn site(rel: &str, line: usize, code: &str) -> String {
    format!("{rel}:{line}: {}", code.trim())
}

fn name_sniffs(crates: &Path) -> Vec<String> {
    // Text read on purpose: a target triple, a C header, a path.
    let skip = ["rask-codegen/src/targets.rs", "rask-codegen/src/dispatch.rs", "rask-mono/src/runtime_abi.rs"];
    source_lines(crates, &["rask-mir/src", "rask-codegen/src", "rask-mono/src", "rask-stdlib/src/mir_metadata.rs"], &skip)
        .into_iter()
        .filter(|(_, _, code)| code.contains("ends_with(\"") || code.contains("starts_with(\""))
        .map(|(rel, line, code)| site(&rel, line, &code))
        .collect()
}

/// Match arms keyed on a method name in the checker: `"len" if args.is_empty() =>`.
/// Each name in one counts once. Primitive type names (`"i64" =>`) are types,
/// not methods, and don't count, and neither do the functions in
/// `NOT_METHOD_TABLES`, whose string arms aren't answering a method call.
fn checker_method_arms(crates: &Path) -> Vec<String> {
    const TYPE_NAMES: &[&str] = &[
        "i8", "i16", "i32", "i64", "i128", "u8", "u16", "u32", "u64", "u128", "f32", "f64",
        "bool", "char", "string", "usize", "isize", "void", "bytes", "self",
    ];
    const NOT_METHOD_TABLES: &[(&str, &str)] = &[
        ("removed_string_method", "names methods `string` doesn't have, to say why"),
        ("closest_type_name", "other languages' type names, to suggest Rask's"),
        ("simd_elem_type", "SIMD type names, not methods"),
        ("check_call", "builtin free functions (`panic`, `todo`), not methods"),
    ];
    let mut out = Vec::new();
    let mut current_fn = String::new();
    for (rel, line, code) in source_lines(crates, &["rask-types/src/checker"], &[]) {
        let head = code.trim_start();
        if let Some(rest) = ["pub(super) fn ", "pub(crate) fn ", "pub fn ", "fn "].iter().find_map(|p| head.strip_prefix(p)) {
            current_fn = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        }
        if NOT_METHOD_TABLES.iter().any(|(f, _)| *f == current_fn) {
            continue;
        }
        let Some(arm) = head.split("=>").next().filter(|_| head.contains("=>")) else { continue };
        let names: Vec<&str> = arm.split('|').map(str::trim).map(|n| n.split(" if ").next().unwrap_or(n).trim()).collect();
        let all_names = names.iter().all(|n| {
            n.len() > 2
                && n.starts_with('"')
                && n.ends_with('"')
                && n[1..n.len() - 1].chars().next().is_some_and(|c| c.is_ascii_lowercase())
                && n[1..n.len() - 1].chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        });
        if !all_names {
            continue;
        }
        for n in names {
            if !TYPE_NAMES.contains(&&n[1..n.len() - 1]) {
                out.push(format!("{rel}:{line}: {current_fn}: {n}"));
            }
        }
    }
    out
}

fn any_wildcards(crates: &Path) -> Vec<String> {
    source_lines(crates, &["rask-types/src"], &[])
        .into_iter()
        .filter(|(_, _, code)| code.contains("\"_Any\""))
        .map(|(rel, line, code)| site(&rel, line, &code))
        .collect()
}

fn mir_metadata_rows(crates: &Path) -> Vec<String> {
    let tables = ["INTERNAL_SPELLINGS", "TRANSFERS_OUT", "BORROWS_ITS_CALLBACK"];
    let mut rows = Vec::new();
    let mut inside: Option<&str> = None;
    for (rel, line, code) in source_lines(crates, &["rask-stdlib/src/mir_metadata.rs"], &[]) {
        if let Some(table) = inside {
            if code.starts_with("];") {
                inside = None;
            } else if code.starts_with("    (") || code.starts_with("    \"") {
                rows.push(format!("{rel}:{line}: {table} {}", code.trim()));
            }
        } else if let Some(table) = tables.iter().find(|t| code.starts_with(&format!("const {t}:"))) {
            inside = Some(table);
        }
    }
    rows
}

fn codegen_internal_entries(crates: &Path) -> Vec<String> {
    let mut rows = Vec::new();
    let mut inside = false;
    for (rel, line, code) in source_lines(crates, &["rask-codegen/src/dispatch.rs"], &[]) {
        if inside {
            if code.starts_with('}') {
                break;
            }
            if code.contains("StdlibEntry {") || code.contains("StdlibEntry::simple(") {
                rows.push(site(&rel, line, &code));
            }
        } else if code.contains("fn internal_entries(") {
            inside = true;
        }
    }
    rows
}

fn pass_entry_points(crates: &Path) -> Vec<String> {
    let libs = [
        "rask-parser/src/lib.rs",
        "rask-desugar/src/lib.rs",
        "rask-resolve/src/lib.rs",
        "rask-types/src/lib.rs",
        "rask-ownership/src/lib.rs",
        "rask-effects/src/lib.rs",
        "rask-comptime/src/lib.rs",
    ];
    let prefixes = ["parse", "desugar", "resolve", "check", "infer_effects", "fold_comptime"];
    source_lines(crates, &libs, &[])
        .into_iter()
        .filter(|(_, _, code)| {
            code.strip_prefix("pub fn ").is_some_and(|n| {
                // `parse_type` parses one type expression for tooling, not a file.
                prefixes.iter().any(|p| n.starts_with(p)) && !n.starts_with("parse_type")
            })
        })
        .map(|(rel, line, code)| site(&rel, line, &code))
        .collect()
}

/// Interpreters a tool builds without `rask_compiler::prepare_interpreter`
/// within the next few lines: run on declarations nothing checked, or set up
/// by hand, which is how the copies drifted (#1177, #1568).
fn interpreter_setups(crates: &Path) -> Vec<String> {
    let mut dirs = Vec::new();
    for entry in fs::read_dir(crates).unwrap().flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // The two evaluators own their own constructors; the driver is where
        // the setup lives.
        if name == "rask-interp" || name == "rask-comptime" || name == "rask-compiler" {
            continue;
        }
        dirs.push(format!("{name}/src"));
    }
    let dirs: Vec<&str> = dirs.iter().map(String::as_str).collect();
    let lines = source_lines(crates, &dirs, &[]);
    let mut out = Vec::new();
    for (i, (rel, line, code)) in lines.iter().enumerate() {
        let builds = ["Interpreter::new(", "Interpreter::with_args(", "Interpreter::with_captured_output("]
            .iter()
            .any(|p| code.contains(p));
        let prepared = lines[i..lines.len().min(i + 5)]
            .iter()
            .any(|(r, _, c)| r == rel && c.contains("prepare_interpreter("));
        if builds && !prepared {
            out.push(site(rel, *line, code));
        }
    }
    out
}

fn size_models(crates: &Path) -> Vec<String> {
    let dirs = ["rask-mir/src/types.rs", "rask-codegen/src/builder.rs", "rask-mono/src", "rask-types/src/copy_rule.rs"];
    let names = ["fn size(", "fn align(", "fn slot_size(", "fn named_layout_size(", "fn struct_elem_size(", "fn value_size("];
    source_lines(crates, &dirs, &[])
        .into_iter()
        .filter(|(_, _, code)| names.iter().any(|n| code.contains(n)))
        .map(|(rel, line, code)| site(&rel, line, &code))
        .collect()
}

fn pointer_bits_readers(crates: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in fs::read_dir(crates).unwrap().flatten() {
        files.push(format!("{}/src", entry.file_name().to_string_lossy()));
    }
    let dirs: Vec<&str> = files.iter().map(String::as_str).collect();
    source_lines(crates, &dirs, &["rask-ast/src/primitives.rs"])
        .into_iter()
        .filter(|(_, _, code)| code.contains("pointer_bits()"))
        .map(|(rel, line, code)| site(&rel, line, &code))
        .collect()
}

#[test]
fn restated_facts_only_go_down() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut report = Vec::new();
    let mut failed = false;
    for r in RATCHETS {
        let sites = (r.count)(&crates);
        let n = sites.len();
        report.push(format!("{:>4} / target {:<3}  {}", n, r.target, r.name));
        if n > r.today {
            failed = true;
            report.push(format!(
                "       ^ up from {}. A new one is {}.\n{}",
                r.today,
                r.why,
                sites.iter().map(|s| format!("         {s}")).collect::<Vec<_>>().join("\n")
            ));
        } else if n < r.today {
            failed = true;
            report.push(format!(
                "       ^ down from {}: lower `today` for this ratchet so the number can't creep back up.",
                r.today
            ));
        }
    }
    let total: usize = RATCHETS.iter().map(|r| r.today).sum();
    let left: usize = RATCHETS.iter().map(|r| r.today.saturating_sub(r.target)).sum();
    report.push(format!("{total:>4} sites restate a fact; {left} to go"));
    assert!(!failed, "\n{}\n", report.join("\n"));
    eprintln!("{}", report.join("\n"));
}
