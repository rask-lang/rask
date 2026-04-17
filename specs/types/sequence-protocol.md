<!-- id: type.sequence -->
<!-- status: decided -->
<!-- summary: Function-valued iteration protocol — for loops desugar to yield-closure calls; zero-cost adapter chains -->
<!-- depends: memory/closures.md, memory/value-semantics.md, control/loops.md -->

# Sequence Protocol

Iteration in Rask is a function that takes a yield closure. `Sequence<T>` is not a trait, not a struct — it's a function type. `for x in seq` desugars to a call with a closure body. Adapters are plain generic functions. No stored references, no state machines, no `Iterator` trait.

## The Type

| Rule | Description |
|------|-------------|
| **SEQ1: Core type** | `type alias Sequence<T> = func(yield: \|T\| -> bool)` |
| **SEQ2: Mutable variant** | `type alias SequenceMut<T> = func(yield: \|mutate item: T\| -> bool)` |
| **SEQ3: Yield return** | `yield` returns `true` to continue, `false` to stop. The sequence must honor the return — on `false`, stop yielding and return |

<!-- test: parse -->
```rask
type alias Sequence<T> = func(yield: |T| -> bool)
type alias SequenceMut<T> = func(yield: |mutate item: T| -> bool)
```

A `Sequence<T>` is a first-class value. It can be stored, passed, returned — subject to the same scope rules as any closure (`mem.closures/SL1-SL2`).

## For-Loop Desugaring

| Rule | Description |
|------|-------------|
| **SEQ4: Range loops** | `for x in range` — built-in range loop, no closure call |
| **SEQ5: Built-in collections** | `for x in vec` / `for mutate x in vec` — inline-alias desugar (`ctrl.loops/LP17`), no `Sequence` involved |
| **SEQ6: Custom types** | `for x in seq_expr { body }` where `seq_expr: Sequence<T>` desugars to a yield-closure call |
| **SEQ7: Break/continue translation** | Inside the desugared closure: `break` becomes `return false`, `continue` becomes `return true`. The closure returns `true` at end-of-body |
| **SEQ8: Return propagation** | `return` inside a for-body exits the enclosing function, not the yield closure — compiler translates via a flag |

<!-- test: parse -->
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

## Laziness and Re-Consumption

| Rule | Description |
|------|-------------|
| **SEQ9: Lazy construction** | Building a `Sequence<T>` runs nothing. Adapter chains (`.filter().map()`) compose closures without executing |
| **SEQ10: Eager consumption** | The chain runs when consumed by a for-loop or a terminal operation |
| **SEQ11: Re-consumption runs again** | A `Sequence<T>` is a function value. Calling it twice runs the underlying traversal twice. Side effects repeat |

<!-- test: skip -->
```rask
const s = users.iter().filter(|u| u.active)
// nothing has run yet

for u in s { print(u.name) }     // runs the chain
const count = s.count()          // runs the chain AGAIN
```

To consume twice without re-running, materialize with `.collect()`:

<!-- test: skip -->
```rask
const active = users.iter().filter(|u| u.active).collect()
for u in active { print(u.name) }
const count = active.len()
```

## Authoring Sequences

A method returning `Sequence<T>` constructs a closure. The closure captures `self` (or whatever source it walks). Per closure rules, the resulting `Sequence<T>` is scope-limited to the captured source's lifetime.

**Pool-backed tree:**

<!-- test: skip -->
```rask
struct Tree<T> {
    public nodes: Pool<Node<T>>
    public root: Handle<Node<T>>?
}

struct Node<T> {
    public value: T
    public left: Handle<Node<T>>?
    public right: Handle<Node<T>>?
}

extend Tree<T> {
    public func in_order(self) -> Sequence<Node<T>> {
        return |yield| {
            walk(self.nodes, self.root, yield)
        }
    }
}

func walk<T>(
    nodes: Pool<Node<T>>,
    h: Handle<Node<T>>?,
    yield: |Node<T>| -> bool,
) -> bool {
    if h is Some(handle) {
        if not walk(nodes, nodes[handle].left, yield): return false
        if not yield(nodes[handle]): return false
        if not walk(nodes, nodes[handle].right, yield): return false
    }
    return true
}
```

**Owned-recursive tree:**

<!-- test: skip -->
```rask
struct Tree<T> { public root: Owned<Node<T>>? }

struct Node<T> {
    public value: T
    public left: Owned<Node<T>>?
    public right: Owned<Node<T>>?
}

extend Tree<T> {
    public func in_order(self) -> Sequence<Node<T>> {
        return |yield| {
            if self.root is Some(r): walk(*r, yield)
        }
    }
}

func walk<T>(node: Node<T>, yield: |Node<T>| -> bool) -> bool {
    if node.left is Some(l): if not walk(*l, yield): return false
    if not yield(node): return false
    if node.right is Some(r): if not walk(*r, yield): return false
    return true
}
```

**From a channel:**

The receiver must be owned by the Sequence because the closure calls `recv()` *after* the method that built it has returned. Any capture of `rx` by borrow would make the closure expression-scoped — it would not type-check at the storage or return site.

<!-- test: skip -->
```rask
extend Receiver<T> {
    public func stream(take self) -> Sequence<T> {
        return |yield| {
            loop {
                const msg = match self.recv() {
                    Ok(m) => m,
                    Err(_) => break,
                }
                if not yield(msg): break
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

Adapters are plain generic functions. They take a `Sequence<T>` and return a new one. Chaining uses Rask's method-call syntax via the extension model.

| Rule | Description |
|------|-------------|
| **SEQ12: Adapter shape** | Adapters are `public func name<T, ...>(seq: Sequence<T>, ...) -> Sequence<U>` |
| **SEQ13: Chain syntax** | `seq.adapter(args)` resolves via extension — identical surface to method calls |
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
| `collect()` | Materialize into a `Vec<T>` | `Vec<T>` |
| `collect<C>()` | Materialize into `C: FromSequence<T>` | `C` |
| `fold(init, f)` | Reduce with initial | `A` |
| `reduce(f)` | Reduce without initial | `T?` (None if empty) |
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
const total = orders.iter().map(|o| o.amount).sum()
const admin = users.iter().find(|u| u.is_admin)
const active = users.iter().filter(|u| u.active).collect()
```

## Lockstep Iteration

| Rule | Description |
|------|-------------|
| **SEQ14: No general zip** | Rask does not provide a general `zip` adapter on `Sequence<T>`. Lockstep over arbitrary sequences would require coroutines or buffering, both of which hide cost |
| **SEQ15: Indexable lockstep** | For indexable sources (Vec, array, Pool+handles), use index iteration: `for i in 0..min(a.len(), b.len()) { use(a[i], b[i]) }` |
| **SEQ16: Non-indexable lockstep** | Non-indexable sources must buffer explicitly. The allocation is visible in the code |

<!-- test: parse -->
```rask
// Indexable (common case, zero-cost)
for i in 0..min(a.len(), b.len()) {
    process(a[i], b[i])
}

// Non-indexable (rare — explicit buffer shows the cost)
const a_items = tree_a.in_order().collect()
mut idx = 0
tree_b.in_order()(|b_node| {
    if idx >= a_items.len(): return false
    process(a_items[idx], b_node)
    idx += 1
    return true
})
```

## Zero-Cost Contract

| Rule | Description |
|------|-------------|
| **SEQ17: Inlining required** | The compiler inlines yield closures at every call site in the sequence body. Adapter closures inline into their inner sequence |
| **SEQ18: Fusion** | Adapter chains (`.filter().map().take()`) compile to a single fused loop, equivalent to a hand-written version |
| **SEQ19: Verified** | Compiler test `compiler/tests/sequence_fusion.rs` verifies MIR output for canonical adapter chains matches the hand-written equivalent |

This is a hard contract. A benchmark regression in adapter fusion is a compiler bug.

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
        .collect()                     // Materialized here — no Sequence escapes
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
                if not yield(u): return
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

FIX 1: Consume inside the function (collect, fold, for-loop):

  return users.iter().filter(|u| u.active).collect()

FIX 2: Take ownership of the source:

  func active(take users: Vec<User>) -> Sequence<User> { ... }
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

  const result = seq.find(|x| matches(x))

  // or
  mut found: T? = None
  for x in seq {
      if matches(x) { found = Some(x); break }
  }
```

## Edge Cases

| Case | Rule | Behavior |
|------|------|----------|
| Empty sequence | SEQ1 | For-loop body never runs |
| Break in sequence body | SEQ7 | Yield closure returns `false`; sequence must stop |
| Continue in sequence body | SEQ7 | Yield closure returns `true` |
| Sequence yields owned non-Copy | SEQ1 | Each yield moves the value to the closure |
| Sequence yields borrow | SEQ1 | Each yield passes a borrow for the closure duration only |
| SequenceMut yields mutable | SEQ2 | Each yield passes a fresh mutable borrow; ends when closure returns |
| Re-consuming a Sequence | SEQ11 | Runs the chain again; side effects repeat |
| Returning a Sequence from a function | SEQ-scope | Only allowed when source is owned by the closure |
| Storing a Sequence in a struct | SEQ-scope | Only allowed when source is owned by the closure (no external borrows) |
| Sending a Sequence cross-task | SEQ-scope | Same rule as sending a closure (`mem.closures`) |
| Infinite sequence with `.take(n)` | SEQ10 | Terminates after n yields |
| Empty chain with `.sum()` | — | Zero value for the type |
| Empty chain with `.reduce()` | — | None |
| Empty chain with `.min()`/`.max()` | — | None |

---

## Appendix (non-normative)

### Rationale

**Why push over pull.** Rask's foundational rule is "no storable references" (`mem.relocatable`). A pull iterator must remember its position across `next()` calls — for anything more complex than a flat array, that position is a reference or pointer into the source. Pull fights the foundation. Push puts traversal state on the real call stack, where it's scoped correctly by construction.

**Why not generators.** A generator function with a `yield` keyword compiles to a state machine that stores locals across pause points. When those locals include borrows into the generator's own state, you get the self-reference problem — the reason Rust needs `Pin`. Rask avoids the whole category by not synthesizing state machines.

**Why not effect handlers.** The 2025 OOPSLA work on zero-overhead lexical effect handlers proves that tail-resumptive handlers (which iteration is) can be zero-cost. But effect handlers are a paradigm, not a feature. Adopting them for iteration alone brings compiler complexity across the whole language for a narrow benefit. Rask's only "effect" system is `using` context, and that stays.

**Why no zip.** General zip over two `Sequence<T>` values requires either buffering (hidden allocation), a green task (not universally available — `conc.async/C1`), or a compiler-synthesized state machine (effectively generators, rejected above). All three hide cost. Indices cover the real use case; explicit buffer covers the rest; cost is visible either way. This matches `core-design/transparency-of-cost`.

**Why SequenceMut is separate.** One unified `Sequence<T, M>` with a mode parameter is possible but makes the signatures noisier without helping users — the two cases are used differently and rarely mixed. Two aliases keep each simple.

**Re-consumption runs twice.** Rust's pull iterators solve this by consuming on use (the iterator is dropped after `.collect()`). Push sequences are function values — no consumption. The tradeoff is visibility: in exchange for simpler authoring, users see a footgun around side-effectful traversals. Documented, not eliminated.

### Migration from `type.iterators`

The retired `Iterator<Item>` trait mapped to these patterns:

| Old | New |
|-----|-----|
| `extend MyType with Iterator<T> { func next(...) }` | `public func iter(self) -> Sequence<T> { return \|yield\| { ... } }` |
| `collection.iterate()` (returned `VecRefIterator<T>` etc.) | `collection.iter()` returns `Sequence<T>` |
| `.take_all()` returning consuming iterator struct | `.take_all()` returns `Sequence<T>` yielding owned items |
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
- [Channels](../concurrency/sync.md) — Streaming via `recv()` in a for-loop
