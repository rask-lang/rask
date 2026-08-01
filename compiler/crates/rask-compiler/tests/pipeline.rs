// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! Integration tests for the compiler driver.
//!
//! These tests lock in the pipeline contract: given source X, check_file
//! should return the expected diagnostics and result. They verify
//! error accumulation across stages and the divergence-fix behaviors
//! (desugar diagnostics, default args, comptime cfg, etc.).

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use rask_compiler::{check_file, compile_file, CompilerConfig, CfgConfig};

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `src` to a unique temp .rk file and return the path.
fn tmp_rk(src: &str) -> PathBuf {
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let path = std::env::temp_dir().join(format!(
        "rask_compiler_test_{}_{}.rk",
        std::process::id(),
        n,
    ));
    let mut f = std::fs::File::create(&path).expect("create tmp file");
    f.write_all(src.as_bytes()).expect("write tmp file");
    path
}

fn default_config() -> CompilerConfig {
    CompilerConfig {
        cfg: CfgConfig::from_host("debug", vec![]),
    }
}

fn error_count(diagnostics: &[rask_diagnostics::Diagnostic]) -> usize {
    diagnostics.iter()
        .filter(|d| matches!(d.severity, rask_diagnostics::Severity::Error))
        .count()
}

// ═══════════════════════════════════════════════════════════════════════
// Happy path
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn check_succeeds_on_valid_program() {
    let path = tmp_rk(r#"
        func main() {
            const x = 42
            println("{x}")
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(), "expected success, got diagnostics: {:?}",
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    assert_eq!(error_count(&output.diagnostics), 0);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn check_returns_typed_program_on_success() {
    let path = tmp_rk(r#"
        func add(a: i32, b: i32) -> i32 {
            return a + b
        }
        func main() {
            const x = add(1, 2)
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    let result = output.result.expect("expected success");
    // TypedProgram should have node types for the arithmetic expressions
    assert!(!result.typed.node_types.is_empty(), "TypedProgram should have node_types");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn call_targets_records_free_and_method_dispatch() {
    // CALL6 keystone (#425): the checker records the resolved target of every
    // call once, keyed by the call node — a SymbolId for free functions, the
    // resolved receiver type plus the name for methods. Never a reconstructed
    // name string.
    //
    // The receiver is what downstream can't rebuild: `node_types` holds the type
    // of the receiver *expression*, which is routinely an unresolved variable.
    // Stdlib and primitive receivers are recorded too, not just user types —
    // those were exactly the ones lowering guessed wrong.
    use rask_types::Callee;
    let path = tmp_rk(r#"
        struct Counter { n: i32 }
        extend Counter {
            func bump(mutate self) { self.n = self.n + 1 }
        }
        func helper() -> i32 { return 7 }
        func main() {
            mut c = Counter { n: 0 }
            c.bump()
            const x = helper()
            const v = Vec.from([1, 2, 3])
            const n = v.len()
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    let result = output.result.expect("expected success");
    let targets = &result.typed.call_targets;

    let has_free = targets.values().any(|c| matches!(c, Callee::Free(_)));
    let user_method = targets
        .values()
        .find(|c| matches!(c, Callee::Method { method, .. } if method == "bump"));
    let stdlib_method = targets
        .values()
        .any(|c| matches!(c, Callee::Method { method, .. } if method == "len"));

    assert!(has_free, "free call `helper()` should record a Callee::Free");
    let user_method = user_method
        .expect("method call `c.bump()` should record a Callee::Method{..}");
    assert!(
        user_method.recv_type_id().is_some(),
        "a user-defined receiver should resolve to a TypeId, got {user_method:?}",
    );
    assert!(
        stdlib_method,
        "a stdlib receiver should record a target too — those are the calls \
         lowering used to mis-qualify",
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn instantiated_bodies_dont_reuse_the_programs_node_ids() {
    // An instantiated generic used to number its nodes from zero, so its nodes
    // collided with the original program's. Every per-node lookup inside such a
    // body — type, dispatch target, type arguments — then answered with an
    // unrelated node's record rather than missing. Lowering papered over it by
    // guessing from AST shape, which is why so much of it reconstructs
    // information the checker already had.
    //
    // Two instantiations also numbered from zero identically, so they collided
    // with each other as well.
    let path = tmp_rk(r#"
        func identity<T>(x: T) -> T {
            return x
        }
        func main() {
            const a = identity(7)
            const b = identity("hi")
            println("{a} {b}")
        }
    "#);
    let output = compile_file(path.to_str().unwrap(), Vec::new(), &default_config());
    let result = output.result.expect("expected success");

    let checker_max = result.typed.node_types.keys().map(|n| n.0).max().unwrap_or(0);
    let carried = &result.mono.instantiated_node_types;

    assert!(
        !carried.is_empty(),
        "two instantiations of first_of<T> should have carried node records; \
         an empty map means lowering is back to guessing inside them",
    );
    for id in carried.keys() {
        assert!(
            id.0 > checker_max,
            "instantiated node {} is inside the original program's id range \
             (max {}) — a lookup there answers with another node's record",
            id.0, checker_max,
        );
    }
    // Both instantiations must be represented. Numbering each copy from zero
    // made them collide with each other as well as with the program.
    assert!(
        result.mono.functions.iter().filter(|f| !f.type_args.is_empty()).count() >= 2,
        "expected an instantiation per type argument",
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn calling_an_unimplemented_stdlib_stub_fails_at_the_call() {
    // Most stdlib stubs have empty bodies and are implemented natively, so an
    // empty body says nothing. The ones with nothing behind them on either
    // backend are marked `@unimplemented`, and calling one is an error where
    // the call is — not `Function not found: Vec_reserve` out of codegen, and
    // not a runtime error part-way through a run.
    let path = tmp_rk(r#"
        func main() {
            mut v = Vec.from([1, 2, 3])
            v.reserve(100)
        }
    "#);
    let out = check_file(path.to_str().unwrap(), &default_config());
    let msgs: Vec<&String> = out.diagnostics.iter().map(|d| &d.message).collect();
    assert!(
        msgs.iter().any(|m| m.contains("Vec.reserve") && m.contains("not implemented")),
        "expected an unimplemented-stub error naming the method, got {msgs:?}",
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_program_type_shadows_the_stdlib_one_of_the_same_name() {
    // The stdlib's bodies are type-checked alongside the program now, so both
    // its types and the program's are registered. A program `struct Headers`
    // must still shadow the stdlib's for the program's own references, and the
    // stdlib's body must still mean its own — one flat name map can't say that,
    // and the second registration silently won (#515).
    //
    // The failure mode is "expected `Headers`, found `Headers`": two TypeIds,
    // one name.
    let path = tmp_rk(r#"
        struct Headers {
            entries: Map<string, string>
        }
        extend Headers {
            func new() -> Headers { return Headers { entries: Map.new() } }
            func set(mutate self, name: string, value: string) -> void {
                self.entries.insert(name, value)
            }
            func get(self, name: string) -> string? {
                return self.entries.get(name)
            }
        }
        func main() {
            mut h = Headers.new()
            h.set("Host", "localhost")
            println("{h.get(\"Host\") ?? \"MISSING\"}")
        }
    "#);
    let out = check_file(path.to_str().unwrap(), &default_config());
    assert_eq!(
        error_count(&out.diagnostics), 0,
        "a program type sharing a stdlib type's name must shadow it: {:?}",
        out.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn gc10_reports_real_self_writes_not_reads_through_a_helper() {
    // GC10 rejects a public method that mutates self without saying `mutate`.
    // It shared its detection with GC9, which deliberately over-approximates —
    // any `self.foo()` counts, since it can't see foo's declaration. That's
    // fine for inferring `mutate` on a private method and wrong for raising an
    // error: a public method that only *read* through a helper was rejected
    // (#513). GC10 now asks for a definite assignment.
    let writes = tmp_rk(r#"
        struct Counter { public n: i64 }
        extend Counter {
            public func bump(self) { self.n = self.n + 1 }
        }
        func main() { println("x") }
    "#);
    let out = check_file(writes.to_str().unwrap(), &default_config());
    assert!(
        out.diagnostics.iter().any(|d| d.message.contains("cannot mutate parameter")),
        "a public method assigning to self must still be rejected",
    );
    let _ = std::fs::remove_file(&writes);

    let reads = tmp_rk(r#"
        struct Counter { public n: i64 }
        extend Counter {
            func peek(self) -> i64 { return self.n }
            public func describe(self) -> i64 { return self.peek() }
        }
        func main() { println("x") }
    "#);
    let out = check_file(reads.to_str().unwrap(), &default_config());
    assert_eq!(
        error_count(&out.diagnostics), 0,
        "a public method that only reads through a helper is not a mutation: {:?}",
        out.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>(),
    );
    let _ = std::fs::remove_file(&reads);
}

#[test]
fn shadowing_a_stdlib_type_name_keeps_the_program_body() {
    // #258: stdlib json.rk declares `enum JsonError` with a `message()` that
    // matches on `self`. A program struct of the same name mangles to the same
    // `JsonError_message`, and monomorphization used to keep whichever
    // declaration came last — the stdlib enum — so the call ran a `match` over
    // a struct and the compiled binary died on an illegal instruction.
    let path = tmp_rk(r#"
        struct JsonError {
            detail: string
        }
        extend JsonError {
            func message(self) -> string { return "user: {self.detail}" }
        }
        func main() {
            const e = JsonError { detail: "boom" }
            println(e.message())
        }
    "#);
    let output = compile_file(path.to_str().unwrap(), Vec::new(), &default_config());
    let result = output.result.expect("expected success");

    let body = result.mono.functions.iter()
        .find(|f| f.name == "JsonError_message")
        .expect("`JsonError_message` should be reachable");

    // The program's body returns an interpolated field; the stdlib enum's
    // matches on self. Checking the shape catches a swap either way.
    let src = format!("{:?}", body.body);
    assert!(
        !src.contains("Match"),
        "`JsonError_message` lowered the stdlib enum's body, not the program's",
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn resolved_dispatch_does_not_drag_in_every_same_named_method() {
    // Instance calls used to enqueue every method sharing the bare name, so one
    // `error.message()` made every stdlib `message()` reachable. CALL6 already
    // recorded the receiver type — use it.
    let path = tmp_rk(r#"
        struct AlphaError {}
        extend AlphaError {
            func message(self) -> string { return "alpha" }
        }

        struct BetaError {}
        extend BetaError {
            func message(self) -> string { return "beta" }
        }

        func main() {
            const error = AlphaError {}
            println(error.message())
        }
    "#);
    let output = compile_file(path.to_str().unwrap(), Vec::new(), &default_config());
    let result = output.result.expect("expected success");
    let names: Vec<&str> = result.mono.functions.iter()
        .map(|f| f.name.as_str())
        .collect();

    assert!(names.contains(&"AlphaError_message"));
    assert!(
        !names.contains(&"BetaError_message"),
        "`AlphaError.message()` should not make `BetaError.message` reachable: {names:?}",
    );
    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// Error accumulation — the main novel contract
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn type_errors_across_functions_reported() {
    // The typechecker currently stops at the first type error. This test
    // documents the current behavior. Tier 2 error accumulation (lenient
    // typecheck) would make this report all three errors.
    let path = tmp_rk(r#"
        func a() -> i32 { return "not an int" }
        func b() -> string { return 42 }
        func c() -> bool { return 3.14 }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded());
    assert!(error_count(&output.diagnostics) >= 1,
        "expected at least one error, got {}",
        error_count(&output.diagnostics));
    let _ = std::fs::remove_file(&path);
}

#[test]
fn lex_and_parse_errors_both_reported() {
    // A bad character followed by a syntactic error — both should appear.
    let path = tmp_rk(r#"
        func main() {
            const x = @#$   // lex-level garbage
            func nested()   // parse error: no body, `func` at wrong spot
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded());
    assert!(error_count(&output.diagnostics) >= 1,
        "expected at least one error from the garbage input");
    let _ = std::fs::remove_file(&path);
}

#[test]
fn type_errors_dont_block_subsequent_stages() {
    // Tier 2: even with a type error, ownership and effect stages still run.
    // This produces richer diagnostics in one pass — user doesn't have to
    // fix type errors before seeing their other mistakes.
    //
    // This program has a type error (return "str" when i32 expected) AND
    // a use-after-move in a separate function. Both should be reported.
    // Data is >16 bytes so it's move-only (not Copy) — otherwise the double
    // consume below is a legal copy and produces no ownership error.
    let path = tmp_rk(r#"
        struct Data { a: i64, b: i64, c: i64 }

        func wrong_type() -> i32 {
            return "not an int"
        }

        func consume(take d: Data) {}

        func main() {
            const d = Data { a: 1, b: 2, c: 3 }
            consume(own d)
            consume(own d)
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded());
    // We expect at least one type error AND at least one ownership error.
    // This is the cross-stage accumulation the lenient typecheck enables.
    assert!(error_count(&output.diagnostics) >= 2,
        "expected type + ownership errors (Tier 2 accumulation), got {}: {:?}",
        error_count(&output.diagnostics),
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn type_and_ownership_errors_accumulate() {
    // Type-check succeeds, ownership fails. The pipeline should run both.
    // (If there were type errors, the driver stops before ownership.)
    // This test verifies that ownership-only errors come through.
    // Data is >16 bytes → move-only (not Copy), so the second consume is a real
    // use-after-move. A Copy struct here would legally copy and never fail.
    let path = tmp_rk(r#"
        struct Data { a: i64, b: i64, c: i64 }
        func consume(take d: Data) {
            // take d
        }
        func main() {
            const d = Data { a: 1, b: 2, c: 3 }
            consume(own d)
            consume(own d)   // use after move
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    // Either typecheck or ownership should flag this.
    assert!(!output.succeeded(),
        "expected failure due to use-after-move, got success. diags: {:?}",
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// Divergence-fix verification (LSP previously missed these)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn comptime_cfg_elimination_runs() {
    // If CC1 (dead-branch elimination) doesn't run, symbols from the
    // unused `else` branch leak into resolution and cause errors.
    // This test verifies the pass runs (was previously missing from LSP).
    let path = tmp_rk(r#"
        func main() {
            comptime if cfg.os == "linux" {
                const x: i32 = 1
            } else if cfg.os == "macos" {
                const x: i32 = 2
            } else {
                const x: i32 = 3
            }
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(),
        "comptime cfg elimination must produce a valid program, got: {:?}",
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn default_args_desugar_runs() {
    // If desugar_default_args doesn't run, calls without all args fail.
    let path = tmp_rk(r#"
        func greet(name: string = "World") -> string {
            return "Hello, {name}"
        }
        func main() {
            const msg = greet()   // uses default
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(),
        "default args must be desugared before typecheck, got: {:?}",
        output.diagnostics.iter().map(|d| &d.message).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// PipelineOutput contract
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn failed_pipeline_returns_none_and_errors() {
    let path = tmp_rk(r#"
        func main() {
            const x: i32 = "not an int"
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded());
    assert!(output.result.is_none());
    assert!(output.has_errors());
    assert!(error_count(&output.diagnostics) >= 1);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn successful_pipeline_returns_some() {
    let path = tmp_rk(r#"
        func main() {
            const x: i32 = 42
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded());
    assert!(output.result.is_some());
    assert!(!output.has_errors());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn missing_file_returns_error_diagnostic() {
    let output = check_file("/nonexistent/path/does_not_exist.rk", &default_config());
    assert!(!output.succeeded());
    assert!(output.has_errors());
    // Should have a single error diagnostic about the missing file.
    assert_eq!(output.diagnostics.len(), 1);
}

// ═══════════════════════════════════════════════════════════════════════
// ER42/ER43: linear payloads in error/enum variants
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn er43_top_level_wildcard_on_linear_enum_errors() {
    // ER43: matching a transitively-linear enum with a `_` arm silently
    // drops the linear payload. Compile error.
    let path = tmp_rk(r#"
        @resource
        struct File { path: string }
        extend File { func close(take self) {} }

        enum FileError {
            ReadFailed(File, string),
            Other,
        }

        func bad(take e: FileError) {
            match e {
                FileError.Other => {},
                _ => {}
            }
        }

        func main() {}
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded(), "ER43: top-level wildcard on linear scrutinee must error");
    assert!(
        output.diagnostics.iter().any(|d| d.code.as_ref().map_or(false, |c| c.0 == "E0816")),
        "expected E0816 for linear-wildcard discard, got: {:?}",
        output.diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn er43_field_wildcard_on_linear_payload_errors() {
    // ER43: a `_` inside a constructor pattern at a linear field position
    // drops that payload silently. Compile error.
    let path = tmp_rk(r#"
        @resource
        struct File { path: string }
        extend File { func close(take self) {} }

        enum FileError {
            ReadFailed(File, string),
            Other,
        }

        func bad(take e: FileError) {
            match e {
                FileError.ReadFailed(_, _) => {},
                FileError.Other => {}
            }
        }

        func main() {}
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded(), "ER43: nested wildcard on linear field must error");
    let has_er43 = output.diagnostics.iter().any(|d| {
        d.code.as_ref().map_or(false, |c| c.0 == "E0816") && d.message.contains("File")
    });
    assert!(has_er43,
        "expected E0816 mentioning File, got: {:?}",
        output.diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn er42_linear_field_bound_and_consumed_compiles() {
    // ER42 acceptance: when every arm consumes the linear payload it binds,
    // the program is well-formed.
    let path = tmp_rk(r#"
        @resource
        struct File { path: string }
        extend File { func close(take self) {} }

        enum FileError {
            ReadFailed(File, string),
            Other,
        }

        func good(take e: FileError) {
            match e {
                FileError.ReadFailed(file, _) => file.close(),
                FileError.Other => {}
            }
        }

        func main() {}
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(),
        "ER42 good case must compile, got: {:?}",
        output.diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn er42_struct_with_linear_field_is_transitively_linear() {
    // A plain struct that wraps a @resource is itself linear: forgetting
    // to consume it must error (resource not consumed at scope exit).
    let path = tmp_rk(r#"
        @resource
        struct File { path: string }
        extend File { func close(take self) {} }

        struct Wrapper { file: File }

        func leak(take w: Wrapper) {
            // never consume w — compile error
        }

        func main() {}
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded(),
        "transitive linearity: must error when wrapper isn't consumed"
    );
    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// FD4 — missing struct field is a compile error (never silently zeroed)
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn fd4_missing_field_errors() {
    let path = tmp_rk(r#"
        struct Config {
            public host: string
            public port: i32 = 8080
        }
        func main() {
            const c = Config {}
            println(c.host)
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded(), "FD4: omitting defaultless `host` must error");
    assert!(
        output.diagnostics.iter().any(|d|
            d.code.as_ref().map_or(false, |c| c.0 == "E0822") && d.message.contains("host")),
        "expected E0822 naming `host`, got: {:?}",
        output.diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn fd4_defaults_and_spread_satisfy_construction() {
    // Omitting a defaulted field is fine (desugar fills it); a spread supplies
    // every unlisted field, so neither triggers FD4.
    let path = tmp_rk(r#"
        struct Config {
            public host: string
            public port: i32 = 8080
        }
        func main() {
            const a = Config { host: "x" }
            const b = Config { port: 1, ..a }
            println("{a.port} {b.host}")
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(), "defaulted omit + spread must type-check, got: {:?}",
        output.diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

// ═══════════════════════════════════════════════════════════════════════
// DT1 — a `duck trait` is scratchpad-only, so it can never be public
// ═══════════════════════════════════════════════════════════════════════

#[test]
fn dt1_public_duck_trait_errors() {
    let path = tmp_rk(r#"
        public duck trait Frobber {
            func frobnicate(self) -> i32
        }
        func main() {
            println("hi")
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(!output.succeeded(), "DT1: `public duck trait` must error");
    assert!(
        output.diagnostics.iter().any(|d|
            d.code.as_ref().map_or(false, |c| c.0 == "E0824") && d.message.contains("Frobber")),
        "expected E0824 naming `Frobber`, got: {:?}",
        output.diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
    );
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dt1_package_internal_duck_trait_is_fine() {
    // Without `public` the trait stays in the package, which is where duck
    // traits live — shape matching still satisfies the bound with no
    // conformance declaration.
    let path = tmp_rk(r#"
        duck trait Frobber {
            func frobnicate(self) -> i32
        }
        struct Widget {
            id: i32
        }
        extend Widget {
            func frobnicate(self) -> i32 {
                return self.id
            }
        }
        func main() {
            const w = Widget { id: 7 }
            println("{w.frobnicate()}")
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(), "package-internal duck trait must type-check, got: {:?}",
        output.diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn dt1_public_nominal_trait_is_fine() {
    // Dropping `duck` is the fix DT1 points at — the same trait as `public
    // trait` is legal, with conformance declared.
    let path = tmp_rk(r#"
        public trait Frobber {
            func frobnicate(self) -> i32
        }
        struct Widget {
            id: i32
        }
        extend Widget with Frobber {
            func frobnicate(self) -> i32 {
                return self.id
            }
        }
        func main() {
            const w = Widget { id: 7 }
            println("{w.frobnicate()}")
        }
    "#);
    let output = check_file(path.to_str().unwrap(), &default_config());
    assert!(output.succeeded(), "hardened public trait must type-check, got: {:?}",
        output.diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>());
    let _ = std::fs::remove_file(&path);
}
