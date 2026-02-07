# Solution: Closure Capture

## The Question
How do closures capture variables? Can closures mutate captured state? How do closures interact with borrowing?

## Decision
Two kinds:
1. **Storable closures** — Capture by value (copy/move), can be stored
2. **Expression-scoped closures** — Access outer scope directly, execute immediately

## Rationale
Capture-by-value makes closures self-contained and thread-safe. Pool+Handle gives you shared mutable state without mutable reference complexity. Expression-scoped closures enable iterator patterns.

## Mental Model: "Closures Are Suitcases"

Closures pack values to travel. Different closure kinds are like different types of luggage:

| Suitcase Type | What It Means | When It Applies |
|---------------|---------------|-----------------|
| **Backpack** (Storable) | Packs copies of everything. Can go anywhere. | Callbacks stored in structs, async tasks |
| **Hand-carry** (Immediate) | Doesn't pack anything—you're holding it directly. Must use now. | Iterator adapters, inline callbacks |
| **Day-trip bag** (Local-only) | Packs some borrowed items. Can't leave the area those items came from. | Closures with slices that stay in scope |

**The one-sentence rule:**
> "If you can pack it (copy/move), the closure can travel. If you're holding it (outer scope access), use it now. If it's borrowed, you can't leave the neighborhood."

### Why This Matters

| Mental Model | Closure Behavior |
|--------------|------------------|
| Packing = Capturing | Putting something in a suitcase means copying it |
| Holding = Accessing | Hand-carry means you're still connected to the source |
| Borrowed items = Travel limits | A day-trip bag with borrowed items can't go farther than those items |

### What Kind of Closure Am I Creating?

The closure kind is determined by **how it's used**, not by what it captures:

```rask
START: Creating a closure || ... or |x| ...
         |
         v
    Is it used immediately in the same expression?
    (e.g., items.filter(|x| ...).collect())
         |
    YES  |  NO
         |    |
         v    v
    IMMEDIATE  Is it stored/returned/assigned to variable?
    (hand-carry)     |
                     v
               YES → Does it capture any borrows (slices, references)?
                     |
                YES  |  NO
                     |   |
                     v   v
               LOCAL-ONLY  STORABLE
               (day-trip)  (backpack)
```

## Specification

### Closure Kinds

| Closure Kind | Capture Mode | Storage | Use Cases |
|--------------|--------------|---------|-----------|
| **Storable** (backpack) | By value (copy/move) | Can be stored, returned | Callbacks, stored handlers, async tasks |
| **Immediate** (hand-carry) | Access outer scope | MUST be called immediately | Iterator adapters, immediate execution |
| **Local-only** (day-trip) | By value + borrows | Scoped to borrow source | Closures with slices, temporary references |

### Storable Closures (Backpack)

Capture by value (copy or move), never by reference. Can be stored in variables, structs, or returned. Think of it as packing a backpack—you copy what you need, and the backpack can travel anywhere.

| Capture type | Behavior |
|--------------|----------|
| Small Copy types | Copied into closure |
| Large/non-Copy types | Moved into closure, source invalid |
| Block-scoped borrows | Closure becomes scope-constrained (see below) |
| Mutable state | Requires Pool + Handle pattern |

**Basic capture:**
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

**Closure parameters:**

Closures can accept parameters passed on each invocation:
<!-- test: skip -->
```rask
const double = |x| x * 2
const result = double(5)  // 10

const format_user = |id| "User #{id}"
button.on_click(|event| {  // event passed by caller
    print(format_user(event.user_id))
})
```

### The Capture Mutation Problem

**WRONG — Capturing mutable state directly:**
```rask
const counter = 0
const increment = || counter += 1  // Captures counter by COPY
increment()
increment()
// counter is still 0! Each call mutates the closure's COPY.
```

**CORRECT — Pool + Handle pattern:**
<!-- test: skip -->
```rask
const state = Pool.new()
const h = state.insert(Counter{value: 0})

// Pattern 1: Capture handle only, receive pool as parameter
const increment = |state_pool| state_pool[h].value += 1
increment(state)  // Pass pool on each call
increment(state)  // Still valid

// Pattern 2: Use parameters only (no capture)
const increment2 = |state_pool, handle| state_pool[handle].value += 1
increment2(state, h)

// Pattern 3: For stored closures, capture handle + pass pool
button.on_click(|event, app_state| {
    // Closure captures h, receives app_state as parameter
    app_state[h].clicks += 1
    app_state[h].last_event = event
})
```

**Canonical pattern for stateful callbacks:**
```rask
struct App {
    state: Pool<AppState>,
    state_handle: Handle<AppState>,
}

func setup_handlers(app: App) {
    const h = app.state_handle

    // Each handler captures its needed handles, receives app state
    button1.on_click(|event, state| {
        state[h].mode = Mode.Edit
    })

    button2.on_click(|event, state| {
        state[h].mode = Mode.View
    })

    button3.on_click(|event, state| {
        try state[h].save()
    })

    // Caller provides state when executing
    app.run()  // Framework calls closures with state parameter
}
```

### Local-Only Closures (Day-Trip Bag)

Closures can capture block-scoped borrows (slices, struct field references) but MUST NOT escape the borrow's scope. This is enforced through **local-only closure types** (scope-constrained), not escape analysis. Think of it as a day-trip bag—you can pack borrowed items, but you can't take them farther than their owner allows.

**Mechanism:**

| Step | What Happens |
|------|--------------|
| 1. Borrow creation | `s[0..3]` creates a slice type with implicit scope marker (tied to `s`'s scope) |
| 2. Closure creation | Compiler analyzes captures; if any have scope markers, closure becomes scope-constrained |
| 3. Assignment check | Scope-constrained closures MUST be assigned to variables in the same or inner scope |
| 4. Return/store check | Scope-constrained closures MUST NOT be returned, stored in structs, or sent cross-task |

**Why this is local analysis (Principle 5):**

- Scope markers are determined at borrow creation (single statement)
- Capture analysis only examines the closure body (single expression)
- Constraint checking happens at each usage site (single assignment/return)
- No cross-function tracking or escape analysis required

**Rules:**

| Rule | Description |
|------|-------------|
| **BC1: Scope inheritance** | Closure inherits the innermost scope constraint from all captured borrows |
| **BC2: Inner scope OK** | Can assign to variables in same or inner scope as the borrow source |
| **BC3: Outer scope forbidden** | Cannot assign to variable declared before borrow source |
| **BC4: No escape** | Cannot return, store in struct, or send cross-task |
| **BC5: Generic propagation** | Scope constraints propagate through generics; functions that don't escape work automatically |

**Examples:**

```rask
const s = get_string()
const slice = s[0..3]               // slice is scope-constrained to s's scope
const f = || process(slice)         // ✅ OK: f inherits scope constraint
f()                               // ✅ OK: called in same scope
return f                          // ❌ ERROR: cannot escape scope (BC4)
```

**Assigning to outer variable:**
```rask
let outer_closure: ???
{
    const s = "hello"
    const slice = s[0..3]
    outer_closure = || process(slice)  // ❌ ERROR: outer_closure outlives s (BC3)
}
```

**Storing in struct:**
```rask
const slice = s[0..3]
const f = || process(slice)
const handler = Handler { callback: f }  // ❌ ERROR: struct fields cannot hold scope-constrained closures (BC4)
```

**Passing to immediate consumer:**
```rask
const slice = s[0..3]
const f = || process(slice)
execute_now(f)                    // ✅ OK: execute_now consumes f immediately (BC5)
// f is moved to execute_now, not stored
```

**Scope constraints in generics (BC5):**

Scope constraints propagate through generic type parameters. No special annotations needed—the type system handles it at monomorphization:

```rask
func run_twice<F: Fn()>(f: F) {
    f()
    f()
}  // F dropped, never stored - works with scope-constrained closures

func store_callback<F: Fn()>(f: F) {
    const holder = Holder { callback: f }  // ❌ ERROR if F is scope-constrained (BC4)
}

const slice = s[0..3]
const greet = || print(slice)   // scope-constrained

run_twice(greet)              // ✅ Works - run_twice doesn't store F
store_callback(greet)         // ❌ Fails - store_callback tries to store F
```

When `run_twice` is monomorphized with `greet`'s type:
- F inherits the scope constraint from `greet`
- The function body is checked with that constraint
- Since `run_twice` only calls F (doesn't store it), no BC violation occurs

When `store_callback` is monomorphized:
- Same F with scope constraint
- `Holder { callback: f }` violates BC4 (struct fields can't hold scope-constrained types)
- Compile error in `store_callback`, not at call site

**Key insight:** Functions don't need to declare "I execute immediately" vs "I store." The constraint propagates, and violations surface where storage is attempted.

**IDE Support (Principle 7):**

The IDE SHOULD display scope constraints as ghost annotations:
```rask
const f = || process(slice)  // ghost: [scoped to line 42]
```

**Interaction with closure kinds:**

| Closure Kind | Can capture borrows? | Constraint |
|--------------|---------------------|------------|
| Storable (backpack) | No | Can be stored, returned |
| Local-only (day-trip) | Yes | Subject to BC1-BC5 |
| Immediate (hand-carry) | N/A (accesses, doesn't capture) | Must execute immediately |

A storable closure that captures a borrow becomes local-only. It's still "storable" in the sense that it can be assigned to a local variable, but it cannot escape the borrow's scope—like a day-trip bag that can't leave the neighborhood.

### Immediate Closures (Hand-Carry)

Access outer scope WITHOUT capturing. MUST be called immediately within the expression. Think of it as carrying something in your hands—you're directly connected to the source, so you must use it now.

| Rule | Description |
|------|-------------|
| **EC1: No capture** | Closure accesses outer scope directly |
| **EC2: Immediate execution** | Must be called before expression completes |
| **EC3: Cannot store** | Compile error if assigned to variable or returned |
| **EC4: Aliasing rules apply** | Mutable access excludes other access during execution |

**Read access (iterators):**
```rask
const items = vec![...]
const vec = vec![...]

// Closure accesses vec WITHOUT capturing it
items.filter(|i| vec[*i].active)
     .map(|i| vec[*i].value * 2)
     .collect()
// vec still valid after chain
```

**Mutable access (immediate callbacks):**
```rask
const app = AppState.new()

// Expression-scoped: closure executed immediately
(try button.on_click(|event| {
    app.counter += 1  // Mutates app directly
    app.last_click = event.timestamp
})).execute_now()  // Must execute in same expression

// app still valid here

// ❌ ILLEGAL: Cannot store expression-scoped closure
const handler = button.on_click(|event| {
    app.counter += 1  // ERROR: captures mutable access to app
})
// Would violate aliasing - app borrowed while handler exists
```

**Storage detection:**

Compiler enforces immediate execution:
```rask
// ✅ Legal: Inline consumption
for i in items.filter(|i| vec[*i].active) {
    process(i)
}

// ❌ Illegal: Stored closure accesses outer scope
const f = items.filter(|i| vec[*i].active)
//          ^^^^^^^^^ ERROR: closure accesses 'vec' but iterator is stored
```

### Choosing Between Closure Kinds

| Scenario | Use | Pattern |
|----------|-----|---------|
| Iterator adapter | Immediate | `items.filter(\|i\| vec[*i].active)` |
| Event handler (run now) | Immediate | `(try btn.on_click(\|e\| app.x += 1)).execute_now()` |
| Event handler (stored) | Storable + params | `btn.on_click(\|e, app\| app[h].x += 1)` |
| Async callback | Storable + params | `task.then(\|result, state\| state[h] = result)` |
| Pure transformation | Either | `\|x\| x * 2` (no outer access) |
| Multi-state mutation | Storable + Pool/Handle | Capture handles, receive pools as params |
| Closure with slice (same scope) | Local-only | `let f = \|\| process(slice); f()` |

### Multiple Closures Sharing State

Use Pool + Handle pattern:
```rask
const app = AppState.new()
const state = Pool.new()
const h = state.insert(AppData{...})

// All closures capture same handle, receive state as parameter
button1.on_click(|_, s| s[h].mode = Mode.A)
button2.on_click(|_, s| s[h].mode = Mode.B)
button3.on_click(|_, s| try s[h].save())

// Framework provides state to all handlers
app.run_with_state(state)
```

## Error Messages

Error messages MUST teach the mental model and suggest concrete fixes. Each error should include:
- **BECAUSE:** Explanation using the suitcase mental model
- **FIX:** Concrete code alternatives

### Error 1: Immediate closure stored

**Trigger:** Assigning an immediate closure (that accesses outer scope) to a variable.

```
ERROR: Hand-carry closure cannot be stored in a suitcase
   |
5  |  let f = items.filter(|i| vec[*i].active)
   |          ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^ This closure holds 'vec' directly
   |                                           (doesn't pack a copy)

BECAUSE: You're trying to store a closure that accesses 'vec' without
packing it. Like carrying something in your hands—you can't put your
hands in a suitcase.

FIX 1: Use the closure immediately (keep holding it):
   |
5  |  items.filter(|i| vec[*i].active).collect()
   |                                  ^^^^^^^^^^^ Used in same expression

FIX 2: Pack what you need (capture by value):
   |
5  |  let active_set: Set<usize> = items.iter()
   |      .filter(|i| vec[*i].active).collect()
   |  let f = items.filter(|i| active_set.contains(i))
```

### Error 2: Local-only closure escapes (BC4)

**Trigger:** Returning, storing in struct, or sending a local-only closure cross-task.

```
ERROR: Day-trip closure cannot leave the neighborhood
   |
3  |  let slice = s[0..3]
   |              ^^^^^^^ 'slice' is borrowed from 's' (line 2)
4  |  let f = || process(slice)
   |      ^ This closure packed a borrowed item
5  |  return f
   |  ^^^^^^^^ Cannot leave scope where 's' lives

BECAUSE: Your closure contains 'slice', which is borrowed from 's'.
Returning the closure would let 'slice' outlive 's' (use-after-free).
Think of it as a day-trip bag—you can't take borrowed items home.

FIX 1: Don't return (use in same scope):
   |
5  |  f()  // Use closure here instead of returning

FIX 2: Pack owned data instead of borrowed:
   |
3  |  let owned = s[0..3].to_string()  // Copy the slice
4  |  let f = || process(owned)        // Packs owned value
5  |  return f                          // ✅ OK: no borrowed items
```

### Error 3: Borrow captured in outer scope (BC3)

**Trigger:** Assigning a closure with borrows to a variable declared in an outer scope.

```
ERROR: Closure packed a borrowed item into a bag that lives too long
   |
1  |  let outer_closure;
   |      ^^^^^^^^^^^^^ This bag was created here (outer scope)
2  |  {
3  |      let s = "hello"
4  |      let slice = s[0..3]
5  |      outer_closure = || process(slice)
   |                      ^^^^^^^^^^^^^^^^^^ Trying to pack 'slice' (borrowed from 's')
6  |  }
   |   ^ But 's' dies here

BECAUSE: 'outer_closure' lives longer than 's', but you're trying to
pack 'slice' (borrowed from 's') into it. The borrowed item would
become invalid when 's' dies.

FIX: Create the bag inside the scope:
   |
2  |  {
3  |      let s = "hello"
4  |      let slice = s[0..3]
5  |      let inner_closure = || process(slice)  // Bag in same scope
6  |      inner_closure()  // Use it here
7  |  }
```

### Error 4: Generic function stores local-only closure (BC5)

**Trigger:** Passing a local-only closure to a generic function that stores its argument.

```
ERROR: Function 'store_callback' tries to store a day-trip closure
   |
10 |  store_callback(greet)
   |                 ^^^^^ This closure packed borrowed 'slice' (line 8)

Note: 'store_callback' stores its argument in a struct:
   |
// in store_callback<F>:
3  |  let holder = Holder { callback: f }
   |                        ^^^^^^^^^^^^ Storage requires a backpack,
   |                                     but you passed a day-trip bag

BECAUSE: 'greet' contains borrowed data and cannot be stored.
Functions that store closures need closures that "travel freely"
(no borrowed items).

FIX 1: Use a function that doesn't store:
   |
10 |  run_twice(greet)  // This just calls f(), doesn't store it

FIX 2: Remove the borrow from your closure:
   |
8  |  let owned = s[0..3].to_string()
9  |  let greet = || print(owned)  // Now a regular backpack
10 |  store_callback(greet)         // ✅ OK
```

## IDE Integration

The IDE makes closure kinds and capture behavior visible through ghost annotations, reducing cognitive load.

### Ghost Annotations

| Closure Kind | Ghost Annotation |
|--------------|------------------|
| Storable (backpack) | `[storable]` |
| Local-only (day-trip) | `[local-only, scoped to line N]` |
| Immediate (hand-carry) | `[immediate]` |

**Example:**
```rask
const greet = || print(slice)  // [local-only, scoped to line 5]
```

### Capture List Display (Hover)

On hover over a closure, show what it captures:

```rask
const f = || process(slice, count)
    ^
    Closure captures:
      slice: borrowed from 's' (line 3) → makes closure local-only
      count: copied (i32, 4 bytes)

    Kind: Local-only (scoped to line 3)
```

### Scope Boundary Visualization

When cursor is in a local-only closure, highlight the scope boundary it's constrained to:

```rask
const s = "hello"        // ┐
const slice = s[0..3]    // │ highlighted: closure scope boundary
const f = || print(slice)// │
use(f)                 // │
                       // ┘ f cannot escape past here
```

### Quick Fix Actions

| Error | Quick Fix |
|-------|-----------|
| Immediate closure stored | "Use immediately" / "Clone captured values" |
| Local-only closure escapes | "Move declaration into scope" / "Clone to owned" |
| BC5 generic error | "Use non-storing alternative" |

## Summary

| Question | Answer |
|----------|--------|
| Can closures mutate outer variables? | No (capture is by copy/move) |
| How to share mutable state? | Pool + Handle, pass pool as parameter |
| Can closures accept parameters? | Yes, passed on each call |
| Can closures access outer scope mutably? | Yes, if immediate (hand-carry) |
| Can I store a closure that mutates outer scope? | No (must be immediate = use now only) |
| Event handler pattern? | Storable (backpack): capture handles, receive state as param |
| Iterator pattern? | Immediate (hand-carry): access outer scope, execute immediately |
| Closure with borrowed slice? | Local-only (day-trip): stays in scope, or clone to owned |

## Edge Cases

| Case | Handling |
|------|----------|
| Closure captures move-only type | Type moved into closure, source invalid |
| Closure captures linear type | Linear must be consumed within closure or transferred out |
| Nested closures | Each level captures from its immediate outer scope |
| Async closure | Treated as storable; captured values moved in |
| Clone inside closure | Creates independent copy |

## Integration Notes

- **Value Semantics:** Capture follows copy/move rules (see [value-semantics.md](value-semantics.md))
- **Borrowing:** Immediate closures respect borrowing rules (see [borrowing.md](borrowing.md))
- **Pools:** Pool+Handle pattern for shared mutable state (see [pools.md](pools.md))
- **Concurrency:** Closures sent cross-task must be storable (backpack) (see [sync.md](../concurrency/sync.md))
- **Tooling:** IDE shows capture list, scope constraints as ghost annotations

## See Also

- [Value Semantics](value-semantics.md) — Copy vs move for captured values
- [Borrowing](borrowing.md) — One rule: views last as long as the source is stable
- [Pools](pools.md) — Pool+Handle pattern for state sharing
