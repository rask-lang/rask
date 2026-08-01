<!-- id: mem.atomics -->
<!-- status: decided -->
<!-- summary: Atomic<T> for any padding-free Copy payload that fits an atomic word; named forms are aliases; explicit memory ordering; no unsafe needed -->
<!-- depends: memory/unsafe.md, concurrency/sync.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Atomics

Atomic types provide safe, data-race-free shared memory access with explicit memory ordering.

There is one atomic type: `Atomic<T>`. It takes any payload the hardware can treat as a single word — integers, floats, `bool`, pointers, and user structs that are Copy, padding-free, and word-sized. The familiar names (`AtomicU64`, `AtomicBool`, …) are transparent aliases for the common payloads. Which operations exist follows from the payload: everything gets load/store/swap/CAS, integers additionally count, floats additionally add — a struct payload gets no `fetch_add` because adding two structs means nothing.

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

## The `Atomic<T>` Type

| Rule | Description |
|------|-------------|
| **GA1: One type** | `Atomic<T>` is the atomic type. The named forms below are transparent aliases (`type.aliases`), not separate types — `Atomic<u64>` and `AtomicU64` are the same thing everywhere |
| **GA2: Eligibility** | `T` must be Copy, contain no padding bytes, and be 1, 2, 4, or 8 bytes — or 16 with `target.has_atomic128` (AT7). Float payloads additionally require `target.has_atomic_float`. Violation is a compile error at the type, with the reason named |
| **GA3: Ops follow the payload** | Every eligible payload gets `new`, `load`, `store`, `swap`, `compare_exchange`, `compare_exchange_weak`, `into_value`, `get_mut`. Integer payloads add the full fetch family; `bool` adds the logical fetches; floats add `fetch_add`/`fetch_sub`/`fetch_max`/`fetch_min`. Struct payloads get none — `fetch_add` on a struct is meaningless |
| **GA4: CAS is bitwise** | `compare_exchange` compares raw bytes. This is why GA2 excludes padding: two logically equal values with different padding bytes would spuriously fail CAS. Same rule float CAS already follows (`NaN == NaN` when bit patterns match, `+0.0 != -0.0`) |
| **GA5: Optional payloads** | `Atomic<T?>` is rejected in general — an arbitrary `T` has no spare bit pattern for `none`. The one exception is `Atomic<Handle<T>?>` (alias: `AtomicHandle<T>`), where the compiler reserves a sentinel (AH2) |

Struct payloads are the point of the generality ([#497](https://github.com/rask-lang/rask/issues/497)). An 8-byte two-field struct is exactly as atomic-eligible as a `u64`, and the compiler does the packing that hand-written shift-and-mask code gets wrong silently:

<!-- test: skip -->
```rask
struct Slot {
    index: u32,
    gen: u32,
}   // 8 bytes, Copy, no padding — fits an atomic word

const current = Atomic<Slot>.new(Slot { index: 0, gen: 0 })

const old = current.load(Acquire)
const next = Slot { index: old.index + 1, gen: old.gen }
match current.compare_exchange(old, next, AcqRel, Relaxed) {
    Slot as _      => {},          // swapped as one unit
    CasFailed as _ => retry(),
}
```

Add a field to `Slot` and it either still fits (nothing to update) or the `Atomic<Slot>` declaration errors — no call site can silently read a garbled value, which is what hand-packing into an `AtomicU64` gives you.

### Named aliases

| Alias | Payload | Size |
|-------|---------|------|
| `AtomicBool` | `bool` | 1 byte |
| `AtomicI8` / `AtomicU8` | `i8` / `u8` | 1 byte |
| `AtomicI16` / `AtomicU16` | `i16` / `u16` | 2 bytes |
| `AtomicI32` / `AtomicU32` | `i32` / `u32` | 4 bytes |
| `AtomicI64` / `AtomicU64` | `i64` / `u64` | 8 bytes |
| `AtomicUsize` / `AtomicIsize` | `usize` / `isize` | Pointer-size |
| `AtomicF32` / `AtomicF64` | `f32` / `f64` | 4 / 8 bytes |
| `AtomicI128` / `AtomicU128` | `i128` / `u128` | 16 bytes (AT7) |
| `AtomicPtr<T>` | `*T` | Pointer-size |
| `AtomicHandle<T>` | `Handle<T>?` | 8 or 16 bytes (AH1) |

Use whichever reads better at the site — `AtomicU64` for a counter, `Atomic<Slot>` for your own payload. They alias, so there's nothing to convert between.

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
| `new(v)` | `T -> Atomic<T>` | Create atomic with initial value |
| `default()` | `() -> Atomic<T>` | Create atomic with default value (0, false, null pointer). Primitive payloads only — a struct payload has no compiler-known default, use `new` |

<!-- test: skip -->
```rask
const counter = AtomicU64.new(0)
const flag = AtomicBool.new(false)
const slot = Atomic<Slot>.new(Slot { index: 0, gen: 0 })
```

### Load, Store, Swap

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `load(order)` | `self, Ordering -> T` | Atomically read the value |
| `store(v, order)` | `self, T, Ordering -> void` | Atomically write the value |
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
| `compare_exchange(current, new, success, fail)` | `self, T, T, Ordering, Ordering -> T or CasFailed<T>` | If value == current, set to new. Returns old on success, `CasFailed(actual)` on failure |
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
        u64 as _ => break,
        CasFailed as _ => continue,
    }
}
```

### Fetch Operations (integer payloads)

Per GA3, the fetch family exists where the payload can do arithmetic. All fetch operations return the OLD value (AT5: wrapping on overflow).

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

`AtomicBool` supports `fetch_and`, `fetch_or`, `fetch_xor`, `fetch_nand` with `bool` operands. Float payloads get `fetch_add`, `fetch_sub`, `fetch_max`, `fetch_min` (see the float section below). Struct payloads get no fetch operations — read-modify-write on a struct is a CAS loop, where the modify step is ordinary visible code.

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

### AtomicHandle Operations

`AtomicHandle<T>` is the alias for `Atomic<Handle<T>?>` — the one optional payload GA5 admits, because the compiler owns `Handle`'s layout and can reserve a bit pattern for `none`. Handle fields (pool_id, index, generation) are packed into a single atomic word; the packing is GA2/GA4 at work on a compiler-defined struct rather than a separate mechanism.

| Rule | Description |
|------|-------------|
| **AH1: Packing** | Handle fields packed into `AtomicU64` (≤8 byte handles) or `AtomicU128` (≤16 byte, requires `target.has_atomic128`) |
| **AH2: Nullable** | Holds `Handle<T>?` — `none` is a sentinel bit pattern distinct from any valid handle |
| **AH3: ABA protection** | Generation counter in the handle prevents ABA — a reused slot gets a different generation, so CAS on a recycled handle correctly fails |
| **AH4: Pool validation** | Atomicity guarantees a consistent load, not that the handle is live. Validate with `pool.get(h)` before access |

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `new(h)` | `Handle<T> -> AtomicHandle<T>` | Create with initial handle |
| `none()` | `() -> AtomicHandle<T>` | Create empty (sentinel) |
| `load(order)` | `self, Ordering -> Handle<T>?` | Atomically read |
| `store(h, order)` | `self, Handle<T>?, Ordering` | Atomically write |
| `swap(h, order)` | `self, Handle<T>?, Ordering -> Handle<T>?` | Replace, return old |
| `compare_exchange(cur, new, succ, fail)` | `self, Handle<T>?, Handle<T>?, Ordering, Ordering -> Handle<T>? or CasFailed<Handle<T>?>` | CAS |
| `compare_exchange_weak(cur, new, succ, fail)` | Same | May spuriously fail |

**Handle size:** Default `Handle<T>` is 12 bytes — requires `AtomicU128` (x86-64, ARM64). Compact handles (`Pool<T, PoolId=u16, Index=u16, Gen=u32>`) are 8 bytes — work everywhere via `AtomicU64`. Compile error if handle exceeds the available atomic word size.

<!-- test: skip -->
```rask
// Atomic "latest value" slot — multiple writers, readers see most recent
const latest: AtomicHandle<Reading> = AtomicHandle.none()

func publish(mutate pool: Pool<Reading>, value: Reading) {
    const h = pool.insert(value)
    const prev = latest.swap(h, Release)
    if prev? as old_h {
        pool.remove(old_h)
    }
}

func read_latest(pool: Pool<Reading>) -> Reading? {
    const h = latest.load(Acquire) ?? return none
    return pool.get(h)   // none if writer just swapped and removed
}
```

### Non-Atomic Access

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `get_mut()` | `self -> *T` | Get raw pointer to inner value (unsafe to dereference) |
| `into_value()` | `take self -> T` | Consume atomic, return inner value |

`into_value` is safe because `take self` guarantees exclusive ownership.

<!-- test: skip -->
```rask
mut counter = AtomicU64.new(0)
const final_value = counter.into_value()
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

## Platform-Dependent Payloads

Per AT7 and GA2, these payloads only compile on platforms with native hardware support.

| Payload | Size | Availability |
|---------|------|--------------|
| `i128` / `u128`, any 16-byte struct | 16 bytes | x86-64, ARM64 |
| `f32` / `f64` | 4 / 8 bytes | Most platforms |

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

**AtomicHandle size mismatch [AH1]:**
```
ERROR [mem.atomics/AH1]: Handle<Entity> is 12 bytes — requires AtomicU128
   |
5  |  const head: AtomicHandle<Entity> = AtomicHandle.none()
   |              ^^^^^^^^^^^^^^^^^^^^ handle does not fit in AtomicU64

WHY: Default Handle is 12 bytes (PoolId=u32, Index=u32, Gen=u32).
     AtomicU128 is not available on this platform.

FIX 1: Use compact pool configuration:

  const pool = Pool<Entity, PoolId=u16, Index=u16, Gen=u32>.new()
  // Handle is now 8 bytes — fits in AtomicU64

FIX 2: Use comptime if target.has_atomic128 { ... } for platform-specific paths.
```

**Padding in the payload [GA2]:**
```
ERROR [mem.atomics/GA2]: Tagged has padding — cannot be an atomic payload
   |
4  |  const state: Atomic<Tagged> = Atomic.new(initial)
   |                      ^^^^^^
   |
1  |  struct Tagged {
2  |      kind: u8,      // 1 byte
3  |      value: u32,    // 4 bytes, aligned — 3 padding bytes after `kind`
4  |  }

WHY: compare_exchange compares raw bytes (GA4). Padding bytes have
     unspecified values, so two equal Tagged values could compare unequal
     and CAS would fail spuriously.

FIX: reorder fields largest-first, or make the padding explicit:

  struct Tagged {
      value: u32,
      kind: u8,
      _pad: [u8; 3],   // now every byte is meaningful
  }
```

**Fetch on a struct payload [GA3]:**
```
ERROR [mem.atomics/GA3]: no fetch_add on Atomic<Slot>
   |
9  |  current.fetch_add(delta, AcqRel)
   |          ^^^^^^^^^ Slot is a struct — arithmetic on it has no meaning

FIX: read-modify-write with a CAS loop; the modify step is ordinary code:

  loop {
      const old = current.load(Relaxed)
      const next = Slot { index: old.index + delta, gen: old.gen }
      if current.compare_exchange_weak(old, next, AcqRel, Relaxed)? { break }
  }
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
| `into_value` on shared atomic | AT3 | Requires `take self` — exclusive ownership |
| Atomics at comptime | — | Not available (no meaningful semantics without threads) |
| Atomic statics | AT1 | Safe to access from multiple threads without `unsafe` |
| Struct payload with padding bytes | GA2 | Compile error — reorder fields or pad explicitly |
| Struct payload > 8 bytes without `target.has_atomic128` | GA2/AT7 | Compile error — same gate as `AtomicU128` |
| `fetch_add` on a struct payload | GA3 | Compile error — use a CAS loop |
| `Atomic<T?>` where `T` is not `Handle` | GA5 | Compile error — no spare bit pattern for `none`; add your own sentinel field |
| `default()` on a struct payload | GA3 | Compile error — no compiler-known default, use `new` |
| Handle too large for atomic word | AH1 | Compile error — use compact pool config or platform with `AtomicU128` |
| `AtomicHandle.load` then `pool[h]` | AH4 | Handle may be stale — use `pool.get(h)` for safe validation |
| CAS on handle to recycled slot | AH3 | Correctly fails — generation mismatch in packed word |
| `AtomicHandle.none()` in CAS expected | AH2 | Works — `none` is a valid bit pattern for comparison |

---

## Appendix (non-normative)

### Rationale

**AT1 (safe operations):** Atomic operations can't cause data races — the hardware guarantees atomicity. The type system prevents mixing atomic and non-atomic access. Logical errors (ABA, incorrect ordering) are possible but don't violate memory safety.

**AT2 (explicit ordering):** CORE_DESIGN says "no shared mutable memory between tasks" — atomics are the explicit escape hatch when you genuinely need it. Making ordering explicit keeps the cost visible.

**AT7 (platform-dependent):** Lock-based emulation of 128-bit atomics is 10x slower than native support. Hiding this cost would violate transparency. Compile-time detection lets library authors provide both paths.

**GA1 (one generic type, names as aliases):** This resolved [#497](https://github.com/rask-lang/rask/issues/497). A fixed menu of named atomics left word-sized user structs with hand-packing — shift-and-mask code where a wrong shift produces a plausible wrong value instead of a type error, and where adding a field breaks nothing visibly. The eligibility check is a static predicate the compiler already answers elsewhere (Copy? fits a word? padding-free?), so withholding it wasn't a design position, just a gap.

The named types stayed as aliases rather than as the primary spelling. The API is tidier with one type, and the cost of that tidiness — operation families that vary by payload — lands entirely inside the compiler, which already special-cases every box-adjacent type. That's the right side of the line: complicated implementation behind a simple surface, the same trade `string`'s refcount elision makes. The alternative (keep named types primary, add `Atomic<T>` alongside) would have been less spec churn but two spellings forever.

`Atomic<T>` over a user payload does not open the box family (`mem.boxes/BX1`–`BX4`): the payload is plain Copy data, and `Atomic` itself stays compiler-provided. Nothing here lets a user type run code at assignment, scope exit, or borrow boundaries — the same relationship `Shared<T>` has to its `T`.

**GA3 (no fetch ops on structs):** `fetch_add` exists because hardware has it for integers. For a struct, "add" has no single meaning, and inventing one (field-wise? user-defined?) would hide a CAS loop behind an innocent-looking method. The CAS loop is the honest spelling: the modify step is visible code between a `load` and a `compare_exchange`.

**GA5 (why `Atomic<Handle<T>?>` and nothing else optional):** an optional payload needs a bit pattern that no valid `T` occupies. The compiler owns `Handle`'s layout and can promise one; it can't promise anything about an arbitrary user struct. Users who need an "empty" state add their own sentinel field — visible in the struct definition, checked by their own code. `AtomicHandle` becoming an alias of the general type (instead of its own privileged thing) was the test that the `Atomic<T>` shape is right.

**C interop:** Atomic types are ABI-compatible with C11 `_Atomic` types and C++ `std::atomic`.

**AH3 (ABA protection):** Traditional lock-free algorithms need separate ABA mitigation — tagged pointers, hazard pointers, or epoch-based reclamation. Handle generation counters provide this structurally: when a pool slot is reused, the generation increments. A stale handle packed into an `AtomicHandle` has a different bit pattern than the new occupant's handle, so CAS correctly rejects it. This doesn't eliminate all concurrency hazards (safe reclamation is still needed), but it removes the most common source of subtle lock-free bugs for free.

**AH4 (pool validation):** AtomicHandle guarantees you loaded a consistent handle value. It does NOT guarantee the handle is still live — another thread may have removed it between your load and your pool access. Always use `pool.get(h)` (returns `T?`) rather than `pool[h]` (panics on stale handle) after loading from an AtomicHandle.

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
| AtomicHandle publish (writer) | `Release` store/swap |
| AtomicHandle consume (reader) | `Acquire` load |
| AtomicHandle CAS (lock-free op) | Success: `AcqRel`, Failure: `Relaxed` |

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
            u64 as _ => return true,
            CasFailed as _ => continue,
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
        ) is CasFailed {
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

**Lock-free stack (sketch using AtomicHandle):**

<!-- test: skip -->
```rask
struct Node<T> {
    data: T
    next: Handle<Node<T>>?
}

struct LockFreeStack<T> {
    pool: Pool<Node<T>, PoolId=u16, Index=u16, Gen=u32>
    head: AtomicHandle<Node<T>>
}

extend LockFreeStack<T> {
    func new() -> LockFreeStack<T> {
        LockFreeStack {
            pool: Pool.new(),
            head: AtomicHandle.none(),
        }
    }

    func push(mutate self, value: T) {
        const node = self.pool.insert(Node { data: value, next: none })
        loop {
            const current = self.head.load(Acquire)
            self.pool[node].next = current
            match self.head.compare_exchange_weak(current, node, Release, Relaxed) {
                Handle as _ => break,
                CasFailed as _ => continue,
            }
        }
    }
}
```

This sketch shows the push path — CAS on handles with generation-based ABA protection. A complete implementation needs thread-safe pool access and deferred reclamation on pop. The stdlib provides `LockFreeStack<T>` and `LockFreeQueue<T>` that handle these concerns internally.

### See Also

- [Synchronization Primitives](../concurrency/sync.md) — `Mutex<T>`, `Shared<T>` for compound data (`conc.sync`)
- [Boxes](boxes.md) — Why atomics sit adjacent to the box family (`mem.boxes`)
- [Concurrency](../concurrency/async.md) — Channels and task spawning (`conc.async`)
- [Unsafe](unsafe.md) — Raw pointer dereferencing for `AtomicPtr` results (`mem.unsafe`)
- [Pools](pools.md) — Handle-based storage, validation for `AtomicHandle` results (`mem.pools`)
- [Ownership](ownership.md) — Atomic values are owned, not reference-typed (`mem.ownership`)
