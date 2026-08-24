# Rask Roadmap

Strategic phases. Open work items are in [TODO.md](TODO.md); bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). For the current spec-vs-compiler gap and its work order, see [PLAN.md](PLAN.md).

## Where things stand

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 73 decided specs.

Simple programs compile natively (hello world, structs, closures, Vec/Map, threads, channels, file I/O). All five validation programs run on both backends. What is left is a registered backlog — 19 tracked bugs and 7 unbuilt features, each with a probe file in the suite. See [PLAN.md](PLAN.md) for the work order.

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

## Phase 1: Stdlib in Rask + HTTP validation

The real test of multi-file compilation, stdlib imports, and native codegen on real code.

- [x] HTTP/1.1 request parser in Rask (method, path, headers, body) — `stdlib/http.rk`, 816 lines, nothing stubbed
- [x] HTTP response serialization in Rask (status line, headers, body)
- [x] JSON parser rewrite in Rask — `stdlib/json.rk`, 689 lines
- [x] Validate `http_api_server.rk` compiles and runs natively — serves on both backends under `tests/http_api_harness.sh`
- [ ] `json.to_value` / `json.from_value` are still `@unimplemented`; the tree↔typed bridge waits on Encode/Decode derivation

## Phase 2: Stdlib breadth

| Module | Language | Purpose |
|--------|----------|---------|
| url | Rask | URL parsing (RFC 3986) |
| encoding | Rask | Base64, hex, URL encoding (RFC 4648) |
| csv | Rask | CSV parsing/writing (RFC 4180) |
| unicode | Rask | Properties, normalization, categories |
| terminal | Rask | ANSI colors, terminal detection |
| hash | Rask (or C for HW accel) | SHA-256, MD5, CRC32 |
| tls | C shim + Rask API | TLS/SSL via OpenSSL/mbedTLS |

Each module needs: spec, implementation, tests.

## Phase 3: Runtime & codegen maturity

- Runtime trait dispatch — `any Trait` for heterogeneous collections (#194)
- Cross-compilation — `--target` flag wired to Cranelift + cross-linker detection (XT1–XT6)

## Phase 4: Incremental compilation

The IR design for function-level granularity can't be retrofitted. Spec: [incremental.md](specs/compiler/incremental.md).

- Semantic hashing — hash computation, Merkle tree, cache keys
- Function identity — `MonoFunctionKey` in monomorphization output
- MIR serialization — `serde` derives on MIR types
- Per-function object caching (Phase 1)
- Fast relink with `mold`/`lld`
- In-place binary patching — function slots + GOT + ELF patcher (Phase 2, when relink becomes bottleneck)

---

## Post-v1.0

- State machine codegen — stackless transforms for green tasks
- Platform-specific deps (XT7), multi-target builds (XT8), `rask targets` (XT9)
- LLVM backend
- Macros / `format!`
- Comptime debugger
- Fuzzing / property-based testing
- Code coverage
- `std.reflect` — comptime reflection
- Inline assembly
- Pointer provenance rules
- `compile_cpp()` build script support
- Auto Rask wrapper generation from cbindgen
