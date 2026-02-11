<!-- id: std.random -->
<!-- status: decided -->
<!-- summary: Pseudo-random number generation via Rng type and module convenience functions -->

# Random

One `Rng` type plus module-level convenience functions. Explicit generator for reproducible sequences, module functions for quick usage.

## Types

| Rule | Description |
|------|-------------|
| **R1: Rng type** | `Rng` is a 32-byte stateful PRNG; not Copy |
| **R2: System seed** | `Rng.new()` seeds from system entropy (time + thread ID) |
| **R3: Deterministic seed** | `Rng.from_seed(seed: u64)` produces identical sequences for identical seeds |

## Instance Methods

| Rule | Description |
|------|-------------|
| **M1: Typed generation** | `rng.u64()`, `rng.i64()`, `rng.f64()`, `rng.f32()`, `rng.bool()` generate typed random values |
| **M2: Range** | `rng.range(lo, hi)` returns value in `[lo, hi)` — panics if `lo >= hi` |
| **M3: Collections** | `rng.shuffle(vec)` does in-place Fisher-Yates; `rng.choice(vec)` returns `T?` (None if empty) |

<!-- test: skip -->
```rask
const rng = Rng.from_seed(42)
const a = rng.range(0, 100)       // deterministic for seed 42
const b = rng.f64()               // [0.0, 1.0)
rng.shuffle(items)
```

## Module Convenience Functions

| Rule | Description |
|------|-------------|
| **C1: Thread-local RNG** | `random.*` functions use a thread-local system-seeded Rng |
| **C2: Same API** | `random.u64()`, `random.range(lo, hi)`, `random.f64()`, `random.bool()`, `random.shuffle(vec)`, `random.choice(vec)` mirror instance methods |

<!-- test: skip -->
```rask
import random

const roll = random.range(1, 7)   // dice roll [1, 6]
const coin = random.bool()
```

## Error Messages

```
ERROR [std.random/M2]: empty range in random.range()
   |
5  |  const x = rng.range(5, 5)
   |                      ^^^^ lo must be less than hi

WHY: Half-open range [lo, hi) is empty when lo >= hi.

FIX: Use rng.range(5, 6) for a single value.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `range(5, 5)` | M2 | Panics (empty range) |
| `choice(empty_vec)` | M3 | Returns `None` |
| `shuffle(single_element)` | M3 | No-op |
| `Rng.from_seed(0)` | R3 | Valid, deterministic (seed 0 not special) |

---

## Appendix (non-normative)

### Rationale

**R1 (not Copy):** Rng is stateful — copying would fork the sequence silently, leading to duplicate values. Move semantics make sequence ownership explicit.

**C1 (thread-local):** Module functions cover the "just give me a random number" case without requiring Rng construction. Thread-local avoids synchronization cost.

### Patterns & Guidance

**Deterministic tests:**

<!-- test: skip -->
```rask
test "shuffle is deterministic with seed" {
    const rng = Rng.from_seed(12345)
    const items = Vec.from([1, 2, 3, 4, 5])
    rng.shuffle(items)
    assert items[0] == 3
}
```

**Weighted random selection:**

<!-- test: skip -->
```rask
func weighted_choice(weights: Vec<f64>) -> i64 {
    let r = random.f64() * weights.iter().sum()
    for i in 0..weights.len() {
        r -= weights[i]
        if r <= 0.0 { return i }
    }
    return weights.len() - 1
}
```

### Security

`Rng` and `random.*` are NOT cryptographically secure. Do not use for passwords, tokens, keys, or any security-sensitive random. A future `crypto.random_bytes(n)` will provide cryptographic randomness.

### Algorithm

xoshiro256++ (Blackman & Vigna, 2019): 4 x u64 state (32 bytes), period 2^256 - 1, passes BigCrush and PractRand, ~4 cycles per u64.

### See Also

- `std.testing` — Seeded random per-test for reproducibility
