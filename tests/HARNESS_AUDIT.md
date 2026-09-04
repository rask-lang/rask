# Test harness audit — 2026-09-04

A review of what the test harness actually verifies, and where the tests, the
specs and the compiler disagree. **Nothing here is fixed.** Each entry says what
the spec says, what the compiler does, and what the test claims — deciding which
of the three is wrong is a separate call.

Measured on `e6bfaad` with a release build of the compiler.

---

## Part 1 — What the harness can't see

These are holes in the checking machinery, not in any one test. They're first
because they set the ceiling on how much the rest of the suite is worth.

### 1.1 A compile-error test passes if the file fails for *any* reason

`compile_error(name)` in `compiler/crates/rask-cli/tests/compile_run.rs` runs
`rask check` and checks the exit code is non-zero. That's it. A file with twelve
`// ERROR:` markers passes when one fires — or when none fire and an unrelated
typo does.

The numbers, over `tests/compile_errors/` (110 files, 392 `// ERROR` markers):

| Wiring | Files | Markers |
|---|---|---|
| Asserts on the message or code (`compile_error_output`) | 61 | 192 |
| Exit code only (`compile_error`) | 21 | 62 |
| Referenced by no test at all | 28 | 138 |

So **200 of 392 claimed rejections (51%) are backed by nothing stronger than
"the file didn't compile"**, and 138 of them are backed by nothing at all.

The worst individual case is `borrow_errors.rk`: 19 markers, no test runs it, and
9 of the 12 errors it does produce are `E0322 cannot mutate — declared let` —
the file forgot `mut` on its own locals. Six of its eleven claimed borrow rules
(move out of a borrowed parameter, move a non-Copy field out of borrowed self,
escaping borrow, overlapping borrows, non-Copy copy out of a collection, return
from `ensure`) produce no diagnostic at all.

`context_missing.rk` is the clean-cut case: 2 markers, **0 errors — it compiles**
— and no test runs it.

### 1.2 A parse error hides every later marker in the same file

`syntax_rejected.rk` has 12 markers and produces 8 diagnostics. The four that
never fire are all *semantic* checks sitting below parse errors, so the pipeline
stops before the checker ever sees them:

| Line | Claim | What happens |
|---|---|---|
| 48 | `?` for propagation is rejected | never reached |
| 66 | `let` reassignment | never reached |
| 71 | missing return | never reached |
| 78 | comparison chaining | never reached |

Three more fire with a different message than the marker claims: `impl` gets
"Expected ';' or newline after statement" rather than "did you mean 'extend'";
turbofish gets a generic "unexpected `::`"; `&i32` gets "reference types are not
yet implemented" (planned) where the marker says "Rask uses parameter modes, not
reference syntax" (rejected by design).

### 1.3 A `compile-fail` spec block passes on a failure at any stage

`run_compile_fail_test` walks lex → parse → resolve → typecheck → ownership and
returns pass at the first failure, whatever it is. Of the 12 `compile-fail`
blocks in `specs/`, **8 pass at *resolve*** — meaning they fail because the
snippet names symbols that don't exist, not because the rule under test fired.

Every one of these reports ✓ today and verifies nothing:

| Spec | Rule it claims to demonstrate |
|---|---|
| `compiler/advanced-analyses.md:41` | TS8 — access through an Invalid handle |
| `compiler/advanced-analyses.md:72` | TS8 through a must-alias |
| `compiler/advanced-analyses.md:174` | flow-sensitive narrowing |
| `memory/borrowing.md:85` | a borrow outliving its block |
| `memory/borrowing.md:225` | W2 — structural mutation inside `with` |
| `memory/borrowing.md:250` | W2c — removing the bound handle |
| `memory/borrowing.md:258` | W2d — clearing the pool |
| `memory/pools.md:141` | (pool rule) |

The blocks are fragments — `pool`, `vec`, `player`, `get_point` are undefined —
so resolve rejects them before any analysis runs. Nothing in the tree tells us
whether typestate analysis exists at all.

### 1.4 Only 8 spec code blocks in the whole tree are executed

`specs/` holds 1115 ` ```rask ` blocks. Unannotated blocks are skipped by
design, so:

| Annotation | Blocks | What it proves |
|---|---|---|
| none | ~332 | nothing |
| `skip` | 579 | nothing |
| `parse` | ~181 | it parses |
| `compile` | ~14 | it type-checks |
| `run \| expected` | 8 | it produces the documented output |
| `compile-fail` | 12 | it fails somewhere (see 1.3) |

Four blocks in `specs/compiler/advanced-analyses.md` carry
`<!-- test: pass -->`, which isn't a valid annotation. `parse_annotation_multi`
returns `None` for anything it doesn't recognise, so the block is dropped
silently — no warning, exit 0. Somebody wrote those four thinking they were
gating something.

### 1.5 "No tests found" is a pass

`run_test_file_native_inner` prints "No tests found." and returns success; the
interpreter path has no failures to report and also exits 0. `differential.sh`
calls a file green when both backends exit 0 with identical output — which an
empty file satisfies. No suite file is in that state right now, but nothing
detects it if one drifts there.

### 1.6 The two backends print different failure text

Same failure, different words:

```
interp:  assertion failed: 1 == 2 (left: 1, right: 2)
native:  a1.rk:4: assertion failed: 1 == 2 (left: 1, right: 2)

interp:  check failed: 1 == 2 (left: 1, right: 2); check failed: 3 == 4 (left: 3, right: 4)
native:  check failed

interp:  got:      got            (assert_eq on strings)
native:  got:      "got"
```

Native's `check` is the one that matters: it loses the expression, both values,
and the count. A test with three failing checks reports the same two words as a
test with one.

### 1.7 Ungated corners

- `tests/http_api_harness.sh` runs in **no CI job**, and if its golden is
  missing it writes one from the backend under test and passes.
- `tests/matrix/run.sh` exits 0 always — documented as a survey, not a gate.
- Companion `*_test.rk` files (T3/T4) exist nowhere in the repo.

---

## Part 2 — `std.testing` spec vs the runner

| Rule | Spec says | Runner does |
|---|---|---|
| **A4** | `assert_eq`, `assert_ne`, `check_eq`, `check_ne` | only `assert_eq` exists; the other three are `E0200 undefined symbol` |
| **T7** | parallel by default, `--sequential` opts out | always sequential; the flag is accepted and the field is never read |
| **T8** | `--seed X` reproduces a run | prints "accepted but random ordering not yet implemented" |
| **T10** (nested) | `test` blocks nest, output `PASS: parent > child` | parse error: "Expected expression, found 'test'" |
| **T10** (no `try`) | bare `try` in a test body is an `ER47` compile error | `rask check` says OK; interp runs it and passes; native fails the Cranelift verifier (#932) |
| **T11** | `comptime test` runs during compilation, failure is a compile error | `rask check` on a failing `comptime test` **exits 0**; it runs as an ordinary runtime test under `rask test` |
| **T14/T15** | doc-comment code blocks extracted and run | not implemented — a doc test asserting `add(2,3) == 999` reports "No tests found", exit 0 |
| **T17** | `spawn` with no `using Multitasking` is a `CC2` compile error at the call site | type-checks fine; fails at runtime with "spawn outside `using Multitasking {}` block" |
| **T3/T4** | tests may live in `*_test.rk`; same-package tests see private members | each file compiles alone, so the companion file can't see anything — `E0200 undefined symbol`. Zero coverage in the repo |
| **T6/T18/T19** | isolation; runtime-holding tests serialised; drain bounds the test | untested (T18 is moot while everything is sequential) |
| CLI `--verbose` | show all names | field never read |

Working as specified: A1 (assert stops), A2 (check continues), A3 (messages),
T2 (`@test` functions), T12 (`skip`), T13 (`expect_fail`, both directions),
B1/B2 (benchmarks, with real statistics).

**Spec bug:** `T10` is used twice — once for "no `try` in a test body" and once
for nested blocks.

---

## Part 3 — Where a test asserts something the spec forbids

### 3.1 CV1's int→float table isn't enforced, and a test bakes that in

`specs/types/primitives.md` restricts `as` to conversions where every source
value survives:

| Target | Sources `as` allows |
|---|---|
| `f64` | `i8` `i16` `i32` `u8` `u16` `u32` |
| `f32` | `i8` `i16` `u8` `u16` |

and spells out the reason: "`i64 as f32` is a compile error. Past 2^24 an `f32`
can only land on multiples of 128, so a billion-scale count comes back wrong by
hundreds — the same silent precision loss the overflow rules exist to prevent,
riding the one operator that promises the opposite."

All four of these compile and run clean on both backends:

```rask
let a: i64 = -1000;  a as f64      // not in the f64 list
let b: i64 = 3;      b as f32      // spec calls this out by name
let c: u64 = 3;      c as f64      // not in the f64 list
let d: i32 = 3;      d as f32      // not in the f32 list
```

And `tests/suite/t_day_casts.rk` **asserts the first one works**:

```rask
test "int to float where nothing can be lost" {
    let d: i64 = -1000
    assert (d as f64) == -1000.0
}
```

in a file whose own header says the opposite — "int→float names one too, because
past 2^53 an i64 doesn't survive an f64." `cast_rules.rk` covers CV2, CV3, CV4,
CH5 and BL3; CV1 has no compile-error test at all.

### 3.2 P2 (no comparison chaining) isn't enforced

`specs/types/operators.md` P2: "`a < b < c` is disallowed", listed in the edge
case table as a compile error.

Nothing implements the rule. `a < b < c` is rejected only as a fallout type
mismatch, with a message that doesn't mention chaining:

```
error[E0308]: mismatched types
  6 |     let x: bool = a < b < c
    |                   ^^^^^^^^^ expected `bool`, found `i64`
```

plus a spurious `E0361 couldn't work out the type of x` on the same line.

And when both halves happen to type-check, the chain compiles and runs:

```rask
let a = 1; let b = 1; let c = true
let x: bool = a == b == c     // Typecheck OK, prints true
let y: bool = a < b == false  // Typecheck OK, prints true
```

The marker for this in `syntax_rejected.rk` is one of the four that never fires
(1.2).

### 3.3 CV14 ties-to-even is right, but the suite can't tell

`t_day_casts.rk` tests `3.5.round<i32>()! == 4` and `(-3.5).round<i32>()! == -4`.
Both are also what round-half-away-from-zero gives, so the assertions don't
distinguish the two policies. The implementation does get it right — `2.5 → 2`,
`4.5 → 4` — but that's verified by nothing in the tree.

---

## Part 4 — Diagnostic bugs noticed on the way

- **`assert` type error has its roles backwards.** `assert opt()` where `opt()`
  returns `i32?` says "expected `i32?`, found `bool`". The bool is what `assert`
  wants; the `i32?` is what it got. Same inversion on `assert "nonempty"`.
  `assert 1` gets it right.
- **Two error-code schemes in one compiler.** Most diagnostics are `E0817`-style;
  the context ones are `error[mem.context/CC8]`. Anything grepping for `^error\[E`
  misses the second family.
- **One error degrades the next into a wrong one.** With an `E0357` (single-letter
  type name) in the file, `?` applied to a `T or E` came out as "expected i32,
  found bool" at the *use* site instead of `E0368`/ER12 at the `?`. Remove the
  E0357 and the right diagnostic appears.

---

## Part 5 — What holds up

Worth saying, because most of this machinery is good:

- `differential.sh` — 356 green, 17 expected-red, 0 untracked, 0 unexpected-pass,
  0 misfiled. The registry claim-check from #1005 (a red file must keep failing on
  the same backends at the same phase) is the strongest single idea in the harness:
  it catches a probe that stops exercising its bug, which plain red/green can't.
- `examples_gate.sh` — 34 of 36 examples gated on both backends, 2 tracked.
  Enrolment by golden presence means no example is silently outside the gate.
- `COVERAGE.md` — 67 of 68 per-file counts current. Only `t_week_ranges.rk` is
  stale (doc says 14/14, it's 15/15). The "Holes left on purpose" section is
  honest about what a suite file structurally can't cover.
- `leak_gate.sh` — no `known_leaks.txt` exists, so the gate is fully strict.
- `assert` requires `bool`. There is no vacuous-assert path.
- `mem.ownership/O2` — enforced, and the diagnostic names the size and the
  threshold ("`Over` is 17 bytes (copy threshold is 16)").
- `type.strings/S5` — a mid-codepoint slice panics with a message that names
  `char_indices()` as the way out.
- Map iteration order is genuinely randomised per process, as `determinism/D7`
  requires. The suite's map tests are all order-independent (counts and sums).

---

## Open question behind every entry

For each line above, one of three things is true, and this audit deliberately
doesn't pick:

1. the spec is right and the compiler is behind (CV1, P2, T11, T14, CC2 look
   like this),
2. the spec described something we no longer want (T7 parallel-by-default costs
   determinism the harness depends on; nested `test` blocks may not be worth it),
3. the test is wrong (`t_day_casts.rk` asserting `i64 as f64`, and every
   `compile-fail` block that passes at resolve).
