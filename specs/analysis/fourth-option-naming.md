<!-- id: analysis.fourth-option-naming -->
<!-- status: exploration -->
<!-- summary: Naming the two types — why Graph and Edge are both wrong, and what to call them instead -->
<!-- depends: analysis/fourth-option.md, stdlib/api-design.md -->

# Naming: Not `Graph`, Not `Edge`

Both working names describe the *topology a program might build*, not the
*job the type does*. That's backwards, and the api-design guess test
(`std.stdlib/SD*`) catches it.

## `Graph<T>` is wrong for most of its uses

Look at what actually gets declared:

<!-- test: skip -->
```rask
tasks: Graph<Task>          // a task store. Not a graph
entities: Graph<Entity>     // a game's entities. Not a graph
lines: Graph<Line>          // a text buffer. Not a graph
users: Graph<User>          // a user table. Not a graph
```

A user with a flat list of tasks has to declare a `Graph` and doesn't have a
graph. The word describes a shape their data *might* take, and usually
doesn't. That's the wrong end of the telescope: name the container for what
it is — a place where many instances of a type live with stable identity —
not for one possible arrangement of its contents.

**Recommendation: keep `Pool<T>`.**

The container's job hasn't actually changed. It's still "many things of one
type live here, individually addressable, individually removable." What
changed is *how you refer to its contents* — handles became something better.
Keeping the name means:

- The concept transfers intact; nobody relearns what a pool is.
- The change lands as "pools hand out links now, not handles" — one idea,
  not a new vocabulary.
- Migration is a find-and-replace on the reference type, not on every
  container declaration in every program.
- It doesn't lie to the 80% of users whose pool holds a flat collection.

The one argument for renaming — that the container now maintains referential
integrity, so it "knows about relationships" — doesn't require the name to
say so. `Vec` doesn't announce that it reallocates.

## `Edge<T>` only makes sense next to `Graph<T>`

Drop `Graph` and `Edge` loses its anchor: an edge is a graph-theory term, and
it's jargon for what is, plainly, a reference to something in a pool.

Options weighed:

| Name | For | Against |
|---|---|---|
| `Edge<T>` | precise if you think in graphs | jargon; meaningless without `Graph`; pairs badly with `Pool` |
| `Ref<T>` | it *is* the language's one storable reference | collides head-on with "Rask has no storable references" — the sentence stops being true and starts needing an asterisk |
| `Ptr<T>` | short | implies raw memory; `&raw` already owns that space |
| **`Link<T>`** | plain English, no jargon; the verb is already right — it links, and when the target dies it *unlinks* | slightly soft-sounding |

**Recommendation: `Link<T>`.**

<!-- test: skip -->
```rask
struct Task {
    title: string
    blocked_by: Link<Task>?
    deps: Vec<Link<Task>>
}

struct Store {
    tasks: Pool<Task>
    by_id: Map<TaskId, Link<Task>>
}
```

Read it aloud: "blocked_by is a link to a task, maybe." "deps is a vector of
links to tasks." Both land without a glossary, and the behaviour has a verb
that matches — the target dies, the link unlinks.

Against `Handle<Task>?` at 13 characters, `Link<Task>?` is 11 and carries
meaning instead of implying a ticket you must redeem.

## Is the `?` pulling its weight?

Yes. Required and optional links both exist (required ones are constructible
inside a batch), and the distinction is real at use sites: a required link
never needs unwrapping, an optional one always does. So `?` marks a genuine
difference rather than decorating every declaration.

And it keeps the read path unified with everything else optional in the
language — `link? as t`, `link?.title`, `link?.title ?? "none"` all work
because it *is* an optional, not because links got their own operators.

## The more radical option, noted and not taken

Declare node-ness on the type, then drop the wrapper entirely:

<!-- test: skip -->
```rask
node struct Task { ... }

struct Something {
    blocked_by: Task?        // unambiguous: Tasks live in pools, so this is a reference
    deps: Vec<Task>
}
```

Cleanest possible use sites, and it removes a generic wrapper from every
declaration. Two reasons not to take it now: it hides the cost (a field
that looks like a plain value carries a back-pointer), and it splits struct
declarations into two kinds, which is a much larger language change than
adding one type. Worth revisiting if `Link<T>` proves noisy in real schemas.

## The container: still iterating

`Link<T>` is settled. The container is not, and the reason surfaced from a
reader's first reaction to `Pool<T>` + `Link<T>`: *"it's a pool of links?"*

That's the real criterion. **The two names have to form a pair that teaches
the model.** `Graph`/`Edge` paired beautifully and one half was wrong.
`Pool`/`Link` has both halves defensible and no relationship between them —
"pool" says nothing about why the things inside can be linked to, so the
reader has to ask.

Working candidates, judged as pairs:

| Pair | Teaches | Against |
|---|---|---|
| `Pool<T>` + `Link<T>` | nothing — two unrelated words | familiar; zero migration on container declarations; but carries object-pool baggage (recycling, reuse) that was never what this is |
| **`Table<T>` + `Link<T>`** | the actual model — this *is* `ON DELETE SET NULL`, so the DB analogy is a teaching tool, not a metaphor | "table" means hash-map in Lua and rows-and-columns to everyone else; game devs may find `Table<Entity>` odd |
| `Registry<T>` + `Link<T>` | things register and get identity | verbose; `Registry<Entity>` is a mouthful in hot-path code |
| `Store<T>` + `Link<T>` | plain and honest | collides with real programs — the flagship's own struct is named `Store` |

**`Table<T>` is out.** It isn't a table — no rows, no columns, no schema in
the SQL sense, and "table" already means hash-map to a large audience. The
model *resembles* a database; the container isn't one.

### The cold-read test

Same three declarations under each candidate. A name has to survive all
three, because the container is general — it holds whatever a program has
many of.

| Candidate | `<Task>` | `<Entity>` | `<Line>` | Verdict |
|---|---|---|---|---|
| `Pool` | fine | fine | fine | survives everywhere, teaches nothing, carries object-pool (recycling) baggage |
| `Roster` | good — a roster of tasks | good | odd | members with identity that join and leave; the meaning is exactly right |
| `Colony` | odd | good | odd | real prior art (`plf::colony` is a stable-reference container), but reads whimsical |
| `Web` | ok | odd | odd | pairs perfectly with `Link` — but Rask has `net`/`http`, so "web" is loaded |
| `Ledger` | good | odd | odd | entries with identity; implies append-only accounting |
| `Nest`, `Cohort`, `Zone` | odd | ok | odd | no meaning carried; just unfamiliar |

Nothing survives all three cleanly except `Pool`, which is the finding.

### `Pool` is out too — it means the opposite of what this is

The case for `Pool` rested on familiarity and migration cost. Both are worth
nothing: nobody uses Rask yet, and the entire corpus is this repo's own
examples. With that crutch gone, the remaining connotation is fatal.

**A pool is a supply of interchangeable things.** Thread pool, connection
pool, object pool — you ask for *a* connection, not *that* connection, and
the whole point is that you don't care which one you get. Identity is exactly
what a pool doesn't have.

This container is the opposite: every instance has a stable identity, is
referenced individually, and is deleted individually. A newcomer's only prior
is from other languages, where "pool" teaches interchangeability — so the
name doesn't fail to teach, it teaches something false. That's worse than
neutral, and it's a bug the current design already inherited.

### What that suggests

Every candidate that *teaches* does so by analogy to one domain, then reads
wrong in the others — that's what happens when a general container gets a
specific name, and it's the same objection that killed `Graph`, `Table` and
the rest. So: **only the reference has to teach.** `Link<T>` carries the
lesson. The container needs to say "many of these live here, each one its own
thing" — accurately, in any domain — and then get out of the way.

That rules out the evocative names (overfit), `Pool` (teaches the opposite),
and leaves the plain ones. Cold-read again, on the survivors:

| Candidate | `<Task>` | `<Entity>` | `<Line>` | Notes |
|---|---|---|---|---|
| **`Store`** | good | good | good | plain, accurate, reads naturally everywhere. Common in user code — but so is `Map`; shadowing is a general issue, not a reason to pick a worse name |
| `Registry` | good | good | good | the most precise word — things register, get identity, deregister — but long in hot-path declarations |
| `Roll` | ok | ok | odd | short and correct (a roll of members) — collides with rotate/dice |
| `Bank` | ok | ok | ok | neutral, faintly financial |

**Recommendation: `Store<T>` + `Link<T>`.**

<!-- test: skip -->
```rask
struct World {
    entities: Store<Entity>
    player: Link<Entity>?
}

struct Tracker {
    tasks: Store<Task>
    by_id: Map<TaskId, Link<Task>>
}
```

It says what it is, in every domain, and claims nothing false. `Registry<T>`
is the runner-up and the more precise word if the extra five characters don't
grate.

### The wrapper-free option is dead — but not for the cost reason

With no installed base, "too big a change" lost its force, and cost-hiding
alone wouldn't have killed it: Rask tolerates small opaque costs (`Vec` hides
its allocation, bounds checks are implicit). A back-pointer is in that tier.

The objection that actually kills it is different. Under `node struct`:

<!-- test: skip -->
```rask
struct Project {
    lead: Task?              // is this a Task value, or a link to one?
    deps: Vec<Task>          // Tasks stored here, or links to Tasks?
}
```

You cannot tell by reading. The answer depends on whether `Task` was declared
`node struct` or plain `struct` — a fact that lives in another file. And it
isn't a cost question, it's a *semantics* question: does assigning this copy
the data or point at it? Does someone else's delete change it? Two completely
different behaviours, spelled identically.

That's non-local reasoning, which is the thing Rask is built to avoid. So the
wrapper stays, and its job is now stated precisely:

**`Link<T>` marks kind, not cost.** It's there so a reader knows, without
leaving the file, whether a field holds a value or points at one. The
back-pointer it implies is an ordinary small opaque cost, and doesn't need
to be visible at all.

## Recommendation so far

`Store<T>` and `Link<T>?`.

- `Link<T>` — decided. The wrapper is non-negotiable: it tells a reader
  whether a field holds a value or points at one, without leaving the file.
- `Store<T>` — the container, with `Registry<T>` the runner-up. `Pool` is
  rejected outright: it names a supply of interchangeable things, the
  opposite of a container built on stable identity.
- `node struct` (wrapper-free) — rejected. Makes value-versus-link a
  non-local fact.

Everything else in the exploration is unaffected — this is spelling, and the
semantics were settled first on purpose.

---

## Naming `Owned<T>`

Separate question, surfaced by reclassifying it: `Owned` answers "stack or
heap?", so does its name say that?

### The problem with `Owned`

**It names a property every value in Rask already has.** Single ownership is
the language default (`mem.ownership`) — every value has exactly one owner,
always. So `Owned<T>` distinguishes nothing by its name, and worse, implies
that values *not* wrapped in it are somehow unowned.

What's actually distinctive is the indirection: this value lives on the heap
rather than inline. That's also where the cost is — an allocation — and
Rask's transparency principle says major costs belong in the source.

`Box<T>` is rejected for two reasons: it's familiar rather than better (a
"box" says nothing about heap allocation), and `mem.boxes` already uses "box"
for the whole family — `Cell`, `Store`, `Shared`, `Owned`. Naming one member
after the category is worse than the status quo.

### `Heap<T>`

<!-- test: skip -->
```rask
enum Expr {
    Binary(left: Heap<Expr>, op: BinaryOp, right: Heap<Expr>)
    Unary(op: UnaryOp, expr: Heap<Expr>)
}

return Expr.Binary(left: heap base, op: BinaryOp.Pow, right: heap exp)
```

"left is a heap `Expr`" — it says exactly where the value lives, which is the
one thing that differs from a plain field, and the allocation is legible in
the type. Nothing metaphorical, nothing borrowed from another language.

**It also fixes the `own` collision.** Today `own` means two unrelated things:

<!-- test: skip -->
```rask
Expr.Binary(left: own base, …)     // heap-allocate
spawn(own || { … })                  // move-capture a closure
parse_args(own args)                 // move an argument
```

With `heap expr` as the allocation operator, `own` means move-capture and
nothing else. Two words, two jobs, instead of one word doing both.

### The one collision to accept

"Heap" is also a data structure — a binary heap, i.e. a priority queue. A
future `Heap<T>` collection would clash.

Resolved deliberately: a priority queue in Rask should be named for its
purpose, not its internal shape. `PriorityQueue<T>` says what it does;
`Heap<T>` is jargon naming an implementation detail, exactly the kind of
lifted-from-`std` naming `std.stdlib/SD*` warns against. The name is free
because the collection that wanted it shouldn't have it.

### Stating the thing exactly

Before picking, say what it is with no name attached.

**What it is:** a value stored separately from its container, reached through
exactly one pointer, owned by exactly one holder, consumed exactly once.

**Why anyone reaches for it:** two reasons, and only two.
1. *A type can't contain itself.* `struct Expr { left: Expr }` has no finite
   size. Something of fixed size has to stand in for the child. This is the
   dominant case — every AST, tree and list node.
2. *A large value should travel as a pointer.* Moving 8 bytes instead of 200.

**Where it appears:** in type declarations, at recursive or large-value
positions. Almost never in a signature, almost never in a local.

**What the programmer means when they write it:** *"this field is not stored
inside me."*

So the essential semantic is **not-inline**. Single ownership is Rask's
default and adds nothing. Consumed-exactly-once is how it's kept safe, not
what it is. Everything else — malloc, arena, pool — is where the allocator
chose to put it.

### Which leaves two honest candidates

| | Names | Cost |
|---|---|---|
| `Heap<T>` | where the value goes | "heap" is allocator terminology — an arena allocation isn't obviously "the heap" |
| `Indirect<T>` | the mechanism, exactly | clinical, and no natural operator (`indirect base` is unusable) |

**The tiebreak is a question about Rask, not about English:** is stack-versus-heap
part of the language's semantic model, or an implementation detail below it?

Rask already answered. Principle 1 lists **allocations** among the major costs
that must be visible in source. A language that has decided allocation is
semantic can name a type after it without naming an implementation detail —
`Vec` hides its allocation and that's a deliberate small opacity; this type
exists *precisely to make one allocation happen*, so saying so is the honest
move.

Under a custom allocator the value still goes to dynamically-allocated
storage rather than inline — which is what "heap" means in ordinary systems
usage, arena or not. The objection is real but narrow.

### A wider sweep

Tested where the type actually lives — a recursive declaration and its
constructor — since that's ~100% of its real usage:

<!-- test: skip -->
```rask
Binary(left: X<Expr>, op: BinaryOp, right: X<Expr>)
return Expr.Binary(left: <ctor> base, op: BinaryOp.Pow, right: <ctor> exp)
```

| Candidate | Declaration | Construction | Read |
|---|---|---|---|
| `Heap<Expr>` | good | `heap base` | "a heap Expr" — says where it goes |
| `Alloc<Expr>` | ok | `alloc base` | allocator-neutral and names the exact cost; reads as an abbreviation |
| `Indirect<Expr>` | good | `Indirect(base)` | most precise; longest |
| `Place<Expr>` | good | `place base` | "a place holding an Expr" — allocator-neutral, plain English |
| `Apart<Expr>` | ok | weak | "an apart Expr" — correct meaning, awkward grammar |
| `Hold<Expr>` | ok | `hold base` | a ship's hold; but "hold" reads as locking |
| `Solo` / `Sole` / `Only` | ok | — | name single ownership, which every Rask value has — same flaw as `Owned` |
| `Stow<Expr>` | odd | `stow base` | good verb, unusual noun |
| `Slot`, `Bay`, `Berth` | — | — | imply a position in something, which this isn't |
| `Deep`, `Nested` | — | — | `Nest` implies containment — the opposite of indirection |
| `Point<Expr>` | odd | `point base` | honest (it is one pointer) but collides with geometry |

### The operator constraint can be removed

`Indirect<T>` was penalised for having no usable prefix operator. But nothing
requires one — construction can be an ordinary call:

<!-- test: skip -->
```rask
return Expr.Binary(left: Indirect(base), op: BinaryOp.Pow, right: Indirect(exp))
return Expr.Binary(left: Heap(base), op: BinaryOp.Pow, right: Heap(exp))
```

That costs a few characters against `heap base`, and buys back the most
semantically precise candidate. It also drops a keyword from the language,
which is worth something on its own — and `own` is freed for move-capture
either way.

### The grammar filter

`Alloc<i64>` reads as *"allocate an i64"* — a verb stem in a noun position.
`let x: Alloc<i64>` says x is an allocate-i64, which isn't a thing. Types name
things, so a candidate has to be a **noun** (naming what it is) or an
**adjective** (modifying the wrapped type). Applying that:

| Candidate | Part of speech | Reads as | |
|---|---|---|---|
| `Heap<Expr>` | noun (a place) | "a heap Expr" — an Expr on the heap | ✓ |
| `Separate<Expr>` | adjective | "a separate Expr" | ✓ |
| `Indirect<Expr>` | adjective | "an indirect Expr" | ✓ |
| `Alloc<Expr>` | **verb stem** | "allocate an Expr" | ✗ |
| `Apart<Expr>` | adverb | "an apart Expr" | ✗ |
| `Place<Expr>` | noun *or* verb | ambiguous — reads both ways | ~ |

`Alloc` is out on grammar, and the noun form that would fix it —
`Allocation<Expr>` — is too long for a type appearing in every recursive
field. There's no short noun for "an allocated thing" that isn't already
taken (`Cell`, `Block`, `Slot`, `Box` all mean something else here).

That leaves three, and the filter has done real work: it removed the
allocator-neutral candidate that was competing with `Heap` on substance.

### Three worth choosing between

- **`Heap<T>`** — noun, shortest, instantly read by any systems programmer,
  and Rask has already decided allocation is semantic (principle 1). Quibble:
  arena allocations aren't colloquially "the heap".
- **`Separate<T>`** — adjective, allocator-neutral, and states the actual
  semantic (not stored inline). Cost: 8 characters, and it describes the
  layout without hinting that an allocation happens.
- **`Indirect<T>`** — adjective, most precise about the mechanism. Cost:
  longest, and clinical.

### Recommendation

`Heap<T>`, with `Heap(expr)` construction rather than a keyword — which
removes the operator argument that was propping it up, so it wins on its own
merits or not at all. It's the only candidate that is both a noun and short,
and Rask's own principle 1 makes allocation a semantic fact rather than an
implementation one.

If allocator-neutrality is decisive, **`Separate<T>`** is the fallback — it
passes the grammar filter, states the semantic exactly, and costs four
characters over `Heap`. `Alloc<T>` was the better neutral candidate until the
grammar test removed it, and no short noun form survives.

What is settled: **`Owned` should go.** It names single ownership, which every
value in Rask has, so it distinguishes nothing and implies other values are
unowned. Whatever replaces it should name the indirection or its cost.
