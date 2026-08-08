# Rask Roadmap

Strategic phases. Open work items are in [TODO.md](TODO.md); bugs are [GitHub issues](https://github.com/rask-lang/rask/issues). For the current spec-vs-compiler gap and its work order, see [PLAN.md](PLAN.md).

## Where things stand

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 73 decided specs.

Simple programs compile natively (hello world, structs, closures, Vec/Map, threads, channels, file I/O). Two of the five validation programs run on both backends; the rest are down to one named bug each, not a general regression.

## Validation programs

Re-measured 2026-08-08 by running all five. Every blocker below is a live
symptom with an issue; the previous table had drifted badly — it blamed
`Pool.insert returns Result` (it returns a bare `Handle<T>`), a string-slice
error in grep clone (which works), and type mismatches in the text editor
(which type-checks).

| Program | Status | Blocker |
|---------|--------|---------|
| Sensor processor | **Works** | — enrolled in the gate with a golden |
| grep clone | **Works** | not gated: the gate can't pass argv (#658) |
| Game loop with entities | Native only | native rejects handles from `for h in pool` (#652) |
| Text editor with undo | Hangs | spins forever at EOF instead of quitting (#659) |
| HTTP JSON API server | Blocked | needs `json.encode`/`decode` (Phase 1) |

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

- HTTP/1.1 request parser in Rask (method, path, headers, body)
- HTTP response serialization in Rask (status line, headers, body)
- JSON parser rewrite in Rask (current C version only handles flat objects)
- Validate `http_api_server.rk` compiles and runs natively

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
