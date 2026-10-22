<!-- id: comp.effects -->
<!-- status: decided -->
<!-- summary: Compiler-inferred effect metadata: blocking, outside state, spawning, structural mutation — tooling-visible, not type-level -->
<!-- depends: concurrency/io-context.md, compiler/advanced-analyses.md -->

# Effect Tracking

The compiler infers which functions can block, touch state outside the program, spawn tasks, or change a collection's size. This information is metadata — visible through IDE ghost text and linter annotations, not enforced in the type system. No function coloring.

## Effect Categories

| Rule | Description |
|------|-------------|
| **FX1: Four categories** | Blocks (the thread can park until something outside it happens: a syscall, `sleep`, a channel op, a `join`), Ambient (reads or changes state nobody passed in and the program doesn't own, without waiting: the clock, environment variables, the process, the random source, the task runtime), Async (spawns a task), Mutation (Grow/Shrink — see `comp.advanced/EF1`, EF3, EF4) |
| **FX2: Transitive inference** | A function has effect X if it or any callee transitively has effect X |
| **FX3: Not in the type system** | Effects are compiler metadata. They don't appear in function signatures, don't constrain calling, don't split ecosystems |

Effects don't restrict what you can call. A function that doesn't block can call one that does — it just inherits the effect. This is tracking, not enforcement.

I split what used to be one "IO" effect in two because its two readers ask different questions. The warnings (CW1, CW2) care whether a thread can park; a channel `receive` does and isn't I/O. Compile-time evaluation (`ctrl.comptime/CT7`) cares whether the compiler can run the call at all; `Instant.now()` can't, and doesn't block.

"Ambient" is the word for what `Instant.now()` reads: state that is just there, reachable without anyone handing it over. Its arguments don't fix its answer, so the compiler can't compute it ahead of time and a replay has to record it. I didn't call it "external" because that reads as `extern`, which is about calling convention, not about what the call touches.

## Blocks and Ambient

| Rule | Description |
|------|-------------|
| **BL1: Sources** | A stdlib native says what it does on its declaration: `@blocks`, `@prints`, `@ambient`. `@prints` is terminal output: it blocks, but only when whatever reads stdout or stderr stops reading, and CW2 leaves it out unless asked. Every other stdlib function is a body, inferred like the program's. An `extern` C function is both unless it says otherwise (INF5) |
| **BL2: Transitive** | Any function that transitively calls a source has its effect |
| **BL3: Unsafe is what it calls** | An `unsafe` block has the effects of the calls in it. Raw pointer work touches nothing outside the program; a C call in the block is conservative on its own (INF5) |

<!-- test: skip -->
```rask
// Blocks — fs.read_text bottoms out at the native that opens the file
func load_config(path: string) -> Config or Error {
    let data = try fs.read_text(path)
    return try json.decode<Config>(data)
}

// Pure — nothing below it blocks or reaches outside
func parse_header(raw: string) -> Header or ParseError {
    let parts = raw.split(":").to_vec()
    if parts.len() != 2 { return ParseError.MalformedHeader }
    return Header(key: parts[0].trim(), value: parts[1].trim())
}
```

### The natives

The marks sit on the natives at the bottom of the stdlib, where no body says
what happens. What a public function does is inferred from them: `fs.exists`
blocks because its body calls the C `access`, `Stdout.write` prints because it
calls the native that writes to the stream.

| Mark | Natives |
|------|---------|
| `@prints` | the print functions; stdout and stderr writes and flush |
| `@blocks` | `fs`'s open and create; `File`'s raw read, write, seek and close; stdin reads; socket listen, connect, accept, read, write and close; process run, wait and pipe I/O; `time.sleep`; `Handle.join`, `Sender.send`, `Receiver.receive` |
| `@ambient` | environment variables, args, `os.pid`, `os.exit`, platform and arch; process spawn and poll; signals; the clock (`Instant.now`, `elapsed`, wall time); the global random source and `Random.new`; socket addresses; `errno`; `async.cancelled`, `Handle.cancel` |
| none | everything else: strings, collections, math, json, reflection, `Duration`, a seeded `Random` |

A call is matched on the callee the type checker resolved, not on how the call is spelled. `t.join()` on a `Handle<i64>` is `Handle.join` whatever the variable is called, and `parts.join(",")` on a `Vec<string>` is `Vec.join`, which blocks on nothing.

## Async Effect

| Rule | Description |
|------|-------------|
| **AS1: Sources** | A task block (`spawn { }`, `Thread.spawn { }`, `ThreadPool.spawn { }`) |
| **AS2: Transitive** | Any function that transitively calls an Async source has the Async effect |
| **AS3: Waiting blocks** | Waiting on another task — `time.sleep()`, the channel ops, `Handle.join()` — is Blocks, not Async. `spawn { }` hands a task to the scheduler and returns, so it is Async and blocks nothing. A loop of spawns blocks nothing; the `join` is where the wait is |

## Mutation Effect

| Rule | Description |
|------|-------------|
| **MU1: Defined by comp.advanced** | Structural effects (Grow, Shrink) are already formalized in `comp.advanced/EF1`, EF3, EF4 |
| **MU2: Orthogonal** | Mutation effects are tracked independently from the others. A function can block and mutate, just block, or neither |

## Inference

| Rule | Description |
|------|-------------|
| **INF1: Per-function** | Compiler computes effects for each function from its body + callees |
| **INF2: Module-local** | Within a module, infer from source. Cross-module: read effects from compiled metadata |
| **INF3: Public function metadata** | Public function effects are stored in compiled module output alongside type information |
| **INF4: No annotation required** | A program's effects are always inferred, never declared. The marks in BL1 are the stdlib's, on natives with no body to infer from, and a program can't write them (`type.annotations/AN5`) |
| **INF5: Conservative for extern** | An `extern` C function is assumed to block and touch ambient state. `@ambient` says it touches ambient state but never waits (a protocol engine that reads the clock). `@memory_only` says neither: the function works on the memory it's given and the heap, and reaches nothing outside the process: `@memory_only extern "C" func strlen(s: *u8) -> usize`. It has no body to check, so the declaration is trusted. `@memory_only` anywhere but an `extern` is an error (E0854): a Rask function's effects are read from its body |

### Inference Algorithm

```
infer_effects(program, stdlib):
    for func in stdlib ++ program:
        effects[func] = declared(func)             // @blocks, @ambient on a native
        for call in func.body.calls:
            if call.target is a spawn:     effects[func].add(Async)
            if call.target grows/shrinks:  effects[func].add(Mutation)
            edges.add(func -> call.target)
    for c in extern C functions:
        effects[c] = if c is @memory_only { none } else if c is @ambient { Ambient } else { Blocks + Ambient }
    propagate effects[callee] into effects[caller] along edges, to a fixed point
```

Fixed-point iteration handles mutual recursion.

## Purity

| Rule | Description |
|------|-------------|
| **PU1: Pure definition** | A function is pure if it has none of the four effects |
| **PU2: Comptime is pure** | `comptime func` is pure by definition (`ctrl.comptime/CT6-CT7`). Effect inference confirms it by measuring the body like any other function's: a marking the body doesn't earn is `ctrl.comptime/CT60`'s error, not a fact inference grants |
| **PU3: Purity is never written** | There's no `pure` keyword or annotation. Purity is an inferred property, shown as `[pure]` ghost text (IDE1). The one hand-written effect claim is `@memory_only` on an `extern`, which has no body to infer from (INF5) |

<!-- test: skip -->
```rask
// Pure — no effects
func add(a: i32, b: i32) -> i32 {
    return a + b
}

// Pure — errors are values, not effects
func parse(input: string) -> Config or ParseError {
    if input.is_empty() { return ParseError.Empty }
    return Config(value: input)
}

// Not pure — fs.read_text blocks
func load(path: string) -> Config or Error {
    let data = try fs.read_text(path)
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
func load_config(path: string) -> Config or Error {    // ghost: [blocks]
    let data = try fs.read_text(path)                 // ← blocking originates here
    return try json.decode<Config>(data)                // (no marker — pure)
}

func process(input: string) -> Result or Error {        // ghost: [pure]
    return try parse(input)
}

func run_server() -> void or Error {                      // ghost: [blocks, async]
    using Multitasking {
        let listener = try TcpListener.bind("0.0.0.0:8080")
        loop {
            let conn = try listener.accept()          // ← blocks
            spawn { handle(conn) }.detach()         // ← Async
        }
    }
}
```

### Ghost Text Format

| Effects | Ghost text |
|---------|-----------|
| No effects | `[pure]` |
| Any others | the ones present, in the order `blocks`, `ambient`, `async`, `mutation`: `[blocks]`, `[blocks, async]`, `[ambient]`, `[mutation]` |

Ghost text appears after the return type (or after `{` if no return type). Same position as inferred `using` clauses — they don't overlap because `using` is declared, effects are inferred.

## Compiler Warnings

| Rule | Description |
|------|-------------|
| **CW1: Blocking in ThreadPool** | Warn when a function that blocks is called inside `ThreadPool.spawn` (holds the pool thread). References `conc.io-context` appendix |
| **CW2: Blocking in a loop** | Warn when a function that blocks is called in a loop without `using Multitasking`, in a program that spawns (repeated blocking). A call that only prints doesn't count unless the function or test says `@warn(blocking_print)` (`tool.warnings/W12`) |

These use the existing `tool.warnings` infrastructure (`@allow` to suppress).

```
WARNING [comp.effects/CW1]: `fs.read_text` can block, and this runs on a thread pool
   |
5  |  ThreadPool.spawn {
6  |      let data = try fs.read_text("big.csv")
   |                     ^^^^^^^^^^^^ `fs.read_text` can block
   |
WHY: ThreadPool is for CPU-bound work. A call that blocks holds the pool
     thread instead of parking a green task.

FIX: Use `spawn` for I/O-heavy work:

  spawn {
      let data = try fs.read_text("big.csv")
      let result = try ThreadPool.spawn { parse(data) }.join()
  }.detach()
```

```
WARNING [comp.effects/CW2]: `http_get` can block, in a loop with no `using Multitasking`
   |
3  |  for url in urls {
4  |      let data = try http_get(url)
   |                       ^^^^^^^^ `http_get` can block
   |
WHY: Without `using Multitasking`, each call that blocks holds the thread in turn.

FIX: Wrap in using Multitasking to enable concurrent I/O:

  using Multitasking {
      for url in urls {
          let data = try http_get(url)
      }
  }
```

## Error Messages

Effect tracking produces no errors — only warnings and IDE annotations. Effects are metadata, not constraints.

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Recursive function | FX2, INF1 | Fixed-point iteration (converges in 2-3 passes) |
| Closure captures a blocking function | FX2 | Closure inherits effects of captured calls |
| Generic function | INF1 | Effects inferred per monomorphized instance (post-monomorphization) |
| `unsafe` block with no C calls | BL3 | Pure: raw pointer work touches nothing outside the program |
| `comptime func` | PU2 | Pure once CT60 has checked it — inference measures the body, and a `comptime func` reaching a call that blocks, outside state, or `spawn` is rejected at its definition. Growing a `Vec` is fine (`ctrl.comptime/CT26`) |
| Cross-module call | INF2, INF3 | Read effects from compiled metadata |
| `extern` function | INF5 | Blocks and Ambient, Ambient only under `@ambient`, neither under `@memory_only` |
| Function pointer / `any Interface` call | INF1 | Unknown: nothing is assumed (dynamic dispatch prevents static analysis) |

---

## Appendix (non-normative)

### Rationale

**FX3 (not in the type system):** I considered putting effects in function signatures — `func f() -> T [io]`. That's function coloring. A pure function that can't call a blocking one needs effect polymorphism for generic code. This is the same ecosystem split I rejected for async/await (`rejected-features.md`). Effects as metadata give you all the visibility (through IDE) without any of the cost (coloring, polymorphism, annotation burden).

**BL3 (unsafe is what it calls):** This used to be conservative: any `unsafe` block counted as I/O. The stdlib allocates through `unsafe`, so once its bodies were inferred that would have made `Vec.push` a blocking call. The C call is what could block, and INF5 is already conservative about it, so the block itself adds nothing.

**INF4 (no annotation required):** Rask's principle is "write intent, not mechanics" (`CORE_DESIGN.md` principle 7). Effect annotations are mechanics. The compiler infers them; the IDE shows them. Code stays clean.

**PU1 (pure definition):** Errors (`T or E`) are NOT effects. They're values in the type system. A function that returns `T or ParseError` and does nothing else is pure. This matches Haskell's distinction between `Either` (pure) and `IO` (effectful).

**CW1 (blocking in ThreadPool):** This replaces the hand-written linter rule sketched under `conc.runtime/HP2` with a principled check. The compiler now has effect metadata to back it up instead of relying on special-case detection.

### What This Doesn't Do

- **No function coloring.** A function that blocks can be called from anywhere. No restrictions.
- **No effect polymorphism.** Generic functions don't need `<E: Effect>` bounds.
- **No user-defined effects.** The three categories are fixed. No handler mechanisms.
- **No algebraic effects.** Effects can't be intercepted, resumed, or composed. See `rejected-features.md`.
- **No compile errors from effects.** Only warnings (`CW1`, `CW2`).

This is deliberately less powerful than Koka, Scala 3's capture checking, or Frank. The goal is visibility, not enforcement. Rask's type system handles the enforcement it needs through `T or E` (errors), the structural-effect rules (mutation under a borrow), and `using Multitasking` (async context).

### Relationship to Existing Features

| Feature | What it does | How effects relate |
|---------|-------------|-------------------|
| `T or E` | Tracks errors in types | Not an effect — already handled by the type system |
| `with v[i] as x { … }` | Borrows into a growable buffer | Mutation effect says which calls in the body could move that buffer (`comp.advanced/EF1`, EF4) |
| `using Multitasking { ... }` block | Installs process-global runtime | Blocks/Async effects track which functions need it; the CC2 reachability check is a separate pass |
| `comptime func` | Restricts to pure computation | Purity (PU2) gives vocabulary for what comptime already enforces |

### Implementation Notes

Effect inference runs as a pass after type checking: one call-graph walk plus fixed-point iteration, annotating metadata only — no AST changes.

**Compilation cost:** O(n) per module for the initial walk, O(n × k) for fixed-point iteration where k is typically 2-3. Well under the 10% analysis budget from `comp.advanced`.

**Storage:** Effects are a small bit mask per function: `Blocks | Ambient | Async | Grow | Shrink`. Stored in the module's compiled metadata alongside type signatures. Negligible space.

### Patterns & Guidance

**When effects are useful to know:**

- Debugging performance: "Why is this slow?" → IDE shows `[blocks]` on an inner function you thought was pure
- Code review: `rask annotate --diff` writes effect labels into the diff, and flags callers whose effects changed even when their lines didn't (`tool.annotate/DF2`)
- Refactoring: moving blocking calls out of a hot path — effects show which functions are safe to inline
- Architecture: effect summary per module shows which modules block vs compute

**When effects don't matter:**

- Most day-to-day coding. The information is there if you want it; it doesn't intrude if you don't.

### See Also

- `conc.io-context` — IO source function categorization
- `comp.advanced/EF1`, EF3, EF4 — the structural-effect rules
- `ctrl.comptime/CT6-CT8` — comptime purity restrictions
- `tool.warnings` — warning infrastructure (`@allow`, `@deny`)
