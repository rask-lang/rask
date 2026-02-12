# Compiler Implementation Readiness

Status of compiler infrastructure and preparatory work before beginning code generation.

## Completed ✓

### 1. Name Mangling Scheme ([specs/compiler/name-mangling.md](specs/compiler/name-mangling.md))

Complete symbol naming scheme for object file emission:
- `_R` prefix for all Rask symbols
- Length-encoded package paths
- Type markers (F=function, M=method, S=struct, etc.)
- Generic argument encoding with brackets
- Collision hashes when needed (4-char FNV-1a)
- Special handling for main, extern, tests, closures

Examples:
```
_R4core_F3add_Gi32i32i32              // core::add<i32, i32, i32>
_R4core_M3Vec4push_Gi32               // Vec<i32>::push
_Rrt_alloc                            // Runtime allocator
```

### 2. Memory Layout Specification ([specs/compiler/memory-layout.md](specs/compiler/memory-layout.md))

Precise ABI-level layouts for all types:

**Structs:**
- Fields in source order, no reordering
- Padding for alignment
- Tail padding to struct alignment

**Enums:**
- Discriminant first (u8/u16)
- Union of variant payloads
- Size = max variant + discriminant + padding

**Closures:**
- Struct of captured values + function pointer
- Captures alphabetically ordered
- Function pointer last

**Trait Objects (`any Trait`):**
- Fat pointer: data ptr (8) + vtable ptr (8) = 16 bytes
- Vtable: size, align, drop, then methods in trait order

**Collections:**
- `Vec<T>`: ptr, len, cap, max_cap (40 bytes)
- `Map<K,V>`: buckets, len, cap, max_cap (40 bytes)
- `Pool<T>`: data, len, cap, free_head, generations (40 bytes)
- `Handle<T>`: index (u32), generation (u32) = 8 bytes
- `string`: ptr, len (16 bytes)

### 3. Test Infrastructure Design ([specs/compiler/test-infrastructure.md](specs/compiler/test-infrastructure.md))

Six-level test pyramid:
1. **Unit tests** - Individual functions (inline in source)
2. **Component tests** - Full compiler phases (tests/ dirs)
3. **Spec tests** - Literate tests from markdown (already implemented)
4. **Integration tests** - Full pipeline, no execution
5. **End-to-end tests** - Compile + run programs
6. **Validation programs** - Real-world stress tests

Includes fuzzing strategy, benchmarks, error message testing.

### 4. Closure Syntax Verified

Parser correctly handles:
- Parameter types: `|x: i32|`
- Return type annotations: `|x| -> i32`
- Block bodies: `|x| { ... }`
- Expression bodies: `|x| x + 1`
- No parameters: `|| expr`

**Found issue:** Type checker doesn't validate declared types against inferred types (affects closures and other constructs). Needs broader type checking work.

### 5. Stdlib Implementation Audit ([specs/compiler/stdlib-audit.md](specs/compiler/stdlib-audit.md))

Comprehensive comparison of interpreter vs spec:

**Vec:** 40+ methods implemented
- ✓ Core: push, pop, len, get, clear, reverse, contains, etc.
- ✓ Iterators: filter, map, fold, reduce, any, all, find, etc.
- ✗ **Missing:** `take_all()`, `modify()`, `read()`, `remove_where()`

**Map:** 12 methods implemented
- ✓ Core: insert, get, remove, len, keys, values, etc.
- ✗ **Missing:** `ensure()`, `ensure_modify()`, `take_all()`

**Pool:** 10 methods implemented
- ✓ Core: insert, get, remove, len, contains, etc.
- ✗ **Missing:** `take_all()`, proper `iter()` (handle+ref pairs)

**Critical gap:** Default iteration yields elements (should yield indices per spec).

### 6. Interpreter Verified Working

All changes tested and interpreter confirmed working:
- Closures with parameters
- Vec methods (map, fold, filter)
- Type inference
- All existing functionality intact

## Remaining Work

### Type Checking (Bug Fix)

Type checker doesn't validate declared types:
```rask
const x: string = 42  // Should fail but doesn't
const f = |x: i32| -> string { x + 1 }  // Should fail but doesn't
```

Needs investigation into `Checker::infer_expr()` and constraint solving.

### Compiler Implementation (Major Tasks)

These are the core code generation tasks:

1. **MIR (Mid-level IR) Lowering**
   - Lower AST to MIR for all constructs
   - Control flow: if, match, loops, try
   - Ownership tracking: moves, borrows
   - Closure desugaring
   - Reference: [specs/compiler/memory-layout.md](specs/compiler/memory-layout.md)

2. **Monomorphization Pass**
   - Reachability analysis from main/exports
   - Generic instantiation
   - Symbol name generation using mangling scheme
   - Layout computation using memory-layout.md

3. **Cranelift Backend**
   - MIR → Cranelift IR
   - Function codegen with calling convention
   - Vtable emission for trait objects
   - Object file output (.o)

4. **Runtime Library (`rask-rt`)**
   - Allocator (dlmalloc or system malloc)
   - Panic handler
   - Vec, Map, Pool implementations
   - String operations
   - Spawn/channel runtime (async?)

5. **Linker Integration**
   - Link generated .o with rask-rt
   - Produce executable binary
   - Handle external C libraries

### Stdlib Completeness

Priority additions to interpreter (also needed for rask-rt):
1. `Vec.take_all()`, `Map.take_all()`, `Pool.take_all()`
2. `Vec.modify()`, `Vec.read()` closure methods
3. `Map.ensure()`, `Map.ensure_modify()`
4. Fix default iteration to yield indices
5. String methods (need spec first)

## Recommended Next Steps

### Option A: Continue Compiler (Long Journey)

1. Start MIR definition in `compiler/crates/rask-mir/`
2. Implement AST → MIR lowering
3. Set up Cranelift integration
4. Begin runtime library in Rust

**Timeline:** Months of work, major effort

### Option B: Improve Interpreter First

1. Fix type checker validation bug
2. Implement missing stdlib methods (`take_all`, `ensure`, etc.)
3. Add string methods
4. Write comprehensive test suite
5. Build validation programs (HTTP server, grep, game demo)

**Timeline:** Weeks of work, proves language design

### Option C: Hybrid Approach

1. Fix type checker (1-2 days)
2. Implement critical stdlib gaps (2-3 days)
3. Write validation programs (3-4 days)
4. **Then** begin MIR work with confidence in design

**Recommended:** Option C validates the design before investing in codegen.

## Design Validation Status

| Validation Program | Status | Blocker |
|-------------------|--------|---------|
| HTTP JSON API | Not started | Need string split, http module |
| grep clone | Not started | Need file I/O, regex |
| Text editor with undo | Not started | Need string slicing |
| Game with entities | Not started | Need proper Pool iteration |
| Embedded sensor | Not started | Need embedded runtime |

None of the validation programs can be written yet due to missing stdlib.

## Summary

**Design work:** Complete ✓
- Name mangling scheme defined
- Memory layouts specified
- Test strategy designed
- Closure syntax verified

**Implementation gaps:**
- Type checker validation bug
- Missing `take_all()` and closure methods in stdlib
- Iteration yields elements (should yield indices)
- No string methods yet

**Recommendation:** Fix stdlib gaps and write validation programs before starting MIR/codegen. This proves the language design works in practice before investing months in the compiler backend.

---

Last updated: 2026-02-12
