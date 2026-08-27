# Rask Roadmap

Strategic phases. Open work items are in [TODO.md](TODO.md); bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). For the current spec-vs-compiler gap and its work order, see [PLAN.md](PLAN.md).

## Where things stand

**Measured 2026-08-27 on `99091447`**, by running the gates rather than by reading the last version of this file.

```
differential      343 green, 15 expected-red, 0 untracked, 0 misfiled
examples          34 ok          projects      21 ok
prototypes        13 agree       fmt           441 round-tripped, 0 failures
http api harness  ok both backends
agentbench        17 green, 1 quarantined
cargo test        0 failures
```

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 133 spec files.

What's left is a registered backlog: **9 tracked bugs and 6 unbuilt features**, each with a probe file and an issue. A red file now has to keep failing on the backend *and* at the phase its registry line claims, so a probe that quietly stops testing its bug is reported instead of counting as still-red.

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

The previous ordering opened with three things: finish the coverage backlog, close the coverage holes, and build the agent benchmark. The third is done and the first two shrank, so the ordering below is what's actually left.

### The benchmark exists. Now read what it says.

[NORTH_STAR](NORTH_STAR.md) names the instrument — models writing Rask against the compiler, convergence measured, the failure transcripts read. `agentbench/` is built and its gate runs in CI on every push.

**Building it is not the same as using it.** Nobody has run it against real models and read the transcripts. That is the next thing, and it is cheap: the harness is done, the tasks are written, the reference solutions are green.

It has already earned its keep once without being run. Integrating it surfaced that it hands a model `LANGUAGE_GUIDE.md` as normative and scores whether the reply compiles — and the guide never said stdlib names need importing. A low solve rate would have read as a language-usability number rather than our own stale documentation. Expect more of that: the benchmark measures the compiler *and everything we tell a model about it*, and the second half has never been audited.

### The sequence protocol is the pivot

`type.sequence` is one piece of work that unblocks three things:

- **User types can't be iterated at all.** That is the feature itself.
- **Range adapters** (#920) route through it. A range has no terminals and no adapters today.
- **The larger program** waits on both. A declared method that doesn't exist is worse than a missing one — the signature promises and the call fails — and a real program meets that in its first hour.

Alongside it, #912: eleven `Vec`/`Map` methods that exist in the signature and not in the implementation. Same reasoning, no shared machinery.

Nothing else in the backlog has this shape. It is the highest-leverage item left.

### Then a program big enough to hurt

The five validation programs are single-file, mostly synchronous, and short-lived. What they don't stress is what breaks next: long-running state, a real dependency graph, concurrency under load.

The larger program is the *instrument*, not the reward. Dogfooding a piece of the toolchain in Rask — `rask fmt`, or the linter — is the honest version, because you feel every rough edge yourself and it exercises multi-package builds, string handling and error paths harder than any example does.

### Then incremental compilation

NORTH_STAR's first commitment is maximum static checking per millisecond of feedback. This is the item that serves it directly, and the IR design for function-level granularity can't be retrofitted — spec in [incremental.md](specs/compiler/incremental.md).

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
