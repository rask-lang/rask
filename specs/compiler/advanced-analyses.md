<!-- id: comp.advanced -->
<!-- status: proposed -->
<!-- summary: Advanced compile-time analyses for stale handle detection and bounds elimination -->
<!-- depends: memory/pools.md, memory/borrowing.md, compiler/generation-coalescing.md -->

# Advanced Compile-Time Analyses

Rask catches memory safety bugs at compile time through structural rules rather than whole-program analysis. This spec describes additional static analyses that close the gap between Rask's runtime safety checks and Rust's compile-time guarantees — specifically for stale handle detection and bounds check elimination.

**Design goal:** Catch 80%+ of stale handle bugs at compile time while maintaining 5× faster compilation than Rust.

## Performance Target

| Metric | Rust (rustc) | Rask Target | Rationale |
|--------|--------------|-------------|-----------|
| Compilation throughput | ~100K LOC/sec | **500K LOC/sec** | No whole-program borrow checking, no lifetime inference |
| Analysis overhead | 30-40% (borrow checking + MIR) | **< 10%** | Local analyses only, lazy evaluation |
| Per-function complexity | O(n²) worst case (NLL) | **O(n × k)** average, k < 10 | Specialized to handles, not all references |

I chose 5× faster because Rask's local-only analysis eliminates the most expensive parts of Rust's compilation: region inference, non-lexical lifetimes, trait coherence checking. The analyses described here are targeted and cheap.

---

## Handle Typestate Analysis

Track handle validity states through control flow to catch stale handle access at compile time.

| Rule | Description |
|------|-------------|
| **TS1: Four states** | Handles have states: Fresh (just created), Valid (checked/accessed), Unknown (unchecked), Invalid (removed) |
| **TS2: Conservative join** | At control flow merge points, take the lower bound: Invalid < Unknown < Valid < Fresh |
| **TS3: Must-alias tracking** | Assignment `h2 = h1` makes h2 a must-alias of h1; they share state transitions |
| **TS4: Invalidation propagates** | `pool.remove(h)` makes h and all must-aliases Invalid |
| **TS5: Structural mutation widens** | `pool.insert()` or `pool.remove(other)` widens Unknown/Valid handles to Unknown |
| **TS6: Successful access narrows** | `pool[h]` or `pool.get(h) is Some` narrows to Valid in continuation |
| **TS7: Local analysis** | Typestate tracking is intraprocedural; function parameters default to Unknown |
| **TS8: Error on Invalid access** | Accessing a handle in Invalid state is a compile error |

<!-- test: compile-fail -->
```rask
func bad_example() using Pool<Player> {
    const h = try pool.insert(player)  // h: Fresh
    pool.remove(h)                     // h: Invalid
    pool[h].health -= 10               // ERROR [comp.advanced/TS8]: h is Invalid
}
```

### State Transitions

| Operation | State Before | State After | Must-Aliases |
|-----------|--------------|-------------|--------------|
| `h = pool.insert(x)` | — | Fresh | None (new handle) |
| `pool[h]` access | Any | Valid | Unchanged |
| `pool.get(h) is Some` | Any | Valid (true branch) | Unchanged |
| `pool.remove(h)` | Any | Invalid | All become Invalid |
| `pool.insert(x)` | Unknown/Valid | Unknown | Unchanged |
| `h2 = h1` | s | s | h2 aliases h1 |
| Function boundary | s | Unknown | Aliases cleared |

### Must-Alias Tracking

| Rule | Description |
|------|-------------|
| **MA1: Copy creates alias** | `h2 = h1` makes h2 a must-alias of h1 |
| **MA2: Fresh handles don't alias** | `pool.insert()` returns a handle that doesn't alias existing handles |
| **MA3: Function calls break aliases** | Passing a handle to a function breaks must-alias relationships (conservative) |
| **MA4: Reassignment breaks alias** | `h = new_value` removes h from its alias set |
| **MA5: Local scope only** | Alias tracking within function scope; cross-function aliasing conservatively Unknown |

<!-- test: compile-fail -->
```rask
func alias_example() using Pool<Player> {
    const h1 = try pool.insert(player)  // h1: Fresh, aliases: {}
    const h2 = h1                       // h2: Fresh, aliases: {h1}
    pool.remove(h1)                     // h1: Invalid, h2: Invalid (via alias)
    pool[h2].health -= 10               // ERROR [comp.advanced/TS8]: h2 is Invalid
}
```

### Flow-Sensitive Narrowing

| Rule | Description |
|------|-------------|
| **FN1: Check narrows** | Successful `pool.get(h) is Some` narrows h to Valid in the true branch |
| **FN2: Access narrows** | `pool[h]` access narrows h to Valid for subsequent uses in same basic block |
| **FN3: Mutation widens** | Pool structural mutation (insert/remove of other handles) widens to Unknown |
| **FN4: Loop reset** | Each loop iteration resets to pre-loop state |

<!-- test: pass -->
```rask
func safe_access(h: Handle<Player>) using Pool<Player> {
    // h: Unknown (parameter)
    if pool.get(h) is None {
        return  // h: Invalid here (narrowed in false continuation)
    }
    // h: Valid here (narrowed by check)
    pool[h].health -= 10  // OK: h is Valid
}
```

---

## Interval Analysis

Demand-driven value range propagation to eliminate bounds checks and catch overflow at compile time.

| Rule | Description |
|------|-------------|
| **IV1: Lazy evaluation** | Range analysis is on-demand; triggered by bounds check or overflow-sensitive operation |
| **IV2: Interval domain** | Track `x in [lo, hi]` for each integer variable |
| **IV3: Backward propagation** | At query point (bounds check), walk backward through SSA graph to compute ranges |
| **IV4: Conditional narrowing** | After `if x > 5`, narrow x to `[6, +∞)` in true branch |
| **IV5: Loop widening** | Widen loop variables to conservative over-approximation at fixpoint |
| **IV6: Eliminate provable checks** | If range proves `i < array.len()`, eliminate the bounds check |
| **IV7: Local analysis** | Per-function with interprocedural summaries for known stdlib functions |

<!-- test: pass -->
```rask
func process(pool: Pool<Entity>) {
    for i in 0..pool.len() {       // i: [0, pool.len())
        const h = pool.handles()[i] // Bounds check eliminated: i provably < len
        process_entity(pool[h])
    }
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
| **BE3: Handle index bounds** | `pool.handles()[i]` eliminates check if `i in [0, pool.len())` |
| **BE4: Slice bounds** | `array[start..end]` eliminates checks if `0 <= start <= end <= len` |

<!-- test: pass -->
```rask
func safe_slice(data: Vec<i32>, start: usize, end: usize) -> Vec<i32> {
    if start > end or end > data.len() {
        panic("invalid range")
    }
    // Compiler knows: start <= end <= data.len()
    return data[start..end]  // Bounds checks eliminated
}
```

---

## Effect System for Pool Mutations

Formalize Rask's `using Pool<T>` context clauses as a lightweight effect system to track structural mutations.

| Rule | Description |
|------|-------------|
| **EF1: Pool effects** | Operations have effects: `Access<Pool<T>>`, `Grow<Pool<T>>`, `Shrink<Pool<T>>` |
| **EF2: Frozen context** | `using frozen Pool<T>` forbids Grow and Shrink effects |
| **EF3: Effect inference** | Private functions infer effects; public functions must declare frozen explicitly |
| **EF4: Effect checking** | Calling a Shrink function from frozen context is a compile error |
| **EF5: Generation elimination** | Frozen contexts guarantee no generation checks needed |
| **EF6: Effect polymorphism** | Functions can be effect-polymorphic: work with both frozen and mutable pools |

<!-- test: compile-fail -->
```rask
// Frozen context — structural mutations forbidden
func render(entities: Vec<Handle<Entity>>) using frozen Pool<Entity> {
    for h in entities {
        draw(pool[h])        // OK: Access effect allowed
        pool.remove(h)       // ERROR [comp.advanced/EF4]: Shrink effect forbidden
    }
}
```

### Effect Annotations

| Annotation | Allowed Effects | Generation Checks | Use Case |
|------------|-----------------|-------------------|----------|
| `using Pool<T>` | Access, Grow, Shrink | Normal (coalesced) | Default |
| `using frozen Pool<T>` | Access only | Zero (eliminated) | Read-only passes |
| `using name: Pool<T>` | Access, Grow, Shrink | Normal | Structural ops via name |
| `using frozen name: Pool<T>` | Access only | Zero | Explicit frozen access |

### Effect Lattice

| Effect | Meaning | Invalidates Typestate? |
|--------|---------|----------------------|
| `Access<Pool<T>>` | Read/write handle fields | No |
| `Grow<Pool<T>>` | Insert new elements | Widens to Unknown |
| `Shrink<Pool<T>>` | Remove elements | Invalidates removed handle |

<!-- test: pass -->
```rask
// Effect-polymorphic: works with frozen or mutable
func count_alive(entities: Vec<Handle<Entity>>) using frozen Pool<Entity> -> usize {
    return entities.filter(|h| pool[h].alive).count()
    // Compiler eliminates all generation checks in this function
}

func cleanup(entities: Vec<Handle<Entity>>) using Pool<Entity> {
    for h in entities {
        if pool[h].health <= 0 {
            pool.remove(h)  // OK: we have Shrink effect
        }
    }
}
```

---

## Compilation Performance Model

All analyses are designed for linear or near-linear time complexity.

| Analysis | Complexity | Cost Model | Typical Overhead |
|----------|------------|------------|------------------|
| **Handle typestate** | O(n × k) | n = program points, k = handles in scope (< 10) | 2-5% compile time |
| **Must-alias tracking** | O(n × k) | SSA form makes this near-linear | 1-2% compile time |
| **Interval analysis** | O(n) lazy | Only computed at query points | 1-3% (demand-driven) |
| **Effect inference** | O(n) | Standard constraint solving, Hindley-Milner style | < 1% (reuses type inference) |
| **Total overhead** | O(n × k) | k is small constant | **< 10% compile time** |

### Comparison with Rust

| Component | Rust (rustc) | Rask (proposed) | Speedup Factor |
|-----------|--------------|-----------------|----------------|
| Borrow checking | O(n²) worst case (NLL) | O(n × k), k < 10 | 10-100× faster |
| Lifetime inference | Region inference + NLL | Not needed (no lifetimes) | ∞ (eliminated) |
| Trait coherence | Global analysis | Local only | 5-10× faster |
| Monomorphization | Same | Same | 1× (same) |
| **Overall** | 100K LOC/sec | **500K LOC/sec** | **5× faster** |

I achieve 5× faster compilation by eliminating the most expensive Rust analyses (lifetime inference, global coherence) and replacing whole-program borrow checking with targeted local analyses specialized for Rask's pool+handle model.

---

## Error Messages

**Stale handle access [TS8]:**
```
ERROR [comp.advanced/TS8]: stale handle access
   |
5  |  pool.remove(h)
   |  ^^^^^^^^^^^^^^ handle invalidated here
8  |  pool[h].health -= 10
   |  ^^^^^^^ handle is Invalid (provably stale)

WHY: Handle typestate analysis proves this handle was removed and is no longer valid.

FIX: Check validity before access:

  if pool.get(h) is Some {
      pool[h].health -= 10
  }
```

**Aliased handle removed [TS4]:**
```
ERROR [comp.advanced/TS8]: stale handle access via alias
   |
3  |  const h2 = h1
   |         ^^ h2 is a must-alias of h1
5  |  pool.remove(h1)
   |  ^^^^^^^^^^^^^^ h1 invalidated here (h2 also becomes Invalid)
6  |  pool[h2].update()
   |  ^^^^^^^ handle is Invalid (h2 aliased h1)

WHY: h2 is a copy of h1. When h1 is removed, h2 becomes stale too.

FIX: Don't access h2 after removing h1.
```

**Frozen context violation [EF4]:**
```
ERROR [comp.advanced/EF4]: structural mutation in frozen context
   |
2  |  func render(h: Handle<Entity>) using frozen Pool<Entity> {
   |                                       ------ context is frozen
3  |      pool.remove(h)
   |      ^^^^^^^^^^^^^^ Shrink effect forbidden in frozen context

WHY: Frozen contexts guarantee no structural mutations, enabling zero-cost generation checks.

FIX: Remove the frozen annotation if mutation is needed:

  func render(h: Handle<Entity>) using Pool<Entity> { ... }
```

**Bounds check not eliminated [BE2]:**
```
NOTE [comp.advanced/BE2]: bounds check could not be eliminated
   |
5  |  const index = compute_index()
   |                ^^^^^^^^^^^^^^^ range unknown
6  |  array[index]
   |  ^^^^^ bounds check retained (conservative)

NOTE: Consider adding a range check:

  if index < array.len() {
      array[index]  // Check eliminated here
  }
```

---

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Handle passed to function | TS7 | Caller state preserved; callee sees Unknown |
| Must-alias across function call | MA3 | Conservative: aliases broken at call boundary |
| Loop with conditional remove | TS4, FN4 | Each iteration resets to pre-loop state |
| Range analysis timeout | BE2 | Conservative: keep the check |
| Effect inference failure | EF3 | Public functions require explicit annotation |
| Frozen pool with mutable handle | EF1 | Allowed: field writes (Access effect) permitted |
| Typestate at merge with Unknown | TS2 | Join takes lower bound (e.g., Valid ∧ Unknown = Unknown) |
| Generation overflow | — | Not addressed by static analysis (runtime invariant) |
| Concurrent mutation | — | Not addressed (use Mutex for cross-task pools) |

---

## Appendix (non-normative)

### Rationale

**TS1–TS8 (handle typestate):** Stale handle access is Rask's biggest gap vs Rust. Rust's borrow checker proves references are valid at compile time; Rask uses generation counters at runtime. Typestate analysis closes 80%+ of this gap by tracking handle validity through control flow. The must-alias analysis (TS3, MA1–MA5) is critical — it catches bugs like "copy handle, remove original, use copy."

I chose a four-state lattice (Fresh > Valid > Unknown > Invalid) because it balances precision and cost. More states (e.g., tracking which specific operations invalidated a handle) would give better error messages but quadratic cost. Four states gives us "definitely invalid" (provable error) and "checked valid" (optimization opportunity) with linear analysis.

**IV1–IV7 (interval analysis):** GCC's Project Ranger showed that demand-driven VRP is nearly free — you only pay for queries you make. By triggering analysis lazily at bounds checks, we avoid computing ranges for all variables. The backward SSA walk (IV3) is fast because SSA has no cycles (except through φ-nodes at loop headers, where we widen conservatively).

**EF1–EF6 (effect system):** Rask already has `using Pool<T>` clauses. I formalized them as an effect system to make the guarantees explicit. The key insight: frozen contexts enable zero-cost generation checks (EF5), same as `FrozenPool<T>` but at the function level. This is more ergonomic than manually calling `pool.freeze()`. Effect inference (EF3) means most code doesn't need annotations — the compiler figures it out.

**5× compilation speed vs Rust:** This is achievable because:
1. **No lifetime inference** — Rust's region inference is expensive. Rask has no lifetimes.
2. **No non-lexical lifetimes (NLL)** — NLL is O(n²) worst case. Rask's borrow scopes are syntactic (expression-scoped, block-scoped).
3. **Local-only analysis** — Rust's borrow checker is interprocedural for trait coherence and some lifetime checks. Rask's analyses are per-function with summaries.
4. **Specialized analyses** — Typestate is specialized for pool handles, not all references. This is much cheaper than general borrow checking.
5. **Lazy evaluation** — Range analysis is demand-driven. Don't pay for what you don't query.

The target of 500K LOC/sec is based on Rask's simpler type system, lack of lifetimes, and local-only analyses. Rust spends 30-40% of compile time on borrow checking and MIR building. Rask's advanced analyses (proposed here) should be < 10% because they're targeted and lazy.

### Implementation Roadmap

**Phase 1: Foundational (2-3 months)**
- **Handle typestate tracking (TS1-TS8)** — Core dataflow framework
- **Must-alias analysis (MA1-MA5)** — Track handle aliasing
- **Flow-sensitive narrowing (FN1-FN4)** — Extend existing `is` pattern narrowing
- Infrastructure: SSA form, CFG construction, dataflow solver

**Phase 2: Optimization (1-2 months)**
- **Interval analysis (IV1-IV7)** — Demand-driven VRP
- **Bounds check elimination (BE1-BE4)** — Use interval analysis to prove safety
- **Effect formalization (EF1-EF6)** — Make frozen contexts explicit
- Integration with existing generation coalescing (comp.gen-coalesce)

**Phase 3: Refinement (ongoing)**
- Interprocedural summaries for typestate (optional precision improvement)
- Loop-sensitive range analysis (optional for complex loops)
- SMT-backed verification for opt-in deep analysis (`rask verify --deep`)
- Performance profiling and optimization

### Patterns & Guidance

**When typestate catches bugs:**
```rask
// Pattern: remove then use
const h = try pool.insert(entity)
pool.remove(h)
pool[h].health -= 10  // ERROR: caught at compile time

// Pattern: aliased remove
const h1 = try pool.insert(a)
const h2 = h1
pool.remove(h1)
pool[h2].update()  // ERROR: caught via must-alias

// Pattern: conditional invalidation
const h = get_handle()
if should_cleanup {
    pool.remove(h)
}
pool[h].render()  // ERROR: h is Invalid in one path
```

**When typestate doesn't catch (requires runtime check):**
```rask
// Pattern: cross-function aliasing
const h1 = try pool.insert(a)
const h2 = get_other_handle()  // Unknown whether h1 == h2
pool.remove(h1)
pool[h2].update()  // OK at compile time, runtime generation check

// Pattern: conditional with Unknown
func process(h: Handle<Entity>) using Pool<Entity> {
    // h is Unknown (parameter)
    pool[h].update()  // OK: runtime check (can't prove Invalid)
}
```

**Optimization patterns for frozen contexts:**
```rask
// Hot read path — zero generation checks
func render_all(entities: Vec<Handle<Entity>>) using frozen Pool<Entity> {
    for h in entities {
        renderer.draw(pool[h])  // Zero checks: frozen context guarantee
    }
}

// Multi-phase processing
func tick(mut pool: Pool<Entity>) {
    // Phase 1: Mutable updates
    for h in pool.handles() {
        pool[h].update_physics()
    }

    // Phase 2: Frozen render
    pool.with_frozen(|frozen_pool| {
        using frozen Pool<Entity> = frozen_pool {
            for h in pool.handles() {
                render(pool[h])  // Zero checks
            }
        }
    })
}
```

**Bounds check elimination patterns:**
```rask
// Pattern: loop with known bounds
for i in 0..array.len() {
    process(array[i])  // Bounds check eliminated
}

// Pattern: validated range
if start < end and end <= data.len() {
    for i in start..end {
        use(data[i])  // Bounds check eliminated
    }
}

// Pattern: stride access
for i in (0..n).step_by(2) {
    if i + 1 < array.len() {
        pair(array[i], array[i+1])  // Both checks eliminated
    }
}
```

### Research Connections

This design draws from recent PL research:

- **Typestate:** Plaid language, Rust's typestate pattern, Obsidian (blockchain typestate)
- **Demand-driven analysis:** GCC Project Ranger (lazy VRP), LLVM's on-demand analysis passes
- **Effect systems:** Koka, Scala 3 capture checking, System Capybara (capture tracking for ownership)
- **Local separation logic:** Prusti (ETH Zurich), Verus (Microsoft/CMU), RefinedC's Lithium proof search
- **Session types:** Ferrite (judgmental embedding in Rust), Linear Actris (deadlock freedom from linearity)

The novel contribution here is **combining typestate specifically for pool handles with Rask's ownership model**. Existing typestate systems (Plaid, Rust pattern) don't have first-class handle types. Existing separation logic tools (Prusti, Verus) are opt-in verification frameworks, not default compiler passes. Rask makes typestate checking automatic for pool handles while keeping compilation fast.

### Metrics Validation

| Metric | Target | This Design | Status |
|--------|--------|-------------|--------|
| MC (Mechanical Correctness) | >= 0.90 | Stale handle detection at compile time | ✓ Improved |
| TC (Transparency of Cost) | >= 0.90 | Effects make mutations visible | ✓ Maintained |
| SN (Syntactic Noise) | <= 0.30 | Effect inference (no annotations needed) | ✓ Maintained |
| Compilation Speed | 5× Rust | Local analyses, lazy evaluation, no lifetimes | ✓ Achievable |

### See Also

- [Pools and Handles](../memory/pools.md) — Handle-based storage (`mem.pools`)
- [Generation Coalescing](generation-coalescing.md) — Existing optimization (`comp.gen-coalesce`)
- [Borrowing](../memory/borrowing.md) — Expression-scoped views (`mem.borrowing`)
- [Resource Types](../memory/resource-types.md) — Must-consume checking (`mem.resources`)
- [Context Clauses](../memory/context-clauses.md) — `using` syntax (`mem.context`)
