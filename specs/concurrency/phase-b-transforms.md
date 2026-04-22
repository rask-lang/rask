<!-- id: conc.phase-b -->
<!-- status: proposed -->
<!-- summary: Phase B compiler transforms — vtable ABI, closure state machines, separate compilation, FFI boundaries -->
<!-- depends: concurrency/runtime-strategy.md, concurrency/io-context.md, compiler/hidden-params.md, compiler/memory-layout.md, compiler/effects.md, types/traits.md -->

# Phase B Compiler Transforms

Phase B upgrades the runtime from OS threads to M:N stackful fibers (`conc.strategy/B1-B4`, `conc.runtime`). The programmer-facing API doesn't change, and — because Rask uses stackful fibers instead of stackless state machines — the codegen story is almost unchanged too. Function bodies, vtables, function pointers, and closures compile exactly as in sync code. Parking happens via context switches performed by stdlib I/O functions, not via state-machine enums built at compile time.

What Phase B actually needs from the compiler:

1. **Cross-module "reaches spawn" metadata** for the `conc.async/CC2` scope check
2. **FFI boundaries** where foreign code can't participate in fiber scheduling
3. **Preemption safe-point instrumentation** in function prologues (see `conc.runtime/P3`)

There are NO state-machine transforms, NO wide ABIs, NO pause-point enumeration at compile time. The stackful model pushes all of this into the runtime.

## Vtable ABI

| Rule | Description |
|------|-------------|
| **VT1: Clean vtable entries** | Vtable method entries have exactly the trait's declared signature. No hidden runtime parameter |
| **VT2: Implementations read the slot** | Any implementation that performs async-capable I/O reads the process-global runtime slot at call time |
| **VT3: Trait object calls are potential pause points** | Inside spawn closures, trait object method calls to implementations that may read the runtime slot generate state machine yield variants |

The runtime lives in a process-global slot (`conc.runtime`), so vtable ABI never needs to carry it. Trait signatures and vtable layouts stay clean.

<!-- test: skip -->
```rask
trait Reader {
    func read(self, buf: []u8) -> usize or IoError
}

// Vtable entry at ABI level:
// fn(data: *u8, buf: []u8) -> usize or IoError
```

### Vtable layout

Unchanged from `compiler.layout/V1-V5`:

```
// Reader vtable for File (does I/O)
File_Reader_vtable:
  [0]  size: 32
  [8]  align: 8
  [16] drop: &File_drop
  [24] read: &File_read           // reads RUNTIME_SLOT internally if async-capable

// Reader vtable for Buffer (in-memory)
Buffer_Reader_vtable:
  [0]  size: 48
  [8]  align: 8
  [16] drop: null
  [24] read: &Buffer_read         // pure memory copy, ignores slot
```

Both vtable entries have identical signatures. Call site doesn't need to know which implementation is behind the pointer.

### Cost

Zero ABI overhead vs. sync code. Implementations that need the runtime pay a single `RUNTIME_SLOT.read()` on entry; non-async implementations pay nothing.

### State machine impact

Inside a spawn closure, `reader.read(buf)` through `any Reader` becomes a yield point:

<!-- test: skip -->
```rask
spawn(|| {
    const reader: any Reader = get_reader()
    const n = try reader.read(buf)  // yield point — concrete type unknown
    process(buf[..n])
})
```

State machine:

```
enum State {
    Start { reader, buf },
    AwaitingRead { reader, buf, io_future },
    Complete,
}
```

For in-memory implementations (Buffer), `io_future.poll()` returns Ready immediately — the scheduler never parks the task. The `AwaitingRead` variant is "dead" but harmless. Cost: a few bytes in the state machine enum per trait object call in the closure.

## Function Pointers and Closures

| Rule | Description |
|------|-------------|
| **FP1: Clean function-pointer ABI** | Storable closure and function-pointer types carry only their declared parameters. No hidden runtime parameter |
| **FP2: Runtime discovery is internal** | Any closure that performs async-capable I/O reads `RUNTIME_SLOT` at execution time, no caller cooperation needed |
| **FP3: Indirect calls are potential pause points** | Inside spawn closures, calls through function pointers or storable closures generate state machine yield variants (if the callee might read the slot) |

The function pointer type `Func([]u8) -> usize or IoError` has the ABI signature:

```
fn(env: *u8, buf: []u8) -> usize or IoError
```

Matches vtable entries (VT1). All indirect calls use the same convention.

### No special handling for spawn closures

The spawn closure body is compiled exactly like any other function. It runs on the fiber's stack. I/O calls inside park the fiber via context switch (handled by the I/O stdlib) and resume when ready. Inner closures (iterator callbacks, event handlers) are ordinary closures, stored as values like anywhere else.

<!-- test: skip -->
```rask
spawn(|| {
    const data = try File.read("input.txt")   // parks fiber if reactor says EAGAIN

    const items = data.lines().filter(|line| line.starts_with("#"))

    for item in items {
        try File.write("out.txt", item)        // parks fiber on backpressure
    }
})
```

Parking is a runtime operation (`fiber_switch`), not a compile-time transform. The compiler does not need to know which call sites might park.

## Separate Compilation

| Rule | Description |
|------|-------------|
| **SC1: Module metadata includes "reaches spawn" bit** | Compiled module metadata stores whether each public function transitively reaches `spawn` (used by `conc.async/CC2` scope check at cross-module call sites) |
| **SC2: Effect bits determine pause-point status** | The 3-bit effect mask from `comp.effects/INF3` (IO \| Async \| Mutation) stored per function. IO or Async → potential pause point |
| **SC3: Public API stability** | Public function signatures carry no runtime-context annotation. Adding a `spawn` call deep in a private helper can propagate the "reaches spawn" bit upward; this is observable to callers only as a CC2 scope requirement |

No `__ctx` parameter exists. Stdlib functions read the process-global runtime slot directly at execution time (see `conc.runtime`). Module metadata stores effect bits for codegen (state-machine transforms) and the "reaches spawn" bit for the `conc.async/CC2` scope check across crate boundaries.

**SC4: Generic functions store conservative effects.** A generic public function `func process<T: Handler>(h: T)` might have different effects depending on `T`. Module metadata stores the union of effects across the generic body plus a "may vary by type parameter" flag. Post-monomorphization, effects are precise per instantiation (`comp.effects` edge case: "Effects inferred per monomorphized instance"). The state machine transform uses the conservative metadata for cross-module generics, precise data for local monomorphizations.

### Compilation flow

```
Module B (compiled first):
  public func process_file(path: string) → metadata: { reaches_spawn: no,  effects: IO }
  public func run_server()               → metadata: { reaches_spawn: yes, effects: IO | Async }
  public func parse_header(raw: string)  → metadata: { reaches_spawn: no,  effects: pure }

Module A (compiled second, imports B):
  using Multitasking {
      process_file("x.txt")    // CC2 not applicable; state-machine sees IO bit → yield point
      run_server()             // CC2 scope check satisfied by enclosing block
  }

  run_server()                 // CC2 error: transitively reaches spawn, no block in scope
```

The state machine transform in module A checks effect bits from module B's metadata. `process_file` has IO → yield point generated. `parse_header` is pure → no yield point. The `reaches_spawn` bit drives the CC2 compile-time scope check; it does not affect codegen.

## FFI Boundary

| Rule | Description |
|------|-------------|
| **FFI1: Extern functions cannot pause tasks** | Foreign functions don't read the process-global runtime slot. C code can't participate in cooperative scheduling |
| **FFI2: Blocking is accepted** | FFI calls that block I/O block the worker thread. Scheduler runs remaining tasks on N-1 workers |
| **FFI3: Compile-time warning** | Extern function calls inside `using Multitasking` context trigger a suppressible warning |
| **FFI4: Runtime worker compensation** | If a worker thread is blocked in FFI for >1ms, the scheduler spawns a temporary replacement worker |
| **FFI5: Convention for long-blocking FFI** | FFI calls expected to block significantly should use `ThreadPool.spawn` |

### Compile-time warning (FFI3)

The effects system (`comp.effects/INF5`) already tags extern functions as conservatively IO. When an extern call appears inside `using Multitasking` scope:

```
WARNING [conc.phase-b/FFI3]: extern call in async context may block worker thread
   |
5  |  const result = sqlite_query(db, sql)
   |                 ^^^^^^^^^^^^ extern function — blocks OS thread
   |
WHY: Foreign functions can't park green tasks. Blocking I/O in FFI
     blocks a scheduler worker thread.

FIX: Wrap in ThreadPool.spawn for blocking FFI:

  const result = try ThreadPool.spawn(|| { sqlite_query(db, sql) }).join()
```

Suppress with `@allow(ffi_in_async)` for fast FFI (crypto primitives, math libraries):

<!-- test: skip -->
```rask
@allow(ffi_in_async)
const hash = crypto_sha256(data)  // extern, returns in µs
```

### Runtime worker compensation (FFI4)

Safety net for FFI calls that block unexpectedly or when warnings are suppressed.

```
FFI call flow:

1. Worker thread signals "entering FFI" to scheduler
2. Worker calls extern function (blocks)
3. Scheduler timer fires after 1ms
   → If worker still blocked: spawn temporary replacement worker
   → If worker returned: no action
4. Worker returns from FFI, resumes normal scheduling
5. Temporary worker idles out when no more tasks available
```

Go's runtime does exactly this for cgo calls. Cost: ~100µs for the temporary thread spawn, only when FFI blocks >1ms. Calls that return quickly (<1ms) have zero overhead — the timer never fires.

### Patterns

<!-- test: skip -->
```rask
using Multitasking, ThreadPool {
    spawn(|| {
        // Good: long-blocking FFI on thread pool
        const rows = try ThreadPool.spawn(|| {
            sqlite_query(db, "SELECT * FROM users")
        }).join()

        process(rows)

        // Acceptable: fast FFI inline (suppress warning)
        @allow(ffi_in_async)
        const checksum = crc32(data)
    }).detach()
}
```

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Trait with no I/O methods (e.g., `Display`) | VT1 | Clean vtable, implementations never touch RUNTIME_SLOT, zero cost |
| `any Reader` in non-async context | VT2 | Implementation reads RUNTIME_SLOT, finds `None`, takes blocking path |
| Pure closure stored in variable, called in spawn | FP1, FP3 | Clean ABI, yield point generated conservatively. Poll returns Ready immediately |
| Cross-module function gains `spawn` internally | SC3 | Caller sees a new CC2 scope requirement. Source-level breakage, same as any API change |
| Cross-module generic with type-dependent effects | SC4 | Conservative metadata (union of effects). Precise after monomorphization |
| FFI call returns in <1ms | FFI4 | No compensation thread, zero overhead |
| Many concurrent blocking FFI calls | FFI4 | Scheduler grows temporarily. Bounded by OS thread limits |
| FFI callback into Rask code | FFI1 | Runs on FFI's OS thread. If no `using Multitasking` block is active, `spawn` in the callback is a CC3 runtime panic |
| `compile_rust()` interop (`struct.build`) | FFI1 | Same rules as C FFI |
| Nested trait object call (e.g., `io.copy(any Reader, any Writer)`) | VT3 | Two potential yield points per loop iteration. Two state machine variants. Acceptable — this is the I/O copy hot path |

## Error Messages

```
WARNING [conc.phase-b/FFI3]: extern call in async context may block worker thread
   |
5  |  const result = ffi_compute(data)
   |                 ^^^^^^^^^^^ extern function — blocks OS thread
   |
WHY: Foreign functions can't park green tasks.

FIX: Wrap in ThreadPool.spawn for blocking FFI:

  const result = try ThreadPool.spawn(|| { ffi_compute(data) }).join()
```

---

## Appendix (non-normative)

### Rationale

**Clean vtable and fn-pointer ABIs (VT1, FP1):** With stackful fibers, runtime discovery happens inside the callee (via `RUNTIME_SLOT`) rather than through a parameter threaded by the caller. Indirect calls therefore don't need wide ABIs. Trait signatures match their vtable entries exactly.

**FFI warnings (FFI3):** The effects system already marks extern functions as conservatively IO (`comp.effects/INF5`). Detecting "extern call in async context" is a subset of the existing IO-in-ThreadPool warning (`comp.effects/CW1`). Same infrastructure, same suppressibility. Warning rather than error because fast FFI calls (crypto, compression, math) are common and harmless. `@allow(ffi_in_async)` makes suppression visible and auditable.

**FFI worker compensation (FFI4):** Go does this for cgo and it works well in practice. The 1 ms threshold avoids thread churn for fast FFI while catching blocking I/O.

### Alternatives Considered

**Stackless state machines (previously speced):** Transform every `spawn` closure into a state-machine enum; treat every I/O call and indirect call as a potential yield variant. Cheaper memory per task (~120 bytes vs ~1 MiB virtual), but:
- Forces a wide ABI (`__ctx` on every vtable entry, every fn pointer)
- Requires cross-crate pause-point detection via metadata bits
- Makes "reaches spawn" observable to callers via signature-level coloring pressure
- Violates Principle 5 indirectly by forcing library signatures to carry runtime plumbing

Rejected in favor of stackful fibers. See `conc.runtime` §Design Rationale.

**Thread-local runtime instead of process-global slot:** Thread-local storage breaks when fibers migrate between workers — a fiber that reads TLS on worker A and gets stolen to worker B would see B's TLS, not its original runtime. Process-global works because there's exactly one runtime per process by design (`conc.async/C1`).

**Go-style copying stacks:** Start small (2 KiB), copy to a larger stack on growth, rewrite pointers. Requires GC to find pointers-into-stack during copy. Rask has no GC (ownership-based memory), so copying isn't viable. Loom-style virtual-reservation stacks avoid the issue entirely.

**Per-trait vtable specialization:** Generate different vtable shapes for "pure" vs "potentially pausing" traits. Rejected as heuristic-based and fragile. With stackful fibers, the ABI is uniform anyway.

### See Also

- `conc.strategy` — Phase A/B implementation strategy
- `conc.runtime` — Task structure, pluggable reactor, preemption, process-global slot
- `conc.io-context` — Runtime discovery via process-global slot
- `comp.hidden-params` — Hidden parameter compiler pass
- `comp.effects` — Effect tracking (IO/Async/Mutation metadata)
- `compiler.layout/V1-V5` — Vtable memory layout
- `type.traits/TR12-TR13` — Vtable dispatch, fat pointer structure
- `mem.closures` — Closure capture rules, storable vs immediate
