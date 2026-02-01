# Solution: Atomics and Memory Ordering

## The Question
How does Rask provide low-level synchronization primitives for lock-free data structures, concurrent counters, and hardware interaction—while maintaining safety guarantees and transparency?

## Decision
Atomic types provide safe, data-race-free access to shared memory with explicit memory ordering. Operations are **safe** (not requiring `unsafe` blocks) because atomics internally handle synchronization. Memory orderings follow the C11/C++11 model, which is well-understood and maps efficiently to all major CPU architectures.

## Rationale
1. **Mechanical Correctness (MC ≥ 0.90):** Atomics eliminate data races by construction—the type system prevents non-atomic access to atomic values.

2. **Transparency (TC ≥ 0.90):** Memory orderings are explicit in every operation. No hidden synchronization costs. Programmers see exactly what guarantees they're paying for.

3. **Use Case Coverage (UCC ≥ 0.80):** Lock-free algorithms, reference counting, metrics, flags, and spin locks are essential for embedded, OS kernels, and high-performance servers.

4. **Safe by default:** Unlike raw pointers, atomic operations are inherently safe—they cannot cause data races when used correctly. The danger is in incorrect *logic*, not memory safety.

5. **Explicit escape hatch:** CORE_DESIGN states "No shared mutable memory between tasks"—atomics are the explicit, transparent mechanism when cross-task shared state is genuinely needed (metrics, flags, lock-free structures). This cost is visible: every atomic operation requires an explicit ordering parameter.

## Specification

### Atomic Types

| Type | Size | Description |
|------|------|-------------|
| `AtomicBool` | 1 byte | Boolean flag |
| `AtomicI8` | 1 byte | Signed 8-bit |
| `AtomicI16` | 2 bytes | Signed 16-bit |
| `AtomicI32` | 4 bytes | Signed 32-bit |
| `AtomicI64` | 8 bytes | Signed 64-bit |
| `AtomicU8` | 1 byte | Unsigned 8-bit |
| `AtomicU16` | 2 bytes | Unsigned 16-bit |
| `AtomicU32` | 4 bytes | Unsigned 32-bit |
| `AtomicU64` | 8 bytes | Unsigned 64-bit |
| `AtomicUsize` | Pointer-size | Unsigned pointer-sized |
| `AtomicIsize` | Pointer-size | Signed pointer-sized |
| `AtomicPtr<T>` | Pointer-size | Raw pointer to T |

**Type properties:**

| Property | Value |
|----------|-------|
| `Sync` | All atomic types implement `Sync` (safe to share across threads) |
| `Send` | All atomic types implement `Send` (safe to transfer across threads) |
| Copy | Atomic types are NOT `Copy` (prevent accidental non-atomic copies) |
| Clone | Atomic types do NOT implement `Clone` |
| Interior mutability | Atomic operations through shared reference (`&AtomicT`) |

**Platform support:**

| Type | 32-bit platforms | 64-bit platforms |
|------|------------------|------------------|
| `AtomicI64`, `AtomicU64` | May be emulated (slower) | Native |
| All others | Native | Native |

**Alignment:** Atomic types are aligned to their size (e.g., `AtomicI32` is 4-byte aligned).

### Extended Atomic Types (Platform-Dependent)

These types are only available on platforms with native hardware support. Code using them MUST NOT compile on unsupported platforms.

| Type | Size | Availability |
|------|------|--------------|
| `AtomicI128` | 16 bytes | x86-64, ARM64 |
| `AtomicU128` | 16 bytes | x86-64, ARM64 |
| `AtomicF32` | 4 bytes | Most platforms |
| `AtomicF64` | 8 bytes | Most platforms |

**Platform detection:**

| Constant | Type | Meaning |
|----------|------|---------|
| `target.has_atomic128` | `comptime bool` | 128-bit atomics available |
| `target.has_atomic_float` | `comptime bool` | Floating-point atomics available |

**Conditional usage:**

```
comptime if target.has_atomic128 {
    static TAGGED_PTR: AtomicU128 = AtomicU128.new(0)
} else {
    // Alternative implementation using locks
    static TAGGED_PTR: Mutex<u128> = Mutex.new(0)
}
```

**Rationale:** Lock-based emulation of 128-bit atomics is 10x slower than native support. Hiding this cost would violate Transparency (TC >= 0.90). Compile-time detection allows library authors to provide both paths explicitly.

#### AtomicU128 / AtomicI128

**Alignment:** MUST be 16-byte aligned. Unaligned access is UB on x86-64 (`CMPXCHG16B` requirement).

**Operations:** Same as integer atomics—`new`, `default`, `load`, `store`, `swap`, `compare_exchange`, `compare_exchange_weak`, `fetch_add`, `fetch_sub`, `fetch_and`, `fetch_or`, `fetch_xor`, `fetch_nand`, `fetch_max`, `fetch_min`, `into_inner`.

**Platform implementation:**

| Platform | Implementation |
|----------|----------------|
| x86-64 | `CMPXCHG16B` (requires `cx16` CPU feature, standard since ~2008) |
| ARM64 | `LDXP`/`STXP` pair or `CASP` (ARMv8.1+) |
| Others | Compile error |

#### AtomicF32 / AtomicF64

Floating-point atomics support a subset of operations:

| Operation | Supported | Notes |
|-----------|-----------|-------|
| `new`, `default`, `load`, `store`, `swap` | Yes | |
| `compare_exchange`, `compare_exchange_weak` | Yes | Uses bitwise comparison |
| `fetch_add`, `fetch_sub` | Yes | Floating-point arithmetic |
| `fetch_max`, `fetch_min` | Yes | IEEE comparison |
| Bitwise operations | No | No `fetch_and`, `fetch_or`, etc. |
| `into_inner` | Yes | |

**Comparison semantics:** `compare_exchange` uses **bitwise equality**, not IEEE equality:
- `NaN == NaN` is true (same bit pattern)
- `+0.0 != -0.0` (different bit patterns)

This matches C++20 `atomic<float>` and is required for correctness in CAS loops.

### Memory Orderings

Memory orderings control how atomic operations synchronize with other memory accesses:

| Ordering | Description | Use Case |
|----------|-------------|----------|
| `Relaxed` | No synchronization. Only atomicity guaranteed. | Counters, statistics |
| `Acquire` | Subsequent reads/writes cannot be reordered before this load. | Lock acquisition |
| `Release` | Previous reads/writes cannot be reordered after this store. | Lock release, publishing data |
| `AcqRel` | Both Acquire and Release. | Read-modify-write in lock |
| `SeqCst` | Total ordering across all SeqCst operations. | When in doubt, simple mental model |

**Ordering rules:**

| Operation Type | Valid Orderings |
|----------------|-----------------|
| Load | `Relaxed`, `Acquire`, `SeqCst` |
| Store | `Relaxed`, `Release`, `SeqCst` |
| Read-modify-write | All orderings |
| Compare-exchange | Success and failure orderings (failure ≤ success) |

**Mental model:**

```
Thread A (producer):         Thread B (consumer):
  data = 42                    while !ready.load(Acquire) {}
  ready.store(true, Release)   print(data)  // guaranteed to see 42
```

Release-Acquire forms a "happens-before" relationship. All writes before the Release are visible after the Acquire.

### Atomic Operations

#### Construction

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `new(v)` | `T -> AtomicT` | Create atomic with initial value |
| `default()` | `() -> AtomicT` | Create atomic with default value (0, false, null) |

```
let counter = AtomicU64.new(0)
let flag = AtomicBool.default()  // false
```

#### Load and Store

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `load(order)` | `read self, Ordering -> T` | Atomically read the value |
| `store(v, order)` | `read self, T, Ordering -> ()` | Atomically write the value |

**Note:** `store` takes `read self` because atomics use interior mutability—the atomic handles synchronization internally.

```
let value = counter.load(Relaxed)
counter.store(100, Release)
```

#### Exchange

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `swap(v, order)` | `read self, T, Ordering -> T` | Atomically replace, return old value |

```
let old = counter.swap(new_value, AcqRel)
```

#### Compare-and-Exchange

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `compare_exchange(current, new, success, fail)` | `read self, T, T, Ordering, Ordering -> Result<T, T>` | If value == current, set to new. Returns Ok(old) on success, Err(actual) on failure. |
| `compare_exchange_weak(current, new, success, fail)` | Same | MAY spuriously fail. Use in loops. |

**Compare-exchange ordering constraint:** `failure_order` MUST be no stronger than `success_order`, and MUST NOT be `Release` or `AcqRel`.

```
// Increment if below threshold
loop {
    let current = counter.load(Relaxed)
    if current >= threshold {
        break
    }
    match counter.compare_exchange_weak(current, current + 1, AcqRel, Relaxed) {
        Ok(_) => break,
        Err(_) => continue,  // Retry
    }
}
```

**Strong vs Weak:**
- `compare_exchange`: MUST succeed if value matches. Use for single-attempt operations.
- `compare_exchange_weak`: MAY fail spuriously even if value matches. More efficient in loops on some architectures.

#### Fetch Operations (Integers Only)

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `fetch_add(v, order)` | `read self, T, Ordering -> T` | Add and return OLD value |
| `fetch_sub(v, order)` | `read self, T, Ordering -> T` | Subtract and return OLD value |
| `fetch_and(v, order)` | `read self, T, Ordering -> T` | Bitwise AND and return OLD value |
| `fetch_or(v, order)` | `read self, T, Ordering -> T` | Bitwise OR and return OLD value |
| `fetch_xor(v, order)` | `read self, T, Ordering -> T` | Bitwise XOR and return OLD value |
| `fetch_nand(v, order)` | `read self, T, Ordering -> T` | Bitwise NAND and return OLD value |
| `fetch_max(v, order)` | `read self, T, Ordering -> T` | Max and return OLD value |
| `fetch_min(v, order)` | `read self, T, Ordering -> T` | Min and return OLD value |

```
let old_count = counter.fetch_add(1, Relaxed)
```

**Wrapping:** Fetch operations MUST wrap on overflow (like `Wrapping<T>` arithmetic). No panic, no undefined behavior.

#### AtomicBool Operations

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `fetch_and(v, order)` | `read self, bool, Ordering -> bool` | AND and return OLD |
| `fetch_or(v, order)` | `read self, bool, Ordering -> bool` | OR and return OLD |
| `fetch_xor(v, order)` | `read self, bool, Ordering -> bool` | XOR and return OLD |
| `fetch_nand(v, order)` | `read self, bool, Ordering -> bool` | NAND and return OLD |

#### AtomicPtr Operations

`AtomicPtr<T>` stores a raw pointer `*T`:

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `new(ptr)` | `*T -> AtomicPtr<T>` | Create from raw pointer |
| `load(order)` | `read self, Ordering -> *T` | Load pointer |
| `store(ptr, order)` | `read self, *T, Ordering -> ()` | Store pointer |
| `swap(ptr, order)` | `read self, *T, Ordering -> *T` | Swap pointer |
| `compare_exchange(...)` | Same as integers | CAS on pointer |

**Dereferencing the loaded pointer requires unsafe:**

```
let ptr = atomic_ptr.load(Acquire)  // Safe: just a pointer value
unsafe {
    let value = *ptr  // Unsafe: dereferencing raw pointer
}
```

### Memory Fences

Fences enforce ordering without an atomic variable:

| Operation | Description |
|-----------|-------------|
| `fence(Acquire)` | All subsequent reads/writes cannot be reordered before this fence |
| `fence(Release)` | All previous reads/writes cannot be reordered after this fence |
| `fence(AcqRel)` | Both Acquire and Release |
| `fence(SeqCst)` | Full memory barrier |

```
// Using fence instead of Release store
data = 42
fence(Release)
ready.store(true, Relaxed)  // Relaxed is now sufficient
```

**Compiler fence (no CPU barrier):**

| Operation | Description |
|-----------|-------------|
| `compiler_fence(order)` | Prevents compiler reordering only |

Use for signal handlers, memory-mapped I/O, or when you know hardware provides ordering.

### Ordering Guidelines

| Scenario | Recommended Ordering |
|----------|---------------------|
| Simple counter (stats, metrics) | `Relaxed` |
| Flag to signal "data ready" | Writer: `Release`, Reader: `Acquire` |
| Spin lock acquire | `Acquire` on successful CAS |
| Spin lock release | `Release` store |
| Reference count increment | `Relaxed` |
| Reference count decrement (checking for zero) | `AcqRel` |
| Unknown / unsure | `SeqCst` (safest, may be slower) |

**Performance hierarchy (fastest to slowest):**
```
Relaxed < Acquire = Release < AcqRel < SeqCst
```

On x86, `Relaxed`, `Acquire`, and `Release` are typically free (x86 has strong ordering). On ARM/RISC-V, weaker orderings can be significantly faster.

### Safe vs Unsafe

**Safe operations (no `unsafe` required):**
- All atomic type operations (load, store, swap, CAS, fetch_*)
- Memory fences

**Unsafe operations:**
- Dereferencing `AtomicPtr<T>.load()` result REQUIRES unsafe
- Converting raw pointers to/from `AtomicPtr` values REQUIRES unsafe for the pointer operations

**Rationale:** Atomic operations CANNOT cause data races—the hardware guarantees atomicity. The type system prevents mixing atomic and non-atomic access to atomic values. Logical errors (ABA problem, incorrect ordering) are possible but do NOT violate memory safety.

### Non-Atomic Access

Getting the inner value when you have exclusive ownership:

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `get_mut()` | `mutate self -> *mut T` | Get raw pointer to inner value (unsafe to dereference) |
| `into_inner()` | `transfer self -> T` | Consume atomic, return inner value |

```
let mut counter = AtomicU64.new(0)
let final_value = counter.into_inner()  // Consume and extract
```

`into_inner` is safe because `transfer self` guarantees exclusive ownership—no other tasks can access the atomic.

## Examples

### Simple Counter

```
static REQUESTS: AtomicU64 = AtomicU64.new(0)

fn handle_request(req: Request) {
    REQUESTS.fetch_add(1, Relaxed)  // No ordering needed for stats
    // ... process request
}

fn get_stats() -> u64 {
    REQUESTS.load(Relaxed)
}
```

### Flag for Signaling

```
static SHUTDOWN: AtomicBool = AtomicBool.new(false)

fn worker_loop() {
    while !SHUTDOWN.load(Acquire) {
        do_work()
    }
}

fn request_shutdown() {
    SHUTDOWN.store(true, Release)
}
```

### Bounded Counter (CAS Loop)

```
fn increment_if_below(counter: read AtomicU64, max: u64) -> bool {
    loop {
        let current = counter.load(Relaxed)
        if current >= max {
            return false
        }
        match counter.compare_exchange_weak(current, current + 1, AcqRel, Relaxed) {
            Ok(_) => return true,
            Err(_) => continue,
        }
    }
}
```

### Reference Counting Pattern

This sketch shows how atomics enable reference counting. Actual `Arc<T>` implementation uses raw pointers internally:

```
// Conceptual structure (uses raw pointers internally)
struct ArcInner<T> {
    count: AtomicUsize,
    value: T,
}

// Clone increments count
fn arc_clone<T>(ptr: *ArcInner<T>) -> *ArcInner<T> {
    unsafe {
        (*ptr).count.fetch_add(1, Relaxed)  // Relaxed: already have access
    }
    ptr
}

// Drop decrements count, frees if zero
fn arc_drop<T>(ptr: *mut ArcInner<T>) {
    unsafe {
        // AcqRel: synchronize with other drops
        if (*ptr).count.fetch_sub(1, AcqRel) == 1 {
            fence(Acquire)  // See all writes before freeing
            dealloc(ptr)
        }
    }
}
```

### Spin Lock Pattern

This sketch shows how atomics enable spin locks. The actual stdlib implementation wraps this in a safe API:

```
struct SpinLockInner<T> {
    locked: AtomicBool,
    data: T,  // Access requires holding lock
}

fn spin_acquire(lock: *SpinLockInner<T>) {
    unsafe {
        while (*lock).locked.compare_exchange_weak(
            false, true, Acquire, Relaxed
        ).is_err() {
            while (*lock).locked.load(Relaxed) {
                spin_hint()
            }
        }
    }
}

fn spin_release(lock: *SpinLockInner<T>) {
    unsafe {
        (*lock).locked.store(false, Release)
    }
}
```

**Note:** These patterns use raw pointers and unsafe blocks. The stdlib provides safe wrappers (e.g., `Mutex<T>`, `Arc<T>`) that encapsulate the unsafe implementation.

## Integration Notes

- **Unsafe:** Atomic operations are safe. Dereferencing `AtomicPtr` results REQUIRES unsafe.
- **Concurrency:** Atomics are the primitive for building higher-level synchronization (Mutex, RwLock, channels use atomics internally). Per CORE_DESIGN "No shared mutable memory between tasks"—atomics are the explicit escape hatch when cross-task state is genuinely needed.
- **Memory Model:** Data races on non-atomic locations are UB. Atomics provide defined behavior for concurrent access. Mixing atomic and non-atomic access to the same location is UB.
- **Statics:** Atomic statics (`static COUNTER: AtomicU64`) MAY be safely accessed from multiple threads without `unsafe`.
- **Comptime:** Atomics are NOT available at compile time (no meaningful semantics without threads).
- **Generics:** Generic code MAY require `T: Sync` to accept atomic types.
- **C Interop:** Atomic types are ABI-compatible with C11 `_Atomic` types and C++ `std::atomic`.
- **No Storable References:** Rask's "no storable references" principle still applies. Lock guards and similar patterns use expression-scoped access or closure-based APIs, not stored references to lock state.

---

## Remaining Issues

### Low Priority

1. **Wait/wake primitives** — Futex-like operations (`wait`, `wake`) for efficient blocking. Currently must use OS primitives. Could be standardized.

2. **Consume ordering** — C11 has `memory_order_consume` but compilers treat it as `Acquire`. Omitted for now.

3. **Seqlock pattern** — Common read-heavy pattern. Consider library support or documentation.
