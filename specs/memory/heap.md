<!-- id: mem.heap -->
<!-- status: decided -->
<!-- summary: Heap<T> — the one-pointer indirection for recursive and large values; linear, zero-overhead -->
<!-- depends: memory/linear.md, memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-interp/ -->

# Heap Values

`Heap<T>` is a linear heap pointer. `Heap(…)` allocates, the linearity rules (`mem.linear`) guarantee safety at compile time, zero runtime overhead.

The consume-exactly-once rules live in [`mem.linear`](linear.md) and apply identically to `@resource` structs, `Heap<T>`, and linear elements in pools. This spec describes the `Heap<T>` type, the `Heap(…)` operator, and patterns specific to recursive types and single-owner heap values.

## Allocation and Usage

| Syntax | Meaning |
|--------|---------|
| `Heap(expr)` | Heap-allocate expr, return `Heap<T>` |
| `Heap<T>` | Linear owning heap pointer |
| `*ptr` | Dereference (borrow the inner value) |
| `drop(ptr)` | Consume and deallocate |

<!-- test: parse -->
```rask
let ptr: Heap<i32> = Heap(42)    // Allocate on heap
let value = *ptr                // Dereference (borrow)
drop(ptr)                         // Consume (deallocate)
```

## Type Properties

| Property | Value |
|----------|-------|
| Size | 8 bytes (pointer) |
| Copy | No (linear) |
| Cloneable | Yes, if T: Cloneable (explicit `.clone()` required) |
| Default | No |

## Linearity Rules

`Heap<T>` is linear: it follows the rules in `mem.linear/L1–L7`. The table below maps the rule identifiers other specs cite to their canonical source.

| Rule | Citation | Description |
|------|----------|-------------|
| **HP1** | `mem.linear/L1` | Must be consumed before scope exit |
| **HP2** | `mem.linear/L2` | Cannot be consumed twice |
| **HP3** | `mem.linear/L3` | Dereference (borrow) does not consume |
| **HP4** | `mem.linear/L5` | Passing to `take` or assigning to another binding consumes |

Consumption methods: `drop(ptr)`, passing to a `take` parameter, assignment to another binding, or `ensure drop(ptr)` for deferred consumption.

**Storing one in an aggregate ends its linearity.** HP4 says assigning to another binding consumes, and a struct field, an enum payload, a tuple element and an array element are each another binding — so the `Heap` is the aggregate's from then on, and the aggregate's release gives the block back when the aggregate dies. The slot is not a second linear thing: there is nothing left to consume, and `drop(h.inner)` is an error rather than a consume, because the hand-drop and the release would both free it.

Reading one back out is a borrow for the same reason. `match l { Cons(head, rest) => … }` gives `rest` no obligation, and `Cons(_, rest)` discards nothing — the enum still owns the block either way. A `@resource` is the opposite: there are no destructors, so nothing but an explicit consume ever closes one, and matching it out of an enum is the last chance to.

A wrapper is not an aggregate. `Heap<T>?` and `Heap<T> or E` carry their payload linearly, because nothing walks a wrapper and consumes what is behind its tag — `o? as v` moves the obligation to `v` instead.

I went back and forth on making that `drop` a silent no-op instead. It's worse: the author writes a consume and gets none, and whether `drop(x)` frees anything then depends on whether `x` is a binding or a projection, which the line doesn't say.

<!-- test: skip -->
```rask
struct Holder {
    inner: Heap<Big>
}

let h = Holder { inner: Heap(Big { a: 1, b: 2, c: 3 }) }
assert h.inner.b == 2
// drop(h.inner)              // error: `h` gives it back already
// nothing here — `h` going out of scope frees the block
```

This is what makes a recursive type work. `Cons(i64, Heap<List>)` holds its tail in a field, so each node owns the one after it, and releasing the head releases the whole chain.

<!-- test: parse -->
```rask
func process(take ptr: Heap<Data>) {
    // ptr consumed when function takes ownership
}

let ptr = Heap(Data { value: 42 })
process(ptr)                  // Consumed by move
// ptr no longer valid here
```

## Dereferencing and Borrowing

Dereferencing borrows the inner value without consuming the `Heap<T>`. Borrow rules follow standard borrowing (`mem.borrowing/S5`).

<!-- test: parse -->
```rask
let ptr = Heap(Point { x: 1, y: 2 })

let x = (*ptr).x               // Borrow for read
(*ptr).x = 10                    // Borrow for mutate

// Still valid, not consumed
drop(ptr)                         // Now consumed
```

## Type Checking

| Rule | Description |
|------|-------------|
| **HP5: Transparent** | `Heap<T>` unifies with `T` in type checking; code accepting `T` also accepts `Heap<T>` |

HP5 is a deliberate simplification — auto-deref without ceremony. It is a rule in unification, not an erasure: `Heap<T>` is still a type that says there is a block, it just fits wherever a `T` does.

That distinction is worth stating because the implementation got it wrong for a while. Unwrapping `Heap<T>` to `T` at parse gives you the same transparency and throws the block away with it — `func() -> Heap<i64>` was checked as `func() -> i64`, and nothing downstream could tell a block from the value in it. `*b` on one read a stack address (#1256).

Linearity is enforced for a `Heap(…)` local: one that nothing consumes is an error, and so is a second consume. Consuming means `drop(name)`, handing it to a `take` parameter, storing it in a field, tuple, array or enum payload, or returning it. `ensure` covers the error paths. This is the same rule set `@resource` follows.

`Heap(…)` allocates exactly one block, whatever the payload. A number gets one too: it would fit the slot, and skipping the allocation for the payloads that do made `Heap<T>` two shapes rather than one — what `*b` reads, what `drop(b)` frees and what a field holds each had two answers, and the compiler picked between them from where the value came from rather than from its type. That fails the moment the value crosses a carrier, because a struct field or a tuple holds a value and not a binding (#1234). One allocation per `Heap(…)` is the cost, and `Heap(…)` is written where the cost is.

| Rule | Description |
|------|-------------|
| **HP5a: One allocation per `Heap(…)`** | `Heap(expr)` allocates exactly once. Moving the result — into a binding, a field, or an enum payload — moves the pointer; it does not allocate again |

## Allocation

| Rule | Description |
|------|-------------|
| **HP6: Context allocator** | `Heap(…)` allocates using the context allocator inherited from the caller |

<!-- test: skip -->
```rask
func build_tree() -> Heap<Tree<i32>> {
    return Heap(Node(Heap(Leaf(1)), Heap(Leaf(2))))
}

func main() {
    let tree = build_tree()     // Uses default system allocator

    with context.allocator = arena {
        let tree2 = build_tree()  // Allocated in arena
    }
}
```

| Context | Behavior |
|---------|----------|
| Default | System allocator (malloc/free equivalent) |
| Custom allocator | Uses provided allocator |
| Arena context | Allocated in arena (bulk free on arena drop) |

## Null-Pointer Optimization

| Rule | Description |
|------|-------------|
| **HP7: Null optimization** | `Heap<T>?` uses null-pointer optimization — same size as `Heap<T>` (8 bytes) |

| Value | Representation |
|-------|----------------|
| `none` | Null pointer (0x0) |
| present | Non-null pointer |

## Recursive Types

The primary use case. A type can't contain itself without indirection.

<!-- test: parse -->
```rask
enum Tree<T> {
    Leaf(T)
    Node(Heap<Tree<T>>, Heap<Tree<T>>)
}

enum List<T> {
    Nil
    Cons(T, Heap<List<T>>)
}

let tree = Tree.Node(Heap(Tree.Leaf(1)), Heap(Tree.Leaf(2)))
let list = List.Cons(1, Heap(List.Cons(2, Heap(List.Nil))))
```

Self-referential types without indirection are rejected:
<!-- test: parse -->
```rask
enum Bad {
    Node(i32, Bad)  // ERROR: infinite size, use Heap<Bad>
}
```

## Pattern Matching

Pattern matching on `Heap<T>` can destructure and consume:

<!-- test: parse -->
```rask
enum Expr {
    Num(i32)
    Add(Heap<Expr>, Heap<Expr>)
}

func eval(take expr: Heap<Expr>) -> i32 {
    match *expr {
        Expr.Num(n) => return n,
        Expr.Add(left, right) => return eval(left) + eval(right),
    }
}
```

## Cloneable

If `T: Cloneable`, then `Heap<T>: Cloneable`. Cloning allocates a new heap value. Clone is explicit — no implicit copying.

<!-- test: parse -->
```rask
let ptr1 = Heap(Point { x: 1, y: 2 })
let ptr2 = ptr1.clone()            // New allocation, deep copy

drop(ptr1)
drop(ptr2)
```

## Drop Behavior

When `Heap<T>` is consumed via `drop()` or scope exit (after `ensure`):

1. If `T` has a destructor, run it
2. Deallocate memory via the allocator that allocated it

<!-- test: parse -->
```rask
@resource
struct File { handle: RawHandle }

let file_ptr = Heap(try File.open("data.txt"))
// ... use file ...
drop(file_ptr)  // Runs File destructor, then frees memory
```

## Error Messages

**Heap value not consumed [L1]:**
```
ERROR [mem.linear/L1]: Heap<i32> not consumed before scope exit
   |
5  |  }
   |  ^ scope ends without consuming 'ptr'

WHY: Heap values must be consumed exactly once to prevent memory leaks.

FIX: Consume the value with drop(), pass to a function, or use ensure:

  drop(ptr)
```

**Double consumption [L2]:**
```
ERROR [mem.linear/L2]: ptr already consumed
   |
4  |  drop(ptr)
   |       ^^^ consumed here
5  |  drop(ptr)
   |       ^^^ cannot consume again

WHY: Heap values can only be consumed once. Double-free is undefined behavior.

FIX: Remove the second consumption.
```

**Use after move [L5]:**
```
ERROR [mem.linear/L5]: ptr used after move
   |
3  |  let other = ptr
   |                ^^^ moved here
4  |  drop(ptr)
   |       ^^^ ptr is invalid after move

WHY: Assignment transfers ownership. The original binding is no longer valid.

FIX: Use the new binding instead:

  drop(other)
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| `Heap(…)` in loop | L1 | Each iteration allocates; each must be consumed |
| `Heap<Heap<T>>` | — | Valid but unusual; double indirection |
| Zero-sized T | — | Valid; allocates minimal (may optimize to no-op) |
| `Heap<[T; N]>` | — | Valid; heap-allocated array |
| Recursive drop | L1 | Dropping a tree drops children recursively |
| `Heap<T>` in error path | L1 | Must be consumed or registered with `ensure` |

---

## Appendix (non-normative)

### Rationale

**Why the name changed from `Owned<T>`.** `Owned` named single ownership, which every Rask value already has — so it distinguished nothing and implied that unwrapped values were somehow unowned. What actually differs from a plain field is the indirection: the value lives on the heap instead of inline. That's also where the cost is, and principle 1 says allocations belong in the source. `Heap<T>` names it.

The name a binary heap would want is a casualty I'll take: a priority queue should be called `PriorityQueue<T>` anyway — "heap" there is an implementation detail, exactly the lifted-from-`std` naming `std.stdlib/SD*` warns against. Full argument in `analysis.fourth-option-naming`.

**Why `Heap(expr)` instead of `own expr`.** `own` was doing two unrelated jobs — heap-allocate here, move-capture there — and you couldn't tell which from the syntax: `f(x)` moves, `Node(x)` allocated. An ordinary call removes the ambiguity and drops a keyword. `own` now means move, everywhere.

**Why linear heap pointers?** I wanted heap allocation with no runtime overhead at all. `Heap<T>` has exactly one owner, so the compiler tracks it statically via the linearity rules (`mem.linear`). Use-after-free, double-free and leaks are all prevented with nothing left to check at run time.

**Why the same rules as `@resource`?** Both `Heap<T>` and `@resource` structs are linear values — the compiler uses one rule set (`mem.linear/L1–L7`) for both. A reader who understands `@resource` already understands `Heap<T>`; only the use cases differ.

**HP5 (transparent type checking):** I don't want `Heap<T>` to infect every function signature. If a function takes `T`, it should accept `Heap<T>` with auto-deref. The alternative — explicit unwrapping everywhere — adds noise without safety benefit.

**HP6 (context allocator):** `Heap(…)` uses the context allocator so arena allocation works without changing call sites. Build a tree with the system allocator, or build it in an arena — same code.

**HP7 (null optimization):** `Heap<T>?` is the natural way to express optional tree children. Null-pointer optimization keeps it at 8 bytes — same as a raw pointer.

### Patterns & Guidance

**When to use `Heap<T>` vs `Link<T>`:**

| Aspect | `Heap<T>` | `Link<T>` |
|--------|------------|-----------|
| Safety mechanism | Linearity (compile-time) | Delete-time edge fixup (`mem.racks/RK3`) |
| Aliasing | Single owner only | Many links to one node |
| Overhead | None | One extra cache access per edge *write*; reads are a plain deref |
| Size | 8 bytes | 8 bytes |
| Use case | Recursive types, single ownership | Graphs whose nodes get deleted individually |

- `Heap<T>`: Tree nodes, AST nodes, single-owner heap values
- `Link<T>`: Entity systems, graphs with cycles, observer patterns

**AST pattern:**

<!-- test: parse -->
```rask
enum Stmt {
    Let(string, Heap<Expr>)
    Return(Heap<Expr>)
    Block(Vec<Heap<Stmt>>)
}

enum Expr {
    Literal(i64)
    Binary(BinOp, Heap<Expr>, Heap<Expr>)
    Call(string, Vec<Heap<Expr>>)
}
```

### IDE Integration

| Context | Annotation |
|---------|------------|
| Heap binding | `[linear: must consume]` |
| After move | `[moved: line N]` |
| After drop | `[consumed: line N]` |

### See Also

- [Linearity](linear.md) — Rule set (L1–L7) shared by `@resource` and `Heap<T>` (`mem.linear`)
- [Shared, Rack and Heap](shared-rack-heap.md) — `Heap<T>` beside the other two (`mem.shared-rack-heap`)
- [Ownership](ownership.md) — Single-owner model (`mem.ownership`)
- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value`)
- [Borrowing](borrowing.md) — Scoped borrowing rules (`mem.borrowing`)
- [Resource Types](resource-types.md) — `@resource` struct annotation (`mem.resources`)
- [Racks and Links](racks.md) — many nodes, individually deleted (`mem.racks`)
