# Rask Roadmap

Strategic phases. Open work items are in [TODO.md](TODO.md); bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). For the current spec-vs-compiler gap and its work order, see [PLAN.md](PLAN.md).

## Where things stand

**Measured 2026-08-27 on `2e9bf998`**, by running the gates rather than by reading the last version of this file.

```
differential      344 green, 18 expected-red, 0 untracked, 0 misfiled
examples          34 ok          projects      21 ok
prototypes        13 agree       fmt           441 round-tripped, 0 failures
http api harness  ok both backends
agentbench        18 green, 1 quarantined
cargo test        0 failures
```

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 133 spec files.

What's left is a registered backlog: **12 tracked bugs and 6 unbuilt features**, each with a probe file and an issue. Three of the twelve arrived in the last week from the agent benchmark, which is the point of it. A red file now has to keep failing on the backend *and* at the phase its registry line claims, so a probe that quietly stops testing its bug is reported instead of counting as still-red.

## Validation programs

All five pass. Four are enrolled in `tests/examples_gate.sh` with goldens diffed on both backends; the server never exits, so it has its own harness driving a CRUD sequence.

| Program | Status | Gate |
|---------|--------|------|
| Sensor processor | **Works** | examples gate, golden |
| grep clone | **Works** | examples gate, golden + argv |
| Game loop with entities | **Works** | examples gate, golden (seeded RNG) |
| Text editor with undo | **Works** | examples gate, golden + stdin |
| HTTP JSON API server | **Works** | `tests/http_api_harness.sh`, both backends |

This milestone is met, so it is no longer what to steer by.

## Stdlib architecture

| Layer | Language | What lives here |
|-------|----------|-----------------|
| **Runtime** | C | OS interface, memory primitives, data structures, concurrency, raw I/O |
| **Stdlib** | Rask | Everything above the OS — HTTP, JSON, CSV, URL, base64, hashing, unicode, terminal |

Dogfooding validates the language. Rask code gets ownership and bounds checking that C doesn't. If the language can't handle an HTTP parser, something's wrong.

C stays for things that must talk to the OS (syscalls, io_uring) or wrap existing C libraries (TLS via OpenSSL/mbedTLS, hardware crypto).

---

## What comes next, and why in this order

The previous ordering opened with three things: finish the coverage backlog, close the coverage holes, and build the agent benchmark. The benchmark is built and has now been run — and what it found reorders the rest, so it goes first.

### The benchmark has been run. Here is what it said.

Four runs, 18 tasks. **Five native-only bugs, none of which any of the eight gates caught**, all found by a model writing the obvious thing — a word count, a stack, a CSV parser:

| what a model wrote | what happened |
|---|---|
| `counts[word] = n` | the key's stack address used as a Vec index, then a panic |
| `self.items.remove(i)` in a generic | 8 bytes of a 16-byte string, no refcount |
| `self.items[i].clone()` in a generic | same, truncated string ([#1020](https://github.com/rask-lang/rask/issues/1020)) |
| `["a","b"].join("-")` | empty result, then a segfault ([#1021](https://github.com/rask-lang/rask/issues/1021)) |
| `for (i, line) in text.lines().enumerate()` | native refuses to build a valid program ([#1022](https://github.com/rask-lang/rask/issues/1022)) |

The interpreter was right every single time.

Against the targets: convergence 1.29 (want ≤1.5) and teach rate hold; ASR is 93% against a 95% floor; **backend divergence is 1 in 18 against a target of zero, and has never once been zero.** Four runs, four different disagreements. That is the exit criterion "native is stable" never had, and it is now failing with names and repros attached rather than passing by absence of evidence.

Two things it caught that were not compiler bugs at all. The language card's only sequence example taught `.collect()`, which the compiler had removed — every model copying it burned an attempt. And `Set` is in the prelude and `Set<T>` type-checks with zero methods ([#1017](https://github.com/rask-lang/rask/issues/1017)). Fixing the card moved pass@1 from 50% to 72%. **The benchmark measures the compiler and everything we tell a model about it**, and the second half had never been audited.

The instrument was wrong too: divergences only counted when one backend was green, so "both red, differently" scored as an ordinary test failure. That is how the map bug hid. Fixed — and the fix is what caught #1020.

### First: `T` survives monomorphization

Three of the five findings are one root cause. Inside a monomorphized generic the checker still says `T`, and MIR reads that as `i64` — so a string element comes back as 8 bytes of a 16-byte value with no refcount. The compiler works around it per call site. Two more sites were fixed this round and a third was filed as [#1020](https://github.com/rask-lang/rask/issues/1020) rather than patched, which is the right call: every new generic site is a new instance of the same bug, and the workaround list only grows.

This leads for a reason beyond its own bug count. **The sequence protocol adds generic surface.** Building it on a checker that loses `T` inside monomorphized generics means building more sites that need the workaround. Fix the substrate, then build on it.

### Then the sequence protocol

`type.sequence` is one piece of work that unblocks three things:

- **User types can't be iterated at all.** That is the feature itself.
- **Range adapters** ([#920](https://github.com/rask-lang/rask/issues/920)) route through it. A range has no terminals and no adapters today.
- **The larger program** waits on both. A declared method that doesn't exist is worse than a missing one — the signature promises and the call fails — and a real program meets that in its first hour.

Alongside it, [#912](https://github.com/rask-lang/rask/issues/912): eleven `Vec`/`Map` methods that exist in the signature and not in the implementation. Same reasoning, no shared machinery.

### Then a program big enough to hurt

The five validation programs are single-file, mostly synchronous, and short-lived. What they don't stress is what breaks next: long-running state, a real dependency graph, concurrency under load.

The larger program is the *instrument*, not the reward. Dogfooding a piece of the toolchain in Rask — `rask fmt`, or the linter — is the honest version, because you feel every rough edge yourself and it exercises multi-package builds, string handling and error paths harder than any example does.

### Then incremental compilation

NORTH_STAR's first commitment is maximum static checking per millisecond of feedback. This is the item that serves it directly, and the IR design for function-level granularity can't be retrofitted — spec in [incremental.md](specs/compiler/incremental.md).

### A discipline note: checks that pass for the wrong reason

Three times now a check reported the expected outcome without testing what it was written to test:

- A registered-red file stopped compiling entirely, stayed red, and quietly stopped exercising its bug ([#1005](https://github.com/rask-lang/rask/issues/1005) — the gate holds every red file to a `(backend phase)` claim now).
- A compile-error fixture and a warning-count fixture failed on a missing import rather than on the error and the count they pin.
- The benchmark scored "both backends red, differently" as an ordinary failure rather than a divergence.

Each one looked green. The pattern is worth watching for directly: **a test that still fails is not the same as a test that still tests its bug**, and the same holds for a metric that still reports.

### What came off this list

**Panics and unwinding** ([#299](https://github.com/rask-lang/rask/issues/299)) was the first of three shipping blockers. It is 10 of 11 sub-issues done: locks release on unwind, ensures run on the panic path including multi-statement bodies and `else` handlers, exit 101 on both backends, `staged()` works, panic messages agree across backends. The one open sub-issue is #298, which three separate sessions have now measured as already implemented — it is open on tracker convention, not on remaining work.

**Cross-compilation** was listed as "the compiler simply doesn't configure them". It does now: `--target` reaches Cranelift's ISA lookup, and the linker has real per-target diagnostics. What's missing is the *runtime* per target — the Windows runtime, `wasm-ld`, bare-metal — not compiler configuration. Different problem, and a smaller one.

### Stdlib breadth, alongside

| Module | Language | Purpose |
|--------|----------|---------|
| url | Rask | URL parsing (RFC 3986) |
| encoding | Rask | Base64, hex, URL encoding (RFC 4648) |
| csv | Rask | CSV parsing/writing (RFC 4180) |
| unicode | Rask | Properties, normalization, categories |
| terminal | Rask | ANSI colors, terminal detection |
| hash | Rask (or C for HW accel) | SHA-256, MD5, CRC32 |
| tls | C shim + Rask API | TLS/SSL via OpenSSL/mbedTLS |

Each needs spec, implementation, tests. `json.to_value` / `json.from_value` are still `@unimplemented` — the tree↔typed bridge waits on Encode/Decode derivation.

Also runtime trait dispatch for heterogeneous collections ([#194](https://github.com/rask-lang/rask/issues/194)).

## On the LLVM backend

Still deferred, and the reason is the bug history rather than the engineering.

The largest single class of bugs in this project is the two backends disagreeing — measured at 39% of open issues when [#724](https://github.com/rask-lang/rask/issues/724) was written, and the differential harness exists because of it. A third thing that can produce an answer is a third thing that can disagree, and the second one still has tracked divergences.

The usual argument for LLVM is more targets. That one is weak here: Cranelift reaches ARM and WASM already, and `--target` is wired to it. The real argument is generated-code quality for a language meant to compete with Rust and C — and that is a decision for a benchmark to make, not taste. `benchmarks/` has 26 files across micro, features and ergonomics, and none of them measures against another language. **Nothing goes to LLVM until something measures slow.**

## Post-v1.0

- State machine codegen — stackless transforms for green tasks
- Platform-specific deps (XT7), multi-target builds (XT8), `rask targets` (XT9)
- LLVM backend, if the benchmarks ask for it
- Macros / `format!`
- Comptime debugger
- Fuzzing / property-based testing
- Code coverage
- Inline assembly
- Pointer provenance rules
- `compile_cpp()` build script support
- Auto Rask wrapper generation from cbindgen
