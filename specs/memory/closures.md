<!-- id: mem.closures -->
<!-- status: decided -->
<!-- summary: One closure concept — compiler infers capture strategy and lifetime constraints -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md, memory/pools.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Closures

Closures capture what they use. If what they use is temporary, they're temporary. One concept, compiler handles the rest.

## The Rule

When a closure uses a variable from outside its body, that variable is *captured*. The compiler decides how:

| What's captured | Strategy | Closure can... |
|----------------|----------|----------------|
| Owned/Copy values only | By value (copy or move) | Be stored, returned, sent cross-task |
| Mutable local (with `mutate`) | Mutable borrow | Live as long as the borrowed variable |
| Block-scoped borrow (struct field, array) | Borrow | Live as long as the borrowed data |
| Nothing (or only reads outer scope inline) | Direct access (optimized) | Be consumed in the expression |

No annotations. No closure "kinds." The compiler infers everything from what the closure captures and how it's used. IDE shows capture list as ghost text.

## Capturing Owned Values

Closures that capture only owned or Copy values are self-contained. They can go anywhere.

| Capture type | Behavior |
|--------------|----------|
| Small Copy types (≤16 bytes) | Copied into closure |
| Large/non-Copy types | Moved into closure, source invalid |

<!-- test: run -->
```rask
const name = "Alice"
const greet = || print("Hello, {name}")  // Copies name (small)
greet()  // "Hello, Alice"
// name still valid

const data = large_vec()
const process_data = || transform(data)  // Moves data
process_data()
// data invalid after move
```

Closures can accept parameters:

<!-- test: skip -->
```rask
const double = |x| x * 2
const result = double(5)  // 10
```

## Mutable Capture

Closures can borrow mutable locals with explicit `mutate` in the capture:

| Rule | Description |
|------|-------------|
| **MC1: Explicit annotation** | `mutate var` in closure capture list declares mutable borrow |
| **MC2: Exclusive access** | While a mutable capture exists, no other access to the variable |
| **MC3: Scope-limited** | Closure can't outlive the captured variable |
| **MC4: See mutations** | Caller sees mutations after closure completes |

<!-- test: skip -->
```rask
let count = 0
const inc = |mutate count| { count += 1 }
inc()
inc()
// count == 2

// Iterator example
let total = 0
items.for_each(|item, mutate total| { total += item.value })
print(total)  // sees accumulated value
```

Without `mutate`, captured values are copies. The mutation stays inside the closure:

<!-- test: skip -->
```rask
let count = 0
const inc = || { count += 1 }  // Captures count by COPY
inc()
// count is still 0 — the closure mutated its own copy
```

The `mutate` keyword makes the intent visible — you see exactly which variables the closure borrows mutably. IDE shows this in the capture annotation.

**Multiple closures can't share a mutable capture:**

<!-- test: skip -->
```rask
let x = 0
const a = |mutate x| { x += 1 }
const b = |mutate x| { x += 2 }  // ERROR: x already mutably captured by a
```

Use `Cell<T>` (see `mem.cell`) or Pool+Handle for shared mutable state across multiple closures.

## Inline Optimization

When a closure is consumed within the expression where it's created, the compiler may optimize it to access the outer scope directly — no capture overhead.

| Rule | Description |
|------|-------------|
| **IO1: No capture overhead** | Closures consumed inline access outer scope directly |
| **IO2: Must execute** | Must be called before expression completes |
| **IO3: Cannot store** | Compile error if assigned to variable or returned |
| **IO4: Aliasing rules** | Mutable access excludes other access during execution (`mem.borrowing/S5`) |

<!-- test: skip -->
```rask
// Compiler optimizes: no capture, direct access
items.filter(|i| vec[*i].active)
     .map(|i| vec[*i].value * 2)
     .collect()

// Mutation in inline context
let count = 0
items.for_each(|item| { count += 1 })  // inline: direct access, no capture needed
```

This is a compiler optimization, not a user concept. Users don't need to think about whether a closure is "inline" — the compiler figures it out. If the closure is consumed immediately in an expression chain, it's optimized. If it's stored, it captures by value.

Storage detection:

<!-- test: skip -->
```rask
// Inline: consumed immediately
for i in items.filter(|i| vec[*i].active) {
    process(i)
}

// Stored: captures by value
const f = |x| x * 2  // stored closure, captures nothing (pure)
```

## Scope-Limited Closures

A closure that captures a block-scoped borrow (struct field, array view) inherits that borrow's scope limit. It can't outlive the data it references.

| Rule | Description |
|------|-------------|
| **SL1: Scope inheritance** | Closure is limited to the innermost block of all its captured borrows |
| **SL2: No escape** | Cannot return, store in struct, or send cross-task |

<!-- test: skip -->
```rask
const entity = get_entity()
const name = entity.name            // block-scoped borrow (struct field)
const f = || process(name)         // f inherits scope constraint
f()                                 // OK: called in same scope
return f                            // ERROR: cannot escape scope
```

<!-- test: compile-fail -->
```rask
let outer_closure
{
    const entity = get_entity()
    const name = entity.name
    outer_closure = || process(name)  // ERROR: outer_closure outlives entity
}
```

**Fix: clone to owned data:**

<!-- test: skip -->
```rask
const name = entity.name.clone()   // owned copy
const f = || process(name)         // captures owned value
return f                            // OK: no scope constraint
```

## Generic Propagation

| Rule | Description |
|------|-------------|
| **GP1: Constraint propagation** | Scope constraints propagate through generic type parameters when the compiler generates specialized code |

Functions don't need to declare whether they store or consume closures. The constraint propagates through generics, and violations surface where storage is attempted.

<!-- test: skip -->
```rask
func run_twice<F: Fn()>(f: F) {
    f()
    f()
}  // F dropped, never stored — works with scope-limited closures

func store_callback<F: Fn()>(f: F) {
    const holder = Holder { callback: f }  // ERROR if F is scope-limited
}

const name = entity.name
const greet = || print(name)   // scope-limited (captures a borrow)

run_twice(greet)              // OK: run_twice doesn't store F
store_callback(greet)         // ERROR: store_callback tries to store F
```

## Error Messages

**Scope-limited closure escapes [SL2]:**
```
ERROR [mem.closures/SL2]: closure cannot escape scope
   |
3  |  const name = entity.name
   |               ^^^^^^^^^^^ borrowed from 'entity' (line 2)
4  |  const f = || process(name)
   |            ^^^^^^^^^^^^^^^^^ closure captures scoped borrow
5  |  return f
   |  ^^^^^^^^ cannot escape scope where 'entity' lives

FIX: Clone to owned data:

  const name = entity.name.clone()
  const f = || process(name)
  return f                          // OK: no scoped borrows
```

**Mutable capture conflict [MC2]:**
```
ERROR [mem.closures/MC2]: variable already mutably captured
   |
3  |  const a = |mutate x| { x += 1 }
   |             ^^^^^^^^^ x mutably captured here
4  |  const b = |mutate x| { x += 2 }
   |             ^^^^^^^^^ cannot capture x again

FIX: Use Cell<T> for shared mutable state:

  const x = Cell.new(0)
  const a = || x.modify(|v| v += 1)
  const b = || x.modify(|v| v += 2)
```

**Inline closure stored [IO3]:**
```
ERROR [mem.closures/IO3]: closure accesses outer scope directly but is stored
   |
5  |  let f = items.filter(|i| vec[*i].active)
   |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ closure accesses 'vec' without capturing

FIX 1: Consume immediately:

  items.filter(|i| vec[*i].active).collect()

FIX 2: Capture explicitly:

  let active_set = items.filter(|i| vec[*i].active).collect()
```

## Edge Cases

| Case | Handling |
|------|----------|
| Closure captures move-only type | Type moved into closure, source invalid |
| Closure captures resource type | Resource must be consumed within closure or transferred out |
| Nested closures | Each level captures from its immediate outer scope |
| Pure closure (no captures, no outer access) | Self-contained, can go anywhere |
| Clone inside closure | Creates independent copy |
| `mutate` capture of Copy type | Borrows mutably (not copied), mutations visible to caller |

---

## Implementation

### Closure Block Layout

All closures use a single contiguous block:

```
[func_ptr (8 bytes) | captured_var_0 | captured_var_1 | ...]
```

The closure value is a pointer to this block. When calling through a closure, `closure_ptr + 8` is the environment pointer — implicit first argument to the closure function. Captured variables live at known offsets relative to that pointer.

### Escape Analysis

MIR lowering initially marks every closure `heap: true`. A per-function optimization pass (`optimize_closures`) then downgrades non-escaping closures to stack allocation:

| Escape condition | Result |
|-----------------|--------|
| Closure appears in `Return` value | Stays `heap: true` |
| Closure passed as `Call` argument | Stays `heap: true` |
| Closure stored via `Store` | Stays `heap: true` |
| Only used via `ClosureCall` | Downgraded to `heap: false` (stack) |

Stack-allocated closures use a Cranelift stack slot — no runtime allocator call, no cleanup needed. Heap-allocated closures call `rask_alloc` and get a matching `ClosureDrop` (which calls `rask_free`) inserted before every return path where the closure isn't the return value.

Conservative local analysis: no cross-function tracking, no dataflow.

## Appendix (non-normative)

### Rationale

**One concept:** I had three closure kinds (stored, inline, scoped) which mapped closely to Rust's `Fn`/`FnMut`/`FnOnce`. Three concepts for the user to learn, three sections in the spec. But the compiler already inferred the kind from context — users never wrote annotations. So the "kinds" were implementation categories, not user concepts. I collapsed them into one: closures capture what they use, the compiler figures out the rest.

**Mutable capture (MC1):** Capture-by-value means mutating a captured variable only mutates the closure's copy — a common source of bugs. The `mutate` annotation makes mutable borrows explicit. You see exactly which outer variables the closure can modify. This replaces the Pool+Handle pattern for the simplest case (one closure mutating one local).

**Inline optimization (IO1-IO4):** Iterator chains like `.filter(|x| ...).map(|x| ...)` need access to surrounding scope without capture overhead. The compiler detects when a closure is consumed immediately and optimizes to direct access. This is transparent — the user writes the same closure syntax either way.

### Patterns & Guidance

**Choosing capture strategy:**

| Scenario | Pattern |
|----------|---------|
| Iterator adapter | `items.filter(\|i\| condition)` (inline optimized) |
| Simple callback | `\|x\| x * 2` (captures nothing, self-contained) |
| Callback with context | `\|event\| process(name, event)` (captures `name` by value) |
| Mutating a local | `\|mutate count\| count += 1` (mutable capture) |
| Shared mutable state (multiple closures) | `Cell<T>` or Pool+Handle |
| Callback stored for later | Capture owned values, or clone borrows |

**Cell<T> for shared mutable state:**

<!-- test: skip -->
```rask
const counter = Cell.new(0)

button1.on_click(|event| counter.modify(|c| c += 1))
button2.on_click(|event| counter.modify(|c| c += 10))

// After clicks: counter.read(|c| print(c))
```

See `mem.cell` for `Cell<T>` details.

### IDE Integration

| Context | Ghost Annotation |
|---------|------------------|
| Self-contained closure | `[captures: name (copy), data (move)]` |
| Scope-limited closure | `[scope-limited to line N]` |
| Inline-optimized closure | `[inline]` |
| Mutable capture | `[mutate: count]` |

On hover over a closure, show captures:

```
Closure captures:
  name: copied (string, 24 bytes)
  count: mutably borrowed (line 3)

Lifetime: scope-limited to line 3
```

When the cursor is in a scope-limited closure, the IDE highlights the block boundary it's limited to.

### See Also

- [Value Semantics](value-semantics.md) -- Copy vs move for captured values (`mem.value`)
- [Borrowing](borrowing.md) -- Block-scoped and statement-scoped access (`mem.borrowing`)
- [Cell](cell.md) -- Single-value mutable container (`mem.cell`)
- [Pools](pools.md) -- Pool+Handle pattern for shared mutable state (`mem.pools`)
- [Concurrency](../concurrency/sync.md) -- Closures sent cross-task must capture owned values (`conc.sync`)
