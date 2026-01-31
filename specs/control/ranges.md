# Range Iteration

See also: [README.md](README.md)

## Range Iteration Edge Cases

**Range Types:**

Rask supports several range syntaxes for iteration:

| Syntax | Type | Start | End | Behavior |
|--------|------|-------|-----|----------|
| `0..n` | `Range<Int>` | 0 | n-1 | Half-open range [0, n) |
| `0..=n` | `RangeInclusive<Int>` | 0 | n | Closed range [0, n] |
| `0..` | `RangeFrom<Int>` | 0 | ∞ | Infinite range, no upper bound |
| `..n` | `RangeTo<Int>` | N/A | n-1 | Cannot iterate (no start) |
| `..` | `RangeFull` | N/A | N/A | Cannot iterate (unbounded) |

**Infinite Range Iteration:**

```
// Infinite range - never terminates without break
for i in 0.. {
    if i > 1000 { break; }
    process(i);
}
```

**Semantics:**
- Iterator yields values indefinitely (0, 1, 2, ...)
- Loop never terminates unless `break`, `return`, or `?` exits
- Valid use case: event loops, generators, polling

**Desugaring:**

```
{
    let mut _pos = 0;
    loop {
        let i = _pos;
        if i > 1000 { break; }
        process(i);
        _pos += 1;
    }
}
```

**Reverse Ranges:**

Reverse iteration requires explicit `.rev()` adapter:

```
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

```
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

```
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

```
for i in 0u8...take(256) {
    print(i);  // Prints 0..255, then stops (256 iterations)
}
```

**Case 3: Inclusive range at max value**

```
for i in 0u8..=255 {
    print(i);  // Prints 0..255
}
// After yielding 255, how does iterator know it's done?
```

**Implementation:** `RangeInclusive` tracks whether the final value has been yielded:

```
struct RangeInclusive<T> {
    start: T,
    end: T,
    exhausted: bool,  // Set to true after yielding end value
}

impl Iterator<T> for RangeInclusive<T> where T: Int {
    fn next(&mut self) -> Option<T> {
        if self.exhausted { return None; }
        if self.start == self.end {
            self.exhausted = true;
            return Some(self.end);
        }
        let val = self.start;
        self.start += 1;  // This wraps after yielding max value, but exhausted=true prevents re-use
        Some(val)
    }
}
```

**Rule:** `RangeInclusive<u8>` for `0..=255` is valid and terminates correctly.

**Step Ranges (Future Extension):**

Rask reserves syntax for stepped ranges but does NOT specify them in this version:

```
// Reserved for future:
for i in (0..100).step(5) {  // Could iterate 0, 5, 10, ..., 95
    process(i);
}
```

**Overflow in steps:** Implementation-defined in future spec. Likely: same rules as increment overflow.

**Type Inference:**

Range type is inferred from context:

```
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

---

## See Also
- [Loop Syntax](loop-syntax.md) - Basic loop syntax and borrowing semantics
- [Iterator Protocol](iterator-protocol.md) - Iterator trait and adapter details
- [Edge Cases](edge-cases.md) - Other edge cases
