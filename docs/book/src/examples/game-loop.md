# Game Loop

An entity-component system demonstrating handle-based indirection.

**Full source:** [game_loop.rk](https://github.com/rask-lang/rask/blob/main/examples/game_loop.rk)

## Key Concepts Demonstrated

- Entity-component system with `Pool<T>`
- Handle-based references (no pointers!)
- Game state management
- Frame-based update loop

## Highlights

### Entity Storage

<!-- test: parse -->
```rask
struct Entity {
    position: Position
    velocity: Velocity
    health: Health
    active: bool
    target: Handle<Entity>?   // a handle, not a reference — and optional
}

func spawn_pair() {
    mut entities = Pool.new()
    let player = entities.insert(Entity.new())
    let enemy = entities.insert(Entity.new())

    // Enemy targets player using handle
    entities[enemy].target = player
}
```

`Handle<Entity>?` is Rask's optional — there's no `Option<T>` wrapper type, and no `Some(...)`
constructor to write. A present value is just the value; absence is `none`.

`insert` returns the handle directly and panics if a bounded pool is full. When you'd rather
handle that, `try_insert` gives you `Handle<T> or InsertError<T>`:

<!-- test: parse -->
```rask
let h = try entities.try_insert(Entity.new())
```

### Update Loop

<!-- test: parse -->
```rask
func movement_system(mutate entities: Pool<Entity>, dt: f32) {
    for h in entities {
        if !entities[h].active: continue

        entities[h].position.x += entities[h].velocity.dx * dt
        entities[h].position.y += entities[h].velocity.dy * dt
    }
}
```

Each `entities[h]` access is expression-scoped — the borrow ends with the expression, so
nothing is held across the next access. That's what lets the two lines above mutate the same
entity in sequence.

`mutate` marks the parameter the function writes through, and callers repeat it at the call
site — `movement_system(mutate entities, dt)` — so mutation is visible on both ends without
looking up the signature. (Method receivers are exempt: `pool.insert(x)` needs no marker.)

Passing the pool as a `mutate` parameter is one option. The other is a context clause, which
threads the pool as a hidden parameter so handles resolve on their own:

<!-- test: parse -->
```rask
func movement_system(dt: f32) using entities: Pool<Entity> {
    // Handle<Entity> access resolves against `entities` automatically
}
```

Use `using frozen Pool<Entity>` for read-only systems — the compiler rejects any insert,
remove, or write, and can then drop the generation checks entirely.

### Why Handles Work

Unlike references, handles:
- Can be stored in structs
- Can form cycles (entity targets another)
- Are validated at runtime (pool ID + generation)
- Don't need lifetime annotations

## Running It

```bash
rask run examples/game_loop.rk
```

## What You'll Learn

- How to use `Pool<T>` for entity systems
- Handle-based indirection patterns
- Expression-scoped borrowing for collections
- Game loop structure in Rask

[View full source →](https://github.com/rask-lang/rask/blob/main/examples/game_loop.rk)
