# Memory Model: Potential Solutions

Analysis of design approaches that could address multiple issues from TODO.md.

---

## Root Cause Analysis

The 14 issues cluster around a few core design decisions:

| Root Cause | Issues Affected |
|------------|-----------------|
| **Handles are "dumb" (need Pool to do anything)** | #2, #3, #10, #11 |
| **Dual borrowing model (block vs expression)** | #5, #6, #7, #12, #14 |
| **Per-type Pools (fragmented memory)** | #1, #9 |
| **Fixed thresholds** | #4 |
| **Missing specifications** | #8, #12, #13 |

The biggest pain comes from **handles being separated from their pools**. This single decision cascades into context-passing, zombie handles, and multi-pool complexity.

---

## Option A: Ambient Pool Scoping

**Idea:** Instead of passing pools everywhere, make them "ambient" in a scope. Handles can auto-dereference through the ambient pool.

```rask
with players {
    const h = players.insert(Entity{...})
    h.health -= 10              // Auto-dereference through ambient 'players'
    if h.health <= 0 {
        players.remove(h)       // Explicit for mutation
    }
}
```

**What it fixes:**
- **#2 Context Passing:** Pool is ambient, not passed to every function
- **#14 Double Access:** Compiler knows ambient pool, can optimize checks
- **#6 Dual Semantics:** Uniform `h.field` access pattern
- **#13 Thread-Local:** `with` establishes task-local context

**Tradeoffs:**
- New scoping construct
- What about multiple pools? `with (players, enemies) { ... }`?
- Hidden dependency (pool must be in scope) — tension with TC ≥ 0.90?

**Key insight:** The `with` block is explicit (visible cost), but inside it, access is clean. This might satisfy TC while improving ED.

**Open questions:**
- How do functions declare they need an ambient pool?
- Can ambient pools nest? Override?
- What's the syntax for "this function requires ambient pool X"?

---

## Option B: Arena-Based Regions (Vale-inspired)

**Idea:** Replace per-type Pools with Arenas that hold multiple types. References are valid for the arena's lifetime.

```rask
const arena = Arena.new()
const player = arena.alloc(Player{...})   // Returns &'arena Player
const enemy = arena.alloc(Enemy{...})     // Returns &'arena Enemy
player.target = enemy                    // Cross-type reference OK (same arena)
// Arena freed → all references invalid (compile-time tracked)
```

**What it fixes:**
- **#1 Fragmentation:** One arena, multiple types
- **#2 Context Passing:** One arena instead of many pools
- **#3 Zombies:** References scoped to arena lifetime (compile-time)
- **#9 Self-Referential:** Arena-internal references are safe

**Tradeoffs:**
- Reintroduces lifetime-like concepts (arena scope)
- Bulk deallocation only — can't free individual items
- Major redesign of memory model

**Key insight:** This is closer to Rust's arenas but with simpler "one lifetime per arena" rather than complex lifetime relationships.

**Variations:**
- **Region polymorphism:** Functions generic over region `func process<R>(entity: &R Entity)`
- **Nested regions:** Inner regions freed before outer
- **Hybrid:** Arenas for graphs, Pools for entity systems

---

## Option C: Smart Handles (Pool Identity at Type Level)

**Idea:** Handles carry their pool identity at compile time. The compiler tracks which pool a handle belongs to.

```rask
pool players: Pool<Player>          // Named pool declaration

const h = players.insert(...)         // h: Handle<Player, @players>
h.health -= 10                      // Compiler knows source, auto-resolves

func damage(target: Handle<Player, @players>) {
    target.health -= 10             // Works: pool identity matches
}
```

**What it fixes:**
- **#2 Context Passing:** Handle carries pool identity, no need to pass pool
- **#10 Multi-Pool:** Compiler knows which pool each handle belongs to
- **#3 Zombies (partially):** Compiler could warn if pool goes out of scope while handles exist

**Tradeoffs:**
- Handle types become more complex (`Handle<T, @pool>`)
- Cross-function handles need pool identity in signature
- Might feel like "lifetime annotations lite"

**Key insight:** This is a middle ground — more type information than current design, but less than Rust's full lifetimes.

**Open questions:**
- How do you pass handles to functions that don't know the pool name?
- Generic pool identity? `func damage<P>(target: Handle<Player, P>)`?
- Does this just recreate lifetime parameters with different syntax?

---

## Option D: Implicit Context (Odin/Jai-inspired)

**Idea:** A task-local implicit context carries commonly needed state.

```rask
// Context is implicitly available
func update_player(h: PlayerHandle) {
    context.players[h].health -= 10
    // or with sugar:
    h@players.health -= 10
}

// Context setup
@entry
func main() {
    const ctx = Context {
        players: Pool.new(),
        enemies: Pool.new(),
        allocator: default_allocator(),
    }
    with_context(ctx) {
        game_loop()
    }
}
```

**What it fixes:**
- **#2 Context Passing:** Context is implicit, not passed
- **#13 Thread-Local:** Context is per-task
- **#11 Allocation:** Context can provide allocators

**Tradeoffs:**
- Hidden dependencies (tension with TC ≥ 0.90)
- Context structure must be defined upfront
- Testing might be harder (need to set up context)

**Key insight:** Odin and Jai prove this works well for games. But it's less "transparent" than Rask's current philosophy.

**Variations:**
- **Typed context:** `context<GameContext>.players[h]`
- **Scoped context:** `with_context(ctx) { ... }` like Option A
- **Optional context:** Functions declare `needs context.players`

---

## Option E: Compiler Optimizations + Patterns (Conservative)

**Idea:** Keep the current model but add targeted fixes.

| Fix | Addresses |
|-----|-----------|
| **Generation check coalescing:** Same handle, no intervening mutations → one check | #14 |
| **Weak handles:** `pool.weak(h)` returns handle that can be invalidated | #3 |
| **Cursor iteration:** `for h in pool.cursor() { }` — no allocation | #11 |
| **Canonical patterns doc:** Graphs, trees, self-ref structures | #9 |
| **Spec clarifications:** Lifetime extension, multi-pool semantics | #10, #12 |

**What it fixes:**
- Performance issues (#14, #11) via optimization
- Zombie issue (#3) via weak handles
- Missing specs (#9, #10, #12) via documentation

**What it doesn't fix:**
- #2 Context Passing (still verbose)
- #6 Dual Semantics (still cognitive load)

**Key insight:** This is the least disruptive but doesn't address the fundamental ergonomics gap.

### E.1: Generation Check Coalescing

```rask
pool[h].health -= damage
if pool[h].health <= 0 { ... }

// Compiler sees:
// - Same handle h
// - No mutations to pool between accesses
// - Coalesce to single generation check
```

### E.2: Weak Handles

```rask
const weak = pool.weak(h)           // Weak handle, can be invalidated

pool.remove(h)                    // Invalidates all weak handles to h

if weak.valid() {
    pool[weak.upgrade()?]         // Safe access
}
```

### E.3: Cursor Iteration

```rask
const cursor = pool.cursor()
while cursor.next() {
    const h = cursor.handle()
    pool[h].update()
    if pool[h].dead {
        cursor.remove()           // Safe removal during iteration
    }
}
```

---

## Option F: View Types for Multi-Statement Access

**Idea:** Explicit "view" that extends borrow lifetime within a controlled scope.

```rask
pool.with_view(h, |entity| {
    entity.health -= damage
    entity.last_hit = now()
    if entity.health <= 0 {
        entity.status = Dead
    }
})

// Or with explicit view binding:
const view = pool.view(h)?          // View locks the slot
view.health -= damage
view.position.x += 1
drop(view)                        // Explicit unlock
pool.remove(h)                    // OK, view released
```

**What it fixes:**
- **#14 Double Access:** Single generation check for view lifetime
- **#6 Dual Semantics:** Uniform "get a view, use it" pattern

**Tradeoffs:**
- More ceremony than expression-scoped
- View must be explicitly dropped before pool mutation
- Similar to `modify()` closure but with explicit binding

---

## Comparison Matrix

| Solution | #2 Context | #3 Zombies | #6 Dual | #14 Double | Disruption |
|----------|------------|------------|---------|------------|------------|
| A: Ambient Pool | ✅ | Partial | ✅ | ✅ | Medium |
| B: Arena Regions | ✅ | ✅ | ✅ | ✅ | **High** |
| C: Smart Handles | ✅ | Partial | ❌ | ✅ | Medium |
| D: Implicit Context | ✅ | ❌ | ❌ | ❌ | Medium |
| E: Conservative | ❌ | ✅ (weak) | ❌ | ✅ (opt) | **Low** |
| F: View Types | ❌ | ❌ | ✅ | ✅ | Low |

---

## Recommended Hybrid Approach

Combine elements for maximum leverage:

### Core Changes

1. **Ambient Pool Scoping (from A)**
   - `with pool { }` blocks for ergonomic access
   - Handle auto-dereference through ambient pool
   - Explicit boundary satisfies TC, clean syntax inside satisfies ED

2. **Weak Handles (from E)**
   - Opt-in runtime invalidation tracking
   - Solves zombie problem for event systems
   - `pool.weak(h)` for handles that might outlive data

3. **Compiler Optimizations (from E)**
   - Generation check coalescing
   - Cursor-based iteration
   - Zero-cost when patterns are recognized

### Keep as Fallback

- Explicit `pool[h]` syntax still works for complex cases
- Expression-scoped semantics remain for simple access
- No breaking changes to existing patterns

### Example of Hybrid

```rask
// Ambient pool for clean game loop
with (players, enemies) {
    for h in players.cursor() {
        h.velocity += gravity * dt       // Auto-deref through ambient
        h.position += h.velocity * dt

        if h.health <= 0 {
            // Weak handles in event queue won't crash
            event_queue.push(PlayerDied(players.weak(h)))
            players.remove(h)
        }
    }
}

// Explicit access still works
func standalone_function(pool: Pool<Player>, h: Handle<Player>) {
    pool[h].health -= 10                 // Traditional syntax
}
```

---

## Decision Criteria

When choosing between options, consider:

| Criterion | Weight | Notes |
|-----------|--------|-------|
| ED ≤ 1.2 improvement | High | Must feel simpler than Go for common cases |
| MC ≥ 0.90 preservation | High | Safety guarantees are non-negotiable |
| TC ≥ 0.90 compliance | High | Costs must remain visible |
| Implementation complexity | Medium | Affects timeline and bug surface |
| Mental model simplicity | Medium | One concept vs multiple |
| Backward compatibility | Low | Can break if improvement is significant |

---

---

## Language Inspiration: Vale and Jai

### Vale: Generational References + Regions

[Vale](https://vale.dev/) is the closest existing implementation to Rask's handle model. Key insights:

**Generational References (like Rask's handles):**
- Every object has a "current generation" integer, incremented on free
- Every pointer stores a "remembered generation" from allocation time
- Dereference checks that generations match
- **Overhead: ~10.84%** in benchmarks (vs 25% for reference counting)

**Regions (Vale's key innovation):**
- A region is a grouping of data that can be frozen/unfrozen
- While frozen (immutable), **no generation checks needed**
- `pure` functions operating on immutable data skip all checks
- This can reduce checks to **zero** in pure code paths

**Applicable to Rask:**

| Vale Concept | Rask Equivalent | Insight |
|--------------|-----------------|---------|
| Generational references | Pool handles | Already have this |
| Region freezing | `pool.freeze()` / `pool.read_only()` | **New idea**: frozen pools skip generation checks |
| Pure functions | `pure func` annotation? | Functions on frozen data get zero-cost access |

**Potential Feature: Frozen Pools**
```rask
const frozen = players.freeze()     // Pool becomes immutable
for h in frozen {
    frozen[h].render()            // Zero generation checks!
}
const players = frozen.thaw()       // Make mutable again
```

This directly addresses **#14 (Double Access)** and **RO ≤ 1.10** with zero syntax overhead for read-heavy code paths.

---

### Jai: Implicit Context

[Jai](https://en.wikipedia.org/wiki/Jai_(programming_language)) (Jonathan Blow's language) uses implicit context for state threading:

**How it works:**
- Every procedure has an implicit `context` pointer (like C++ `this`)
- Context contains: allocator, logger, temporary storage, user data
- Context can be pushed/modified for a scope
- Works like a thread-local value (possibly kept in a register)

**Four Lifetime Categories (Jai philosophy):**
1. **Extremely short-lived** — thrown away by end of function (temp allocator)
2. **Short-lived + defined lifetime** — "per frame" allocation
3. **Long-lived + clear owner** — uniquely owned by subsystem
4. **Long-lived + unclear owner** — shared, unknown free time

**Why Rask rejected full implicit context (Option D):**
- Violates **TC ≥ 0.90** — hidden dependencies
- "Where does `context.players` come from?" is non-obvious

**What Rask CAN take from Jai:**
- **Temporary allocator** — context provides scratch space
- **Scoped context modification** — `with_allocator(arena) { ... }`
- **Lifetime categories** — inform Pool design patterns

**Potential Feature: Temporary Allocator in Context**
```rask
// Jai-inspired: temp allocator for short-lived data
func process_frame() {
    const scratch = context.temp      // Per-frame scratch space
    const handles = pool.handles().collect_in(scratch)  // No heap alloc!
    for h in handles {
        pool[h].update()
    }
}   // scratch auto-cleared at frame end
```

This addresses **#11 (Iterator Allocation)** without hidden costs—`context.temp` is explicit.

---

### Summary: What to Take from Vale and Jai

| Source | Feature | Rask Adaptation | Metrics |
|--------|---------|-----------------|---------|
| **Vale** | Generational refs | Already have (handles) | — |
| **Vale** | Region freezing | Frozen pools | RO ✅ (zero checks) |
| **Vale** | Pure functions | `pure func` annotation | RO ✅ |
| **Jai** | Implicit context | **Rejected** (TC violation) | TC ❌ |
| **Jai** | Temp allocator | Scoped scratch space | TC ✅, ED ✅ |
| **Jai** | Lifetime categories | Inform Pool patterns | Documentation |

**Key insight from Vale:** Generation checks can be **eliminated entirely** for immutable data. This is huge for read-heavy workloads (rendering, queries, analysis).

**Key insight from Jai:** Short-lived allocations dominate most programs. A dedicated temp allocator solves this without hidden costs.

---

## Hylo Comparison: Rask's Competitive Edge

Hylo (formerly Val) is the closest "spiritual cousin" to Rask—both pursue value semantics without a traditional borrow checker. Understanding Hylo's limitations reveals Rask's opportunity.

### Feature Comparison

| Feature | Hylo | Rask |
|---------|------|------|
| Main Abstraction | Values & Purity | Handles & Pools |
| Memory Model | Scoped lifetimes (automatic) | Deterministic Pools (explicit) |
| Data Relationships | Tree-like (hard to do graphs) | Relational (graphs easy via handles) |
| Mutation | `inout` parameters | Ambient scoping / closures |
| Remote State | Difficult (pass by value) | Easy (channels pass handles) |
| Async Safety | Weak (stack-bound values) | Strong (pool-resident data) |

### The Core Insight: Spatial vs Logical Safety

> **Hylo** is about **Logical safety**: "This value belongs to this function."
>
> **Rask** is about **Spatial safety**: "This data lives in this Bin, and here is your Receipt."

Spatial safety is easier for humans to visualize. It's the difference between following a complex legal contract (Hylo) and just having a ticket to a concert (Rask).

### Hylo's Weaknesses = Rask's Opportunities

**1. Shared State:**
Hylo forces you to pass values back and forth. If two parts of your program need to talk about the same "User," it's verbose. Rask says: "Keep the User in the Pool, just pass the Receipt (Handle)."

**2. Graph Structures:**
Hylo's tree-like ownership makes graphs, neural nets, and game worlds awkward. Rask's handles naturally express relationships.

**3. Async:**
If a value is on the stack, moving it to a background thread is hard without GC. Rask's pool-resident data isn't stack-bound.

---

## Advanced Features to Surpass Hylo

### Feature 1: Relational Borrows (Joins)

Allow `with` blocks to enable cross-pool operations that the compiler optimizes.

```rask
// Rask can do "relational" logic that Hylo can't easily express
with (users, teams) {
    const u = users[h_user]
    const t = teams[u.team_id]    // Allowed: both pools are "active"
    t.score += u.points
}
```

**What it enables:**
- Database-like joins across entity types
- Game engines: access player AND their inventory in one block
- No need to pass multiple pools to every function

**Compiler optimization:**
- Verify no conflicting mutations (can't mutate `users` while reading `u`)
- Coalesce generation checks across related accesses

---

### Feature 2: Pinned Handles for Async (Sticky Scopes)

When sending a handle through a channel, automatically "pin" that generation so the pool can't reuse the slot until the receiving task is done.

```rask
const h = players.insert(Player{...})

// Sending pins the handle
channel.send(h)                    // Generation is now "sticky"

// Receiver can safely use it
spawn {
    const h = channel.recv()
    players[h].update()            // Guaranteed valid
}                                  // Unpin on task completion
```

**What it enables:**
- GC-level safety with Pool-level speed
- Safe async without lifetime annotations
- Solves #3 (Zombie Handles) for cross-task scenarios

**Implementation:**
- Reference count per slot (increment on send, decrement on task end)
- Slot reuse blocked while count > 0
- Zero cost for non-sent handles

---

### Feature 3: View Handles (Projected Access)

Generate handles that only allow access to specific fields—smaller, faster, and capability-restricted.

```rask
struct User {
    id: u64,
    name: string,
    email: string,
    password_hash: [u8; 32],
    profile: LargeProfileData,
}

// Full handle: 16 bytes, full access
let h: Handle<User> = users.insert(user)

// View handle: 8 bytes, restricted access
let email_view: ViewHandle<User, .email> = h.view(.email)

// Can only access email field
send_notification(email_view)

func send_notification(h: ViewHandle<User, .email>) {
    const email = users[h]          // Returns only the email field
}
```

**What it enables:**
- Principle of least privilege for handles
- Smaller handle sizes for specific use cases
- Safe capability restriction without wrapper types

---

### Feature 4: Implicit Pool Discovery

If a function takes a `Handle<User>`, the compiler finds the `Users` pool in the ambient scope without explicit `with`.

```rask
// Instead of:
func damage(h: Handle<Player>, players: Pool<Player>) {
    players[h].health -= 10
}

// Allow:
func damage(h: Handle<Player>) {
    h.health -= 10                 // Compiler finds ambient pool
}

// Caller just needs pool in scope:
with players {
    damage(player_handle)          // Pool discovered implicitly
}
```

**What it enables:**
- Dramatically reduces context passing (#2)
- Functions declare what handles they need, not what pools
- Caller controls which pool satisfies the requirement

**Rules:**
- Function signature: `Handle<T>` (pool implicit) vs `Handle<T>, Pool<T>` (pool explicit)
- Ambiguity = compile error (must disambiguate with explicit pool)
- Works with `with` blocks or explicit pool parameters

---

### Feature 5: Pool Partitioning for Concurrency

Split a pool into read-only and write-only views for safe multi-threaded access.

```rask
let players: Pool<Player> = Pool.new()

// Partition for parallel processing
let (readers, writer) = players.partition()

// Multiple readers can run in parallel
parallel for r in readers.chunks(4) {
    for h in r {
        analyze(r[h])              // Read-only access
    }
}

// Single writer has exclusive mutation
writer[h].health = 100

// Reunify
players = Pool.reunify(readers, writer)
```

**What it enables:**
- Hylo's `inout` is strictly single-threaded; Rask can be multi-threaded
- Safe parallel iteration without locks
- ECS-style parallel systems

**Variations:**
- `pool.read_only()` — returns view that can only read
- `pool.write_only()` — returns view that can only write
- `pool.split(predicate)` — partition by condition

---

## Revised Hybrid Approach (with Hylo-Beating Features)

Combine the original hybrid with advanced features:

### Tier 1: Core (Implement First)
1. **Ambient Pool Scoping** — `with pool { }` for ergonomic access
2. **Weak Handles** — Opt-in zombie prevention
3. **Compiler Optimizations** — Generation coalescing, cursor iteration
4. **Frozen Pools** (Vale-inspired) — `pool.freeze()` skips generation checks for read-only access

### Tier 2: Competitive Edge (Implement Second)
5. **Relational Borrows** — Multi-pool `with (a, b) { }` blocks
6. **Temp Allocator** (Jai-inspired) — `context.temp` for frame-local scratch space
7. **Pinned Handles** — Safe async via explicit `pool.pin(h)`

### Tier 2: Competitive Edge (Now Specified)
8. **Pool Partitioning** — Chunked frozen pools for parallel iteration, snapshot isolation for concurrent read-write (specified in pools.md)
9. **SliceDescriptor** — Built-in Handle + Range pattern for storable slices (specified in collections.md)

### Tier 3: Advanced (Future)
10. **View Handles** — Projected field access with capability restrictions
11. **Pinned Handles (async)** — Reference-counted slots for safe async handle passing

### Rejected
- **Implicit Pool Discovery** — Violates TC (magic resolution)
- **Full Implicit Context** — Violates TC (hidden dependencies)

---

## The Rask Pitch (vs Hylo)

| Problem | Hylo | Rask |
|---------|------|------|
| Graph structures | Awkward (tree-like) | Natural (handles) |
| Shared state | Pass values back and forth | Pass handles, data stays put |
| Async/channels | Hard (stack-bound) | Easy (pool-resident) |
| Multi-threading | Single-threaded `inout` | Pool partitioning |
| Mental model | Legal contract | Concert ticket |

**Rask's tagline:** "Keep your data in the Pool. Pass the Receipt."

---

## Next Steps

1. **Prototype ambient pool scoping** in a subset of examples
2. **Measure ergonomics** against Go/Odin/Hylo equivalents
3. **Verify TC compliance** — is `with pool { }` sufficiently explicit?
4. **Design weak handle semantics** — invalidation, checking, upgrade
5. **Spec cursor iteration** — safe removal, concurrent modification
6. **Prototype relational borrows** — multi-pool `with` blocks
7. **Design implicit pool discovery** — resolution rules, ambiguity handling
8. **Spec pinned handles** — reference counting, async integration
