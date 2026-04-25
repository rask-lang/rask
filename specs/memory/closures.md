<!-- id: mem.closures -->
<!-- status: decided -->
<!-- summary: Two modes — |x| expr borrows outer scope (scope-limited), own |x| expr moves/copies (self-contained) -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md, memory/pools.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-ownership/ -->

# Closures

Two modes, one keyword:

```rask
|x| expr        // scope-limited: borrows outer variables, can't outlive their scope
own |x| expr    // owned: moves/copies outer variables, self-contained
```

The `own` prefix is the explicit opt-in to move-capture. Without it, closures borrow.

## Capture rules

| Mode | Non-Copy captures | Copy captures | Can escape scope? |
|------|-------------------|---------------|-------------------|
| `\|x\| expr` | Borrowed (source stays valid) | Copied | No |
| `own \|x\| expr` | Moved (source consumed) | Copied | Yes |

```rask
const tags = get_tags()  // Vec<string>

// Borrows tags — tags still valid after the call
filter_vec(items, |item| tags.contains(item.tag))
print(tags.len())  // OK

// Moves tags — tags consumed
const f = own |entry: Entry| -> bool { return tags.contains(entry.tag) }
print(tags.len())  // ERROR: tags moved into closure
```

No inference, no context-dependence. The `own` prefix is visible at the use site.

## When to use own

Use `own` when the closure needs to outlive its creation scope — returned from a function,
stored in a struct, sent to another task:

```rask
func make_filter(tags: Vec<string>) -> |Entry| -> bool {
    return own |entry: Entry| -> bool { return tags.contains(entry.tag) }
}
```

Without `own`, the closure can't escape (the compiler rejects it at the store/return point).
This matches the existing scope-limited closure rules (SL1-SL2).

## Closure parameters

Parameters are independent of capture mode. Both closure modes use the same parameter syntax.

| Rule | Description |
|------|-------------|
| **CP1: Borrow by default** | `\|x\|` binds parameter `x` by read-only borrow |
| **CP2: Mutable parameter with explicit type** | `\|mutate x: T\|` binds parameter `x` by mutable borrow. Explicit type required to distinguish from mutable-capture syntax |
| **CP3: No untyped mutable parameter** | `\|mutate x\|` without a type is always mutable-capture syntax, never a parameter |
| **CP4: No take parameter** | Closures cannot take ownership via a parameter. Use a standalone function |

```rask
// Borrow parameter (default)
const print_name = |u: User| print(u.name)

// Mutable-borrow parameter (explicit type required)
const grow = |mutate item: Item| { item.level += 1 }
```

**Return semantics:** `return` inside a closure exits the closure, not the enclosing function
(`ctrl.flow/CF26`). Expression-bodied closures implicitly return their expression; block-bodied
closures require explicit `return`.

```rask
const double = |x| x * 2          // implicit return

const parse = |s| {
    if s == "" { return none }
    return parse_inner(s)
}
```

## Mutable capture

Closures can borrow mutable locals with explicit `mutate` in the capture list. Works the same
for both scope-limited and owned closures, though owned closures with mutable captures are
unusual (mutate implies the closure needs a live reference, which conflicts with escaping).

| Rule | Description |
|------|-------------|
| **MC1: Explicit annotation** | `mutate var` in closure capture list declares mutable borrow |
| **MC2: Exclusive access** | While a mutable capture exists, no other access to the variable |
| **MC3: Scope-limited** | Closure can't outlive the captured variable |
| **MC4: See mutations** | Caller sees mutations after closure completes |

```rask
mut count = 0
const inc = |mutate count| { count += 1 }
inc()
inc()
// count == 2

// Iterator example
mut total = 0
items.for_each(|item, mutate total| { total += item.value })
print(total)  // sees accumulated value
```

Without `mutate`, a captured variable is borrowed (scope-limited) or moved (own). Mutation
inside the closure stays inside:

```rask
mut count = 0
const inc = || { count += 1 }  // borrows count read-only; mutation not visible
inc()
// count is still 0
```

**Multiple closures can't share a mutable capture:**

```rask
mut x = 0
const a = |mutate x| { x += 1 }
const b = |mutate x| { x += 2 }  // ERROR: x already mutably captured by a
```

Use `Cell<T>` or Pool+Handle for shared mutable state across multiple closures.

## Scope-limited closures (non-own)

All non-`own` closures are scope-limited. They cannot escape the scope where their borrows live.

| Rule | Description |
|------|-------------|
| **SL1: Scope inheritance** | Closure is limited to the scope of its outermost captured variable |
| **SL2: No escape** | Cannot return, store in struct, or send cross-task |

```rask
const tags = get_tags()
const f = || process(tags)   // f inherits tags' scope
f()                           // OK: called in same scope
return f                      // ERROR: cannot escape scope
```

```rask
// ERROR: closure outlives the binding it captures
mut outer_closure
{
    const tags = get_tags()
    outer_closure = || process(tags)  // ERROR: outer_closure outlives tags
}
```

**Fix: use own and move the value in:**

```rask
const tags = get_tags()
const f = own || process(tags)  // tags moved into closure
return f                         // OK: self-contained
```

## Generic propagation

Functions don't need to declare whether they store or consume closures. The constraint propagates
through generics, and violations surface where storage is attempted.

| Rule | Description |
|------|-------------|
| **GP1: Constraint propagation** | Scope constraints propagate through generic type parameters when the compiler generates specialized code |

```rask
func run_twice<F: Fn()>(f: F) {
    f()
    f()
}  // F dropped, never stored — works with scope-limited closures

func store_callback<F: Fn()>(f: F) {
    const holder = Holder { callback: f }  // ERROR if F is scope-limited
}

const tags = get_tags()
const greet = || print(tags)   // scope-limited

run_twice(greet)               // OK: run_twice doesn't store F
store_callback(greet)          // ERROR: store_callback tries to store F
```

## spawn

`spawn` requires owned closures. The existing syntax works:

```rask
spawn(own || {
    vec.push(1)  // OK: task owns vec
})
```

A scope-limited closure passed to `spawn` is a compile error — the task could outlive the
spawning scope.

## Error messages

**Scope-limited closure escapes [SL2]:**
```
ERROR [mem.closures/SL2]: closure cannot escape scope
   |
3  |  const tags = get_tags()
   |               ^^^^^^^^^^^ borrowed from outer scope (line 3)
4  |  const f = || process(tags)
   |            ^^^^^^^^^^^^^^^^^ closure captures scoped variable
5  |  return f
   |  ^^^^^^^^ cannot escape scope where 'tags' lives

FIX: capture by value with own:

  const f = own || process(tags)
  return f                          // OK: tags moved into closure
```

**Owned closure used where scope-limited expected — rarely an error. The reverse:**

```
ERROR [mem.closures/SL2]: scope-limited closure passed to function that stores it
   |
5  |  store_callback(greet)
   |  ^^^^^^^^^^^^^^^^^^^^^ 'greet' is scope-limited (borrows 'tags')
   |                        but 'store_callback' stores its argument

FIX: use own closure:

  const greet = own || print(tags.clone())
  store_callback(greet)
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

## Edge cases

| Case | Handling |
|------|----------|
| `own` closure captures Copy type | Value copied (same as non-own) |
| `own` closure captures move-only type | Type moved into closure, source invalid |
| `own` closure captures resource type | Resource consumed by closure; must be used within or returned |
| Non-`own` closure captures resource type | Resource borrowed; can't escape scope |
| Nested closures | Each level borrows/moves from its immediate outer scope |
| Pure closure (no captures) | Self-contained either way; `own` is redundant but allowed |
| `mutate` capture of Copy type | Borrows mutably (not copied), mutations visible to caller |

---

## Implementation

### Capture semantics

`own` closures move non-Copy values into the closure environment block. The source variable is
marked consumed by the ownership checker.

Non-`own` closures borrow. The ownership checker records a shared borrow on each captured
variable; the source stays valid. At the MIR level, the closure environment currently holds
copies of the values (the borrow is enforced by scope-limiting, not by pointer indirection).
True reference-based capture is a planned optimization.

### Closure block layout

```
[func_ptr (8 bytes) | captured_var_0 | captured_var_1 | ...]
```

The closure value is a pointer to this block. `closure_ptr + 8` is the environment pointer —
implicit first argument to the closure function.

### Heap vs. stack

`own` closures start as heap-allocated. A per-function pass downgrades non-escaping ones to
stack allocation. Non-`own` closures are always stack-allocated (they can't escape by contract).

| Closure kind | Initial allocation | Can be downgraded? |
|---|---|---|
| `own` | Heap | Yes, if provably non-escaping |
| Non-`own` (scope-limited) | Stack | N/A — never heap |

---

## Appendix (non-normative)

### Rationale

**Why explicit own rather than inference?** An earlier design inferred capture mode from context
— inline closures borrow, stored closures move. The same `|x| ...` syntax had different
semantics depending on how the closure was used, which the developer couldn't see at the closure
site. Extracting a closure to name it would silently change ownership. `own` makes the intent
visible where it matters — at the closure literal — and the rule is unconditional: `own` moves,
no `own` borrows.

**Consistency with spawn.** `spawn(own || {...})` already required `own` to communicate that the
task takes ownership of its captures. Extending `own` to all closures unifies the rule.

### Patterns & guidance

| Scenario | Pattern |
|----------|---------|
| Iterator adapter | `items.filter(\|i\| condition)` (borrows, scope-limited) |
| Simple callback | `\|x\| x * 2` (pure, no captures) |
| Callback with context | `own \|event\| process(context, event)` (moves context) |
| Mutating a local | `\|mutate count\| count += 1` (mutable capture) |
| Shared mutable state (multiple closures) | `Cell<T>` or Pool+Handle |
| Callback stored for later | `own \|...\|` — capture owned values |

**Cell<T> for shared mutable state:**

```rask
const counter = Cell.new(0)

button1.on_click(own |event| {
    with counter as c { c += 1 }
})
button2.on_click(own |event| {
    with counter as c { c += 10 }
})
```

### IDE integration

| Context | Ghost annotation |
|---------|------------------|
| Non-`own` closure, no captures | `[inline]` |
| Non-`own` closure with borrows | `[borrows: name, other]` |
| `own` closure with copies | `[copies: name (i32)]` |
| `own` closure with moves | `[moves: name (Vec<string>)]` |
| Mutable capture | `[mutate: count]` |

### See also

- [Value Semantics](value-semantics.md) — Copy vs move (`mem.value`)
- [Borrowing](borrowing.md) — Block-scoped views and `with`-based access (`mem.borrowing`)
- [Boxes](boxes.md) — Cell and Pool as containers for shared mutable state (`mem.boxes`)
- [Cell](cell.md) — Single-value mutable container (`mem.cell`)
- [Pools](pools.md) — Pool+Handle pattern for shared mutable state (`mem.pools`)
- [Linearity](linear.md) — Closures capturing linear values must consume them (`mem.linear`)
- [Owned Pointers](owned.md) — Moving an `Owned<T>` into a closure consumes it (`mem.owned`)
- [Concurrency](../concurrency/sync.md) — Closures sent cross-task must use `own` (`conc.sync`)
