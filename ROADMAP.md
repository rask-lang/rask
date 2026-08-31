# Rask Roadmap

Strategic phases. Open work items are in [TODO.md](TODO.md); bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). For the current spec-vs-compiler gap and its work order, see [PLAN.md](PLAN.md).

## Where things stand

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 90 decided specs (up from 73 a week ago — a batch of stdlib specs landed: url, base64, hex, csv, terminal, digest, tls).

Simple programs compile natively (hello world, structs, closures, Vec/Map, threads, channels, file I/O). What's left is a registered backlog — 11 tracked bugs and 6 unbuilt features, each with a probe file in the suite (down from 13+7 at the last measure). See [PLAN.md](PLAN.md) for the work order.

## Validation programs

Re-measured 2026-08-31 by running all five.

| Program | Status | Gate |
|---------|--------|------|
| Sensor processor | **Works** | examples gate, golden |
| grep clone | **Works** | examples gate, golden + argv |
| Game loop with entities | **Works** | examples gate, golden (seeded RNG) |
| Text editor with undo | **Works** | examples gate, golden + stdin |
| HTTP JSON API server | **Broken on native** | `tests/http_api_harness.sh` fails; interp is fine |

The HTTP server is a new regression, found by this re-measure: every response's
first 8 bytes come back as garbage instead of `HTTP/1.1 `. Traced to a minimal
repro — `StringBuilder` plus one `unsafe` call to a native function taking a
`string` argument — so it's not HTTP-specific: **anything native that hands a
built string across an `unsafe` FFI boundary is corrupting its first 8 bytes
right now.** Filed as [#1036](https://github.com/rask-lang/rask/issues/1036)
with the repro. This is the same lesson as the `match n { 1 => 2.5, _ => 0.0 }`
bug from last month: every other gate was green while the flagship example
silently broke. Fix this first — it's the widest blast radius of anything on
this list, and it undoes "the five validation programs work."

## Stdlib architecture

| Layer | Language | What lives here |
|-------|----------|-----------------|
| **Runtime** | C | OS interface, memory primitives, data structures, concurrency, raw I/O |
| **Stdlib** | Rask | Everything above the OS — HTTP, JSON, CSV, URL, base64, hashing, unicode, terminal |

Dogfooding validates the language. Rask code gets ownership and bounds checking that C doesn't. If the language can't handle an HTTP parser, something's wrong.

C stays for things that must talk to the OS (syscalls, io_uring) or wrap existing C libraries (TLS via OpenSSL/mbedTLS, hardware crypto).

---

## What comes next, and why in this order

### 1. Fix the native string→FFI corruption (#1036)

Leads the list because of blast radius, not because it's hard to characterize.
Every native program that writes a built string to a raw fd through `unsafe`
gets its first 8 bytes clobbered — that's the actual send path for the whole
HTTP server (`write_raw` in `stdlib/http.rk`), so every native HTTP response is
wrong today. Repro is 12 lines, no networking needed. Whoever picks this up:
start at how `string as i64` gets codegen'd for an argument headed into an
`unsafe` block.

### 2. The sequence protocol — now the leading feature gap

Panics used to be here (see "what came off this list" below); with that mostly
done, this is the item that unblocks the most other things. `type.sequence` is
unimplemented (`p08_sequence.rk`), and three separate gaps chain off it:

- Ranges have no methods beyond `for` — no `.sum()`, `.map()`, `.to_vec()` — because
  they're meant to reach adapters through the sequence protocol
  ([#920](https://github.com/rask-lang/rask/issues/920)).
- Eleven declared Vec/Map methods (capacity control, `get_clone`, `remove_where`)
  are unimplemented on both backends
  ([#912](https://github.com/rask-lang/rask/issues/912)).
- `Atomic` — the one atomic-type spelling the spec mandates — has zero operations,
  while the eleven `AtomicU64`-style names the spec forbids are still registered
  ([#927](https://github.com/rask-lang/rask/issues/927)).

One protocol landing turns three "unbuilt" rows into "done" rows, which is why
it leads over finishing the smaller registered-bug backlog.

### 3. Finish the coverage backlog

11 registered bugs (`tests/known_divergences.txt`) + 6 unbuilt features
(`tests/pending_features.txt`, 3 of which are the sequence-protocol cluster
above). Each has a probe file and an issue. Spot-checked a sample against
their issues and code this pass — all still accurately described, nothing
found already fixed or already built.

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

Corrected this pass: the roadmap used to say "the compiler simply doesn't
configure" ARM/WASM targets. Wrong — `--target` reaches Cranelift's ISA lookup
today, and `rask targets` lists all three tiers. Tried it directly:

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

## The agent benchmark (built, since last measure it didn't exist)

Last measure said this instrument "does not exist." It does now:
`agentbench/` — 19 tasks, reference solutions, model adapters (`mock:*`, `cli:<model>`
against a Claude subscription, `api:<model>`), and it measures solve rate, pass@1,
convergence, backend divergences, thrash, and teach rate against the targets in
its README. CI runs `agentbench_gate.sh` (the free `selftest` — do the references
still build), which passed this measure: 18 green, 1 quarantined
(`month_error_union`, [#1002](https://github.com/rask-lang/rask/issues/1002),
already tracked). The one real-model run on record (2026-08-28): pass@1 61%→72%,
convergence 1.47→1.29, after the language card got a "method surface" section —
method-not-found was the top first-attempt failure. Running it against a live
model isn't automated (deliberately — it spends plan quota or API credit), so
that number will go stale between measures; re-run it by hand when a stdlib or
diagnostics change is large enough to matter.

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

## What came off this list since last measure (2026-08-24)

- **Panics and unwinding** — was the #1 blocker, now #5 and nearly closed. `ensure`
  runs on panic, native exits 101. Verified directly, not just by issue status.
- **The agent benchmark** — was "doesn't exist," now built, in CI, and has one
  real measured run on record.
- **Cross-compilation was reported worse than it is.** `--target` and
  `rask targets` work; only the wider toolchain (cross-linkers, multi-target)
  is unbuilt. Corrected the framing above rather than re-deriving it.
- **Stackless state-machine transform for spawned tasks** — landed 2026-08-14
  (`rask-mir/src/transform/state_machine.rs`, wired into spawn lowering), was
  still listed under Post-v1.0 as future work. Removed from that list.
- **#194 (trait-object vtable dispatch)** — the roadmap cited this as an open
  gap ("runtime trait dispatch for heterogeneous collections"). It's been
  closed since July 22, fixed by #344. Dropped the reference.
- **Coverage backlog shrank 20 → 17** (13 bugs + 7 unbuilt → 11 bugs + 6 unbuilt).

## New this measure

- **HTTP server broken on native (#1036)** — see Validation programs above. Not
  fixed as part of this pass per the "measure, don't fix" rule for this task,
  but filed with a minimal repro since it's a live regression, not a documentation
  correction.
