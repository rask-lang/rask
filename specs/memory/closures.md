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
let tags = get_tags()  // Vec<string>

// Borrows tags — tags still valid after the call
filter_vec(items, |item| tags.contains(item.tag))
print(tags.len())  // OK

// Moves tags — tags consumed
let f = own |entry: Entry| -> bool { return tags.contains(entry.tag) }
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
| **CP2: Mutable parameter** | `\|mutate x: T\|` binds parameter `x` by mutable borrow. The type is required for the same reason a public function's is — this parameter writes back to the caller, so the shape it writes gets named |
| **CP3: Only parameters live in the pipes** | Everything in `\|…\|` is a parameter. Captures never appear there — they're inferred (MC1) — so there is nothing for a reader to disambiguate |
| **CP4: No take parameter** | Closures cannot take ownership via a parameter. Use a standalone function |

```rask
// Borrow parameter (default)
let print_name = |u: User| print(u.name)

// Mutable-borrow parameter (explicit type required)
let grow = |mutate item: Item| { item.level += 1 }
```

**Return semantics:** `return` inside a closure exits the closure, not the enclosing function
(`ctrl.flow/CF26`). Expression-bodied closures implicitly return their expression; block-bodied
closures require explicit `return`.

```rask
let double = |x| x * 2          // implicit return

let parse = |s| {
    if s == "" { return none }
    return parse_inner(s)
}
```

## Mutable capture

A closure that writes an enclosing local mutably borrows it. Nobody writes that down — it's
inferred from the body, exactly as a read capture already is.

| Rule | Description |
|------|-------------|
| **MC1: Inferred from use** | A closure's captures are inferred: read the variable and it's borrowed, write it and it's borrowed mutably. There is no capture list and no `mutate` annotation on a capture |
| **MC2: Exclusive access** | While a mutable capture exists, no other access to the variable |
| **MC3: Scope-limited** | Closure can't outlive the captured variable |
| **MC4: See mutations** | Caller sees mutations after closure completes |
```rask
mut total = 0
let add = |x| { total = total + x }   // `total` captured mutably, inferred
add(5)
add(3)
// total == 8
```

**Why inferred, when `ensure`, `take` and `mutate`-on-a-parameter are all explicit.** Those three
are visible because each one costs something or changes what the caller may do afterwards: `take`
kills the variable, a `mutate` parameter writes back through the call, `ensure` schedules code.
A mutable *borrow* capture does none of that. It's one pointer in an environment that's
stack-allocated when the closure doesn't escape — no allocation, no move, no clone, nothing the
caller has to know.

What it does buy is a safety guarantee (MC2, MC3), and that guarantee is mechanical: the compiler
enforces it whether or not you wrote a word. Principle 5 says where that kind of fact belongs —
"track effects, **captures**, and modes as metadata surfaced via tooling (IDE ghosts, lints)
instead of type-system constraints". An annotation the compiler doesn't need is an experience of
safety, and the goal is for safety to be a property instead.

The split that matters is already in this spec, one section up: read captures are inferred, and
`own` — the one that moves or clones — is a visible prefix. Requiring `mutate` on a capture was
the odd rule out, not the pattern.

**The desugar needs nothing special.** `for x in seq { total = total + x }` lowers to
`seq(|x| { total = total + x; return true })`. `total` is captured mutably by inference, like any
other closure. There is no capture list to emit, no mixed capture-and-parameter bracket to
design, and no exemption for compiler-generated code — an earlier draft invented one (an "MC5")
and it is not needed once captures are inferred.

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
3  |  let tags = get_tags()
   |               ^^^^^^^^^^^ borrowed from outer scope (line 3)
4  |  let f = || process(tags)
   |            ^^^^^^^^^^^^^^^^^ closure captures scoped variable
5  |  return f
   |  ^^^^^^^^ cannot escape scope where 'tags' lives

FIX: capture by value with own:

  let f = own || process(tags)
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

  let greet = own || print(tags.clone())
  store_callback(greet)
```

**Mutable capture conflict [MC2]:**
```
ERROR [mem.closures/MC2]: variable already mutably captured
   |
3  |  let a = |mutate x| { x += 1 }
   |             ^^^^^^^^^ x mutably captured here
4  |  let b = |mutate x| { x += 2 }
   |             ^^^^^^^^^ cannot capture x again

FIX: Use Shared<T> for shared mutable state:

  let x = Shared.new(0)
  let a = || x.modify(|v| v += 1)
  let b = || x.modify(|v| v += 2)
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
| Mutating a local | `\|x\| count += x` — the mutable capture is inferred (MC1) |
| Shared mutable state (multiple closures) | `Shared<T>` |
| Callback stored for later | `own \|...\|` — capture owned values |

**`Shared<T>` for shared mutable state:**

```rask
let counter = Shared.new(0)

button1.on_click(own |event| {
    with counter.write() as c { c += 1 }
})
button2.on_click(own |event| {
    with counter.write() as c { c += 10 }
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
- [Boxes](boxes.md) — The box family (`mem.boxes`)
- [Synchronization](../concurrency/sync.md) — `Shared<T, S>`, the single-value container (`conc.sync`)
- [Pools](pools.md) — Pool+Handle pattern for shared mutable state (`mem.pools`)
- [Linearity](linear.md) — Closures capturing linear values must consume them (`mem.linear`)
- [Owned Pointers](heap.md) — Moving an `Heap<T>` into a closure consumes it (`mem.heap`)
- [Concurrency](../concurrency/sync.md) — Closures sent cross-task must use `own` (`conc.sync`)
