<!-- id: analysis.fourth-option-guide -->
<!-- status: exploration -->
<!-- summary: Teaching guide for graphs and edges — how they work, how to use them, when to reach for them -->
<!-- depends: analysis/fourth-option.md -->

# Racks and Links: A Guide

How you'd write Rask if this lands. Written as a tutorial partly because
that's the honest design test — a model you can't teach in one page is a model
that's wrong.

## The one idea

**When something is deleted, everything pointing at it becomes `none`.**

That's it. Everything below follows.

## 1. A node is just a struct

Nothing special about the type. What makes it a node is where it lives.

<!-- test: skip -->
```rask
struct Task {
    title: string
    done: bool
}
```

## 2. A graph is where they live

<!-- test: skip -->
```rask
mut tasks: Rack<Task> = Graph.new()

let design = tasks.insert(Task { title: "design", done: false })
let build  = tasks.insert(Task { title: "build",  done: false })
```

`insert` hands back the node itself, borrowed for the current block — use it,
wire it up, but it can't be stored in a struct or returned. Same rule as every
other borrow in Rask.

## 3. An edge points at a node

Declare it in the struct, like any field.

<!-- test: skip -->
```rask
struct Task {
    title: string
    done: bool
    blocked_by: Link<Task>?        // this task waits on another one
}

let design = tasks.insert(Task { title: "design", done: false, blocked_by: none })
let build  = tasks.insert(Task { title: "build",  done: false, blocked_by: design })
```

Assign one later exactly as you'd expect:

<!-- test: skip -->
```rask
build.blocked_by = design
build.blocked_by = none        // clear it
```

## 4. Follow it with `?`

An edge is optional, so you read it with the optional operators you already
use. There is no other API to learn.

<!-- test: skip -->
```rask
if build.blocked_by? as b {
    println("waiting on {b.title}")
}

let name = build.blocked_by?.title ?? "nothing"
```

Once you're inside the `if`, `b` is a live task. No check, no unwrap, no
staleness — the branch you already wrote is the whole safety story.

## 5. Delete — this is the part that's different

<!-- test: skip -->
```rask
tasks.delete(design)

if build.blocked_by? as b {
    println("waiting on {b.title}")     // does not run
} else {
    println("unblocked")                  // runs
}
```

`build.blocked_by` is **`none`** — not dangling, not stale, not a panic
waiting to happen. The delete walked everything pointing at `design` and
cleared it. You wrote no cleanup code, and there is no cleanup code to
forget.

Compare what the same program needs today: after `pool.remove(h)`, every
holder of that handle is carrying a live grenade, and each reader has to
`pool.get(h)?` before touching it. Forgetting is a panic in production — that
is [#740](https://github.com/rask-lang/rask/issues/740), in the flagship.

## 6. Many references: an ordinary Vec

<!-- test: skip -->
```rask
struct Task {
    title: string
    done: bool
    deps: Vec<Link<Task>> = Vec.new()
}

build.deps.push(design)
build.deps.push(research)

for d in build.deps {
    if !d.done { return true }      // no staleness check — dead ones aren't here
}
```

Delete a dependency and it leaves the list. The list gets shorter. That's the
entire behaviour.

## 7. Inverses: two fields, one fact

"X's parent is Y" and "Y's children include X" are the same fact written from
both ends. Declare that, and the compiler maintains both sides:

<!-- test: skip -->
```rask
struct Task {
    title: string
    subtasks: Vec<Link<Task>> = Vec.new()

    @inverse(subtasks)
    parent: Link<Task>?
}

child.parent = epic         // child now appears in epic.subtasks, automatically
child.parent = other_epic   // leaves epic.subtasks, joins other_epic.subtasks
```

Two reasons to bother. It kills the classic bug — set one direction, forget
the other. And it's free: the `parent` field *is* the back-pointer the graph
needs, so a declared inverse costs no extra memory at all.

## 8. Indexes: a Map of edges

<!-- test: skip -->
```rask
struct Rack {
    tasks: Rack<Task>
    by_id: Map<TaskId, Link<Task>>
}

rack.by_id.insert(id, task)

if rack.by_id.get(id)? as t {
    println(t.title)
}
```

Delete the task and the map entry goes with it. No orphaned index rows.

## The ownership ladder

Three ways to hold data, each costing more than the last. Pick the cheapest
one that fits — that's the whole decision.

| Rung | Use when | Cost |
|---|---|---|
| **Plain field** — `Entity { body: Body }` | Owned by exactly one thing, fixed size, nothing else references it | Nothing. It's inline |
| **`Heap<T>`** — `Expr { left: Heap<Expr> }` | Owned by exactly one thing, but recursive or variable-sized so it needs the heap. Still nothing else references it | One pointer, one allocation. No bookkeeping |
| **`Rack<T>` + `Link<T>`** | Other things reference it, and it can be deleted out from under them | A pointer plus a back-pointer, and a fixup walk at delete |

The question that decides it: **does anything else point at this?** If not, you
never needed a graph — nothing can be left dangling by a delete, so there's
nothing to fix, and paying for back-pointers buys you nothing.

### `Heap<T>` keeps its job

Graphs don't replace it, and the reason is sharper than cost. **An `Heap<T>`
can leave.** It's a value: return it from a function, store it in a struct,
move it into a channel. A parser writes `func parse_expr() -> Heap<Expr>`
and hands the tree back to its caller. A graph node can't be returned at all —
edges live inside the graph's world.

So for an AST: `Heap<T>` wins on every axis. It's movable, it's roughly half
the memory (no back-pointer per child), the delete is free (drop the root,
the tree goes), and linearity already guarantees exactly-once consumption. A
graph would add bookkeeping for a question — "who else points at this
subexpression?" — that a syntax tree never asks.

Reach for a graph when that question has an answer other than "nobody."

### Why not have the compiler prove a graph is owned?

The obvious unification: one mechanism, and where the compiler can prove at
most one edge ever points at a node, drop the back-pointer and the fixup.
Three reasons that doesn't work, and they compound.

**The proof is whole-program.** "At most one edge points at any `Expr`" isn't
a property of the schema — `a.left = c` and `b.left = c` use one field and
produce two references. Establishing uniqueness means tracking every
assignment everywhere, which is exactly the analysis METRICS lists as an
auto-fail (`CS`: no whole-program analysis, compile time must stay linear).

**Silent elision would make struct size optimizer-dependent.** If the
back-pointer appears or vanishes based on what an analysis managed to prove,
then a node's memory layout changes when unrelated code in another module
changes. That breaks the predictability principle outright — you're supposed
to be able to read layout off the page.

**And what you'd have proven is linearity.** Follow the logic: if a node
genuinely has one owner, then deleting it happens *through* that owner, so
the owner already has the field in hand and no back-pointer was ever needed
to find it. That's not a graph with an optimization applied — that is
`drop(ptr)` on an `Heap<T>`, exactly. The proof obligation *is*
`Heap<T>`'s contract.

So the two aren't one mechanism and its special case; they're two contracts,
and the cost difference **is** the contract. `Heap<T>` says "exactly one
owner, consumed exactly once," gets checked at compile time, and is rewarded
with zero bookkeeping. A graph says "an unknown number of referrers," and
pays back-pointers to answer at runtime what linearity answers statically.

The same reasoning explains why Rask has `const` rather than "mutable that
the optimizer noticed was never written." Declaring the stronger fact buys
the guarantee. Inferring it silently buys unpredictability.

## When to use a graph

Reach for one when **things reference each other and can be deleted**. That
combination is the whole trigger.

| Your data | Use |
|---|---|
| Entities that target, own, or depend on each other | `Rack<T>` + `Link<T>?` |
| A list of values you iterate, no cross-references | `Vec<T>` — no graph |
| One thing that belongs to exactly one other and is never referenced elsewhere | a plain field: `Entity { body: Body }` |
| A tree you own and traverse, no back-pointers, single owner | `Heap<T>` |
| Something that must survive being written to a file or sent to another task | a domain id (`TaskId`), plus an index |
| Config, constants, anything never deleted | ordinary values |

Three concrete calls:

- **A game's entities** → graph. They target each other and die constantly.
- **A parsed AST** → `Heap<T>`. Single owner, no cross-references, never
  partially deleted.
- **Lines in a text buffer** → graph if lines reference each other (marks,
  folds, annotations); a plain `Vec` if they don't.

## When *not* to reach for one

- **You never delete.** No deletes, no dangling, no reason to pay for
  back-pointers. Use a `Vec`.
- **You rewire far more often than you read.** Each edge write costs several
  racks (it maintains the back-pointer); each read costs nothing. That trade
  is excellent when reads dominate — which is nearly always — and poor when
  they don't.
- **You need the reference to leave.** Links live inside the graph's world.
  Crossing to a file, a socket, or another task means a domain id.

## The mental model, ported

| If you know | An `Link<T>?` is |
|---|---|
| SQL | a foreign key with `ON DELETE SET NULL` |
| Java, C#, Python | a reference that goes null on delete instead of keeping the object alive |
| Go | a pointer that becomes nil when the object is deleted, with no GC |
| Rust | a `Weak<T>` that's already upgraded — dangling can't happen, so there's nothing to check |
| C++ / Qt | `QPointer`, for every type, enforced by the compiler |

One sentence: **the reference doesn't keep the thing alive, and it doesn't
dangle — it empties.**

## What you never write

Worth listing, because these are the habits the model retires:

- `if pool.get(h)? as x` before touching anything
- `else { thing.ref = none }` cleanup branches
- `active: bool` flags and the sweep that reads them
- `using Pool<T>` on every helper that touches a reference
- an O(n) pass to scrub deleted ids out of other objects' lists

## Where this is still unfinished

Being honest about the edges of the map: batches (for building mutual
references), the pinned scope (for collecting nodes into a local `Vec`), and
staged parallel iteration all exist in the design but aren't taught here
because they aren't settled enough to teach. Delete policies beyond
set-to-`none` are deliberately not shipping. See
[fourth-option.md](fourth-option.md) for the decision list.
