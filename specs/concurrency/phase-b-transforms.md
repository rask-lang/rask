<!-- id: conc.phase-b -->
<!-- status: proposed -->
<!-- summary: Phase B compiler transforms — vtable ABI, closure state machines, separate compilation, FFI boundaries -->
<!-- depends: concurrency/runtime-strategy.md, concurrency/io-context.md, compiler/hidden-params.md, compiler/memory-layout.md, compiler/effects.md, types/traits.md -->

# Phase B Compiler Transforms

Phase B upgrades the runtime from OS threads to M:N green tasks (`conc.strategy/B1-B4`). The programmer-facing API doesn't change. What changes is how the compiler handles indirect calls, state machine generation, cross-module boundaries, and foreign code.

Four problems that the happy-path specs don't address:

1. **Trait objects:** Vtable function pointers have the wrong arity for `__ctx` threading
2. **Function pointers/closures:** Indirect calls can't be statically analyzed for pause points
3. **Separate compilation:** Cross-module pause-point detection
4. **FFI:** Foreign code can't participate in cooperative scheduling

## Vtable ABI

| Rule | Description |
|------|-------------|
| **VT1: Wide vtable entries** | All vtable method entries include `__ctx: RuntimeContext?` as the final parameter, regardless of whether the trait or implementation uses I/O |
| **VT2: Implementations compile to wide signature** | Vtable entry functions for all trait implementations accept `__ctx` — implementations that don't need it ignore the parameter |
| **VT3: Call sites pass __ctx when available** | Trait object method calls pass `__ctx` if in async context, `None` if in sync context |
| **VT4: Trait object calls are potential pause points** | Inside spawn closures, every trait object method call generates a state machine yield variant |

The trait signature stays clean (`conc.io-context/IO7`). The wide ABI is a codegen detail — hidden from programmers, visible in vtables.

<!-- test: skip -->
```rask
// What the programmer writes — no __ctx anywhere
trait Reader {
    func read(self, buf: []u8) -> usize or IoError
}

// What the vtable entry looks like at ABI level:
// fn(data: *u8, buf: []u8, __ctx: RuntimeContext?) -> usize or IoError
//                          ^^^^^^^^^^^^^^^^^^^^^^^^ added by compiler
```

### Vtable layout change

The vtable layout from `compiler.layout/V1-V5` extends naturally. Each method slot points to a function with the wide signature:

```
// Reader vtable for File (does I/O)
File_Reader_vtable:
  [0]  size: 32
  [8]  align: 8
  [16] drop: &File_drop
  [24] read: &File_read_wide     // uses __ctx for non-blocking I/O

// Reader vtable for Buffer (in-memory)
Buffer_Reader_vtable:
  [0]  size: 48
  [8]  align: 8
  [16] drop: null
  [24] read: &Buffer_read_wide   // ignores __ctx, pure memory copy
```

Both vtable entries have identical function signatures. Call site doesn't need to know which implementation is behind the pointer.

### Cost

One extra register argument per trait object method call. On x86-64, `__ctx` occupies one register slot (typically RCX or R8 depending on position). For `None`, the register is zeroed.

- Memory: zero (register, not stack)
- CPU: negligible (one register write per call)
- Vtable size: unchanged (same number of entries, same pointer size)

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
| **FP1: Storable closures use wide ABI** | All storable closures and function pointer types include `__ctx: RuntimeContext?` as a hidden final parameter |
| **FP2: Immediate closures use narrow ABI** | Inline closures consumed at their creation site (`mem.closures/IO1`) are monomorphized — the compiler handles `__ctx` threading statically |
| **FP3: Indirect calls are potential pause points** | Inside spawn closures, calls through function pointers or storable closures generate state machine yield variants |

The function pointer type `Func([]u8) -> usize or IoError` has the ABI signature:

```
fn(env: *u8, buf: []u8, __ctx: RuntimeContext?) -> usize or IoError
```

This matches vtable entries (VT1). All indirect calls use the same wide convention.

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
| **SC1: Module metadata includes __ctx acceptance** | Compiled module metadata stores whether each public function accepts `__ctx: RuntimeContext?` |
| **SC2: Effect bits determine pause-point status** | The 3-bit effect mask from `comp.effects/INF3` (IO \| Async \| Mutation) stored per function. IO or Async → potential pause point |
| **SC3: Public API stability** | A public function's `using` clause is part of its signature (`comp.hidden-params/PUB1`). Private functions gaining `__ctx` through propagation don't change the public interface |

This section makes explicit what existing specs already imply. The `__ctx` parameter is part of the compiled function signature, stored in module metadata like any other type information. Cross-module calls resolve it the same way they resolve regular parameters.

**SC4: Generic functions store conservative effects.** A generic public function `func process<T: Handler>(h: T)` might have different effects depending on `T`. Module metadata stores the union of effects across the generic body plus a "may vary by type parameter" flag. Post-monomorphization, effects are precise per instantiation (`comp.effects` edge case: "Effects inferred per monomorphized instance"). The state machine transform uses the conservative metadata for cross-module generics, precise data for local monomorphizations.

### Compilation flow

```
Module B (compiled first):
  public func process_file(path: string) using Multitasking → metadata: { __ctx: yes, effects: IO }
  public func parse_header(raw: string)                     → metadata: { __ctx: no,  effects: pure }

Module A (compiled second, imports B):
  using Multitasking {
      process_file("x.txt")    // compiler reads metadata → passes __ctx
      parse_header(raw)        // compiler reads metadata → no __ctx
  }
```

The state machine transform in module A checks effect bits from module B's metadata. `process_file` has IO → yield point generated. `parse_header` is pure → no yield point.

## FFI Boundary

| Rule | Description |
|------|-------------|
| **FFI1: No __ctx for extern functions** | Foreign functions never receive `RuntimeContext`. C code can't participate in cooperative scheduling |
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
| Trait with no I/O methods (e.g., `Display`) | VT1 | Vtable has __ctx slot. All implementations ignore it. One wasted register per call |
| `any Reader` in non-async context | VT3 | __ctx = None passed. Blocking path used |
| Pure closure stored in variable, called in spawn | FP1, FP3 | Wide ABI, yield point generated. Poll returns Ready immediately |
| Cross-module function gains `__ctx` internally | SC3 | Not a breaking change if function is private. Public functions require explicit `using` clause |
| Cross-module generic with type-dependent effects | SC4 | Conservative metadata (union of effects). Precise after monomorphization |
| FFI call returns in <1ms | FFI4 | No compensation thread, zero overhead |
| Many concurrent blocking FFI calls | FFI4 | Scheduler grows temporarily. Bounded by OS thread limits |
| FFI callback into Rask code | FFI1 | Runs on FFI's OS thread. No __ctx. Blocking behavior |
| `compile_rust()` interop (`struct.build`) | FFI1 | Same rules as C FFI — no __ctx, blocks worker |
| Nested trait object call (e.g., `io.copy(any Reader, any Writer)`) | VT4 | Two yield points per loop iteration. Two state machine variants. Acceptable — this is the I/O copy hot path, pausing is expected |

## Error Messages

```
ERROR [conc.phase-b/VT1]: trait object method has wrong ABI
   |
   | (internal compiler error — this should never surface to users)
   |
WHY: Vtable entry function was generated without __ctx parameter slot.
     This indicates a codegen bug in the hidden-params pass.
```

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

**VT1 (wide vtable entries):** I considered three alternatives: (a) two vtables per trait (sync/async), (b) whole-program analysis to track which implementations do I/O, (c) always-wide entries. Option (a) doubles vtable count and requires runtime vtable selection. Option (b) breaks Rask's "local analysis only" principle and doesn't work with separate compilation. Option (c) costs one register per call — negligible on modern CPUs where function call overhead is dominated by branch prediction and cache effects, not register setup. One register is the right price for simplicity.

**VT4 (trait object calls as pause points):** This means "dead" state machine variants for in-memory trait implementations. I think that's acceptable. The alternative — tracking which concrete types are behind a trait object — requires whole-program devirtualization, which is an optimization, not something the correctness of state machine generation should depend on. Dead variants poll as Ready, the scheduler never parks, no observable cost beyond a few bytes in the enum.

**FP1 (wide ABI for storable closures):** Consistency with VT1. All indirect calls — vtable dispatch, function pointers, storable closures — use the same calling convention. One rule to remember, one codegen path to maintain.

**FFI3 (compile-time warning):** The effects system already marks extern functions as conservatively IO (`comp.effects/INF5`). Detecting "extern call in async context" is a subset of the existing IO-in-ThreadPool warning (`comp.effects/CW1`). Same infrastructure, same suppressibility. I chose a warning over an error because fast FFI calls (crypto, compression, math) are common and harmless. The `@allow` annotation makes the suppression visible and auditable.

**FFI4 (runtime compensation):** Go does this for cgo and it works well in practice. The 1ms threshold avoids thread churn for fast FFI while catching blocking I/O. I thought about making the threshold configurable but decided against it — 1ms is a good default, and tuning knobs invite premature optimization. If profiling shows a different threshold is better, it can be changed in a point release without API changes.

### Alternatives Considered

**Per-trait vtable specialization:** Generate wide vtable entries only for traits whose methods could plausibly do I/O (traits with `[]u8` buffer parameters, traits returning `or IoError`, etc.). Rejected: heuristic-based, fragile, and a custom `trait Processor { func process(self) }` could do I/O internally. The heuristic would need constant updating.

**Thread-local __ctx instead of parameter threading:** Thread-local storage avoids the vtable ABI question entirely — implementations read __ctx from TLS. Rejected for the reasons already documented in `comp.hidden-params` appendix: not composable, not explicit, breaks on green task migration between worker threads.

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
