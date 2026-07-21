# Track 1 Prompts

One prompt per Track 1 item in [PLAN.md](PLAN.md). Each is self-contained — paste into a fresh
session. Shared rules baked into every prompt: understand before changing, fix the cause, error
messages go through rask-diagnostics, and every fix ships with conformance tests that pass on
**both** backends (`rask check` + `rask run --interp` + `rask run`). Evidence line numbers were
verified at commit ce8b13e; re-locate if the file has moved.

---

## 1.1 Ownership branch-merge unsoundness

```
Fix an unsoundness in the ownership checker's branch merge. In
compiler/crates/rask-ownership/src/lib.rs, merge_branch_bindings (~line 1241) treats a binding as
Moved only if it was moved in BOTH branches of an if/else — its own comments say "Moved in then but
not else — keep else state (not moved)". So a value moved or a linear resource consumed in exactly
one branch is considered live/unconsumed after the join. This defeats mem.ownership/O3 (use after
move) and mem.linear/L1 (consume exactly once) for all conditional code, and is the general form of
issue #294 (if-without-else accepting single-branch consumption of a linear resource).

The correct merge for use-after-move is: moved in EITHER branch => "maybe moved" after the join, and
any later use is an error (spec treats maybe-moved as moved). For linear consumption the dual holds:
consumed in only one branch is an error at the join unless the other branch also consumes or the
value is consumed on every path afterward — check what specs/memory/linear.md and
specs/control/ensure.md (C3–C5 definiteness) say before deciding how strict the join must be, and
read issues #294, #295, #296 first; this fix should subsume #294 and may interact with the ensure
runtime-flag machinery.

Also audit the loop case: check how binding states from a loop body merge back (a move inside a loop
body is a use-after-move on the second iteration).

Deliverables:
- Fix in rask-ownership with clear diagnostics (what moved where, which branch) via rask-diagnostics.
- Negative tests in tests/compile_errors/ covering: move in one branch then use after join; linear
  resource consumed in one branch only (both if/else and if-without-else); move inside a loop body.
  Tag each with the rule it witnesses (mem.ownership/O3, mem.linear/L1).
- Positive tests in tests/suite/ for the legal forms: moved in both branches; consumed in both;
  conditional move followed by reassignment before use.
- Run the full tests/suite/ on check+interp+native to catch programs the stricter checker now
  rejects; fix fallout properly (the examples/ programs must still compile).
- Close #294 via the PR if subsumed; update #295/#296 with findings.
```

## 1.2 Integer overflow semantics

```
Implement integer overflow semantics per specs/types/integer-overflow.md — this is currently
unimplemented in both backends and it's a headline spec promise (panic on overflow in ALL builds,
OV1/OV3/OV4).

Current state:
- Native: compiler/crates/rask-codegen/src/builder.rs (~line 2199) emits raw iadd/isub/imul with no
  overflow trap, and raw udiv/sdiv with NO divide-by-zero check (OV2) — the interpreter has the
  div-zero check, so the backends diverge.
- Interpreter: compiler/crates/rask-interp/src/interp/operators.rs does i64 arithmetic with no
  width-aware overflow checks.
- No shift-amount-exceeds-width panic (SH1) anywhere.
- specs/types/integer-overflow.md also defines elision rules (EL1–EL3) via range analysis —
  rask-mir/src/analysis/intervals.rs exists for bounds checks and could be reused later; do NOT
  block the semantics on the optimization. Correctness first, elision as a follow-up.

Read the spec fully first, including negation (-x at MIN), the panic message format, and comptime
behavior (CT1: overflow in comptime eval is a compile error — check rask-comptime already does
width-aware arithmetic; fix if not).

Deliverables:
- Interp: width-aware checked arithmetic for all integer types (i8..i128, u8..u128) on + - * / % -x
  and shifts, panicking with a message that names the operation and type.
- Native: overflow traps (Cranelift supports trapping variants / explicit checks), div-zero and
  shift checks. Use the same panic path as other runtime panics so ctrl.panic semantics apply.
- Positive tests: arithmetic at type boundaries that should NOT panic (MAX with no overflow, etc.).
- Panic tests: overflow on each op class, div by zero, shift == width, on both backends. Follow the
  existing test conventions for expected-panic tests; if none exist, establish one.
- Benchmark note: run benchmarks/ before/after to quantify the check cost; report, don't optimize
  prematurely.
- File a tracking issue first if none exists (PLAN.md Track 1.2 says it needs filing); reference it
  in the commit.
Out of scope (file follow-ups): Wrapping<T>/Saturating<T> and wrapping_*/checked_* methods (W1–W5,
M1), elision (EL1–EL3).
```

## 1.3 `as` cast rules + lossy conversion forms

```
Enforce the conversion rules from specs/types/primitives.md (CV1–CV10, CH5, BL3). Today the type
checker accepts ANY `as` cast: in compiler/crates/rask-types/src/checker/check_expr.rs
(ExprKind::Cast, ~line 948) the only validation is trait satisfaction for `as any Trait`; every
numeric cast falls through and returns the target type unchecked.

Spec: `as` is lossless widening only (CV1–CV4). Narrowing (i32 as i8), sign reinterpretation
(i32 as u32 when negative-capable), float<->int via `as`, int-to-char (CH5), and int-to-bool (BL3)
are compile errors. The sanctioned lossy forms are separate syntax (CV5–CV10): `truncate to T`,
`saturate to T`, `try convert to T` (returns T?), and explicit float-to-int forms — read the spec
for the exact surface; none of it is parsed today (grep the lexer/parser first to confirm).

This is a two-part task; do them in order:
1. Reject invalid `as` casts in the checker with diagnostics that suggest the correct form
   (suggestions.rs — show `x truncate to i8` as the fix, not prose). Then run tests/suite/,
   examples/, and stdlib/*.rk on check to find code relying on the old permissive `as`; fix each
   site to the spec-correct form. Expect fallout — that's the point.
2. Implement the CV5–CV10 forms end to end: lexer/parser (specs/SYNTAX.md for precedence), AST,
   typecheck, interp semantics, MIR lowering, native codegen. Truncate/saturate/try-convert
   semantics must match the spec tables exactly at the boundaries (round-toward for floats etc. —
   read the spec, don't assume).

Deliverables:
- compile_errors tests for each rejected cast class (tag CV1–CV4, CH5, BL3).
- suite tests exercising every CV5–CV10 form at boundary values, identical results on both backends.
- char.from_u32 must return char? per CH3 — the interp currently returns a bare char with
  unwrap_or('\0') (rask-interp/src/interp/eval_expr.rs ~line 1409); fix it with the same change.
- File the tracking issue first (PLAN.md Track 1.3 — not yet filed).
Coordinate with overflow work (1.2): checked arithmetic and conversions share panic/runtime helpers.
```

## 1.4 Trait-object vtable dispatch

```
Fix trait-object method dispatch for native codegen. Two defects in the `any Trait` path
(specs/types/traits.md):

1. Vtable offset bug: compiler/crates/rask-mir/src/lower/expr.rs (~line 1467) computes
   vtable_offset = 24 + idx*8 where idx is the method's position among ALL of the trait's methods
   (self.ctx.trait_methods). Check how the vtable is actually laid out in
   rask-codegen/src/vtable.rs — the spec (TR1) says only object-compatible methods go in the
   vtable, so if the layout skips incompatible methods (Self-returning, generic), any trait with an
   incompatible method before a compatible one dispatches through the WRONG SLOT — a silent
   miscompile. First write a failing test that demonstrates it (trait with a Self-returning method
   declared before a normal method; call the normal method through `any Trait` natively). Then fix:
   either both sides index the compatible-only list, or the vtable stores all methods with
   compatible ones populated — pick the one the spec's layout table prescribes.
2. TR3 unenforced: calling a generic method through `any Trait` should be a compile error at the
   conversion site (or the call site — read TR1–TR3 for which), with a diagnostic explaining why
   it can't be dispatched dynamically. Today nothing rejects it.

Also reproduce and fold in issue #194 ("vtable not found for any Trait") — likely the same area;
diagnose whether it's a separate registration gap in rask-codegen/src/dispatch.rs before fixing.

Deliverables:
- Failing-then-passing native test for the offset bug (this cannot be tested through the
  interpreter — its dynamic dispatch masks the bug; the test must run the compiled binary).
- compile_errors test for TR3.
- suite test: heterogeneous Vec<any Trait> calling methods through the box on both backends (TR7).
- Update/close #194 with the diagnosis.
```

## 1.5 Panic semantics (ctrl.panic)

```
Work the panic-semantics tracking issue #299 (spec: specs/control/panics.md, accepted; also
specs/control/ensure.md). Read #299 and its sub-issues (#287, #288, #289, #290, #291, #298) and the
spec's own "Implementation status" section first — the spec is precise about the target semantics.

Current state, verified:
- Native: the panic path runs NO ensures and abort()s the whole process
  (compiler/runtime/panic.c ~line 118); codegen only emits ensure bodies on the normal
  CleanupReturn path (#287). Spec: panic kills the TASK, unwinds running ensures (P1/U1), main
  task panic exits the process with code 101 (P4).
- Interpreter: ensures DO run on panic, but an ensure-body panic skips the remaining ensures and
  the secondary panic is silently dropped (#289; spec E2/E3: remaining ensures still run,
  first panic wins, secondary panics are reported). with-block writes are buffered and discarded
  on panic (#290; spec U2: released writes are kept). Uncaught panic exits code 1, spec says 101
  (#291).
- Runtime: green.c join re-panics in the joiner instead of returning JoinError.Panicked (#288) —
  but note green.c is not currently linked into builds (PLAN.md Track 5.5), so prioritize the
  thread.c/OS-thread join path; fix green.c in passing only if trivial.

Suggested order (each its own PR-sized chunk):
1. Interpreter E2/E3 + U2 + exit code 101 (#289, #290, #291) — smallest, establishes the reference
   semantics and the test corpus.
2. Native unwind: panic runs ensures for the panicking task, then task-kill; main-task panic =>
   exit 101 after unwind (#287). This needs a real unwind mechanism in codegen/runtime — read the
   spec's implementation notes; do not guess an ABI. If the full design is too large for one
   session, land the runtime plumbing behind the existing panic entry point and file precise
   follow-ups on #299.
3. Join semantics (#288, ctrl.panic/O1) + the runtime surface items in #298.

Every semantic rule you implement gets a suite test that panics and asserts observable cleanup
(e.g. ensure writes to a file that the harness checks) — on both backends once step 2 lands.
staged() (ST1–ST4) is out of scope here — it's tracked separately (#292).
```

## 1.6 Generic-layout miscompile (Cranelift verifier errors)

```
Fix a native-codegen miscompile class: generic struct methods hit Cranelift verifier errors —
f64 values used where a pointer/int is expected. Repro right now:

  compiler/target/release/rask compile examples/sensor_processor.rk -o /tmp/out
  => "Compilation(Verifier(... arg 0 (v24) with type f64 failed to satisfy type set ..." in
  ProcessorState_compute_averages.

Related open issues — read both before starting: #272 (generic struct method returning T where
T = string returns empty string) and #259 (ER24 early-exit narrowing reads wrong layout). This may
be one root cause (monomorphized layout/type substitution disagreeing between rask-mono layout
computation and rask-codegen) or several; diagnose first with
`rask compile --dump-mir examples/sensor_processor.rk` and a minimized .rk repro before touching
code. compiler/CLAUDE.md: struct layout bugs live in rask-codegen/src/layouts.rs and
rask-mono/src/layout.rs.

Understand-before-changing applies doubly here: explain in the PR description exactly why the
wrong Cranelift type is chosen (which substitution or layout lookup goes stale) before the fix.

Deliverables:
- Minimal repro added to tests/suite/ (generic struct + method over f64 fields and over string,
  asserting correct values) — must pass check+interp+native.
- examples/sensor_processor.rk compiles and runs natively.
- If #272/#259 turn out to be the same root cause, close them via the PR; if not, update them
  with what you learned and file a new issue for this specific bug.
```

## 1.7 Index expression types unchecked

```
Fix issue #310: index expression types are never checked — `vec[string_key]` typechecks and fails
at runtime. In rask-types/src/checker/check_expr.rs, find the Index/subscript path and add type
checking: Vec<T> requires an integer index (which widths? check specs/stdlib/collections.md),
Map<K,V> requires K, Pool<T> requires Handle<T> (and reject cross-pool handle types where
statically known — see specs/memory/pools.md PH rules), tuples take only literal indices (TU5/TU6,
specs/types/tuples.md — verify out-of-bounds literal is a compile error as TU6 says).

The diagnostic must say what index type the container expects and what was found, with a
suggestion when there's an obvious fix (e.g. `.get(&key)` vs indexing — match whatever the spec
sanctions; read collections.md first, don't invent API).

Deliverables: compile_errors tests per container class; suite check that valid indexing still
works everywhere (run full suite on all three paths); close #310.
```

## 1.8 Linear values in containers

```
Enforce the container rules for linear values from specs/memory/resource-types.md (RC1–RC4) and
specs/memory/linear.md: Vec<T> and Map<K,V> must REJECT element types that are linear
(@resource structs, Owned<T>, transitively-linear structs) at compile time — today nothing stops
Vec<FileHandle>, and linear values can be silently dropped inside collections, breaking
consume-exactly-once. Pool<T> and T? are the sanctioned containers (RC2/RC4).

Implementation: rask-ownership already computes transitive resource-ness
(is_transitive_resource_by_id, rask-ownership/src/lib.rs ~line 2225); the rejection likely belongs
in the type checker at instantiation sites (Vec<T> construction, push/insert calls, struct fields
of type Vec<Resource>, generic instantiation where T binds to a linear type through mono).
Find ALL the routes a linear type can enter a Vec/Map — direct literal, push, generic call,
collect — and cover each. Check the specs for the exact rule statements and error wording direction.

Also verify R5 while you're here: Pool<Resource> panics on drop if non-empty (take_all is the
consume path). If the runtime panic isn't implemented in the interpreter and native runtime
(pool.c), implement or file precisely.

Deliverables: compile_errors tests for each entry route (tag RC1/RC3); suite test that
Pool<Resource> + take_all works and non-empty drop panics (both backends); file the tracking
issue first (PLAN.md Track 1.8 — not yet filed).
```

## 1.9 Cross-task ownership

```
Implement the cross-task ownership rules from specs/memory/ownership.md T1–T3, currently
completely unenforced (no logic in rask-ownership):
- T1: sending a value over a channel transfers ownership — the send consumes the value like a
  take-param; use after send is use-after-move. Today channel.send() doesn't consume (related:
  #296, where take-param/send consumption isn't recognized without call-site `own`).
- T2: no shared mutable state across tasks — verify what the spec requires the checker to reject
  beyond what Send-ability of Shared/Mutex already covers.
- T3: borrows must not cross task boundaries — a spawn closure capturing a borrow/view whose
  source lives outside must be rejected (interaction with specs/memory/closures.md capture rules —
  read both).

Start by reading how spawn closures capture today (rask-ownership closure handling +
rask-types/src/checker/borrow.rs) and how send is typed (rask-stdlib stubs for channels). The fix
is checker-side flow logic, not runtime.

Deliverables: compile_errors tests for use-after-send, borrow-capturing spawn, and whatever T2
mandates; suite tests for the legal patterns (send owned value, clone-then-send, move-capture
spawn); resolve or fold in #296; file the tracking issue first (PLAN.md Track 1.9 — not yet
filed).
```

## 1.10 Ensure cancellation: static definiteness (C3–C5)

```
Replace the runtime drop-flag mechanism for ensure cancellation with the static definiteness
analysis the spec requires: specs/control/ensure.md C3–C5. Read the spec's own implementation-
status section and issues #293 (the implementation task), #295 (nested-block cancellation bug:
rollback runs after commit), #296 (consumption recognition) first.

Target semantics: `ensure` on a linear value is cancelled by explicit consumption; whether the
compiler ACCEPTS a maybe-consumed value is a static property (C4: consumption must be definite on
every path or on none — maybe-consumed is a compile error), so no runtime flags are needed
(C3), and consumption is recognized uniformly across take-calls, sends, and returns (C5, ties into
#296 and Track 1.1's branch-merge fix — coordinate: this analysis sits on top of the fixed merge).

This is checker/ownership work in rask-ownership (ensure_registered / mark_ensure_resources,
~line 2246) plus removal of the interpreter's runtime cancellation flags
(rask-interp/src/resource.rs, ensure_receiver_consumed in call.rs) once the static analysis
guarantees definiteness. Sequence it AFTER 1.1 lands — the definiteness analysis is only sound on
a sound branch merge.

Deliverables: compile_errors test for maybe-consumed (consume in one branch, not the other, with
an ensure pending — C4); suite tests for definite-consume-cancels-ensure and no-consume-runs-
ensure including the #295 nested-block case (both backends); close #293/#295 as delivered,
update #296.
```

---

Suggested order: 1.1 → 1.7 → 1.6 → 1.4 (independent, unblock validation programs) can run in
parallel with 1.2 → 1.3 (shared runtime helpers); then 1.5; 1.8/1.9 after 1.1; 1.10 last (depends
on 1.1, coordinates with 1.5's ensure work).
