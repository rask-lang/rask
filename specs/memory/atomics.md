<!-- id: mem.atomics -->
<!-- status: decided -->
<!-- summary: Atomic<T> for any padding-free Copy payload that fits an atomic word; one spelling, no named variants; sequentially consistent unless an order is asked for; no unsafe needed -->
<!-- depends: memory/unsafe.md, concurrency/sync.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Atomics

Atomic types provide safe, data-race-free shared memory access. Every operation is sequentially consistent unless the call asks for something weaker.

There is one atomic type and one way to spell it: `Atomic<T>`. It takes any payload the hardware can treat as a single word — integers, floats, `bool`, pointers, and user structs that are Copy, padding-free, and word-sized. There are no named variants: no `Atomic<u64>`, no `Atomic<bool>` — a reader never has to wonder whether a named form and the generic form differ. Which operations exist follows from the payload: everything gets load/store/swap/CAS, integers additionally count, floats additionally add — a struct payload gets no `fetch_add` because adding two structs means nothing.

## Core Rules

| Rule | Description |
|------|-------------|
| **AT1: Safe operations** | All atomic load/store/swap/CAS/fetch operations are safe — no `unsafe` needed |
| **AT2: Sequentially consistent by default** | An operation takes an optional `order: MemoryOrder`, defaulting to `SeqCst`. A weaker order is opt-in at the call: `counter.fetch_add(1, order: MemoryOrder.Relaxed)` |
| **AT3: Not Copy** | Atomic types are not `Copy` or `Clone` (prevents accidental non-atomic copies) |
| **AT4: Interior mutability** | Operations take `self`, not `mutate self` — the atomic itself handles synchronization |
| **AT5: Wrapping arithmetic** | Fetch operations wrap on overflow. No panic, no undefined behavior |
| **AT6: The order fits the operation** | A load only reads, so it takes `Relaxed`, `Acquire` or `SeqCst`. A store only writes, so it takes `Relaxed`, `Release` or `SeqCst`. A read-modify-write takes any order. A misfit written at the call is a compile error (E0927); one that arrives in a variable panics when the operation runs. Compare-exchange takes one order, and a failure, which only reads, uses its acquire half |
| **AT7: Platform-dependent types** | 128-bit and float atomics require hardware support; code must not compile on unsupported platforms |

## The `Atomic<T>` Type

| Rule | Description |
|------|-------------|
| **GA1: One type, one spelling** | `Atomic<T>` is the only atomic type and the only way to write it. There are no `AtomicU64`-style named types and no aliases — one pattern per operation (`CORE_DESIGN` principle 8) |
| **GA2: Eligibility** | `T` must be Copy, contain no padding bytes, and be 1, 2, 4, or 8 bytes — or 16 with `target.has_atomic128` (AT7). Float payloads additionally require `target.has_atomic_float`. Violation is a compile error at the type, with the reason named |

Rask gives every struct field its own word, so a struct payload is word-sized when it has *one* field: `{ index: i32, gen: i32 }` is 16 bytes however small the fields are written, and E0384 says so. That's a consequence of the layout model rather than of this rule, and it moves if the layout does.
| **GA3: Ops follow the payload** | Every eligible payload gets `new`, `load`, `store`, `swap`, `compare_exchange`, `compare_exchange_weak`, `take`, `get_mut`. Integer payloads add the full fetch family; `bool` adds the logical fetches; floats add `fetch_add`/`fetch_sub`/`fetch_max`/`fetch_min`. Struct payloads get none — `fetch_add` on a struct is meaningless |
| **GA4: CAS is bitwise** | `compare_exchange` compares raw bytes. This is why GA2 excludes padding: two logically equal values with different padding bytes would spuriously fail CAS. Same rule float CAS already follows (`NaN == NaN` when bit patterns match, `+0.0 != -0.0`) |
| **GA5: No optional payloads** | `Atomic<T?>` is rejected. An optional payload needs a bit pattern no valid `T` occupies, and nothing here can promise one. Add your own sentinel field, visible in the struct |

Struct payloads are the point of the generality ([#497](https://github.com/rask-lang/rask/issues/497)). An 8-byte two-field struct is exactly as atomic-eligible as a `u64`, and the compiler does the packing that hand-written shift-and-mask code gets wrong silently:

<!-- test: skip -->
```rask
struct Slot {
    index: u32,
    gen: u32,
}   // 8 bytes, Copy, no padding — fits an atomic word

let current = Atomic<Slot>.new(Slot(index: 0, gen: 0))

let old = current.load()
let next = Slot(index: old.index + 1, gen: old.gen)
match current.compare_exchange(old, next) {
    Slot as _      => {},          // swapped as one unit
    CasFailed as _ => retry(),
}
```

Add a field to `Slot` and it either still fits (nothing to update) or the `Atomic<Slot>` declaration errors — no call site can silently read a garbled value, which is what hand-packing into a bare `u64` gives you.

### Eligible payloads

| Payload | Size | Notes |
|---------|------|-------|
| `bool` | 1 byte | Adds logical fetches (GA3) |
| `i8`–`i64`, `u8`–`u64`, `usize`, `isize` | 1–8 bytes | Full fetch family |
| `f32` / `f64` | 4 / 8 bytes | `fetch_add/sub/max/min`; needs `target.has_atomic_float` (AT7) |
| `i128` / `u128` | 16 bytes | Needs `target.has_atomic128` (AT7) |
| `*T` (raw pointer) | Pointer-size | Load is safe; deref needs `unsafe` |
| Copy struct, no padding | 1–16 bytes | Bitwise CAS only, no fetches (GA3, GA4) |

**Properties:**

| Property | Value |
|----------|-------|
| `Sync` | Yes — safe to share across threads |
| `Send` | Yes — safe to transfer across threads |
| `Copy` / `Clone` | No (AT3) |
| Interior mutability | Yes (AT4) |
| Alignment | Aligned to payload size (e.g. `Atomic<i32>` = 4-byte aligned) |

`Atomic<i64>` / `Atomic<u64>` may be emulated (slower) on 32-bit platforms. All others are native everywhere.

## Memory Orderings

Most code never names an order. The default, `SeqCst`, is the one where every task sees every atomic operation happen in the same single sequence, which is what people assume without thinking about it. The weaker orders are an optimization for lock-free code, and they live in `MemoryOrder` (`import sync.MemoryOrder`), passed by name.

| Order | Description | Use Case |
|----------|-------------|----------|
| `Relaxed` | No synchronization. Only atomicity guaranteed. | Counters, statistics |
| `Acquire` | Later reads and writes can't move before this load. | Lock acquisition |
| `Release` | Earlier reads and writes can't move after this store. | Lock release, publishing data |
| `AcqRel` | Both. | Read-modify-write in a lock |
| `SeqCst` | One total order across all `SeqCst` operations. | The default |

A load has no release half and a store no acquire half, so `Release` on a load or `Acquire` on a store is an error (AT6). Compare-exchange takes one order, and a failed one only reads, so it keeps the acquire half.

**Mental model:** a release store and the acquire load that sees it form a "happens-before" edge. Everything written before the release is visible after the acquire.

<!-- test: parse -->
```rask
// Task A (producer):                               Task B (consumer):
//   data = 42                                        while !ready.load(order: MemoryOrder.Acquire) {}
//   ready.store(true, order: MemoryOrder.Release)    print(data)  // guaranteed to see 42
```

## Operations

### Construction

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `new(v)` | `T -> Atomic<T>` | Create atomic with initial value |
| `default()` | `() -> Atomic<T>` | Create atomic with default value (0, false, null pointer). Primitive payloads only — a struct payload has no compiler-known default, use `new` |

<!-- test: skip -->
```rask
let counter = Atomic<u64>.new(0)
let flag = Atomic<bool>.new(false)
let slot = Atomic<Slot>.new(Slot(index: 0, gen: 0))
```

### Load, Store, Swap

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `load()` | `self -> T` | Atomically read the value |
| `store(v)` | `self, T -> void` | Atomically write the value |
| `swap(v)` | `self, T -> T` | Atomically replace, return old value |

Every operation here and below also takes `order: MemoryOrder = MemoryOrder.SeqCst` last (AT2). The tables leave it out.

`store` takes `self` (not `mutate self`) because atomics use interior mutability (AT4).

<!-- test: skip -->
```rask
let value = counter.load()
counter.store(100)
let old = counter.swap(new_value)

// opting in to a weaker order
let hits = counter.load(order: MemoryOrder.Relaxed)
```

### Compare-and-Exchange

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `compare_exchange(current, new)` | `self, T, T -> T or CasFailed<T>` | If value == current, set to new. Returns old on success, `CasFailed(actual)` on failure |
| `compare_exchange_weak(current, new)` | Same | May spuriously fail. Use in loops |

- `compare_exchange`: Must succeed if value matches. Use for single-attempt operations.
- `compare_exchange_weak`: May fail spuriously even if value matches. More efficient in loops on some architectures.

`CasFailed<T>` is one field — the value the atomic actually held — so it is the same word and costs nothing at run time. The wrapper is what separates the branches: both sides carry a `T`, and as `T or T` no pattern could tell success from failure. A branch written without its type argument (`CasFailed as e`) names the whole branch, so `e.found` reads.

<!-- test: skip -->
```rask
loop {
    let current = counter.load()
    if current >= threshold {
        break
    }
    match counter.compare_exchange_weak(current, current + 1) {
        u64 as _ => break,
        CasFailed as _ => continue,
    }
}
```

### Fetch Operations (integer payloads)

Per GA3, the fetch family exists where the payload can do arithmetic. All fetch operations return the OLD value (AT5: wrapping on overflow).

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `fetch_add(v)` | `self, T -> T` | Add |
| `fetch_sub(v)` | `self, T -> T` | Subtract |
| `fetch_and(v)` | `self, T -> T` | Bitwise AND |
| `fetch_or(v)` | `self, T -> T` | Bitwise OR |
| `fetch_xor(v)` | `self, T -> T` | Bitwise XOR |
| `fetch_nand(v)` | `self, T -> T` | Bitwise NAND |
| `fetch_max(v)` | `self, T -> T` | Max |
| `fetch_min(v)` | `self, T -> T` | Min |

`Atomic<bool>` supports `fetch_and`, `fetch_or`, `fetch_xor`, `fetch_nand` with `bool` operands. Float payloads get `fetch_add`, `fetch_sub`, `fetch_max`, `fetch_min` (see the float section below). Struct payloads get no fetch operations — read-modify-write on a struct is a CAS loop, where the modify step is ordinary visible code.

### Pointer Payloads

`Atomic<*T>` stores a raw pointer `*T`. Supports `new`, `load`, `store`, `swap`, `compare_exchange`, `compare_exchange_weak`.

Dereferencing the loaded pointer requires `unsafe` (AT1 applies to the atomic operation itself, not the pointer):

<!-- test: skip -->
```rask
let ptr = atomic_ptr.load()  // Safe: just a pointer value
unsafe {
    let value = *ptr  // Unsafe: dereferencing raw pointer
}
```

### Non-Atomic Access

| Operation | Signature | Description |
|-----------|-----------|-------------|
| `get_mut()` | `self -> *T` | Get raw pointer to inner value (unsafe to dereference) |
| `take()` | `take self -> T` | Consume atomic, return inner value |

`take` is safe because `take self` guarantees exclusive ownership.

<!-- test: skip -->
```rask
mut counter = Atomic<u64>.new(0)
let final_value = counter.take()
```

## Memory Fences

Fences enforce ordering without an atomic variable.

| Operation | Description |
|-----------|-------------|
| `fence(MemoryOrder.Acquire)` | All subsequent reads/writes cannot be reordered before this fence |
| `fence(MemoryOrder.Release)` | All previous reads/writes cannot be reordered after this fence |
| `fence(MemoryOrder.AcqRel)` | Both Acquire and Release |
| `fence(MemoryOrder.SeqCst)` | Full memory barrier |
| `compiler_fence(order)` | Prevents compiler reordering only (no CPU barrier) |

`compiler_fence` is for signal handlers, memory-mapped I/O, or when hardware provides ordering guarantees.

<!-- test: skip -->
```rask
data = 42
fence(MemoryOrder.Release)
ready.store(true, order: MemoryOrder.Relaxed)  // Relaxed is enough after the fence
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
    static TAGGED_PTR: Atomic<u128> = Atomic<u128>.new(0)
} else {
    static TAGGED_PTR: Shared<u128, Mutex> = Shared.mutex(0)
}
```

### Atomic<u128> / Atomic<i128>

Must be 16-byte aligned (unaligned access is UB on x86-64 `CMPXCHG16B`). Same operations as integer atomics.

| Platform | Implementation |
|----------|----------------|
| x86-64 | `CMPXCHG16B` (requires `cx16`, standard since ~2008) |
| ARM64 | `LDXP`/`STXP` or `CASP` (ARMv8.1+) |
| Others | Compile error |

### Atomic<f32> / Atomic<f64>

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
ERROR [mem.atomics/AT6]: a `store` can't be `Acquire`
   |
8  |  ready.store(true, order: MemoryOrder.Acquire)
   |                           ^^^^^^^^^^^^^^^^^^^ a store only writes, so this order has no acquire half to apply

FIX: `order: MemoryOrder.Release` if you meant to synchronize, or leave the order out
```

```
ERROR [mem.atomics/AT7]: Atomic<u128> not available on this platform
   |
3  |  static COUNTER: Atomic<u128> = Atomic<u128>.new(0)
   |                  ^^^^^^^^^^ requires native 128-bit atomic support

WHY: Lock-based emulation would hide a 10x cost, violating transparency.

FIX: Use comptime if target.has_atomic128 { ... } to provide both paths.
```

**Padding in the payload [GA2]:**
```
ERROR [mem.atomics/GA2]: Tagged has padding — cannot be an atomic payload
   |
4  |  let state: Atomic<Tagged> = Atomic.new(initial)
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
9  |  current.fetch_add(delta)
   |          ^^^^^^^^^ Slot is a struct — arithmetic on it has no meaning

FIX: read-modify-write with a CAS loop; the modify step is ordinary code:

  loop {
      let old = current.load()
      let next = Slot { index: old.index + delta, gen: old.gen }
      if current.compare_exchange_weak(old, next)? { break }
  }
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `Release` or `AcqRel` on a load | AT6 | Compile error; a panic if the order came in a variable |
| `Acquire` or `AcqRel` on a store | AT6 | Compile error; a panic if the order came in a variable |
| Mixing atomic and non-atomic access to same location | — | Undefined behavior |
| Overflow on `fetch_add` | AT5 | Wraps (no panic) |
| `Atomic<*T>` load then deref | AT1 | Load is safe; deref requires `unsafe` |
| `take` on shared atomic | AT3 | Requires `take self` — exclusive ownership |
| Atomics at comptime | — | Not available (no meaningful semantics without threads) |
| Atomic statics | AT1 | Safe to access from multiple threads without `unsafe` |
| Struct payload with padding bytes | GA2 | Compile error — reorder fields or pad explicitly |
| Struct payload > 8 bytes without `target.has_atomic128` | GA2/AT7 | Compile error — same gate as `Atomic<u128>` |
| `fetch_add` on a struct payload | GA3 | Compile error — use a CAS loop |
| `Atomic<T?>` | GA5 | Compile error — no spare bit pattern for `none`; add your own sentinel field |
| `default()` on a struct payload | GA3 | Compile error — no compiler-known default, use `new` |
| `Atomic<Link<T>?>` | GA5 | Compile error — see the note below on why a link isn't the exception a handle was |

---

## Appendix (non-normative)

### Rationale

**AT1 (safe operations):** Atomic operations can't cause data races — the hardware guarantees atomicity. The type system prevents mixing atomic and non-atomic access. Logical errors (ABA, incorrect ordering) are possible but don't violate memory safety.

**AT2 (sequentially consistent by default):** This used to require an order on every call, on the theory that it kept the cost visible. It didn't: it made every reader of a counter learn the C11 memory model to write `fetch_add`, and the safe choice was the one that took the most typing. The order is a low-level optimization, so it's opt-in. Code that never names one gets the strongest, and the call that does names it with a label, so a weakened order stands out in review ([#1684](https://github.com/rask-lang/rask/issues/1684)).

**AT6 (the order fits):** A failed CAS only reads, so its order follows from the one given, and the second order argument went. A load or store given an order it has no half for is different: the program asked for a guarantee the operation can't give. Running it as `Relaxed` would hide that, so it's an error. C11 leaves both undefined.

**AT7 (platform-dependent):** Lock-based emulation of 128-bit atomics is 10x slower than native support. Hiding this cost would violate transparency. Compile-time detection lets library authors provide both paths.

**GA1 (one generic type, one spelling):** This resolved [#497](https://github.com/rask-lang/rask/issues/497). A fixed menu of named atomics left word-sized user structs with hand-packing — shift-and-mask code where a wrong shift produces a plausible wrong value instead of a type error, and where adding a field breaks nothing visibly. The eligibility check is a static predicate the compiler already answers elsewhere (Copy? fits a word? padding-free?), so withholding it wasn't a design position, just a gap.

The named types (`AtomicU64`, `AtomicBool`, …) were deleted outright, not kept as aliases. A first draft kept them for familiarity, and that was the wrong instinct: two spellings for the same type means every new reader has to discover they're the same — `AtomicU64` in one file and `Atomic<u64>` in another *look* like different types, and nothing on the page says otherwise. One spelling per operation is already the language's own rule (`CORE_DESIGN` principle 8); atomics don't get a Rust-familiarity exemption. `Atomic<u64>` costs three characters over `AtomicU64` and removes a whole table from the mental model.

The cost of the generic surface — operation families that vary by payload — lands entirely inside the compiler, which already special-cases every type in that carve-out. That's the right side of the line: complicated implementation behind a simple surface, the same trade `string`'s refcount elision makes.

`Atomic<T>` over a user payload does not open that set (`mem.shared-rack-heap/BX1`–`BX4`): the payload is plain Copy data, and `Atomic` itself stays compiler-provided. Nothing here lets a user type run code at assignment, scope exit, or borrow boundaries — the same relationship `Shared<T>` has to its `T`.

**GA3 (no fetch ops on structs):** `fetch_add` exists because hardware has it for integers. For a struct, "add" has no single meaning, and inventing one (field-wise? user-defined?) would hide a CAS loop behind an innocent-looking method. The CAS loop is the honest spelling: the modify step is visible code between a `load` and a `compare_exchange`.

**GA5 (no optional payloads at all).** There used to be exactly one exception: `Atomic<Handle<T>?>`, with four rules of its own (AH1–AH4) for packing the handle's three fields into a word and reserving all-ones for `none`. It worked because the compiler owned the handle's layout, and because a handle carried a generation — which gave lock-free code ABA protection for free, since a recycled slot got a different generation and a CAS on the stale handle correctly failed. Handles are gone (rask-lang/rask#908) and that section went with them.

`Atomic<Link<T>?>` is *not* the replacement, even though a link is one word and null is a perfectly good `none`. Two things it can't do that a handle could. It carries no generation, so ABA comes back: an address freed and handed out again compares equal to the one you loaded. And `mem.racks/RK3` — delete nulls every edge pointing at the node — works by finding the node's holders in the rack's edge index, which an atomic word another thread is writing is not. A link published through an atomic could outlive its node with nothing to catch it. That's the same gap `mem.racks` already lists as deferred under "structural mutation under concurrency"; until it has an answer, this stays closed.

Users who need an "empty" state add their own sentinel field — visible in the struct definition, checked by their own code.

**C interop:** Atomic types are ABI-compatible with C11 `_Atomic` types and C++ `std::atomic`.

### Patterns & Guidance

**Ordering selection:**

| Scenario | Recommended Ordering |
|----------|---------------------|
| Anything you're unsure about | Leave it out (`SeqCst`) |
| Simple counter (stats, metrics) | `Relaxed` |
| Flag to signal "data ready" | Writer: `Release`, Reader: `Acquire` |
| Spin lock acquire | `Acquire` on successful CAS |
| Spin lock release | `Release` store |
| Reference count increment | `Relaxed` |
| Reference count decrement (checking for zero) | `AcqRel` |
| Publishing a value (writer) | `Release` store/swap |
| Consuming a published value (reader) | `Acquire` load |
| CAS in a lock-free op | `AcqRel` |

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
static REQUESTS: Atomic<u64> = Atomic<u64>.new(0)

func handle_request(req: Request) {
    REQUESTS.fetch_add(1)
    // ... process request
}

func get_stats() -> u64 {
    return REQUESTS.load()
}
```

**Flag for signaling:**

<!-- test: skip -->
```rask
static SHUTDOWN: Atomic<bool> = Atomic<bool>.new(false)

func worker_loop() {
    while !SHUTDOWN.load() {
        do_work()
    }
}

func request_shutdown() {
    SHUTDOWN.store(true)
}
```

**Bounded counter (CAS loop):**

<!-- test: skip -->
```rask
func increment_if_below(counter: Atomic<u64>, max: u64) -> bool {
    loop {
        let current = counter.load()
        if current >= max {
            return false
        }
        match counter.compare_exchange_weak(current, current + 1) {
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
    count: Atomic<usize>,
    value: T,
}

func arc_clone<T>(ptr: *ArcInner<T>) -> *ArcInner<T> {
    unsafe {
        (*ptr).count.fetch_add(1, order: MemoryOrder.Relaxed)
    }
    return ptr
}

func arc_drop<T>(ptr: *ArcInner<T>) {
    unsafe {
        if (*ptr).count.fetch_sub(1, order: MemoryOrder.AcqRel) == 1 {
            fence(MemoryOrder.Acquire)
            dealloc(ptr)
        }
    }
}
```

**Spin lock (sketch):**

<!-- test: skip -->
```rask
struct SpinLockInner<T> {
    locked: Atomic<bool>,
    data: T,
}

func spin_acquire<T>(lock: *SpinLockInner<T>) {
    unsafe {
        while (*lock).locked.compare_exchange_weak(
            false, true, order: MemoryOrder.Acquire
        ) is CasFailed {
            while (*lock).locked.load(order: MemoryOrder.Relaxed) {
                spin_hint()
            }
        }
    }
}

func spin_release<T>(lock: *SpinLockInner<T>) {
    unsafe {
        (*lock).locked.store(false, order: MemoryOrder.Release)
    }
}
```

These patterns use raw pointers and unsafe blocks. The stdlib provides a safe wrapper (`Shared<T, Mutex>`) that encapsulates the unsafe implementation.

**Lock-free containers.** The stdlib is where these live: a correct one needs deferred reclamation and an ABA answer, and neither is something a sketch in a spec can show honestly. `Atomic<*T>` is the primitive they are built on, inside `unsafe`.

The sketch that used to sit here built one out of `Atomic<Handle<T>?>` and got its ABA protection from the handle's generation counter. It went with the handles (rask-lang/rask#908); see the GA5 note above for why a link doesn't take the place.

### See Also

- [Synchronization Primitives](../concurrency/sync.md) — `Shared<T, S>` for compound data (`conc.sync`)
- [Shared, Rack and Heap](shared-rack-heap.md) — Why atomics sit adjacent to that set (`mem.shared-rack-heap`)
- [Concurrency](../concurrency/async.md) — Channels and task spawning (`conc.async`)
- [Unsafe](unsafe.md) — Raw pointer dereferencing for `Atomic<*T>` results (`mem.unsafe`)
- [Racks and Links](racks.md) — nodes with stable identity, and why a link may not be an atomic payload (`mem.racks`)
- [Ownership](ownership.md) — Atomic values are owned, not reference-typed (`mem.ownership`)
