# SIMD Types

## The Question

How does Rask express SIMD operations?

## Decision

Explicit vector types with parametric width. Vectors distinct from scalars. Width fixed (`Vec[f32, 8]`) or target-native (`Vec[f32, native]`). Operators auto-broadcast scalars. Masking via `.where()`. Reductions as methods. Lane access via indexing.

## Rationale

**Explicit vectors** provide transparent cost—users know when SIMD happens. **Parametric width** enables both fixed-width (for specific algorithms) and portable (using `native`). **Auto-broadcast** is ergonomic without hiding cost (splat ~1 cycle). **Method-style masking** reads left-to-right, matching expression-oriented style.

Avoids:
- Hidden vectorization (auto-vectorization unreliable)
- Implicit model complexity (SPMD mental model differs)
- Graphics-specific features (letter swizzles) until demand emerges

---

## Specification

### Vector Type

```rask
Vec[T, N]
```

**Parameters:**
- `T` — Element type. Must be primitive numeric: `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`, `f32`, `f64`
- `N` — Lane count. Must be power of 2: 2, 4, 8, 16, 32, ... or special value `native`

**The `native` width:** Resolved at compile time to target's optimal width:

| Target | f32 native | f64 native |
|--------|------------|------------|
| SSE | 4 | 2 |
| AVX/AVX2 | 8 | 4 |
| AVX-512 | 16 | 8 |
| NEON | 4 | 2 |

### Shorthand Aliases

```rask
f32x4   = Vec[f32, 4]
f32x8   = Vec[f32, 8]
i32x4   = Vec[i32, 4]
i32x8   = Vec[i32, 8]
f32xN   = Vec[f32, native]
i32xN   = Vec[i32, native]
boolx4  = Vec[bool, 4]
boolx8  = Vec[bool, 8]
```

Pattern: `{type}x{lanes}` where `N` means native width.

### Construction

| Syntax | Meaning |
|--------|---------|
| `[a, b, c, d]` | Literal (lane count must match type) |
| `[x; N]` | Repeat value `x` for `N` lanes |
| `splat[T, N](x)` | Explicit broadcast (when clarity needed) |
| `default` | Zero-initialized |
| `Vec[T, N].load(slice)` | Load from slice |

```rask
v1: f32x4 = [1.0, 2.0, 3.0, 4.0]   // Literal
v2: f32x4 = [0.0; 4]                // Repeat: [0, 0, 0, 0]
v3: f32x4 = default                 // Zero: [0, 0, 0, 0]
v4 = splat[f32, 4](x)               // Broadcast scalar x
```

---

### Arithmetic Operations

All arithmetic operators are element-wise:

| Operator | Meaning |
|----------|---------|
| `a + b` | Element-wise add |
| `a - b` | Element-wise subtract |
| `a * b` | Element-wise multiply |
| `a / b` | Element-wise divide |
| `a % b` | Element-wise remainder |
| `-a` | Element-wise negate |

**Operands:** Both `Vec[T, N]` with same `T` and `N`, OR one scalar `T` (see Broadcasting).

### Scalar Broadcasting

When one operand is a scalar and the other is `Vec[T, N]`, the scalar is implicitly broadcast to all lanes:

```rask
v: f32x8 = [1, 2, 3, 4, 5, 6, 7, 8]
s: f32 = 2.0

v * s         // [2, 4, 6, 8, 10, 12, 14, 16]
v + 1.0       // Literal also broadcasts
v * 2.0 + 0.5 // Chained operations
```

**Transparency:** Splat costs ~1 cycle. Within threshold for implicit small costs (like bounds checks).

### Comparison Operations

Comparisons produce mask vectors (`Vec[bool, N]`):

| Operator | Meaning |
|----------|---------|
| `a == b` | Element-wise equality |
| `a != b` | Element-wise inequality |
| `a < b` | Element-wise less-than |
| `a <= b` | Element-wise less-or-equal |
| `a > b` | Element-wise greater-than |
| `a >= b` | Element-wise greater-or-equal |

```rask
a: f32x4 = [1, 2, 3, 4]
b: f32x4 = [2, 2, 2, 2]

mask: boolx4 = a > b   // [false, false, true, true]
```

### Bitwise Operations (Integer Vectors)

| Operator | Meaning |
|----------|---------|
| `a & b` | Element-wise AND |
| `a \| b` | Element-wise OR |
| `a ^ b` | Element-wise XOR |
| `~a` | Element-wise NOT |
| `a << n` | Element-wise left shift (n is scalar) |
| `a >> n` | Element-wise right shift (n is scalar) |

---

### Masking

#### Mask Type

Comparisons produce `Vec[bool, N]`. Mask types have shorthand aliases: `boolx4`, `boolx8`, etc.

#### Conditional Selection

```rask
result = a.where(mask, else: b)
```

Semantics: For each lane `i`, `result[i] = mask[i] ? a[i] : b[i]`

```rask
a: f32x4 = [1, 2, 3, 4]
b: f32x4 = [10, 20, 30, 40]
mask: boolx4 = [true, false, true, false]

result = a.where(mask, else: b)  // [1, 20, 3, 40]
```

#### Mask Operations

| Operation | Meaning |
|-----------|---------|
| `mask1 & mask2` | AND masks |
| `mask1 \| mask2` | OR masks |
| `!mask` | Invert mask |
| `mask.any()` | `true` if any lane is true |
| `mask.all()` | `true` if all lanes are true |
| `mask.none()` | `true` if no lanes are true |
| `mask.count()` | Number of true lanes |

---

### Reductions

Reductions collapse a vector to a scalar:

| Method | Meaning |
|--------|---------|
| `.sum()` | Add all lanes |
| `.product()` | Multiply all lanes |
| `.min()` | Minimum lane value |
| `.max()` | Maximum lane value |
| `.reduce(op)` | Reduce with given operator |

```rask
v: f32x8 = [1, 2, 3, 4, 5, 6, 7, 8]

v.sum()      // 36.0
v.product()  // 40320.0
v.min()      // 1.0
v.max()      // 8.0
v.reduce(+)  // 36.0 (same as sum)
```

**Floating-point:** Reductions may reorder, affecting precision. Use `.reduce_ordered()` for strict left-to-right (slower).

---

### Lane Access

#### Indexing

```rask
v: f32x8 = [1, 2, 3, 4, 5, 6, 7, 8]

x = v[0]        // Read lane 0: 1.0
x = v[7]        // Read lane 7: 8.0

v[3] = 10.0     // Write lane 3 (requires mutable v)
```

**Bounds:** Index must be in range `0..N`. Out-of-bounds is:
- Compile error if index is comptime-known
- Runtime panic otherwise

#### Slicing

Extract contiguous lanes (indices must be comptime-known):

```rask
v: f32x8 = [1, 2, 3, 4, 5, 6, 7, 8]

v[0..4]    // f32x4: [1, 2, 3, 4]
v[4..8]    // f32x4: [5, 6, 7, 8]
```

---

### Shuffles

#### Single-Vector Shuffle

```rask
v: f32x4 = [1, 2, 3, 4]

v.shuffle([3, 2, 1, 0])   // Reverse: [4, 3, 2, 1]
v.shuffle([0, 0, 0, 0])   // Broadcast: [1, 1, 1, 1]
v.shuffle([0, 2])         // Extract: f32x2 [1, 3]
```

**Indices must be comptime-known.** Hardware shuffles need immediate operands.

#### Two-Vector Shuffle

```rask
a: f32x4 = [1, 2, 3, 4]
b: f32x4 = [5, 6, 7, 8]

// Indices 0-3 select from a, 4-7 select from b
shuffle(a, b, [0, 4, 1, 5])   // Interleave: [1, 5, 2, 6]
shuffle(a, b, [0, 1, 4, 5])   // Concat halves: [1, 2, 5, 6]
```

#### Common Patterns

| Method | Meaning |
|--------|---------|
| `.reverse()` | Reverse all lanes |
| `.rotate_left(n)` | Rotate lanes left by n |
| `.rotate_right(n)` | Rotate lanes right by n |
| `.interleave_low(other)` | Interleave low halves |
| `.interleave_high(other)` | Interleave high halves |

---

### Memory Operations

#### Load/Store

```rask
data: [f32; 64]

// Load from slice (bounds-checked)
v = Vec[f32, 8].load(data[0..])
v = Vec[f32, 8].load(data[8..])   // Load starting at index 8

// Store to mutable slice
v.store(data[0..])
v.store(data[8..])

// Aligned variants (faster, requires alignment)
v = Vec[f32, 8].load_aligned(data[0..])   // Slice start must be 32-byte aligned for f32x8
v.store_aligned(data[0..])
```

**Alignment requirements:** `N * sizeof(T)` bytes. For `f32x8`: 32 bytes.

#### Masked Load/Store

```rask
mask: boolx8

// Load: unset lanes get default value
v = Vec[f32, 8].load_masked(data[0..], mask, default: 0.0)

// Store: only write lanes where mask is true
v.store_masked(data[0..], mask)
```

#### Gather/Scatter

Non-contiguous memory access:

```rask
data: [f32; 1000]
indices: i32x8 = [0, 10, 20, 30, 40, 50, 60, 70]

// Gather: v[i] = data[indices[i]]
v = data.gather(indices)

// Scatter: data[indices[i]] = values[i]
data.scatter(indices, values)
```

**Performance:** Gather/scatter significantly slower than contiguous (~10-20x). Use only when required.

---

### Type Conversions

#### Widening

Converts lanes to larger type, reducing lane count:

```rask
narrow: i16x8 = [1, 2, 3, 4, 5, 6, 7, 8]

wide_lo: i32x4 = narrow.widen_low()    // [1, 2, 3, 4] as i32
wide_hi: i32x4 = narrow.widen_high()   // [5, 6, 7, 8] as i32
```

#### Narrowing

Converts lanes to smaller type, increasing lane count:

```rask
wide: i32x4 = [1000, 2000, 3000, 4000]

// Saturating (clamp to target range)
narrow: i16x4 = wide.narrow_saturate()

// Truncating (take low bits only)
narrow: i16x4 = wide.narrow_truncate()
```

#### Bitcast

Reinterpret bits as different type (total size must match):

```rask
f: f32x4 = [1.0, 2.0, 3.0, 4.0]
i: i32x4 = f.bitcast()      // Same bits as i32

bytes: u8x16 = f.bitcast()  // 4 * 4 bytes = 16 bytes
```

#### Numeric Conversion

```rask
i: i32x4 = [1, 2, 3, 4]
f: f32x4 = i.to[f32]()      // [1.0, 2.0, 3.0, 4.0]

f: f32x4 = [1.5, 2.7, 3.2, 4.9]
i: i32x4 = f.to[i32]()      // Truncate toward zero: [1, 2, 3, 4]
```

---

### Generic Programming

#### Width-Generic Functions

```rask
func scale_vector[N: usize](v: Vec[f32, N], factor: f32) -> Vec[f32, N] {
    v * factor
}

// Works with any width
result4 = scale_vector(v4, 2.0)
result8 = scale_vector(v8, 2.0)
```

#### Type-Generic Functions

```rask
func dot_product[T: Numeric, N: usize](a: Vec[T, N], b: Vec[T, N]) -> T {
    (a * b).sum()
}
```

#### Native-Width Patterns

```rask
func process(data: []f32, scale: f32) {
    const N = Vec[f32, native].lanes    // Comptime constant
    const main_end = (data.len() / N) * N

    // Vectorized loop
    for i in (0..main_end).step_by(N) {
        const v = Vec[f32, native].load(data[i..])
        (v * scale).store(data[i..])
    }

    // Scalar remainder
    for i in main_end..data.len() {
        data[i] *= scale
    }
}
```

---

### C Interop

SIMD types are ABI-compatible with C intrinsic types:

| Rask | C (x86) | C (ARM) |
|------|---------|---------|
| `f32x4` | `__m128` | `float32x4_t` |
| `f32x8` | `__m256` | N/A |
| `f64x2` | `__m128d` | `float64x2_t` |
| `i32x4` | `__m128i` | `int32x4_t` |
| `i32x8` | `__m256i` | N/A |

---

## Edge Cases

| Case | Behavior |
|------|----------|
| Lane count mismatch in operation | Compile error |
| Element type mismatch | Compile error |
| Lane count not power of 2 | Compile error |
| Out-of-bounds index (comptime) | Compile error |
| Out-of-bounds index (runtime) | Panic |
| Misaligned `load_aligned` | Undefined behavior (or panic in debug) |
| Shuffle with runtime indices | Compile error |
| `native` width on unsupported target | Falls back to smallest (2 or 4) |

---

## Examples

### Vectorized Array Sum

```rask
func sum_array(data: []f32) -> f32 {
    const N = Vec[f32, native].lanes
    let acc: Vec[f32, native] = [0.0; N]
    const main_end = (data.len() / N) * N

    for i in (0..main_end).step_by(N) {
        acc = acc + Vec[f32, native].load(data[i..])
    }

    const total = acc.sum()
    for i in main_end..data.len() {
        total += data[i]
    }
    total
}
```

### Masked Conditional Update

```rask
func clamp_negatives(data: []f32) {
    const N = Vec[f32, native].lanes

    for i in (0..data.len()).step_by(N) {
        const v = Vec[f32, native].load(data[i..])
        const mask = v < 0.0
        const clamped = v.where(!mask, else: [0.0; N])
        clamped.store(data[i..])
    }
}
```

### Dot Product

```rask
func dot[N: usize](a: Vec[f32, N], b: Vec[f32, N]) -> f32 {
    (a * b).sum()
}
```

---

## Deferred Features

Reserved for future consideration:

1. **Letter swizzles** (`v.xyzw`, `v.rgba`) — if graphics workloads become common
2. **Runtime width dispatch** — if single-binary portability becomes important
3. **FMA intrinsics** — `fma(a, b, c)` for fused multiply-add
4. **Platform-specific intrinsics** — escape hatch for instructions not covered by portable API

---

## Integration

- **Primitives:** See [Primitives](primitives.md) for scalar numeric types
- **Operators:** See [Operators](operators.md) for operator traits
- **Generics:** See [Generics](generics.md) for parameterized types
- **C Interop:** See [C Interop](../structure/c-interop.md) for FFI
