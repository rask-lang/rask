# Range Iteration

See also: [README.md](README.md)

## Range Iteration Edge Cases

**Range Types:**

Rask supports several range syntaxes for iteration:

| Syntax | Type | Start | End | Behavior |
|--------|------|-------|-----|----------|
| `0..n` | `Range<Int>` | 0 | n-1 | Half-open range [0, n) |
| `0..=n` | `RangeInclusive<Int>` | 0 | n | Closed range [0, n] |
| `0..n step s` | `StepRange<Int>` | 0 | <n | Stepped half-open range |
| `0..=n step s` | `StepRangeInclusive<Int>` | 0 | ≤n | Stepped closed range |
| `0..` | `RangeFrom<Int>` | 0 | ∞ | Infinite range, no upper bound |
| `..n` | `RangeTo<Int>` | N/A | n-1 | Cannot iterate (no start) |
| `..` | `RangeFull` | N/A | N/A | Cannot iterate (unbounded) |

**Infinite Range Iteration:**

```rask
// Infinite range - never terminates without break
for i in 0.. {
    if i > 1000 { break; }
    process(i);
}
```

**Semantics:**
- Iterator yields values indefinitely (0, 1, 2, ...)
- Loop never terminates unless `break`, `return`, or `try` exits
- Valid use case: event loops, generators, polling

**Desugaring:**

```rask
{
    let _pos = 0;
    loop {
        const i = _pos;
        if i > 1000 { break; }
        process(i);
        _pos += 1;
    }
}
```

**Reverse Ranges:**

Reverse iteration requires explicit `.rev()` adapter:

```rask
// This does NOT work (empty range):
for i in 10..0 {  // Never executes: 10 >= 0, so range is empty
    unreachable();
}

// Correct approach:
for i in (0..10).rev() {  // Iterates 9, 8, 7, ..., 0
    process(i);
}
```

**Rule:** If `start >= end`, range is empty (loop body never executes). This is NOT an error.

| Range | Iterations | Values |
|-------|-----------|--------|
| `0..10` | 10 | 0, 1, 2, ..., 9 |
| `10..10` | 0 | (empty) |
| `10..0` | 0 | (empty) |
| `(0..10).rev()` | 10 | 9, 8, 7, ..., 0 |
| `(-5..5)` | 10 | -5, -4, ..., 4 |

**Overflow Behavior:**

When range iteration would overflow the integer type:

**Case 1: End value overflow**

```rask
// u8 max is 255
for i in 0..256 {  // COMPILE ERROR: 256 doesn't fit in u8
    print(i);
}

// Workaround: use larger type
for i in 0u16..256 {  // OK: u16 can hold 256
    print(i);
}
```

**Rule:** Range end value MUST fit in the iterator type. Compiler enforces at type-checking.

**Case 2: Increment overflow**

```rask
for i in 0u8.. {
    print(i);  // Prints 0..255, then what?
}
```

**Behavior:** When incrementing from max value:
- Unsigned types: Wraps to 0 (infinite loop: 0, 1, ..., 255, 0, 1, ...)
- Signed types: Wraps to min value (e.g., i8: 127 → -128)
- In **debug mode**: PANIC on overflow
- In **release mode**: Wrap (silent)

**Recommendation:** Use `.take()` adapter to bound infinite ranges:

```rask
for i in 0u8...take(256) {
    print(i);  // Prints 0..255, then stops (256 iterations)
}
```

**Case 3: Inclusive range at max value**

```rask
for i in 0u8..=255 {
    print(i);  // Prints 0..255
}
// After yielding 255, how does iterator know it's done?
```

**Implementation:** `RangeInclusive` tracks whether the final value has been yielded:

```rask
struct RangeInclusive<T> {
    start: T,
    end: T,
    exhausted: bool,  // Set to true after yielding end value
}

extend RangeInclusive<T> with Iterator<T> where T: Int {
    func next(self) -> Option<T> {
        if self.exhausted { return None; }
        if self.start == self.end {
            self.exhausted = true;
            return Some(self.end);
        }
        const val = self.start;
        self.start += 1;  // This wraps after yielding max value, but exhausted=true prevents re-use
        Some(val)
    }
}
```

**Rule:** `RangeInclusive<u8>` for `0..=255` is valid and terminates correctly.

**Step Ranges:**

The `step` keyword allows iteration with custom increments:

```rask
for i in 0..100 step 2 {     // 0, 2, 4, ..., 98
    process(i)
}

for i in 0..=100 step 5 {    // 0, 5, 10, ..., 100
    process(i)
}

for i in 10..0 step -1 {     // 10, 9, 8, ..., 1
    countdown(i)
}

for x in 0.0..1.0 step 0.1 { // Floats: 0.0, 0.1, 0.2, ..., 0.9
    interpolate(x)
}
```

**Syntax:** `start..end step increment` or `start..=end step increment`

**Rules:**

| Rule | Description |
|------|-------------|
| Positive step | `start < end` required, iterates upward |
| Negative step | `start > end` required, iterates downward |
| Zero step | Compile error |
| Step doesn't divide evenly | Last value before exceeding bound |

**Examples:**

| Expression | Values |
|------------|--------|
| `0..10 step 3` | 0, 3, 6, 9 |
| `0..=10 step 3` | 0, 3, 6, 9 (10 not included, 9+3 > 10) |
| `10..0 step -2` | 10, 8, 6, 4, 2 |
| `10..=0 step -2` | 10, 8, 6, 4, 2, 0 |
| `0..10 step -1` | Empty (direction mismatch) |

**Type constraints:**
- Step type must match range type
- For floats, beware of precision: `0.0..1.0 step 0.3` yields 0.0, 0.3, 0.6, 0.9 (not exactly 0.9)

**Overflow:** Same rules as regular increment—debug panics, release wraps. Use `.take()` to bound.

**Type Inference:**

Range type is inferred from context:

```rask
let vec: Vec<u16> = ...;
for i in 0..vec.len() {  // i inferred as usize (vec.len() returns usize)
    process(vec[i]);
}

for i in 0..10 {  // i inferred as i32 (default integer type)
    print(i);
}

for i in 0u8..10 {  // i explicitly u8
    print(i);
}
```

**Summary Table:**

| Range Expression | Valid | Behavior |
|-----------------|-------|----------|
| `0..10` | ✅ Yes | Iterates 0..9 |
| `10..0` | ✅ Yes | Empty (no iterations) |
| `0..` | ✅ Yes | Infinite (never terminates) |
| `(0..10).rev()` | ✅ Yes | Iterates 9..0 |
| `0..=255` (u8) | ✅ Yes | Iterates 0..255, terminates correctly |
| `0..256` (u8) | ❌ No | Compile error: 256 doesn't fit in u8 |
| `0u8...take(300)` | ✅ Yes | Wraps, takes 300 values (with wrapping) |
| `0..100 step 2` | ✅ Yes | Iterates 0, 2, 4, ..., 98 |
| `10..0 step -1` | ✅ Yes | Iterates 10, 9, 8, ..., 1 |
| `0..10 step 0` | ❌ No | Compile error: zero step |

---

## See Also
- [Loop Syntax](loop-syntax.md) - Basic loop syntax and borrowing semantics
- [Iterator Protocol](iterator-protocol.md) - Iterator trait and adapter details
- [Edge Cases](edge-cases.md) - Other edge cases
