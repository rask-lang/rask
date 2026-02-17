# Rask — Status & Roadmap

## Current State (2026-02-17)

**Language design:** Complete. 70+ spec files, all core semantics stable.

**Frontend (Phases 1-4):** Lexer, parser, resolver work. Type checker has significant gaps — no validation program passes `rask check` (see bugs below). Ownership checker implemented but linear resource enforcement missing.

**Interpreter:** 15+ stdlib modules. Previously ran 4/5 validation programs, but type checker regressions now block all 5 from running.

**Monomorphization + MIR:** Struct/enum layouts, generic instantiation, reachability analysis, AST→MIR lowering. 104 tests. Has placeholder types for compound types and incomplete generic substitution.

**Cranelift backend:** Functional for core programs. All MIR statements/terminators implemented. Stdlib dispatch (Vec, String, Map, Pool, Rng, File → C runtime). Closures. Integer widening. For-range loops. 42 codegen tests.

**Tooling:** LSP, formatter, linter, test runner, describe, explain — all done.

**Spec tests:** 126 total, 124 pass, 2 fail (resource-types.md — linear resource checker not enforced).

### What compiles natively today

Hello world, string ops, structs with field access, for/while loops, closures (mixed-type captures), Vec/Map/Pool operations, enum construction, multi-function programs, arithmetic, control flow, Rng (seeded + module-level), File I/O (open/read_all/lines/write), nested JSON encoding, array literals/indexing/iteration/repeat, iterator chains (`.iter().filter().map().collect()` and 6 other terminals).

### Validation programs

All 5 fail `rask check`. None compile natively.

| Program | Errors | Blockers |
|---------|--------|----------|
| grep clone | 2 | Vec generic inference, match return type inference |
| Text editor | 22 | Vec generic inference, missing Vec.insert/remove, try-on-non-Result, int type mismatch |
| Game loop | 9 | Missing mutate annotations, Pool.insert return type, missing Pool.handles() |
| HTTP server | 1 | `import async.spawn` false shadow conflict |
| Sensor processor | 26 | Integer literal defaults to i32 (not u64/u8), float literal defaults to f64 (not f32), bad error spans pointing to line 1 |

---

## Recently Completed (2026-02-17)

- **Iterator codegen** — Inline expansion at MIR level. `.iter().filter(|x| pred).map(|x| f(x))` chains recognized and fused into index-based loops with inlined closure bodies. Zero runtime overhead — no iterator objects, no dispatch table entries. Terminals: `.collect()`, `.fold()`, `.any()`, `.all()`, `.count()`, `.sum()`, `.find()`. `for-in` over chains works too.
- **Top-level statement support** — Parser accepts bare statements (`println(x)`, `for i in 0..10 {}`, `const x = 42`) at top level, wraps them in a synthetic `func main()`. `let` (mutable) rejected with clear error. 50 spec tests unskipped (126 total, 124 pass).
- **Rng codegen** — xoshiro256++ in `random.c`. Instance methods (`Rng.from_seed()`, `rng.range()`, etc.) and module convenience (`random.range()`, `random.bool()`) dispatch through to C runtime. Compiles to native.
- **File instance methods** — `file.lines()`, `file.read_all()`, `file.write()`, `file.write_line()`, `file.close()` wired from MIR dispatch to C runtime functions. Compiles to native.
- **Closure leak in loops** — `ClosureDrop` now inserts at loop back-edges (Goto/Branch), not just before Return terminators. Heap closures in loops no longer leak per iteration.
- **Nested JSON** — `json.encode` handles nested struct fields via recursive MIR lowering + `json_buf_add_raw` C helper.
- **Array codegen** — `arr.len()` constant-folds at compile time. `arr[i]` and `arr[i] = val` lower to `MirRValue::ArrayIndex` / `MirStmt::ArrayStore` (direct pointer arithmetic, no runtime calls). `for item in arr` uses array length + ArrayIndex. `[val; N]` expands to N stores. All array operations are structural — no dispatch table entries.
- **Rng.from_seed() in interpreter** — `Rng`, `File`, `f32x8` registered as builtin types in interpreter (matching resolver builtins). Selective imports (`import random.Rng`, `import time.Instant`, etc.) also work.
- **Local type prefix tracking** — MIR lowerer tracks stdlib type prefixes through variable bindings (e.g. `const rng = Rng.from_seed(42)` → rng has prefix "Rng"). Fixes method dispatch for Rng, File, and chained calls like `file.lines().len()`. Also fixes Vec method dispatch that was silently broken in codegen.
- **Lazy iterator protocol** — 9 adapter variants (Vec, Map, Filter, Enumerate, Take, Skip, Range, FlatMap, Zip), 16 methods, `for-in` loop support in interpreter.
- **Rng type (interpreter)** — xoshiro256++ PRNG with `Rng.new()`, `Rng.from_seed()`, instance methods (u64, i64, f64, f32, bool, range, shuffle, choice).
- **Import resolution** — All 13 builtin modules resolve in the resolver (was only 5).
- **Spec-test type checking** — `compile`/`compile-fail` tests now run resolve + typecheck. 76/76 pass.
- **Complex assignment targets** — `a[i] = val` lowers to `set(a, i, val)` in MIR.
- **Array elem_ty** — `array_repeat` passes elem_size to runtime. `elem_size_for_type()` computes sizes from MirType.
- **File/Rng stdlib types** — Method definitions registered in `rask-stdlib/types.rs` for type checker.
- **Closure escape analysis** — Non-escaping closures stack-allocated; heap closures get `ClosureDrop` before returns.
- **Stdlib module calls in codegen** — Module-qualified names resolve through MIR to C runtime dispatch.
- **Green task scheduler** — Work-stealing scheduler with io_uring/epoll I/O engine (C runtime).
- **Typed C runtime** — `vec.c`/`map.c`/`pool.c`/`string.c` replace inline i64 implementations.

## Active Work — Phase 5: Code Generation Completeness

### Next session priorities

4. **Native validation programs** — Get all 5 validation programs compiling and running natively. This is the milestone that proves the backend works.

### Smaller items to pick off

- [x] **Niche optimization** — Done. `Option<Handle<T>>` uses all-ones sentinel (-1) as None. Layout reports 8 bytes (no tag). MIR lowering emits compare-to-sentinel instead of EnumTag reads.
- [x] **Top-level statement support** — Done. Parser accepts bare statements, `const`, and expressions at top level. `let` rejected with clear error. Wraps in synthetic `func main()`. 50 spec tests unskipped (126 total, 124 pass).
- [x] **Iterator codegen** — Done. Inline expansion at MIR level: `.iter().filter().map()` chains fuse into index-based loops with inlined closures. Zero runtime overhead (no iterator objects, no dispatch). Terminals: `.collect()`, `.fold()`, `.any()`, `.all()`, `.count()`, `.sum()`, `.find()`. `for-in` over chains works too.
- [x] **Rng codegen** — Done. `random.c` (xoshiro256++), dispatch entries, linker wiring. `Rng.from_seed()` + module convenience both compile.
- [x] **Closure leak in loops** — Done. Back-edge detection inserts `ClosureDrop` at loop boundaries.
- [x] **Rng.from_seed() in interpreter** — Done. Builtin types (Rng, File, f32x8) registered in interpreter, matching resolver.

### Known codegen limitations (not blocking, track for later)

- ~~Stdlib dispatch uses bare names — ambiguous without type info.~~ Fixed: type-qualified dispatch (`Vec_push`, `Map_get`, `string_contains`, etc.) using node_types from type checker + local_type_prefix fallback.
- CleanupReturn inlines cleanup blocks — works but duplicates cleanup code. Fine for now, revisit if code size matters.
- ~~`map_err` dispatch is a pass-through.~~ Fixed: inline MIR expansion branches on Result tag, calls closure on Err payload via `ClosureCall`. Non-closure `map_err` (variant constructors) still uses pass-through stub.
- ~~Type checker leaves many builtin types as `Var(TypeVarId(...))` — local_type_prefix in MIR lowerer compensates but only works for direct variable references, not chained expressions like `Vec.new().len()`.~~ Fixed: type checker recognizes Vec, Map, Pool, Rng as type namespaces and resolves their methods directly (static + instance). `local_type_prefix` retained as fallback.

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
