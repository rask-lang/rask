<!-- id: std.math -->
<!-- status: decided -->
<!-- summary: Mathematical functions (trig, log, multi-arg) and constants in the math module -->

# Math

Dedicated `math` module for functions that don't attach to a single value (trig, logarithms, multi-argument). Common single-value operations (`abs`, `sqrt`, `pow`, `floor`, `ceil`, `round`, `min`, `max`) are methods on `f64`/`i64`.

## Constants

| Rule | Description |
|------|-------------|
| **C1: Float constants** | `math.PI`, `math.E`, `math.TAU`, `math.INF`, `math.NEG_INF`, `math.NAN` are `f64` |

| Constant | Value |
|----------|-------|
| `math.PI` | 3.14159265358979... |
| `math.E` | 2.71828182845904... |
| `math.TAU` | 6.28318530717958... (2 * PI) |
| `math.INF` | +infinity |
| `math.NEG_INF` | -infinity |
| `math.NAN` | Not a number |

## Trigonometric Functions

| Rule | Description |
|------|-------------|
| **T1: Trig** | `math.sin`, `math.cos`, `math.tan` take `f64` radians, return `f64` |
| **T2: Inverse trig** | `math.asin`, `math.acos`, `math.atan` return radians |
| **T3: atan2** | `math.atan2(y, x)` — two-argument arc tangent |

## Exponential and Logarithmic Functions

| Rule | Description |
|------|-------------|
| **L1: exp** | `math.exp(x)` computes e^x |
| **L2: ln** | `math.ln(x)` is natural log (not `log` — avoids base ambiguity) |
| **L3: Explicit base** | `math.log2(x)` and `math.log10(x)` for base-2 and base-10 |

## Multi-Argument Functions

| Rule | Description |
|------|-------------|
| **M1: hypot** | `math.hypot(x, y)` computes sqrt(x^2 + y^2) without overflow |
| **M2: clamp** | `math.clamp(x, lo, hi)` clamps to [lo, hi]; generic over ordered numeric types |

## Conversion and Classification

| Rule | Description |
|------|-------------|
| **V1: Angle conversion** | `math.to_radians(degrees)` and `math.to_degrees(radians)` |
| **V2: Classification** | `math.is_nan(x)`, `math.is_inf(x)`, `math.is_finite(x)` return `bool` |

<!-- test: skip -->
```rask
import math

const angle = math.PI / 4.0
const result = math.sin(angle)
const dist = math.hypot(3.0, 4.0)   // 5.0
const clamped = math.clamp(150.0, 0.0, 100.0)  // 100.0
```

## Value Methods (not in math module)

| Rule | Description |
|------|-------------|
| **N1: f64 methods** | `abs`, `sqrt`, `pow`, `floor`, `ceil`, `round`, `min`, `max` are methods on `f64` |
| **N2: i64 methods** | `abs`, `min`, `max` are methods on `i64` |

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

## Error Messages

```
ERROR [std.math/N1]: no method `sin` on type f64
   |
5  |  const r = x.sin()
   |              ^^^ f64 does not have a sin method

WHY: Trig functions are in the math module, not on f64.

FIX: Use math.sin(x) instead.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `math.ln(0.0)` | L2 | Returns `NEG_INF` |
| `math.ln(-1.0)` | L2 | Returns `NAN` |
| `math.sqrt(-1.0)` | N1 | Returns `NAN` |
| `math.sin(NAN)` | T1 | Returns `NAN` (NaN propagates) |
| `math.clamp(NAN, 0.0, 1.0)` | M2 | Returns `NAN` |
| `math.INF + math.NEG_INF` | C1 | Returns `NAN` |
| `math.is_nan(math.NAN)` | V2 | Returns `true` |

---

## Appendix (non-normative)

### Rationale

**L2 (ln not log):** `log` is ambiguous — base-e, base-2, or base-10? `ln` is unambiguous. `log2` and `log10` are explicit.

**N1 (methods vs module):** Single-value operations (`x.abs()`, `x.sqrt()`) read naturally as methods. Multi-argument or transcendental functions (`math.atan2(y, x)`, `math.sin(x)`) don't attach to one value, so they live in the module.

### Patterns & Guidance

**Distance and angle:**

<!-- test: parse -->
```rask
import math

func distance(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    return math.hypot(x2 - x1, y2 - y1)
}

func angle_between(x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    return math.atan2(y2 - y1, x2 - x1)
}
```

### Implementation

All functions map directly to platform `libm` or hardware FPU instructions. No allocation, no error returns -- pure numeric operations following IEEE 754 semantics.

### See Also

- `std.time` — Duration arithmetic
