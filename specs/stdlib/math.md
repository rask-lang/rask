# Math — Mathematical Functions and Constants

## The Question

Where do mathematical functions like `sin`, `cos`, `log` live? How do we handle constants like `PI`? Should math be methods on numbers or a module?

## Decision

Dedicated `math` module for functions that don't naturally attach to a single value (trig, logarithms, multi-argument functions) plus constants. Common single-value operations (`abs`, `sqrt`, `pow`, `floor`, `ceil`, `round`, `min`, `max`) remain as methods on `f64`/`i64`.

## Rationale

**Why a module instead of all-methods?**
- `sin(x)` reads better as `math.sin(x)` than `x.sin()` — it's a function you apply, not a property of the value
- Multi-argument functions like `atan2(y, x)` and `hypot(x, y)` don't belong on either argument
- Constants need a home: `math.PI` is clear, `f64.PI` is awkward

**Why keep some operations as methods?**
- `x.abs()`, `x.sqrt()`, `x.floor()` are natural — they transform the value
- `x.min(y)`, `x.max(y)` read well as methods
- These are already implemented as f64/i64 methods in Rask

**Why not `std::f64::consts::PI` like Rust?**
- Three levels of nesting for a math constant is absurd
- `math.PI` is what every other language does (Go, Python, Java, C)
- One import, one namespace, done

## Specification

### Constants

All constants are `f64`:

| Constant | Value | Description |
|----------|-------|-------------|
| `math.PI` | 3.14159265358979... | Circle ratio |
| `math.E` | 2.71828182845904... | Euler's number |
| `math.TAU` | 6.28318530717958... | 2 * PI |
| `math.INF` | +∞ | Positive infinity |
| `math.NEG_INF` | -∞ | Negative infinity |
| `math.NAN` | NaN | Not a number |

### Trigonometric Functions

All operate on `f64`, angles in radians:

```rask
math.sin(x: f64) -> f64
math.cos(x: f64) -> f64
math.tan(x: f64) -> f64
math.asin(x: f64) -> f64       // arc sine, result in [-PI/2, PI/2]
math.acos(x: f64) -> f64       // arc cosine, result in [0, PI]
math.atan(x: f64) -> f64       // arc tangent, result in [-PI/2, PI/2]
math.atan2(y: f64, x: f64) -> f64  // two-argument arc tangent
```

### Exponential and Logarithmic Functions

```rask
math.exp(x: f64) -> f64        // e^x
math.ln(x: f64) -> f64         // natural log (not "log" — avoids base ambiguity)
math.log2(x: f64) -> f64       // base-2 log
math.log10(x: f64) -> f64      // base-10 log
```

**Why `ln` not `log`?** `log` is ambiguous — is it base-e, base-2, or base-10? `ln` is unambiguous (natural log). `log2` and `log10` are explicit.

### Multi-Argument Functions

```rask
math.hypot(x: f64, y: f64) -> f64          // sqrt(x² + y²) without overflow
math.clamp(x: f64, lo: f64, hi: f64) -> f64  // clamp x to [lo, hi]
```

`clamp` also works for integers — it's generic over ordered numeric types.

### Conversion Functions

```rask
math.to_radians(degrees: f64) -> f64
math.to_degrees(radians: f64) -> f64
```

### Classification Functions

```rask
math.is_nan(x: f64) -> bool
math.is_inf(x: f64) -> bool
math.is_finite(x: f64) -> bool
```

### Access Pattern

```rask
import math

const angle = math.PI / 4.0
const result = math.sin(angle)
const dist = math.hypot(3.0, 4.0)   // 5.0
```

### Relationship to Methods

These operations are methods on `f64` / `i64` and are NOT in the `math` module:

| Method | Available on | Example |
|--------|-------------|---------|
| `abs()` | f64, i64 | `x.abs()` |
| `sqrt()` | f64 | `x.sqrt()` |
| `pow(n)` | f64 | `x.pow(2.0)` |
| `floor()` | f64 | `x.floor()` |
| `ceil()` | f64 | `x.ceil()` |
| `round()` | f64 | `x.round()` |
| `min(y)` | f64, i64 | `x.min(y)` |
| `max(y)` | f64, i64 | `x.max(y)` |

## Examples

### Distance Calculation

```rask
import math

func distance(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    return math.hypot(x2 - x1, y2 - y1)
}
```

### Angle Between Points

```rask
import math

func angle_between(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    return math.atan2(y2 - y1, x2 - x1)
}

func main() {
    const angle = angle_between(0.0, 0.0, 1.0, 1.0)
    const degrees = math.to_degrees(angle)
    println("Angle: {degrees} degrees")  // ~45 degrees
}
```

### Clamping Values

```rask
import math

func apply_damage(health: f64, damage: f64) -> f64 {
    return math.clamp(health - damage, 0.0, 100.0)
}
```

### Sensor Processing

```rask
import math

func moving_average(samples: Vec<f64>, window: i64) -> Vec<f64> {
    const result = Vec.new()
    for i in 0..samples.len() {
        let sum = 0.0
        let count = 0
        for j in (i - window + 1)..=i {
            if j >= 0 && j < samples.len() {
                sum += samples[j]
                count += 1
            }
        }
        try result.push(sum / count.to_float())
    }
    return result
}
```

## Edge Cases

- `math.ln(0.0)` returns `NEG_INF`
- `math.ln(-1.0)` returns `NAN`
- `math.sqrt(-1.0)` returns `NAN`
- NaN propagates: `math.sin(NAN)` returns `NAN`
- `math.clamp(NAN, 0.0, 1.0)` returns `NAN`
- `math.is_nan(math.NAN)` returns `true`
- `math.INF + math.NEG_INF` returns `NAN`

## Implementation Notes

All functions map directly to platform `libm` or hardware FPU instructions. No allocation, no error returns — these are pure numeric operations following IEEE 754 semantics.

## References

- specs/stdlib/time.md — Uses Duration arithmetic
- CORE_DESIGN.md — Transparent cost (all operations are O(1), no allocation)

## Status

**Specified** — ready for implementation in interpreter.
