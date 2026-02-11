<!-- id: mem.closures -->
<!-- status: decided -->
<!-- summary: Three closure kinds — storable (by value), immediate (outer scope), local-only (scoped borrows) -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md, memory/pools.md -->
<!-- implemented-by: compiler/crates/rask-types/ -->

# Closures

Closures capture by value (copy/move) for storage, access outer scope directly for immediate use, or capture scoped borrows for local-only use. Kind is determined by usage context, not by annotation.

## Closure Kinds

| Rule | Kind | Capture | Storage | Use Cases |
|------|------|---------|---------|-----------|
| **CL1: Storable** | Storable | By value (copy/move) | Can be stored, returned, sent cross-task | Callbacks, event handlers, async tasks |
| **CL2: Immediate** | Immediate | None (accesses outer scope) | Cannot store | Iterator adapters, inline callbacks |
| **CL3: Local-only** | Local-only | By value + borrows | Scoped to borrow source | Closures capturing slices, temporary refs |

Kind is inferred from context:
- Used inline in an expression chain &rarr; immediate
- Stored/returned, captures only owned values &rarr; storable
- Stored/assigned, captures borrows &rarr; local-only

## Storable Closures

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

**Capture mutation:** Storable closures capture by copy, so mutating a captured value only mutates the closure's copy. Use Pool + Handle for shared mutable state.

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

## Immediate Closures

Access outer scope directly without capturing. Must be called immediately within the expression.

| Rule | Description |
|------|-------------|
| **CL6: No capture** | Closure accesses outer scope directly, does not capture |
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

Mutable access with immediate execution:

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

// Illegal: stored closure accesses outer scope
const f = items.filter(|i| vec[*i].active)
//        ^^^^^^^^^ ERROR: closure accesses 'vec' but iterator is stored
```

## Local-Only Closures

Closures that capture block-scoped borrows (slices, struct field references). Must not escape the borrow's scope.

| Rule | Description |
|------|-------------|
| **CL4: Scope inheritance** | Closure inherits the innermost scope constraint from all captured borrows |
| **CL5: No escape** | Cannot return, store in struct, or send cross-task |

The compiler determines scope constraints through local analysis only:

| Step | What Happens |
|------|--------------|
| Borrow creation | `s[0..3]` creates a slice with implicit scope marker tied to `s` |
| Closure creation | Compiler analyzes captures; scope markers make the closure scope-constrained |
| Assignment check | Scope-constrained closures must be assigned in the same or inner scope |
| Return/store check | Scope-constrained closures cannot be returned, stored in structs, or sent cross-task |

<!-- test: skip -->
```rask
const s = get_string()
const slice = s[0..3]               // scope-constrained to s's scope
const f = || process(slice)         // f inherits scope constraint
f()                                 // OK: called in same scope
return f                            // ERROR: cannot escape scope (CL5)
```

Assigning to outer variable:

<!-- test: compile-fail -->
```rask
let outer_closure
{
    const s = "hello"
    const slice = s[0..3]
    outer_closure = || process(slice)  // ERROR: outer_closure outlives s (CL4)
}
```

Storing in struct:

<!-- test: compile-fail -->
```rask
const slice = s[0..3]
const f = || process(slice)
const handler = Handler { callback: f }  // ERROR: struct fields cannot hold scope-constrained closures (CL5)
```

Passing to immediate consumer:

<!-- test: skip -->
```rask
const slice = s[0..3]
const f = || process(slice)
execute_now(f)                    // OK: execute_now consumes f immediately
```

## Generic Propagation

| Rule | Description |
|------|-------------|
| **CL10: Constraint propagation** | Scope constraints propagate through generic type parameters at monomorphization |

No special annotations needed. Functions that don't store their generic closure argument work with scope-constrained closures automatically. Functions that store the argument produce a compile error at the storage site.

<!-- test: skip -->
```rask
func run_twice<F: Fn()>(f: F) {
    f()
    f()
}  // F dropped, never stored - works with scope-constrained closures

func store_callback<F: Fn()>(f: F) {
    const holder = Holder { callback: f }  // ERROR if F is scope-constrained (CL5)
}

const slice = s[0..3]
const greet = || print(slice)   // scope-constrained

run_twice(greet)              // OK: run_twice doesn't store F
store_callback(greet)         // ERROR: store_callback tries to store F
```

The error surfaces inside `store_callback` at the storage site, not at the call site. Functions don't need to declare whether they store or execute immediately.

## Error Messages

**Immediate closure stored [CL8]:**
```
ERROR [mem.closures/CL8]: immediate closure cannot be stored
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
5  |  let active_set: Set<usize> = items.iter()
   |      .filter(|i| vec[*i].active).collect()
   |  let f = items.filter(|i| active_set.contains(i))
```

**Local-only closure escapes [CL5]:**
```
ERROR [mem.closures/CL5]: scope-constrained closure cannot escape
   |
3  |  let slice = s[0..3]
   |              ^^^^^^^ borrowed from 's' (line 2)
4  |  let f = || process(slice)
   |          ^^^^^^^^^^^^^^^^^ closure captures scoped borrow
5  |  return f
   |  ^^^^^^^^ cannot escape scope where 's' lives

WHY: 'slice' is borrowed from 's'. Returning the closure would let
     'slice' outlive 's' (use-after-free).

FIX 1: Use in same scope:
   |
5  |  f()

FIX 2: Capture owned data instead:
   |
3  |  let owned = s[0..3].to_string()
4  |  let f = || process(owned)
5  |  return f                          // OK: no scoped borrows
```

**Scope-constrained closure assigned to outer variable [CL4]:**
```
ERROR [mem.closures/CL4]: closure outlives its captured borrow
   |
1  |  let outer_closure
   |      ^^^^^^^^^^^^^ declared here (outer scope)
3  |      let s = "hello"
4  |      let slice = s[0..3]
5  |      outer_closure = || process(slice)
   |                      ^^^^^^^^^^^^^^^^^ captures 'slice' (borrowed from 's')
6  |  }
   |   ^ 's' dropped here

WHY: 'outer_closure' lives longer than 's', but it captures 'slice'
     which borrows from 's'.

FIX: Create the closure inside the scope:
   |
3  |      let s = "hello"
4  |      let slice = s[0..3]
5  |      let inner_closure = || process(slice)
6  |      inner_closure()
```

**Generic function stores scope-constrained closure [CL10]:**
```
ERROR [mem.closures/CL10]: cannot store scope-constrained type
   |
// in store_callback<F>:
3  |  let holder = Holder { callback: f }
   |                        ^^^^^^^^^^^^ F is scope-constrained

Note: called with scope-constrained closure at:
10 |  store_callback(greet)
   |                 ^^^^^ 'greet' captures borrowed 'slice' (line 8)

FIX 1: Use a function that doesn't store:
   |
10 |  run_twice(greet)

FIX 2: Remove the borrow from the closure:
   |
8  |  let owned = s[0..3].to_string()
9  |  let greet = || print(owned)
10 |  store_callback(greet)         // OK
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Closure captures move-only type | CL1 | Type moved into closure, source invalid |
| Closure captures linear type | CL1 | Linear must be consumed within closure or transferred out |
| Nested closures | CL1 | Each level captures from its immediate outer scope |
| Async closure | CL1 | Treated as storable; captured values moved in |
| Clone inside closure | CL1 | Creates independent copy |
| Storable closure captures borrow | CL3 | Closure becomes local-only |
| Pure closure (no captures, no outer access) | CL1 | Storable by default |
| Mutable access in immediate closure | CL9 | Aliasing XOR mutation applies during execution |

---

## Appendix (non-normative)

### Rationale

**CL1 (storable capture by value):** I chose capture-by-value to make closures self-contained and thread-safe. The tradeoff is more `.clone()` calls and the Pool+Handle pattern for shared mutable state. I think that's better than reference-capturing closures that drag lifetime annotations into everything.

**CL2 (immediate closures):** Iterator chains like `.filter(|x| ...).map(|x| ...)` need access to surrounding scope without the overhead of capturing. Immediate closures give you this with the constraint that the closure can't escape the expression. This covers the most common closure pattern (iterator adapters) with zero ceremony.

**CL3–CL5 (local-only scope constraints):** A storable closure that happens to capture a borrow becomes scope-constrained. This is enforced through type-level scope markers, not escape analysis. All checks are local to the function — no cross-function tracking needed.

**CL10 (generic propagation):** Functions don't need to declare "I execute immediately" vs "I store." The constraint propagates through generics at monomorphization, and violations surface where storage is attempted. This keeps function signatures clean.

### Metaphor: Closures as Luggage

A useful way to think about the three closure kinds:

| Kind | Metaphor | Explanation |
|------|----------|-------------|
| Storable | Backpack | Packs copies of everything. Can go anywhere. |
| Immediate | Hand-carry | Holding items directly. Must use now. |
| Local-only | Day-trip bag | Packs some borrowed items. Can't leave the area those items came from. |

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
| Iterator adapter | Immediate | `items.filter(\|i\| vec[*i].active)` |
| Event handler (run now) | Immediate | `(try btn.on_click(\|e\| app.x += 1)).execute_now()` |
| Event handler (stored) | Storable + params | `btn.on_click(\|e, app\| app[h].x += 1)` |
| Async callback | Storable + params | `task.then(\|result, state\| state[h] = result)` |
| Pure transformation | Either | `\|x\| x * 2` (no outer access) |
| Closure with slice (same scope) | Local-only | `let f = \|\| process(slice); f()` |

### IDE Integration

| Closure Kind | Ghost Annotation |
|--------------|------------------|
| Storable | `[storable]` |
| Local-only | `[local-only, scoped to line N]` |
| Immediate | `[immediate]` |

On hover over a closure, show captures:

```
Closure captures:
  slice: borrowed from 's' (line 3) -> makes closure local-only
  count: copied (i32, 4 bytes)

Kind: Local-only (scoped to line 3)
```

When the cursor is in a local-only closure, the IDE highlights the scope boundary it is constrained to.

| Error | Quick Fix |
|-------|-----------|
| Immediate closure stored | "Use immediately" / "Clone captured values" |
| Local-only closure escapes | "Move declaration into scope" / "Clone to owned" |
| Generic stores scope-constrained | "Use non-storing alternative" |

### See Also

- [Value Semantics](value-semantics.md) -- Copy vs move for captured values (`mem.value`)
- [Borrowing](borrowing.md) -- Block-scoped views and expression-scoped access (`mem.borrowing`)
- [Pools](pools.md) -- Pool+Handle pattern for shared mutable state (`mem.pools`)
- [Concurrency](../concurrency/sync.md) -- Closures sent cross-task must be storable (`conc.sync`)
