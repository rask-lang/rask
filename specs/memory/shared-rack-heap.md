<!-- id: mem.shared-rack-heap -->
<!-- status: decided -->
<!-- summary: The three types that hand out scoped access, and why the set is closed -->
<!-- depends: memory/borrowing.md, memory/ownership.md -->

# Shared, Rack and Heap

Three types keep a value somewhere other than the name that holds them, and give access through a scope — an inline expression, or a `with` block. You don't touch the inner value directly: you ask for access, do the work, and the scope ends.

One shape, one syntax, three access disciplines. No user-defined type gets a fourth, and the second half of this page is why.

## The set

| Type | Access discipline | Cross-task? | Use when |
|-----|-------------------|-------------|----------|
| [`Shared<T, S>`](../concurrency/sync.md) | Scoped `read()`/`write()`; `S` picks the synchronization | Yes, unless `S` is `Local` | One mutable value several names reach |
| [`Rack<T>`](racks.md) + `Link<T>` | Stored references; delete nulls every incoming edge | No (copy with `snapshot()`) | Graphs, scene trees, entity systems |
| [`Heap<T>`](heap.md) | Linear (single consumer) | Sendable | Recursive types, AST nodes |

`Atomic<T>` (see [`mem.atomics`](atomics.md)) sits adjacent: same carve-out, but its access is intrinsic operations rather than a scope.

`Cell<T>` and `Mutex<T>` are gone as types. They were `Shared<T>` with different synchronization, so they're strategies now: `Shared<T>` (a read-write lock, the default), `Shared<T, Mutex>` (a plain lock), `Shared<T, Local>` (no lock, one task). The familiar words survive; the choice of type doesn't.

## Two ways to reach the value

**Inline** — single expression, scope is the expression:

<!-- test: skip -->
```rask
shared.read().timeout          // Shared (expression-scoped read access)
shared.write().push(item)      // Shared (expression-scoped write access)
shared.get()                   // Shared (Copy types only)
node.health -= 10              // Link — a stored reference, no ceremony
```

**`with` block** — multi-statement, scope is the block:

<!-- test: skip -->
```rask
with shared.write() as c {
    c.timeout = 60.seconds
    c.retries = 5
}
with queue.write() as q {
    q.push(a)
    q.push(b)
}
with counter.write() as v { v.count += 1 }
```

`return`, `try`, `break`, and `continue` work through every `with` block (`mem.borrowing/W1`). This is why Rask uses `with` instead of closure-based access — control flow propagates naturally.

## Why scoped access, not guards

Rust-style guards (`MutexGuard`, `Ref`, `RefMut`) let a reference escape the acquisition site. These three don't — the inner value is reachable only inside the `with` block or inline expression. This falls out of "no storable references" and gives three properties:

- **No escaping references** — the view can't outlive the scope, by construction.
- **Explicit unlock timing** — lock released at block/expression end, visible in code.
- **Control flow works** — `return`/`try`/`break`/`continue` propagate naturally; closures can't do this.

## The set is closed

| Rule | Description |
|------|-------------|
| **BX1: Fixed set** | The set is `Shared` (with its `Local`/`Readers`/`Mutex` strategies), `Rack` + `Link` and `Heap`, plus adjacent `Atomic`. They're language constructs with type-shaped names, like `T or E` and `T?` — not library types |
| **BX2: No user-built equivalent** | No user-defined type gets these semantics: refcounted copy, shared interior, or `with`-scoped access. There is no annotation, interface, or generic parameter that grants them |
| **BX3: Compose instead** | Types that need sharing wrap one — `Shared<Map<K,V>>` for a cache, `Rack<T>` + `Link<T>` for a graph, `Shared<Vec<u8>>` for a refcounted buffer |
| **BX4: `unsafe` doesn't unlock it** | Raw pointers let you build any data structure you like (`mem.unsafe`). They don't let a type opt into running code on assignment, on scope exit, or at borrow boundaries — that's what these semantics require, and it isn't a pointer capability |

The `BX` prefix is left over from when this page had a collective noun for the three. The rules kept their numbers because issues and diagnostics cite them.

## Choosing

Ask in this order — the answers are sequential, not simultaneous (`analysis.storage-consolidation`):

1. **One value, held by exactly one owner?** → a plain field. Done.
2. **Many values?** → `Vec` or `Map`, unless…
3. **…other things reference them, and they can be deleted?** → `Rack<T>` + `Link<T>`.
4. **Several accessors share one mutable value?** → `Shared<T>`. Name a strategy only to change the default: `Mutex` when writes dominate, `Local` when it provably never leaves its task.

Two questions sit *outside* that list, which is why mixing them in made the set unchooseable:

- **Does it need to be on the heap** (recursive, or large and moved often)? → wrap it in `Heap<T>`. Independent of every answer above.
- **Is this a contended counter or flag you've measured?** → `Atomic<T>`. A concurrency primitive, not a storage choice.

Read as a rule: plain fields until you have many; `Vec`/`Map` until they reference each other; `Rack` when they do; and the concurrency strategies only when a second task exists. Nothing above step 3 is reached by an ordinary program.

Don't nest them without a reason. `Shared<Shared<T>>` and similar compositions usually mean the wrong one was picked first — and the old `Shared<Mutex<T>>` is a strategy now, not a nesting.

## Cross-cutting properties

| Property | `Shared<T>` | `Shared<T, Mutex>` | `Shared<T, Local>` | Rack + Link | Heap |
|----------|------|--------|-------|------|------|
| Copy | No (@unique) | No (@unique) | No (@unique) | No | No |
| Sendable cross-task | Yes | Yes | No (SH7) | By `snapshot()` | If `T: Send` |
| Blocking access | Yes (writers) | Yes | No | No | — |
| Linear (must consume) | No | No | No | If `T` is linear | Yes |
| Heap-allocated inner value | Yes | Yes | Yes | Yes | Yes |

All of them heap-allocate their contents. Scoped access is what makes reaching that storage safe without tracking lifetimes.

## See Also

- [Synchronization](../concurrency/sync.md) — `Shared<T, S>` and its strategies (`conc.sync`)
- [Racks and Links](racks.md) — Graph storage with delete-time edge fixup (`mem.racks`)
- [Heap Values](heap.md) — A value moved to the heap, consumed once (`mem.heap`)
- [Cell](cell.md) — Retired; folded into `Shared<T, Local>` (`mem.cell`)
- [Atomics](atomics.md) — Adjacent: intrinsic operations, not `with` (`mem.atomics`)
- [Ownership](ownership.md) — Why these hold heap data by value (`mem.ownership`)
- [Borrowing](borrowing.md) — `with` semantics and rules (`mem.borrowing`)
- [Linearity](linear.md) — Must-consume rules (`mem.linear`)

---

## Appendix (non-normative)

### Why one page?

Before this spec, Cell, Shared, Mutex, Pool and Owned each stood alone with their own "when to use what" tables duplicated across specs. Readers had to cross-reference five pages to build a mental model. Three of those five collapsed into one, which is what putting them side by side made visible; Pool went too, replaced by Rack + Link (rask-lang/rask#908).

They share one syntax and one decision. Collecting them turns five questions ("which type do I pick?") into one: `with` access is the common shape, so pick the access discipline that fits the problem. The individual specs still own their details.

### Why users can't build one (BX1–BX4)

The usual objection: if the stdlib needs magic its users don't get, the type system must be too weak. That reads the situation backwards. The privileged types don't use a hidden type-system feature — they have permission to run code at three moments Rask deliberately keeps free of user code. Handing that permission out is what would break, and no amount of type-system power changes it.

**1. Assignment stays a memcpy.** Copy is structural and bitwise (`mem.value/VS8`, `VS9`) — `let b = a` copies bytes and nothing else. A refcounted-copy type needs a hook there. Allow one and you have C++ copy constructors: assignment can allocate, lock, or panic, and you can no longer read cost off the page. `string`'s refcount bump doesn't break this because the compiler emits it, knows what it is, and deletes it when it's provably unnecessary (`comp.string-refcount-elision`). User code in that slot is opaque — never elidable, so a hand-built `Shared` would be permanently slower than the blessed one anyway.

**2. Scope exit stays free of user code.** A refcounted type needs a decrement-and-maybe-free on every exit path, unwind included. That's a destructor, and Rask doesn't have them — cleanup is `ensure` you can see, plus linearity (`ctrl.panic/U5`). Giving users a scope-exit hook to build `Shared` with reintroduces invisible cleanup language-wide to serve five types that already exist.

**3. Borrow regions stay compiler-owned.** These hand out no guards; the inner view can't outlive its scope because the compiler decides where the scope ends. A user-built equivalent would either return a storable reference (banned outright — principle 3) or need its own `with` protocol, i.e. user code opening and closing a borrow region the checker has to trust. Then aliasing safety is "trust the library author" instead of guaranteed by structure, and mechanical safety drops to advisory.

Same shape as `T or E`, `T?`, and `none` (`type.errors/ER1`, `type.optionals/OPT2`): built in, not user-definable, and nobody calls those a weakness. The set is small, closed, and documented — three types, a deprecated fourth, and the atomics. Predictability over abstraction power.

**What library authors actually do (BX3):**

| Want | Build it as |
|------|-------------|
| String interner | `Map<string, Link<T>>` — `string` is already refcounted, interning is deduplication, not new sharing |
| Arena with handout semantics | `Rack<T>` + `Link<T>` — this *is* the blessed pattern for many values with stable identity |
| Refcounted immutable buffer (zero-copy net) | `Shared<Vec<u8>>` for bytes; `string`/`StringView` for text |
| Shared cache | `Shared<Map<K,V>, Readers>` |

The real limit is narrow: you can't put your own type *into* the privileged set, so you compose with one instead of becoming one. That costs a wrapper and one `with` block. It buys the guarantee that every type in the language copies, drops, and borrows the same way.

### What if you need a sixth?

Then it becomes the sixth, in the compiler, and every program gets it. The set is closed, not frozen — it was assembled from types that already existed, and it can grow the same way. That's the inversion worth noticing: the privilege isn't withheld from users, it's the *delivery mechanism* for them. A new discipline ships as a language feature everyone can audit, instead of as an unsafe reimplementation buried in one library.

The bar is a design bar, not a popularity one. A new one needs a scoped access discipline the current ones don't cover — several-accessors-one-value, stored-reference-with-delete-fixup, linear-heap. Note which way the set moved when it was last examined: three names collapsed into one, because they were one discipline wearing three hats. "I want refcounting" doesn't qualify; refcounting is how `Shared` is implemented, not what it is. Nobody has named a sixth discipline yet, which is some evidence the set is close to complete.

Why not just add an `unsafe` hatch and let libraries do it? Because the two hatches cost different things. Raw pointers are contained — they don't change what `let b = a` means for anyone else. A hatch for these does: every reader of every dependency starts having to ask "is assignment free for this type, and does something run when it drops?" The carve-out costs you five names learned once. The hatch costs you an audit of everything you import. That's a bad trade for a language selling local reasoning.

Two costs I'll own. A library whose whole pitch is "feels like a plain value, shares underneath" can't be written in Rask — that's the design working, but it is a real thing you can't have. And composing means `Shared<T>` shows up in your public signatures, pushing callers into `with` blocks. Fine for a cache; annoying for a type you wanted to feel primitive.

`string` gets the same treatment for the same reason, argued separately in `std.strings` ("Why Only String?").

### `with` isn't only for these three

Vec, Map and arrays also take `with <source>[key] as binding`. The difference: these three wrap *one* value (one per link, for a rack), while a collection wraps a sequence or a mapping and has structural operations — push, remove, clear — that they don't have. What's shared is element access through a scope.

So `with` is the universal scoped-access syntax, and these three are the types whose whole purpose is to be reached through it.
