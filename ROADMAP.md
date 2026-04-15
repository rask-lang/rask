# Rask Roadmap

Strategic phases. Tactical open items are in [TODO.md](TODO.md); runtime bugs in [KNOWN_BUGS.md](KNOWN_BUGS.md).

## Where things stand

Frontend, ownership, interpreter, monomorphization, MIR lowering, Cranelift backend, build system, package management — all working. 73 decided specs.

Everything in the core language compiles natively today. The HTTP server is the one validation program that doesn't — blocked on the stdlib rewrites in Phase 1.

## Validation programs

| Program | Native | Notes |
|---------|--------|-------|
| grep clone | Yes | CLI args + file I/O |
| Text editor with undo | Yes | Pool + Handle + ensure |
| Game loop with entities | Yes | Pool, handles, threading |
| Sensor processor | Yes | Threads, timing, shared Vec |
| HTTP JSON API server | No | Needs HTTP + JSON stdlib in Rask |

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

- Runtime trait dispatch — `any Trait` for heterogeneous collections
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
