# Code Generation

## The Question
How does Rask go from type-checked AST to native executable? What sits between AST and machine code? Which backend? What does the runtime provide?

## Decision
Rask uses mid-level IR (MIR) as non-SSA control-flow graph. Monomorphization produces MIR. Rask-specific optimizations run on MIR (generation check coalescing, ensure lowering). MIR lowers to backend IR—Cranelift for dev builds, LLVM optional later for release. Programs link against runtime library (`rask-rt`) written in Rust.

## Rationale

### Why MIR?

AST is tree-shaped with high-level constructs (ensure, try, pattern matching, expression-scoped borrows). Backend IRs (Cranelift, LLVM) are flat CFGs with basic blocks. Direct AST-to-backend lowering conflates two concerns:

1. **Rask-specific lowering** — desugar ensure, try, patterns, resource tracking into control flow
2. **Backend-specific lowering** — emit instructions for Cranelift vs LLVM

MIR separates these. Rask → MIR handles (1), MIR → backend handles (2). Adding a second backend is straightforward. Rask-specific optimization passes have a natural home.

### Why non-SSA?

Cranelift's `FunctionBuilder` accepts non-SSA input—`declare_var`, `def_var`, `use_var` construct SSA internally. LLVM has `mem2reg` promoting stack allocations to SSA registers. Non-SSA MIR is simplest representation both backends accept. Avoids SSA construction complexity in Rask compiler.

### Why Cranelift first?

| Criterion | Cranelift | LLVM (inkwell) |
|-----------|-----------|----------------|
| Build dependency | None (pure Rust crate) | LLVM C++ libs (~hundreds of MB) |
| Compilation speed | ~40% faster than LLVM | Slowest major option |
| Output quality | ~10-15% slower runtime | Best-in-class |
| SSA construction | Handled by FunctionBuilder | Must emit phi nodes or use mem2reg |
| Debug info (DWARF) | Incomplete (improving) | Full support |
| Target platforms | x86_64, aarch64, s390x, riscv64 | Nearly everything |

Cranelift is pure Rust, no external dependencies. `FunctionBuilder` handles SSA construction. Compile speed significantly faster. Output quality gap (~10-15%) acceptable during language development—priority is fast iteration on semantics, not peak runtime performance.

LLVM added later as `rask build --release` for production. MIR is backend-independent. Additive work that doesn't change core pipeline.

**Industry precedent:** Rust adding Cranelift for dev builds. Zig uses LLVM for release + custom backend for dev. Odin building custom dev backend (Tilde). Universal pattern: fast backend for dev, optimizing backend for release.

## Specification

### Full Pipeline

```
Source
  ↓ Lexer
Tokens
  ↓ Parser
AST (Vec<Decl>)
  ↓ Desugar
Desugared AST
  ↓ Resolve
ResolvedProgram (symbols, name→definition mapping)
  ↓ TypeCheck
TypedProgram (type table, type for every node)
  ↓ Ownership Check
OwnershipResult (move/borrow validation)
  ↓ Monomorphize                          ← NEW
MonomorphizedProgram (concrete types only)
  ↓ Lower to MIR                          ← NEW
MirProgram (basic blocks, flat control flow)
  ↓ MIR Optimization Passes               ← NEW
Optimized MirProgram
  ↓ Backend Lowering                       ← NEW
Object file (.o)
  ↓ Link with rask-rt                     ← NEW
Executable
```

### Monomorphization

Monomorphization produces concrete function instances from generic definitions. Runs after type checking and ownership checking, producing fully-typed AST where all generic parameters are replaced with concrete types.

**Process:**

1. **Collect call sites:** Walk reachable code from `main()`. For each generic call, record `(function_id, [concrete_type_args])`.
2. **Instantiate:** For each unique `(function_id, [type_args])`, produce a copy of the function's AST with all type parameters replaced.
3. **Compute layouts:** For each concrete struct and enum type, compute field offsets, sizes, alignments.
4. **Transitively collect:** If an instantiated function calls other generics, add to work queue. Continue until no new instantiations discovered.

**Output:**

```
MonomorphizedProgram {
    functions: Vec<MonoFunction>,    // All concrete functions
    layouts: LayoutTable,            // Size/align/offset for all concrete types
    vtables: Vec<VtableLayout>,      // For `any Trait` dispatch
    entry: FunctionId,               // main()
}

MonoFunction {
    id: MonoFunctionId,
    name: String,                    // Mangled: "sort$i32", "process$Player"
    original_id: FunctionId,         // Back-reference to generic definition
    type_args: Vec<ConcreteType>,    // The concrete types this was instantiated with
    params: Vec<(String, ConcreteType)>,
    return_ty: ConcreteType,
    body: Vec<Stmt>,                 // AST with all types resolved
}
```

**Caching integration:** Semantic hash cache stores monomorphized AST (see [semantic-hash-caching.md](semantic-hash-caching.md)). Cache key is `(function_id, [type_args], body_semantic_hash, [type_definition_hashes])`. Cache hit skips monomorphization entirely.

**`any Trait` (runtime polymorphism):**

`any Trait` values use vtable dispatch, not monomorphization. For each concrete type `T` that satisfies a trait, generate a vtable:

```
VtableLayout {
    trait_name: String,
    concrete_type: ConcreteType,
    methods: Vec<(String, MonoFunctionId)>,  // method name → concrete implementation
    drop_fn: Option<MonoFunctionId>,         // destructor if needed
    size: u32,                               // sizeof(T)
    align: u32,                              // alignof(T)
}
```

An `any Trait` value at runtime is a fat pointer: `(data_ptr, vtable_ptr)`. Method dispatch loads the function pointer from the vtable and calls it.

### MIR Data Structures

#### Types

All MIR types have known sizes. No generics remain.

```
MirType:
    Void                              // ()
    Bool                              // 1 byte
    I8, I16, I32, I64                 // Signed integers
    U8, U16, U32, U64                 // Unsigned integers
    F32, F64                          // IEEE floats
    Ptr                               // Machine-width pointer
    FatPtr                            // (ptr, ptr) — for any Trait, slices
    Struct(StructLayoutId)            // Known-layout aggregate
    Enum(EnumLayoutId)                // Tagged union
    Array { elem: MirType, len: u32 } // Fixed-size array
    FuncPtr(SignatureId)              // Function pointer
```

#### Struct Layout

```
StructLayout {
    id: StructLayoutId,
    name: String,
    size: u32,
    align: u32,
    fields: Vec<FieldLayout>,
}

FieldLayout {
    name: String,
    offset: u32,
    ty: MirType,
    size: u32,
}
```

**Field ordering:** Fields sorted by alignment (largest first) to minimize padding. Matches Rust's default `repr(Rust)` strategy. `@layout(C)` overrides to declaration order for C interop.

#### Enum Layout

```
EnumLayout {
    id: EnumLayoutId,
    name: String,
    tag_type: MirType,                // u8 for ≤256 variants, u16 for more
    size: u32,                        // tag + max payload + padding
    align: u32,
    variants: Vec<VariantLayout>,
}

VariantLayout {
    name: String,
    tag_value: u64,
    payload_offset: u32,              // Offset past tag
    fields: Vec<FieldLayout>,
}
```

**Niche optimization:** `Option<Ptr>` uses null as `None` discriminant—no tag needed, same size as bare pointer. `Option<Handle>` uses sentinel generation value (`0`). Compiler applies niche optimization when type has invalid bit pattern representing `None`.

#### Functions

```
MirFunction {
    id: MonoFunctionId,
    name: String,
    params: Vec<MirLocal>,
    return_ty: MirType,
    locals: Vec<MirLocal>,           // All local variables (including temporaries)
    blocks: Vec<MirBlock>,           // Basic blocks forming the CFG
}

MirLocal {
    id: LocalId,
    name: Option<String>,            // Debug name (None for temporaries)
    ty: MirType,
}

MirBlock {
    id: BlockId,
    stmts: Vec<MirStmt>,
    terminator: MirTerminator,
}
```

#### Statements

```
MirStmt:
    // Variable operations
    Assign { dst: LocalId, value: MirRValue }

    // Memory operations
    Store { addr: MirOperand, offset: u32, value: MirOperand }

    // Function calls
    Call { dst: Option<LocalId>, func: Callable, args: Vec<MirOperand> }

    // Resource tracking (Rask-specific)
    ResourceRegister { dst: LocalId, type_name: String, scope_depth: u32 }
    ResourceConsume { resource_id: MirOperand }
    ResourceScopeCheck { scope_depth: u32 }
    ResourceTransfer { resource_id: MirOperand, new_scope_depth: u32 }

    // Ensure (Rask-specific)
    EnsurePush { cleanup_block: BlockId }
    EnsurePop

    // Pool operations (Rask-specific — target for coalescing optimization)
    PoolCheckedAccess { dst: LocalId, pool: MirOperand, handle: MirOperand }

    // Debug info
    SourceLocation { line: u32, col: u32 }

MirRValue:
    Use(MirOperand)                              // Copy/move a value
    BinaryOp { op: BinOp, left, right }          // Arithmetic, comparison, logic
    UnaryOp { op: UnaryOp, operand }             // Negation, bitwise not
    FieldAccess { base: MirOperand, offset: u32, ty: MirType }
    EnumTag { value: MirOperand }                // Extract discriminant
    EnumPayload { value: MirOperand, variant: u64, ty: MirType }
    Ref { value: LocalId }                       // Take address (block-scoped only)
    Cast { value: MirOperand, from: MirType, to: MirType }
    StructLit { layout: StructLayoutId, fields: Vec<(u32, MirOperand)> }
    EnumLit { layout: EnumLayoutId, tag: u64, fields: Vec<(u32, MirOperand)> }
    ArrayLit { elems: Vec<MirOperand> }
    SizeOf(MirType)
    AlignOf(MirType)

MirOperand:
    Local(LocalId)
    Const(MirConst)

MirConst:
    Bool(bool)
    I64(i64)
    U64(u64)
    F64(f64)
    Str(String)                     // String literal → pointer to static data
    Null                            // null pointer
```

#### Terminators

```
MirTerminator:
    Return(Option<MirOperand>)
    Goto(BlockId)
    Branch { cond: MirOperand, then_block: BlockId, else_block: BlockId }
    Switch { value: MirOperand, cases: Vec<(i64, BlockId)>, default: BlockId }
    Unreachable
    // Ensure-aware return: runs cleanup blocks before returning
    CleanupReturn { value: Option<MirOperand>, cleanup_chain: Vec<BlockId> }
```

### MIR Optimization Passes

Passes run on MIR before backend lowering. All semantics-preserving.

| Pass | What it does | Priority |
|------|-------------|----------|
| **Generation check coalescing** | Merge redundant `PoolCheckedAccess` ops on same (pool, handle) | High — specced in [generation-coalescing.md](generation-coalescing.md) |
| **Dead code elimination** | Remove unreachable blocks, unused assignments | Medium |
| **Constant folding** | Evaluate constant expressions at compile time | Medium |
| **Copy propagation** | Replace `x = y; use(x)` with `use(y)` | Low |
| **Inline small functions** | Inline leaf functions under a size threshold | Low (release only) |

### Lowering: AST → MIR

Each Rask construct has a defined lowering to MIR blocks:

#### If/Else

```rask
if cond {
    then_body
} else {
    else_body
}
```

```
block_entry:
    cond = <evaluate condition>
    branch cond, block_then, block_else

block_then:
    <then_body stmts>
    goto block_merge

block_else:
    <else_body stmts>
    goto block_merge

block_merge:
    <continues>
```

#### Match

```rask
match value {
    Variant1(x) => body1,
    Variant2(y) => body2,
    _ => default_body,
}
```

```
block_entry:
    tag = enum_tag(value)
    switch tag, [0 → block_v1, 1 → block_v2], default → block_default

block_v1:
    x = enum_payload(value, variant=0)
    <body1>
    goto block_merge

block_v2:
    y = enum_payload(value, variant=1)
    <body2>
    goto block_merge

block_default:
    <default_body>
    goto block_merge

block_merge:
    <continues>
```

#### Try (Error Propagation)

```rask
const value = try fallible_call()
```

```
block_try:
    result = call fallible_call()
    tag = enum_tag(result)              // 0 = Ok, 1 = Err
    branch tag == 0, block_ok, block_err

block_ok:
    value = enum_payload(result, variant=0)
    goto block_continue

block_err:
    err = enum_payload(result, variant=1)
    // Run ensure cleanup chain
    ensure_pop  // for each registered ensure
    // ... execute cleanup blocks ...
    return enum_lit(Result.Err, err)

block_continue:
    <rest of function>
```

#### Ensure

```rask
const file = try fs.open("data.txt")
ensure file.close()
const data = try file.read()
```

```
block_open:
    result = call fs_open("data.txt")
    branch is_ok(result), block_opened, block_open_err

block_open_err:
    return result   // propagate, no ensure registered yet

block_opened:
    file = unwrap_ok(result)
    ensure_push(block_cleanup_file)     // register cleanup
    result2 = call file_read(file)
    branch is_ok(result2), block_read_ok, block_read_err

block_read_ok:
    data = unwrap_ok(result2)
    ensure_pop
    goto block_cleanup_file_then_return

block_read_err:
    err = unwrap_err(result2)
    ensure_pop
    goto block_cleanup_file_then_propagate

block_cleanup_file:
    call file_close(file)               // ensure body
    goto <dynamic return target>

block_cleanup_file_then_return:
    call file_close(file)
    return data

block_cleanup_file_then_propagate:
    call file_close(file)
    return enum_lit(Result.Err, err)
```

#### Loops

```rask
for item in collection {
    body
}
```

```
block_loop_init:
    iter = call collection.iter()
    len = call iter.len()
    idx = 0
    goto block_loop_check

block_loop_check:
    cond = idx < len
    branch cond, block_loop_body, block_loop_exit

block_loop_body:
    item = call iter.get(idx)
    <body>
    idx = idx + 1
    goto block_loop_check

block_loop_exit:
    <continues>
```

`break value` in a loop creates a block that assigns the value and jumps to `block_loop_exit`, where the value is available as the loop's result.

#### Closures

Closures lower to struct (captured environment) + function pointer:

```
ClosureLayout {
    env: StructLayout,               // Captured variables
    func: MonoFunctionId,            // The closure body as a function
}
```

Closure function takes `env: Ptr` as first parameter. Captures loaded from env struct. Storable closures copy/move captured values into env struct at creation. Expression-scoped closures take pointer to enclosing stack frame.

### Runtime Library (`rask-rt`)

Runtime is Rust, compiled to static library. Compiled Rask programs link against it. Runtime surface area maps to interpreter's Value enum—every runtime concept the interpreter handles, runtime library provides.

#### Core (always linked)

| Component | API | Notes |
|-----------|-----|-------|
| **Allocator** | `rask_alloc(size, align) -> *mut u8`, `rask_dealloc(ptr, size, align)` | Wraps system allocator. Rask collections use this for all heap allocation. |
| **Panic** | `rask_panic(msg: *const u8, len: usize, file: *const u8, line: u32) -> !` | Prints message, runs registered ensure handlers (LIFO), exits process. |
| **String** | `RaskString` struct: `{ ptr: *mut u8, len: usize, cap: usize }` | Heap-allocated UTF-8. Methods: `new`, `from_static`, `push_str`, `len`, `clone`, `drop`, `eq`, all string methods from the spec. |
| **Vec** | `RaskVec<T>` (monomorphized per element type) | Standard growable array. `push`, `pop`, `get`, `len`, `drop`. Allocation is fallible (returns Result). |
| **Map** | `RaskMap<K,V>` | Hash map. Robin Hood or Swiss Table implementation. |
| **Pool** | `RaskPool<T>` | Generational sparse storage. `insert`, `get`, `remove`, handle validation. |
| **IO** | `rask_print(s: *const u8, len: usize)`, `rask_println(...)`, file wrappers | Wraps OS file descriptors. |
| **Ensure stack** | `rask_ensure_push(handler: fn())`, `rask_ensure_pop()`, `rask_ensure_run_all()` | LIFO cleanup handler registration. Panic handler calls `rask_ensure_run_all()`. |
| **Resource tracker** | `rask_resource_register(type_name, scope) -> u64`, `rask_resource_consume(id)`, `rask_resource_check_scope(scope)` | Runtime enforcement of linear resource consumption. Panics on leak at scope exit. |

#### Concurrency (linked when used)

| Component | API | Notes |
|-----------|-----|-------|
| **Thread** | `rask_spawn_raw(func, env) -> ThreadHandle` | OS thread creation. Closure env is moved to new thread. |
| **Thread pool** | `rask_thread_pool_new(n) -> ThreadPool`, `rask_thread_pool_submit(pool, func, env) -> ThreadHandle` | Worker pool with task queue. Shutdown on `drop`. |
| **Channels** | `rask_channel_buffered(n) -> (Sender, Receiver)`, `rask_channel_unbuffered() -> (Sender, Receiver)` | MPSC channels. Send transfers ownership. |
| **Join** | `rask_thread_join(handle) -> Result` | Blocks until thread completes, returns result or error. |

#### Why Rust?

Runtime is Rust:
1. Rust stdlib provides threads, channels, IO, allocation—no need to reimplement.
2. Cranelift emits object files. Linking Rust `.a` with Cranelift `.o` is straightforward.
3. No bootstrapping problem—runtime doesn't need Rask to compile.
4. Runtime is small (~2-3k lines). When Rask self-hosts, rewrite in Rask.

### Backend Lowering: MIR → Cranelift

Cranelift backend translates MIR to Cranelift IR using `cranelift-frontend` crate.

**Mapping:**

| MIR | Cranelift |
|-----|-----------|
| `MirBlock` | `Block` |
| `LocalId` | `Variable` (via `declare_var` / `def_var` / `use_var`) |
| `MirType::I32` | `types::I32` |
| `MirType::Ptr` | `types::I64` (on 64-bit) |
| `MirType::Struct(id)` | Stack slot (`StackSlot`) with known size |
| `BinaryOp::Add` | `ins().iadd(a, b)` |
| `Call` | `ins().call(func_ref, args)` |
| `Branch` | `ins().brif(cond, then, else)` |
| `Return` | `ins().return_(values)` |
| `FieldAccess` | `ins().load(ty, base, offset)` |
| `Store` | `ins().store(flags, value, addr, offset)` |

**Struct passing:** Small structs (≤16 bytes) passed in registers. Larger structs by pointer (caller allocates stack space, callee receives pointer). Standard calling convention behavior.

**Object emission:** `cranelift-object` emits ELF (Linux) or Mach-O (macOS) object files. Linked with `rask-rt.a` using system linker (`cc`).

### C ABI

Neither Cranelift nor LLVM handles C calling conventions automatically. Rask compiler must implement platform ABI rules for C interop:

| Platform | ABI | Register passing |
|----------|-----|-----------------|
| Linux x86_64 | System V AMD64 | First 6 integer args in rdi, rsi, rdx, rcx, r8, r9. First 8 float args in xmm0-7. |
| macOS aarch64 | AAPCS64 | First 8 integer args in x0-x7. First 8 float args in v0-v7. |
| Windows x86_64 | Microsoft x64 | First 4 args in rcx, rdx, r8, r9. Shadow space required. |

For Rask-to-Rask calls, compiler can use its own (simpler) calling convention. C ABI only needed at `unsafe` FFI boundaries.

### Linking

```
rask build main.rk -o myapp

1. Compile: main.rk → main.o (via Cranelift)
2. Link: cc -o myapp main.o -lrask_rt -lpthread -ldl
```

System linker (`cc`) handles linking. Runtime library precompiled as `librask_rt.a`. `-lpthread` and `-ldl` needed for threading and dynamic loading on Linux.

**Multi-file projects:** Each `.rk` file compiles to `.o` file. All linked together with runtime.

## Examples

### Hello World (Minimum Viable)

```rask
func main() {
    println("Hello, world!")
}
```

**MIR:**
```
func main():
  block0:
    call rask_println("Hello, world!")
    return
```

**Cranelift IR (conceptual):**
```
function %main() {
    block0:
        v0 = iconst.i64 <ptr to "Hello, world!">
        v1 = iconst.i64 13    ; length
        call %rask_println(v0, v1)
        return
}
```

### Struct Field Access

```rask
struct Point { x: i32, y: i32 }

func manhattan(p: Point) -> i32 {
    return p.x + p.y
}
```

**Layout:** `Point { size: 8, align: 4, fields: [x: offset 0, y: offset 4] }`

**MIR:**
```
func manhattan(p: Point) -> i32:
  block0:
    %0 = field_access p, offset=0, ty=i32    // p.x
    %1 = field_access p, offset=4, ty=i32    // p.y
    %2 = add %0, %1
    return %2
```

### Error Propagation

```rask
func read_config(path: string) -> string or IoError {
    const file = try fs.open(path)
    ensure file.close()
    return try file.read_all()
}
```

**MIR:**
```
func read_config(path: RaskString) -> Result<RaskString, IoError>:
  block0:
    result0 = call fs_open(path)
    tag0 = enum_tag(result0)
    branch tag0 == 0, block_opened, block_err0

  block_err0:
    err = enum_payload(result0, variant=1)
    return enum_lit(Result.Err, err)

  block_opened:
    file = enum_payload(result0, variant=0)
    ensure_push(block_cleanup)
    result1 = call file_read_all(file)
    tag1 = enum_tag(result1)
    branch tag1 == 0, block_ok, block_err1

  block_ok:
    data = enum_payload(result1, variant=0)
    ensure_pop
    call file_close(file)
    return enum_lit(Result.Ok, data)

  block_err1:
    err1 = enum_payload(result1, variant=1)
    ensure_pop
    call file_close(file)
    return enum_lit(Result.Err, err1)

  block_cleanup:
    call file_close(file)
    goto <caller>
```

## Integration Notes

- **Semantic Hash Caching:** Monomorphized AST cached. MIR and machine code NOT cached (depend on target and optimization level). See [semantic-hash-caching.md](semantic-hash-caching.md).
- **Generation Check Coalescing:** Implemented as MIR optimization pass on `PoolCheckedAccess` statements. See [generation-coalescing.md](generation-coalescing.md).
- **Type System:** MIR types derived from `Type` enum in `rask-types`. All generics resolved to concrete types by monomorphization before MIR lowering.
- **Ownership:** Ownership checker runs before codegen. By MIR stage, all moves and borrows validated. MIR doesn't recheck ownership—trusts checker results.
- **Concurrency:** Thread spawning in MIR is `Call` to runtime library. Closure environment is struct (captured variables). Channels are opaque handles managed by runtime.
- **Comptime:** Comptime expressions evaluated before codegen (by comptime interpreter). MIR sees only resulting constants.
- **Build System:** `rask build` compiles all `.rk` files to `.o` files, links with `rask-rt`. Build scripts (`rask.build`) run before compilation, may generate additional `.rk` source files.
- **Debug Info:** Cranelift's DWARF support incomplete. `SourceLocation` statements in MIR provide line/column info. Full debug info improves when Cranelift matures or LLVM backend added.

## Implementation Roadmap

Implementation is phased. Each step produces working compiler that handles larger subset of Rask.

| Step | What compiles | New crates |
|------|--------------|------------|
| 1. Hello World | `println("text")`, function calls, string literals | `rask-codegen`, `rask-rt` |
| 2. Primitives | Integer/float arithmetic, booleans, local variables, if/else, loops | — |
| 3. Structs + Enums | Struct layout, field access, enum tags, pattern matching, move semantics | — |
| 4. Collections | String runtime type, Vec, Pool + Handle, generation check coalescing | — |
| 5. Errors + Resources | Result type, `try` lowering, ensure blocks, linear resource tracking | — |
| 6. Generics + Traits | Monomorphization, semantic hash cache integration, `any Trait` vtables | — |
| 7. Concurrency | Thread spawn, channels, thread pools | — |
| 8. LLVM Backend | `rask build --release` uses LLVM | `rask-codegen-llvm` (optional) |

## Remaining Issues

### High Priority
1. **C ABI implementation** — Platform-specific calling convention handling for unsafe FFI. Required for C interop. Significant engineering per platform.
2. **Debug info strategy** — Cranelift's DWARF support incomplete. Need plan for debugging compiled programs. Options: print debugging initially, invest in Cranelift DWARF upstream, or prioritize LLVM backend.

### Medium Priority
3. **Green task runtime** — `spawn { }` requires M:N scheduler. Significant runtime component (work stealing, cooperative scheduling). Can defer until after basic threading works.
4. **Shared instantiation dedup** — If packages A and B both instantiate `sort<i32>`, linker should deduplicate. Affects binary size.
5. **Stack overflow detection** — Need guard pages or stack probes for deep recursion.

### Low Priority
6. **Profile-guided optimization** — Feed runtime profiles back into MIR optimization passes.
7. **Link-time optimization (LTO)** — Cross-module inlining. Possible with LLVM backend.
8. **Cross-compilation** — Emit code for different target than host. Cranelift supports natively.
