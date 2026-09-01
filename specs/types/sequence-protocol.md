<!-- id: type.sequence -->
<!-- status: decided -->
<!-- summary: Function-valued iteration protocol — for loops desugar to yield-closure calls; zero-cost adapter chains -->
<!-- depends: memory/closures.md, memory/value-semantics.md, control/loops.md -->

# Sequence Protocol

Iteration in Rask is **push**: the source owns the loop and hands you each item. A `Sequence<T>` wraps a function that takes a yield closure and calls it per item. `for x in seq` desugars to a call with the loop body as that closure. Adapters are methods. No stored references, no state machines, no `Iterator` trait, no generators.

## The Type

| Rule | Description |
|------|-------------|
| **SEQ1: Core type** | `Sequence<T>` is a nominal type wrapping `func(yield: \|T\| -> bool)`. Nominal, not an alias — methods need a name to attach to, and an alias is transparent by the time the checker sees a value |
| **SEQ2: Mutable variant** | `SequenceMut<T>` wraps `func(yield: \|mutate item: T\| -> bool)` |
| **SEQ3: Yield return** | `yield` returns `true` to continue, `false` to stop. The sequence must honor the return — on `false`, stop yielding and return |
| **SEQ34: Yields are borrows** | A yield lends its item for the length of one call. `Sequence<T>` yields a read-only borrow, `SequenceMut<T>` a mutable one. No sequence hands over ownership: `mem.closures/CP1` already says what `\|T\|` means, and `CP4` (no `take` parameter on a closure) stands unamended |
| **SEQ35: Owned iteration is not a sequence** | Consuming a collection is `take_all()`, which returns the drained `Vec<T>`. `for x in v.take_all()` is an ordinary for-over-Vec on a temporary the loop owns |
| **SEQ36: A closure literal fills a Sequence slot** | Where a `Sequence<T>` is expected, a closure of the right shape is one — no constructor call. Same rule as `let xs: Vec<i64> = [1, 2, 3]`: the slot picks the shape (`std.collections/C4`) |

<!-- test: skip -->
```rask
public func in_order(self) -> Sequence<i32> {
    return |emit| { walk(self.root, emit) }    // SEQ36: the return type says what this is
}
```

A `Sequence<T>` is a first-class value. It can be stored, passed, returned — subject to the same scope rules as any closure (`mem.closures/SL1-SL2`).

**Why nominal.** The earlier draft made `Sequence<T>` a bare `type alias`, on the grounds that it's "just a function type." That reads well and it doesn't work: `extend` blocks attach methods to a *name*, and an alias has dissolved into `func(func(T) -> bool)` before method resolution runs. There would be nothing for `seq.filter(p)` to find. Making the type nominal costs one sentence of framing and buys ordinary dispatch, chains that can be split across statements, and a type users can write in a signature.

## Yields Lend, They Don't Give

The yield closure's parameter is a parameter like any other, so `mem.closures` has already decided this: `|T|` is a read-only borrow, `|mutate item: T|` a mutable one, and a closure cannot take ownership through a parameter at all. A sequence lends each item for one call and takes it back.

Two things fall out.

**Re-running is safe.** SEQ11 says calling a sequence twice runs the traversal twice. If a yield could move, a sequence over a drained buffer would hand the same element out twice and free it twice. Because yields only lend, a second run is a second traversal of data the sequence still owns — repeated side effects, never a double free.

**Nothing needs a linearity story.** There is no once-only sequence, no consumed-ness to thread through ten adapters, no second protocol type. `filter` lends on what its source lent it.

The cost: `files.take_all().filter(|f| f.stale)` does not exist. Draining and adapting are separate steps (SEQ35).

| Rule | Description |
|------|-------------|
| **SEQ44: A yield parameter is not storage** | A yielded value may be an aggregate carrying a borrow — `enumerate()` yields `(usize, T)` where `T` is the lent item. The no-storable-references rule (`mem.relocatable`) is about what a value keeps between statements; a yield parameter lives for one call and is gone. Without this, `enumerate` would only work for `Copy` elements |

<!-- test: skip -->
```rask
for file in files.take_all() {     // drained Vec — the loop owns each file
    try file.close()
}

let stale = files.iter().filter(|f| f.stale).to_vec()    // borrowed — adapt freely
```

A sequence that *produces* values rather than walking a collection — a channel reader, a parser — owns them in its own frame and lends from there. `rx.stream()` receives a message into a local and yields a borrow of it; a body that wants to keep the message clones it.

## Lazy, But Not Resumable

Push buys one kind of laziness completely and rules out the other. Naming both together is worth more than the three separate limitations they otherwise look like.

| Rule | Description |
|------|-------------|
| **SEQ37: Lazy and fused** | Building a chain runs nothing and allocates nothing. Work happens on consumption, in one pass, with no intermediate collection. `.take(n)` over an infinite source terminates |
| **SEQ38: Not resumable** | A sequence cannot be half-consumed. There is no `next()`, no peek, no holding a position across unrelated work. Once called, it runs to completion or to a `false` |
| **SEQ39: Two positions means indices** | Anything needing two independent positions — zip, merge, element-wise compare — uses indices over indexable sources, or materializes with `to_vec()` first. `zip`, `peekable` and friends are absent for one reason, not several |

The reason is the same one that makes authoring easy: a push source keeps its position on the call stack. Suspending it mid-traversal would mean heap-allocating that stack, which is a coroutine, which Rask does not have (SEQ22). So the position is free to create and impossible to save.

<!-- test: skip -->
```rask
// Lazy: nothing runs until to_vec, and no intermediate Vec is built
let first_ten = readings.iter().filter(|r| r.valid).map(|r| r.value).take(10).to_vec()

// Not resumable: two positions, so use indices
for i in 0..min(a.len(), b.len()) {
    merge(a[i], b[i])
}
```

## For-Loop Desugaring

| Rule | Description |
|------|-------------|
| **SEQ4: Range loops** | `for x in range` — built-in range loop, no closure call |
| **SEQ5: Built-in collections** | `for x in vec` / `for mutate x in vec` — inline-alias desugar (`ctrl.loops/LP17`), no `Sequence` involved |
| **SEQ6: Custom types** | `for x in seq_expr { body }` where `seq_expr: Sequence<T>` desugars to a yield-closure call |
| **SEQ7: Break/continue translation** | Inside the desugared closure: `break` becomes `return false`, `continue` becomes `return true`. The closure returns `true` at end-of-body |
| **SEQ8: Return propagation** | `return` in a for-body exits the enclosing function, not the yield closure. The closure writes the return value to a slot in the enclosing frame, sets a flag beside it, and returns `false`; the frame checks the flag after the call and returns if set. This is why SEQ13a is load-bearing — an adapter that swallowed the `false` would swallow the `return` with it |
| **SEQ40: Desugared closures capture mutably** | The yield closure the compiler builds for a for-body gets mutable access to every enclosing local the body writes, without anyone spelling `mutate`. The explicit-capture rule (`mem.closures/MC1`) exists so an *escaping* closure can't quietly alias a local; this one is called and discarded inside a single statement, so the hazard it guards against doesn't arise. Hand-written closures are unaffected |

A `Sequence<T>` is nominal but still callable — `seq(f)` invokes it. The wrapper exists for method dispatch, not to hide the call.

<!-- test: skip -->
```rask
// Source
for node in tree.in_order() {
    print(node.value)
    if node.skip: continue
    if node.stop: break
    process(node)
}

// Desugars to:
tree.in_order()(|node| {
    print(node.value)
    if node.skip: return true    // continue
    if node.stop: return false   // break
    process(node)
    return true
})
```

**The accumulating body (SEQ40).** This is the common shape, and it's the one that needs the capture rule — the loop body writes a local that lives in the enclosing function:

<!-- test: skip -->
```rask
// Source
mut total = 0
for v in tree.in_order() {
    total = total + v
}

// Desugars to — `total` is a mutable capture the compiler added:
tree.in_order()(|mutate total, v| {
    total = total + v
    return true
})
```

Without SEQ40 every accumulating loop would need the programmer to write the capture list of a closure they never wrote. With it, `for` over a sequence reads exactly like `for` over a Vec, which is the point.

## Laziness and Re-Consumption

| Rule | Description |
|------|-------------|
| **SEQ9: Lazy construction** | Building a `Sequence<T>` runs nothing. Adapter chains (`.filter().map()`) compose closures without executing |
| **SEQ10: Eager consumption** | The chain runs when consumed by a for-loop or a terminal operation |
| **SEQ11: Re-consumption runs again** | A `Sequence<T>` is a function value. Calling it twice runs the underlying traversal twice. Side effects repeat |

<!-- test: skip -->
```rask
let s = users.iter().filter(|u| u.active)
// nothing has run yet

for u in s { print(u.name) }     // runs the chain
let count = s.count()          // runs the chain AGAIN
```

To consume twice without re-running, materialize with `.to_vec()`:

<!-- test: skip -->
```rask
let active = users.iter().filter(|u| u.active).to_vec()
for u in active { print(u.name) }
let count = active.len()
```

## Authoring Sequences

A method returning `Sequence<T>` constructs a closure. The closure captures `self` (or whatever source it walks). Per closure rules, the resulting `Sequence<T>` is scope-limited to the captured source's lifetime.

The recursion is the point. Traversal position lives on the call stack, so a walk is written the way the algorithm is written — no explicit stack, no state machine, no self-reference.

**Rack-backed tree** — the idiomatic shape for anything with edges (`mem.racks`):

<!-- test: skip -->
```rask
struct Node<T> {
    public value: T
    public left: Link<Node<T>>?
    public right: Link<Node<T>>?
}

struct Tree<T> { public root: Link<Node<T>>? }

extend Tree<T> {
    public func in_order(self) -> Sequence<Node<T>> {
        return |yield| { walk(self.root, yield) }
    }
}

func walk<T>(n: Link<Node<T>>?, yield: |Node<T>| -> bool) -> bool {
    if n? as node {
        if !walk(node.left, yield)  { return false }
        if !yield(node)             { return false }
        if !walk(node.right, yield) { return false }
    }
    return true
}
```

**Owned-recursive tree** — one owner per child, no edges to maintain:

<!-- test: skip -->
```rask
struct Tree<T> { public root: Heap<Node<T>>? }

struct Node<T> {
    public value: T
    public left: Heap<Node<T>>?
    public right: Heap<Node<T>>?
}

extend Tree<T> {
    public func in_order(self) -> Sequence<Node<T>> {
        return |yield| {
            if self.root? as r { walk(*r, yield) }
        }
    }
}

func walk<T>(node: Node<T>, yield: |Node<T>| -> bool) -> bool {
    if node.left?  as l { if !walk(*l, yield) { return false } }
    if !yield(node)                           { return false }
    if node.right? as r { if !walk(*r, yield) { return false } }
    return true
}
```

This second shape is the one push exists to serve, and the one a pull cursor could not express: `Heap<T>` is a linear owning box, so there is no non-owning value that names a node halfway down the tree. The position has nowhere to live except the call stack.

**Stack depth is the cost.** A push traversal recurses once per level, and nothing bounds that. A balanced tree is fine; a degenerate one overflows the native stack. Push trades a visible allocation (the explicit stack a pull cursor would need) for an invisible one, and this is the place where that shows.

**From a channel:**

The receiver must be owned by the Sequence because the closure calls `receive()` *after* the method that built it has returned. Any capture of `rx` by borrow would make the closure expression-scoped — it would not type-check at the storage or return site.

<!-- test: skip -->
```rask
extend Receiver<T> {
    public func stream(take self) -> Sequence<T> {
        return |yield| {
            loop {
                if self.receive()? as msg {
                    if !yield(msg) { break }
                } else {
                    break
                }
            }
        }
    }
}

for msg in rx.stream() {      // rx is consumed here
    handle(msg)
}
```

If the returned `Sequence<T>` is dropped without being consumed, the captured `Receiver` drops with it — channel close follows normal Receiver-drop semantics. Senders do not block on a dropped receiver.

## Standard Adapters

Adapters are methods on `Sequence<T>`, declared in one `extend Sequence<T>` block in the stdlib and written in ordinary Rask. Each takes the sequence it extends and returns a new one, so chains read left to right.

| Rule | Description |
|------|-------------|
| **SEQ12: Adapter shape** | Adapters are `extend Sequence<T> { public func name<...>(self, ...) -> Sequence<U> }`. Ordinary methods on a nominal type — nothing special-cased in the checker, and users can add their own |
| **SEQ13: Chain syntax** | `seq.adapter(args)` resolves like any method call. An earlier draft declared adapters as free functions taking the sequence as the first parameter *and* said they resolve as methods; those are different mechanisms and the method one wins |
| **SEQ41: One adapter surface** | Adapters and terminals live on `Sequence<T>` and nowhere else. Collections carry no eager `map`/`filter`/`take`/`fold`/`sum`/… of their own: `v.map(f)` returning a fresh `Vec` is a second spelling of `v.iter().map(f).to_vec()` with a different cost profile, and `std.api/SD5` says one operation gets one spelling. The chain is the spelling; SEQ18 is what makes it free |
| **SEQ13a: Short-circuit propagation** | If the downstream yield returns `false`, the adapter must stop and return `false` from its own yield call. Sources must likewise stop emitting when their yield returns `false`. This is the contract that makes `.take(n)`, `.find()`, and `break` work. Violating it changes observable semantics |

| Adapter | Behavior | Signature |
|---------|----------|-----------|
| `filter(pred)` | Yield items where pred is true | `(Sequence<T>, \|T\| -> bool) -> Sequence<T>` |
| `map(f)` | Transform each item | `(Sequence<T>, \|T\| -> U) -> Sequence<U>` |
| `take(n)` | Yield first n items | `(Sequence<T>, usize) -> Sequence<T>` |
| `skip(n)` | Skip first n items | `(Sequence<T>, usize) -> Sequence<T>` |
| `take_while(pred)` | Yield while pred true | `(Sequence<T>, \|T\| -> bool) -> Sequence<T>` |
| `skip_while(pred)` | Skip while pred true, then yield rest | `(Sequence<T>, \|T\| -> bool) -> Sequence<T>` |
| `chain(other)` | Concatenate two sequences | `(Sequence<T>, Sequence<T>) -> Sequence<T>` |
| `enumerate()` | Pair each item with its index | `(Sequence<T>) -> Sequence<(usize, T)>` |
| `flatten()` | Flatten one level | `(Sequence<Sequence<T>>) -> Sequence<T>` |
| `flat_map(f)` | Map then flatten | `(Sequence<T>, \|T\| -> Sequence<U>) -> Sequence<U>` |

<!-- test: skip -->
```rask
for name in users
    .iter()
    .filter(|u| u.active)
    .map(|u| u.name)
    .take(10)
{
    print(name)
}
```

## Terminal Operations

Terminals drive the chain to completion (or short-circuit) and produce a value.

| Terminal | Behavior | Returns |
|----------|----------|---------|
| `to_vec()` | Materialize into a `Vec<T>` | `Vec<T>` |
| `to_map()` | Materialize a `Sequence<(K, V)>` into a `Map<K, V>` | `Map<K, V>` |
| `join(sep)` | Concatenate a `Sequence<string>` with a separator | `string` |
| `fold(init, f)` | Reduce with initial | `A` |
| `reduce(f)` | Reduce without initial | `T?` (`none` if empty) |
| `sum()` | Sum items | `T where T: Numeric` |
| `product()` | Multiply items | `T where T: Numeric` |
| `count()` | Count items | `usize` |
| `min()` | Smallest | `T?` |
| `max()` | Largest | `T?` |
| `min_by(cmp)` | Smallest by comparator | `T?` |
| `max_by(cmp)` | Largest by comparator | `T?` |
| `min_by_key(f)` | Smallest by key | `T?` |
| `max_by_key(f)` | Largest by key | `T?` |
| `any(pred)` | True if any matches | `bool` |
| `all(pred)` | True if all match | `bool` |
| `find(pred)` | First match | `T?` |
| `for_each(f)` | Apply to each item | `()` |

<!-- test: skip -->
```rask
let total = orders.iter().map(|o| o.amount).sum()
let admin = users.iter().find(|u| u.is_admin)
let active = users.iter().filter(|u| u.active).to_vec()   // clones each User (SEQ43)
```

## Specialized Terminals

| Rule | Description |
|------|-------------|
| **SEQ42: A terminal may be computed** | A source that knows an answer without walking may compute it, provided the result is what the walk would have produced. `(0..n).count()` is `n`; `(a..b).sum()` is the closed form; `v.iter().count()` is `v.len()`. Specialization is invisible — same answer, same type, less work |
| **SEQ43: Only where the walk is pure** | A terminal that runs a user closure (`find`, `any`, `fold`, `for_each`) is never specialized: the closure may have effects, and skipping it would change what the program does. `count`, `sum`, `product`, `min`, `max` over a range or a Vec are the specializable set |

Routing `(1..n).sum()` through a yield closure to add integers one at a time would be a pessimisation of the two most obvious calls anyone makes on a range. The protocol says what the answer is; it doesn't require arriving at it the slow way.

## Materializing

Every terminal that builds a collection names the collection it builds. There is no one terminal that builds "whatever you asked for" — that would need a type argument, an annotation, or backwards inference, and Rask pays for none of them.

| Rule | Description |
|------|-------------|
| **SEQ28: `to_vec()` builds a `Vec<T>`** | The target is fixed. No type parameter, no annotation, no inference from later use. `seq.to_vec()` on a `Sequence<T>` is `Vec<T>` and nothing else |
| **SEQ29: `to_map()` builds a `Map<K, V>`** | Defined only on `Sequence<(K, V)>`. Later keys overwrite earlier ones — identical to repeated `insert`. A sequence of non-pairs is a type error at the call, not a silent tuple coercion |
| **SEQ30: `join(sep)` builds a `string`** | Defined only on `Sequence<string>`. This is the third materializing target and it does not read as a "collect" at all — evidence that the polymorphic version was never the right shape |
| **SEQ31: No generic target** | There is no `collect()`, no `collect<C>()`, no `FromSequence` trait, no turbofish. Adding a materializing target means adding a named terminal to this table |
| **SEQ43: `to_vec` clones each item** | A yield lends (SEQ34) and a `Vec` owns, so materializing copies — `to_vec` needs a cloneable element and allocates one clone per item. The `to_` prefix is what makes that visible: it means "allocates a new value" (`canonical-patterns`), the same contract `to_string` and `to_lowercase` already carry. This is not the implicit-copy case `mem.value-semantics/VS1` guards against — that rule is about a bare binding silently deep-copying, not a method whose name advertises the allocation |
| **SEQ32: Terminals borrow, they don't consume** | `to_*`, never `into_*`. A `Sequence<T>` is a function value and survives the call, so `to_vec()` twice runs the traversal twice (SEQ11). The `to_*` prefix already means "non-consuming, allocates" (`canonical-patterns`) |
| **SEQ33: `Vec.from` / `Map.from` stay array-only** | The static constructors take array literals (`std.collections`). They do not take a `Sequence<T>`. One operation, one spelling (`std.api/SD5`) |

<!-- test: skip -->
```rask
let lines = input.lines().to_vec()
let parts = version.split(".").to_vec()

let views = rows()
    .iter()
    .skip(page * size)
    .take(size)
    .map(|r| r.view.clone())
    .to_vec()

let by_id = users.iter().map(|u| (u.id, u.clone())).to_map()
let csv = fields.iter().map(|f| f.escaped()).join(",")
```

Each line says what it produces, at the end of the chain, with no annotation and no type argument. The element type comes from the chain; the container type comes from the method name.

## Lockstep Iteration

| Rule | Description |
|------|-------------|
| **SEQ14: No general zip** | There is no `zip` adapter. This is SEQ38 and SEQ39 in a particular costume, not a separate decision: lockstep needs two positions held at once, and a push source cannot hold one |
| **SEQ15: Indexable lockstep** | For indexable sources (Vec, array, Pool+handles), use index iteration: `for i in 0..min(a.len(), b.len()) { use(a[i], b[i]) }` |
| **SEQ16: Non-indexable lockstep** | Non-indexable sources must buffer explicitly. The allocation is visible in the code |

<!-- test: parse -->
```rask
func zip_indexable(a: Vec<i32>, b: Vec<i32>) {
    // Indexable (common case, zero-cost)
    for i in 0..min(a.len(), b.len()) {
        process(a[i], b[i])
    }
}

func zip_buffered(tree_a: Tree<Node>, tree_b: Tree<Node>) {
    // Non-indexable (rare — explicit buffer shows the cost)
    let a_items = tree_a.in_order().to_vec()
    mut idx = 0
    tree_b.in_order()(|b_node| {
        if idx >= a_items.len(): return false
        process(a_items[idx], b_node)
        idx += 1
        return true
    })
}
```

## SequenceMut Has No Adapters

| Rule | Description |
|------|-------------|
| **SEQ45: Mutable sequences are terminal-only** | `SequenceMut<T>` supports `for mutate x in seq` and nothing else. No `filter`, no `map`, no terminals |

`filter` on a mutable sequence would work. `map` cannot: it produces new values, and there is nothing to hand back a mutable borrow *of* — the mapped value belongs to the adapter's frame and dies when the yield returns, so writing through it would write to a temporary.

That leaves a half-surface where the adapters that survive are the ones nobody reaches for first, which is worse than none. So `SequenceMut` does one job. A mutable walk that needs filtering puts the test in the loop body, where it reads better anyway:

<!-- test: skip -->
```rask
for mutate node in tree.in_order_mut() {
    if node.value < 0 { continue }
    node.value += 1
}
```

Anything needing more than that — reordering, removal, two positions — is index work (SEQ39).

## Zero-Cost Contract

| Rule | Description |
|------|-------------|
| **SEQ17: Inlining required** | The compiler inlines yield closures at every call site in the sequence body. Adapter closures inline into their inner sequence |
| **SEQ18: Fusion** | Adapter chains (`.filter().map().take()`) compile to a single fused loop, equivalent to a hand-written version |
| **SEQ19: Verified** | Compiler test `compiler/tests/sequence_fusion.rs` verifies MIR output for canonical adapter chains matches the hand-written equivalent |

**What this requires, and doesn't have yet.** An adapter chain composes closures, so calling it emits an indirect call per adapter per item. The general inliner only inlines *direct* calls, so nothing fuses on its own. What closes the gap is devirtualization: where a call's target traces back to a known closure literal, rewrite it to a direct call and let the ordinary inliner take it from there. That pass does not exist.

So SEQ17–SEQ19 are the target, not the present state, and this section says so rather than asserting a guarantee nothing enforces. Until the pass lands, a chain is correct and slow, and the honest claim is "fused for built-in sources, whose chains are recognized and lowered as loops already." Once it lands, a fusion regression is a compiler bug.

## What Does Not Exist

| Rule | Description |
|------|-------------|
| **SEQ20: No Iterator trait** | There is no user-facing `Iterator<Item>` trait. Types do not implement a "is an iterator" contract — they expose methods that return `Sequence<T>` |
| **SEQ21: No lending iterators** | Per-call mutable yields are expressed via `SequenceMut<T>`. Rask does not have GATs or lifetime-parameterized Item types |
| **SEQ22: No generators** | Rask does not have a `yield` keyword in regular functions. Sequences are closure-based; traversal state lives on the real call stack or in explicit struct fields |
| **SEQ23: No zip adapter** | See SEQ14. Use indices or explicit buffer |
| **SEQ24: No Pin** | Self-referential state is impossible by construction — `Sequence<T>` is a closure, and closures cannot borrow from their own captures (`mem.closures`) |

## Scope Rules

`Sequence<T>` storability is not a new rule — it falls out of ordinary closure capture rules (`mem.closures/SL1-SL2`, `mem.closures/MC3`). A `Sequence<T>` is a closure value; its lifetime is the lifetime of whatever it captures.

| Rule | Description |
|------|-------------|
| **SEQ25: Owned captures = storable** | A `Sequence<T>` whose closure captures only owned or Copy data can be stored in structs, returned across function boundaries, and sent across tasks. The canonical pattern is `take self` on the method that builds it |
| **SEQ26: Borrow captures = expression-scoped** | A `Sequence<T>` whose closure captures any block-scoped borrow is limited to that borrow's scope. It cannot be returned past the source, stored in a struct, or sent across tasks |
| **SEQ27: No separate closed-world rule** | There is no "Sequence-specific" storability constraint. The rule above is `mem.closures/SL1-SL2` applied verbatim to the closure that implements the sequence |

Concretely: if your method builds a `Sequence<T>` by borrowing `self`, the returned Sequence is expression-scoped (like a closure that captures a block-scoped borrow). If the method takes `take self`, the Sequence owns the source and is freely storable.

<!-- test: skip -->
```rask
func collect_active(users: Vec<User>) -> Vec<User> {
    return users
        .iter()                        // Sequence borrows users
        .filter(|u| u.active)          // Filter borrows the Sequence
        .to_vec()                      // Materialized here — no Sequence escapes
}

func bad_return(users: Vec<User>) -> Sequence<User> {
    return users.iter().filter(|u| u.active)
    // ERROR: Sequence borrows `users` (a parameter borrow);
    // cannot escape the function. Same rule as returning a closure
    // that captures a block-scoped borrow (mem.closures/SL2).
}
```

To return a sequence-producing function, accept the source by `take`:

<!-- test: skip -->
```rask
func make_active_seq(take users: Vec<User>) -> Sequence<User> {
    // users is owned by this closure — no borrow to outlive
    return |yield| {
        for u in users {
            if u.active {
                if !yield(u) { return }
            }
        }
    }
}
```

## Error Messages

**Sequence escapes scope [mem.closures/SL2]:**
```
ERROR [mem.closures/SL2]: sequence borrows a value that does not outlive the return
   |
3  |  return users.iter().filter(|u| u.active)
   |         ^^^^^^^^^^^^ borrows `users` (parameter borrow)
   |                      sequence cannot escape the function

WHY: A Sequence<T> built over a borrowed source is scope-limited
     to that borrow. Returning it would outlive the source.

FIX 1: Consume inside the function (to_vec, fold, for-loop):

  return users.iter().filter(|u| u.active).to_vec()

FIX 2: Take ownership of the source:

  func active(take users: Vec<User>) -> Sequence<User> { ... }
```

**`collect()` no longer exists [type.sequence/SEQ31]:**
```
ERROR [type.sequence/SEQ31]: no method `collect` on Sequence<View>
   |
5  |      .collect()
   |       ^^^^^^^ materializing terminals name what they build

WHY: `collect` didn't say what it produced, so it needed an annotation
     or a type argument at every call. Each target got its own name.

FIX: pick the one you meant:

  .to_vec()          // Vec<View>
  .to_map()          // Map<K, V> — on a sequence of (K, V) pairs
  .join(", ")        // string — on a sequence of strings
```

**`to_map()` on a non-pair sequence [type.sequence/SEQ29]:**
```
ERROR [type.sequence/SEQ29]: `to_map` needs a sequence of pairs, got Sequence<User>
   |
3  |  let by_id = users.iter().to_map()
   |                           ^^^^^^ each item must be a (K, V) tuple

WHY: A Map needs a key per value. `to_map` reads the key out of the
     first tuple slot — it will not invent one.

FIX: produce the pairs first:

  let by_id = users.iter().map(|u| (u.id, u.clone())).to_map()
```

**Break with value in Sequence for-loop:**
```
ERROR [type.sequence/SEQ7]: break with value not supported in Sequence for-loops
   |
5  |  break found_item
   |  ^^^^^^^^^^^^^^^^ Sequence for-loops do not support break-with-value

WHY: Sequence for-loops desugar to a closure body. `break value`
     would require translating to a non-local return from the closure.

FIX: Use find() or capture via a local:

  let result = seq.find(|x| matches(x))

  // or
  mut found: T? = none
  for x in seq {
      if matches(x) { found = x; break }
  }
```

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Empty sequence | SEQ1 | For-loop body never runs |
| Break in sequence body | SEQ7 | Yield closure returns `false`; sequence must stop |
| Continue in sequence body | SEQ7 | Yield closure returns `true` |
| Sequence yields owned non-Copy | SEQ34 | Impossible — a yield lends. Draining is `take_all()`, which is a `Vec<T>` (SEQ35) |
| Sequence yields borrow | SEQ1 | Each yield passes a borrow for the closure duration only |
| SequenceMut yields mutable | SEQ2 | Each yield passes a fresh mutable borrow; ends when closure returns |
| Re-consuming a Sequence | SEQ11 | Runs the chain again; side effects repeat |
| Returning a Sequence from a function | SEQ-scope | Only allowed when source is owned by the closure |
| Storing a Sequence in a struct | SEQ-scope | Only allowed when source is owned by the closure (no external borrows) |
| Sending a Sequence cross-task | SEQ-scope | Same rule as sending a closure (`mem.closures`) |
| Infinite sequence with `.take(n)` | SEQ10 | Terminates after n yields |
| Empty chain with `.sum()` | — | Zero value for the type |
| Empty chain with `.reduce()` | — | `none` |
| Empty chain with `.min()`/`.max()` | — | `none` |

---

## Appendix (non-normative)

### Rationale

**Why push over pull.** Push and pull are the same loop with the stack inverted, and the only question is whose frame holds it. Pull hands the frame to the consumer, so the *producer* has to turn its position into data. Push hands it to the producer, so the *consumer's* locals become captures. Whoever holds the frame keeps their state for free.

An earlier draft justified push with "Rask has no storable references, so a pull cursor's position has nowhere to live." That argument died with `mem.pools`: `Link<T>` is a storable reference — `mem.racks` says so in its own summary line — and a cursor over a rack is `struct InOrder<T> { stack: Vec<Link<Node<T>>> }`, perfectly ordinary Rask. Anyone re-deriving this decision from the old sentence will find it false and reopen the question, so here is the argument that actually holds.

**Nothing can point into a `Vec`.** A pull cursor over a Vec is `struct VecCursor<T> { source: <a borrow of the Vec>, i: usize }`, and there is no such field. `Link` points into a rack and nothing else; there is no value naming a position inside a `Vec`, a `Map`, a `string`, or a `Heap` chain. So pull is authorable only for rack-shaped data, and every other collection stays compiler-magic — user types and stdlib types iterating by different mechanisms, permanently.

Making it uniform needs a struct that is scope-limited by a borrow it holds. Closures already have that property (`mem.closures/SL1-SL2`) and so do links (`mem.racks/RK6`); generalizing it to structs is lifetime annotations with the serial numbers filed off, which is the one bill Rask has refused to pay since the start.

Push has no such asymmetry. The closure captures its source *by borrow*, which is legal precisely because the closure can't escape — Vec, Map, string, Heap, Rack, channels, one mechanism, all of it user-authorable. That is the reason, and it costs what SEQ34 (no owned yields), SEQ38 (no resuming), SEQ8 (return needs a flag) and SEQ40 (the desugar captures for you) say it costs.

Go reached the same shape in 1.23 — `func(yield func(V) bool)`, same `bool`, same short-circuit — for the same reason: iteration that works without lifetimes and without coroutines. It has since been through a large ecosystem, no `zip`, `break` on the bool, compiler help for `return`. Corroboration, not the argument.

**Why not generators.** A generator function with a `yield` keyword compiles to a state machine that stores locals across pause points. When those locals include borrows into the generator's own state, you get the self-reference problem — the reason Rust needs `Pin`. Rask avoids the whole category by not synthesizing state machines.

**Why not effect handlers.** The 2025 OOPSLA work on zero-overhead lexical effect handlers proves that tail-resumptive handlers (which iteration is) can be zero-cost. But effect handlers are a paradigm, not a feature. Adopting them for iteration alone brings compiler complexity across the whole language for a narrow benefit. Rask's only "effect" system is `using` context, and that stays.

**Why no zip.** General zip over two `Sequence<T>` values requires either buffering (hidden allocation), a green task (not universally available — `conc.async/C1`), or a compiler-synthesized state machine (effectively generators, rejected above). All three hide cost. Indices cover the real use case; explicit buffer covers the rest; cost is visible either way. This matches `core-design/transparency-of-cost`.

**Why there's no `collect` (SEQ28–SEQ33).** `collect` is polymorphic in its result, and a result-polymorphic function has to get its answer from somewhere the call site doesn't say. Rust's somewhere is the annotation or the turbofish. I don't want either on a line this common, so I looked at what the polymorphism was actually buying.

It was buying almost nothing. Rask has three collection types — `Vec`, `Map`, `Pool` — and `Pool` isn't a materializing target, because what comes out of a Pool is handles. Across every `.collect()` in `examples/`, the spec corpus and the test suite, the answer was `Vec` in *every single case*. Not "mostly Vec, with an escape hatch" — Vec, always. A trait, a type argument and an inference story, to serve a choice nobody was making. That's `FromIterator` imported by reflex (`std.api/SD4`), and the give-away is `join`: the string target already had a better name than "collect into a string" and nobody ever missed it.

So each target gets a name, and the name is the one the naming table already assigns: `to_*` is "non-consuming conversion, allocates" (`canonical-patterns`), which is exactly what a terminal on a re-runnable sequence is. `to_vec` isn't borrowed from `slice::to_vec` — it falls out of Rask's own vocabulary, and `into_vec` would be wrong here for a real reason (SEQ32: the sequence survives).

Rejected, with the sketches that killed them:

- **Infer the target from later use.** Best-looking call site, and it doesn't work on Rask's own code. `let lines = input.lines().collect()` is followed by `lines[i]`, `lines.len()` and `for l in lines` — every one of those is shared between `Vec` and `Map`, so there's nothing to infer *from*. Inference only bites when the value is returned or passed to a typed parameter, which the common local-buffer case never does. Paying for backwards type flow through a function body, and getting a worse error when it fails, to resolve a fraction of call sites — no.
- **Require the annotation.** `let parts: Vec<string> = s.split(".").collect()` — the source already said `string` twice and the reader already knew it was a Vec. Worse in the shape that motivated it: `let users: Vec<UserResponse> = d.users.values().map(|u| UserResponse { … }).collect()` names `UserResponse` twice in one statement. Go writes this in one term with no annotation; principle 4 says that's a design bug, not a style preference.
- **Keep `collect()` for Vec, add `to_map()` for the rest.** Works, and it's the closest runner-up. It loses on consistency: `collect` names the process, `to_map` names the result, and they sit in the same slot at the end of the same chain. Renaming the Vec case to match is a smaller change than teaching everyone why the two look different.
- **Target leads: `Vec.from(seq)`, `Map.from(seq)`.** Reads fine on one-liners and badly on the chains that matter. `Vec.from(rows().iter().skip(n).take(m).map(|r| r.view.clone()))` puts the opening paren four lines above its close and forces the reader to jump back to the head to find out what's being built. Terminals belong in trailing position because that's the direction chains are read. `Vec.from` keeps its array-literal job (SEQ33) and doesn't grow a sequence overload — one operation, one spelling.

This also settles the note in `rejected-features.md` about associated types being worth promoting for "a `collect` that targets `Vec` or `Map`": there is no such `collect`, so that particular argument for associated types is withdrawn.

**Why `to_map` overwrites instead of erroring.** Duplicate keys are the normal case for the pattern this serves — indexing a list by some field, where a later record supersedes an earlier one. An error type would put a `try` on a line whose failure mode nobody handles. `to_map` behaves like a loop of `insert`, which is what it replaces, and grouping (keeping every value) is a different operation that would need a different name if it ever earns one.

**Why SequenceMut is separate.** One unified `Sequence<T, M>` with a mode parameter is possible but makes the signatures noisier without helping users — the two cases are used differently and rarely mixed. Two aliases keep each simple.

**Re-consumption runs twice.** Rust's pull iterators solve this by consuming on use (the iterator is dropped after `.collect()`). Push sequences are function values — no consumption. The tradeoff is visibility: in exchange for simpler authoring, users see a footgun around side-effectful traversals. Documented, not eliminated.

### Migration from `type.iterators`

The retired `Iterator<Item>` trait mapped to these patterns:

| Old | New |
|-----|-----|
| `extend MyType with Iterator<T> { func next(...) }` | `public func iter(self) -> Sequence<T> { return \|yield\| { ... } }` |
| `collection.iterate()` (returned `VecRefIterator<T>` etc.) | `collection.iter()` returns `Sequence<T>` |
| `iter.collect()` | `iter.to_vec()` (SEQ28) — or `.to_map()` / `.join(sep)` |
| `.take_all()` returning consuming iterator struct | `.take_all()` returns the drained `Vec<T>` (SEQ35) — not a Sequence |
| `pool.handles()` returning handle iterator | `pool.handles()` returns `Sequence<Handle<T>>` |
| `iter.zip(other)` | Use indices: `for i in 0..min(a.len(), b.len())` |

### Patterns & Guidance

**Pool-backed graph traversal** — yield handles, compose freely:

<!-- test: skip -->
```rask
for h in graph.bfs(start) {
    if graph.nodes[h].visited { continue }
    mark_visited(h)
}
```

**Streaming over a channel:**

<!-- test: skip -->
```rask
for msg in messages(rx).filter(|m| m.kind == Kind.Important).take(100) {
    handle(msg)
}
```

**Mutable walk over a custom type:**

<!-- test: skip -->
```rask
extend Tree<T> {
    public func in_order_mut(mutate self) -> SequenceMut<Node<T>> {
        return |yield| { walk_mut(self.root, yield) }
    }
}

for mutate node in tree.in_order_mut() {
    node.value += 1
}
```

### Performance Guarantees

- Adapter chains compile to hand-written loop equivalents (SEQ17-SEQ19)
- No heap allocation for sequence construction — closures used inline are stack-allocated per `mem.closures`
- Terminals short-circuit: `any`/`all`/`find` stop at first matching item via `yield -> false`

### IDE Integration

| Context | Ghost annotation |
|---------|------------------|
| Sequence value | `[Sequence<T>]` with captures listed |
| Scope-limited Sequence | `[scope-limited to line N]` |
| Adapter chain | `[fused loop]` after optimization |

Hovering over a for-loop over a Sequence shows:
- The yielded type
- Whether the body is inlined
- Whether the chain is fused

### See Also

- [Closures](../memory/closures.md) — Capture rules, scope limits, mutable params (`mem.closures`)
- [Loops](../control/loops.md) — For-loop desugar for built-ins and Sequences (`ctrl.loops`)
- [Iteration Patterns](../stdlib/iteration.md) — Collection iteration modes (`std.iteration`)
- [Collections](../stdlib/collections.md) — Vec, Pool, Map APIs (`std.collections`)
- [Channels](../concurrency/sync.md) — Streaming via `receive()` in a for-loop
