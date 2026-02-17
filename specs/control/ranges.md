<!-- id: ctrl.ranges -->
<!-- status: decided -->
<!-- summary: Half-open and inclusive ranges with step, reverse, and infinite iteration -->
<!-- depends: control/loops.md, types/iterator-protocol.md -->

# Range Iteration

Half-open (`0..n`) and inclusive (`0..=n`) ranges with step, reverse, and infinite variants.

## Range Types

| Rule | Description |
|------|-------------|
| **R1: Half-open** | `0..n` iterates [0, n) — excludes end |
| **R2: Inclusive** | `0..=n` iterates [0, n] — includes end |
| **R3: Infinite** | `0..` iterates indefinitely — requires `break`, `return`, or `.take()` |
| **R4: Empty range** | `start >= end` produces zero iterations, not an error |
| **R5: End fits type** | Range end value must fit in iterator type — compile error otherwise |

| Syntax | Type | Behavior |
|--------|------|----------|
| `0..n` | `Range<Int>` | Half-open [0, n) |
| `0..=n` | `RangeInclusive<Int>` | Closed [0, n] |
| `(0..n).step(s)` | `StepRange<Int>` | Stepped half-open |
| `(0..=n).step(s)` | `StepRangeInclusive<Int>` | Stepped closed |
| `0..` | `RangeFrom<Int>` | Infinite |
| `..n` | `RangeTo<Int>` | Cannot iterate (no start) |
| `..` | `RangeFull` | Cannot iterate (unbounded) |

```rask
for i in 0..10 {
    process(i)
}
```

## Reverse Ranges

| Rule | Description |
|------|-------------|
| **RV1: Explicit rev** | Reverse iteration requires `.rev()` adapter |
| **RV2: Backwards empty** | `10..0` is empty (not reverse) — use `(0..10).rev()` |

| Range | Values |
|-------|--------|
| `0..10` | 0, 1, 2, ..., 9 |
| `10..0` | (empty) |
| `(0..10).rev()` | 9, 8, 7, ..., 0 |

## Step Ranges

| Rule | Description |
|------|-------------|
| **SP1: Positive step** | `start < end` required, iterates upward |
| **SP2: Negative step** | `start > end` required, iterates downward |
| **SP3: Zero step** | Compile error |
| **SP4: Uneven step** | Last value before exceeding bound |

<!-- test: parse -->
```rask
for i in (0..100).step(2) { }      // 0, 2, 4, ..., 98
for i in (10..0).step(-1) { }      // 10, 9, 8, ..., 1
for x in (0.0..1.0).step(0.1) { }  // Floats: 0.0, 0.1, ..., 0.9
```

| Expression | Values |
|------------|--------|
| `(0..10).step(3)` | 0, 3, 6, 9 |
| `(0..=10).step(3)` | 0, 3, 6, 9 |
| `(10..0).step(-2)` | 10, 8, 6, 4, 2 |
| `(10..=0).step(-2)` | 10, 8, 6, 4, 2, 0 |
| `(0..10).step(-1)` | (empty — direction mismatch) |

## Overflow Behavior

| Rule | Description |
|------|-------------|
| **OV1: End overflow** | End value must fit in type — compile error if not |
| **OV2: Increment overflow (debug)** | Panic on overflow |
| **OV3: Increment overflow (release)** | Wraps silently |
| **OV4: Inclusive at max** | `RangeInclusive` tracks `exhausted` flag — `0u8..=255` terminates correctly |

## Type Inference

<!-- test: skip -->
```rask
let vec: Vec<u16> = Vec.new()
for i in 0..vec.len() { }  // i inferred as usize
for i in 0..10 { }          // i inferred as i32 (default)
for i in 0u8..10 { }        // i explicitly u8
```

## Error Messages

```
ERROR [ctrl.ranges/R5]: range end doesn't fit in type
   |
3  |  for i in 0u8..256 {
   |                ^^^ 256 doesn't fit in u8

FIX: for i in 0u16..256 {
```

```
ERROR [ctrl.ranges/SP3]: zero step
   |
5  |  (0..10).step(0)
   |               ^ step must be non-zero
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `start >= end` | R4 | Empty range, no iterations |
| `10..0` | R4 | Empty — use `(0..10).rev()` for reverse |
| `0u8..256` | OV1 | Compile error |
| `0u8..=255` | OV4 | Valid, terminates correctly |
| `0u8..` in release | OV3 | Wraps: 0, 1, ..., 255, 0, 1, ... |
| `(0..10).step(0)` | SP3 | Compile error |
| Float step precision | SP4 | `(0.0..1.0).step(0.3)` yields 0.0, 0.3, 0.6, 0.9 (not exact) |

---

## Appendix (non-normative)

### RangeInclusive Implementation

<!-- test: skip -->
```rask
struct RangeInclusive<T> {
    start: T
    end: T
    exhausted: bool
}

extend RangeInclusive<T> with Iterator<T> where T: Int {
    func next(self) -> Option<T> {
        if self.exhausted { return None }
        if self.start == self.end {
            self.exhausted = true
            return Some(self.end)
        }
        const val = self.start
        self.start += 1
        Some(val)
    }
}
```

### See Also

- `ctrl.loops` — loop syntax and borrowing
- `type.iterators` — iterator trait and adapters
