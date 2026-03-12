<!-- id: comp.architecture -->
<!-- status: proposed -->
<!-- summary: Target compiler architecture — IR layers, analysis framework, pass pipeline, CTFE, debug info -->
<!-- depends: compiler/codegen.md, compiler/advanced-analyses.md, compiler/clone-elision.md, compiler/string-refcount-elision.md, compiler/incremental.md, compiler/effects.md -->

# Compiler Architecture

The compiler grew feature-by-feature. This spec defines the target architecture so that string RC optimization, SSO strings, MIR-based CTFE, reflection, debugging, borrow analysis, effects, incremental compilation, and parallel compilation all have clean extension points — not bolted on after the fact.

I'm writing this now because every one of those features touches the MIR layer. If MIR's structure is wrong, every feature fights it. Getting the bones right means each feature is a pass, not a rewrite.

---

## IR Layers

| Rule | Description |
|------|-------------|
| **IR1: Three representations** | Source → AST → MIR (SSA) → machine code (via Cranelift/LLVM). No HIR — AST is rich enough after desugaring |
| **IR2: MIR is the optimization target** | All Rask-specific optimizations (string RC, clone elision, generation coalescing, typestate) run on MIR |
| **IR3: Hybrid SSA** | MIR lowering produces non-SSA form (variables as slots). Immediate SSA conversion before optimization. De-SSA before codegen |
| **IR4: Source spans everywhere** | Every MIR statement and terminator carries a `Span`. Lossless source mapping from AST through MIR to machine code |
| **IR5: Serializable MIR** | MIR types are serializable for incremental compilation caching (`comp.incremental/IC5`) |

### Why Hybrid SSA (IR3)

Non-SSA is easier to generate during lowering — each variable maps to a mutable slot, no phi insertion needed. But optimization passes need SSA's single-definition property:

- String RC insertion needs precise def-use chains for drop placement
- Constant propagation needs single definitions to track values
- Escape analysis needs to follow definitions to uses
- Interval analysis explicitly assumes SSA form (`comp.advanced/IV3`)

I lower to non-SSA first (simpler lowering), convert to SSA immediately (one pass), optimize in SSA, then de-SSA for codegen. This is the same approach as LLVM's `mem2reg` and GCC's SSA construction. Cranelift does its own SSA construction internally, so de-SSA before Cranelift is just "lower phi nodes to copies in predecessor blocks" — straightforward.

**Supersedes** `comp.codegen/P2` ("MIR uses non-SSA form"). MIR is non-SSA only during initial lowering; the canonical optimization form is SSA.

### MIR Span Extension

```
// Current
pub enum MirStmt {
    Assign { dst: LocalId, value: MirRValue },
    Call { dst: Option<LocalId>, func: String, args: Vec<MirOperand> },
    ...
}

// Target
pub struct MirStmt {
    pub kind: MirStmtKind,
    pub span: Span,       // source location — always present
}
```

Same for `MirTerminator`. Every IR node carries provenance.

---

## Cleanup and Resource Tracking

| Rule | Description |
|------|-------------|
| **CL1: Ensure in MIR** | `ensure` blocks lower to `EnsurePush { cleanup_block }` / `EnsurePop` statements with `CleanupReturn` terminators. Already implemented — no structural change needed |
| **CL2: Cleanup chains** | `CleanupReturn` carries a `cleanup_chain: Vec<BlockId>` — the LIFO stack of ensure blocks to execute on scope exit. MIR lowering builds the chain; codegen emits the cleanup sequence |
| **CL3: SSA interaction** | Cleanup blocks are side exits — they don't produce values that flow back into the main CFG. SSA conversion treats cleanup blocks as separate regions. Phi nodes are never inserted at cleanup block boundaries |
| **CL4: RC interaction** | `RcDec` operations must run *before* cleanup blocks execute (ensure handlers may reference the values being decremented). RC drop placement respects cleanup chain ordering: RC drops go in normal blocks, not cleanup blocks |
| **CL5: Resource tracking in MIR** | `@resource` types have dedicated MIR statements: `ResourceRegister` (track new resource), `ResourceConsume` (mark consumed), `ResourceScopeCheck` (verify consumed before scope exit). Already implemented |
| **CL6: Resource vs RC** | Resource types are NOT refcounted — they are linear (must-consume). String RC skips `@resource` types entirely. The two systems are orthogonal: RC handles shared-ownership values (strings), resource tracking handles exactly-once values (files, connections) |
| **CL7: Ensure in MIR CTFE** | The MIR interpreter handles cleanup chains — executes ensure blocks on scope exit, respecting LIFO order |

---

## Closure Compilation

| Rule | Description |
|------|-------------|
| **CC1: Capture inference** | Compiler infers capture strategy per variable: copy (≤16 bytes Copy), move (large/non-Copy), or mutable borrow (`mutate` keyword in capture list). No annotation except `mutate` |
| **CC2: Escape analysis determines allocation** | Non-escaping closures (inline callbacks, iterator adapters) are stack-allocated. Escaping closures (stored, returned, sent cross-task) are heap-allocated |
| **CC3: MIR representation** | `ClosureCreate` builds the environment. `ClosureCall` invokes through it. `ClosureDrop` frees heap closures. Already in MIR |
| **CC4: Cross-function pass** | Closure escape analysis is a cross-function pass (PC2) — needs to see all call sites to determine whether a closure escapes |
| **CC5: RC interaction** | Heap closures capturing strings need RC ops on captured values. The RC pass inserts `RcInc` when building the environment and `RcDec` when dropping it |

---

## Unsafe Blocks

| Rule | Description |
|------|-------------|
| **UB1: Validated at typecheck** | `unsafe` block scoping enforced during type checking. Unsafe operations outside `unsafe {}` are compile errors |
| **UB2: Debug-mode checks** | In debug builds, raw pointer operations emit runtime bounds/null checks (`mem.unsafe/D1-D5`). These lower to conditional panics in MIR |
| **UB3: Release-mode elision** | In release builds, debug-mode pointer checks are elided. `BuildMode` in codegen context controls this |
| **UB4: No special MIR form** | Unsafe blocks don't have a separate MIR representation — the safety boundary is the type checker's job. In MIR, raw pointer ops are just statements |

---

## Pattern Matching

| Rule | Description |
|------|-------------|
| **PM1: Exhaustiveness at typecheck** | Pattern exhaustiveness checking runs during type checking. Missing patterns are compile errors |
| **PM2: Decision trees in MIR** | `match` lowers to decision trees — nested `Branch`/`Switch` terminators. `match_lower.rs` handles this |
| **PM3: Enum dispatch** | `EnumTag` rvalue extracts discriminant. `Switch` dispatches on it. Payload destructuring uses field access at computed offsets |
| **PM4: Optimization opportunity** | Jump threading and redundant branch merging are future MIR passes that improve match codegen |

---

## Allocator Contexts

| Rule | Description |
|------|-------------|
| **AL1: Desugared by hidden params** | `using Allocator` and `using Arena.scoped(...)` desugar via the hidden parameters pass (`comp.hidden-params`). The allocator becomes an explicit `__ctx_allocator` parameter |
| **AL2: Collection dispatch** | Collections resolve to allocator-aware runtime functions when an allocator context is active. MIR lowering passes the allocator parameter to allocation calls |
| **AL3: Scoped arena** | `using Arena.scoped(size) { ... }` desugars to: allocate arena → set as context → execute body → free arena. Hidden params handles scoping; MIR lowering emits alloc/dealloc |

---

## Concurrency in Pipeline

| Rule | Description |
|------|-------------|
| **CO1: Spawn is a call** | `spawn(|| { ... })` is a function call, not a special MIR construct. The closure is built via CC1-CC5, the spawn call goes through normal dispatch |
| **CO2: Channel ops are calls** | `channel.send()`/`channel.recv()` are stdlib calls. No special MIR representation |
| **CO3: Multitasking context** | `using Multitasking { ... }` desugars via hidden params — the runtime executor is an implicit parameter. MIR sees it as a regular context argument |
| **CO4: State machine transform** | Async functions that need suspension lower to state machines via `transform/state_machine.rs`. This is a MIR pass, already implemented |

---

## MirProgram Wrapper

| Rule | Description |
|------|-------------|
| **MP1: Unified context** | `MirProgram` bundles `Vec<MirFunction>` with shared metadata: file table, struct/enum layouts, type metadata, call graph |
| **MP2: Replaces ad-hoc threading** | Currently, layouts and type info are threaded separately through pipeline functions. `MirProgram` is the single context object |
| **MP3: Pass manager operates on MirProgram** | `PassManager::run(&self, program: &mut MirProgram)` — passes access cross-function data (call graph, layouts) and per-function data |

```rust
pub struct MirProgram {
    pub functions: Vec<MirFunction>,
    pub file_table: Vec<String>,
    pub struct_layouts: Vec<StructLayout>,
    pub enum_layouts: Vec<EnumLayout>,
    pub type_metadata: Vec<TypeMeta>,       // for reflection/debug
    pub call_graph: Option<CallGraph>,      // built on demand by cross-function passes
}
```

---

## Dataflow Analysis Framework

| Rule | Description |
|------|-------------|
| **DF1: Generic solver** | A single dataflow engine parameterized by lattice, transfer function, and direction |
| **DF2: Forward and backward** | Supports forward (reaching definitions, typestate) and backward (liveness) analyses |
| **DF3: Fixpoint iteration** | Iterates until lattice values stabilize. Widening for loops |
| **DF4: Per-function** | All analyses are intraprocedural. Cross-function information via summaries, not whole-program analysis |
| **DF5: Cached results** | Analysis results cached per function, invalidated when function changes |
| **DF6: Demand-driven option** | Analyses can be lazy — computed only when queried (interval analysis, `comp.advanced/IV1`) |

### Framework Interface

```rust
pub trait DataflowAnalysis {
    type Domain: Clone + Eq;
    fn direction() -> Direction;        // Forward or Backward
    fn bottom() -> Self::Domain;        // lattice bottom
    fn join(a: &Self::Domain, b: &Self::Domain) -> Self::Domain;
    fn transfer_stmt(stmt: &MirStmt, state: &mut Self::Domain);
    fn transfer_terminator(term: &MirTerminator, state: &mut Self::Domain);
}

pub struct DataflowResults<A: DataflowAnalysis> {
    pub entry: HashMap<BlockId, A::Domain>,
    pub exit: HashMap<BlockId, A::Domain>,
}

pub fn solve<A: DataflowAnalysis>(func: &MirFunction, analysis: &A) -> DataflowResults<A>;
```

### Concrete Analyses

| Analysis | Direction | Domain | Used By |
|----------|-----------|--------|---------|
| **Liveness** | Backward | `BitSet<LocalId>` (set of live locals) | RC drop placement, clone elision, DCE, register hints |
| **Reaching definitions** | Forward | `Map<LocalId, Set<DefPoint>>` | Constant propagation, copy propagation |
| **Handle typestate** | Forward | `Map<HandleLocal, {Fresh,Valid,Unknown,Invalid}>` | `comp.advanced/TS1-TS8` |
| **Escape analysis** | Forward | `Map<LocalId, {Local,MayEscape,Escaped}>` | String refcount elision (`comp.string-refcount-elision/RE2`) |
| **Interval analysis** | Forward (demand-driven) | `Map<LocalId, [lo, hi]>` | Bounds check elimination (`comp.advanced/BE1-BE4`) |

All five share the same solver. Adding a new analysis means implementing the trait — the iteration, caching, and invalidation are free.

---

## SSA Construction

| Rule | Description |
|------|-------------|
| **SSA1: Dominance-based** | SSA conversion uses the iterated dominance frontier algorithm (Cytron et al. 1991) |
| **SSA2: Phi nodes** | Phi nodes inserted at dominance frontiers for each variable live across a join point |
| **SSA3: Pruned SSA** | Only insert phi nodes where the variable is actually live (uses liveness analysis) — avoids bloated IR |
| **SSA4: De-SSA** | Before codegen, phi nodes are lowered to copies in predecessor blocks. Cranelift handles its own SSA, so this is mechanical |

### Required Infrastructure

- **Dominator tree** — `analysis/dominators.rs`. Lengauer-Tarjan or simple iterative algorithm. Needed for SSA, loop detection, and typestate
- **Dominance frontiers** — computed from dominator tree. Needed for phi insertion
- **Loop detection** — natural loops from back-edges in dominator tree. Needed for widening in dataflow, loop optimization

---

## String Refcount Optimization

`string` is the only refcounted type in Rask. Everything else is Copy, Move, or linear (`@resource`). The spec explicitly closes the door on general user-defined RC types (`comp.string-refcount-elision`, "Why This Is String-Only"). This keeps the RC story simple — one type, known layout, known immutability.

### Rules

| Rule | Description |
|------|-------------|
| **RC1: Explicit RC operations** | After SSA conversion, insert explicit `RcInc` and `RcDec` MIR statements for string-typed locals |
| **RC2: Precise drop placement** | `RcDec` placed at each last-use point (from liveness analysis), not at scope exit |
| **RC3: Inc/dec fusion** | Adjacent or provably paired inc/dec cancel out (`comp.string-refcount-elision/RE1`). This is the highest-value optimization — most string copies are immediately followed by a drop of the original |
| **RC4: Local-only elision** | Strings that don't escape the function skip all RC ops (`comp.string-refcount-elision/RE2`). Refcount stays at 1, just free on drop |
| **RC5: Literal propagation** | Strings provably tracing back to literals skip all RC ops (`comp.string-refcount-elision/RE3`). Literals use a sentinel refcount |
| **RC6: Buffer reuse** | When `RcDec` drops a string to zero and a same-capacity allocation follows, reuse the buffer instead of free+malloc. Allocators use size classes (e.g., 24-byte and 30-byte strings both get 32-byte blocks), so capacity matching is more common than exact length matching. Many string operations (replace, trim, case conversion) produce similar-capacity output — reuse turns a deallocation + allocation into a pointer swap |

### Scope Constraints

| Constraint | Reason |
|-----------|--------|
| **String-only** | No other types are refcounted. No general RC framework |
| **No RC on collections** | Vec, Map are single-owner (move semantics). `.clone()` is explicit deep copy. Clone elision handles this separately |
| **Reuse is capacity-class only** | Buffer reuse (RC6) matches on allocator size class, not exact length. No cross-type reuse |

### Pipeline Position

```
MIR Lowering → SSA Conversion → String RC Insertion → RC Fusion/Elision/Reuse → Other Passes → De-SSA → Codegen
```

RC insertion runs after SSA because it needs precise def-use chains. It runs before other optimization passes so they can see (and potentially eliminate) the RC operations.

### Interaction with SSO

When SSO is implemented (`string` inline for ≤15 bytes), RC operations become conditional:

```
// Pseudo-MIR
if string.is_heap() {
    RcDec(string)
}
```

SSO awareness lives in codegen, not MIR — codegen emits the tag check before each RC operation. This keeps MIR clean (no SSO conditionals) while avoiding unnecessary atomic ops at runtime. String literals are statically known to be inline or heap based on length, so codegen elides the tag check entirely for literals ≤15 bytes.

### What This Needs

- **Liveness analysis** (DF framework) — for drop placement (RC2)
- **Escape analysis** (DF framework) — for local-only elision (RC4)
- **SSA form** — for precise def-use tracking (RC1, RC3)
- **Literal tracking** — forward dataflow to propagate "provably literal" through copies (RC5)

All four are useful for other purposes too. The string RC pass is a client of the analysis framework, not a standalone system.

---

## MIR-Based CTFE (Compile-Time Function Evaluation)

| Rule | Description |
|------|-------------|
| **CT1: MIR interpreter** | Comptime evaluation runs on MIR, not AST. Same semantics as compiled code, guaranteed |
| **CT2: Virtual memory model** | The interpreter simulates a stack + heap. Allocations are tracked, freed on scope exit |
| **CT3: Stdlib dispatch** | Stdlib calls route through a trait — comptime uses pure implementations (no I/O), runtime uses real implementations |
| **CT4: Step limit** | Backwards branch quota (`ctrl.comptime/CT7`) enforced by counting executed terminators |
| **CT5: Debug stepping** | Each MIR statement is a step. Comptime debugger hooks in here (post-v1.0) |
| **CT6: Replaces AST interpreter for comptime** | `rask-interp` stays for `rask run` scripting mode. Comptime switches to MIR interpreter |

### Why Not One Interpreter for Everything?

The AST interpreter has faster startup (skip mono + MIR lowering) which matters for scripting. For comptime, correctness matters more than startup — MIR interpretation guarantees the same behavior as compiled code.

Long-term, the AST interpreter can be retired once MIR interpretation is fast enough for scripting. But that's not a priority — it works fine today.

### Structure

New crate: `rask-miri`

```
rask-miri/src/
  lib.rs          — MiriEngine: execute(func, args) -> Value
  memory.rs       — virtual heap + stack frames, allocation tracking
  eval.rs         — statement/terminator execution loop
  intrinsics.rs   — arithmetic, comparisons, casts
  stdlib.rs       — StdlibProvider trait for I/O dispatch
```

Comptime uses `MiriEngine` with a `PureStdlib` provider (no I/O, errors on syscalls). Future `rask run` migration would use `RealStdlib` provider.

---

## Effect System

| Rule | Description |
|------|-------------|
| **EF1: Frozen is enforced** | `using frozen Pool<T>` violations are compile errors, not lint warnings. This enables guaranteed optimization (skip generation checks in frozen iteration) |
| **EF2: Computed during typechecking** | Effect signatures computed alongside types. `rask-effects` becomes a library called by the type checker, not a separate pipeline stage |
| **EF3: Attached to function signatures** | `EffectSignature` stored in `TypedProgram` per function. MIR passes can query effects |
| **EF4: Pool effects in MIR** | MIR operations carry effect annotations. `PoolInsert` has `Grow` effect, `PoolRemove` has `Shrink` effect. Frozen context checking happens at MIR level |
| **EF5: IO/Async stay metadata** | IO and Async effects remain non-enforcing (`comp.effects/FX3`). Only pool mutation effects (`comp.advanced/EF1-EF6`) are enforced |

### Why Enforce Frozen

Frozen context violation is a correctness issue, not a style issue. If a function promises frozen (no structural mutation) and then mutates, handles that callers assumed were stable might be stale. This is the same category as type errors — the type system should catch it.

Making frozen enforced also unlocks optimization: in a frozen context, the compiler can skip all generation checks during iteration, not just coalesce them. That's a meaningful performance win for hot read paths.

---

## Reflection

| Rule | Description |
|------|-------------|
| **RF1: Comptime-only** | `reflect.fields<T>()` and `reflect.variants<T>()` only work in comptime context |
| **RF2: Generated during monomorphization** | When mono encounters a reflection call, it generates field/variant metadata for the concrete type |
| **RF3: Evaluated by MIR CTFE** | `comptime for` over reflection data is evaluated by the MIR interpreter, producing unrolled concrete code |
| **RF4: No runtime reflection** | Reflection data doesn't exist at runtime. Everything resolves at compile time |

### Pipeline Position

```
Monomorphize (generates reflection metadata)
  → MIR Lower (comptime for becomes MIR loop)
  → MIR CTFE (evaluates loop, unrolls into concrete field accesses)
  → Normal MIR optimization
```

This means `rask-mono` needs to know about reflection types and generate them. The MIR interpreter needs to handle reflection values. But no new IR — reflection is just data that the CTFE evaluates.

---

## Debug Information

| Rule | Description |
|------|-------------|
| **DI1: DWARF emission** | Codegen emits DWARF debug info in debug builds |
| **DI2: Source locations** | Every MIR statement maps to a source span (IR4). Codegen calls `builder.set_srcloc()` per statement |
| **DI3: Variable mapping** | MIR locals map to source variable names. Emitted as DWARF `DW_TAG_variable` |
| **DI4: Type mapping** | MIR types map to DWARF type descriptions |
| **DI5: Inline info** | After inlining, inlined code carries original source location + inline info for debugger "step into" |

### What This Requires

- `Span` on every MIR node (IR4)
- Variable name preservation through SSA (phi nodes inherit names)
- Cranelift's `set_srcloc()` API (already available, just not called)
- DWARF section emission (Cranelift can produce this via `object` crate)

---

## Parallel Compilation

| Rule | Description |
|------|-------------|
| **PC1: Per-function parallelism** | After cross-function passes, per-function optimization and codegen run in parallel |
| **PC2: Cross-function passes first** | Closure optimization, inlining decisions, and escape analysis summaries run sequentially (they need global view) |
| **PC3: Rayon work-stealing** | Use `rayon` for parallel iteration over functions. Scales with available cores |
| **PC4: Thread-safe codegen** | Each function gets its own Cranelift `Function` builder. Object file assembly is sequential |

### Pass Manager Changes

```rust
impl PassManager {
    pub fn run(&self, fns: &mut [MirFunction]) {
        // Phase 1: cross-function passes (sequential)
        for pass in &self.cross_function_passes {
            pass.run(fns);
        }
        // Phase 2: per-function passes (parallel)
        fns.par_iter_mut().for_each(|func| {
            for pass in &self.per_function_passes {
                pass.run_function(func);
            }
        });
    }
}
```

---

## Incremental Compilation Hooks

| Rule | Description |
|------|-------------|
| **IC1: Serializable MIR** | `MirFunction` implements `Serialize`/`Deserialize` for caching (IR5) |
| **IC2: Semantic hash integration** | After frontend, semantic hashes computed per `comp.semantic-hash`. Unchanged functions skip MIR lowering entirely |
| **IC3: Cache granularity** | Cache stores: optimized MIR + object code per function. Both are needed — MIR for re-optimization if passes change, object code for fast relink |
| **IC4: Invalidation** | Function cache invalidated when its semantic hash changes or any dependency's hash changes (`comp.semantic-hash/MK2`) |

This doesn't require architectural changes — it requires MIR serialization (IR5) and hooks in the pass manager to check/store cache. The existing `rask-semantic-hash` crate provides the hashing.

---

## Target Pipeline (Full)

```
Source → Lexer → Parser → AST
  → Desugar (default args, syntax sugar, match desugaring)
  → Resolve (names → symbols)
  → Typecheck + Effects + Exhaustiveness → TypedProgram
      (type inference, pattern exhaustiveness, frozen enforcement,
       unsafe block validation, effect signatures)
  → Ownership check (AST-level: moves, borrows, resource consumption)
  → Hidden params desugaring (using clauses → explicit params, allocator contexts)
  → Monomorphize + Reflection metadata → MonoProgram
  → MIR Lowering (with Spans) → MirProgram (non-SSA)
      (ensure → EnsurePush/EnsurePop/CleanupReturn,
       closures → ClosureCreate/ClosureCall/ClosureDrop,
       resources → ResourceRegister/ResourceConsume/ResourceScopeCheck,
       match → decision trees, unsafe → debug-mode checks)
  → [Cache check — skip unchanged functions]
  → SSA Conversion (dominators → phi insertion → variable renaming)
  → Cross-function passes (sequential):
      - Closure escape analysis + capture strategy (CC2, CC4)
      - Inlining decisions + inline expansion
  → Per-function passes (parallel via rayon):
      - String RC insertion + fusion/elision/reuse (RC1-RC6)
      - Clone elision
      - Constant propagation
      - Copy propagation
      - Handle typestate checking (compile errors for TS8)
      - Interval analysis + bounds check elimination
      - Generation coalescing
      - Dead code elimination
  → De-SSA (phi → copies)
  → [Cache store — serialized MIR + object code]
  → Codegen (Cranelift/LLVM, parallel per function)
      (DWARF debug info, SSO tag checks for RC ops,
       debug-mode unsafe checks, release-mode elision)
  → Link with rask-rt → Executable
```

---

## Implementation Phases

| Phase | What | Enables |
|-------|------|---------|
| **A: Analysis foundation** | Dominator tree, dataflow framework, liveness | Everything else |
| **B: SSA** | SSA construction + de-SSA | String RC, constant prop, precise analyses |
| **C: String RC** | RC insertion/fusion/elision for strings | `comp.string-refcount-elision`, SSO preparation |
| **D: MIR CTFE** | MIR interpreter crate | Comptime correctness, reflection |
| **E: Debug info** | Spans on MIR, DWARF emission | Debugger support |
| **F: Advanced analyses** | Typestate, intervals, bounds check elimination | `comp.advanced` spec |
| **G: Parallel + Incremental** | Rayon, MIR serialization, cache layer | Build performance |

Phase A is prerequisite for B, C, F. Phases D and E are independent. Phase G is independent but benefits from all others.

---

## What Changes in Existing Specs

| Spec | Change |
|------|--------|
| `comp.codegen/P2` | Superseded by IR3 (hybrid SSA). MIR is SSA during optimization |
| `comp.effects/FX3` | Partially superseded by EF1. Pool effects enforced; IO/Async stay metadata |
| `comp.clone-elision` | Unchanged, but implementation benefits from SSA form and liveness analysis |
| `comp.string-refcount-elision` | Unchanged, but implemented via RC insertion pass using dataflow framework |
| `comp.advanced` | Unchanged — dataflow framework provides the infrastructure it assumes |

---

## Appendix (non-normative)

### Rationale

**IR3 (hybrid SSA):** I resisted SSA for a while because Cranelift does its own SSA conversion. But trying to implement string RC optimization, constant propagation, and interval analysis on non-SSA MIR means reimplementing def-use chains, reaching definitions, and variable renaming in every pass. SSA gives you all of that for free. The lowering complexity is a one-time cost; the optimization simplicity pays forever.

**DF1 (generic framework):** The specs promise 5+ different dataflow analyses. Building each ad-hoc means 5 different iteration strategies, 5 different caching approaches, 5 different ways to handle loops. A generic framework means getting iteration right once. Kildall's algorithm is textbook — there's no reason to reimplement it per analysis.

**CT1 (MIR CTFE):** The AST interpreter already has 40+ files and its own stdlib implementation. It will inevitably diverge from compiled behavior as the language evolves. Every language that's done both (Zig, Rust) converged on "interpret the IR" because semantic fidelity matters more than implementation convenience.

**EF1 (frozen enforced):** I went back and forth on this. Lint is less disruptive. But frozen is a semantic guarantee — callers depend on it for correctness (handles stay valid). Making it a lint means the guarantee is advisory, which means the optimizer can't rely on it. If frozen is enforced, the compiler can unconditionally skip generation checks in frozen contexts. That's a real performance win for the 80% of code that's read-heavy.

**PC1 (per-function parallelism):** Rask's monomorphized functions are independent after cross-function analysis. This is the easiest parallelism win in a compiler — no shared mutable state, just map over functions. Rayon makes it trivial. The bottleneck will be the sequential frontend (lex → typecheck), but that's fast enough for now.

### Research References

- **Perceus:** Reinking, Xie, de Moura, Leijen. "Perceus: Garbage Free Reference Counting with Reuse." ICFP 2021.
- **SSA construction:** Cytron, Ferrante, Rosen, Wegman, Zadeck. "Efficiently Computing Static Single Assignment Form and the Control Dependence Graph." TOPLAS 1991.
- **Dataflow analysis:** Kildall. "A Unified Approach to Global Program Optimization." POPL 1973.
- **Demand-driven VRP:** GCC Project Ranger. MacLeod, Law. GCC Summit 2019.

### See Also

- [Code Generation](codegen.md) — current pipeline (`comp.codegen`)
- [Advanced Analyses](advanced-analyses.md) — typestate, intervals, effects (`comp.advanced`)
- [Clone Elision](clone-elision.md) — last-use optimization (`comp.clone-elision`)
- [String Refcount Elision](string-refcount-elision.md) — atomic op elision (`comp.string-refcount-elision`)
- [Incremental Compilation](incremental.md) — caching strategy (`comp.incremental`)
- [Effects](effects.md) — effect tracking (`comp.effects`)
- [Compile-Time Execution](../control/comptime.md) — comptime rules (`ctrl.comptime`)
- [Ensure](../control/ensure.md) — deferred cleanup (`ctrl.ensure`)
- [Resource Types](../memory/resource-types.md) — must-consume types (`mem.resources`)
- [Closures](../memory/closures.md) — capture inference (`mem.closures`)
- [Unsafe Blocks](../memory/unsafe.md) — unsafe operations (`mem.unsafe`)
- [Allocators](../memory/allocators.md) — allocator contexts (`mem.alloc`)
- [Hidden Parameters](hidden-params.md) — using clause desugaring (`comp.hidden-params`)
