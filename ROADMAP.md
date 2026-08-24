# Rask Roadmap

Strategic phases. Open work items are in [TODO.md](TODO.md); bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). For the current spec-vs-compiler gap and its work order, see [PLAN.md](PLAN.md).

## Where things stand

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 73 decided specs.

Simple programs compile natively (hello world, structs, closures, Vec/Map, threads, channels, file I/O). All five validation programs run on both backends. What is left is a registered backlog — 14 tracked bugs and 7 unbuilt features, each with a probe file in the suite. See [PLAN.md](PLAN.md) for the work order.

## Validation programs

Re-measured 2026-08-24 by running all five. All pass. The four that print and
exit are enrolled in `tests/examples_gate.sh` with goldens diffed on both
backends; the server never exits, so it has its own harness driving a CRUD
request sequence.

| Program | Status | Gate |
|---------|--------|------|
| Sensor processor | **Works** | examples gate, golden |
| grep clone | **Works** | examples gate, golden + argv |
| Game loop with entities | **Works** | examples gate, golden (seeded RNG) |
| Text editor with undo | **Works** | examples gate, golden + stdin |
| HTTP JSON API server | **Works** | `tests/http_api_harness.sh`, both backends |

This milestone is met, so it is no longer what to steer by. The next instrument
is the one NORTH_STAR names: models writing Rask against the compiler, with the
failure transcripts read.

## Stdlib architecture

| Layer | Language | What lives here |
|-------|----------|-----------------|
| **Runtime** | C | OS interface, memory primitives, data structures, concurrency, raw I/O |
| **Stdlib** | Rask | Everything above the OS — HTTP, JSON, CSV, URL, base64, hashing, unicode, terminal |

Dogfooding validates the language. Rask code gets ownership and bounds checking that C doesn't. If the language can't handle an HTTP parser, something's wrong.

C stays for things that must talk to the OS (syscalls, io_uring) or wrap existing C libraries (TLS via OpenSSL/mbedTLS, hardware crypto).

---

## What comes next, and why in this order

The old phase list was organized around getting the validation programs to run. They
all run. So the ordering below is organized around the thing that replaces it: knowing
when the compiler is *done enough*, rather than knowing which five programs work.

### The measure comes first

Green gates are not the same as a correct compiler. On 2026-08-24 every gate passed
while this returned `2`:

```rask
let a = match n { 1 => 2.5, _ => 0.0 }
```

`t_week_enums.rk` was 13/13 throughout, because none of its variants happened to carry
a float. That is the shape of every bug found this year: not a deep design fault, but a
missing case — one arm of a match, one name absent from a list, one site that answered
a question locally instead of asking. They are found by *running programs*, never by
reading the compiler.

So the first work isn't a feature:

1. **Finish the coverage backlog.** The registered red files in
   `tests/known_divergences.txt`, each with a probe and an issue.
2. **Close the coverage holes**, not just the red files — the "holes left on purpose"
   list in `tests/COVERAGE.md` is the real todo. An area file gates the shapes it
   happens to use, and nothing else.
3. **Build the agent benchmark.** [NORTH_STAR](NORTH_STAR.md) names it as the
   instrument — models writing Rask against the compiler, convergence measured, the
   failure transcripts read — and it does not exist. Without it, "the native compiler
   is stable" has no exit criterion and stays a feeling.

### Then a program big enough to hurt

The five validation programs are single-file, mostly synchronous, and short-lived. What
they don't stress is what breaks next: long-running state, a real dependency graph,
concurrency under load.

The larger program is the *instrument*, not the reward. Dogfooding a piece of the
toolchain in Rask — `rask fmt`, or the linter — is the honest version, because you feel
every rough edge yourself and it exercises multi-package builds, string handling and
error paths harder than any example does.

Before that, the declared-but-unbuilt stdlib surface has to close: `Vec`/`Map` methods
that exist in the signature and not in the implementation (#912), and ranges with no
terminals or adapters (#920). A declared method that doesn't exist is worse than a
missing one — the signature promises and the call fails. A larger program meets those
in its first hour.

### Then the three that block shipping

1. **Panics and unwinding** ([#299](https://github.com/rask-lang/rask/issues/299)). The
   panic path runs no `ensure` blocks and aborts the process. A server cannot ship with
   that, and it undercuts the resource-safety promise everywhere else.
2. **The sequence protocol** (`type.sequence`). User types can't be iterated at all, and
   range adapters route through it, so it gates more than it looks like it does.
3. **Incremental compilation.** NORTH_STAR's first commitment is maximum static checking
   per millisecond of feedback. This is the item that serves it directly. The IR design
   for function-level granularity can't be retrofitted — spec in
   [incremental.md](specs/compiler/incremental.md).

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

Each needs spec, implementation, tests. `json.to_value` / `json.from_value` are still
`@unimplemented` — the tree↔typed bridge waits on Encode/Decode derivation.

Also runtime trait dispatch for heterogeneous collections
([#194](https://github.com/rask-lang/rask/issues/194)), and cross-compilation: the
`--target` flag wired to Cranelift plus cross-linker detection (XT1–XT6). Cranelift
already does ARM and WASM; the compiler simply doesn't configure them.

## On the LLVM backend

I'm deferring it, and the reason is the bug history rather than the engineering.

The largest single class of bugs in this project is the two backends disagreeing —
measured at 39% of open issues when [#724](https://github.com/rask-lang/rask/issues/724)
was written, and the differential harness exists because of it. A third thing that can
produce an answer is a third thing that can disagree, and the second one still has
tracked divergences.

The usual argument for LLVM is more targets. That one is weak here: Cranelift reaches
ARM and WASM already. The real argument is generated-code quality for a language meant
to compete with Rust and C — and that is a decision for a benchmark to make, not taste.
`benchmarks/` is nearly empty. **Nothing goes to LLVM until something measures slow.**

## Post-v1.0

- State machine codegen — stackless transforms for green tasks
- Platform-specific deps (XT7), multi-target builds (XT8), `rask targets` (XT9)
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
