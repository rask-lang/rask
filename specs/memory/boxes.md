<!-- id: mem.boxes -->
<!-- status: decided -->
<!-- summary: Container types that give scoped access through `with` — one shape, several access disciplines -->
<!-- depends: memory/borrowing.md, memory/ownership.md -->

# The Box Family

A **box** wraps a value and gives access through a scoped construct — inline expression access, or a `with` block. You don't touch the inner value directly; you ask for scoped access, do your work, and the scope ends.

One shape, one syntax, several access disciplines.

## The family

| Box | Access discipline | Cross-task? | Use when |
|-----|-------------------|-------------|----------|
| [`Cell<T>`](cell.md) | Exclusive, single-task | No | One mutable value shared across closures |
| [`Pool<T>`](pools.md) + `Handle<T>` | Identity-based (generation-checked) | Sendable | Collections, graphs, entity systems |
| [`Shared<T>`](../concurrency/sync.md) | Read-heavy (many readers XOR one writer) | Yes | Config, feature flags |
| [`Mutex<T>`](../concurrency/sync.md) | Exclusive lock | Yes | Queues, state machines |
| [`Owned<T>`](owned.md) | Linear (single consumer) | Sendable | Recursive types, AST nodes |

`Atomic*<T>` (see [`mem.atomics`](atomics.md)) is adjacent but not a box — its access is intrinsic operations, not `with`.

## The shared shape

All boxes support two access patterns.

**Inline** — single expression, scope is the expression:

<!-- test: skip -->
```rask
pool[h].health -= 10           // Pool
shared.read().timeout          // Shared (expression-scoped read lock)
mutex.lock().push(item)        // Mutex (expression-scoped lock)
cell.get()                     // Cell (Copy types only)
```

**`with` block** — multi-statement, scope is the block:

<!-- test: skip -->
```rask
with pool[h] as entity {
    entity.update()
    entity.mark_dirty()
}
with shared.write() as c {
    c.timeout = 60.seconds
    c.retries = 5
}
with mutex as q {
    q.push(a)
    q.push(b)
}
with cell as v { v.count += 1 }
```

`return`, `try`, `break`, and `continue` work through every `with` block (`mem.borrowing/W1`). This is why Rask uses `with` instead of closure-based access — control flow propagates naturally.

## Why scoped access, not guards

Rust-style guards (`MutexGuard`, `Ref`, `RefMut`) let a reference escape the acquisition site. Rask's boxes don't — the inner value is reachable only inside the `with` block or inline expression. This falls out of "no storable references" and gives three properties:

- **No escaping references** — the view can't outlive the scope, by construction.
- **Explicit unlock timing** — lock released at block/expression end, visible in code.
- **Control flow works** — `return`/`try`/`break`/`continue` propagate naturally; closures can't do this.

## The family is closed

| Rule | Description |
|------|-------------|
| **BX1: Fixed set** | The box family is `Cell`, `Pool`, `Shared`, `Mutex`, `Owned`, plus adjacent `Atomic*`. Boxes are language constructs with type-shaped names, like `T or E` and `T?` — not library types |
| **BX2: No user boxes** | No user-defined type gets box semantics: refcounted copy, shared interior, or `with`-scoped access. There is no annotation, trait, or generic parameter that grants them |
| **BX3: Compose instead** | Types that need sharing wrap a box — `Shared<Map<K,V>>` for a cache, `Pool<T>` + `Handle<T>` for an arena, `Shared<Vec<u8>>` for a refcounted buffer |
| **BX4: `unsafe` doesn't unlock it** | Raw pointers let you build any data structure you like (`mem.unsafe`). They don't let a type opt into running code on assignment, on scope exit, or at borrow boundaries — that's what box semantics require, and it isn't a pointer capability |

## Choosing a box

Quick decision path:

1. **Cross-task shared state?** → `Shared<T>` (read-heavy), `Mutex<T>` (write-heavy), or a channel (transfer ownership).
2. **Many values with stable identity, graphs, or ECS?** → `Pool<T>` + `Handle<T>`.
3. **Recursive type (tree, AST, linked list)?** → `Owned<T>`.
4. **One mutable value shared across closures, single-task?** → `Cell<T>`.
5. **Single counter, flag, or pointer-sized value?** → Atomic (not a box — lock-free primitive).

Don't nest boxes without a reason. `Cell<Cell<T>>`, `Shared<Mutex<T>>`, and similar compositions usually indicate the wrong box was chosen first. If you need cross-task mutation, reach for `Mutex<T>` directly — don't wrap a `Cell<T>`.

## Cross-cutting properties

| Property | Cell | Pool | Shared | Mutex | Owned |
|----------|------|------|--------|-------|-------|
| Copy | No (@unique) | No | No | No | No |
| Sendable cross-task | No | If `T: Send` | Yes | Yes | If `T: Send` |
| Blocking access | — | — | Yes (writers) | Yes | — |
| Linear (must consume) | No | If `T` is linear | No | No | Yes |
| Heap-allocated inner value | Yes | Yes | Yes | Yes | Yes |

Every box heap-allocates its contents — that's part of being a box. The `with`-scoped access is what keeps the indirection safe without tracking lifetimes.

## See Also

- [Cell](cell.md) — Single-value mutable box (`mem.cell`)
- [Pools](pools.md) — Handle-based identity box (`mem.pools`)
- [Owned Pointers](owned.md) — Linear heap box (`mem.owned`)
- [Synchronization](../concurrency/sync.md) — `Shared<T>` and `Mutex<T>` (`conc.sync`)
- [Atomics](atomics.md) — Adjacent family: intrinsic operations, not `with` (`mem.atomics`)
- [Ownership](ownership.md) — Why boxes hold heap data by value (`mem.ownership`)
- [Borrowing](borrowing.md) — `with` semantics and rules (`mem.borrowing`)
- [Linearity](linear.md) — Must-consume rules (`mem.linear`)

---

## Appendix (non-normative)

### Why name the family?

Before this spec, Cell, Shared, Mutex, Pool, and Owned each stood alone with their own "when to use what" tables duplicated across specs. Readers had to cross-reference five pages to build a mental model.

They're one family with one syntax. Naming it collapses five decisions ("which type do I pick?") into one (`with` access is the common shape; pick the access discipline that fits your problem). The individual specs still own their details — this page just makes the family visible.

### Why users can't build a box (BX1–BX4)

The usual objection: if the stdlib needs magic its users don't get, the type system must be too weak. That reads the situation backwards. The privileged types don't use a hidden type-system feature — they have permission to run code at three moments Rask deliberately keeps free of user code. Handing that permission out is what would break, and no amount of type-system power changes it.

**1. Assignment stays a memcpy.** Copy is structural and bitwise (`mem.value/VS8`, `VS9`) — `const b = a` copies bytes and nothing else. A refcounted-copy type needs a hook there. Allow one and you have C++ copy constructors: assignment can allocate, lock, or panic, and you can no longer read cost off the page. `string`'s refcount bump doesn't break this because the compiler emits it, knows what it is, and deletes it when it's provably unnecessary (`comp.string-refcount-elision`). User code in that slot is opaque — never elidable, so a hand-built `Shared` would be permanently slower than the blessed one anyway.

**2. Scope exit stays free of user code.** A refcounted box needs a decrement-and-maybe-free on every exit path, unwind included. That's a destructor, and Rask doesn't have them — cleanup is `ensure` you can see, plus linearity (`ctrl.panic/U5`). Giving users a scope-exit hook to build `Shared` with reintroduces invisible cleanup language-wide to serve five types that already exist.

**3. Borrow regions stay compiler-owned.** Boxes hand out no guards; the inner view can't outlive its scope because the compiler decides where the scope ends. A user-built box would either return a storable reference (banned outright — principle 3) or need its own `with` protocol, i.e. user code opening and closing a borrow region the checker has to trust. Then aliasing safety is "trust the library author" instead of guaranteed by structure, and mechanical safety drops to advisory.

Same shape as `T or E`, `T?`, and `none` (`type.errors/ER1`, `type.optionals/OPT2`): built in, not user-definable, and nobody calls those a weakness. The set is small, closed, and documented — five boxes and the atomics. Predictability over abstraction power.

**What library authors actually do (BX3):**

| Want | Build it as |
|------|-------------|
| String interner | `Map<string, Handle<T>>` — `string` is already refcounted, interning is deduplication, not new sharing |
| Arena with handout semantics | `Pool<T>` + `Handle<T>` — this *is* the blessed pattern for many values with stable identity |
| Refcounted immutable buffer (zero-copy net) | `Shared<Vec<u8>>`, or `string` for text |
| Shared cache | `Shared<Map<K,V>>` |

The real limit is narrow: you can't put your own type *into* the privileged set, so you compose with a box instead of becoming one. That costs a wrapper and one `with` block. It buys the guarantee that every type in the language copies, drops, and borrows the same way.

### What if you need a sixth box?

Then it becomes box six, in the compiler, and every program gets it. The set is closed, not frozen — the family was assembled from types that already existed, and it can grow the same way. That's the inversion worth noticing: the privilege isn't withheld from users, it's the *delivery mechanism* for them. A sixth discipline ships as a language feature everyone can audit, instead of as an unsafe reimplementation buried in one library.

The bar is a design bar, not a popularity one. Box six needs a scoped access discipline the five don't cover — exclusive-single-task, identity, read-heavy, exclusive-lock, linear. "I want refcounting" doesn't qualify; refcounting is how `Shared` is implemented, not what it is. Nobody has named a sixth discipline yet, which is some evidence the set is close to complete.

Why not just add an `unsafe` hatch and let libraries do it? Because the two hatches cost different things. Raw pointers are contained — they don't change what `const b = a` means for anyone else. A box hatch does: every reader of every dependency starts having to ask "is assignment free for this type, and does something run when it drops?" The carve-out costs you five names learned once. The hatch costs you an audit of everything you import. That's a bad trade for a language selling local reasoning.

Two costs I'll own. A library whose whole pitch is "feels like a plain value, shares underneath" can't be written in Rask — that's the design working, but it is a real thing you can't have. And composing means `Shared<T>` shows up in your public signatures, pushing callers into `with` blocks. Fine for a cache; annoying for a type you wanted to feel primitive.

`string` gets the same treatment for the same reason, argued separately in `std.strings` ("Why Only String?").

### Is every type with `with` access a box?

Not quite. Vec, Map, and arrays also work with `with <source>[key] as binding` — but they're collections, not boxes. The distinction: a box wraps *one* value (or one value per handle, for Pool). Vec/Map wrap a sequence/mapping and have structural operations (push, remove, clear) that boxes don't. The shared piece is the `with`-based element access.

Think of it this way: `with` is the universal scoped-access syntax; boxes are the types whose primary purpose is to *be* accessed through it.
