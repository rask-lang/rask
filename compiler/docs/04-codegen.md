# Code Generation Deep Dive

How `rask compile` turns a Rask program into a native binary. This covers
the back half of the pipeline: monomorphization, MIR lowering, Cranelift
codegen, and linking.


## Overview

After type checking and ownership verification, the compilation path is:

```
Typed AST
    │
    ▼
Hidden parameter desugaring    rask-hidden-params
    │
    ▼
Monomorphization               rask-mono
    │  - Eliminate generics
    │  - Compute memory layouts
    │  - Tree-shake unreachable code
    ▼
MIR lowering                   rask-mir
    │  - Flatten AST into basic blocks
    │  - Create control-flow graph
    ▼
Cranelift codegen              rask-codegen
    │  - Translate MIR → Cranelift IR
    │  - Emit object file (.o)
    ▼
Link with C runtime           cc runtime.c output.o -o output
    │
    ▼
Native executable
```

You can inspect each intermediate step: `rask mono file.rk` shows
monomorphization output, `rask mir file.rk` shows MIR.


## Monomorphization (`rask-mono`)

**Problem:** The type checker lets you write `func identity<T>(x: T) -> T`.
Machine code doesn't have generics—the CPU needs to know the exact size and
layout of every value. Monomorphization solves this by creating specialized
copies.

**Algorithm:**

1. Start with `main()` as the root.
2. Walk its body, find all function calls.
3. For each call to a generic function, look at `TypedProgram.call_type_args`
   to see which concrete types were used.
4. Create a copy of the function with all type parameters replaced by concrete
   types. This is `instantiate.rs`.
5. Walk the new copy for more calls (BFS).
6. Repeat until no new instances are discovered.

```rust
pub struct MonoProgram {
    pub functions: Vec<MonoFunction>,       // concrete instances
    pub struct_layouts: Vec<StructLayout>,  // memory layout per struct
    pub enum_layouts: Vec<EnumLayout>,      // memory layout per enum
}

pub struct MonoFunction {
    pub name: String,           // mangled name for unique instances
    pub type_args: Vec<Type>,   // which types this was instantiated with
    pub body: Decl,             // the AST with type params replaced
}
```

**Tree shaking:** Only functions reachable from `main()` end up in the output.
If you define 100 functions but only call 3, the binary only contains those 3
(plus anything they call transitively).

### Memory layouts (`layout.rs`)

After monomorphization, every struct and enum has known, concrete field types.
The layout engine computes:

```
struct Point { x: f64, y: f64 }
→ StructLayout {
    name: "Point",
    size: 16,      // total bytes
    align: 8,      // alignment requirement
    fields: [
        FieldLayout { name: "x", ty: f64, offset: 0, size: 8 },
        FieldLayout { name: "y", ty: f64, offset: 8, size: 8 },
    ]
}
```

Enum layouts include a **tag** (discriminant) plus space for the largest
variant's payload:

```
enum Shape { Circle(f64), Rect(f64, f64) }
→ EnumLayout {
    name: "Shape",
    size: 24,       // 8 (tag) + 16 (largest payload)
    align: 8,
    tag_ty: I64,
    variants: [
        VariantLayout { name: "Circle", tag: 0, payload_offset: 8, payload_size: 8 },
        VariantLayout { name: "Rect",   tag: 1, payload_offset: 8, payload_size: 16 },
    ]
}
```


## MIR Lowering (`rask-mir`)

MIR stands for **Mid-level Intermediate Representation**. It sits between the
high-level AST (tree-shaped, close to source) and the low-level machine code.

**Why not go directly from AST to machine code?** Because the AST is deeply
nested—an `if` inside a `match` inside a `for` inside a function. Machine code
is flat: jump here, branch there. MIR bridges the gap.

### What MIR looks like

```rust
pub struct MirFunction {
    pub name: String,
    pub params: Vec<MirLocal>,
    pub ret_ty: MirType,
    pub locals: Vec<MirLocal>,     // all variables + temporaries
    pub blocks: Vec<MirBlock>,     // the control-flow graph
    pub entry_block: BlockId,
}

pub struct MirBlock {
    pub id: BlockId,
    pub statements: Vec<MirStmt>,       // assignments, calls
    pub terminator: MirTerminator,      // branch, jump, return
}
```

A function is a list of **basic blocks**. Each block is a straight-line sequence
of statements ending with a **terminator** that says where to go next.

### Example

Rask source:
```rask
func abs(x: i32) -> i32 {
    if x.lt(0) {
        return x.neg()
    }
    return x
}
```

MIR output:
```
func abs(_0: i32) -> i32:
  bb0:
    _1 = call i32.lt(_0, const 0)
    branch _1 → bb1, bb2

  bb1:
    _2 = call i32.neg(_0)
    return _2

  bb2:
    return _0
```

Each `_N` is a **local** (a temporary or named variable). Locals have types.
The lowering logic creates new locals as needed for intermediate values.

### Lowering process

The lowering happens in `rask-mir/src/lower/`:

- **`mod.rs`** — Top-level: create `MirLowerer`, add parameters as locals,
  lower the function body, add implicit void return if needed.
- **`stmt.rs`** — Statements become MIR statements + terminators. An `if`
  creates three blocks (condition check → then → else → merge). A `while`
  creates a loop header, body, and exit block.
- **`expr.rs`** — Expressions return `(MirOperand, MirType)`. A literal
  becomes a constant operand. A call becomes a `MirStmt::Call` that stores
  the result in a fresh local.

**`MirContext`** provides layout information:

```rust
pub struct MirContext<'a> {
    pub struct_layouts: &'a [StructLayout],
    pub enum_layouts: &'a [EnumLayout],
    pub node_types: &'a HashMap<NodeId, Type>,
}
```

The lowerer uses this to resolve type strings to struct/enum IDs, look up field
offsets, and determine value sizes.

### Loop lowering

Loops create a **loop context** that tracks:
- `continue_block`: where `continue` jumps to (loop header)
- `exit_block`: where `break` jumps to (after the loop)
- `result_local`: for loops that produce a value via `break value`


## Cranelift Codegen (`rask-codegen`)

Cranelift is a code generator library, like a smaller LLVM. It takes an
intermediate representation and produces machine code.

### The compilation steps

In `cmd_compile()` (codegen.rs):

```rust
// 1. Create a CodeGenerator (wraps Cranelift module)
let mut codegen = CodeGenerator::new()?;

// 2. Declare C runtime functions (print, exit, malloc, etc.)
codegen.declare_runtime_functions()?;
codegen.declare_stdlib_functions()?;

// 3. Declare all Rask functions
codegen.declare_functions(&mono, &mir_functions)?;

// 4. Register string literals as global data
codegen.register_strings(&mir_functions)?;

// 5. Generate machine code for each function
for mir_fn in &mir_functions {
    codegen.gen_function(mir_fn)?;
}

// 6. Emit object file
codegen.emit_object("output.o")?;

// 7. Link with C runtime
link_executable("output.o", "output")?;
```

### Function generation (`builder.rs`)

For each MIR function, the builder:

1. Pre-computes stack allocations for aggregate types (structs, enums).
2. Creates Cranelift function with parameter and return types.
3. Maps MIR locals → Cranelift variables, MIR blocks → Cranelift blocks.
4. Translates each MIR block:
   - Statements → Cranelift instructions
   - Terminators → branch/jump/return instructions
5. Finalizes and verifies the Cranelift IR.

String literals become global data references. When MIR contains a string
constant, the builder creates a `GlobalValue` pointing to the string data.

### Closure support (`closures.rs`)

Closures are compiled as 16-byte structs:
- `[0..8]` — function pointer
- `[8..16]` — environment pointer

The environment is a stack-allocated block containing captured variables.
`ClosureEnvLayout` tracks each captured variable's name, offset, and type.

### Method dispatch (`dispatch.rs`)

The codegen layer maintains a dispatch table mapping MIR method names to C
runtime functions. For example, `Vec.push` maps to `rask_vec_push` in the C
runtime.


## Linking (`rask-cli/src/commands/link.rs`)

After code generation produces an object file, it's linked with the C runtime:

```
cc runtime.c args.c output.o -o output -no-pie
```

The runtime is found via:
1. `RASK_RUNTIME_DIR` environment variable, or
2. `../runtime/` relative to the `rask` binary

The C runtime (`runtime/runtime.c`) provides:
- Memory allocation (malloc/free wrappers)
- String operations
- Vec, Map, Pool implementations
- CLI argument handling
- I/O functions

The `.o` file is cleaned up after linking.


## The `rask compile` vs `rask build` distinction

**`rask compile file.rk`** — Single-file compilation. Runs the full pipeline
on one file, produces a native binary.

**`rask build`** — Multi-package build. Reads `build.rk` for package metadata,
discovers dependencies, resolves them, type-checks everything, then compiles
the root package. Supports profiles (`--release`), cross-compilation targets
(`--target`), and build scripts (a `func build()` in `build.rk` runs via the
interpreter).

**`rask run --native file.rk`** — Compiles to a temp binary, executes it,
deletes it. Useful for testing the native codegen path without leaving
artifacts.
