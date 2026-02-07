# Game Loop

An entity-component system demonstrating handle-based indirection.

**Full source:** [game_loop.rk](https://github.com/dritory/rask/blob/main/examples/game_loop.rk)

## Key Concepts Demonstrated

- Entity-component system with `Pool<T>`
- Handle-based references (no pointers!)
- Game state management
- Frame-based update loop

## Highlights

### Entity Storage

```rask
struct Entity {
    pos: Vec2,
    vel: Vec2,
    health: i32,
    target: Option<Handle<Entity>>,  // Handle, not reference!
}

const entities = Pool.new()
const player = try entities.insert(Entity.new())
const enemy = try entities.insert(Entity.new())

// Enemy targets player using handle
entities[enemy].target = Some(player)
```

### Update Loop

```rask
func update(delta: f32) with entities: Pool<Entity> {
    for h in entities {
        entities[h].pos.x += entities[h].vel.x * delta
        entities[h].pos.y += entities[h].vel.y * delta

        // Handle AI, collision, etc.
    }
}
```

Each `entities[h]` access is expression-scoped - the borrow ends at the semicolon. This allows mutation between accesses.

### Why Handles Work

Unlike references, handles:
- Can be stored in structs
- Can form cycles (entity targets another)
- Are validated at runtime (pool ID + generation)
- Don't need lifetime annotations

## Running It

```bash
rask game_loop.rk
```

## What You'll Learn

- How to use `Pool<T>` for entity systems
- Handle-based indirection patterns
- Expression-scoped borrowing for collections
- Game loop structure in Rask

[View full source →](https://github.com/dritory/rask/blob/main/examples/game_loop.rk)
