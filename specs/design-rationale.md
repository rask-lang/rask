// SPDX-License-Identifier: (MIT OR Apache-2.0)
# Design Rationale

This document explains design decisions where Rask intentionally rejected patterns from other languages, and why.

---

## Rejected: Algebraic Effects

**Languages:** OCaml, Koka, Unison
**Decision:** Do not add algebraic effects
**Date:** 2026-02-08

### What Are Algebraic Effects?

Algebraic effects allow functions to raise "effects" (like exceptions, but more general) that handlers up the call stack can intercept and resume. They enable:
- Implicit control flow (non-local jumps)
- Handler composition across function boundaries
- Effect polymorphism (functions abstract over effects)

Example (hypothetical):
```
// Function can raise Async effect without declaring it
func process(data: Data) -> Result {
    return parse(file.read())  // Implicitly raises Async, handled elsewhere
}
```

### Why Rejected

Algebraic effects **fundamentally violate Rask's core design principles:**

#### 1. Transparency of Cost (Principle 4)

From `CORE_DESIGN.md`:
> "Major costs are visible in code. Small safety checks can be implicit."

Effects hide major costs (I/O, allocation, locking) behind implicit control flow:

```rask
// Current Rask: all costs visible
const file = try open(path)    // Visible: I/O syscall
ensure file.close()            // Visible: cleanup guarantee
try process(file)              // Visible: error propagation

// With effects: costs hidden
process(file)  // What actually happens?
               // - Does it allocate?
               // - Does it do I/O?
               // - Does it call handlers invisibly?
               // - Does ensure still run? When?
```

A function like `process(data)` could pause for I/O, allocate, or jump to handlers—none visible at the call site.

#### 2. Local Analysis Only (Principle 5)

From `CORE_DESIGN.md`:
> "All compiler analysis is function-local. No whole-program inference, no cross-function lifetime tracking."

Effects require **whole-program analysis** to understand behavior:
1. Track all effects a function can raise
2. Trace all possible handlers in scope
3. Analyze handler compositions across the call stack

This breaks local compilation. Changing a handler deep in the call stack affects type-checking and behavior of functions far above it.

#### 3. Mechanical Safety (Principle 2)

Rask's safety is "by structure"—linear resources, affine handles, block-scoped cleanup. Effects introduce non-local control flow that breaks structural guarantees:

```rask
const file = try open(path)
ensure file.close()       // Does this ALWAYS run?
try process(file)         // What if process raises an effect that jumps the stack?
                          // Is file still valid across the effect boundary?
```

Currently, `ensure` has **clear semantics** (LIFO, block-scoped, runs on all exits). Effects make this ambiguous.

#### 4. No Function Coloring

From `CORE_DESIGN.md`:
> "There is no `async`/`await`. Functions are just functions."

Effects create a **hidden coloring problem**: Functions that can raise effects become implicitly constrained. You can't use them in contexts without handlers. This is the `async`/`await` split Rask explicitly rejects—just hidden in effect types instead of keywords.

#### 5. Explicit Error Handling

Rask's error model is intentionally **visible and non-local**:

```rask
const value = try some_operation()   // IDE shows: "→ returns Err"
```

Effects hide error flows. A handler could catch errors before they reach the caller, violating:
> "No hidden control flow. Errors do not throw or unwind. All error paths are visible in types."

### What Rask Chose Instead

Rather than algebraic effects, Rask provides **explicit, transparent mechanisms**:

| Need | Effect Solution | Rask Solution |
|------|-----------------|---------------|
| Error handling | Effect handlers | `Result<T, E>` types + `try` |
| Async I/O | Async effect | `with multitasking`, I/O pauses tasks |
| Resource cleanup | Finalizer effect | `ensure` blocks (LIFO, guaranteed) |
| Context passing | Reader effect | Function parameters or `with` blocks |
| State | State effect | `Shared<T>` or explicit parameters |

These are more **verbose** than effects in some cases, but **every cost is visible and every path is local**.

### The Fundamental Tradeoff

**Algebraic effects trade:**
- ✅ Less ceremony for effect handling
- ❌ Loss of local reasoning
- ❌ Whole-program compilation required
- ❌ Hidden control flow
- ❌ Implicit resource semantics

**Rask trades:**
- ✅ Complete local analysis
- ✅ Linear compile times
- ✅ Visible error paths
- ✅ Predictable resource cleanup
- ❌ More explicit propagation in error-heavy code

### Conclusion

If Rask added effects, it would no longer be Rask. The entire design would shift from "mechanical safety by structure" to "safety by effect algebra," requiring whole-program analysis, breaking incremental compilation, and hiding costs.

The stated goal—"safety is invisible" but "major costs are visible"—becomes impossible to achieve with algebraic effects.

---

## Rejected: Automatic Supervision (Erlang-Style)

**Languages:** Erlang, Elixir
**Decision:** Supervision is a library pattern, not a language feature
**Date:** 2026-02-08

### What Is Automatic Supervision?

Erlang has built-in supervision trees where supervisors automatically monitor and restart failed processes:
- Processes are linked in hierarchies
- When a child process crashes, supervisor automatically restarts it
- Propagation and restart strategies are language primitives

### Why Rejected

Automatic restart is a **hidden side effect** that violates transparency of cost.

From `CORE_DESIGN.md`:
> "Major costs are visible in code."

Automatic restarts are **major costs**—they're not cheap, and they should be explicit:

```rask
// Explicit restart loop (all costs visible)
let restart_count = 0
loop {
    const h = spawn { worker_task() }
    match h.join() {
        Ok(()) => { break },
        Err(e) => {
            restart_count += 1
            if restart_count > 5: return Err("too many restarts")
            println("Restarting after error: {e}")
            // Loop continues, spawns new task
        }
    }
}
```

This is **intentionally explicit** per Rask's design principles.

### What Conflicts with Rask

1. **Hidden costs** - Automatic restart happens invisibly
2. **Global analysis** - Supervision trees require whole-program process graph tracking
3. **Implicit propagation** - Process linking and failure cascades are magical
4. **Non-local behavior** - Changing a supervisor affects distant child processes

### What Rask Chose Instead

**Supervision as a library pattern** (like TaskGroup):

```rask
const sup = Supervisor.new()
    .with_policy(RestartPolicy.OneForOne)
    .with_max_restarts(5)

sup.spawn_child("worker", || worker_task())
sup.spawn_child("logger", || logger_task())

sup.run()  // Explicit supervision loop
```

**Why library, not language feature:**

| Aspect | Erlang (language) | Rask (library) |
|--------|-------------------|----------------|
| Restart mechanism | Automatic | Explicit (library code) |
| Task tracking | Implicit linking | Explicit spawn_child() |
| Supervision scope | Global process tree | Local struct methods |
| Failure visibility | Escalation to supervisor | Result<T, E> + monitoring loop |

**Rask's approach:**
- ✅ Transparency: Restart loops are explicit code
- ✅ Local analysis: Supervisors are regular structs
- ✅ Mechanical safety: Affine handles prevent orphaned tasks
- ✅ No magic: All behavior is visible user code

### Why NOT `with supervisor`?

We considered making supervision a `with` block (like `with threading`), but rejected it:

1. **Ambiguous spawn tracking** - How does supervisor know which spawns to supervise?
   - All spawns? Breaks explicit tracking
   - Only `spawn_supervised`? Adds new spawn variant, breaks consistency

2. **Conflicts with block scope** - `with` blocks cleanup on exit, supervisors run indefinitely

3. **Less flexible** - Can't easily model supervision trees or selective supervision

**Supervision is more like `TaskGroup` than `with threading`:**

| Pattern | Purpose | Scope |
|---------|---------|-------|
| `with threading { }` | Provides thread pool **capability** | Block-scoped |
| `TaskGroup.new()` | Provides task **grouping** | Explicit tracking |
| `Supervisor.new()` | Provides task **supervision** | Explicit monitoring |

TaskGroup is already a struct because it needs explicit control over which spawns join the group. Supervision has the same requirement.

### Conclusion

Erlang-style supervision patterns are **valuable and compatible with Rask**—but as explicit library code, not magical language behavior. This maintains transparency of cost while still enabling fault-tolerant systems.

---

## Rejected: Kotlin-Style Implicit Scope Functions

**Languages:** Kotlin
**Decision:** Use explicit closure parameters instead of implicit `it`/`this`
**Date:** 2026-02-08

### What Are Kotlin-Style Scope Functions?

Kotlin has `.let`, `.apply`, `.also`, `.run` that use implicit receivers:

```kotlin
// Kotlin - implicit 'it' receiver
val result = config.let { it.validate() }

// Kotlin - implicit 'this' receiver
val configured = Config().apply {
    this.host = "localhost"
    this.port = 8080
}
```

### Why Rejected (Syntax Sugar)

Rask **already has scope function patterns** via explicit closure parameters. Adding Kotlin-style syntax would:

1. **Add parser complexity** - Special-case `.let { }` desugaring
2. **Contradict "explicit over implicit"** - Rask prefers clarity over terseness
3. **Less debuggable** - Implicit `it` doesn't appear in stack traces
4. **Unclear scope entry** - What resource is being borrowed/locked?

### What Rask Chose Instead

**Explicit closure parameters:**

```rask
// Rask - explicit parameter name
const users = db.read(|d| {
    d.users.values().map(|u| transform(u)).collect()
})

// Rask - explicit mutation
db.write(|d| {
    d.users.insert(id, user)
})
```

**Comparison:**

| Kotlin | Rask Equivalent | Rask Advantage |
|--------|-----------------|----------------|
| `obj.let { it.field }` | `obj.read(\|d\| d.field)` | Explicit parameter name (no magic `it`) |
| `obj.apply { mutate() }` | `obj.write(\|d\| d.mutate())` | Clear borrow semantics |
| `obj.also { log(it) }` | `obj.read(\|d\| { log(d); d })` | Explicit return value |

**Why explicit is better:**
- No implicit `it` or `this` - parameter name shows intent
- Clear what scope you're entering (lock, borrow, etc.)
- Compiler can enforce cleanup naturally
- More debuggable - parameter appears in stack traces

### Optional: Generic Scope Functions

If desired, Rask could provide **generic methods** without syntax sugar:

```rask
extend<T> T {
    func let<R>(self, f: |Self| -> R) -> R {
        return f(own self)
    }

    func apply(self, f: |Self| -> ()) -> Self {
        f(self)
        return self
    }
}

// Usage (no parser changes needed)
const result = config.let(|c| c.validate())
const configured = Config.new().apply(|c| {
    c.host = "localhost"
    c.port = 8080
})
```

This works with **current syntax**—no language changes required.

### Conclusion

Rask's explicit closure parameters are **more readable than Kotlin's implicit receivers**, not less. The pattern already exists and works naturally. Adding Kotlin-style syntax sugar would sacrifice clarity for minimal ergonomic gain.

Position explicit parameters as a **feature, not a limitation**.

---

## Design Philosophy Summary

These rejections share a common thread:

| Rejected Feature | Why | Rask Alternative |
|------------------|-----|------------------|
| Algebraic effects | Hidden control flow, whole-program analysis | Explicit Result types, `try`, `ensure` |
| Automatic supervision | Hidden restart costs, non-local behavior | Library pattern, explicit loops |
| Implicit scope functions | Magic receivers, unclear scope | Explicit closure parameters |

**Core principle:** Rask optimizes for **transparency and local reasoning** over **terseness and magic**.

This is not a judgment on other languages—Kotlin, Erlang, and OCaml made different tradeoffs for different goals. Rask's goal is systems programming where:
- Costs must be visible
- Analysis must be local
- Safety must be structural
- Debugging must be predictable

These rejected features would make Rask more **convenient** in some cases, but less **transparent**—which contradicts the core design philosophy.
