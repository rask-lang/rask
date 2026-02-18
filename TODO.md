# Rask — Status & Roadmap

## Current State (2026-02-18)

**Language design:** Complete. 70+ spec files, all core semantics stable.

**Frontend (Phases 1-4):** Lexer, parser, resolver, type checker all work. 5/5 validation programs pass `rask check`. Multiple trait bounds, literal type inference, struct field generic unification all fixed.

**Interpreter:** 15+ stdlib modules. Nested index assignment refactored. spawn() checks for `using Multitasking` context (but still uses OS threads internally).

**Monomorphization + MIR:** Struct/enum layouts, generic instantiation (including structs/enums), reachability analysis, AST→MIR lowering. Real MIR types for Tuple, Option, Result, Union, Slice. 104 tests.

**Cranelift backend:** Functional for core programs. All MIR statements/terminators implemented. Stdlib dispatch (Vec, String, Map, Pool, Rng, File → C runtime). Closures. Integer widening. For-range loops. Compound type field access. Binaries output to `build/debug/`. 42 codegen tests.

**Tooling:** LSP, formatter, linter, test runner, describe, explain — all done.

**Spec tests:** 126 total, 126 pass.

### What compiles natively today

Hello world, string ops, structs with field access, for/while loops, closures (mixed-type captures), Vec/Map/Pool operations, enum construction, multi-function programs, arithmetic, control flow, Rng (seeded + module-level), File I/O (open/read_all/lines/write), nested JSON encoding, array literals/indexing/iteration/repeat, iterator chains (`.iter().filter().map().collect()` and 6 other terminals).

### Validation programs

5/5 pass `rask check`. None compile natively yet.

| Program | `rask check` | Native | Remaining blockers |
|---------|-------------|--------|--------------------|
| grep clone | Pass | No | Needs stdlib module calls in codegen |
| Text editor | Pass | No | Needs stdlib module calls in codegen |
| Game loop | Pass | No | Needs stdlib module calls in codegen |
| HTTP server | Pass | No | Needs concurrency runtime |
| Sensor processor | Pass | No | Needs SIMD/atomics codegen |

---

## Recently Completed (2026-02-18)

### Cleanup (2026-02-18)
- **Codegen bug fixes** — Pool_insert, Vec_insert missing pointer adaptation; Pool_index missing DerefResult. All three would crash compiled programs.
- **Zero compiler warnings** — Cleaned up 21 warnings across 5 crates (dead code from runtime migration, unused imports/variables).
- **Build output** — Binaries now go to `build/debug/`, intermediate `.o` files cleaned up.

### Previous (2026-02-17)

### Type checker fixes (was: 60 errors across 5 programs → 6 remaining)
- **Literal type inference** — Integer/float literals no longer default to i32/f64 immediately. Fresh type variables with deferred defaults let context (struct fields, function params) drive the type. `apply_literal_defaults()` runs post-solve.
- **Generic inference in struct literals** — `infer_expr_expecting` pushes expected field type into expression inference. `Vec.new()` now correctly unifies with `Vec<string>` from struct field context.
- **Match expression return type** — Match in expression position propagates arm types correctly.
- **`try` on non-Result** — Dedicated `TryOnNonResult` error instead of confusing `expected _ or _`.
- **Import shadow relaxed** — `import async.spawn` can override builtins (not user-defined bindings).
- **Multiple trait bounds** — `T: A + B + C` parses all bounds.
- **Profile blocks** — Parsed into `ProfileDecl` structs instead of discarded.

### Stdlib methods + runtime
- **Vec.insert/remove** — Registered in type checker, C runtime (`rask_vec_insert_at`/`rask_vec_remove_at`), dispatch table.
- **Pool.handles()** — Returns `Vec<Handle<T>>`. C runtime (`rask_pool_handles_packed`), dispatch table.

### MIR / codegen improvements
- **Real compound MIR types** — Tuple, Option, Result, Union, Slice get proper MIR types with layout info instead of collapsing to `MirType::Ptr`.
- **Complex function expressions** — Non-ident callees (field access, returned functions) emit `ClosureCall`.
- **enumerate() tuple lowering** — Builds `(index, element)` on the stack.
- **Field access on compound types** — Computed offsets using element type sizes with alignment.
- **map_err with variant constructors** — Inline MIR expansion (branch on tag, wrap error payload).
- **Ok/Some/Err constructor types** — Use type checker info for result MIR type.

### Monomorphization
- **Generic substitution rewrite** — Handles `Vec<T>`, `Result<T, E>`, `T?`, `(A, B)` tuples, nested generics.
- **Struct/enum instantiation** — `clone_struct_decl`/`clone_enum_decl` added. Generic structs no longer panic.
- **Layout robustness** — Known builtins (Vec=24, Map/Rng/Channel=8, I128/U128=16). Unresolved types warn instead of panicking.

### Comptime
- **Closures evaluatable** — `ComptimeValue::Closure` with captured environment, `call_closure` for invocation.
- **Literal pattern matching** — Actually evaluates and compares (was always-true).
- **Non-exhaustive match** — Returns `NonExhaustiveMatch` error instead of silent Unit.
- **Indirect function calls** — Non-ident callees evaluate as closures.

### Interpreter
- **Index assignment refactored** — Nested `container[a][b].field = val` works. Map support in index assignment.

### Spec tests
- **126/126 pass** — Resource-types examples rewritten as self-contained (`DbConn` struct instead of `File`).

### Previous
- Iterator codegen, top-level statements, Rng/File codegen, closure leak fix, nested JSON, array codegen, lazy iterators, import resolution, closure escape analysis, green task scheduler, typed C runtime.

## Compiler Bugs

### Resolved
- [x] **Ownership checker false positives on copy types** — Fixed: copy semantics for primitives and small value types.

### Type checker — correctness (not blocking validation programs)

- [x] **Trait method matching checks arity only** — Fixed: `signatures_match()` now checks parameter types, modes, and return type. Type variables (Self) are skipped during comparison.
- [x] **Unification catch-all produces useless errors** — Already fixed: line 223 in `unify.rs` handles `(Type::Error, _) | (_, Type::Error)` before the catch-all. No change needed.

### MIR / codegen — remaining

- [x] **Void → i64 placeholder** — Fixed: void-typed call destinations are skipped in builder.rs. The i64 mapping remains as fallback for variable declarations (Cranelift requires a concrete type).

### Monomorphization — remaining

- [x] **Named types use layout cache** — Fixed: `type_size_align()` takes a `LayoutCache` parameter. User-defined `UnresolvedNamed` types are resolved from the cache. Monomorphizer populates the cache incrementally as layouts are computed.

### Comptime — remaining

- [x] **`if`-pattern-match in comptime** — Fixed: `ExprKind::IfLet` now evaluatable. Method calls already worked (dead error entry removed). Block calls remain correctly forbidden.

### Interpreter — remaining

- [x] **spawn() uses bounded thread pool** — Fixed: `MultitaskingRuntime` now creates a bounded thread pool (N workers). `spawn()` inside `using Multitasking` submits tasks to the pool instead of creating unbounded OS threads. Results flow back via channels. Pool shuts down on block exit.

---

## Active Work — Phase 5: Code Generation Completeness

### Next priorities

2. **Native validation programs** — Get all 5 compiling and running natively. Type checker passes; now need codegen for stdlib module calls + concurrency runtime.

Shared<T>, Channel<T>, Sender<T>, HttpResponse types (HTTP server)
io.read_line() runtime (text editor)
thread.ThreadPool module (game loop)
Atomic<T>, SIMD intrinsics, comptime arrays (sensor processor

### Completed

- [x] **Instant/Duration arithmetic** — `+`, `-`, `<`, `<=`, `>`, `>=`, `==` on time types.
- [x] **Niche optimization** — `Option<Handle<T>>` uses all-ones sentinel (-1) as None.
- [x] **Top-level statement support** — Bare statements wrapped in synthetic `func main()`.
- [x] **Iterator codegen** — Inline expansion at MIR level, zero runtime overhead.
- [x] **Rng codegen** — `random.c` (xoshiro256++), dispatch entries, linker wiring.
- [x] **Closure leak in loops** — Back-edge detection inserts `ClosureDrop` at loop boundaries.
- [x] **Rng.from_seed() in interpreter** — Builtin types registered in interpreter.

### Known codegen limitations (not blocking, track for later)

- CleanupReturn inlines cleanup blocks — works but duplicates cleanup code. Fine for now.
- Non-closure `map_err` (variant constructors) uses pass-through stub.

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
