# Solution: Owned Pointers

## The Question
How do we provide heap-allocated values with single ownership for recursive data structures and large values, without runtime overhead?

## Decision
`Owned<T>` is a linear heap pointer. The `own` keyword allocates and returns an `Owned<T>`. Safety comes from linearity (compile-time), not generation checks (runtime). This is the same as a Box<T> in Rust.

## Rationale
Recursive types (trees, linked lists, ASTs) require indirection—a type cannot contain itself directly. `Owned<T>` provides this indirection with:

- **Compile-time safety:** Linearity prevents use-after-free, double-free, and leaks
- **Zero runtime overhead:** No generation checks, no reference counting
- **Visible allocation:** The `own` keyword marks allocation sites explicitly

Unlike `Handle<T>` (which uses generation checks for safety), `Owned<T>` relies entirely on the compiler. This is possible because `Owned<T>` has exactly one owner—there's no aliasing to track at runtime.

## Specification

### Basic Usage

```rask
let ptr: Owned<i32> = own 42       // Allocate on heap
const value = *ptr                    // Dereference (borrow)
drop(ptr)                           // Consume (deallocate)
```

| Syntax | Meaning |
|--------|---------|
| `own expr` | Heap-allocate expr, return `Owned<T>` |
| `Owned<T>` | Linear owning heap pointer |
| `*ptr` | Dereference (borrow the inner value) |
| `drop(ptr)` | Consume and deallocate |

### Type Properties

| Property | Value |
|----------|-------|
| Size | 8 bytes (pointer) |
| Copy | No (linear) |
| Clone | Yes, if T: Clone (explicit `.clone()` required) |
| Default | No |

### Linearity Rules

`Owned<T>` is linear: must be consumed exactly once.

| Rule | Description |
|------|-------------|
| **O1: Must consume** | Owned value must be consumed before scope exit |
| **O2: Consume once** | Cannot consume same Owned value twice |
| **O3: Borrow allowed** | Can dereference for reading/writing without consuming |
| **O4: Move consumes** | Passing to a function or assigning to another variable moves (consumes) |

**Valid consumption:**
```rask
func process(take ptr: Owned<Data>) { ... }  // Function takes ownership

const ptr = own Data{...}
process(ptr)                                // Consumed by move
// ptr no longer valid here
```

**Compiler errors:**
```rask
const ptr = own 42
// scope ends without consuming ptr
// ❌ ERROR: Owned<i32> not consumed before scope exit

const ptr = own 42
drop(ptr)
drop(ptr)  // ❌ ERROR: ptr already consumed
```

### Dereferencing and Borrowing

Dereferencing borrows the inner value without consuming the `Owned<T>`:

```rask
const ptr = own Point{x: 1, y: 2}

// Read borrow
const x = (*ptr).x                  // Borrow for read

// Mutate borrow
(*ptr).x = 10                     // Borrow for mutate

// Still valid, not consumed
drop(ptr)                         // Now consumed
```

Borrow rules follow standard borrowing (B1-B5 from borrowing.md):
- Cannot have mutable borrow while other borrows exist
- Borrows must not outlive the `Owned<T>`

### Recursive Types

The primary use case for `Owned<T>` is recursive type definitions:

```rask
enum Tree<T> {
    Leaf(T),
    Node(Owned<Tree<T>>, Owned<Tree<T>>)
}

enum List<T> {
    Nil,
    Cons(T, Owned<List<T>>)
}

// Construction
const tree = Node(own Leaf(1), own Leaf(2))
const list = Cons(1, own Cons(2, own Nil))
```

**Compiler requirement:** Self-referential types without indirection are rejected:
```rask
enum Bad {
    Node(i32, Bad)  // ❌ ERROR: infinite size, use Owned<Bad>
}
```

### Allocation Strategy

The `own` keyword allocates using the **context allocator**:

```rask
func build_tree() -> Owned<Tree<i32>> {
    // Uses context.allocator (inherited from caller)
    own Node(own Leaf(1), own Leaf(2))
}

func main() {
    // Default context uses system allocator
    const tree = build_tree()

    // Custom allocator via context
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

### Comparison with Handle<T>

| Aspect | `Owned<T>` | `Handle<T>` |
|--------|------------|-------------|
| Safety mechanism | Linearity (compile-time) | Generation check (runtime) |
| Aliasing | Single owner only | Multiple handles allowed |
| Overhead | None | 4+ bytes for generation, check on access |
| Size | 8 bytes | 12 bytes (default) |
| Use case | Recursive types, single ownership | Collections, graphs, shared references |
| Invalidation | Compiler tracks | Runtime detection |

**When to use which:**
- `Owned<T>`: Tree nodes, AST nodes, single-owner heap values
- `Handle<T>`: Entity systems, graphs with cycles, observer patterns

### Pattern Matching

Pattern matching on `Owned<T>` can destructure and consume:

```rask
enum Expr {
    Num(i32),
    Add(Owned<Expr>, Owned<Expr>)
}

func eval(take expr: Owned<Expr>) -> i32 {
    match *expr {
        Num(n) => n,
        Add(left, right) => eval(left) + eval(right)
    }
    // expr consumed by match destructuring
}
```

### Clone

If `T: Clone`, then `Owned<T>: Clone`. Cloning allocates a new heap value:

```rask
const ptr1 = own Point{x: 1, y: 2}
const ptr2 = ptr1.clone()           // New allocation, deep copy

// Both must be consumed
drop(ptr1)
drop(ptr2)
```

Clone is explicit (`.clone()` call required)—no implicit copying.

### Drop Behavior

When `Owned<T>` is consumed via `drop()` or scope exit (after `ensure`):

1. If `T` has a destructor, run it
2. Deallocate memory via the allocator that allocated it

```rask
@linear
struct File { handle: RawHandle }

const file_ptr = own File.open("data.txt")?
// ... use file ...
drop(file_ptr)  // Runs File destructor, then frees memory
```

### Null-Pointer Optimization

`Option<Owned<T>>` uses null-pointer optimization:

| Value | Representation |
|-------|----------------|
| `None` | Null pointer (0x0) |
| `Some(ptr)` | Non-null pointer |

Size of `Option<Owned<T>>` = 8 bytes (same as `Owned<T>`).

## Edge Cases

| Situation | Behavior |
|-----------|----------|
| `own` in loop | Each iteration allocates; each must be consumed |
| `Owned<Owned<T>>` | Valid but unusual; double indirection |
| Zero-sized T | Valid; allocates minimal (may be optimized to no-op) |
| `Owned<[T; N]>` | Valid; heap-allocated array |

## Summary

`Owned<T>` provides safe heap allocation through linearity:

- **`own expr`** — Allocate, returns `Owned<T>`
- **Linear** — Must consume exactly once
- **Zero overhead** — No runtime checks
- **Primary use** — Recursive data structures
