# Rask Roadmap

Strategic phases. Open work items are in [TODO.md](TODO.md); bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). For the current spec-vs-compiler gap and its work order, see [PLAN.md](PLAN.md).

## Where things stand

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 91 decided specs (counted directly this measure; unchanged from the 90 claimed last time — no spec file moved since, so that was off by one, not a real gain).

Simple programs compile natively (hello world, structs, closures, Vec/Map, threads, channels, file I/O). The registered backlog is down to **three** unbuilt-feature probes — `p09_simd.rk`, `p10_binary.rk`, `p11_gradual_generalization.rk` — and two permanent/near-permanent entries in `known_divergences.txt` (an interpreter laziness bug and a documented native-only asymmetry, neither a live bug). See [PLAN.md](PLAN.md) for the work order.

**The big move since last measure: the sequence-protocol cluster, which this file called "the leading feature gap," is done.** #920 (range adapters), #912 (eleven Vec/Map methods), #927 (Atomic), plus the two blockers under them (#1058 slices-vs-Vec\<u8\>, #1141 gradual-generalization design) all closed. Verified directly, not from issue status — see §1 below. All of it landed *before* this file's last re-measure (2026-09-11) and got missed then; this is the third time a re-measure has shipped a stale "still open" claim, which is why every number below was re-run this session rather than copied forward.

## Validation programs

Re-measured 2026-09-14 by running all five. All five work on native — the HTTP server since #1036 was fixed on 2026-09-07.

| Program | Status | Gate |
|---------|--------|------|
| Sensor processor | **Works** | examples gate, golden |
| grep clone | **Works** | examples gate, golden + argv |
| Game loop with entities | **Works** | examples gate, golden (seeded RNG) |
| Text editor with undo | **Works** | examples gate, golden + stdin |
| HTTP JSON API server | **Works** | `tests/http_api_harness.sh`, both backends |

The HTTP server was red from 2026-08-31 to 2026-09-07: every response's first
eight bytes came back as garbage instead of `HTTP/1.1 `, because a `string`
handed across an `unsafe` FFI boundary got a temporary built for it that was
never fully written. Not HTTP-specific — it hit anything native writing a built
string to a raw fd, which happens to be the whole send path. Fixed in
[#1036](https://github.com/rask-lang/rask/issues/1036).

The lesson it shares with the `match n { 1 => 2.5, _ => 0.0 }` bug is worth
keeping: every other gate was green for a week while the flagship example was
silently broken. A gate nobody runs on the thing users actually see is not a
gate. `tests/http_api_harness.sh` now runs both backends.

## Stdlib architecture

| Layer | Language | What lives here |
|-------|----------|-----------------|
| **Runtime** | C | OS interface, memory primitives, data structures, concurrency, raw I/O |
| **Stdlib** | Rask | Everything above the OS — HTTP, JSON, CSV, URL, base64, hashing, unicode, terminal |

Dogfooding validates the language. Rask code gets ownership and bounds checking that C doesn't. If the language can't handle an HTTP parser, something's wrong.

C stays for things that must talk to the OS (syscalls, io_uring) or wrap existing C libraries (TLS via OpenSSL/mbedTLS, hardware crypto).

---

## What comes next, and why in this order

### 1. The sequence protocol cluster — closed, verified directly

Last measure's #1 item. All of it is done: `stdlib/sequence.rk` exists, and
every dependent gap is fixed.

- Ranges have their adapters — `t_week_range_adapters.rk`, 8/8 passing on
  native ([#920](https://github.com/rask-lang/rask/issues/920), closed).
- The eleven Vec/Map methods (capacity control, `get_clone`, `remove_where`)
  are implemented — `t_week_collection_stubs.rk`, 9/9
  ([#912](https://github.com/rask-lang/rask/issues/912), closed).
- `Atomic<T>` has its operations, and the forbidden `AtomicU64`-style names are
  gone from the registry — `t_month_atomics.rk`, 8/8
  ([#927](https://github.com/rask-lang/rask/issues/927), closed).
- The protocol itself: `p08_sequence.rk` is 6/6 on native. The interpreter gets
  5/6 — one lazy-chain `mutate` writeback bug, tracked in
  `known_divergences.txt` against [#1046](https://github.com/rask-lang/rask/issues/1046),
  not a roadmap item.

I ran all four suite files myself this session rather than trust the issue
tracker — they're green. What's left in this area is small and already listed
below (`p10_binary.rk`, `p09_simd.rk`, `p11_gradual_generalization.rk`).

### 2. An unanswered critical-severity question: panic during a `mutate` hand-back

[#882](https://github.com/rask-lang/rask/issues/882) is the linearity-enforcement
audit — six holes where "a resource is consumed exactly once" wasn't actually
checked, five fixed. The sixth is still an open question, not a bug someone's
reproduced: a `mutate` parameter works by taking the value out of the slot,
letting the callee edit it, and writing it back. If the callee **panics**
between those two steps, the slot holds nothing, and native unwind has to
reclaim that frame's fields without ever running a destructor (that's the
language's design — no hidden runtime state). Nobody's said what happens to a
half-emptied slot on that path: freed twice, leaked, or actually already fine.
The issue has sat since 2026-08-19 with one follow-up comment and no repro
either way.

This is the only priority:critical item in the tracker with a live, unresolved
question behind it — worth someone actually writing the reduction and checking,
rather than leaving it as a suspicion. Everything else below is scoped and
either in progress or deliberately deferred.

### 3. Finish the coverage backlog

`tests/known_divergences.txt` carries two lines, neither a live bug: the
sequence-protocol interpreter lag above, and `t_raw_pointer_width.rk`, a
permanent native-only asymmetry (`extern "C"` needs a C ABI a tree-walker
doesn't have) that isn't going to close.

`tests/pending_features.txt` has three unbuilt-feature probes left:

- **`p10_binary.rk`** — closer than last measure. #1058 (the blocking design
  question — can a `Vec<u8>` satisfy a declared `[]u8` parameter) is resolved:
  the answer is no slice type, `@binary`'s generated methods take `Vec<u8>`.
  The interpreter runs the whole probe now. Native still generates neither
  `build` nor `parse` — that's a codegen task, not a design one, so this
  should be quick to finish and is worth doing next given the blocker is gone.
- **`p09_simd.rk`** — unchanged, low priority. Surveyed in
  [#1059](https://github.com/rask-lang/rask/issues/1059): the checker's half is
  built (`import math.f32x4` type-checks); nothing below it is. A SIMD literal
  lowers to `[f64; 4]` and the binding is a `ptr` nothing writes to.
  `splat[T, N](…)` doesn't parse — square-bracket type application isn't in
  the grammar, and the spec's own `Vec[T, N]` form needs it too.
- **`p11_gradual_generalization.rk`** — the design question is decided
  ([#1141](https://github.com/rask-lang/rask/issues/1141): a bare method call
  like `.len()` pins the parameter's type rather than inferring a structural
  bound — GC13). Not implemented: the structural bound still needs the
  requirement collected in the checker instead of desugar, which is real work
  with no urgency behind it.

### 4. Incremental compilation

NORTH_STAR's first commitment is maximum static checking per millisecond of
feedback. Unchanged since last measure: the function-granularity design
(spec: [incremental.md](specs/compiler/incremental.md)) has no implementation
yet — semantic hashing is done, the LSP has its own editor-facing incremental
checking, but `rask build` itself doesn't cache or patch at function
granularity. The IR design can't be retrofitted, so this has to be deliberate
when it's picked up.

### 5. Panics — nearly done, one small tracker left

This used to be the headline blocker ("the panic path runs no `ensure` blocks
and aborts the process"). That's fixed:

```
$ rask run panic_test.rk
panic at panic_test.rk:10: boom
closing g1
exit: 101
```

Verified directly this pass — `ensure` runs on panic, native exits 101 instead
of aborting. 10 of [#299](https://github.com/rask-lang/rask/issues/299)'s 11
sub-issues are closed. What's left is
[#298](https://github.com/rask-lang/rask/issues/298) — genuinely small,
runtime-surface items, not a redo: a detached task's panic should print to
stderr (currently prints nothing), a guard that panics during unwind should be
contained and reported as a secondary panic instead of replacing the original,
the task id should prefix the panic line when a runtime is active, and a panic
that reaches an FFI boundary should abort there instead of unwinding into
foreign frames.

### 6. Cross-compilation — partly built already, don't re-derive it

Unchanged since it was corrected two measures ago: the roadmap used to say "the
compiler simply doesn't configure" ARM/WASM targets. Wrong — `--target` reaches
Cranelift's ISA lookup today, and `rask targets` lists all three tiers. Tried it
directly, again, same result:

```
$ rask compile examples/http_api_server.rk --target aarch64-linux -o out
error: link: cross-compilation to aarch64-linux requires a C cross-compiler
Install one of: zig (recommended), aarch64-linux-gnu-gcc, or set CC=...
```

That's the compiler working correctly and reporting what's missing (this is
literally what spec rule XT3 asks for), not a gap. What's actually missing,
per `specs/structure/build.md`'s own status table: the wider toolchain — cross
compiler detection, platform-specific deps, multi-target builds (XT1–XT8,
listed "Not started"). Also worth knowing: the runtime is a static C library
linked into every binary, so "pure Rask needs only the compiler to
cross-compile" (XT2) doesn't hold yet even for programs with no `unsafe` in
them — the C runtime always needs a matching cross-linker. Couldn't verify the
zig/gcc path end-to-end — neither is installed in this environment.

## On the LLVM backend

Deferring it, and the reason is the bug history rather than the engineering.

The largest single class of bugs in this project is the two backends disagreeing —
measured at 39% of open issues when [#724](https://github.com/rask-lang/rask/issues/724)
was written, and the differential harness exists because of it. A third thing that can
produce an answer is a third thing that can disagree, and the second one still has
tracked divergences.

The usual argument for LLVM is more targets. That one is weak here: Cranelift reaches
ARM and WASM already. The real argument is generated-code quality for a language meant
to compete with Rust and C — and that is a decision for a benchmark to make, not taste.
`benchmarks/` now has one apples-to-apples pair (`grep.c` vs `examples/grep_clone.rk` —
ceremony came out a tie, ED 0.96) but nothing measuring raw speed yet.
**Nothing goes to LLVM until something measures slow.**

## The agent benchmark

`agentbench/` — 19 tasks, reference solutions, model adapters (`mock:*`, `cli:<model>`
against a Claude subscription, `api:<model>`), measuring solve rate, pass@1,
convergence, backend divergences, thrash, and teach rate against the targets in
its README. CI runs `agentbench_gate.sh` (the free `selftest` — do the references
still build), which passed this measure: 18 green, 1 quarantined.

That one quarantined task's own citation is stale, corrected here: `quarantine.txt`
still names [#1002](https://github.com/rask-lang/rask/issues/1002) (native
mis-dispatching a method on a union-narrowed error), which is fixed — I
hand-combined the task's reference and checks and ran it natively; all 5 pass.
The task is still red for an unrelated reason, already filed as
[#1133](https://github.com/rask-lang/rask/issues/1133): `verify.rk` has a stray
`import string.ParseError` that collides with the task's own `ParseError` enum
(E0208, correctly). One line to fix (delete the import, re-point
`quarantine.txt` at #1133 or drop the entry entirely), not done here since it's
a fixture bug rather than a doc.

The one real-model run on record (2026-08-28): pass@1 61%→72%,
convergence 1.47→1.29, after the language card got a "method surface" section —
method-not-found was the top first-attempt failure. Running it against a live
model isn't automated (deliberately — it spends plan quota or API credit), so
that number will go stale between measures; re-run it by hand when a stdlib or
diagnostics change is large enough to matter.

## Leak tracking (built 2026-08-29, never described here before)

`tests/leak_gate.sh` runs every suite file under `RASK_LEAK_CHECK=1` and fails
on a file that leaks without a line in `tests/known_leaks.txt` — same shape as
`known_divergences.txt`, but for allocations instead of wrong answers. It's
been in CI for two weeks; this file just never said so.

This measure: **471 clean, 31 known-leaking, 0 new.** The 31 are registered
against real issues, mostly closures capturing containers and containers nested
in containers. The biggest remaining shape, per `known_leaks.txt`'s own notes:
a container captured by a closure, or nested inside another container, is never
freed ([#1035](https://github.com/rask-lang/rask/issues/1035)). This is steady
grinding work, not a planning decision — every recent session has chipped a few
more files off the list, and the gate keeps it from regressing silently.

## Stdlib breadth, alongside

| Module | Language | Purpose |
|--------|----------|---------|
| url | Rask | URL parsing (RFC 3986) |
| encoding | Rask | Base64, hex, URL encoding (RFC 4648) |
| csv | Rask | CSV parsing/writing (RFC 4180) |
| unicode | Rask | Properties, normalization, categories |
| terminal | Rask | ANSI colors, terminal detection |
| hash | Rask (or C for HW accel) | SHA-256, MD5, CRC32 |
| tls | C shim + Rask API | TLS/SSL via OpenSSL/mbedTLS |

All seven now have specs (landed this week). Implementation and tests still
open per module. `json.to_value` / `json.from_value` are still `@unimplemented` —
the tree↔typed bridge waits on Encode/Decode derivation.

## Post-v1.0

- Platform-specific deps (XT7), multi-target builds (XT8), `rask targets` polish (XT9 itself already ships)
- LLVM backend, if the benchmarks ask for it
- Macros / `format!`
- Comptime debugger
- Fuzzing / property-based testing
- Code coverage
- `std.reflect` — comptime reflection
- Inline assembly
- Pointer provenance rules
- `compile_cpp()` build script support
- Auto Rask wrapper generation from cbindgen

## What came off this list since last measure (2026-09-11 → 2026-09-14)

- **The sequence-protocol cluster — this file's #1 item — is done.** #920, #912,
  #927, plus their two blockers #1058 and #1141, all closed. Verified by running
  the four suite files directly rather than trusting issue state (§1 above). All
  five closed *before* 2026-09-11, meaning the last re-measure already had stale
  data and reported it anyway — third time that's happened, which is why this
  pass re-ran everything instead of diffing issue titles.
- **The agentbench quarantine's cited reason was stale.** #1002 (the bug it
  named) is fixed; the task stays red for a different, already-filed reason
  (#1133, a stray import). Corrected the citation rather than leaving a fixed
  issue number pointing at a currently-red task.
- Most of the 62 commits since last measure were the front page, book, and
  playground rewrite (a genuine push to make the site readable, not a compiler
  change) plus a large leak/closure-capture bug crunch — see the leak-tracking
  section above for that half.

## New this measure (2026-09-14)

- **Full gate re-run, everything green:** differential 500 green / 5 expected-red
  (unchanged categories), examples 35/35, projects 21/21, prototypes 13/13, fmt
  round-trip and `--check` clean, HTTP harness ok on both backends, `cargo test
  --release` all green, agentbench selftest 18 green / 1 quarantined (see above).
- **Found and described `tests/leak_gate.sh`/`known_leaks.txt` for the first
  time in this file** — built 2026-08-29, been in CI two weeks, nobody had added
  a section for it. See "Leak tracking" above.
- **Flagged [#882](https://github.com/rask-lang/rask/issues/882)'s open design
  question** — a panic during a `mutate` parameter's take-and-replace, and
  whether native unwind double-frees or leaks the half-emptied slot. Priority
  critical, open since 2026-08-19, no repro either way yet. Judgment call: worth
  someone reducing and checking rather than leaving as a suspicion.
