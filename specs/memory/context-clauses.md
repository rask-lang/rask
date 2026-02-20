<!-- id: mem.context -->
<!-- status: decided -->
<!-- summary: using clauses declare pool dependencies; compiler threads as hidden parameters -->
<!-- depends: memory/pools.md, memory/borrowing.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Context Clauses

`using` clauses declare a function's pool dependencies without explicit passing. Compiler threads contexts as hidden parameters — zero overhead, compile-time checked.

## Syntax

| Rule | Form | Syntax | Effect |
|------|------|--------|--------|
| **CC1: Named context** | `using name: Pool<T>` | `func f() using players: Pool<Player>` | Creates binding `players` + enables auto-resolution for `Handle<Player>` |
| **CC2: Unnamed context** | `using Pool<T>` | `func f() using Pool<Player>` | Auto-resolution only, no binding for structural ops |
| **CC3: Frozen context** | `using frozen Pool<T>` | `func f() using frozen Pool<Player>` | Read-only, accepts both `Pool<T>` and `FrozenPool<T>` |

Without `frozen`, contexts are mutable by default — writes through handles are allowed, but only `Pool<T>` satisfies the context (not `FrozenPool<T>`). See `mem.pools` for the full subsumption rule.

<!-- test: skip -->
```rask
// Named — binding + auto-resolution
func award_bonus(h: Handle<Player>, amount: i32)
    using players: Pool<Player>
{
    h.score += amount              // Auto-resolves via players context
    players.mark_dirty(h)          // Named binding for structural op
}

// Unnamed — auto-resolution only
func damage(h: Handle<Player>, amount: i32) using Pool<Player> {
    h.health -= amount        // Auto-resolves
    // players.remove(h)      // ERROR: 'players' not in scope
}

// Frozen — read-only
func get_health(h: Handle<Player>) using frozen Pool<Player> -> i32 {
    return h.health
}

// Multiple contexts
func transfer_item(
    player_h: Handle<Player>,
    item_h: Handle<Item>
) using players: Pool<Player>, items: Pool<Item> {
    player_h.inventory.push(item_h)
    item_h.owner = Some(player_h)
}
```

**Full signature grammar:**

<!-- test: skip -->
```rask
[public] func name<Generics>(params) -> ReturnType
    using [frozen] [context_name:] ContextType, ...
    where TraitBounds
{ body }
```

Order: generics, parameters, return type, `using` clause, `where` clause, body.

## Resolution

| Rule | Description |
|------|-------------|
| **CC4: Resolution order** | At call sites, compiler searches: local variables, function parameters, fields of `self`, own `using` clause |
| **CC5: Propagation** | A function's `using` clause satisfies callees requiring the same context type |
| **CC8: Ambiguity error** | Multiple pools of the same type in scope is a compile error — pass explicitly |

<!-- test: skip -->
```rask
func game_tick() {
    const players = Pool.new()
    const h = try players.insert(Player.new())
    damage(h, 10)    // CC4: compiler finds local `players`, passes it
}

struct Game {
    players: Pool<Player>,
}

extend Game {
    func tick(self) {
        for h in self.players.cursor() {
            damage(h, 10)    // CC4: compiler finds self.players
        }
    }
}

// CC5: context propagates through call chain
func update_player(h: Handle<Player>) using Pool<Player> {
    take_damage(h, 5)        // Propagates Pool<Player> context
    check_death(h)           // Propagates Pool<Player> context
}

func take_damage(h: Handle<Player>, amount: i32) using Pool<Player> {
    h.health -= amount
}

func check_death(h: Handle<Player>) using players: Pool<Player> {
    if h.health <= 0 {
        players.remove(h)
    }
}
```

Both named and unnamed contexts satisfy context requirements — the name is local to the function.

## Compiler Desugaring

<!-- test: skip -->
```rask
// What you write:
func damage(h: Handle<Player>, amount: i32) using players: Pool<Player> {
    h.health -= amount
}

// What the compiler generates (conceptual):
func damage(h: Handle<Player>, amount: i32, __ctx_players: &Pool<Player>) {
    __ctx_players[h].health -= amount
}
```

Call sites automatically pass the pool as a hidden argument. No runtime lookups, no registry.

## Visibility

| Rule | Description |
|------|-------------|
| **CC6: Public declaration required** | Public functions must declare `using` clauses — part of API contract |
| **CC7: Private inference** | Private functions can have unnamed contexts inferred from handle field access |

The compiler can infer an unnamed context from field access, but cannot infer a name. If the body uses a pool for structural operations, the name must be declared.

<!-- test: skip -->
```rask
// ERROR: public function with Handle parameter needs explicit context
public func damage(h: Handle<Player>, amount: i32) {
    h.health -= amount
}

// OK: context declared
public func damage(h: Handle<Player>, amount: i32) using Pool<Player> {
    h.health -= amount
}

// Private: compiler infers `using Pool<Player>` from h.health usage
func damage(h: Handle<Player>, amount: i32) {
    h.health -= amount
}

// But structural operations need explicit name:
func kill(h: Handle<Player>) using players: Pool<Player> {
    h.on_death()
    players.remove(h)
}
```

## Closures

| Rule | Description |
|------|-------------|
| **CC9: Immediate closure inheritance** | Expression-scoped closures (iterators, immediate callbacks) inherit enclosing contexts |
| **CC10: Storable closure exclusion** | Storable closures cannot use auto-resolution — pools must be explicit parameters |

<!-- test: parse -->
```rask
// CC9: expression-scoped inherits context
func process_all(handles: Vec<Handle<Player>>) using Pool<Player> {
    for h in handles {
        h.score += 10    // Pool<Player> context inherited
    }
}

// CC10: storable closure cannot capture context
const callback: |Handle<Player>| = |h| {
    h.health -= 10    // ERROR: no Pool<Player> context
}

// OK: pool passed explicitly
const callback: |Pool<Player>, Handle<Player>| = |pool, h| {
    pool[h].health -= 10
}
```

## Generic Functions

Context clauses work with generics:

<!-- test: skip -->
```rask
public func process_all<T>(handles: Vec<Handle<T>>)
    using Pool<T>
    where T: Processable
{
    for h in handles {
        h.process()
    }
}
```

## Error Messages

**Missing context on public function [CC6]:**
```
ERROR [mem.context/CC6]: public function with Handle parameter needs context
   |
1  |  public func damage(h: Handle<Player>, amount: i32) {
   |                      ^^^^^^^^^^^^^^^^ Handle<Player> requires pool context
   |
FIX: Add a using clause:

  public func damage(h: Handle<Player>, amount: i32) using Pool<Player> {
```

**Ambiguous context [CC8]:**
```
ERROR [mem.context/CC8]: ambiguous context — multiple Pool<Player> in scope
   |
3  |  const pool_a = Pool::<Player>.new()
4  |  const pool_b = Pool::<Player>.new()
6  |  damage(h, 10)
   |  ^^^^^^^^^^^ which pool satisfies Pool<Player>?

FIX: Pass the pool explicitly as a regular parameter:

  func damage_explicit(pool: Pool<Player>, h: Handle<Player>, amount: i32) {
      pool[h].health -= amount
  }
```

**Storable closure context [CC10]:**
```
ERROR [mem.context/CC10]: storable closure cannot use context auto-resolution
   |
1  |  const callback: |Handle<Player>| = |h| {
2  |      h.health -= 10
   |      ^ no Pool<Player> context available

WHY: Storable closures may execute where pool availability can't be
     guaranteed at compile time.

FIX: Pass the pool as an explicit parameter to the closure.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Multiple pools of same type in scope | CC8 | Compile error — pass explicitly |
| Cross-thread handle sent via channel | CC4 | Runtime check — panics if pool on different thread |
| Recursive function with context | CC5 | Context propagates through recursive calls |
| Context + error propagation (`try`) | — | Works normally; `try` is orthogonal |
| Context + resource types | — | Contexts are passed by reference, cannot be consumed |
| Contexts at comptime | — | Not applicable — pools cannot be used at comptime |

---

## Appendix (non-normative)

### Rationale

**CC1 (named contexts):** Enables both auto-resolution and structural operations with a single mechanism. Without named contexts, you'd need separate approaches for field access and pool mutations.

**CC2 (unnamed contexts):** Most handle usage is field access. Unnamed contexts eliminate the need to pick a name when you don't need one.

**CC5 (propagation):** Pool contexts are ambient — they thread through many layers. Treating them as special reduces signature clutter compared to explicit parameters.

**CC6/CC7 (public vs private):** Follows the gradual constraints pattern. Private functions infer to reduce ceremony; public functions declare for API clarity.

**CC10 (storable closure exclusion):** Storable closures may execute in contexts where pool availability can't be verified at compile time. Requiring explicit parameters makes the dependency visible.

**Why compile-time threading instead of runtime registry?** Eliminates overhead, turns runtime panics into compile errors, and makes dependencies explicit in signatures.

**Why `using` not `with`?** Reserves `with` for block-scoped constructs (element binding, runtime context blocks). `using` is for function-level declarations only — clean separation.

### Patterns & Guidance

**Call chain propagation:**
<!-- test: skip -->
```rask
// Top-level: has the pool
func game_loop() {
    const players = Pool.new()
    for h in players.cursor() {
        update_player(h, 0.016)    // Passes players implicitly
    }
}

// Mid-level: propagates context
func update_player(h: Handle<Player>, dt: f32) using Pool<Player> {
    apply_physics(h, dt)
    check_collisions(h)
}

// Low-level: uses context
func apply_physics(h: Handle<Player>, dt: f32) using Pool<Player> {
    h.velocity.y -= 9.8 * dt
    h.position += h.velocity * dt
}
```

**Method access via `self`:**
<!-- test: skip -->
```rask
struct GameWorld {
    players: Pool<Player>,
    enemies: Pool<Enemy>,
}

extend GameWorld {
    public func tick(self, dt: f32) {
        for h in self.players.cursor() {
            update_player(h, dt)    // self.players provides context
        }
        for h in self.enemies.cursor() {
            update_enemy(h, dt)     // self.enemies provides context
        }
    }
}
```

**Disambiguation when ambiguous:** Fall back to explicit pool parameters.
<!-- test: skip -->
```rask
func damage_explicit(pool: Pool<Player>, h: Handle<Player>, amount: i32) {
    pool[h].health -= amount
}

damage_explicit(pool_a, h, 10)
```

### IDE Integration

The IDE displays inferred contexts as ghost text:

<!-- test: skip -->
```rask
func damage(h: Handle<Player>, amount: i32) {    // ghost: using Pool<Player>
    h.health -= amount
}
```

Quick action: "Make context explicit" fills in the inferred `using` clause.

### See Also

- [Pools](pools.md) — Handle-based sparse storage (`mem.pools`)
- [Borrowing](borrowing.md) — Expression-scoped views, `with` element binding (`mem.borrowing`)
- [Resource Types](resource-types.md) — Must-consume types (`mem.resources`)
- [Closures](closures.md) — Closure capture semantics (`mem.closures`)
- [Async](../concurrency/async.md) — `using Multitasking` runtime contexts (`conc.async`)
