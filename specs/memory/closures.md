<!-- id: mem.closures -->
<!-- status: decided -->
<!-- summary: Two modes — |x| expr borrows outer scope (scope-limited), own |x| expr moves/copies (self-contained) -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md -->
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
| `\|x\| expr` | Borrowed (source stays valid) | Borrowed | Only as far as its borrow lives (MC3) |
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

A scope-limited closure borrows a Copy capture rather than copying it. That's
what makes MC1's inference mean anything: `mut total = 0; let add = |x| { total
= total + x }` has to reach the caller's `total`, and an `i32` is Copy. Copy
decides what happens when a value *escapes* — which is why `own` copies it —
not whether a borrow is a borrow.

What isn't inferred is the mode. The `own` prefix is visible at the use site,
and it's the only thing that changes which of these two rows applies.

## When to use own

Use `own` when the closure needs to outlive its creation scope — returned from a function,
stored in a struct, sent to another task:

```rask
func make_filter(tags: Vec<string>) -> |Entry| -> bool {
    return own |entry: Entry| -> bool { return tags.contains(entry.tag) }
}
```

Without `own`, a closure can still escape — it just can't outlive what it borrowed (MC3).

## What a returned closure may capture

| Rule | Description |
|------|-------------|
| **SL3: Parameters, not locals** | A non-`own` closure that leaves the function may capture the function's *lent* parameters — `param: T`, `mutate param: T`, `self` — and not its locals or its `take` parameters. A lent parameter is the caller's and is still there when the call returns; a local and a `take` are the frame's, and the frame is going away |
| **SL4: The limit rides the return** | A call that answers a closure hands back whatever limits its borrowed arguments had. `make_filter(tags)` gives a closure that lives as long as `tags` does. A `take` argument contributes no limit — it was given away, and handing a scope-limited closure to a `take` parameter is the MC3 error instead |

SL3 is the difference between these two:

```rask
func logging(next: |Request| -> Response) -> |Request| -> Response {
    return |request| { … next(request) … }   // fine — `next` is the caller's
}

func broken() -> |i64| -> i64 {
    let tags = get_tags()
    return |x| x + tags.len()                // error: `tags` dies here
}
```

and it is what the whole sequence protocol rests on — `vec.filter(|u| u.active)` is
`Vec.filter(self, pred) -> Sequence<T>` returning a closure over a borrowed receiver. Requiring
`own` there would cost every adapter chain a `take self`.

SL4 is why nobody writes a lifetime. The signature already says it: the return type is a
closure and the parameters say `take` or not, so the caller works out what the result is
limited to without reading the body. `type.sequence/SEQ26` is this rule under another name —
a sequence over a borrowed source is limited to that source, so it can be consumed in place
and not stored.

What makes SL4 sound is PM6: a borrowed parameter can't be given away or stored in an
aggregate, so the *only* way one reaches the caller again is the return value. There is
nowhere else for the borrow to go.

**Which means the signature has to be true.** SL4 reads parameter modes and nothing else —
there is no "and if the mode can't be determined, assume the worst" clause, because a rule
whose meaning depends on what the compiler managed to look up is not a rule. `spawn` is the
case that proves it: it is declared

```rask
public func spawn(take f: func() -> T) -> TaskHandle<T>
```

and the `take` is not decoration. The task keeps the closure and runs it after the call
returns, so a scope-limited closure handed to `spawn` is the MC3 error, and that falls out of
the signature rather than out of `spawn` being special. It said `f: func() -> T` for a long
time — a borrow — which is how `conc.tasks/T3` came to be enforced by a guess.

I had this written as a flat "scope-limited closures cannot escape", which is what SL1-SL2 were
originally drafted against. That was never what the compiler did, and it contradicted the escape
analysis described further down this page — escaping is exactly the case that gets a heap
environment. The rule is the lifetime, not the prefix.

## Closure parameters

Parameters are independent of capture mode. Both closure modes use the same parameter syntax.

| Rule | Description |
|------|-------------|
| **CP1: Borrow by default** | `\|x\|` binds parameter `x` by read-only borrow |
| **CP2: Mutable parameter** | `\|mutate x: T\|` binds parameter `x` by mutable borrow. The type is required for the same reason a public function's is — this parameter writes back to the caller, so the shape it writes gets named |
| **CP3: Only parameters live in the pipes** | Everything in `\|…\|` is a parameter. Captures never appear there — they're inferred (MC1) — so there is nothing for a reader to disambiguate |
| **CP4: No take parameter** | Closures cannot take ownership via a parameter. Use a standalone function |

<!-- test: parse -->
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
<!-- test: run | 8 -->
```rask
func main() {
    mut total = 0
    let add = |x| { total = total + x }   // `total` captured mutably, inferred
    add(5)
    add(3)
    println("{total}")                    // 8 — the capture is over, MC4
    return
}
```

**How long "exists" lasts.** Until the closure's last use, not until the end of the block.
MC4 is the reason: seeing the mutations is the whole point, so the read after the last call
has to work. A closure written inline — `v.filter(|x| { seen = seen + 1; return x > 1 })` —
dies at the end of its statement, so nothing else can overlap it and the rule never bites.

What MC2 rejects is two things reaching the variable at once:

<!-- test: compile-fail: ownership -->
```rask
func two_writers() {
    mut n = 0
    let a = || { n = n + 1 }
    let b = || { n = n + 2 }   // error: `n` is already captured for writing by `a`
    a()
    b()
}
```

and the same for a read or a write from outside between two calls. One closure that does both
jobs is usually the answer; `Shared` is the answer when they genuinely have to be separate.

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

**Scope-limited closure escapes [SL3]:**
```
ERROR [mem.closures/SL3]: closure cannot escape scope
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
ERROR [mem.closures/MC3]: scope-limited closure passed to function that stores it
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
3  |  let a = || { x += 1 }
   |               ^ x mutably captured here — the body writes it
4  |  let b = || { x += 2 }
   |               ^ cannot capture x again

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
| Non-`own` closure captures resource type | Resource borrowed; consuming it in the body is an error (E0891) |
| Nested closures | Each level borrows/moves from its immediate outer scope |
| Pure closure (no captures) | Self-contained either way; `own` is redundant but allowed |
| Mutable capture of a Copy type | Borrows mutably (not copied), mutations visible to caller |

The resource rows are the same rule as `mem.linear/L3` — a borrow isn't a
consumption — and there is a second reason for them here: nothing says how many
times a closure runs. A `close()` in the body of a plain closure is one
consumption to read and any number at runtime, so it has to be `own`, which
moves the resource in and leaves the outer binding with nothing to owe.

```rask
func twice(f: func()) { f() f() }

let c = Conn.open(1)
twice(|| { c.close() })         // error[E0891] — the closure borrowed `c`
twice(own || { c.close() })     // fine: `c` is the closure's now
```

---

## Implementation

### Capture semantics

`own` closures move non-Copy values into the closure environment block. The source variable is
marked consumed by the ownership checker.

Non-`own` closures borrow. The ownership checker records a shared borrow on each captured
variable; the source stays valid.

The environment slot is what makes the difference concrete, and there are three shapes of it:

| Capture | Slot holds | A write inside the body lands on |
|---|---|---|
| Non-`own` | The variable's address (8 bytes) | The creating frame's variable |
| `own` | The variable itself | The environment — so it survives to the next call |
| `spawn` | A copy | The task's own state, by construction |

The `own` row is the one that's easy to get wrong. Loading the value out at the top of the call
and working on the loaded copy reads correctly and throws every write away, so a counter closure
answers 1 however many times you call it. The environment *is* the variable's home once `own`
moved it there, so the body works through the slot's address for its whole life.

The block itself is owned like any other value: whoever is holding it when their
frame ends frees it. What that free doesn't yet do is release the captures inside
— an `own` closure holding a `Vec` frees the block and leaks the Vec (#1045).

### Closure block layout

```
[func_ptr (8 bytes) | captured_var_0 | captured_var_1 | ...]
```

The closure value is a pointer to this block. `closure_ptr + 8` is the environment pointer —
implicit first argument to the closure function.

### Heap vs. stack

Heap exactly when the closure outlives the frame that built it — returned, stored through a
pointer, or handed to something that keeps it. Everything else is a stack slot.

`own` is not the question, and treating it as one is what made a returned scope-limited closure
read a popped frame. A scope-limited closure *can* escape, by being returned; the escape analysis
decides, not the keyword.

| Escapes its frame? | Allocation | Freed by |
|---|---|---|
| No | Stack slot | The frame, on the way out |
| Yes | Heap, behind a size header | Whichever frame is still holding it when it ends |

The size header is there because the frame that frees a closure usually isn't the one that built
it: `let tick = counter()` hands the caller a block whose capture layout only `counter` knew.

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
- [Shared, Rack and Heap](shared-rack-heap.md) — The three that hand out scoped access (`mem.shared-rack-heap`)
- [Synchronization](../concurrency/sync.md) — `Shared<T, S>`, the single-value container (`conc.sync`)
- [Racks and Links](racks.md) — a reference that can live in a field, for graph-shaped state (`mem.racks`)
- [Linearity](linear.md) — Closures capturing linear values must consume them (`mem.linear`)
- [Heap Values](heap.md) — Moving an `Heap<T>` into a closure consumes it (`mem.heap`)
- [Concurrency](../concurrency/sync.md) — Closures sent cross-task must use `own` (`conc.sync`)
