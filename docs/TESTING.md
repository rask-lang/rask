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
| **Agent benchmark gate** | `tests/agentbench_gate.sh` | Every `agentbench/` reference solution still builds on both backends, and the harness runs end to end on a mock model |

The agent-benchmark gate is a different question. `agentbench/` measures how
well models write Rask against this compiler — solve rate, how many attempts to
converge, and which diagnostics actually get a model unstuck. The gate here
calls no model and spends nothing; it checks that every task's hand-written
reference solution is still green on both backends (a task whose reference has
gone red is measuring the compiler, not the model) and that the harness itself
still works, using a deterministic mock. `agentbench/quarantine.txt` is that
side's `known_divergences.txt`. See [agentbench/README.md](../agentbench/README.md).

The differential harness is the parity gate. `rask test` runs one backend per
invocation, so interp/native drift was invisible; the harness runs both, strips
timings, and compares pass/fail **and** output. A test that passes on interp but
crashes, mis-prints, or emits nothing on native is a failure — that's the point.

## Expected-red files: bugs vs. the TDD backlog

Not every suite file is green — some are deliberately red, in two registries the
harness treats as non-fatal:

- **`tests/known_divergences.txt`** — bugs/regressions: a feature that *should*
  work but is broken on a backend. Red here is bad news.
- **`tests/pending_features.txt`** — the **TDD backlog**: tests that assert
  spec-correct behavior for features that aren't built yet (SIMD, bits, `select`,
  numeric limits, `.rev()`/`.step()`, operator overloading, `Owned<T>`, the
  sequence protocol, `@binary`, native math/boxes/os…). These are *supposed* to
  be red — the test encodes the spec and drives the implementation. When the
  feature lands, the test flips green and the harness prints **UNEXPECTED PASS**
  telling you to promote it out of the backlog.

Between them, these two files are the complete list of red suite files. The
harness **fails if any other file is red** (a new regression) or if a
registered file silently starts passing (prune/promote it). Green is enforced
for everything else. Pending files carry a `// PENDING <spec-id> — …` header and
assert the real expected values, never the current wrong behavior — so the suite
doubles as an executable spec and a to-do list, not just a regression net.

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
- **Pending feature (TDD):** write the test for an *unbuilt* spec feature the
  same way — assert the correct values, `// PENDING <spec-id>` header — and
  register it in `pending_features.txt`. It's red until someone builds the
  feature, then it's the acceptance test.

## Current coverage (2026-07-26)

Witnessed on BOTH backends (green in the harness):

| Area | Rules / surface | File |
|------|-----------------|------|
| Stdlib renames (#302) | `receive`, `fs.*_text`, `Duration.as_seconds*`, `Random`, `{:debug}`, `time.sleep`, `Channel.receive` | `suite/t40_stdlib_renames.rk` |
| Old names rejected | `recv`/`as_secs`/`getpid`/`vars`/`read_file`/`File.lines` (E0313) | `compile_errors/stdlib_renames.rk` |
| Optionals / results | OPT5/6/9/10/11/13/15/19, ER9/12/15/16/23 | `suite/t42_optionals_results.rk` |
| Tuples | TU1/TU2/TU5/TU7 construction, access, destructuring, params (scalar) | `suite/t44_tuples.rk` |
| Multi-variant unions | anonymous-enum construction, per-variant `try` widening, success extraction | `suite/t45_unions.rk` |
| Primitives | P1/P4 sized-int + bounds, CV1/CV5/CV6/CV7 casts, bool, char | `suite/t46_primitives.rk` |
| Ranges | R1/R2/R4 exclusive/inclusive/empty iteration + bounds | `suite/t47_ranges.rk` |
| Loops | while/loop/for, break-value, nested break/continue | `suite/t48_loops.rk` |
| Comptime | CT2/5/9/10/20/22/23 const/block eval; native fold proven | `suite/t49_comptime.rk` |
| Pools / Handle | insert→Handle, access, len, remove, iterate | `suite/t51_pools.rk` |
| Value semantics | Copy vs move, clone independence, 16-byte threshold | `suite/t52_value_semantics.rk` |
| Strings | len/find/contains/trim/case/substring/replace/split/… | `suite/t54_strings.rk` |
| Collections | Vec + Map method surface (beyond t09/t13) | `suite/t55_collections.rk` |
| Concurrency | `Thread.spawn`/`join`, own-capture, channel return | `suite/t57_spawn.rk` |
| Operators | user `<`/`>`, `Comparable` generic bound, int comparisons | `suite/t59_operators.rk` |
| os (interp; native Track 4) | `os.pid`/`env_vars`/`env` | `suite/t41_os.rk` |

Tracked KNOWN-FAIL witnesses (RED until fixed, in `known_divergences.txt`):
`t43_widening_regressions.rk` (optional widening interp #393, result T-bind native #389),
`t53_math.rk` (math module unlinked in codegen), `t50_boxes.rk` (`Shared` strategies, native).

Filed from the harness — native codegen: #386 (struct param+return), #387 (enum string
payload), #388 (f64 enum payload), #389 (union error handling), #390 (test-registration
drop); interp: #391 (T-or-E discrimination), #392 (labeled break/continue), #393 (optional
widening); checker: #394 (`??` on `T or E`), #395 (bogus import / `{:?}`). Reopened: #256,
#258, #270. The breadth pass added a second wave (math/bits/collections-codegen/operator-
overloading/select/boxes/comptime-native/interp-struct-copy-aliasing) — see the issue
tracker and `known_divergences.txt` for the live list.

Pending-feature backlog (RED tests that assert spec behavior, in `pending_features.txt`):
`p01_bits`, `p02_numeric_limits`, `p03_comparison_surface` (char/tuple/bool ops),
`p04_ranges_rev_step`, `p05_operator_overload`, `p06_select`, `p07_owned` (`Owned<T>`),
`p08_sequence` (sequence protocol), `p09_simd`, `p10_binary` (`@binary`, interp-done/native-gap),
plus the native-backend gaps `t41_os`, `t50_boxes`, `t53_math`. Each flips green when its
feature is built.

Still without a witness (next): typed JSON, net/http (native-thin); panics/ensure have
`cargo test` + `compile_errors/` coverage but no suite blocks (panics abort test runs); Batch 2
soundness (overflow #325 done; linear-in-containers, cross-task send, ensure definiteness) have
`compile_errors/` coverage — audit each against its rule table.
