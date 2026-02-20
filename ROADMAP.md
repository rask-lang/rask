# Rask Roadmap

## What's done

1. **Language design** — 73 decided specs across memory, types, control flow, concurrency, build system
2. **Frontend** — lexer, parser, resolver, type checker, ownership checker
3. **Interpreter** — runs all validation programs (I/O, threads, channels, closures, collections, pools)
4. **Tooling** — `rask test`, `rask fmt`, `rask lint`, `rask describe`, `rask explain`, `rask check`, LSP
5. **Monomorphization** — reachability analysis, generic instantiation, layout computation
6. **MIR lowering** — full AST → MIR for all constructs (control flow, closures, error handling)
7. **Cranelift backend** — MIR → native code, 244 dispatch entries, links with C runtime
8. **C runtime** — Vec, Map, Pool, String, channels, green scheduler (M:N with io_uring/epoll), threading, file I/O, JSON encoding, atomics, Shared<T>
9. **Build system** — full pipeline (`rask build`), `rask init/fetch/add/remove/clean`, watch mode, build scripts, feature resolution, capability inference
10. **Package management** — remote registry (`packages.rask-lang.dev`), semver resolution, SHA-256 verified cache, lock files, transitive deps

## What compiles natively today

Hello world, string ops/interpolation, structs, field access, for/while loops, closures (mixed-type captures), Vec/Map/Pool operations, enum construction, multi-function programs, arithmetic, control flow, threads (`Thread.spawn`, `ThreadPool.spawn`), channels, timing, file I/O, unsafe blocks, raw pointers, result types, error propagation.

## Validation programs

| Program | Native | Notes |
|---------|--------|-------|
| grep clone | Yes | CLI args + file I/O |
| Text editor with undo | Yes | Pool + Handle + ensure |
| Game loop with entities | Yes | Pool, handles, threading |
| Sensor processor | Yes | Threads, timing, shared Vec |
| HTTP JSON API server | No | Needs HTTP + JSON stdlib in Rask |

## Stdlib architecture

The stdlib has two layers:

| Layer | Language | What lives here |
|-------|----------|-----------------|
| **Runtime** | C | OS interface, memory primitives, data structures (Vec, Map, Pool, String), concurrency (threads, channels, green scheduler, atomics), raw I/O syscalls |
| **Stdlib** | Rask | Everything above the OS — HTTP, JSON, CSV, URL, base64, hashing, unicode, terminal |

**Why Rask for stdlib:** Dogfooding validates the language. Rask code gets ownership/bounds checking that C doesn't. Writing an HTTP parser in Rask is a harder test than an example program — if the language can't handle it, something's wrong.

**C stays for:** things that must talk to the OS (syscalls, memory allocation, thread creation, io_uring) or wrap existing C libraries (TLS via OpenSSL/mbedTLS, hardware-accelerated crypto).

## What's next

### Phase 1: Stdlib in Rask + HTTP server validation

Write the first stdlib modules in Rask and validate the HTTP server compiles natively. This is the real test of multi-file compilation + stdlib imports + native codegen on real code.

- [ ] HTTP/1.1 request parser in Rask (method, path, headers, body)
- [ ] HTTP response serialization in Rask (status line, headers, body)
- [ ] JSON parser rewrite in Rask (nested objects, arrays — current C version only handles flat objects)
- [ ] Validate http_api_server.rk compiles and runs natively — all 5 programs done

### Phase 2: Stdlib breadth

Remaining modules, all written in Rask (except TLS which wraps a C library):

| Module | Language | Purpose |
|--------|----------|---------|
| url | Rask | URL parsing (RFC 3986) |
| encoding | Rask | Base64, hex, URL encoding (RFC 4648) |
| csv | Rask | CSV parsing/writing (RFC 4180) |
| unicode | Rask | Properties, normalization, categories |
| terminal | Rask | ANSI colors, terminal detection |
| hash | Rask (or C for HW accel) | SHA-256, MD5, CRC32 |
| tls | C shim + Rask API | TLS/SSL via OpenSSL/mbedTLS |

Each module needs: spec, implementation in Rask, tests.

### Phase 3: Build system completion

Specified but not implemented:

- [x] **Vendoring** — `rask vendor` for offline builds (VD1-VD5)
- [x] **Workspaces** — multi-package repos with shared lock file (WS1-WS3)
- [ ] **Conditional compilation** — `comptime if cfg.os/arch/features` (CC1-CC2)
- [x] **`rask publish`** — push packages to registry (PB1-PB7)
- [x] **Dependency auditing** — `rask audit` for CVE checking (AU1-AU5)

### Phase 4: Runtime & codegen maturity

- [ ] **Runtime trait dispatch** — `any Trait` for heterogeneous collections
- [ ] **State machine codegen** — stackless transforms for green tasks (optimization, not blocking)
- [ ] **Cross-compilation** — `--target` flag wired to Cranelift + cross-linker detection (XT1-XT8)

### Post-v1.0

- LLVM backend
- Incremental compilation (semantic hashing)
- Macros / `format!`
- Comptime debugger
- Fuzzing / property-based testing
- Code coverage
- `std.reflect` — comptime reflection
- Inline assembly
- Pointer provenance rules

## Open design questions

- Package granularity — folder = package (Go-style) vs file = package (Zig-style)
- Field projections for `ThreadPool.spawn` closures (disjoint field access)
- Task-local storage syntax
- `Projectable` trait — custom containers with `with...as`
- String C interop — `as_c_str()`, `string.from_c()`
- `pool.remove_with(h, |val| { ... })` — cascading @resource cleanup
- Style guideline: max 3 context clauses per function
