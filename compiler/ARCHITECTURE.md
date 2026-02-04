# Rask Compiler Architecture Design

## Overview

Three-phase evolution: **Tree-Walk Interpreter → Bytecode VM → Native Codegen**

This design prioritizes validating language semantics before optimizing performance.

---

## Current State

| Component | Status | Notes |
|-----------|--------|-------|
| Lexer | Complete | `rask-lexer`, uses `logos` |
| Parser | Complete | Pratt parser, all examples parse |
| AST | Complete | Full expression/statement/declaration types |
| Type System | Stub | Types defined, no checking |
| Interpreter | Stub | Returns `Unit` |

---

## Phase 1: Tree-Walk Interpreter

**Goal:** Validate language design. First target: `simple_test.rask`, then `grep_clone.rask`.

### 1.1 Compiler Pipeline

```
Source → Lexer → Parser → AST
                           ↓
                    Name Resolution
                           ↓
                    Type Inference
                           ↓
                   Ownership Analysis
                           ↓
                    Interpretation
```

### 1.2 Name Resolution Pass

**Purpose:** Resolve all identifiers to their declarations.

```rust
// rask-resolve/src/lib.rs
pub struct Resolver {
    scopes: Vec<HashMap<String, Symbol>>,
    current_function: Option<FuncId>,
}

pub enum Symbol {
    Variable { id: VarId, ty: Option<Type>, mutable: bool },
    Function { id: FuncId, signature: FuncSignature },
    Type { id: TypeId, definition: TypeDef },
    Module { path: ModulePath },
}
```

**Output:** AST annotated with `Symbol` references.

### 1.3 Type Inference Pass

**Purpose:** Infer and check all types.

```rust
// rask-typecheck/src/lib.rs
pub struct TypeChecker {
    types: TypeTable,
    constraints: Vec<TypeConstraint>,
    substitutions: HashMap<TypeVar, Type>,
}

pub enum TypeConstraint {
    Equal(Type, Type),
    HasMethod { ty: Type, method: String, signature: FuncSignature },
    Satisfies { ty: Type, trait_: TraitId },
}
```

**Key behaviors:**
- Operator desugaring: `a + b` → `a.add(b)` before type checking
- Structural trait satisfaction at instantiation site
- Monomorphization: each generic instantiation generates specialized types
- Const generic inference from array literals

### 1.4 Ownership Analysis Pass

**Purpose:** Verify memory safety statically.

```rust
// rask-ownership/src/lib.rs
pub struct OwnershipChecker {
    bindings: HashMap<VarId, BindingState>,
    borrows: Vec<ActiveBorrow>,
    linear_obligations: Vec<LinearObligation>,
}

pub enum BindingState {
    Owned,
    Moved { at: Span },
    Borrowed { mode: BorrowMode, scope: ScopeId },
}

pub enum BorrowMode {
    Shared,    // &T - multiple allowed
    Exclusive, // &mut T - single only
}

pub struct ActiveBorrow {
    source: VarId,
    mode: BorrowMode,
    scope: BorrowScope,
}

pub enum BorrowScope {
    Persistent { until_block_end: BlockId },  // strings, struct fields
    Instant { until_semicolon: StmtId },      // Vec, Map, Pool
}
```

**Analysis rules:**

1. **Move tracking:**
   - Assignment of non-Copy type → source becomes `Moved`
   - Use of `Moved` binding → error

2. **Borrow classification:**
   - Collection types (Vec, Map, Pool) → `Instant` scope
   - Fixed types (String, struct fields) → `Persistent` scope

3. **Linear resource types:**
   - `@resource` values must be consumed on all paths
   - Consumption via: `take` params, `consume()` methods, `ensure` registration

4. **Pool handles:**
   - Generation check at each `pool[handle]` access
   - Coalesce checks within same expression

### 1.5 Comptime Execution

**Purpose:** Execute compile-time code (included from the start).

```rust
// rask-comptime/src/lib.rs
pub struct ComptimeInterpreter {
    env: ComptimeEnv,
}

pub enum ComptimeValue {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(Vec<ComptimeValue>),
    Tuple(Vec<ComptimeValue>),
}
```

**Restrictions:**
- No I/O, no heap allocation (arrays OK)
- No pool/handle operations
- No concurrency
- Only pure computation: arithmetic, conditionals, loops, function calls

**Integration:** Runs after type checking, before ownership analysis. Evaluates `comptime` blocks and substitutes results into AST.

### 1.6 Tree-Walk Interpreter

**Purpose:** Execute validated AST directly.

```rust
// rask-interp/src/lib.rs
pub struct Interpreter {
    env: Environment,
    pools: HashMap<PoolId, Pool>,
    call_stack: Vec<CallFrame>,
}

pub struct Environment {
    scopes: Vec<Scope>,
}

pub struct Scope {
    bindings: HashMap<VarId, Value>,
    ensures: Vec<EnsureBlock>,
}

pub enum Value {
    Unit,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(RaskString),
    Struct { type_id: TypeId, fields: HashMap<String, Value> },
    Enum { type_id: TypeId, variant: String, data: Option<Box<Value>> },
    Vec(Vec<Value>),
    Handle { pool_id: PoolId, index: usize, generation: u64 },
    Closure { params: Vec<VarId>, body: Expr, captures: HashMap<VarId, Value> },
    // ...
}
```

---

## Phase 2: Bytecode VM (Post-Validation)

**Goal:** 5-20x speedup over tree-walk, clearer ownership semantics.

### 2.1 Bytecode IR

```rust
// rask-bytecode/src/lib.rs
pub enum Opcode {
    // Values
    LoadConst(ConstId),
    LoadLocal(LocalId),
    StoreLocal(LocalId),

    // Ownership (explicit in bytecode)
    Move(LocalId),           // Transfer ownership, invalidate source
    Borrow(LocalId),         // Expression-scoped borrow
    BorrowMut(LocalId),      // Mutable borrow

    // Structs
    StructNew(TypeId),
    FieldGet(FieldId),
    FieldSet(FieldId),

    // Collections
    VecNew,
    VecPush,
    VecGet,                  // Expression-scoped borrow
    VecLen,

    // Pools
    PoolNew(TypeId),
    PoolInsert,              // Returns Handle
    PoolGet,                 // Validates generation, expression-scoped
    PoolRemove,

    // Control flow
    Jump(BlockId),
    JumpIf(BlockId),
    JumpIfNone(BlockId),     // For Option handling
    Match(Vec<BlockId>),

    // Calls
    Call(FuncId),
    Return,

    // Linear resources
    EnsureRegister(CleanupId),
    Consume(LocalId),        // Verify linear type consumed
}

pub struct Function {
    name: String,
    params: Vec<(LocalId, TypeId)>,
    locals: Vec<(LocalId, TypeId)>,
    blocks: Vec<BasicBlock>,
}

pub struct BasicBlock {
    id: BlockId,
    ops: Vec<Opcode>,
    terminator: Terminator,
}
```

### 2.2 Bytecode VM

```rust
// rask-vm/src/lib.rs
pub struct VM {
    stack: Vec<Value>,
    frames: Vec<CallFrame>,
    pools: HashMap<PoolId, PoolData>,
    heap: Heap,
}

pub struct CallFrame {
    func: FuncId,
    ip: usize,
    block: BlockId,
    locals: Vec<Value>,
    ensures: Vec<CleanupId>,
}
```

---

## Phase 3: Native Codegen (Production)

**Goal:** Near-C performance via LLVM or Cranelift.

### 3.1 Why Cranelift over LLVM

| Factor | Cranelift | LLVM |
|--------|-----------|------|
| Compile speed | Fast | Slow |
| Code quality | Good (80-90% of LLVM) | Best |
| Rust integration | Native | FFI |
| Complexity | Lower | Higher |

**Recommendation:** Start with Cranelift for faster iteration.

### 3.2 Codegen Strategy

Ownership already verified in earlier phases, so codegen is mechanical:
- `Move` → transfer pointer ownership
- `Borrow` → pass address
- Pool access → inline generation check
- Linear types → already verified consumed

---

## Key Design Decisions

### Local Analysis Only

Per Rask's design principle: **no whole-program analysis**.

- Function signatures fully describe interfaces
- Generics checked at instantiation site
- No lifetime parameters in signatures
- Borrow checking per-function only

### Two Borrow Scopes

| Type | Scope | Examples |
|------|-------|----------|
| Persistent | Block end | `String`, struct fields, arrays |
| Instant | Semicolon | `Vec`, `Map`, `Pool` |

**Classification:** At borrow creation, check if source is a "growable" collection.

### 16-Byte Copy Threshold

Types ≤16 bytes with all-Copy fields are implicitly copyable:
- Primitives, small structs → copy on assignment
- Larger types → move on assignment

### Pool Generation Checking

```rust
pub struct Handle<T> {
    index: u32,
    generation: u32,
}

pub struct Pool<T> {
    entries: Vec<Option<T>>,
    generations: Vec<u32>,
    free_list: Vec<u32>,
}

// Access validation (runtime)
fn get(&self, handle: Handle<T>) -> &T {
    assert!(self.generations[handle.index] == handle.generation, "stale handle");
    self.entries[handle.index].as_ref().unwrap()
}
```

---

## Implementation Order

### Step 1: Name Resolution
- [ ] Create `rask-resolve` crate
- [ ] Build scope stack
- [ ] Resolve all identifiers
- [ ] Report undefined/duplicate errors

### Step 2: Type System Core
- [ ] Create `rask-typecheck` crate
- [ ] Implement type unification
- [ ] Basic inference for let/const
- [ ] Function signature checking

### Step 3: Operator Desugaring
- [ ] Transform `a + b` → `a.add(b)`
- [ ] All arithmetic, comparison, assignment ops
- [ ] Before type checking

### Step 4: Comptime Execution
- [ ] Create `rask-comptime` crate
- [ ] Evaluate comptime blocks
- [ ] Substitute results into AST
- [ ] Enforce comptime restrictions (no I/O, no pools)

### Step 5: Trait Checking
- [ ] Structural satisfaction (not nominal)
- [ ] Method signature matching
- [ ] Generic instantiation verification

### Step 6: Ownership Analysis
- [ ] Create `rask-ownership` crate
- [ ] Move tracking
- [ ] Borrow scope analysis
- [ ] Linear type verification

### Step 7: Basic Interpreter
- [ ] Extend `rask-interp`
- [ ] Evaluate expressions
- [ ] Execute statements
- [ ] Function calls

### Step 8: Milestone: simple_test.rask
- [ ] All basic features working
- [ ] Functions, structs, control flow
- [ ] Basic print output

### Step 9: Collections & Pools
- [ ] Vec operations
- [ ] Map operations
- [ ] Pool with generation checks

### Step 10: Milestone: grep_clone.rask
- [ ] Imports, Result types
- [ ] Pattern matching
- [ ] Full validation target

---

## File Structure

```
compiler/crates/
├── rask-lexer/        # Done
├── rask-parser/       # Done
├── rask-ast/          # Done
├── rask-resolve/      # NEW: name resolution
├── rask-typecheck/    # NEW: type inference + checking
├── rask-comptime/     # NEW: compile-time execution
├── rask-ownership/    # NEW: ownership/borrow analysis
├── rask-types/        # Extend: type definitions
├── rask-interp/       # Extend: tree-walk interpreter
├── rask-bytecode/     # FUTURE: bytecode IR
├── rask-vm/           # FUTURE: bytecode VM
├── rask-codegen/      # FUTURE: native codegen
├── rask-cli/          # Extend: add `run` command
└── rask-lsp/          # FUTURE: language server
```

---

## Verification Plan

1. **Unit tests:** Each pass has isolated tests
2. **Integration tests:** Full pipeline on example files
3. **Validation programs:**
   - `simple_test.rask` - basic functions, structs
   - `grep_clone.rask` - comprehensive (the main litmus test)
   - All 7 examples in `examples/`

---

## Decisions Made

| Question | Decision |
|----------|----------|
| Comptime | Include from start, separate interpreter with restrictions |
| Error messages | Minimal: "value moved here", "cannot borrow" |
| First target | `simple_test.rask`, then `grep_clone.rask` |
| Incremental compilation | Defer to Phase 2 |
