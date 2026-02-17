<!-- id: mem.owned -->
<!-- status: decided -->
<!-- summary: Linear heap pointer for recursive types; own keyword, zero-overhead, compile-time safety -->
<!-- depends: memory/ownership.md, memory/value-semantics.md -->
<!-- implemented-by: compiler/crates/rask-types/, compiler/crates/rask-interp/ -->

# Owned Pointers

`Owned<T>` is a linear heap pointer. `own` allocates, linearity guarantees safety at compile time, zero runtime overhead.

## Allocation and Usage

| Syntax | Meaning |
|--------|---------|
| `own expr` | Heap-allocate expr, return `Owned<T>` |
| `Owned<T>` | Linear owning heap pointer |
| `*ptr` | Dereference (borrow the inner value) |
| `drop(ptr)` | Consume and deallocate |

<!-- test: skip -->
```rask
const ptr: Owned<i32> = own 42    // Allocate on heap
const value = *ptr                // Dereference (borrow)
drop(ptr)                         // Consume (deallocate)
```

## Type Properties

| Property | Value |
|----------|-------|
| Size | 8 bytes (pointer) |
| Copy | No (linear) |
| Clone | Yes, if T: Clone (explicit `.clone()` required) |
| Default | No |

## Linearity Rules

`Owned<T>` is linear: must be consumed exactly once.

| Rule | Description |
|------|-------------|
| **OW1: Must consume** | Owned value must be consumed before scope exit |
| **OW2: Consume once** | Cannot consume same Owned value twice |
| **OW3: Borrow allowed** | Can dereference for reading/writing without consuming |
| **OW4: Move consumes** | Passing to a function or assigning to another variable moves (consumes) |

<!-- test: skip -->
```rask
func process(take ptr: Owned<Data>) {
    // ptr consumed when function takes ownership
}

const ptr = own Data { value: 42 }
process(own ptr)                  // Consumed by move
// ptr no longer valid here
```

## Dereferencing and Borrowing

Dereferencing borrows the inner value without consuming the `Owned<T>`. Borrow rules follow standard borrowing (`mem.borrowing/S5`).

<!-- test: skip -->
```rask
const ptr = own Point { x: 1, y: 2 }

const x = (*ptr).x               // Borrow for read
(*ptr).x = 10                    // Borrow for mutate

// Still valid, not consumed
drop(ptr)                         // Now consumed
```

## Type Checking

| Rule | Description |
|------|-------------|
| **OW5: Transparent** | `Owned<T>` unifies with `T` in type checking; code accepting `T` also accepts `Owned<T>` |

Full linearity enforcement (OW1-OW4) is a Phase 4 compiler feature. OW5 is a deliberate simplification — auto-deref without ceremony.

## Allocation

| Rule | Description |
|------|-------------|
| **OW6: Context allocator** | `own` allocates using the context allocator inherited from the caller |

<!-- test: skip -->
```rask
func build_tree() -> Owned<Tree<i32>> {
    return own Node(own Leaf(1), own Leaf(2))
}

func main() {
    const tree = build_tree()     // Uses default system allocator

    with context.allocator = arena {
        const tree2 = build_tree()  // Allocated in arena
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
| **OW7: Null optimization** | `Option<Owned<T>>` uses null-pointer optimization — same size as `Owned<T>` (8 bytes) |

| Value | Representation |
|-------|----------------|
| `None` | Null pointer (0x0) |
| `Some(ptr)` | Non-null pointer |

## Recursive Types

The primary use case. A type can't contain itself without indirection.

<!-- test: skip -->
```rask
enum Tree<T> {
    Leaf(T)
    Node(Owned<Tree<T>>, Owned<Tree<T>>)
}

enum List<T> {
    Nil
    Cons(T, Owned<List<T>>)
}

const tree = Tree.Node(own Tree.Leaf(1), own Tree.Leaf(2))
const list = List.Cons(1, own List.Cons(2, own List.Nil))
```

Self-referential types without indirection are rejected:
<!-- test: skip -->
```rask
enum Bad {
    Node(i32, Bad)  // ERROR: infinite size, use Owned<Bad>
}
```

## Pattern Matching

Pattern matching on `Owned<T>` can destructure and consume:

<!-- test: skip -->
```rask
enum Expr {
    Num(i32)
    Add(Owned<Expr>, Owned<Expr>)
}

func eval(take expr: Owned<Expr>) -> i32 {
    match *expr {
        Expr.Num(n) => return n,
        Expr.Add(left, right) => return eval(own left) + eval(own right),
    }
}
```

## Clone

If `T: Clone`, then `Owned<T>: Clone`. Cloning allocates a new heap value. Clone is explicit — no implicit copying.

<!-- test: skip -->
```rask
const ptr1 = own Point { x: 1, y: 2 }
const ptr2 = ptr1.clone()            // New allocation, deep copy

drop(ptr1)
drop(ptr2)
```

## Drop Behavior

When `Owned<T>` is consumed via `drop()` or scope exit (after `ensure`):

1. If `T` has a destructor, run it
2. Deallocate memory via the allocator that allocated it

<!-- test: skip -->
```rask
@resource
struct File { handle: RawHandle }

const file_ptr = own (try File.open("data.txt"))
// ... use file ...
drop(file_ptr)  // Runs File destructor, then frees memory
```

## Error Messages

**Owned value not consumed [OW1]:**
```
ERROR [mem.owned/OW1]: Owned<i32> not consumed before scope exit
   |
5  |  }
   |  ^ scope ends without consuming 'ptr'

WHY: Owned values must be consumed exactly once to prevent memory leaks.

FIX: Consume the value with drop(), pass to a function, or use ensure:

  drop(ptr)
```

**Double consumption [OW2]:**
```
ERROR [mem.owned/OW2]: ptr already consumed
   |
4  |  drop(ptr)
   |       ^^^ consumed here
5  |  drop(ptr)
   |       ^^^ cannot consume again

WHY: Owned values can only be consumed once. Double-free is undefined behavior.

FIX: Remove the second consumption.
```

**Use after move [OW4]:**
```
ERROR [mem.owned/OW4]: ptr used after move
   |
3  |  const other = ptr
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
| `own` in loop | OW1 | Each iteration allocates; each must be consumed |
| `Owned<Owned<T>>` | — | Valid but unusual; double indirection |
| Zero-sized T | — | Valid; allocates minimal (may optimize to no-op) |
| `Owned<[T; N]>` | — | Valid; heap-allocated array |
| Recursive drop | OW1 | Dropping a tree drops children recursively |
| `Owned<T>` in error path | OW1 | Must be consumed or registered with `ensure` |

---

## Appendix (non-normative)

### Rationale

**OW1-OW4 (linearity):** I wanted heap allocation without runtime overhead. `Handle<T>` uses generation checks — safe but costs 4+ bytes and a branch on every access. `Owned<T>` has exactly one owner, so the compiler can track it statically. Linearity prevents use-after-free, double-free, and leaks without any runtime cost.

**OW5 (transparent type checking):** I don't want `Owned<T>` to infect every function signature. If a function takes `T`, it should accept `Owned<T>` with auto-deref. The alternative — explicit unwrapping everywhere — adds noise without safety benefit.

**OW6 (context allocator):** `own` uses the context allocator so arena allocation works without changing call sites. Build a tree with the system allocator, or build it in an arena — same code.

**OW7 (null optimization):** `Option<Owned<T>>` is the natural way to express optional tree children. Null-pointer optimization keeps it at 8 bytes — same as a raw pointer.

### Patterns & Guidance

**When to use `Owned<T>` vs `Handle<T>`:**

| Aspect | `Owned<T>` | `Handle<T>` |
|--------|------------|-------------|
| Safety mechanism | Linearity (compile-time) | Generation check (runtime) |
| Aliasing | Single owner only | Multiple handles allowed |
| Overhead | None | 4+ bytes for generation, check on access |
| Size | 8 bytes | 12 bytes (default) |
| Use case | Recursive types, single ownership | Collections, graphs, shared references |

- `Owned<T>`: Tree nodes, AST nodes, single-owner heap values
- `Handle<T>`: Entity systems, graphs with cycles, observer patterns

**AST pattern:**

<!-- test: skip -->
```rask
enum Stmt {
    Let(string, Owned<Expr>)
    Return(Owned<Expr>)
    Block(Vec<Owned<Stmt>>)
}

enum Expr {
    Literal(i64)
    Binary(BinOp, Owned<Expr>, Owned<Expr>)
    Call(string, Vec<Owned<Expr>>)
}
```

### IDE Integration

| Context | Annotation |
|---------|------------|
| Owned binding | `[linear: must consume]` |
| After move | `[moved: line N]` |
| After drop | `[consumed: line N]` |

### See Also

- [Ownership](ownership.md) — Single-owner model (`mem.ownership`)
- [Value Semantics](value-semantics.md) — Copy vs move behavior (`mem.value`)
- [Borrowing](borrowing.md) — Scoped borrowing rules (`mem.borrowing`)
- [Resource Types](resource-types.md) — Must-consume resources (`mem.resources`)
- [Pools](pools.md) — Handle-based indirection (`mem.pools`)
