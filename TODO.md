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

## Recently Completed (2026-02-16)

- [x] **Float codegen** — `fadd`/`fsub`/`fmul`/`fdiv`/`fneg` + `fcmp` with `FloatCC`. Previously all float ops used integer instructions.
- [x] **Unsigned codegen** — `udiv`/`urem`/`ushr` + unsigned `icmp` for U8/U16/U32/U64. Previously always signed.
- [x] **Type-correct printing** — `rask_print_f32`, `rask_print_char` (UTF-8), `rask_print_u64`. Previously F32→F64 implicit, no char print, unsigned printed as signed.
- [x] **Iterator skip** — `rask_iter_skip` now creates a new Vec skipping N elements. Was a no-op stub.
- [x] **Map operations** — `get`, `remove`, `len`, `is_empty`, `clear`, `keys`, `values` in runtime + dispatch. Previously only `new`/`insert`/`contains_key`.
- [x] **String operations** — `split`, `parse_int`, `parse_float`, `substr`, `ends_with`, `replace` in runtime + dispatch. Also `f64_to_string`, `char_to_string`.
- [x] **MirType helpers** — `is_float()`, `is_unsigned()` on MirType for codegen instruction selection.

## Active Work — Phase 5: Code Generation Completeness

### Next session priorities

1. **Runtime type migration** — The big architectural debt. Two parallel C implementations exist:
   - Old i64-based (inline in `runtime.c`) — currently linked and used
   - New typed (`vec.c`, `string.c`, `map.c`, `pool.c` + `rask_runtime.h`) — proper structs with `elem_size`, not linked
   - Steps: update `link.rs` to compile the separate `.c` files, update dispatch signatures to match typed API, remove old i64 duplicates from `runtime.c`
   - See `dispatch.rs` lines 9-25 for the full migration plan

2. **Stdlib module calls in codegen** — `cli.parse()`, `fs.read()`, `io.stdin()` etc. Module-qualified names aren't resolved in MIR. The C runtime already has backing functions (`rask_cli_args`, `rask_fs_read_lines`, etc. are in dispatch.rs); the gap is MIR lowering losing the module prefix.

3. **Concurrency runtime (rask-rt)** — spawn, join, channels, Shared<T>/Mutex as native code. Interpreter has the semantics, need C or Rust implementations that compiled programs can link against. Green thread scheduler exists in `rask-rt/green/` but isn't integrated into the spawn path.

4. ~~**Closure escape handling**~~ — Done. Escape analysis downgrades non-escaping closures to stack; heap closures get `ClosureDrop` before returns. Cross-function analysis (`optimize_all_closures`) checks if callee parameters escape — borrow-only callees (e.g., `forEach`) get proper caller-side drops, ownership-taking callees (e.g., `spawn`, runtime functions) suppress drops. Remaining gaps: concurrency integration blocked on runtime (#3). Note: `ClosureDrop` only inserts before `Return` terminators — heap closures created in loops that aren't fully transferred will leak per iteration. Not a problem for stack-allocated (non-escaping) closures.

5. **Native validation programs** — Get all 5 validation programs compiling and running natively. This is the milestone that proves the backend works.

### Smaller items to pick off

- [ ] **Spec-test type checking** — `rask-spec-test/runner.rs` only lexes+parses, so `compile-fail` tests can't verify type errors. Wire the type checker into the test runner.
- [ ] **Complex assignment targets** — `a[i].field = val` fails in MIR lowering (`rask-mir/lower/stmt.rs:84`). Only `Ident` and `Field` targets work.
- [ ] **Iterator protocol** — `iter()` returns identity (clone), no real iterator abstraction. Need at minimum `map`, `filter`, `collect`.
- [ ] **Rng type** — Completely unimplemented in interpreter (`rask-interp/stdlib/mod.rs:147`). Returns error for all methods.
- [ ] **Array elem_ty** — `rask-mir/lower/expr.rs:809` has `let _ = elem_ty; // TODO: Use for proper array type`.
- [ ] **Niche optimization** — `rask-mono/layout.rs:60` — Handle/Reference could be smaller.
- [ ] **100+ skipped spec test blocks** — loops (11), ensure (9), ranges (3), comptime (5), and many more across stdlib specs.

### Known codegen limitations (not blocking, track for later)

- Stdlib dispatch uses bare names (`push`, `len`, `get`) — ambiguous without type info. Needs qualified names or type-directed dispatch when monomorphizer evolves.
- CleanupReturn inlines cleanup blocks — works but duplicates cleanup code. Fine for now, revisit if code size matters.
- `map_err` dispatch is a pass-through (no closure application). Needs closure dispatch infrastructure.

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
