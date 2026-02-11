<!-- id: comp.codegen -->
<!-- status: decided -->
<!-- summary: MIR-based compilation pipeline with Cranelift backend and Rust runtime -->
<!-- depends: compiler/generation-coalescing.md, compiler/semantic-hash-caching.md, memory/ownership.md, types/generics.md -->

# Code Generation

Rask compiles through non-SSA MIR (mid-level IR) as control-flow graph. Monomorphization produces MIR. Rask-specific optimizations run on MIR. MIR lowers to Cranelift IR for dev builds (LLVM optional later for release). Programs link against `rask-rt` runtime library written in Rust.

## Pipeline

| Rule | Description |
|------|-------------|
| **P1: MIR intermediary** | All source compiles through MIR before backend lowering |
| **P2: Non-SSA** | MIR uses non-SSA form; backends construct SSA internally |
| **P3: Cranelift dev** | Dev builds use Cranelift backend |
| **P4: LLVM release** | Release builds may use LLVM backend (additive, future) |
| **P5: Runtime link** | Executables link against `rask-rt` static library |

```
Source → Lexer → Tokens → Parser → AST
  → Desugar → Resolve → TypeCheck → OwnershipCheck
  → Monomorphize → Lower to MIR → MIR Optimization
  → Backend Lowering → Object file → Link with rask-rt → Executable
```

## Monomorphization

| Rule | Description |
|------|-------------|
| **M1: Reachability** | Walk reachable code from `main()`; collect `(function_id, [concrete_type_args])` |
| **M2: Instantiate** | For each unique `(function_id, [type_args])`, produce AST copy with all type parameters replaced |
| **M3: Layout computation** | Compute field offsets, sizes, alignments for each concrete struct and enum |
| **M4: Transitive** | If an instantiated function calls other generics, add to work queue until fixpoint |
| **M5: Cache integration** | Monomorphized AST cached by `(function_id, [type_args], body_semantic_hash, [type_definition_hashes])` per `comp.semantic-hash` |

<!-- test: skip -->
```rask
// Monomorphized output (conceptual)
// MonoFunction { name: "sort$i32", type_args: [i32], body: ... }
// MonoFunction { name: "process$Player", type_args: [Player], body: ... }
```

## Vtable Dispatch (`any Trait`)

| Rule | Description |
|------|-------------|
| **V1: Fat pointer** | `any Trait` values are `(data_ptr, vtable_ptr)` at runtime |
| **V2: Vtable per type** | For each concrete type satisfying a trait, generate vtable with method pointers, drop fn, size, align |
| **V3: No monomorphization** | `any Trait` uses vtable dispatch, not monomorphization |

## MIR Types

All MIR types have known sizes. No generics remain.

| Rule | Description |
|------|-------------|
| **T1: Concrete only** | All types fully resolved before MIR lowering |
| **T2: Field ordering** | Struct fields sorted by alignment (largest first) to minimize padding |
| **T3: C layout override** | `@layout(C)` forces declaration order for C interop |
| **T4: Niche optimization** | `Option<Ptr>` uses null as `None`; `Option<Handle>` uses sentinel generation `0` |

```
MirType:
    Void | Bool
    I8, I16, I32, I64 | U8, U16, U32, U64 | F32, F64
    Ptr | FatPtr
    Struct(StructLayoutId) | Enum(EnumLayoutId)
    Array { elem: MirType, len: u32 }
    FuncPtr(SignatureId)
```

## MIR Statements and Terminators

| Rule | Description |
|------|-------------|
| **S1: Resource tracking** | MIR has `ResourceRegister`, `ResourceConsume`, `ResourceScopeCheck`, `ResourceTransfer` statements |
| **S2: Ensure lowering** | `ensure` blocks lower to `EnsurePush`/`EnsurePop` with cleanup block chains |
| **S3: Pool access** | `PoolCheckedAccess` is the target for generation check coalescing (`comp.gen-coalesce`) |
| **S4: Source locations** | `SourceLocation` statements carry debug line/column info |

```
MirStmt:
    Assign { dst, value: MirRValue }
    Store { addr, offset, value }
    Call { dst, func, args }
    ResourceRegister { dst, type_name, scope_depth }
    ResourceConsume { resource_id } | ResourceScopeCheck { scope_depth }
    EnsurePush { cleanup_block } | EnsurePop
    PoolCheckedAccess { dst, pool, handle }
    SourceLocation { line, col }

MirTerminator:
    Return | Goto | Branch { cond, then_block, else_block }
    Switch { value, cases, default }
    Unreachable | CleanupReturn { value, cleanup_chain }
```

## AST-to-MIR Lowering

Each Rask construct has a defined lowering to MIR blocks.

| Rule | Description |
|------|-------------|
| **L1: If/else** | Condition evaluates in entry block; branch to then/else blocks; both goto merge block |
| **L2: Match** | Extract enum tag; switch to variant blocks; each extracts payload then goto merge |
| **L3: Try** | Call, branch on Ok/Err tag; Err path runs ensure cleanup chain then returns error |
| **L4: Ensure** | `ensure_push` registers cleanup block; on all exit paths, cleanup block runs before return |
| **L5: Loops** | Init/check/body/exit blocks; `break value` assigns and jumps to exit |
| **L6: Closures** | Lower to struct (captured env) + function pointer; closure fn takes `env: Ptr` as first param |

<!-- test: skip -->
```rask
// try lowering example
const value = try fallible_call()
// → call, branch on tag, Ok path continues, Err path runs ensure chain + returns error
```

## MIR Optimization Passes

| Rule | Description |
|------|-------------|
| **O1: Semantics-preserving** | All MIR passes preserve program semantics |
| **O2: Generation coalescing** | Merge redundant `PoolCheckedAccess` on same (pool, handle) — see `comp.gen-coalesce` |
| **O3: Dead code elimination** | Remove unreachable blocks, unused assignments |
| **O4: Constant folding** | Evaluate constant expressions at compile time |
| **O5: Copy propagation** | Replace `x = y; use(x)` with `use(y)` |
| **O6: Inline small functions** | Inline leaf functions under size threshold (release only) |

## Backend Lowering (Cranelift)

| Rule | Description |
|------|-------------|
| **B1: Block mapping** | `MirBlock` → Cranelift `Block` |
| **B2: Variable mapping** | `LocalId` → Cranelift `Variable` via `declare_var`/`def_var`/`use_var` |
| **B3: Struct passing** | Small structs (16 bytes or less) in registers; larger by pointer |
| **B4: Object emission** | `cranelift-object` emits ELF (Linux) or Mach-O (macOS) |

## Runtime Library (`rask-rt`)

| Rule | Description |
|------|-------------|
| **RT1: Core always linked** | Allocator, panic, string, Vec, Map, Pool, IO, ensure stack, resource tracker |
| **RT2: Concurrency conditional** | Thread, thread pool, channels, join — linked only when used |
| **RT3: Rust implementation** | Runtime is Rust, compiled to static library |

### Core API

| Component | API |
|-----------|-----|
| **Allocator** | `rask_alloc(size, align) -> *u8`, `rask_dealloc(ptr, size, align)` |
| **Panic** | `rask_panic(msg, len, file, line) -> !` — runs ensure handlers LIFO, exits |
| **String** | `RaskString { ptr, len, cap }` — heap UTF-8 |
| **Vec** | `RaskVec<T>` — monomorphized growable array |
| **Map** | `RaskMap<K,V>` — hash map |
| **Pool** | `RaskPool<T>` — generational sparse storage |
| **IO** | `rask_print(s, len)`, `rask_println(...)` |
| **Ensure stack** | `rask_ensure_push(handler)`, `rask_ensure_pop()`, `rask_ensure_run_all()` |
| **Resource tracker** | `rask_resource_register(type_name, scope) -> u64`, `rask_resource_consume(id)`, `rask_resource_check_scope(scope)` |

### Concurrency API

| Component | API |
|-----------|-----|
| **Thread** | `rask_spawn_raw(func, env) -> ThreadHandle` |
| **Thread pool** | `rask_thread_pool_new(n)`, `rask_thread_pool_submit(pool, func, env)` |
| **Channels** | `rask_channel_buffered(n)`, `rask_channel_unbuffered()` — MPSC, send transfers ownership |
| **Join** | `rask_thread_join(handle) -> Result` |

## C ABI

| Rule | Description |
|------|-------------|
| **C1: Platform ABI** | C interop requires platform-specific calling convention implementation |
| **C2: Rask-to-Rask** | Internal calls may use simpler custom calling convention |
| **C3: FFI boundary** | C ABI only at `unsafe` FFI boundaries |

| Platform | ABI | Register passing |
|----------|-----|-----------------|
| Linux x86_64 | System V AMD64 | 6 integer in rdi/rsi/rdx/rcx/r8/r9, 8 float in xmm0-7 |
| macOS aarch64 | AAPCS64 | 8 integer in x0-x7, 8 float in v0-v7 |
| Windows x86_64 | Microsoft x64 | 4 args in rcx/rdx/r8/r9, shadow space required |

## Linking

| Rule | Description |
|------|-------------|
| **LK1: System linker** | System `cc` handles linking |
| **LK2: Multi-file** | Each `.rk` file compiles to `.o`; all linked with runtime |

```
rask build main.rk -o myapp
  1. Compile: main.rk → main.o (via Cranelift)
  2. Link: cc -o myapp main.o -lrask_rt -lpthread -ldl
```

## Edge Cases

| Case | Handling | Rule |
|------|---------|------|
| Niche optimization for Option | `Option<Ptr>` uses null as None, no tag needed | T4 |
| `@layout(C)` structs | Declaration order preserved, no field reordering | T3 |
| Storable closures | Copy/move captured values into env struct at creation | L6 |
| Expression-scoped closures | Take pointer to enclosing stack frame | L6 |
| Shared instantiation dedup | If packages A and B both instantiate `sort<i32>`, linker should dedup | M5 |
| Cranelift DWARF gaps | `SourceLocation` in MIR provides line/col; full debug info improves with Cranelift or LLVM backend | S4 |

## Error Messages

```
ERROR [comp.codegen/T1]: unresolved generic type `T` in MIR lowering
   |
12 |  func process(x: T) -> T {
   |                 ^ generic type not monomorphized

WHY: All generics must be resolved to concrete types before MIR lowering.

FIX: Ensure all call sites provide concrete type arguments.
```

---

## Appendix (non-normative)

### Rationale

**P1, P2 (MIR as intermediary):** AST is tree-shaped with high-level constructs (ensure, try, pattern matching). Backend IRs are flat CFGs. MIR separates Rask-specific lowering from backend-specific lowering. Adding a second backend is straightforward.

**P2 (non-SSA):** Cranelift's `FunctionBuilder` accepts non-SSA input. LLVM has `mem2reg`. Non-SSA MIR is the simplest form both backends accept.

**P3 (Cranelift first):** Pure Rust, no external dependencies. ~40% faster compilation than LLVM. ~10-15% slower runtime output — acceptable during language development. Industry precedent: Rust adding Cranelift for dev builds, Zig uses LLVM for release + custom backend for dev.

**RT3 (Rust runtime):** Rust stdlib provides threads, channels, IO, allocation. Linking Rust `.a` with Cranelift `.o` is straightforward. No bootstrapping problem. Runtime is small (~2-3k lines); rewrite in Rask when self-hosting.

### Patterns & Guidance

**Implementation roadmap:**

| Step | What compiles | New crates |
|------|--------------|------------|
| 1. Hello World | `println("text")`, function calls, string literals | `rask-codegen`, `rask-rt` |
| 2. Primitives | Integer/float arithmetic, booleans, locals, if/else, loops | — |
| 3. Structs + Enums | Layout, field access, tags, pattern matching, moves | — |
| 4. Collections | String runtime, Vec, Pool + Handle, generation coalescing | — |
| 5. Errors + Resources | Result, `try`, ensure blocks, linear resource tracking | — |
| 6. Generics + Traits | Monomorphization, semantic hash cache, `any Trait` vtables | — |
| 7. Concurrency | Thread spawn, channels, thread pools | — |
| 8. LLVM Backend | `rask build --release` uses LLVM | `rask-codegen-llvm` (optional) |

### Open Issues

1. **C ABI implementation** — Platform-specific calling convention handling for unsafe FFI. Significant per-platform work.
2. **Debug info strategy** — Cranelift DWARF incomplete. Options: print debugging initially, upstream Cranelift DWARF, or prioritize LLVM backend.
3. **Green task runtime** — `spawn { }` requires M:N scheduler. Defer until after basic threading.
4. **Stack overflow detection** — Guard pages or stack probes for deep recursion.

### See Also

- `comp.gen-coalesce` — Generation check coalescing MIR pass
- `comp.semantic-hash` — Semantic hash caching for monomorphization
- `mem.ownership` — Ownership checking (runs before codegen)
- `type.generics` — Generic type system and monomorphization
- `ctrl.comptime` — Comptime evaluation (runs before codegen)
- `struct.build` — Build system and Rust interop
