<!-- id: conc.phase-b -->
<!-- status: proposed -->
<!-- summary: Phase B compiler transforms — vtable ABI, closure state machines, separate compilation, FFI boundaries -->
<!-- depends: concurrency/runtime-strategy.md, concurrency/io-context.md, compiler/hidden-params.md, compiler/memory-layout.md, compiler/effects.md, types/traits.md -->

# Phase B Compiler Transforms

Phase B upgrades the runtime from OS threads to M:N green tasks (`conc.strategy/B1-B4`). The programmer-facing API doesn't change. What changes is how the compiler handles indirect calls, state machine generation, cross-module boundaries, and foreign code.

Four problems that the happy-path specs don't address:

1. **Trait objects:** Indirect calls can resolve to implementations that may park tasks
2. **Function pointers/closures:** Same problem — indirect dispatch hides the callee
3. **Separate compilation:** Cross-module pause-point detection
4. **FFI:** Foreign code can't participate in cooperative scheduling

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

### What doesn't become a state machine

Only `spawn(|| { ... })` closures are transformed into state machines (`conc.runtime/T3`). Inner closures within a spawn closure — iterator callbacks, stored callbacks, event handlers — are captured data in the state machine, not transformed themselves.

<!-- test: skip -->
```rask
spawn(|| {
    // This spawn closure → state machine
    const data = try File.read("input.txt")  // yield point

    // This inner closure is NOT a state machine — it's captured data
    const items = data.lines().filter(|line| line.starts_with("#"))

    for item in items {
        try File.write("out.txt", item)      // yield point
    }
})
```

State machine variants correspond to yield points in the spawn closure's control flow. The `.filter(|line| ...)` closure is just a value held in a state machine variant.

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

**VT1 (clean vtable entries):** The process-global runtime slot (`conc.runtime`) removes the need to thread runtime state through vtables. Earlier drafts considered wide vtable entries that carried `__ctx: RuntimeContext?` — that's obsolete now. Trait signatures match vtable ABI exactly; implementations that need the runtime read the slot themselves.

**VT3 (trait object calls as pause points):** This means "dead" state machine variants for in-memory trait implementations. I think that's acceptable. The alternative — tracking which concrete types are behind a trait object — requires whole-program devirtualization, which is an optimization, not something the correctness of state machine generation should depend on. Dead variants poll as Ready, the scheduler never parks, no observable cost beyond a few bytes in the enum.

**FP1 (clean ABI for storable closures):** Consistency with VT1. Indirect calls — vtable dispatch, function pointers, storable closures — all use the same convention: exactly the declared signature. No hidden parameters.

**FFI3 (compile-time warning):** The effects system already marks extern functions as conservatively IO (`comp.effects/INF5`). Detecting "extern call in async context" is a subset of the existing IO-in-ThreadPool warning (`comp.effects/CW1`). Same infrastructure, same suppressibility. I chose a warning over an error because fast FFI calls (crypto, compression, math) are common and harmless. The `@allow` annotation makes the suppression visible and auditable.

**FFI4 (runtime compensation):** Go does this for cgo and it works well in practice. The 1ms threshold avoids thread churn for fast FFI while catching blocking I/O. I thought about making the threshold configurable but decided against it — 1ms is a good default, and tuning knobs invite premature optimization. If profiling shows a different threshold is better, it can be changed in a point release without API changes.

### Alternatives Considered

**Per-trait vtable specialization:** Generate wide vtable entries only for traits whose methods could plausibly do I/O (traits with `[]u8` buffer parameters, traits returning `or IoError`, etc.). Rejected: heuristic-based, fragile, and a custom `trait Processor { func process(self) }` could do I/O internally. The heuristic would need constant updating.

**Thread-local runtime instead of a process-global slot:** Thread-local storage breaks on green-task migration between worker threads — a task that reads TLS on worker A and migrates to worker B would see worker B's TLS, not its original runtime. Process-global works because there's exactly one runtime per process anyway (C1).

**Stackful coroutines instead of state machines:** Allocate a small stack per green task (like Go's goroutines). Avoids the state machine transform entirely — function calls just work, including through trait objects. Rejected because: (a) stack overflow detection is complex, (b) stack size tuning is a footgun (Go's goroutines start at 8KB, grow to 1MB — segmented stacks have real overhead), (c) state machines have predictable memory cost (sum of live variables at each yield point). I think the state machine approach is more Rask — costs are transparent and mechanical.

### See Also

- `conc.strategy` — Phase A/B implementation strategy
- `conc.runtime/T1-T3` — Task structure and state machine transform
- `conc.io-context/IO7-IO9` — Trait signature / context threading orthogonality
- `comp.hidden-params` — Hidden parameter compiler pass
- `comp.effects` — Effect tracking (IO/Async/Mutation metadata)
- `compiler.layout/V1-V5` — Vtable memory layout
- `type.traits/TR12-TR13` — Vtable dispatch, fat pointer structure
- `mem.closures` — Closure capture rules, storable vs immediate
