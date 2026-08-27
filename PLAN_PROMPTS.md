# Session prompts

Self-contained prompts, one per lane. Paste into a fresh session; each ends in its own PR.

**They're partitioned by crate so several can run at once.** The lane table says which files each
one owns. A lane that needs to touch a file outside its list should say so in the PR rather than
just doing it — that's how two sessions end up rewriting the same 7,000-line file.

Measured 2026-08-24 at `bd83147` (after #971). Re-measure before trusting any number here.

```
tests/differential.sh      329 green, 20 expected-red, 0 untracked, 0 unexpected-pass
tests/examples_gate.sh     34 ok      tests/projects_gate.sh   21 ok
tests/fmt_roundtrip_gate.sh clean     tests/http_api_harness.sh ok both backends
cargo test --release --workspace      52 binaries, 0 failures
```

## Lanes

| Lane | Owns | Issues |
|---|---|---|
| **L1 Namespace** | `rask-resolve/`, `rask-stdlib/mir_metadata.rs`, corpus `import` lines | #977, #923, #975 |
| **L2 Interpreter** | `rask-interp/` | #935, interp halves generally |
| **L3 Diagnostics & tooling** | `rask-diagnostics/`, `rask-lint/`, `rask-fmt/` | #900, #892, #893, #897, #898 |
| **L4 Codegen singles** | `rask-codegen/src/builder.rs` | #903, #933, #929 |
| **L5 Comptime** | `rask-comptime/`, MIR comptime paths | #930, #931 |
| **L6 Panics & unwinding** | `runtime/*.c`, codegen panic path | #299 family |
| **L7 Agent benchmark** | new files only | — |

**L4 and L6 both reach `rask-codegen/src/builder.rs`.** Run one or the other, not both, unless
you're willing to merge that file by hand.

**Every lane touches `tests/known_divergences.txt` and `tests/COVERAGE.md`** — one line added or
deleted per fix. Conflicts there are trivial: take both sides. Tell lanes not to edit `PLAN.md`;
fold that in when you merge.

---

## Shared preamble

Paste this at the top of any lane prompt.

```
Read CLAUDE.md and compiler/CLAUDE.md first. Work on branch <BRANCH>, open a draft PR when the
first fix lands, keep it green.

Verify every change on BOTH backends. A fix isn't done until:
  rask test --interp tests/suite/<probe>.rk   and   rask test tests/suite/<probe>.rk
agree, and the file has left tests/known_divergences.txt.

Before pushing, run: tests/differential.sh, tests/examples_gate.sh, tests/projects_gate.sh,
and `cd compiler && cargo test --release --workspace`. All were green at bd83147, so any red is
yours.

Five things that cost real time in the last session — they will cost you the same:

1. YOUR FIRST EXPLANATION IS PROBABLY WRONG. It was wrong about half the time last session, every
   time convincingly. Instrument before you theorise: `--dump-mir`, a temporary eprintln, `gdb
   -batch -ex run -ex 'bt 12'`. One bug was "diagnosed" three times before the measurement said
   what it actually was.

2. GREEN GATES DON'T MEAN CORRECT. `match n { 1 => 2.5, _ => 0.0 }` returned 2 while all six gates
   passed, because no test happened to put a float in a match arm. When you fix something, ask what
   *shape* was untested and add that, not just the reported case.

3. A HALF-FIX LOOKS LIKE A WHOLE ONE. Scoping type parameters fixed the declaration site and left
   every call site broken — the second failure looked identical to the first. Write repros that
   exercise two shapes, not one.

4. WATCH FOR HAND-MAINTAINED LISTS. Several bugs were a name missing from a hardcoded match:
   opaque types in the layout pass, stdlib modules in the resolver (two such lists, disagreeing),
   struct-like types in `resolve_stdlib_symbol`. If your fix is "add a name to a list", ask what
   derives that list and whether it should.

5. NAME YOUR REPRO TYPES SOMETHING UNLIKELY. Calling a test struct `Timer` collided with
   `stdlib/time.rk`'s and hid the bug under a different one (#975). `Budget9` is a fine name.

Explain in chat, not by pointing at a diff — quote the 10-20 relevant lines. End with the 2-4 most
questionable calls you made.
```

---

## L1 — Namespace rules (#977, #923, #975)

The biggest lane and the one with corpus churn. Do it alone if you're only running one.

```
<SHARED PREAMBLE>

Enforce structure.modules' namespace rules. They are specified and none are enforced — issue #977
has the four one-line programs that all wrongly compile today.

  IM1  "there is no set that comes pre-imported"      — every stdlib type resolves bare
  IM8  local shadowing an import is a compile error   — accepted
  BI3  local type named `Vec` is a compile error      — accepted
  BF3  local `func println` is a compile error        — accepted

BLOCKER, FOUND AND NOT YET FIXED — do this first or IM1's error message advises something
impossible. Only 17 of 29 stdlib modules can be imported at all:

  $ rask check <<< 'import memory'     → error[E0207]: unknown package: `memory`
  Same for: string, sync, collections, bits, builtins, char, encoding, error_context,
            fmt, num, option, reflect, result, sequence

`BuiltinModuleKind` in rask-resolve/src/symbol.rs is a hand-written enum, and
`ALL_BUILTIN_MODULES` lists 17 variants. A *second*, different hardcoded list lives at
resolver.rs:1276 (`is_stdlib_module`) and includes `num`, which the enum doesn't. Both should
derive from the actual stub set. Make every stdlib module importable before enforcing IM1.

IM1 is already half-built: `stdlib_module_needs_import` (resolver.rs ~2158) enforces it for module
names only, deliberately — see its comment. Widen it to types, minus BI1's closed builtin set
(primitives, string, Vec, Map, Set, Error, Channel, none). Note IM1 means `import time` gives
`time.Duration`, NOT a bare `Duration`; bare names come only from IM4 `import time.Duration`.

Two halves, both needed: the resolver sees expression positions, but type ANNOTATIONS
(`func f(d: Duration)`, `struct S { d: Duration }`) are strings resolved by the checker's
parse_type_string and slip through entirely. Confirm with a repro before and after.

Blast radius, measured: 102 of 444 corpus files need a new import (13 examples, 80 suite, 6
tutorials, 3 projects), dominated by memory.Pool (20), string.StringBuilder (8), memory.Rack (7).
Script the mechanical part.

DECISION YOU MAY NEED: BI1's builtin list is closed and does NOT include the box family — Pool,
Handle, Rack, Link, Mutex, Shared. That's ~60 of the 102 files. Following BI1 literally means
`import memory` everywhere Pool is used. If that reads as too much ceremony, ask before deciding —
it changes half the work.

Then #923 falls out: `import time.Duration as Span` binds Span as SymbolKind::Variable, not a type,
because resolve_stdlib_symbol's hardcoded is_struct list covers five modules. See the comment on
#923 for the full trace — the fix is to desugar an aliased type import into `type alias Span =
Duration`, which reuses machinery that already works.

And #975 gets much less urgent: a user struct named like a stdlib type currently takes the stdlib's
layout and segfaults, because MirContext::find_struct resolves by bare name against a flat
Vec<StructLayout>. With IM1 the name isn't in scope unasked; with IM8 shadowing is a named error.
Fix the flat-map lookup too if you have room — the checker already solved this with separate
type_names/stdlib_type_names maps (#515).
```

---

## L2 — Interpreter (#935)

```
<SHARED PREAMBLE>

The interpreter treats a raw pointer as a plain i64 (#935): `*ptr` silently yields 0, and
`ptr.read()` / `ptr.offset()` don't exist. Native handles all of it, so this is the interpreter
lagging, and `tests/suite/t_month_unsafe.rk` is 1/6 there against 6/6 native.

Native is the reference for what the answer should be — read rask-codegen's raw-pointer path and
mirror its semantics, don't invent them. specs/memory/unsafe.md has the rules (U3, UF1).

While you're in rask-interp, `t_month_unsafe.rk` and `examples/19_unsafe.rk` are the corpus for
this; 19_unsafe is in tests/known_fail_examples.txt because the interpreter has no `extern "C"` at
all, which is the same gap one layer up. Enrolling that example is a bonus, not the task.
```

---

## L3 — Diagnostics & tooling (#900, #892, #893, #897, #898)

Five small, independent, all in the presentation layer. Good first lane.

```
<SHARED PREAMBLE>

Five diagnostics/tooling bugs, all independent:

#900  E0335 tells you to write `string.concat(a, b)`, which doesn't exist.
#892  `rask explain E0831` prints the wrong error — duplicate match arm in the code registry.
#893  `rask lint` rejects the stdlib's own try_send/try_receive under the `T or E` naming rule.
#897  `assert a == b` on two string variables prints their addresses instead of the strings (native).
#898  A failed float assertion rounds both operands to 6 digits, so unequal floats print as equal.

Diagnostics are a first-class feature here — read the "Error messages" section of CLAUDE.md. For
#900 in particular: write the message you WANT first, then make it true. Every user-facing error
goes through rask-diagnostics; don't eprintln.

#897 and #898 are assertion *rendering*, which lives on the native side — check whether the
interpreter prints these correctly and match it.
```

---

## L4 — Codegen singles (#903, #933, #929)

```
<SHARED PREAMBLE>

Three native codegen bugs, all in rask-codegen/src/builder.rs. Do NOT run this lane alongside L6 —
you'd both be editing a 7,273-line file.

#903  Map.insert returns a replaced/not-replaced flag instead of the displaced value, and segfaults
      for string values. Probe: tests/suite/t_day_map_insert_displaced.rk (interp 5/5, native 0/3).
#933  An i128 inside an aggregate emits `load.i64` from an i128 and fails the Cranelift verifier.
      Probe: t_month_i128_aggregates.rk (interp 10/10, native BUILD-FAIL).
#929  Native defers an `ensure` in a bare block or `if` body to function exit instead of block exit
      (EN1). Probe: t_month_ensure_block_scope.rk (interp 5/5, native 0/5).

For #933, note rask_mono::abi::slot_scalar_bytes now owns "how wide does a scalar sit in a slot"
for floats and narrow integers — if i128 wants a rule, it belongs there, not in a fourth local
answer. Four sites disagreed about this before it existed (#902, #972, both halves of #973).
```

---

## L5 — Comptime (#930, #931)

```
<SHARED PREAMBLE>

Two comptime bugs, both native-only:

#930  Comptime field access by string literal — `p.("x")` — reaches native MIR unresolved. Only the
      `comptime for` form is handled. Probe: t_month_comptime_field_literal.rk (interp 4/4, native
      BUILD-FAIL).
#931  A comptime FieldInfo.name has no resolved type natively: string methods fail at MIR lowering,
      and pushing it into an inferred Vec corrupts the string. Probe:
      t_month_reflect_field_strings.rk (interp 6/6, native BUILD-FAIL).

Both are "the comptime value reached MIR without a type". Related: every node_types miss the
compiler makes is in an auto-derived `compare` body, because auto_derive_traits registers a
MethodSig with no body and the checker never visits what's synthesized later — see the measurement
on #725. If comptime bodies have the same shape, say so on that issue.
```

---

## L6 — Panics and unwinding (#299)

The ROADMAP's first long-term item. Big; probably its own session.

```
<SHARED PREAMBLE>

Make panics run their ensures. Today the panic path in compiled code runs NO ensure blocks and
aborts the process (runtime/panic.c:118), which undercuts the resource-safety promise everywhere
else — a server can't ship with it. Tracking issue #299, sub-issues #287 #288 #289 #290 #291 #298.

specs/control/panics.md is the contract: task-kill plus unwind, ensures run, locks release without
poisoning, opt-in `staged()`. P1, P4, U1, E2-E3.

The interpreter gets multi-ensure panics wrong too (the first ensure-panic skips the rest, the
secondary panic is dropped), so neither backend is the reference here — the spec is.

tests/COVERAGE.md's "holes left on purpose" says why there's no coverage: a test that panics fails,
so asserting on panic behaviour needs a harness that runs a program expecting a non-zero exit.
tests/compile_errors/ is the nearest existing pattern. Building that harness is part of the job.

Do NOT run alongside L4 — you'd both be editing rask-codegen/src/builder.rs.
```

---

## L7 — The agent benchmark

All new files. Zero conflict with any other lane. ROADMAP calls this the instrument that tells you
when the compiler is done.

```
<SHARED PREAMBLE>

Build the agent benchmark NORTH_STAR names and that doesn't exist: models writing Rask against the
compiler, convergence measured, failure transcripts readable.

The problem it solves: "the native compiler is stable" currently has no exit criterion. Every gate
was green while `match n { 1 => 2.5, _ => 0.0 }` returned 2. A bug count doesn't say what fraction
of ordinary programs compile and run correctly — this should.

Shape it yourself, but it needs at minimum: a task set (small programs with known-correct output),
a runner that compiles and runs each on both backends, a convergence metric (how many attempts to a
correct program), and transcripts saved so failures can be read rather than counted.

Read NORTH_STAR.md for what it's meant to measure and METRICS.md for the scoring conventions
already in use. The five validation programs in examples/ are the shape of a task but too big for
one — the day/week/month files in tests/suite/ are closer to the right granularity.

Keep it out of the compiler crates entirely. A new top-level directory is right.
```
