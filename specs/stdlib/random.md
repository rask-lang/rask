# Random — Random Number Generation

One `Rng` type plus module-level convenience functions. Explicit generator for reproducible sequences, quick module functions for when you just need a random number.

## Specification

### Types

| Type | Description | Size | Copy? |
|------|-------------|------|-------|
| `Rng` | Pseudo-random number generator | 32 bytes | No (stateful) |

### Rng Constructors

```rask
Rng.new() -> Rng                // system-seeded (time + thread ID)
Rng.from_seed(seed: u64) -> Rng // deterministic — same seed = same sequence
```

### Rng Instance Methods

```rask
rng.u64() -> u64                    // full u64 range
rng.i64() -> i64                    // full i64 range
rng.range(lo: i64, hi: i64) -> i64  // [lo, hi) — lo inclusive, hi exclusive
rng.f64() -> f64                    // [0.0, 1.0)
rng.f32() -> f32                    // [0.0, 1.0)
rng.bool() -> bool                  // 50/50
rng.shuffle(vec: Vec<T>)            // in-place Fisher-Yates shuffle
rng.choice(vec: Vec<T>) -> T?       // random element, None if empty
```

### Module Convenience Functions

These use a thread-local system-seeded Rng:

```rask
random.u64() -> u64
random.i64() -> i64
random.range(lo: i64, hi: i64) -> i64
random.f64() -> f64
random.f32() -> f32
random.bool() -> bool
random.shuffle(vec: Vec<T>)
random.choice(vec: Vec<T>) -> T?
```

### Access Pattern

```rask
import random

// Quick usage — module functions
const roll = random.range(1, 7)   // dice roll [1, 6]
const coin = random.bool()

// Reproducible — explicit Rng
const rng = Rng.from_seed(42)
const a = rng.range(0, 100)       // always same value for seed 42
const b = rng.range(0, 100)       // always same second value
```

## Examples

### Game — Random Enemy Spawning

```rask
import random
import time

func spawn_enemies(count: i64, area_width: f64, area_height: f64) -> Vec<Enemy> {
    const enemies = Vec.new()
    for _ in 0..count {
        const enemy = Enemy {
            x: random.f64() * area_width,
            y: random.f64() * area_height,
            health: random.range(50, 101),
        }
        try enemies.push(enemy)
    }
    return enemies
}
```

### Deterministic Tests

```rask
import random

test "shuffle is deterministic with seed" {
    const rng = Rng.from_seed(12345)
    const items = Vec.from([1, 2, 3, 4, 5])
    rng.shuffle(items)

    // Same seed always produces same shuffle
    assert items[0] == 3
    assert items[1] == 1
}
```

### Weighted Random Selection

```rask
import random

func weighted_choice(weights: Vec<f64>) -> i64 {
    let total = 0.0
    for w in weights.iter() {
        total += w
    }

    let r = random.f64() * total
    for i in 0..weights.len() {
        r -= weights[i]
        if r <= 0.0 {
            return i
        }
    }
    return weights.len() - 1
}
```

### Shuffle a Deck

```rask
import random

func new_deck() -> Vec<string> {
    const suits = ["Hearts", "Diamonds", "Clubs", "Spades"]
    const ranks = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"]
    const deck = Vec.new()
    for suit in suits {
        for rank in ranks {
            try deck.push("{rank} of {suit}")
        }
    }
    random.shuffle(deck)
    return deck
}
```

## Edge Cases

- `random.range(5, 5)` — panics (empty range). Use `random.range(5, 6)` for a single value
- `random.choice(empty_vec)` — returns `None`
- `random.shuffle(single_element_vec)` — no-op
- `Rng.from_seed(0)` — valid, produces deterministic sequence (seed 0 is not special)

## Security Note

`Rng` and `random.*` are NOT cryptographically secure. Do not use for:
- Password generation
- Session tokens
- Encryption keys
- Any security-sensitive random

A future `crypto.random_bytes(n)` will provide cryptographic randomness. For now, use platform APIs via unsafe if needed.

## Algorithm

xoshiro256++ (Blackman & Vigna, 2019):
- State: 4 × u64 = 32 bytes
- Period: 2^256 - 1
- Passes BigCrush and PractRand statistical tests
- ~4 CPU cycles per 64-bit number

## References

- specs/stdlib/testing.md — Seeded random per-test for reproducibility
- CORE_DESIGN.md — Transparent cost (RNG is pure computation, no syscall after init)

## Status

**Specified** — ready for implementation in interpreter.
