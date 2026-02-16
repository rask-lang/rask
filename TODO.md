# Rask — Status & Roadmap

## Current State (2026-02-16)

**Language design:** Complete. 70+ spec files, all core semantics stable.

**Frontend (Phases 1-4):** Complete. Lexer, parser, resolver, type checker, ownership checker all work. Union types, select, using clauses, linear resources, closure captures — all implemented.

**Interpreter:** Fully functional. 15+ stdlib modules. 4/5 validation programs run (grep, editor, game loop, HTTP server). Sensor processor typechecks but doesn't run (SIMD not interpreted).

**Monomorphization + MIR:** Complete. Struct/enum layouts, generic instantiation, reachability analysis, full AST→MIR lowering with type inference. 94 tests.

**Cranelift backend:** String interpolation (desugared at compile time), enum pattern matching, for-in loops (index-based lowering), stdlib module calls (cli, fs, io, std, string → C runtime), closure escape analysis (heap vs stack allocation). 35+ codegen tests.

**C runtime:** Vec, String, Map, Pool, I/O, CLI args, threads (OS-level spawn/join/cancel), buffered+unbuffered channels, Mutex, Shared (RwLock), panic handler with backtraces, swappable allocator with stats tracking.

**Tooling:** LSP, formatter, linter, test runner, describe, explain — all done.

### What compiles natively today

Hello world, string ops, structs with field access, for/while/for-in loops, closures (mixed-type captures, escape analysis), Vec/Map/Pool operations, enum construction + pattern matching, string interpolation, multi-function programs, arithmetic, control flow.

### Validation programs

| Program | Interpreter | Native |
|---------|-------------|--------|
| grep clone | Runs | No (needs struct methods, remaining I/O wiring) |
| Text editor | Runs | No (needs struct methods, file I/O) |
| Game loop | Runs | No (needs struct methods) |
| HTTP server | Runs | No (needs concurrency codegen wiring) |
| Sensor processor | Typechecks | No (needs SIMD codegen) |

---

## Active Work — Phase 5: Code Generation Completeness

### Done

- [x] **Stdlib module calls in codegen** — `cli.args()`, `fs.read_lines()`, `fs.read_file()`, `fs.write_file()`, `fs.exists()`, `io.read_line()`, `std.exit()`, string methods all dispatch to C runtime.
- [x] **Closure escape analysis** — Per-function analysis determines heap vs stack allocation. Escaping closures (returned, passed to calls, stored) get heap-allocated. Non-escaping closures use stack slots. `ClosureDrop` inserted for cleanup.
- [x] **Concurrency runtime** — OS threads (spawn/join/cancel), buffered+unbuffered channels, Mutex, Shared<T>, panic handler, allocator. All built as C runtime, ready to link.
- [x] **String interpolation** — Desugared to `.concat()` + `.to_string()` calls at compile time.
- [x] **Enum pattern matching** — Variant tag resolution, switch lowering with comparison chains.
- [x] **For-in loops** — Lowered to index-based while loops (avoids iterator state machines).

### Next up

- [ ] **Struct methods in codegen** — `extend Type { func method(self) }` calls aren't wired through codegen. This blocks most validation programs.
- [ ] **Concurrency codegen wiring** — Runtime primitives exist in C but `spawn()`, `join()`, channel ops aren't lowered from MIR to C runtime calls yet.
- [ ] **Native validation programs** — Get all 5 validation programs compiling and running natively. This is the milestone that proves the backend works.

### Known codegen limitations (not blocking, track for later)

- Stdlib dispatch uses bare names (`push`, `len`, `get`) — ambiguous without type info. Needs qualified names or type-directed dispatch when monomorphizer evolves.
- CleanupReturn inlines cleanup blocks — works but duplicates cleanup code. Fine for now, revisit if code size matters.
- Trait dispatch not wired to runtime closures.
- Unsafe blocks / raw pointers not lowered.

---

## Phase 6: Ecosystem

### Done

- [x] LSP — type-aware completions, go-to-definition, hover, diagnostics
- [x] Test runner — `rask test`
- [x] Formatter — `rask fmt`
- [x] Linter — `rask lint`
- [x] Describe — `rask describe`
- [x] Explain — `rask explain` (43 error codes)
- [x] Structured diagnostics — `fix:` / `why:` fields

### Remaining

- [ ] **Build system** — output dirs, `rask add`/`remove`, watch mode, cross-compilation. `build.rk` manifest exists, not wired end-to-end. See [build.md](specs/structure/build.md).
- [ ] **Package manager** — directory-based imports work initially, registry/versioning later.

---

## Open Design Questions

### After codegen works (evaluate with real usage)

- [ ] Package granularity — folder = package (Go-style) vs file = package (Zig-style)
- [ ] Field projections for `ThreadPool.spawn` closures — disjoint field access across threads
- [ ] Task-local storage syntax
- [ ] `Projectable` trait — custom containers with `with...as`
- [ ] String interop — `as_c_str()`, `string.from_c()`
- [ ] `pool.remove_with(h, |val| { ... })` — cascading @resource cleanup helper
- [ ] Style guideline: max 3 context clauses per function

### Deferred (post-v1.0)

**Compilation:**
- [ ] LLVM backend
- [ ] Incremental compilation (semantic hashing)
- [ ] Cross-compilation

**Tooling:**
- [ ] Comptime debugger
- [ ] Fuzzing / property-based testing
- [ ] Code coverage

**Language extensions:**
- [ ] `std.reflect` — comptime reflection. See [reflect.md](specs/stdlib/reflect.md)
- [ ] Macros / `format!`
- [ ] Inline assembly
- [ ] Pointer provenance rules
- [ ] Comptime memoization

**Ecosystem:**
- [ ] `compile_cpp()` build script support
- [ ] Auto Rask wrapper generation from cbindgen
- [ ] Capability-based security for dependencies
