# Compiler–Spec Alignment Plan

The specs moved ahead of the compiler — the trait-system review, the stdlib consistency pass (#303),
the comptime staging model, and the panic/ensure work all landed as spec-only changes. This plan maps
the full gap and orders the work. Sourced from a spec-by-spec audit of the implementation
(2026-07-21, at 6153d73) plus empirical runs.

**Where things actually stand:**

- Core suite is healthy: 30/31 of `tests/suite/` pass check + interp + native (only `t25_iterator_adapters`
  fails — fold accumulator type never resolves). `rask test-specs` passes 178/178, but it only checks that
  spec snippets *parse* — weak conformance signal.
- Validation programs: grep_clone and text_editor compile natively. game_loop fails MIR lowering
  (unresolved closure capture in `spawn`), sensor_processor fails Cranelift verification (generic layout
  miscompile), http_api_server fails `check` (import shadowing) and is blocked on typed JSON + `http.serve`.
- The optimization/analysis passes claimed in TODO genuinely exist and run (clone elision, RC elision,
  generation coalescing, typestate, intervals). The lag is elsewhere: soundness holes, the recent spec
  delta, interp/native divergence, and stdlib wiring.
- The interpreter masks native gaps: `@binary`, trait dispatch, typed JSON, and `os.env` all "work" under
  `rask run` and fail or miscompile natively. Interpreter success must not be read as conformance.

Tracks are ordered. Within a track, items are ordered by impact. Bugs get filed as issues when work
starts; items marked **(file)** aren't tracked yet.

---

## Track 1 — Soundness: make the safety guarantees true

These are cases where the compiler *accepts wrong programs* or *miscompiles correct ones*. They
undermine the core promise (mechanical safety) and get fixed before feature work.

| # | Item | Spec | Status / evidence | Issue |
|---|------|------|-------------------|-------|
| 1.1 | Ownership branch merge is unsound: a value moved/consumed in one branch is treated as live after the join. Defeats conditional use-after-move and linear-consumption checking. | mem.ownership/O3, mem.linear/L1 | `rask-ownership/src/lib.rs:1241-1256` keeps the not-moved state on merge | #294 is the visible symptom; widen it to the general merge bug |
| 1.2 | Integer overflow semantics unimplemented: `+ - * -x` wrap silently in both backends; native also skips the divide-by-zero check the interp has; no shift-width check. | type.overflow/OV1–OV4, SH1 | `rask-codegen/src/builder.rs:2199-2217` raw `iadd/imul/sdiv` | **(file)** |
| 1.3 | `as` casts unchecked: narrowing, sign reinterpret, float↔int, `n as char`, `1 as bool` all silently accepted. And the sanctioned lossy forms (`truncate to`, `saturate to`, `try convert`) don't exist — no correct path, wrong path open. | type.primitives/CV1–CV10, CH5, BL3 | `rask-types/src/checker/check_expr.rs:948-970` only validates `as any Trait` | **(file)** |
| 1.4 | Trait-object dispatch miscompiles: vtable offset computed from position among *all* trait methods, but vtable stores compatible methods only — wrong slot when an incompatible method precedes. TR3 (reject generic methods through `any`) unenforced. | type.traits/TR1, TR3 | `rask-mir/src/lower/expr.rs:1467` | related: #194 |
| 1.5 | Panic path in compiled code runs no ensures and aborts the process; interp gets multi-ensure panics wrong (first ensure-panic skips the rest, secondary panic dropped). | ctrl.panic/P1, P4, U1, E2–E3 | `runtime/panic.c:118` aborts | #299 tracking (#287, #288, #289, #290, #291, #298) |
| 1.6 | Generic layout miscompile: Cranelift verifier errors on generic struct methods (f64 treated as pointer). Breaks sensor_processor today. | — | `rask compile examples/sensor_processor.rk` | #272, #259 family; verify coverage, else file |
| 1.7 | Index expression types never checked (`vec[string]` typechecks). | stdlib.collections | — | #310 |
| 1.8 | Linear values in containers unenforced: `Vec<@resource>` / `Map<_, @resource>` accepted; values silently droppable. `Pool<Resource>` drop-non-empty panic unverified. | mem.resource-types/RC1, RC3, R5 | no enforcement in rask-ownership or checker | **(file)** |
| 1.9 | Cross-task ownership rules unimplemented: channel send doesn't consume, borrows can cross task boundaries. | mem.ownership/T1–T3 | no logic in rask-ownership | **(file)** |
| 1.10 | ensure cancellation is runtime drop-flags, spec requires static definiteness. | ctrl.ensure/C3–C5 | spec's own status section | #293, #295, #296 |

Exit criteria: each item has a compile-error (or panic) conformance test in `tests/compile_errors/` or
`tests/suite/`, passing on both backends. Ready-to-use session prompts: [PLAN_PROMPTS.md](PLAN_PROMPTS.md).

## Track 2 — Recent spec delta: catch the compiler up to decided design

The trait review and consistency passes are decided design; the compiler still implements the old world.
One epic, several mechanical sweeps. (Of the July spec wave, only PC1 — single-letter auto-generics —
landed compiler-side.)

**2a. Nominal trait flip (the epic).** Trait satisfaction is still purely structural
(`rask-types/src/traits.rs`). Implement in dependency order:

1. `where` clause parsing — spec syntax fails today (#313); prerequisite for everything below.
2. Nominal conformance registry: `extend T with Trait` declares, checker consults declarations
   instead of shapes (trait-review G1, #283). Explicit bounds must grant methods (#314).
3. Comma-list conformance `extend T with A, B, C` + block semantics (CD1–CD3) — parser holds a single
   trait name today (`parser.rs:1654`).
4. Explicit `as any` at trait-object conversion sites; drop TR5 implicit boxing (#284).
5. `duck trait` keyword for structural opt-in; `scoped extend` + trait-qualified calls (MN1–MN5).
6. Auto-derive roster: Default is deleted from the spec but still derived
   (`declarations.rs:559` DF1) — remove it; struct field defaults replace it (FD1–FD6, #311) —
   fields don't parse defaults yet. ErrorMessage becomes auto-derived for enums (overridable), not
   structurally required (`errors.rs:325`).
7. Override coherence (OC1–OC3), conditional conformance with explicit `where` (CC1–CC3).
8. Cross-package conformance rules need a decision first (#312).

**2b. Rename/name-drift sweep.** Programs written to spec fail to typecheck where implementations
exist under old names. One reconciliation pass over stubs + interp + runtime + examples (#302):

- `recv`/`try_recv`/`RecvError` → `receive`/`try_receive`/`ReceiveError` (`stdlib/async.rk:96`,
  `rask-interp/builtins/threading.rs:324`)
- `{:?}` → `{:debug}` (`rask-interp/src/interp/format.rs:155`)
- `fs.read_file/write_file/append_file` → `read_text/write_text/append_text`; `io.write_str` → `write_text`
- `Duration.as_secs` → `as_seconds`; `Rng` → `Random`; `os.getpid` → `os.pid`; `os.vars` → `os.env_vars`
- `vec.extend` → `push_all` (`try_push_all` reuses `PushError`; `ExtendError` is gone);
  `BufReader`/`BufWriter` → `BufferedReader`/`BufferedWriter`
- #276/#277/#278 signature changes (TcpConnection `read_text/write_text`, shared `SysError`,
  `time.sleep -> void or SysError`, `File.lines()` dropped — `read_lines` eager, `BufferedReader.lines` lazy)
- Growth ops panic on alloc failure; `try_` variants return the rejected value (std.collections/C2) —
  supersedes the `AllocError` line in TODO.md

**2c. Origin tracking opt-in.** Spec revised to `@traced` + `any Error`; compiler still captures origin
on every error, 16 bytes on every `T or E` (ER33/ER34; `rask-mir/src/lower/errors.rs:56-66`).
Transparency-of-cost violation.

**2d. Comptime staging model.** CT55–CT68 (staging classifier, demand-driven per-instantiation eval,
memoization, cycle/depth limits) — nothing exists yet. Sequenced with Track 3's comptime work.

## Track 3 — Validation-program path: HTTP JSON API server end to end

The litmus programs are the design's own success metric. Program 1 (HTTP server) exercises the longest
dependency chain; clearing it clears most of the stdlib gaps for the others.

1. **Load the orphaned stubs.** `encoding.rk`, `fmt.rk`, `reflect.rk` exist on disk but are not in
   `STUB_SOURCES` (`rask-stdlib/src/registry.rs:13-39`) — the typechecker never sees them. **(file)**
2. **Comptime foundations:** `comptime for` over `reflect.fields<T>()` with `value.(field)` access
   (CT48–CT54) — comptime-for is also broken on native (#317, mono never unrolls); comptime failures
   silently swallowed (#318); `.freeze()` (CT17–CT19); `@embed_file` (CT40–CT44).
3. **Encode/Decode derivation** on top of 1+2: auto-derive, field annotations (`@rename`, `@skip`,
   `@default`), enum tagging (spec.encoding E22–E25). This replaces the interp's ad-hoc runtime
   reflection for typed JSON and gives natively-compiled struct round-trips.
4. **`http.serve` / `listen_and_serve`** — stub bodies are empty (`stdlib/http.rk:727`); Phase A can
   back them with OS threads (conc.runtime-strategy/RS1) without waiting for fibers.
5. **Supporting stdlib for the other four programs:** CLI builder API + `CliError`; IO Reader/Writer
   traits + BufferedReader (the checker also never registers Reader/Writer, #320); Duration/Instant
   arithmetic operators + `SystemTime`; integer bit methods (bits.rk claims they're registered — they
   aren't); `Random.shuffle/choice`; UDP + `net.resolve`.
6. **Fix the three broken examples** (game_loop spawn capture, sensor_processor = 1.6,
   http_api_server import shadowing) and wire all five validation programs into CI as native-compile
   gates. Re-scope #203 to current failures.

## Track 4 — Backend parity: one language, two backends, same answers

Divergence table (each row: bring the lagging backend up, add a dual-backend test):

| Feature | Interp | Native |
|---------|--------|--------|
| `@binary` parse/build | works | missing entirely (no MIR lowering) |
| Typed JSON encode/decode | works (runtime reflection) | missing (C runtime is primitive-only) |
| `select` | works | missing (no MIR/codegen handling) |
| `os.env/platform/arch/pid` | works | missing (no C symbols) |
| Atomics | load/store on 3 types only | full int set + CAS + fences |
| Raw pointers / transmute / unions | typechecked, no runtime | works |
| `comptime` evaluation | deferred to runtime | comptime crate (correct model) |
| Trait objects | dynamic dispatch (masks bugs) | vtables (1.4 bug) |
| Overflow/div-zero checks | div-zero only | neither |
| Map iteration order | diverges from native | diverges from interp — spec wants per-process seeded hash order (determinism/D7, #285); neither conforms |

Rule going forward: a feature isn't done until both backends pass the same test. The suite runner
should exercise check + interp + native for every suite file (it currently can — the gap is
coverage: zero suite tests for panics, ensure, channels, select, spawn, comptime, ranges, overflow,
casts, SIMD, `@binary`).

## Track 5 — Concurrency runtime

Phase A (OS threads) is the decided stopgap; make it *complete* before Phase B (fibers):

1. Cancellation is stubbed: `cancel()` no-op, `cancelled()` always false (H4, CN1–CN3). Timeouts and
   graceful shutdown are impossible today. **(file)**
2. `select` completeness: send-arm ownership (OW1–OW2), `Timer.after` arms, proper `Closed` error
   (currently a string), native support (Track 4).
3. Deadlock prevention checks (conc.sync/DL1–DL4) and `staged()` (ST1–ST4, #292).
4. Runtime-slot / io-context model (CTX1–CTX4): spawn-outside-runtime should be a compile error
   (CC1/CC2), not just the runtime panic it is now.
5. Phase B: `green.c` (798 lines, full scheduler) exists but **isn't in the Makefile**, while codegen
   emits `rask_green_spawn` calls for yielding bodies — that symbol can't link. Either wire green.c in
   behind the decided fiber design or stop emitting the call. **(file the link hole now)**
6. `join` on a panicked task: `JoinError.Panicked`, not re-panic (#288); drain semantics for detached
   tasks at `using` exit (C4).

## Track 6 — Language completeness (post-validation)

Spec'd, absent, and not blocking the validation programs — schedule after Tracks 1–3:

- **Sequence protocol** (type.sequence SEQ1–SEQ24): user types can't be iterated at all; interp ships
  a pull-iterator with `zip`, which the spec forbids. This is an architectural replacement, not a patch.
  Also fixes the t25 fold-inference failure class.
- **Ranges:** `.rev()` and `.step()` don't exist (ctrl.ranges RV1–RV2, SP1–SP4); inclusive-range-at-max
  overflow handling (OV3).
- **`Owned<T>` linearity** (mem.owned OW1–OW4): `own` is a parse-time no-op; no consume-exactly-once,
  no drop path. Spec'd as Phase 4, but boxes.md/linear.md reference it as working — align the specs'
  cross-references or build it.
- **SIMD** (type.simd): stub end to end; only 6 hardcoded type names, no vector instructions. Needs its
  own plan when prioritized.
- **Wrapping/Saturating + `checked_*`/`wrapping_*` methods** (type.overflow W1–W5, M1) — pairs with 1.2.
- **Inline `asm`** — reserved token only.
- **Loop constraint rules** (ctrl.loops LP4, LP8, LP14) and custom-sequence for-loop desugar (LP18–LP23,
  lands with the Sequence protocol).
- **Determinism contract / sim mode** (determinism/D1–D14, proposed): `rask test --sim` — seeded
  scheduler, virtual clock, fault injection. The only always-on semantic piece is D7 (seeded Map hash
  order), which is in Track 4 now; the rest waits on the Phase B runtime it virtualizes.

## Track 7 — Tooling, DX, performance

Real but not blocking language correctness:

- Analyses don't run in `check`/LSP — typestate/stale-handle detection (the headline safety feature)
  only fires at build time (`rask-compiler/src/lib.rs` check path stops after ownership+effects).
- IDE ghost text: all of it is missing (effects `[io]`/`[pure]`, `[clone elided]`, `[rc elided]`,
  `[coalesced]`) — `inlay_hints.rs` emits only types. Effects engine already runs; surfacing is the gap.
- Purity lint P1–P3 (`@pure` teeth) and frozen-suggestion lint FL1–FL4 — missing from rask-lint;
  canonical-patterns says lint enforces its naming rules (now incl. the name-provenance check) — audit
  rask-lint against that list.
- Incremental compilation: `rask-semantic-hash` crate is built but wired to nothing; cache is a coarse
  per-package content hash. ROADMAP Phase 4 stands; MIR serde derives are the first prerequisite.
- `compile_rust()` (struct.build PM10) — only `compile_c` exists.
- `--emit-header` C export (EX3); DWARF emits `DW_LANG_C99` (debuggers see C).
- Dead code to reconcile: duplicate `rask-hidden-params` crate (the wired copy lives in
  `rask-mir/src/hidden_params/`).

---

## Test strategy (cross-cutting)

- `rask test-specs` proves snippets parse; it doesn't prove semantics. Add per-rule conformance tests
  in `tests/suite/` (positive) and `tests/compile_errors/` (negative) as each track lands, tagged with
  the rule ID they witness (`ctrl.panic/E2` etc.).
- Every suite test runs check + interp + native; divergence is a failure even when both "pass".
- The five validation programs become CI gates at `rask compile` level, not just `check`.

## Issue hygiene

Existing issues cover ~half the plan (refs inline above). Items marked **(file)** need issues before
work starts: ownership branch-merge unsoundness (generalizing #294), overflow semantics, `as`-cast
holes + missing conversions, linear-in-containers, cross-task ownership, orphaned stdlib stubs,
cancellation stubs, the green.c link hole, and the t25 fold-inference bug.
