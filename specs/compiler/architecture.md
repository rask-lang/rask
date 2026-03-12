<!-- id: comp.architecture -->
<!-- status: proposed -->
<!-- summary: Target compiler architecture — IR layers, analysis framework, pass pipeline, CTFE, debug info -->
<!-- depends: compiler/codegen.md, compiler/advanced-analyses.md, compiler/clone-elision.md, compiler/string-refcount-elision.md, compiler/incremental.md, compiler/effects.md -->

# Compiler Architecture

The compiler grew feature-by-feature. This spec defines the target architecture so that Perceus RC, SSO strings, MIR-based CTFE, reflection, debugging, borrow analysis, effects, incremental compilation, and parallel compilation all have clean extension points — not bolted on after the fact.

I'm writing this now because every one of those features touches the MIR layer. If MIR's structure is wrong, every feature fights it. Getting the bones right means each feature is a pass, not a rewrite.

---

## IR Layers

| Rule | Description |
|------|-------------|
| **IR1: Three representations** | Source → AST → MIR (SSA) → machine code (via Cranelift/LLVM). No HIR — AST is rich enough after desugaring |
| **IR2: MIR is the optimization target** | All Rask-specific optimizations (Perceus, clone elision, refcount elision, generation coalescing, typestate) run on MIR |
| **IR3: Hybrid SSA** | MIR lowering produces non-SSA form (variables as slots). Immediate SSA conversion before optimization. De-SSA before codegen |
| **IR4: Source spans everywhere** | Every MIR statement and terminator carries a `Span`. Lossless source mapping from AST through MIR to machine code |
| **IR5: Serializable MIR** | MIR types are serializable for incremental compilation caching (`comp.incremental/IC5`) |

### Why Hybrid SSA (IR3)

Non-SSA is easier to generate during lowering — each variable maps to a mutable slot, no phi insertion needed. But optimization passes need SSA's single-definition property:

- Perceus needs precise def-use chains for RC placement
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
| **Liveness** | Backward | `BitSet<LocalId>` (set of live locals) | Perceus drop placement, clone elision, DCE, register hints |
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

## Perceus Reference Counting

| Rule | Description |
|------|-------------|
| **RC1: Explicit RC operations** | After SSA conversion, insert explicit `RcInc` and `RcDec` MIR statements for refcounted types |
| **RC2: Precise drop placement** | `RcDec` placed at each last-use point (from liveness analysis), not at scope exit |
| **RC3: Inc/dec fusion** | Adjacent or provably paired inc/dec cancel out (`comp.string-refcount-elision/RE1`) |
| **RC4: Local-only elision** | Refcounted values that don't escape the function skip all RC ops (`comp.string-refcount-elision/RE2`) |
| **RC5: Reuse analysis** | If `RcDec` would drop to zero and an allocation of the same size follows, reuse the memory (Perceus reuse credit) |
| **RC6: Applies to strings** | Primary target is `string` (16-byte header, heap buffer). Future refcounted types get this for free |

### Pipeline Position

```
MIR Lowering → SSA Conversion → Perceus RC Insertion → RC Optimization → Other Passes → De-SSA → Codegen
```

Perceus runs early (right after SSA) because later passes benefit from seeing explicit RC operations — clone elision can reason about what's actually an RC bump vs a deep clone.

### Interaction with SSO

When SSO is implemented (`string` inline for ≤15 bytes), RC operations become conditional:

```
// Pseudo-MIR
if string.is_heap() {
    RcDec(string)
}
```

The Perceus pass inserts unconditional RC ops. A later SSO-aware pass (or codegen) adds the tag check. Alternatively, Perceus skips strings provably inline (from constant propagation — string literals ≤15 bytes are always inline).

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
  → Desugar (default args, syntax sugar)
  → Resolve (names → symbols)
  → Typecheck + Effects → TypedProgram (with EffectSignatures)
  → Ownership check (AST-level; eventually MIR-level)
  → Hidden params desugaring
  → Monomorphize + Reflection metadata → MonoProgram
  → MIR Lowering (with Spans) → Vec<MirFunction> (non-SSA)
  → [Cache check — skip unchanged functions]
  → SSA Conversion (dominators → phi insertion → variable renaming)
  → Cross-function passes (sequential):
      - Closure escape analysis
      - Inlining decisions + inline expansion
  → Per-function passes (parallel via rayon):
      - Perceus RC insertion + RC optimization
      - String refcount elision
      - Clone elision
      - Constant propagation
      - Copy propagation
      - Handle typestate checking (compile errors for TS8)
      - Interval analysis + bounds check elimination
      - Generation coalescing
      - Dead code elimination
  → De-SSA (phi → copies)
  → [Cache store — serialized MIR + object code]
  → Codegen (Cranelift/LLVM, parallel per function) with DWARF debug info
  → Link with rask-rt → Executable
```

---

## Implementation Phases

| Phase | What | Enables |
|-------|------|---------|
| **A: Analysis foundation** | Dominator tree, dataflow framework, liveness | Everything else |
| **B: SSA** | SSA construction + de-SSA | Perceus, constant prop, precise analyses |
| **C: Perceus** | RC insertion/optimization for strings | String refcount elision, SSO preparation |
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
| `comp.string-refcount-elision` | Unchanged, but implemented via Perceus framework rather than standalone pass |
| `comp.advanced` | Unchanged — dataflow framework provides the infrastructure it assumes |

---

## Appendix (non-normative)

### Rationale

**IR3 (hybrid SSA):** I resisted SSA for a while because Cranelift does its own SSA conversion. But trying to implement Perceus, constant propagation, and interval analysis on non-SSA MIR means reimplementing def-use chains, reaching definitions, and variable renaming in every pass. SSA gives you all of that for free. The lowering complexity is a one-time cost; the optimization simplicity pays forever.

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
