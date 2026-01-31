# Parallel Compute

Data parallelism primitives for CPU-bound work.

## Overview

Orthogonal to sync/async concurrency. Pure computation over collections.

| Property | Value |
|----------|-------|
| Purpose | CPU-bound parallel computation |
| Scaling | Bounded by CPU cores |
| Ownership | Consumes input, produces output |

## Primitives

### parallel_map

Transform each element in parallel:

```
items = vec![1, 2, 3, 4]

results = parallel_map(items) { |item|
    compute(item)  // item owned by this closure
}
// items consumed (moved into parallel units)
```

**Signature:** `fn<T, U>(Vec<T>, fn(T) -> U) -> Vec<U>`

**Semantics:**
- Consumes input vector
- Each closure owns one element
- Output order matches input order
- Uses thread pool (bounded by CPU cores)

### parallel_reduce

Fold with parallelism:

```
sum = parallel_reduce(numbers, 0) { |acc, n|
    acc + n
}
```

**Signature:** `fn<T, U>(Vec<T>, U, fn(U, T) -> U) -> U`

**Semantics:**
- Consumes input vector
- Combines in parallel (associativity required for determinism)
- Returns single result

### parallel_for

Side-effect only:

```
parallel_for(urls) { |url|
    fetch_and_cache(url)
}
```

**Signature:** `fn<T>(Vec<T>, fn(T))`

**Semantics:**
- Consumes input vector
- No return value
- For side-effects (caching, logging, I/O)

## Ownership Rules

All primitives consume their input:

```
images = load_images()
thumbnails = parallel_map(images) { |img| resize(img) }
// images is INVALID here (moved)
```

**To retain access:** Clone before parallel operation:

```
thumbnails = parallel_map(images.clone()) { |img| resize(img) }
// images still valid (cloned)
```

This makes the copy cost visible (transparent costs principle).

## Thread Pool

Parallel primitives use a shared thread pool:

| Property | Value |
|----------|-------|
| Size | `num_cpus()` by default |
| Initialization | Lazy (first parallel call) |
| Scope | Process-global |

### Configuration (TBD)

Thread pool configuration API is unspecified. Options:

```
// Option A: Environment variable
RASK_PARALLEL_THREADS=4

// Option B: Explicit initialization
parallel_init(threads: 4)

// Option C: Per-call override
parallel_map(items, threads: 4) { |item| ... }
```

---

## Remaining Issues

### High Priority

1. **Error handling in parallel operations**
   - What if `f` returns `Result<U, E>`?
   - Short-circuit on first error? Collect all errors?
   - Current signature assumes infallible `f`

### Medium Priority

2. **Thread pool configuration**
   - No API specified
   - Need initialization, sizing, shutdown

3. **Work distribution**
   - How are items distributed across threads?
   - Chunk size? Work-stealing?

4. **Nested parallelism**
   - What happens if `f` calls parallel_map?
   - Deadlock risk with fixed thread pool

### Low Priority

5. **Parallel iterators**
   - Should there be a lazy parallel iterator API?
   - `items.par_iter().map(...).filter(...).collect()`
