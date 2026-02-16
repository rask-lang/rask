# Rask — Status & Roadmap

## Current State (2026-02-16)

**Language design:** Complete. 70+ spec files, all core semantics stable.

**Frontend (Phases 1-4):** Complete. Lexer, parser, resolver, type checker, ownership checker all work. Union types, select, using clauses, linear resources, closure captures — all implemented.

**Interpreter:** Fully functional. 15+ stdlib modules. 4/5 validation programs run (grep, editor, game loop, HTTP server). Sensor processor typechecks but doesn't run (SIMD not interpreted).

**Monomorphization + MIR:** Complete. Struct/enum layouts, generic instantiation, reachability analysis, full AST→MIR lowering with type inference. 94 tests.

**Cranelift backend:** Functional for core programs. All MIR statements/terminators implemented. Stdlib dispatch (Vec, String, Map, Pool → C runtime). Closures. Integer widening. For-range loops. 35 codegen tests.

**Tooling:** LSP, formatter, linter, test runner, describe, explain — all done.

### What compiles natively today

Hello world, string ops, structs with field access, for/while loops, closures (mixed-type captures), Vec/Map/Pool operations, enum construction, multi-function programs, arithmetic, control flow.

### Validation programs

| Program | Interpreter | Native |
|---------|-------------|--------|
| grep clone | Runs | No (needs stdlib module calls) |
| Text editor | Runs | No (needs stdlib module calls) |
| Game loop | Runs | No (needs stdlib module calls) |
| HTTP server | Runs | No (needs concurrency runtime) |
| Sensor processor | Typechecks | No (needs SIMD codegen) |

---

## Active Work — Phase 5: Code Generation Completeness

### Next up

- [ ] **Stdlib module calls in codegen** — `cli.parse()`, `fs.read()`, `io.stdin()` etc. Module-qualified names aren't resolved in MIR. The C runtime already has backing functions; this is plumbing.
- [ ] **Concurrency runtime (rask-rt)** — spawn, join, channels, Shared<T>/Mutex as native code. Interpreter has the semantics, need C or Rust implementations that compiled programs can link against.
- [ ] **Closure escape handling** — Closures are stack-allocated. Escaping closures (returned, passed to spawn) dangle. Needs heap allocation or escape analysis.
- [ ] **Native validation programs** — Get all 5 validation programs compiling and running natively. This is the milestone that proves the backend works.

### Known codegen limitations (not blocking, track for later)

- Stdlib dispatch uses bare names (`push`, `len`, `get`) — ambiguous without type info. Needs qualified names or type-directed dispatch when monomorphizer evolves.
- CleanupReturn inlines cleanup blocks — works but duplicates cleanup code. Fine for now, revisit if code size matters.

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
