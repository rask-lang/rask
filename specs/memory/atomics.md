<!-- id: mem.atomics -->
<!-- status: decided -->
<!-- summary: Safe atomic types with explicit memory ordering; no unsafe needed for atomic operations -->
<!-- depends: memory/unsafe.md, concurrency/sync.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Atomics

Atomic types provide safe, data-race-free shared memory access with explicit memory ordering.

## Core Rules

| Rule | Description |
|------|-------------|
| **AT1: Safe operations** | All atomic load/store/swap/CAS/fetch operations are safe — no `unsafe` needed |
| **AT2: Explicit ordering** | Every operation requires a memory ordering parameter |
| **AT3: Not Copy** | Atomic types are not `Copy` or `Clone` (prevents accidental non-atomic copies) |
| **AT4: Interior mutability** | Operations through shared reference (`&AtomicT`) — the atomic handles synchronization |
| **AT5: Wrapping arithmetic** | Fetch operations wrap on overflow. No panic, no undefined behavior |
| **AT6: Ordering constraints** | CAS failure ordering must be no stronger than success ordering, and must not be `Release` or `AcqRel` |
| **AT7: Platform-dependent types** | 128-bit and float atomics require hardware support; code must not compile on unsupported platforms |

## Atomic Types

| Type | Size | Description |
|------|------|-------------|
| `AtomicBool` | 1 byte | Boolean flag |
| `AtomicI8` / `AtomicU8` | 1 byte | 8-bit integer |
| `AtomicI16` / `AtomicU16` | 2 bytes | 16-bit integer |
| `AtomicI32` / `AtomicU32` | 4 bytes | 32-bit integer |
| `AtomicI64` / `AtomicU64` | 8 bytes | 64-bit integer |
| `AtomicUsize` / `AtomicIsize` | Pointer-size | Pointer-sized integer |
| `AtomicPtr<T>` | Pointer-size | Raw pointer to T |

**Properties:**

| Property | Value |
|----------|-------|
| `Sync` | Yes — safe to share across threads |
| `Send` | Yes — safe to transfer across threads |
| `Copy` / `Clone` | No (AT3) |
| Interior mutability | Yes (AT4) |
| Alignment | Aligned to type size (e.g. `AtomicI32` = 4-byte aligned) |

`AtomicI64` / `AtomicU64` may be emulated (slower) on 32-bit platforms. All others are native everywhere.

## Memory Orderings

| Ordering | Description | Use Case |
|----------|-------------|----------|
| `Relaxed` | No synchronization. Only atomicity guaranteed. | Counters, statistics |
| `Acquire` | Subsequent reads/writes cannot be reordered before this load. | Lock acquisition |
| `Release` | Previous reads/writes cannot be reordered after this store. | Lock release, publishing data |
| `AcqRel` | Both Acquire and Release. | Read-modify-write in lock |
| `SeqCst` | Total ordering across all SeqCst operations. | When in doubt |

**Valid orderings per operation type:**

| Operation Type | Valid Orderings |
|----------------|-----------------|
| Load | `Relaxed`, `Acquire`, `SeqCst` |
| Store | `Relaxed`, `Release`, `SeqCst` |
| Read-modify-write | All orderings |
| Compare-exchange | Success and failure orderings (AT6: failure ≤ success) |

**Mental model:** Release-Acquire forms a "happens-before" relationship. All writes before the Release are visible after the Acquire.

<!-- test: parse -->
```rask
// Thread A (producer):          Thread B (consumer):
//   data = 42                     while !ready.load(Acquire) {}
//   ready.store(true, Release)    print(data)  // guaranteed to see 42
```

## Operations

### Construction

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `new(v)` | `T -> AtomicT` | Create atomic with initial value |
| `default()` | `() -> AtomicT` | Create atomic with default value (0, false, null) |

<!-- test: skip -->
```rask
const counter = AtomicU64.new(0)
const flag = AtomicBool.default()  // false
```

### Load, Store, Swap

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `load(order)` | `self, Ordering -> T` | Atomically read the value |
| `store(v, order)` | `self, T, Ordering -> ()` | Atomically write the value |
| `swap(v, order)` | `self, T, Ordering -> T` | Atomically replace, return old value |

`store` takes `self` (not `mutate self`) because atomics use interior mutability (AT4).

<!-- test: skip -->
```rask
const value = counter.load(Relaxed)
counter.store(100, Release)
const old = counter.swap(new_value, AcqRel)
```

### Compare-and-Exchange

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `compare_exchange(current, new, success, fail)` | `self, T, T, Ordering, Ordering -> T or T` | If value == current, set to new. Ok(old) on success, Err(actual) on failure |
| `compare_exchange_weak(current, new, success, fail)` | Same | May spuriously fail. Use in loops |

- `compare_exchange`: Must succeed if value matches. Use for single-attempt operations.
- `compare_exchange_weak`: May fail spuriously even if value matches. More efficient in loops on some architectures.

<!-- test: skip -->
```rask
loop {
    const current = counter.load(Relaxed)
    if current >= threshold {
        break
    }
    match counter.compare_exchange_weak(current, current + 1, AcqRel, Relaxed) {
        Ok(_) => break,
        Err(_) => continue,
    }
}
```

### Fetch Operations (Integers Only)

All fetch operations return the OLD value (AT5: wrapping on overflow).

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `fetch_add(v, order)` | `self, T, Ordering -> T` | Add |
| `fetch_sub(v, order)` | `self, T, Ordering -> T` | Subtract |
| `fetch_and(v, order)` | `self, T, Ordering -> T` | Bitwise AND |
| `fetch_or(v, order)` | `self, T, Ordering -> T` | Bitwise OR |
| `fetch_xor(v, order)` | `self, T, Ordering -> T` | Bitwise XOR |
| `fetch_nand(v, order)` | `self, T, Ordering -> T` | Bitwise NAND |
| `fetch_max(v, order)` | `self, T, Ordering -> T` | Max |
| `fetch_min(v, order)` | `self, T, Ordering -> T` | Min |

`AtomicBool` supports `fetch_and`, `fetch_or`, `fetch_xor`, `fetch_nand` with `bool` operands.

### AtomicPtr Operations

`AtomicPtr<T>` stores a raw pointer `*T`. Supports `new`, `load`, `store`, `swap`, `compare_exchange`, `compare_exchange_weak`.

Dereferencing the loaded pointer requires `unsafe` (AT1 applies to the atomic operation itself, not the pointer):

<!-- test: skip -->
```rask
const ptr = atomic_ptr.load(Acquire)  // Safe: just a pointer value
unsafe {
    const value = *ptr  // Unsafe: dereferencing raw pointer
}
```

### Non-Atomic Access

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `get_mut()` | `self -> *T` | Get raw pointer to inner value (unsafe to dereference) |
| `into_inner()` | `take self -> T` | Consume atomic, return inner value |

`into_inner` is safe because `take self` guarantees exclusive ownership.

<!-- test: skip -->
```rask
let counter = AtomicU64.new(0)
const final_value = counter.into_inner()
```

## Memory Fences

Fences enforce ordering without an atomic variable.

| Operation | Description |
|-----------|-------------|
| `fence(Acquire)` | All subsequent reads/writes cannot be reordered before this fence |
| `fence(Release)` | All previous reads/writes cannot be reordered after this fence |
| `fence(AcqRel)` | Both Acquire and Release |
| `fence(SeqCst)` | Full memory barrier |
| `compiler_fence(order)` | Prevents compiler reordering only (no CPU barrier) |

`compiler_fence` is for signal handlers, memory-mapped I/O, or when hardware provides ordering guarantees.

<!-- test: skip -->
```rask
data = 42
fence(Release)
ready.store(true, Relaxed)  // Relaxed is sufficient after fence
```

## Extended Atomic Types (Platform-Dependent)

Per AT7, these only compile on platforms with native hardware support.

| Type | Size | Availability |
|------|------|--------------|
| `AtomicI128` / `AtomicU128` | 16 bytes | x86-64, ARM64 |
| `AtomicF32` / `AtomicF64` | 4 / 8 bytes | Most platforms |

**Platform detection:**

| Constant | Type | Meaning |
|----------|------|---------|
| `target.has_atomic128` | `comptime bool` | 128-bit atomics available |
| `target.has_atomic_float` | `comptime bool` | Floating-point atomics available |

<!-- test: skip -->
```rask
comptime if target.has_atomic128 {
    static TAGGED_PTR: AtomicU128 = AtomicU128.new(0)
} else {
    static TAGGED_PTR: Mutex<u128> = Mutex.new(0)
}
```

### AtomicU128 / AtomicI128

Must be 16-byte aligned (unaligned access is UB on x86-64 `CMPXCHG16B`). Same operations as integer atomics.

| Platform | Implementation |
|----------|----------------|
| x86-64 | `CMPXCHG16B` (requires `cx16`, standard since ~2008) |
| ARM64 | `LDXP`/`STXP` or `CASP` (ARMv8.1+) |
| Others | Compile error |

### AtomicF32 / AtomicF64

Floating-point atomics support a subset of operations:

| Operation | Supported | Notes |
|-----------|-----------|-------|
| `new`, `default`, `load`, `store`, `swap` | Yes | |
| `compare_exchange`, `compare_exchange_weak` | Yes | Uses bitwise comparison |
| `fetch_add`, `fetch_sub` | Yes | Floating-point arithmetic |
| `fetch_max`, `fetch_min` | Yes | IEEE comparison |
| Bitwise operations | No | No `fetch_and`, `fetch_or`, etc. |

`compare_exchange` uses **bitwise equality**: `NaN == NaN` (same bit pattern), `+0.0 != -0.0` (different bit patterns). This matches C++20 `atomic<float>` and is required for correctness in CAS loops.

## Error Messages

```
ERROR [mem.atomics/AT2]: missing memory ordering
   |
12 |  counter.fetch_add(1)
   |  ^^^^^^^^^^^^^^^^^^^^ atomic operations require an explicit ordering parameter

FIX: counter.fetch_add(1, Relaxed)
```

```
ERROR [mem.atomics/AT6]: invalid failure ordering for compare_exchange
   |
8  |  x.compare_exchange(old, new, Acquire, AcqRel)
   |                                        ^^^^^^ failure ordering must be ≤ success ordering

WHY: Failure ordering cannot be Release or AcqRel, and cannot be stronger than success ordering.

FIX: x.compare_exchange(old, new, Acquire, Relaxed)
```

```
ERROR [mem.atomics/AT7]: AtomicU128 not available on this platform
   |
3  |  static COUNTER: AtomicU128 = AtomicU128.new(0)
   |                  ^^^^^^^^^^ requires native 128-bit atomic support

WHY: Lock-based emulation would hide a 10x cost, violating transparency.

FIX: Use comptime if target.has_atomic128 { ... } to provide both paths.
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| CAS failure ordering > success ordering | AT6 | Compile error |
| `Release` ordering on load | AT2 | Compile error (invalid for loads) |
| `Acquire` ordering on store | AT2 | Compile error (invalid for stores) |
| Mixing atomic and non-atomic access to same location | — | Undefined behavior |
| Overflow on `fetch_add` | AT5 | Wraps (no panic) |
| `AtomicPtr.load` then deref | AT1 | Load is safe; deref requires `unsafe` |
| `into_inner` on shared atomic | AT3 | Requires `take self` — exclusive ownership |
| Atomics at comptime | — | Not available (no meaningful semantics without threads) |
| Atomic statics | AT1 | Safe to access from multiple threads without `unsafe` |

---

## Appendix (non-normative)

### Rationale

**AT1 (safe operations):** Atomic operations can't cause data races — the hardware guarantees atomicity. The type system prevents mixing atomic and non-atomic access. Logical errors (ABA, incorrect ordering) are possible but don't violate memory safety.

**AT2 (explicit ordering):** CORE_DESIGN says "no shared mutable memory between tasks" — atomics are the explicit escape hatch when you genuinely need it. Making ordering explicit keeps the cost visible.

**AT7 (platform-dependent):** Lock-based emulation of 128-bit atomics is 10x slower than native support. Hiding this cost would violate transparency. Compile-time detection lets library authors provide both paths.

**C interop:** Atomic types are ABI-compatible with C11 `_Atomic` types and C++ `std::atomic`.

### Patterns & Guidance

**Ordering selection:**

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

<!-- test: parse -->
```rask
// Relaxed < Acquire = Release < AcqRel < SeqCst
```

On x86, `Relaxed`, `Acquire`, and `Release` are typically free (x86 has strong ordering). On ARM/RISC-V, weaker orderings can be significantly faster.

### Examples

**Simple counter:**

<!-- test: skip -->
```rask
static REQUESTS: AtomicU64 = AtomicU64.new(0)

func handle_request(req: Request) {
    REQUESTS.fetch_add(1, Relaxed)
    // ... process request
}

func get_stats() -> u64 {
    return REQUESTS.load(Relaxed)
}
```

**Flag for signaling:**

<!-- test: skip -->
```rask
static SHUTDOWN: AtomicBool = AtomicBool.new(false)

func worker_loop() {
    while !SHUTDOWN.load(Acquire) {
        do_work()
    }
}

func request_shutdown() {
    SHUTDOWN.store(true, Release)
}
```

**Bounded counter (CAS loop):**

<!-- test: skip -->
```rask
func increment_if_below(counter: AtomicU64, max: u64) -> bool {
    loop {
        const current = counter.load(Relaxed)
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

**Reference counting (sketch):**

<!-- test: skip -->
```rask
struct ArcInner<T> {
    count: AtomicUsize,
    value: T,
}

func arc_clone<T>(ptr: *ArcInner<T>) -> *ArcInner<T> {
    unsafe {
        (*ptr).count.fetch_add(1, Relaxed)
    }
    return ptr
}

func arc_drop<T>(ptr: *ArcInner<T>) {
    unsafe {
        if (*ptr).count.fetch_sub(1, AcqRel) == 1 {
            fence(Acquire)
            dealloc(ptr)
        }
    }
}
```

**Spin lock (sketch):**

<!-- test: skip -->
```rask
struct SpinLockInner<T> {
    locked: AtomicBool,
    data: T,
}

func spin_acquire<T>(lock: *SpinLockInner<T>) {
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

func spin_release<T>(lock: *SpinLockInner<T>) {
    unsafe {
        (*lock).locked.store(false, Release)
    }
}
```

These patterns use raw pointers and unsafe blocks. The stdlib provides safe wrappers (`Mutex<T>`, `Arc<T>`) that encapsulate the unsafe implementation.

### See Also

- [Synchronization Primitives](../concurrency/sync.md) — `Mutex<T>`, `Shared<T>` for compound data (`conc.sync`)
- [Concurrency](../concurrency/async.md) — Channels and task spawning (`conc.async`)
- [Unsafe](unsafe.md) — Raw pointer dereferencing for `AtomicPtr` results (`mem.unsafe`)
