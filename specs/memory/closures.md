<!-- id: mem.closures -->
<!-- status: decided -->
<!-- summary: Three closure kinds — stored (by value), inline (outer scope), scoped (block-limited borrows) -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md, memory/pools.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Closures

When a closure uses a variable from outside its body, that variable is *captured* — copied or moved into the closure so it's available when the closure runs later. Closures capture by value (copy/move) for storage, access the outer scope directly for inline use, or capture borrows for scoped use. The kind is inferred from how you use the closure, not by annotation.

## Closure Kinds

| Rule | Kind | Capture | Storage | Use Cases |
|------|------|---------|---------|-----------|
| **CL1: Stored** | Stored | By value (copy/move) | Can be stored, returned, sent cross-task | Callbacks, event handlers, async tasks |
| **CL2: Inline** | Inline | None (accesses outer scope) | Cannot store | Iterator adapters, inline callbacks |
| **CL3: Scoped** | Scoped | By value + borrows | Limited to borrow's block | Closures capturing slices, temporary refs |

Kind is inferred from context:
- Used inline in an expression chain &rarr; inline
- Stored/returned, captures only owned values &rarr; stored
- Stored/assigned, captures borrows &rarr; scoped

## Stored Closures

Capture by value (copy or move), never by reference. Can be stored in variables, structs, or returned.

| Capture type | Behavior |
|--------------|----------|
| Small Copy types (≤16 bytes) | Copied into closure |
| Large/non-Copy types | Moved into closure, source invalid |
| Mutable state | Requires Pool + Handle pattern |

<!-- test: run -->
```rask
const name = "Alice"
const greet = || print("Hello, {name}")  // Copies name (string is small)
greet()  // "Hello, Alice"
// name still valid

const data = large_vec()
const process_data = || transform(data)  // Moves data
process_data()
// data invalid after move
```

Closures can accept parameters passed on each invocation:

<!-- test: skip -->
```rask
const double = |x| x * 2
const result = double(5)  // 10

const format_user = |id| "User #{id}"
button.on_click(|event| {
    print(format_user(event.user_id))
})
```

**Capture mutation:** Stored closures capture by copy, so mutating a captured value only mutates the closure's copy. Use Pool + Handle for shared mutable state.

<!-- test: skip -->
```rask
// WRONG: mutating a copy
const counter = 0
const increment = || counter += 1  // Captures counter by COPY
increment()
// counter is still 0

// CORRECT: Pool + Handle
const state = Pool.new()
const h = state.insert(Counter{value: 0})
const increment = |state_pool| state_pool[h].value += 1
increment(state)
increment(state)
```

## Inline Closures

Access outer scope directly without capturing. Must be consumed within the expression — they can't be stored or returned.

| Rule | Description |
|------|-------------|
| **CL6: No capture** | Closure reads/writes the outer scope directly, does not copy values in |
| **CL7: Must execute** | Must be called before expression completes |
| **CL8: Cannot store** | Compile error if assigned to variable or returned |
| **CL9: Aliasing rules** | Mutable access excludes other access during execution (`mem.borrowing/S5`) |

<!-- test: skip -->
```rask
const items = vec![...]
const vec = vec![...]

// Closure accesses vec WITHOUT capturing it
items.filter(|i| vec[*i].active)
     .map(|i| vec[*i].value * 2)
     .collect()
// vec still valid after chain
```

Mutable access with inline execution:

<!-- test: skip -->
```rask
const app = AppState.new()

// Immediate: closure executed within expression
(try button.on_click(|event| {
    app.counter += 1
    app.last_click = event.timestamp
})).execute_now()

// app still valid here
```

Storage detection:

<!-- test: skip -->
```rask
// Legal: inline consumption
for i in items.filter(|i| vec[*i].active) {
    process(i)
}

// Illegal: storing an inline closure
const f = items.filter(|i| vec[*i].active)
//        ^^^^^^^^^ ERROR: closure accesses 'vec' but iterator is stored
```

## Scoped Closures

Closures that capture block-scoped borrows (struct field references, array views). Can't escape the block where the borrowed data lives.

| Rule | Description |
|------|-------------|
| **CL4: Scope inheritance** | Closure is limited to the innermost block of all its captured borrows |
| **CL5: No escape** | Cannot return, store in struct, or send cross-task |

The compiler determines scope constraints through local analysis only:

| Step | What Happens |
|------|--------------|
| Borrow creation | `entity.name` creates a view tied to `entity`'s block |
| Closure creation | Compiler sees the closure captures a borrow — marks it scoped |
| Assignment check | Scoped closures must be assigned in the same or inner block |
| Return/store check | Scoped closures cannot be returned, stored in structs, or sent cross-task |

<!-- test: skip -->
```rask
const entity = get_entity()
const name = entity.name            // block-scoped borrow (struct field)
const f = || process(name)         // f inherits scope constraint
f()                                 // OK: called in same scope
return f                            // ERROR: cannot escape scope (CL5)
```

Assigning to outer variable:

<!-- test: compile-fail -->
```rask
let outer_closure
{
    const entity = get_entity()
    const name = entity.name
    outer_closure = || process(name)  // ERROR: outer_closure outlives entity (CL4)
}
```

Storing in struct:

<!-- test: compile-fail -->
```rask
const name = entity.name
const f = || process(name)
const handler = Handler { callback: f }  // ERROR: struct fields cannot hold scope-constrained closures (CL5)
```

Passing to immediate consumer:

<!-- test: skip -->
```rask
const name = entity.name
const f = || process(name)
execute_now(f)                    // OK: execute_now consumes f immediately
```

## Generic Propagation

| Rule | Description |
|------|-------------|
| **CL10: Constraint propagation** | Scope constraints propagate through generic type parameters when the compiler generates specialized code |

No special annotations needed. Functions that don't store their generic closure argument work with scoped closures automatically. Functions that store the argument produce a compile error at the storage site.

<!-- test: skip -->
```rask
func run_twice<F: Fn()>(f: F) {
    f()
    f()
}  // F dropped, never stored - works with scope-constrained closures

func store_callback<F: Fn()>(f: F) {
    const holder = Holder { callback: f }  // ERROR if F is scoped (CL5)
}

const name = entity.name
const greet = || print(name)   // scoped (captures a borrow)

run_twice(greet)              // OK: run_twice doesn't store F
store_callback(greet)         // ERROR: store_callback tries to store F
```

The error surfaces inside `store_callback` at the storage site, not at the call site. Functions don't need to declare whether they store or consume inline.

## Error Messages

**Inline closure stored [CL8]:**
```
ERROR [mem.closures/CL8]: inline closure cannot be stored
   |
5  |  let f = items.filter(|i| vec[*i].active)
   |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ closure accesses 'vec' without capturing

WHY: This closure accesses 'vec' in the outer scope directly.
     It must be consumed within the same expression.

FIX 1: Use the closure immediately:
   |
5  |  items.filter(|i| vec[*i].active).collect()

FIX 2: Capture by value instead:
   |
5  |  let active_set: Set<usize> = items
   |      .filter(|i| vec[*i].active).collect()
   |  let f = items.filter(|i| active_set.contains(i))
```

**Scoped closure escapes [CL5]:**
```
ERROR [mem.closures/CL5]: scoped closure cannot escape
   |
3  |  const name = entity.name
   |               ^^^^^^^^^^^ borrowed from 'entity' (line 2)
4  |  const f = || process(name)
   |            ^^^^^^^^^^^^^^^^^ closure captures scoped borrow
5  |  return f
   |  ^^^^^^^^ cannot escape scope where 'entity' lives

WHY: 'name' is borrowed from 'entity'. Returning the closure would let
     'name' outlive 'entity' (use-after-free).

FIX 1: Use in same scope:
   |
5  |  f()

FIX 2: Capture owned data instead:
   |
3  |  const name = entity.name.clone()
4  |  const f = || process(name)
5  |  return f                          // OK: no scoped borrows
```

**Scoped closure assigned to outer variable [CL4]:**
```
ERROR [mem.closures/CL4]: closure outlives its captured borrow
   |
1  |  let outer_closure
   |      ^^^^^^^^^^^^^ declared here (outer scope)
3  |      const entity = get_entity()
4  |      const name = entity.name
5  |      outer_closure = || process(name)
   |                      ^^^^^^^^^^^^^^^^ captures 'name' (borrowed from 'entity')
6  |  }
   |   ^ 'entity' dropped here

WHY: 'outer_closure' lives longer than 'entity', but it captures 'name'
     which borrows from 'entity'.

FIX: Create the closure inside the scope:
   |
3  |      const entity = get_entity()
4  |      const name = entity.name
5  |      const inner_closure = || process(name)
6  |      inner_closure()
```

**Generic function stores scoped closure [CL10]:**
```
ERROR [mem.closures/CL10]: cannot store scoped type
   |
// in store_callback<F>:
3  |  const holder = Holder { callback: f }
   |                          ^^^^^^^^^^^^ F is scope-constrained

Note: called with scoped closure at:
10 |  store_callback(greet)
   |                 ^^^^^ 'greet' captures borrowed 'name' (line 8)

FIX 1: Use a function that doesn't store:
   |
10 |  run_twice(greet)

FIX 2: Remove the borrow from the closure:
   |
8  |  const name = entity.name.clone()
9  |  const greet = || print(name)
10 |  store_callback(greet)         // OK
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Closure captures move-only type | CL1 | Type moved into closure, source invalid |
| Closure captures resource type | CL1 | Resource must be consumed within closure or transferred out |
| Nested closures | CL1 | Each level captures from its immediate outer scope |
| Async closure | CL1 | Treated as storable; captured values moved in |
| Clone inside closure | CL1 | Creates independent copy |
| Stored closure captures borrow | CL3 | Closure becomes scoped |
| Pure closure (no captures, no outer access) | CL1 | Stored by default |
| Mutable access in inline closure | CL9 | Exclusive access rule applies during execution |

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

This is conservative local analysis: no cross-function tracking, no dataflow. A closure that's only called locally but happens to be passed to another function stays heap-allocated. Correctness over cleverness.

## Appendix (non-normative)

### Rationale

**CL1 (stored, capture by value):** I chose capture-by-value to make closures self-contained and thread-safe. The tradeoff is more `.clone()` calls and the Pool+Handle pattern for shared mutable state. I think that's better than reference-capturing closures that drag scope annotations into everything.

**CL2 (inline closures):** Iterator chains like `.filter(|x| ...).map(|x| ...)` need access to surrounding scope without the overhead of capturing. Inline closures give you this with the constraint that the closure can't escape the expression. This covers the most common closure pattern (iterator adapters) with zero ceremony.

**CL3–CL5 (scoped closures):** A stored closure that captures a block-scoped borrow (struct field, array element) becomes scoped. This is enforced through type-level scope markers, not escape analysis. All checks are local to the function — no cross-function tracking needed.

**CL10 (generic propagation):** Functions don't need to declare "I consume inline" vs "I store." The constraint propagates through generics when specialized code is generated, and violations surface where storage is attempted. This keeps function signatures clean.

### Metaphor: Closures as Luggage

A useful way to think about the three closure kinds:

| Kind | Metaphor | Explanation |
|------|----------|-------------|
| Stored | Backpack | Packs copies of everything. Can go anywhere. |
| Inline | Hand-carry | Holding items directly. Must use now. |
| Scoped | Day-trip bag | Borrows some items from nearby. Can't leave the area those items came from. |

### Patterns & Guidance

**Pool+Handle for stateful callbacks:**

<!-- test: skip -->
```rask
struct App {
    state: Pool<AppState>
    state_handle: Handle<AppState>
}

func setup_handlers(app: App) {
    const h = app.state_handle

    // Each handler captures handles, receives state as parameter
    button1.on_click(|event, state| {
        state[h].mode = Mode.Edit
    })

    button2.on_click(|event, state| {
        state[h].mode = Mode.View
    })

    button3.on_click(|event, state| {
        try state[h].save()
    })

    app.run()  // Framework calls closures with state parameter
}
```

**Multiple closures sharing state:**

<!-- test: skip -->
```rask
const state = Pool.new()
const h = state.insert(AppData{...})

// All closures capture same handle, receive state as parameter
button1.on_click(|_, s| s[h].mode = Mode.A)
button2.on_click(|_, s| s[h].mode = Mode.B)
button3.on_click(|_, s| try s[h].save())

app.run_with_state(state)
```

**Choosing between closure kinds:**

| Scenario | Kind | Pattern |
|----------|------|---------|
| Iterator adapter | Inline | `items.filter(\|i\| vec[*i].active)` |
| Event handler (run now) | Inline | `(try btn.on_click(\|e\| app.x += 1)).execute_now()` |
| Event handler (stored) | Stored + params | `btn.on_click(\|e, app\| app[h].x += 1)` |
| Async callback | Stored + params | `task.then(\|result, state\| state[h] = result)` |
| Pure transformation | Either | `\|x\| x * 2` (no outer access) |
| Closure with field borrow (same scope) | Scoped | `const f = \|\| process(entity.name); f()` |

### IDE Integration

| Closure Kind | Ghost Annotation |
|--------------|------------------|
| Stored | `[stored]` |
| Scoped | `[scoped to line N]` |
| Inline | `[inline]` |

On hover over a closure, show captures:

```
Closure captures:
  name: borrowed from 'entity' (line 3) -> makes closure scoped
  count: copied (i32, 4 bytes)

Kind: Scoped (to line 3)
```

When the cursor is in a scoped closure, the IDE highlights the block boundary it's limited to.

| Error | Quick Fix |
|-------|-----------|
| Inline closure stored | "Use inline" / "Clone captured values" |
| Scoped closure escapes | "Move declaration into scope" / "Clone to owned" |
| Generic stores scoped closure | "Use non-storing alternative" |

### See Also

- [Value Semantics](value-semantics.md) -- Copy vs move for captured values (`mem.value`)
- [Borrowing](borrowing.md) -- Block-scoped (struct fields) and statement-scoped (buffers) access (`mem.borrowing`)
- [Pools](pools.md) -- Pool+Handle pattern for shared mutable state (`mem.pools`)
- [Concurrency](../concurrency/sync.md) -- Closures sent cross-task must be storable (`conc.sync`)
