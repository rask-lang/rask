# Testing & backend parity

Rask executes on two backends — the tree-walking interpreter (`rask run --interp`,
the reference) and native Cranelift codegen (`rask run --native`). A feature isn't
done until both agree. This is the map of what enforces that.

## Layers

| Layer | Command | What it proves |
|-------|---------|----------------|
| Parse | `rask test-specs specs/` | Spec snippets parse (weak — no semantics) |
| Rust integration | `cargo test` (compiler/) | Fixtures compile+run natively; `compile_errors/` are rejected for the right reason |
| Suite (per backend) | `rask test tests/suite/` | `test { … assert }` blocks on one backend |
| **Differential** | `tests/differential.sh` | **Every suite file on BOTH backends; fails on any untracked divergence** |
| **Example gate** | `tests/examples_gate.sh` | Every example with a golden matches on both backends |

The differential harness is the parity gate. `rask test` runs one backend per
invocation, so interp/native drift was invisible; the harness runs both, strips
timings, and compares pass/fail **and** output. A test that passes on interp but
crashes, mis-prints, or emits nothing on native is a failure — that's the point.

## Tracked divergences

Known interp/native divergences (and shared bugs) are registered in
`tests/known_divergences.txt`, one suite basename per line with its issue. The
harness reports them as KNOWN-FAIL (non-fatal) and **fails if a tracked file
silently starts passing** (prune it) or a **new untracked red file appears**. So
the registry is the complete list of red suite files, and green is enforced for
everything else.

Examples that don't yet run on both backends have no golden and are listed in
`tests/known_fail_examples.txt`. Generate a golden and delete the line when an
example goes green.

## Adding a test

- **Positive (behavior):** a `test "id/rule … " { … assert … }` block in
  `tests/suite/`. Must pass check + interp + native. Put the spec rule ID in the
  name. Assert **values/output**, never just exit code.
- **Negative (rejection):** a file in `tests/compile_errors/` with a `// ERROR:`
  line per rule, wired into `compiler/crates/rask-cli/tests/compile_run.rs`
  (`compile_error_output` checks the diagnostic code/message, not just non-zero
  exit) and a row in `tests/compile_errors/README.md`.
- **Regression for an open bug:** witness the spec-correct behavior (leave it
  RED), add a `// KNOWN-FAIL #NNN` comment, and register the file in
  `known_divergences.txt`. Don't assert the current wrong behavior.

## Current coverage (2026-07-26)

New this session — witnessed on both backends:

| Area | Rules | File |
|------|-------|------|
| Stdlib renames (#302) | `receive`, `fs.*_text`, `Duration.as_seconds*`, `Random`, `{:debug}`, `time.sleep`, `Channel.receive` | `suite/t40_stdlib_renames.rk` |
| Old names rejected | `recv`/`as_secs`/`getpid`/`vars`/`read_file`/`File.lines` (E0313) | `compile_errors/stdlib_renames.rk` |
| Optionals / results | OPT5/6/9/10/11/13/15/19, ER9/12/15/16/23 | `suite/t42_optionals_results.rk` |
| os (interp; native = Track 4) | `os.pid`/`env_vars`/`env` | `suite/t41_os.rk` |

Regression witnesses (RED until fixed, tracked): `suite/t43_widening_regressions.rk`
(optional widening interp lag #393, result T-bind native #389).

Native codegen bugs surfaced by the harness and filed: #386 (struct param+return),
#387 (enum string payload), #388 (f64 enum payload), #389 (union error handling),
#390 (Map<_,string> test blocks), #391 (interp T-or-E discrimination), #392 (interp
labeled break/continue), #393 (interp optional widening), #394 (`??` on `T or E`),
#395 (bogus import / `{:?}`). Reopened regressions: #256, #258, #270.

Still uncovered (next): overflow on both backends is done (#325); linear-in-containers,
cross-task send, ensure definiteness have `compile_errors/` coverage — audit each
against its rule table (Batch 2). Sequence protocol, ranges, SIMD, `Owned<T>` linearity
have no witnesses yet.
