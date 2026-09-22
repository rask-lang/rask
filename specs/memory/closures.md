<!-- summary: A closure that outlives its frame carries what it captured; one that stays points at it, and the compiler decides which -->
<!-- depends: memory/borrowing.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-ownership/ -->

# Closures

A closure either points at what it captured or carries it:

```rask
mut total = 0
let add = |x| { total = total + x }      // points at `total` — the write lands outside

func make_filter(tags: Vec<string>) -> func(Entry) -> bool {
    return |entry| { return tags.contains(entry.tag) }   // carries `tags`
}
```

There is no word for this, because there is never a choice. A closure that
outlives the frame that built it *must* carry — pointing at a frame that is gone
is the bug the rule exists to prevent — and one that stays *should* point, or
MC4's write-back has nowhere to land. One legal answer per literal, so the
compiler works it out.

| Rule | Description |
|------|-------------|
| **CM1: Outliving decides** | A closure carries its captures exactly when it outlives its frame: handed to a `take` parameter (which is where `spawn` lives), returned, or stored into a field. Everything else points |
| **CM2: Carrying consumes** | A carried non-Copy capture is moved, so the outer name is gone and a later use is the ordinary use-after-move error. A Copy capture is copied and the outer name is fine (VS1/VS2) |
| **CM3: Lent parameters are still borrowed** | A carrying closure can't move what the frame doesn't own. A `param: T`, `mutate param: T` or `self` belongs to the caller and is still there when the call returns, so it stays borrowed and SL4's limit rides the return |

## Capture rules

| Where the closure goes | Non-Copy captures | Copy captures |
|------|-------------------|---------------|
| Stays in its frame | Borrowed (source stays valid) | Borrowed — the write comes back (MC4) |
| Outlives it | Moved (source consumed) | Copied |

```rask
let tags = get_tags()  // Vec<string>

// Stays — borrows tags, tags still valid after the call
filter_vec(items, |item| tags.contains(item.tag))
print(tags.len())  // OK

// Outlives — carries tags, so tags is consumed
store_callback(|entry: Entry| -> bool { return tags.contains(entry.tag) })
print(tags.len())  // ERROR: tags moved into the closure
```

A closure that stays borrows a Copy capture rather than copying it. That's what
makes MC1's inference mean anything: `mut total = 0; let add = |x| { total =
total + x }` has to reach the caller's `total`, and an `i32` is Copy. Copy
decides what happens when a value *escapes* — which is why a carried one is
copied — not whether a borrow is a borrow.

### Why no keyword

An earlier design wrote `own` at the literal. The argument for it was
visibility: the reader should see where a move happens. Two things sank it.

It restated what the compiler already knew. Omitting it where it was needed was
a compile error whose fix was *"prefix the closure with `own`"* — the compiler
printing the word back at you is the sign that the word is not carrying
information (commitment 5).

And it was a footgun in the other direction. Adding `own` to a closure that
didn't need it silently changed the answer:

```rask
mut n = 0
let f = || { n += 1 }
f()
print(n)                // 1

mut m = 0
let g = || { m += 1 }
g()
print(m)                // 0 — the write landed on the closure's copy
```

Same shape, one keyword, different result, no diagnostic. A marker whose only
power is to select the wrong thing is not visibility.

What is left of the visibility argument is served better by the move itself: a
capture that got carried is consumed, so the next use of that name is an error
pointing at both places.

## What a returned closure may capture

| Rule | Description |
|------|-------------|
| **SL3: A local travels, a lent parameter is borrowed** | A closure leaving the function takes its locals and `take` parameters with it — they are the frame's to give, and the frame is going away. A *lent* parameter — `param: T`, `mutate param: T`, `self` — is the caller's, so the closure borrows it and SL4 decides how long that lasts |
| **SL4: The limit rides the return** | A call that answers a closure hands back whatever limits its borrowed arguments had. `make_filter(tags)` gives a closure that lives as long as `tags` does. A `take` argument contributes no limit — it was given away |

SL3 is the difference between these two:

```rask
func logging(next: func(Request) -> Response) -> func(Request) -> Response {
    return |request| { … next(request) … }   // borrows `next` — the caller's
}

func counter() -> func() -> i64 {
    mut n = 0
    return || { n += 1  return n }           // carries `n` — the frame's
}
```

and the borrow half is what the whole sequence protocol rests on —
`vec.filter(|u| u.active)` is `Vec.filter(self, pred) -> Sequence<T>` returning a
closure over a borrowed receiver. Moving the receiver in would cost every adapter
chain a `take self`.

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

The split that matters is already in this spec, one section up: whether a capture is pointed at
or carried is worked out, not written. Requiring `mutate` on a capture was the odd rule out, not
the pattern.

**The desugar needs nothing special.** `for x in seq { total = total + x }` lowers to
`seq(|x| { total = total + x; return true })`. `total` is captured mutably by inference, like any
other closure. There is no capture list to emit, no mixed capture-and-parameter bracket to
design, and no exemption for compiler-generated code — an earlier draft invented one (an "MC5")
and it is not needed once captures are inferred.

## spawn

`spawn` declares `take f: func() -> T`, so a closure handed to it outlives the frame and
CM1 makes it carry. A task gets its own copy of everything its closure captured, and that
copy lives in the task's environment, which dies when the task does.

```rask
spawn(|| {
    vec.push(1)  // the task's vec — carried in, the outer name is gone
})
```

Carrying keeps the task memory-safe; it doesn't make the program right.

| Rule | Description |
|------|-------------|
| **SP1: A write the task never uses is an error** | Inside a spawned closure, a write to a capture that nothing downstream puts to use is a compile error (E0892). The task is writing its own copy and the copy is about to die, so the write goes nowhere |

SP1 exists because carrying is silent for the sizes that matter least. A `Vec` capture is
moved and the outer name dies with it, which a reader can't miss; an `i64` is copied and
the outer name reads fine, so `mut count = 0` followed by `spawn(|| { count += 1 })` used
to type-check, run, and print `0`. Memory-safe and wrong, which is the worst quadrant.

```rask
mut count = 0
spawn(|| { count += 1 })          // error E0892 — lands on the task's copy

let total = Shared.new(0)         // the fix: one value, two holders
let t = total.clone()
spawn(|| { with t.write() as c { c += 1 } })
```

A write the task puts to use is doing work, so it stays legal — a task that sums into a
local and returns it, or counts something for its own output, is unaffected. `join()` hands
back the closure's return value; it is not a write-back for captures.

"Puts to use" is stricter than "reads again", and the loop is why:

```rask
spawn(|| {
    for i in 0..10 { total += i }     // error E0892
})
```

Every write here is read — by the next iteration. The accumulation is still thrown away,
because the only thing those reads feed is another write that goes nowhere. So a read only
counts when it reaches a use, which lets the whole chain collapse at once.

Deadness here is decidable from the closure body alone, which is why it's an error and not
a lint: no program wants the write it rejects.

## Error messages

**Scope-limited closure escapes [SL3]:**
```
ERROR [E0800]: use of moved value: `tags`
   |
3  |  let f = || process(tags)
   |            ^^^^^^^^^^^^^^ `tags` carried into a closure that outlives this frame
4  |  return f
5  |  print(tags.len())
   |        ^^^^ value used here after move

WHY: the closure is returned, so it outlives the frame `tags` lives in (CM1).
     It takes `tags` with it, and there is nothing left here to read.

FIX: give the closure its own copy, and keep yours:

  let f = || process(tags.clone())
```

There is no "closure cannot escape" error any more. A closure that escapes takes
what it captured; the only thing left to report is the outer name being gone,
which is the move error every other consumption prints.

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
| Carrying closure captures Copy type | Value copied; the outer name is untouched |
| Carrying closure captures move-only type | Type moved in, source invalid |
| Carrying closure captures resource type | Resource consumed by the closure; must be used within or returned |
| Pointing closure captures resource type | Resource borrowed; consuming it in the body is an error (E0891) |
| Nested closures | Each level borrows or carries from its immediate outer scope |
| Pure closure (no captures) | Self-contained either way; nothing to decide |
| Mutable capture of a Copy type | Borrows mutably (not copied), mutations visible to caller |

The resource rows are the same rule as `mem.linear/L3` — a borrow isn't a
consumption — and there is a second reason for them here: nothing says how many
times a closure runs. A `close()` in the body of a pointing closure is one
consumption to read and any number at runtime, so it has to be the carrying
kind, which takes the resource in and leaves the outer binding with nothing to
owe.

```rask
func twice(f: func()) { f() f() }
func store(take f: func()) { … }

let c = Conn.open(1)
twice(|| { c.close() })         // error[E0891] — `twice` borrows, so the closure does
store(|| { c.close() })         // fine: `store` takes it, so `c` is the closure's now
```

---

## Implementation

### Capture semantics

The ownership pass answers CM1 for every closure literal and publishes the set
(`escaping_closures`). Lowering and the interpreter both read it, so the two
backends cannot disagree about what a capture is — they did once, when each got
its own half of the rule.

A carrying closure moves non-Copy values into its environment block, and the
ownership checker marks the source consumed. A pointing closure records a shared
borrow on each captured variable; the source stays valid.

The environment slot is what makes the difference concrete, and there are three
shapes of it:

| Capture | Slot holds | A write inside the body lands on |
|---|---|---|
| Points | The variable's address (8 bytes) | The creating frame's variable |
| Carries | The variable itself | The environment — so it survives to the next call |
| `spawn` | A copy | The task's own state, by construction |

The carrying row is the one that's easy to get wrong. Loading the value out at
the top of the call and working on the loaded copy reads correctly and throws
every write away, so a counter closure answers 1 however many times you call it.
The environment *is* the variable's home once the closure carried it there, so
the body works through the slot's address for its whole life.

The block itself is owned like any other value: whoever is holding it when their
frame ends frees it. What that free doesn't yet do is release the captures inside
— a carrying closure holding a `Vec` frees the block and leaks the Vec (#1045).

### Closure block layout

```
[func_ptr (8 bytes) | captured_var_0 | captured_var_1 | ...]
```

The closure value is a pointer to this block. `closure_ptr + 8` is the environment pointer —
implicit first argument to the closure function.

### Heap vs. stack

Heap exactly when the closure outlives the frame that built it — returned, stored through a
pointer, or handed to something that keeps it. Everything else is a stack slot.

This is the same question CM1 answers, and it is answered once: the closure that
outlives its frame is exactly the one that carries its captures and exactly the
one that needs a heap block. Treating a keyword as the question instead is what
made a returned closure read a popped frame.

| Escapes its frame? | Allocation | Freed by |
|---|---|---|
| No | Stack slot | The frame, on the way out |
| Yes | Heap, behind a size header | Whichever frame is still holding it when it ends |

The size header is there because the frame that frees a closure usually isn't the one that built
it: `let tick = counter()` hands the caller a block whose capture layout only `counter` knew.

---

## Appendix (non-normative)

### Rationale

**Why inference rather than a keyword.** This went the other way first. The
objection to inferring was that the same `|x| …` would mean different things
depending on how it was used, which the reader can't see at the literal — so
`own` was introduced to say it out loud.

That objection doesn't survive contact. Inference picks between *legal* and
*rejected*, not between two meanings: when a closure escapes, pointing at its
captures is a dangling read, and when it doesn't, carrying them drops the
write-back MC4 promises. There was never a second answer for the keyword to
select, which is why omitting it printed a fix that just said to write it.

The case where two answers really did exist was a closure kept in a frame with
private state across calls — `own` there bought you a counter the caller
couldn't see. That went with the keyword. A callable with state of its own is a
struct with a method, which is how it reads anyway.

### Patterns & guidance

| Scenario | Pattern |
|----------|---------|
| Iterator adapter | `items.filter(\|i\| condition)` — points at the source, dies with the chain |
| Simple callback | `\|x\| x * 2` (pure, no captures) |
| Callback with context | `\|event\| process(context, event)` handed to a `take` parameter — carries `context` |
| Mutating a local | `\|x\| count += x` — the mutable capture is inferred (MC1) |
| Shared mutable state (multiple closures) | `Shared<T>` |
| Callback stored for later | Whatever stores it declares `take`, and the closure carries |

**`Shared<T>` for shared mutable state:**

```rask
let counter = Shared.new(0)

button1.on_click(|event| {
    with counter.write() as c { c += 1 }
})
button2.on_click(|event| {
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
