<!-- id: comp.effects -->
<!-- status: decided -->
<!-- summary: Compiler-inferred effect metadata for IO, async, and pool mutation — tooling-visible, not type-level -->
<!-- depends: concurrency/io-context.md, compiler/hidden-params.md, compiler/advanced-analyses.md, memory/context-clauses.md -->

# Effect Tracking

The compiler infers which functions perform I/O, async operations, or pool mutations. This information is metadata — visible through IDE ghost text and linter annotations, not enforced in the type system. No function coloring.

## Effect Categories

| Rule | Description |
|------|-------------|
| **FX1: Three categories** | IO (syscalls, file/network/stdio), Async (spawn, sleep, channel ops), Mutation (pool Grow/Shrink — see `comp.advanced/EF1-EF6`) |
| **FX2: Transitive inference** | A function has effect X if it or any callee transitively has effect X |
| **FX3: Not in the type system** | Effects are compiler metadata. They don't appear in function signatures, don't constrain calling, don't split ecosystems |

Effects don't restrict what you can call. A function without IO effects can call a function with IO effects — it just inherits the IO effect. This is tracking, not enforcement.

## IO Effect

| Rule | Description |
|------|-------------|
| **IO1: Source functions** | Ground truth: stdlib functions that accept `__ctx: RuntimeContext?` (see `conc.io-context` lines 117-129) |
| **IO2: Transitive** | Any function that transitively calls an IO source has the IO effect |
| **IO3: Unsafe I/O** | `unsafe` blocks that call C functions with I/O semantics are conservatively marked IO |

<!-- test: skip -->
```rask
// IO effect — calls File.open (source function)
func load_config(path: string) -> Config or Error {
    const data = try fs.read_file(path)
    return try json.decode<Config>(data)
}

// No IO effect — pure computation
func parse_header(raw: string) -> Header or ParseError {
    const parts = raw.split(":")
    if parts.len() != 2 { return Err(ParseError.MalformedHeader) }
    return Header { key: parts[0].trim(), value: parts[1].trim() }
}
```

### IO Source Functions

From `conc.io-context`:

| Module | Functions | IO? |
|--------|-----------|-----|
| `fs` | `File.open`, `File.read`, `File.write`, `File.close`, `fs.read_file`, `fs.write_file`, `fs.exists` | Yes |
| `net` | `TcpListener.accept`, `TcpConnection.read/write`, `UdpSocket.send/recv` | Yes |
| `io` | `Stdin.read`, `Stdout.write`, `Stderr.write` | Yes |
| `async` | `sleep`, `timeout` | Yes (also Async) |
| `io` | `Buffer.read`, `Buffer.write` | No |
| collections | `Vec`, `Map`, `Pool` | No |
| `json` | `json.encode`, `json.decode` | No |
| `fmt` | `format` | No |
| `math` | All functions | No |

## Async Effect

| Rule | Description |
|------|-------------|
| **AS1: Source functions** | `spawn()`, `sleep()`, `timeout()`, `Channel.send()`, `Channel.recv()`, `TaskHandle.join()` |
| **AS2: Transitive** | Any function that transitively calls an Async source has the Async effect |
| **AS3: Subset of IO** | All Async source functions are also IO sources (they involve scheduler/reactor). A function with Async always has IO too |

## Mutation Effect

| Rule | Description |
|------|-------------|
| **MU1: Defined by comp.advanced** | Pool mutation effects (Access, Grow, Shrink) are already formalized in `comp.advanced/EF1-EF6` |
| **MU2: Orthogonal** | Mutation effects are tracked independently from IO/Async. A function can have IO + Mutation, just IO, or neither |

## Inference

| Rule | Description |
|------|-------------|
| **INF1: Per-function** | Compiler computes effects for each function from its body + callees |
| **INF2: Module-local** | Within a module, infer from source. Cross-module: read effects from compiled metadata |
| **INF3: Public function metadata** | Public function effects are stored in compiled module output alongside type information |
| **INF4: No annotation required** | Effects are always inferred, never declared in source. The compiler does the work |
| **INF5: Conservative for extern** | `extern` functions and C FFI calls are conservatively assumed IO unless annotated `@no_io` |

### Inference Algorithm

```
infer_effects(func):
    effects = {}

    for call in func.body.calls:
        if call.target is stdlib_io_source:
            effects.add(IO)
        if call.target is async_source:
            effects.add(Async)
            effects.add(IO)  // AS3: Async implies IO
        if call.target is pool_grow_or_shrink:
            effects.add(Mutation)

        // Transitive: add callee's effects
        effects.union(callee_effects(call.target))

    if func.body.contains_unsafe_block:
        effects.add(IO)  // IO3: conservative

    return effects
```

Fixed-point iteration handles mutual recursion (same mechanism as `comp.hidden-params` context propagation).

## Purity

| Rule | Description |
|------|-------------|
| **PU1: Pure definition** | A function is pure if it has no IO, no Async, and no Mutation effects |
| **PU2: Comptime is pure** | `comptime func` is pure by definition (`ctrl.comptime/CT6-CT7`). Effect inference confirms this — the restriction set matches |
| **PU3: No pure keyword** | There's no `pure` keyword in the language. Purity is an inferred property. `@pure` is a lint annotation (see `tool.lint`) |

<!-- test: skip -->
```rask
// Pure — no IO, no Async, no Mutation
func add(a: i32, b: i32) -> i32 {
    return a + b
}

// Pure — errors are values, not effects
func parse(input: string) -> Config or ParseError {
    if input.is_empty() { return Err(ParseError.Empty) }
    return Config { value: input }
}

// Not pure — calls File.open (IO effect)
func load(path: string) -> Config or Error {
    const data = try fs.read_file(path)
    return try parse(data)
}
```

## IDE Integration

| Rule | Description |
|------|-------------|
| **IDE1: Ghost annotations** | IDE shows inferred effects as ghost text on function definitions |
| **IDE2: Call site markers** | IDE marks call sites where effects originate (not just propagate) |
| **IDE3: Hover detail** | Hovering a function shows its effect summary and the transitive chain |

<!-- test: skip -->
```rask
func load_config(path: string) -> Config or Error {    // ghost: [io]
    const data = try fs.read_file(path)                 // ← IO originates here
    return try json.decode<Config>(data)                // (no marker — pure)
}

func process(input: string) -> Result or Error {        // ghost: [pure]
    return try parse(input)
}

func run_server() -> () or Error {                      // ghost: [io, async]
    using Multitasking {
        const listener = try TcpListener.bind("0.0.0.0:8080")
        loop {
            const conn = try listener.accept()          // ← IO + Async
            spawn(|| { handle(conn) }).detach()         // ← Async
        }
    }
}
```

### Ghost Text Format

| Effects | Ghost text |
|---------|-----------|
| No effects | `[pure]` |
| IO only | `[io]` |
| IO + Async | `[io, async]` |
| Mutation only | `[mutation]` |
| IO + Mutation | `[io, mutation]` |

Ghost text appears after the return type (or after `{` if no return type). Same position as inferred `using` clauses — they don't overlap because `using` is declared, effects are inferred.

## Compiler Warnings

| Rule | Description |
|------|-------------|
| **CW1: IO in ThreadPool** | Warn when a function with IO effect is called inside `ThreadPool.spawn` (blocks pool thread). References `conc.io-context` appendix |
| **CW2: IO in tight loop** | Warn when IO function is called in a loop without `using Multitasking` (repeated blocking) |

These use the existing `tool.warnings` infrastructure (`@allow` to suppress).

```
WARNING [comp.effects/CW1]: I/O function called in thread pool context
   |
5  |  ThreadPool.spawn(|| {
6  |      const data = try File.read("big.csv")
   |                       ^^^^^^^^^ File.read has IO effect — blocks pool thread
   |
WHY: ThreadPool is for CPU-bound work. I/O blocks the pool thread instead of
     parking a green task.

FIX: Use spawn() for I/O-heavy work:

  spawn(|| {
      const data = try File.read("big.csv")
      const result = try ThreadPool.spawn(|| { parse(data) }).join()
  }).detach()
```

```
WARNING [comp.effects/CW2]: I/O in loop without Multitasking context
   |
3  |  for url in urls {
4  |      const data = try http_get(url)
   |                       ^^^^^^^^ IO effect in loop — blocks thread on each iteration
   |
WHY: Without `using Multitasking`, each I/O call blocks the thread sequentially.

FIX: Wrap in using Multitasking to enable concurrent I/O:

  using Multitasking {
      for url in urls {
          const data = try http_get(url)
      }
  }
```

## Error Messages

Effect tracking produces no errors — only warnings and IDE annotations. Effects are metadata, not constraints.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Recursive function | FX2, INF1 | Fixed-point iteration (converges in 2-3 passes) |
| Closure captures IO function | FX2 | Closure inherits effects of captured calls |
| Generic function | INF1 | Effects inferred per monomorphized instance (post-monomorphization) |
| `unsafe` block with no C calls | IO3 | Still conservative (IO assumed). Suppress with `@no_io` on the function |
| `comptime func` | PU2 | Always pure — `ctrl.comptime/CT6-CT7` enforces this structurally |
| Cross-module call | INF2, INF3 | Read effects from compiled metadata |
| `extern` function | INF5 | Conservative IO unless `@no_io` annotated |
| Function pointer / `any Trait` call | INF1 | Conservative: assumed IO + Async (dynamic dispatch prevents static analysis) |

---

## Appendix (non-normative)

### Rationale

**FX3 (not in the type system):** I considered putting effects in function signatures — `func f() -> T [io]`. That's function coloring. A pure function that can't call an IO function needs effect polymorphism for generic code. This is the same ecosystem split I rejected for async/await (`rejected-features.md`). Effects as metadata give you all the visibility (through IDE) without any of the cost (coloring, polymorphism, annotation burden).

**IO3 (conservative unsafe):** Unsafe blocks might call C functions that do I/O. I'd rather over-approximate than miss an IO effect. The `@no_io` escape hatch handles the rare case where unsafe code is provably pure.

**INF4 (no annotation required):** Rask's principle is "write intent, not mechanics" (`CORE_DESIGN.md` principle 7). Effect annotations are mechanics. The compiler infers them; the IDE shows them. Code stays clean.

**PU1 (pure definition):** Errors (`T or E`) are NOT effects. They're values in the type system. A function that returns `T or ParseError` and does nothing else is pure. This matches Haskell's distinction between `Either` (pure) and `IO` (effectful).

**CW1 (IO in ThreadPool):** This replaces the ad-hoc rule `conc.runtime/HP2.4` with a principled check. The compiler now has effect metadata to back it up instead of relying on special-case detection.

### What This Doesn't Do

- **No function coloring.** A function with IO effects can be called from anywhere. No restrictions.
- **No effect polymorphism.** Generic functions don't need `<E: Effect>` bounds.
- **No user-defined effects.** The three categories are fixed. No handler mechanisms.
- **No algebraic effects.** Effects can't be intercepted, resumed, or composed. See `rejected-features.md`.
- **No compile errors from effects.** Only warnings (`CW1`, `CW2`) and lint annotations (`@pure`).

This is deliberately less powerful than Koka, Scala 3's capture checking, or Frank. The goal is visibility, not enforcement. Rask's type system handles the enforcement it needs through `T or E` (errors), `using frozen` (mutation), and `using Multitasking` (async context).

### Relationship to Existing Features

| Feature | What it does | How effects relate |
|---------|-------------|-------------------|
| `T or E` | Tracks errors in types | Not an effect — already handled by the type system |
| `using Pool<T>` | Threads pool as hidden param | Mutation effect tracks Grow/Shrink from `comp.advanced/EF1-EF6` |
| `using frozen Pool<T>` | Restricts to Access-only | Already enforced at type level — effects confirm it |
| `using Multitasking` | Provides RuntimeContext | IO/Async effects track which functions need it |
| `comptime func` | Restricts to pure computation | Purity (PU2) gives vocabulary for what comptime already enforces |
| Hidden params pass | Threads contexts | Effect inference reuses the same transitive propagation mechanism |

### Implementation Notes

Effect inference runs as a pass after type checking, alongside the hidden-params pass. It reuses the same call-graph walk and fixed-point iteration (`comp.hidden-params` propagation algorithm). The difference: hidden-params modifies the AST (inserts parameters), effect inference only annotates metadata (no AST changes).

**Compilation cost:** O(n) per module for the initial walk, O(n × k) for fixed-point iteration where k is typically 2-3. Well under the 10% analysis budget from `comp.advanced`.

**Storage:** Effects are a 3-bit mask per function: `IO | Async | Mutation`. Stored in the module's compiled metadata alongside type signatures. Negligible space.

### Patterns & Guidance

**When effects are useful to know:**

- Debugging performance: "Why is this slow?" → IDE shows IO effect on an inner function you thought was pure
- Code review: ghost annotations make side effects visible in diffs
- Refactoring: moving IO out of a hot path — effects show which functions are safe to inline
- Architecture: effect summary per module shows which modules are IO-heavy vs pure computation

**When effects don't matter:**

- Most day-to-day coding. The information is there if you want it; it doesn't intrude if you don't.

### See Also

- `conc.io-context` — IO source function categorization
- `comp.hidden-params` — context threading mechanism (shared infrastructure)
- `comp.advanced/EF1-EF6` — pool mutation effect system
- `ctrl.comptime/CT6-CT8` — comptime purity restrictions
- `tool.lint` — `@pure` annotation enforcement
- `tool.warnings` — warning infrastructure (`@allow`, `@deny`)
