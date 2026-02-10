# Context Clauses

## Overview

Context clauses declare a function needs a pool without passing it explicitly. Compiler threads contexts as hidden parameters—compile-time checking, no ceremony of passing pools through call chains.

**Key properties:**
- Pool dependencies visible in public signatures
- Compile-time checking at every call site
- Zero runtime overhead (direct parameter passing, no registry)
- Named contexts enable field access and structural operations

## Motivation

Without context clauses, handle code has two pain points:

**Problem 1: Pool threading ceremony**
```rask
// Every function in the call chain needs the pool
func update_score(players: Pool<Player>, h: Handle<Player>, points: i32) {
    players[h].score += points
    check_achievements(players, h)    // Pass it along
}

func check_achievements(players: Pool<Player>, h: Handle<Player>) {
    if players[h].score > 1000 {
        grant_reward(players, h, Reward.GoldTrophy)
    }
}

func grant_reward(players: Pool<Player>, h: Handle<Player>, reward: Reward) {
    players[h].rewards.push(reward)
}
```

**Problem 2: Split access patterns**
```rask
func cleanup_dead(players: Pool<Player>) {
    for h in players.cursor() {
        if players[h].health <= 0 {    // Need pool for field access
            players.remove(h)           // Need pool for structural op
        }
    }
}
```

Context clauses solve both: functions declare pool requirements once, callers provide pools implicitly, and the function body uses a single mechanism for both field access and structural operations.

## Syntax

### Basic Form

```rask
func function_name(params) with pool_name: Pool<T> {
    // pool_name is available as local binding
    // Handles of type Handle<T> auto-resolve via this pool
}
```

### Unnamed Context

When only field access is needed (no structural operations):

```rask
func function_name(params) with Pool<T> {
    // Handles of type Handle<T> auto-resolve
    // But no binding available for pool.insert/remove/etc
}
```

### Multiple Contexts

```rask
func function_name(params)
    with players: Pool<Player>,
         items: Pool<Item>
{
    // Both pools available
}
```

### Frozen Context

Add `frozen` to mark a context as read-only. Frozen contexts accept both `Pool<T>` and `FrozenPool<T>`:

```rask
// Read-only — accepts Pool or FrozenPool
func get_health(h: Handle<Player>) with frozen Pool<Player> -> i32 {
    return h.health
}

// Named frozen — read-only with pool binding
func count_all() with frozen players: Pool<Player> -> i32 {
    return players.len()
}
```

Without `frozen`, contexts are mutable by default — writes through handles are allowed, but only `Pool<T>` satisfies the context (not `FrozenPool<T>`).

See [pools.md](pools.md#frozenpool-context-subsumption) for the full subsumption rule.

### Full Signature Grammar

```rask
[public] func name<Generics>(params) -> ReturnType
    with [frozen] context_name: ContextType, ...
    where TraitBounds
{ body }
```

Order: generics → parameters → return type → `with` clause → `where` clause → body.

## Semantics

### Named Context Binding

A named context like `with players: Pool<Player>` creates:
1. A local binding `players` usable for structural operations
2. Auto-resolution for any `Handle<Player>` field access in the function body

```rask
func award_bonus(h: Handle<Player>, amount: i32)
    with players: Pool<Player>
{
    h.score += amount              // Auto-resolves via players context
    players.mark_dirty(h)          // Named binding for structural op
}
```

### Unnamed Context Auto-Resolution

An unnamed context like `with Pool<Player>` enables only auto-resolution:

```rask
func damage(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health -= amount        // Auto-resolves
    // players.remove(h)      // ERROR: 'players' not in scope
}
```

Use unnamed contexts when the function only reads/writes handle fields.

### Compiler Desugaring

```rask
// What you write:
func damage(h: Handle<Player>, amount: i32) with players: Pool<Player> {
    h.health -= amount
}

// What the compiler generates (conceptual):
func damage(h: Handle<Player>, amount: i32, __ctx_players: &Pool<Player>) {
    __ctx_players[h].health -= amount
}
```

Call sites automatically pass the pool as a hidden argument. No runtime lookups, no registry.

### Context Resolution at Call Sites

When the compiler sees a function call requiring contexts, it searches in this order:

1. **Local variables** of matching pool type
2. **Function parameters** of matching pool type
3. **Fields of `self`** (in methods) of matching pool type
4. **Own `with` clause** of matching pool type (propagation)

```rask
func game_tick() {
    const players = Pool.new()
    const h = try players.insert(Player.new())

    damage(h, 10)    // ✅ Compiler finds local `players`, passes it
}

struct Game {
    players: Pool<Player>,
}

extend Game {
    func tick(self) {
        for h in self.players.cursor() {
            damage(h, 10)    // ✅ Compiler finds self.players, passes it
        }
    }
}
```

### Context Propagation

Functions with contexts can call other functions with matching contexts:

```rask
func update_player(h: Handle<Player>) with Pool<Player> {
    take_damage(h, 5)        // ✅ Propagates Pool<Player> context
    check_death(h)           // ✅ Propagates Pool<Player> context
}

func take_damage(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health -= amount
}

func check_death(h: Handle<Player>) with players: Pool<Player> {
    if h.health <= 0 {
        players.remove(h)
    }
}
```

Both named and unnamed contexts satisfy context requirements — the name is local to the function.

## Public vs Private Functions

Following the gradual constraints pattern:

| Visibility | Context Clause | Notes |
|------------|----------------|-------|
| **Public** | Required if body uses handles | Part of API contract |
| **Private** | Inferred from body | Can be explicit for clarity |

### Public: Must Declare

```rask
// ERROR: public function with Handle parameter needs explicit context
public func damage(h: Handle<Player>, amount: i32) {
    h.health -= amount
}

// OK: context declared
public func damage(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health -= amount
}
```

### Private: Inferred

```rask
// Private: compiler infers `with Pool<Player>` from h.health usage
func damage(h: Handle<Player>, amount: i32) {
    h.health -= amount
}

// But if structural operations needed, name must be explicit:
func kill(h: Handle<Player>) with players: Pool<Player> {
    h.on_death()
    players.remove(h)    // ERROR without explicit `with players:`
}
```

**Inference rule:** The compiler can infer an unnamed context from field access, but it **cannot** infer a name. If the body uses an identifier that looks like a pool name for structural operations, that identifier must be declared in the signature.

### IDE Support

The IDE displays inferred contexts as ghost text:

```rask
func damage(h: Handle<Player>, amount: i32) {    // ghost: with Pool<Player>
    h.health -= amount
}
```

Quick action: "Make context explicit" fills in the inferred `with` clause.

## Examples

### Simple Field Access

```rask
public func heal(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health = min(h.health + amount, h.max_health)
}
```

### Structural Operations

```rask
public func spawn_wave(count: i32, pos: Vec3)
    with enemies: Pool<Enemy>
    -> Vec<Handle<Enemy>> or PoolFull
{
    let handles = Vec.new()
    for i in 0..count {
        const offset = Vec3.new(i * 10.0, 0.0, 0.0)
        const h = try enemies.insert(Enemy.new(pos + offset))
        handles.push(h)
    }
    handles
}
```

### Multiple Pools

```rask
public func transfer_item(
    player_h: Handle<Player>,
    item_h: Handle<Item>
) with players: Pool<Player>, items: Pool<Item> {
    player_h.inventory.push(item_h)
    item_h.owner = Some(player_h)

    if players.len() > 100 {
        players.compact()    // Structural operation
    }
}
```

### Context Propagation Through Call Chain

```rask
// Top-level: has the pool
func game_loop() {
    const players = Pool.new()
    // ... populate pool ...

    for h in players.cursor() {
        update_player(h, 0.016)    // Passes players implicitly
    }
}

// Mid-level: propagates context
func update_player(h: Handle<Player>, dt: f32) with Pool<Player> {
    apply_physics(h, dt)
    check_collisions(h)
    update_animation(h)
}

// Low-level: uses context
func apply_physics(h: Handle<Player>, dt: f32) with Pool<Player> {
    h.velocity.y -= 9.8 * dt
    h.position += h.velocity * dt
}
```

### Method Access via `self`

```rask
struct GameWorld {
    players: Pool<Player>,
    enemies: Pool<Enemy>,
}

extend GameWorld {
    public func tick(self, dt: f32) {
        // self.players satisfies context requirements
        for h in self.players.cursor() {
            update_player(h, dt)    // ✅ self.players provides context
        }

        for h in self.enemies.cursor() {
            update_enemy(h, dt)     // ✅ self.enemies provides context
        }
    }
}

func update_player(h: Handle<Player>, dt: f32) with Pool<Player> {
    h.position += h.velocity * dt
}

func update_enemy(h: Handle<Enemy>, dt: f32) with Pool<Enemy> {
    h.position += h.velocity * dt
}
```

## Closures

### Expression-Scoped Closures

Expression-scoped closures (used in iterators, immediate callbacks) inherit the enclosing function's contexts:

```rask
func process_all(handles: Vec<Handle<Player>>) with Pool<Player> {
    handles.iter().for_each(|h| {
        h.score += 10    // ✅ Pool<Player> context inherited
    })
}
```

### Storable Closures

Storable closures **cannot** use auto-resolution. They must receive pools as explicit parameters:

```rask
// ERROR: storable closure can't capture context
const callback: |Handle<Player>| = |h| {
    h.health -= 10    // ERROR: no Pool<Player> context
}

// OK: pool passed explicitly when closure is called
const callback: |Pool<Player>, Handle<Player>| = |pool, h| {
    pool[h].health -= 10
}
```

This is intentional — storable closures may execute in different contexts where pool availability cannot be guaranteed at compile time.

## Edge Cases

### Ambiguous Context

If multiple pools of the same type are in scope, the compiler cannot choose:

```rask
func broken() {
    const pool_a = Pool::<Player>.new()
    const pool_b = Pool::<Player>.new()

    // ERROR: ambiguous — which pool satisfies Pool<Player>?
    damage(h, 10)
}
```

**Solution:** Pass the pool explicitly as a regular parameter:

```rask
func damage_explicit(pool: Pool<Player>, h: Handle<Player>, amount: i32) {
    pool[h].health -= amount
}

damage_explicit(pool_a, h, 10)    // ✅ Clear
```

### Cross-Thread Access

Contexts are thread-local. Handles sent to other threads cannot auto-resolve:

```rask
func worker_thread() with Pool<Player> {
    const h = receive_handle_from_channel()

    // ✅ Works if pool was moved to this thread
    h.health -= 10

    // ❌ Panic if pool stayed in another thread
}
```

**Design note:** This is a runtime check (same category as bounds checks). The compiler cannot statically verify which thread owns a pool, since pools can be moved at runtime.

### Recursive Context Requirements

```rask
func recursive(h: Handle<Node>, depth: i32) with nodes: Pool<Node> {
    if depth <= 0: return

    const child = h.first_child
    if child? {
        recursive(child, depth - 1)    // ✅ Propagates nodes context
    }
}
```

## Interaction with Other Features

### Generic Functions

Context clauses work with generics:

```rask
public func process_all<T>(handles: Vec<Handle<T>>)
    with Pool<T>
    where T: Processable
{
    for h in handles {
        h.process()
    }
}
```

### Error Propagation

`try` works normally in functions with contexts:

```rask
func spawn_entity(pos: Vec3)
    with entities: Pool<Entity>
    -> Handle<Entity> or PoolFull
{
    const h = try entities.insert(Entity.new(pos))
    h.active = true
    h
}
```

### Resource Types

Contexts are not resource types — they are passed as read-only references:

```rask
func cleanup() with players: Pool<Player> {
    players.clear()    // ✅ Mutates via read-only ref (interior mutability)
    // take players    // ERROR: can't consume context
}
```

### Comptime

Pools cannot be used at comptime, so context clauses have no comptime interaction.

## Performance

| Aspect | Cost | Notes |
|--------|------|-------|
| Auto-resolution | Zero | Direct field access via hidden parameter |
| Call overhead | Zero | Same as passing a reference parameter |
| Handle validation | 1-2 cycles | Generation + bounds check (same as before) |

Context clauses eliminate the thread-local registry lookup (~1-2ns) from the previous design.

## Comparison to Alternatives

### vs Explicit Pool Parameters

```rask
// Explicit pool parameters (alternative design)
func damage(pool: Pool<Player>, h: Handle<Player>, amount: i32) {
    pool[h].health -= amount
}

// With context clauses (current design)
func damage(h: Handle<Player>, amount: i32) with Pool<Player> {
    h.health -= amount
}
```

Context clauses reduce visual noise and match the "handles are first-class" mental model. Explicit parameters are still available for disambiguation or preference.

### vs Thread-Local Registry (Previous Design)

| Aspect | Registry (Old) | Context Clauses (New) |
|--------|----------------|------------------------|
| Pool lookup | Runtime (~1-2ns) | Compile-time (zero cost) |
| Missing pool | Runtime panic | Compile error |
| Wrong thread | Runtime panic | Compile error (or runtime if pool moved) |
| Signature visibility | Hidden dependency | Explicit in public signatures |
| Debugging | Opaque | Clear from signature |

Context clauses turn runtime errors into compile errors while eliminating overhead.

## FrozenPool Subsumption

`FrozenPool<T>` satisfies `frozen` context clauses. Without `frozen`, only `Pool<T>` is accepted.

```rask
func get_health(h: Handle<Entity>) with frozen Pool<Entity> -> i32 {
    return h.health
}

func render_all(entities: FrozenPool<Entity>) {
    for h in entities.handles() {
        const hp = get_health(h)    // ✅ FrozenPool satisfies frozen context
        draw_health_bar(hp)
    }
}
```

See [pools.md](pools.md#frozenpool-context-subsumption) for the full rule.

## Design Rationale

**Why named contexts?** Enables both auto-resolution and structural operations with a single mechanism.

**Why inference for private functions?** Reduces ceremony in implementation code (same philosophy as gradual constraints for trait bounds).

**Why separate from parameters?** Pool contexts are ambient — they thread through many layers. Treating them as special reduces signature clutter.

**Why compile-time threading instead of runtime registry?** Eliminates overhead, turns panics into compile errors, and makes dependencies explicit.

## Integration Notes

- **Principle 5 (Local Analysis):** Context requirements are part of the signature and checked at every call site — no whole-program analysis.
- **Principle 7 (Compiler Knowledge is Visible):** IDE displays inferred contexts as ghost text.
- **Gradual Constraints:** Private functions infer contexts from body; public functions require explicit declaration.
- **Type System:** Context clauses are orthogonal to trait bounds — both can appear in the same signature.
