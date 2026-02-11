<!-- id: type.simd -->
<!-- status: decided -->
<!-- summary: Explicit SIMD vector types with parametric width, auto-broadcast, method-style masking -->
<!-- depends: types/primitives.md, types/operators.md, types/generics.md -->

# SIMD Types

Explicit vector types with parametric width. `Vec[T, N]` where N is fixed or `native`. Operators auto-broadcast scalars. Masking via `.where()`. Reductions as methods.

## Vector Type

| Rule | Description |
|------|-------------|
| **T1: Type form** | `Vec[T, N]` — T is primitive numeric, N is power of 2 or `native` |
| **T2: Element types** | `i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`, `f32`, `f64` |
| **T3: Lane count** | Must be power of 2: 2, 4, 8, 16, 32, ... or `native` |
| **T4: Native width** | `native` resolved at compile time to target's optimal width |

| Target | f32 native | f64 native |
|--------|------------|------------|
| SSE | 4 | 2 |
| AVX/AVX2 | 8 | 4 |
| AVX-512 | 16 | 8 |
| NEON | 4 | 2 |

**Shorthand aliases:** `f32x4 = Vec[f32, 4]`, `i32x8 = Vec[i32, 8]`, `f32xN = Vec[f32, native]`, `boolx4 = Vec[bool, 4]`. Pattern: `{type}x{lanes}` where `N` means native.

## Construction

| Rule | Description |
|------|-------------|
| **C1: Literal** | `[a, b, c, d]` — lane count must match type |
| **C2: Repeat** | `[x; N]` — repeat value x for N lanes |
| **C3: Splat** | `splat[T, N](x)` — explicit broadcast |
| **C4: Default** | `default` — zero-initialized |
| **C5: Load** | `Vec[T, N].load(slice)` — load from slice |

<!-- test: skip -->
```rask
v1: f32x4 = [1.0, 2.0, 3.0, 4.0]   // Literal
v2: f32x4 = [0.0; 4]                // Repeat
v3: f32x4 = default                 // Zero
v4 = splat[f32, 4](x)               // Broadcast scalar
```

## Arithmetic and Broadcasting

| Rule | Description |
|------|-------------|
| **A1: Element-wise** | All arithmetic operators (`+`, `-`, `*`, `/`, `%`, unary `-`) are element-wise |
| **A2: Same type required** | Both operands must have same T and N, or one is scalar T |
| **A3: Scalar broadcast** | Scalar operand implicitly broadcast to all lanes |

<!-- test: skip -->
```rask
v: f32x8 = [1, 2, 3, 4, 5, 6, 7, 8]
v * 2.0           // [2, 4, 6, 8, 10, 12, 14, 16]
v * 2.0 + 0.5     // Chained operations
```

## Comparisons

| Rule | Description |
|------|-------------|
| **CMP1: Mask result** | Comparisons (`==`, `!=`, `<`, `<=`, `>`, `>=`) produce `Vec[bool, N]` |
| **CMP2: Broadcast applies** | Scalar comparison operands broadcast |

<!-- test: skip -->
```rask
a: f32x4 = [1, 2, 3, 4]
b: f32x4 = [2, 2, 2, 2]
mask: boolx4 = a > b   // [false, false, true, true]
```

## Bitwise Operations (Integer Vectors)

| Rule | Description |
|------|-------------|
| **BW1: Element-wise** | `&`, `\|`, `^`, `~` are element-wise |
| **BW2: Shift** | `<<`, `>>` are element-wise; shift amount is scalar |

## Masking and Selection

| Rule | Description |
|------|-------------|
| **K1: Conditional select** | `a.where(mask, else: b)` — per-lane selection based on mask |
| **K2: Mask operations** | `&`, `\|`, `!` on masks; `.any()`, `.all()`, `.none()`, `.count()` queries |

<!-- test: skip -->
```rask
a: f32x4 = [1, 2, 3, 4]
b: f32x4 = [10, 20, 30, 40]
mask: boolx4 = [true, false, true, false]
result = a.where(mask, else: b)  // [1, 20, 3, 40]
```

## Reductions

| Rule | Description |
|------|-------------|
| **R1: Collapse to scalar** | `.sum()`, `.product()`, `.min()`, `.max()`, `.reduce(op)` |
| **R2: Floating-point order** | Reductions may reorder; use `.reduce_ordered()` for strict left-to-right |

<!-- test: skip -->
```rask
v: f32x8 = [1, 2, 3, 4, 5, 6, 7, 8]
v.sum()      // 36.0
v.min()      // 1.0
v.reduce(+)  // 36.0
```

## Lane Access

| Rule | Description |
|------|-------------|
| **LA1: Indexing** | `v[i]` reads/writes lane i; out-of-bounds is compile error (comptime) or panic (runtime) |
| **LA2: Slicing** | `v[0..4]` extracts contiguous lanes; indices must be comptime-known |

## Shuffles

| Rule | Description |
|------|-------------|
| **SH1: Single-vector** | `v.shuffle([indices])` — indices must be comptime-known |
| **SH2: Two-vector** | `shuffle(a, b, [indices])` — indices 0..N select from a, N..2N from b |
| **SH3: Convenience methods** | `.reverse()`, `.rotate_left(n)`, `.rotate_right(n)`, `.interleave_low(other)`, `.interleave_high(other)` |

<!-- test: skip -->
```rask
v: f32x4 = [1, 2, 3, 4]
v.shuffle([3, 2, 1, 0])   // Reverse: [4, 3, 2, 1]
v.shuffle([0, 0, 0, 0])   // Broadcast: [1, 1, 1, 1]
```

## Memory Operations

| Rule | Description |
|------|-------------|
| **MEM1: Load/store** | `.load(slice)` and `.store(slice)` — bounds-checked |
| **MEM2: Aligned variants** | `.load_aligned()` / `.store_aligned()` — requires `N * sizeof(T)` alignment |
| **MEM3: Masked load/store** | `.load_masked(slice, mask, default:)` / `.store_masked(slice, mask)` |
| **MEM4: Gather/scatter** | `data.gather(indices)` / `data.scatter(indices, values)` — non-contiguous, ~10-20x slower |

## Type Conversions

| Rule | Description |
|------|-------------|
| **CV1: Widen** | `.widen_low()` / `.widen_high()` — to larger type, fewer lanes |
| **CV2: Narrow** | `.narrow_saturate()` / `.narrow_truncate()` — to smaller type, more lanes |
| **CV3: Bitcast** | `.bitcast()` — reinterpret bits, total size must match |
| **CV4: Numeric** | `.to[U]()` — convert element type (e.g., i32 to f32) |

## C Interop

| Rask | C (x86) | C (ARM) |
|------|---------|---------|
| `f32x4` | `__m128` | `float32x4_t` |
| `f32x8` | `__m256` | N/A |
| `f64x2` | `__m128d` | `float64x2_t` |
| `i32x4` | `__m128i` | `int32x4_t` |
| `i32x8` | `__m256i` | N/A |

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Lane count mismatch in operation | A2 | Compile error |
| Element type mismatch | A2 | Compile error |
| Lane count not power of 2 | T3 | Compile error |
| Out-of-bounds index (comptime) | LA1 | Compile error |
| Out-of-bounds index (runtime) | LA1 | Panic |
| Misaligned `load_aligned` | MEM2 | Undefined behavior (or panic in debug) |
| Shuffle with runtime indices | SH1 | Compile error |
| `native` on unsupported target | T4 | Falls back to smallest (2 or 4) |

---

## Appendix (non-normative)

### Rationale

**T1 (explicit vectors):** Transparent cost — users know when SIMD happens. Auto-vectorization is unreliable. Parametric width enables both fixed-width (specific algorithms) and portable (`native`).

**A3 (scalar broadcast):** Splat costs ~1 cycle, within threshold for implicit small costs (like bounds checks). Ergonomic without hiding meaningful cost.

**K1 (method-style masking):** `.where(mask, else: b)` reads left-to-right, matching Rask's expression-oriented style. Avoids a separate `select` function.

### Patterns & Guidance

**Vectorized loop with native width:**

<!-- test: skip -->
```rask
func process(data: []f32, scale: f32) {
    const N = Vec[f32, native].lanes
    const main_end = (data.len() / N) * N

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

**Vectorized array sum:**

<!-- test: skip -->
```rask
func sum_array(data: []f32) -> f32 {
    const N = Vec[f32, native].lanes
    let acc: Vec[f32, native] = [0.0; N]
    const main_end = (data.len() / N) * N

    for i in (0..main_end).step_by(N) {
        acc = acc + Vec[f32, native].load(data[i..])
    }

    let total = acc.sum()
    for i in main_end..data.len() {
        total += data[i]
    }
    return total
}
```

**Generic width and type:**

<!-- test: skip -->
```rask
func dot_product[T: Numeric, N: usize](a: Vec[T, N], b: Vec[T, N]) -> T {
    return (a * b).sum()
}
```

**Deferred features:** Letter swizzles (`v.xyzw`), runtime width dispatch, FMA intrinsics, platform-specific intrinsics — reserved for future consideration.

### See Also

- `type.primitives` — Scalar numeric types
- `type.operators` — Operator traits
- `type.generics` — Parameterized types
- `struct.c-interop` — C FFI
