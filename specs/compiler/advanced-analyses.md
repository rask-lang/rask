<!-- id: comp.advanced -->
<!-- status: implemented -->
<!-- summary: Interval analysis for bounds-check elimination, and the grow/shrink effect inference -->
<!-- depends: memory/borrowing.md, memory/racks.md -->
<!-- implemented-by: compiler/crates/rask-mir/src/analysis/intervals.rs, compiler/crates/rask-effects/ -->

# Advanced Compile-Time Analyses

Rask catches memory safety bugs at compile time through structural rules rather than whole-program analysis. Two analyses go beyond those rules: interval analysis, which removes bounds checks it can prove redundant, and effect inference, which tells the rest of the compiler which calls restructure a collection.

**Design goal:** stay linear, and stay out of the way. Every analysis here is per-function, with summaries for the stdlib.

## What used to be here

Most of this spec was handle typestate analysis (TS1–TS8), must-alias tracking (MA1–MA5), and the frozen-context effect rules built on top of them — 1,450 lines of compiler whose whole job was proving away the generation check a `Pool<T>` performed on every access. Pools are gone (rask-lang/rask#908), and a `Link<T>` is followed rather than redeemed, so there is no check to prove away and no stale state to track. See `mem.racks/RK3`: delete nulls every incoming edge, so the invalid state doesn't exist to be analysed for.

## Performance Target

| Metric | Rust (rustc) | Rask Target | Rationale |
|--------|--------------|-------------|-----------|
| Compilation throughput | ~100K LOC/sec | **500K LOC/sec** | No whole-program borrow checking, no lifetime inference |
| Analysis overhead | 30-40% (borrow checking + MIR) | **< 10%** | Local analyses only, lazy evaluation |

I chose 5× faster because Rask's local-only analysis eliminates the most expensive parts of Rust's compilation: region inference, non-lexical lifetimes, trait coherence checking. The analyses described here are targeted and cheap.

## Interval Analysis

Demand-driven value range propagation to eliminate bounds checks and catch overflow at compile time.

| Rule | Description |
|------|-------------|
| **IV1: Lazy evaluation** | Range analysis is on-demand; triggered by bounds check or overflow-sensitive operation |
| **IV2: Interval domain** | Track `x in [lo, hi]` for each integer variable |
| **IV3: Backward propagation** | At query point (bounds check), walk backward through SSA graph to compute ranges |
| **IV4: Conditional narrowing** | After `if x > 5`, narrow x to `[6, +∞)` in true branch |
| **IV5: Loop widening** | Widen loop variables to conservative over-approximation at fixpoint |
| **IV6: Eliminate provable checks** | If range proves `i < collection.len()`, eliminate the bounds check |
| **IV7: Local analysis** | Per-function with interprocedural summaries for known stdlib functions |

<!-- test: parse -->
```rask
func total(scores: Vec<i64>) -> i64 {
    mut sum = 0
    for i in 0..scores.len() {   // i: [0, scores.len())
        sum += scores[i]         // bounds check eliminated: i provably < len
    }
    return sum
}
```

### Range Propagation

| Operation | Input Ranges | Output Range |
|-----------|--------------|--------------|
| `x + y` | x ∈ [a,b], y ∈ [c,d] | [a+c, b+d] (with overflow handling) |
| `x - y` | x ∈ [a,b], y ∈ [c,d] | [a-d, b-c] |
| `x * y` | x ∈ [a,b], y ∈ [c,d] | [min(products), max(products)] |
| `if x < c` | x ∈ [a,b] | True: [a, min(b,c-1)], False: [max(a,c), b] |
| `for i in a..b` | — | i ∈ [a, b) |

### Bounds Check Elimination

| Rule | Description |
|------|-------------|
| **BE1: Provable in-bounds** | If range analysis proves `0 <= i < len`, eliminate the check |
| **BE2: Conservative default** | If analysis is uncertain, keep the check |
| **BE4: Slice bounds** | `array[start..end]` eliminates checks if `0 <= start <= end <= len` |

<!-- test: parse -->
```rask
func safe_slice(data: Vec<i32>, start: usize, end: usize) -> Vec<i32> {
    if start > end || end > data.len() {
        panic("invalid range")
    }
    // Compiler knows: start <= end <= data.len()
    return data[start..end]  // Bounds checks eliminated
}
```

---

## Effect Inference

Calls are classified by what they do to a collection's shape. This is metadata, not a type-system constraint (`CORE_DESIGN` principle 9): nothing in a signature carries it, and no function is coloured by it.

| Rule | Description |
|------|-------------|
| **EF1: Structural effects** | A call carries `Grow` (insert, alloc), `Shrink` (remove, delete, clear, drain), both, or neither |
| **EF3: Inference, not annotation** | Effects are inferred from the body, transitively. Nothing is written in a signature |
| **EF4: What reads them** | `for mutate` structural-mutation rejection (`ctrl.loops/LP14`), the `with`-block rule (`mem.borrowing/W2`), and the blocking-I/O-in-a-loop warning (`comp.effects/CW2`) |

The effect map is where "which calls can move the buffer under a borrow" is answered once, rather than at each of the three sites that ask.

---

## Compilation Performance Model

| Analysis | Complexity | Cost Model | Typical Overhead |
|----------|------------|------------|------------------|
| **Interval analysis** | O(n) lazy | Only computed at query points | 1-3% (demand-driven) |
| **Effect inference** | O(n) | Standard constraint solving | < 1% (reuses type inference) |

### Comparison with Rust

| Component | Rust (rustc) | Rask | Speedup Factor |
|-----------|--------------|------|----------------|
| Borrow checking | O(n²) worst case (NLL) | Syntactic scopes, no inference | 10-100× faster |
| Lifetime inference | Region inference + NLL | Not needed (no lifetimes) | ∞ (eliminated) |
| Trait coherence | Global analysis | Local only | 5-10× faster |
| Monomorphization | Same | Same | 1× (same) |
| **Overall** | 100K LOC/sec | **500K LOC/sec** | **5× faster** |

I achieve 5× faster compilation by eliminating the most expensive Rust analyses (lifetime inference, global coherence) and replacing whole-program borrow checking with structural rules that need no analysis at all.

---

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Range analysis timeout | BE2 | Conservative: keep the check |
| Effect through a closure | EF3 | The closure's body contributes to the enclosing function's effects |
| Effect through a trait object | EF3 | Conservative: assume both Grow and Shrink |

---

## Appendix (non-normative)

### Rationale

**IV1–IV7 (interval analysis):** GCC's Project Ranger showed that demand-driven VRP is nearly free — you only pay for queries you make. By triggering analysis lazily at bounds checks, we avoid computing ranges for all variables. The backward SSA walk (IV3) is fast because SSA has no cycles (except through φ-nodes at loop headers, where we widen conservatively).

**EF1–EF4 (effects):** this started as a formalisation of `using frozen Pool<T>` — a clause that let a signature promise it wouldn't restructure the pool, so the compiler could skip generation checks during iteration. The clause and the checks both went with `Pool`. What survived is the half that was never about pools: three unrelated rules each need to know whether a call can move a buffer under something that is borrowing into it, and this is where that question gets one answer.

**Why handle typestate is gone rather than ported.** It was the right analysis for the wrong model. A handle is a ticket redeemed at a container, so it can be stale, so a generation check runs on every access, so an analysis that proves the check redundant pays for itself. A link is an address the rack keeps current: `delete` nulls every edge pointing at the node before it returns, and a local link the rack can't reach is rejected outright (`mem.racks/RK5`). There is no third state, so there is nothing for a four-state lattice to say.

**5× compilation speed vs Rust:** achievable because:
1. **No lifetime inference** — Rust's region inference is expensive. Rask has no lifetimes.
2. **No non-lexical lifetimes (NLL)** — NLL is O(n²) worst case. Rask's borrow scopes are syntactic (expression-scoped, block-scoped).
3. **Local-only analysis** — Rust's borrow checker is interprocedural for trait coherence and some lifetime checks. Rask's analyses are per-function with summaries.
4. **Structure over analysis** — the safety properties fall out of the rules (single owner, scoped borrows, delete-time edge fixup) rather than out of a solver.
5. **Lazy evaluation** — Range analysis is demand-driven. Don't pay for what you don't query.
