<!-- id: analysis.storage-consolidation -->
<!-- status: accepted -->
<!-- summary: Can the storage types merge? One real merge, two false ones, and a decision procedure for what's left -->
<!-- depends: memory/boxes.md, analysis/fourth-option.md -->

# Can the Storage Types Consolidate?

> **Accepted.** The recommendation at the bottom of this page is now the design.
> `Cell` and `Mutex` are strategies on `Shared<T, S>` (`conc.sync`), `Owned<T>`
> is `Heap<T>` (`mem.heap`), `Atomic<T>` keeps its own type and leaves the
> storage decision. This page is kept as the argument, not as an open question.

Prompted by the sharpest usability signal available: the language's own
designer can't reliably pick between them. If choosing is hard for the person
who designed the set, it's not a documentation problem.

## Why choosing is hard

The types don't differ along one axis. They differ along three, and a
programmer has to answer all three at once, without being told those are the
questions:

| | How many? | Who can reach it? | Deletable while referenced? |
|---|---|---|---|
| plain field | one | this value | — |
| `Vec` / `Map` | many | this scope | no |
| `Rack` + `Link` | many | this scope | **yes** |
| `Heap<T>` | one | movable, exclusive | no |
| `Cell<T>` | one | closures, one task | no |
| `Mutex<T>` | any | many tasks | no |
| `Shared<T>` | any | many tasks | no |
| `Atomic<T>` | one word | many tasks | no |

Two rows are identical in that table — `Mutex` and `Shared` differ in nothing
but lock strategy. That's the first finding.

## What *is* the difference between `Mutex` and `Shared`?

Only this: **`Shared` lets many readers in at once; `Mutex` lets exactly one
holder in, reader or writer.** `Shared` is a read-write lock, `Mutex` is a
plain lock.

Concretely, with eight tasks reading a config: under `Shared` all eight
proceed simultaneously; under `Mutex` they queue one at a time. In exchange,
`Shared` tracks a reader count, so a write-mostly workload pays bookkeeping
a plain lock doesn't have.

That's the entire distinction. It is a benchmarking question — *do concurrent
readers matter more than write overhead here?* — and it currently has to be
answered by choosing a type, at declaration time, before the program exists.

**The fact that this question needed asking is the finding.** When the
language's designer has to ask what separates two of its types, no user will
choose correctly, and the ones who guess right will have guessed. This moves
the merge below from "recommended" to "indicated."

## The first merge: `Shared` + `Mutex`

They are the same concept: a box holding one value, reached by several
accessors, serialized by a lock. The split is a *performance* distinction —
many-readers versus one-at-a-time — and asking a user to pick a **type** from
their expected read/write ratio is asking for a benchmarking decision at
declaration time, before the program exists.

Merge into one, with the discipline in the methods:

<!-- test: skip -->
```rask
let t = config.read().timeout          // shared access
with config.write() as c {             // exclusive access
    c.retries = 5
}
```

`Mutex<T>` disappears as a type and survives as a strategy name. The question
"which lock type?" stops existing.

(This turns out to be the *first* merge, not the only one — `Cell` folds in
the same way, worked out below. Three types become one.)

## Two merges that look right and aren't

**`Cell` into the lock family — reversed later in this document.** The
original objection was that `func bump(c: Cell<i32>)` would need a sharing
parameter carried by every signature. That objection doesn't survive once the
strategy parameter exists for locks anyway; see
[what `Cell` really is](#what-owned-and-cell-really-are). Left here because
the reasoning that changed is worth seeing.

**`Rack` into `Vec`.** Also tempting: a `Rack` with no links declared is
nearly a `Vec`. I argued the guarantees were opposite — `Vec` gives contiguity
and moves its elements on `push`; `Rack` gives stable addresses and never moves
them — so merging had to break one of them.

**That named the wrong pair.** A slab is contiguous *and* stable: a flat array
of slots, `None` for a hole, a free list, elements that never move. It's what
`Rack` is already built on (`slots: Vec<Option<T>>` + `free_list` + `slot_of`).
What a slab actually gives up is **density and order**: iteration walks the
high-water mark rather than the live count, and slot reuse means insertion order
doesn't survive churn.

That's a much weaker objection, and the first half largely self-heals — freed
slots are reused, so 1000 live nodes still sit in 1000 slots after 500 deletes
and 500 inserts. Holes persist only if you delete and stop.

So the merge is more available than this section claimed. The remaining reason to
keep two types is `Vec`'s *order*, which a program indexing by position depends
on and a slab cannot promise. That's a real difference; "contiguity versus
stability" was not. See [the slab section](fourth-option.md#the-rack-is-a-slab)
for what exposing the slab shape would buy.

`Heap` into `Rack` fails on cost, argued in
[the guide](fourth-option-guide.md): a rack node carries a back-pointer per
reference, an AST doesn't need one, and a `Heap<T>` can be returned from a
function where a node can't.

## `Atomic` isn't a storage type at all

It's in the list by accident of adjacency, and taking it out removes a chunk
of the confusion.

`Atomic<T>` is a lock-free operation on a single word — `fetch_add`,
`compare_and_swap`. Its reason to exist is a performance cliff, not a
storage decision: a counter bumped by many tasks costs one instruction as an
atomic and a full lock acquisition under `Shared`, which is one to two orders
of magnitude apart on contended paths. That gap is too big to hide, and too
big to leave to an optimizer that might or might not recognise
`with counter.write() as v { v += 1 }` as an atomic increment — a "did I get
the fast path?" you can't read off the page.

So it stays, but it belongs with SIMD and inline assembly: **a primitive you
reach for after measuring**, not an answer to "where do I put my data?" It
should be absent from the storage decision entirely, and documented in the
concurrency section as an optimization.

That reclassification is worth as much as a merge — the set feels large partly
because a performance primitive was being counted as a storage choice.

## What's left, and how to choose

Six, after the lock merge — and the choice is mechanical if the questions are
asked in the right order:

1. **Is it one value, held by exactly one owner?** → a plain field. Done.
2. **Many values?** → `Vec` or `Map`, unless…
3. **…other things reference them, and they can be deleted?** → `Rack` +
   `Link`.
4. **Do several accessors share one mutable value?** → `Shared<T>`, plus a
   strategy if it crosses tasks (`Mutex` / `Readers`).

Two orthogonal questions sit *outside* this list, which is why mixing them in
made it unchooseable:

- **Does it need to be on the heap** (recursive, or large and moved often)?
  → wrap it in `Heap<T>`. Independent of every answer above.
- **Is this a contended counter or flag you've measured?** → `Atomic<T>`.
  A concurrency primitive, not a storage choice.

Read as a rule: **plain fields until you have many; `Vec`/`Map` until they
reference each other; `Rack` when they do; and the concurrency types only
when a second task exists.** Nothing above step 3 is reached by an ordinary
program.

The reason this reads better than the table is that the axes are now
*sequential*. The current set forces a simultaneous three-way judgement,
which is exactly the thing that's hard, and no amount of naming fixes it.

## Performance as an opt-in variant, not a second type

The general form of the fix, and Rask already uses it: `Pool.new()` versus
`Pool.with_capacity(n)` is one type with two performance contracts, chosen at
the constructor. Nothing new is being invented — the lock split just never
got the same treatment.

**Rule: the type says what it is; the constructor says how it's built.**

<!-- test: skip -->
```rask
let counter = Shared.new(0)           // default: task-local, no lock at all
let config  = Shared.readers(cfg)     // opt-in: many tasks, concurrent reads
let queue   = Shared.mutex(q)         // opt-in: many tasks, one at a time
```

Everyone writes `Shared.new` and never learns the others exist until they send
one across a task boundary — at which point a compile error names them. The
distinction stops being a fork in the road every user must take and becomes a
thing the compiler points at when it becomes relevant.

(An earlier draft added `Shared.atomic` here too. That's since been rejected —
see below: atomics need an API that doesn't look like locking, so they keep
their own type.)

**Why the type stays uniform.** The strategy is a defaulted type parameter —
`Shared<T, S = Local>` — resolved at monomorphization, exactly like the
allocator parameter (`mem.alloc/AL4`). So it's zero-cost, and one type covers
what `Mutex<T>` and `Shared<T>` need two of today: an API no longer has to pick
a lock type that its callers must then match.

### One rule for what bare `Shared<T>` means

**`Shared<T>` means `Shared<T, Local>`, in every position.** A `let`, a
parameter, a return type, a field: the same type expression means the same
thing everywhere. A function that works with any strategy says so:

<!-- test: skip -->
```rask
func serve(c: Shared<Config>)                    // Local only
func serve<S>(c: Shared<Config, S>)              // any strategy
```

Earlier drafts of this document had it both ways — bare-in-parameter accepting
every variant while bare-in-`let` defaulted to `Local` — which would make one
type expression mean two things depending on where it sits. That is the
elision-shaped context-dependence this language removes everywhere else, so it
loses to the version that costs ceremony: a strategy-agnostic API writes its
type parameter.

The ceremony is real and worth naming. Every function that genuinely doesn't
care gains `<S>`, and library code that wants to accept all three has to say so
rather than getting it by default. The alternative — bare means "any" in
signatures — buys that back and pays for it with a positional rule, and a
positional rule is the more expensive of the two.

This is *not* the objection that killed folding `Cell` into the lock family.
There, the parameter carries **semantics** — sendable across tasks or not,
which changes what the code means, and is exactly why the default has to be the
conservative one. Between `Readers` and `Mutex` the choice is only speed, and a
caller can be suboptimal but never wrong.

**Which way the default should lean.** Conservative. A reader of
`Shared.new(x)` should never discover it was secretly doing something
cleverer than it says; discovering "I could have been faster if I'd asked" is
the acceptable direction for a surprise. That also rules out having the
compiler auto-detect an atomic increment inside a `write` block — it'd be
fast when the optimizer recognised the pattern and slow when it didn't, with
no way to tell from the page.

**Where the pattern stops.** It covers lock strategies, because every variant
is still a lock and the API is honest for all of them. It does *not* stretch
to atomics: those aren't a faster lock, they're the absence of one, and
dressing them in `read`/`write` would misprice them at every use site. That
boundary is argued below.

## The API of the unified `Shared`

### Declaring

The strategy lives in the type, so it's always writable and always
inspectable — a constructor that silently changed behaviour with no type-level
trace would be magic, which is the thing to avoid.

<!-- test: skip -->
```rask
let counter: Shared<i64> = Shared.new(0)                    // default: task-local
let config:  Shared<Config, Readers> = Shared.readers(cfg)  // many tasks, concurrent reads
let queue:   Shared<Queue, Mutex> = Shared.mutex(q)         // many tasks, one at a time
```

`Mutex` stays as the **strategy** name — it's the word people already know for
"one holder at a time," and keeping it means the familiar term survives even
though the type doesn't. (An earlier draft called it `Exclusive`, which is
both non-standard and misleading: a plain mutex covers reads *and* writes, so
"exclusive" describes the locking, not what you're allowed to do.)

Write the bare `Shared<i64>` and you get `Local` — in a `let`, a parameter, a
field or a return type alike (see the rule above). Write the strategy and you've
said it out loud. Constructor and annotation agree, and either one alone is
enough for a reader to know what they have.

**On the name.** A `Shared<T>` whose default cannot be shared between tasks
reads oddly, and anyone arriving from Rust or Go will assume thread-safe until
told otherwise. Kept deliberately: the sharing in the name is among
*accessors* — several names reaching one value through a box — and the task
question is the strategy's business. What makes it a wrong assumption rather
than a trap is that it fails loudly at the boundary, not quietly at runtime:
sending a `Shared<T, Local>` is `SH7`, and the error names the fix. Noted here
so the tension is a decision on the record rather than an omission someone
files a bug about.

### Accessing

Two verbs, and both work inline or as a block — the box family's existing
shape (`mem.boxes`), unchanged:

| Form | Use |
|---|---|
| `config.read().timeout` | inline read, scope is the expression |
| `with config.read() as c { … }` | multi-statement read |
| `queue.write().push(item)` | inline write |
| `with queue.write() as q { … }` | multi-statement write |

<!-- test: skip -->
```rask
with config.write() as c {
    c.timeout = 60.seconds
    c.retries = 5
}
```

`return`, `try`, `break` and `continue` propagate through the block as they do
today.

**A quiet improvement:** today's `with mutex as q { … }` doesn't say whether
you're reading or writing. Under the unified type you always say, at every use
site. Read/write intent becomes visible in code that currently hides it.

**Uniformity is the point:** `read()` under the `Mutex` strategy takes the
exclusive lock — slower than `Readers` would be there, never wrong. Code
written generically over the strategy compiles and behaves correctly against
all three.

**`staged()` composes per strategy.** `ctrl.panic` specifies staged access on
`Mutex` (`with mutex.staged() as v { }`, `conc.sync/ST1–ST4`) — a copy that
commits as one move on non-panic exit and is discarded on unwind, so a
multi-field update can't be seen torn. Under the unified type it stays available
wherever it means something: legal on `Mutex` and `Readers`, and rejected on
`Local`, where there is no other task to observe a torn update and no unwind
boundary to protect against. `tool.warnings/W9` (`torn_lock_update`) keeps its
escape hatch: it fires on a `with` block over a sync box that assigns two or
more fields without `staged()`, and that means the two shared strategies, so the
warning and its remedy survive the merge together. A `Local` box never trips it.

## `Atomic` earns its own type after all

The previous draft folded atomics in as a third strategy, with
`with hits.write() as v { v += 1 }` compiling to a fetch-add. That was wrong,
and the objection is decisive: **the entire appeal of an atomic is that no
lock is taken, and `write()` is lock-shaped syntax.** Writing lock ceremony
for a lock-free operation misrepresents the cost at every use site — the
precise opposite of what the transparency principle asks for.

So `Atomic<T>` keeps its own type and its own vocabulary:

<!-- test: skip -->
```rask
let hits: Atomic<i64> = Atomic.new(0)

hits.add(1)                          // one instruction, no lock, no block
let n = hits.load()
hits.store(0)
let won = hits.compare_swap(old, new)
```

No `with`, no `read`/`write`, nothing that resembles acquiring anything —
because nothing is acquired. It reads as cheap because it *is* cheap.

That also removes the restriction the previous draft needed (a `write` block
limited to one supported operation, with a compile error explaining why).
The restriction was a symptom of forcing a lock-free primitive into a locking
API; with a separate type it simply doesn't arise.

**But `Atomic` still leaves the storage decision.** It earns a type on API
grounds, not because it answers "where does my data live." It belongs with
SIMD and inline assembly: documented under concurrency, reached for after
measuring, absent from the five questions below.

## What `Heap` (was `Owned`) and `Cell` really are

Both were sitting in the storage table under false pretenses, and stripping
each to its essential job moves one out and folds the other in.

### `Heap<T>` is heap indirection, not a sharing type

Its job, plainly: **put this value on the heap.** That's the whole thing. It
exists for two reasons and neither is about who can reach the data —

- a struct can't contain itself by value (infinite size), so recursion needs
  indirection;
- a large value moves in one pointer instead of a memcpy.

The linearity — consumed exactly once — isn't its purpose, it's how Rask makes
it safe without a destructor. Answering "who can reach this?" for it
is trivial: exactly one thing, the owner. It never varies, so it isn't a
question.

So it belongs with `Atomic` in the reclassified pile: a real type, doing
real work, but answering **"stack or heap?"** rather than "where does my data
live and who sees it?". It should leave the storage decision table for the
same reason `Atomic` did — it's an orthogonal axis, and mixing axes is what
made the table unchooseable.

The old name, `Owned`, named the linearity instead of the indirection —
naming the mechanism over the purpose, and worse, naming a property every
Rask value already has. Renamed to `Heap<T>`; see
[naming](fourth-option-naming.md).

### `Cell<T>` is `Shared<T>` with a task-local strategy

This reverses a call made earlier in this document, so the reasoning is worth
being explicit about.

Strip `Cell` down: **one value, reached by several accessors, mutated through
a scoped view.** That is word-for-word what `Shared` is. The only difference
is *which* accessors — closures in one task, versus tasks — and therefore what
synchronization is needed: none, versus a lock.

Which is exactly the axis the strategy parameter already models:

| | Who reaches it | Synchronization | Sendable |
|---|---|---|---|
| `Shared<T>` *(default: `Local`)* | closures and scopes in one task | none | no |
| `Shared<T, Readers>` | many tasks | read-write lock | yes |
| `Shared<T, Mutex>` | many tasks | plain lock | yes |

**Three types become one.** `Cell` and `Mutex` both disappear as type names
and survive as strategies.

**Why the earlier objection doesn't hold.** This document rejected the merge
because `func bump(c: Cell<i32>)` would need a sharing parameter, and every
signature would carry it. Two things make that acceptable now. First, the
strategy parameter already exists for locks, so this adds no new machinery —
it extends a mechanism to its natural end. Second, and more importantly, the
information *is* the caller's business: whether a value can cross a task
boundary is a genuine type-level fact, not a hidden performance detail, and
the compiler enforces it. A function that needs any strategy writes the
generic form; one that needs a sendable value says so and gets a compile
error otherwise. That's generics working correctly, not a leak.

**Defaulting to `Local` is the right way round.** Write `Shared<Counter>` in
single-task code and you pay no synchronization at all — the same cost `Cell`
has today. Try to send it to another task and the compile error names the
fix:

```
ERROR [conc.sync/SH7]: this `Shared` is task-local and cannot be sent
   |
 8 |  spawn(own || { counter.write() … })
   |                 ^^^^^^^ `Shared<i64>` uses the `Local` strategy

WHY: `Local` takes no lock, so two tasks touching it would race.

FIX: declare which lock you want:

  let counter: Shared<i64, Mutex> = Shared.mutex(0)
```

You can never accidentally pay for synchronization you didn't need, and you
can never accidentally skip synchronization you did — the expensive direction
is opt-in and the unsafe direction is a compile error.

**The cost, owned:** single-task access gains one method call —
`with counter.write() as v` where `Cell` said `with counter as v`. In
exchange, read-versus-write intent becomes visible at every use site, which
`Cell` currently hides.

## Recommendation

- **Merge `Cell`, `Shared` and `Mutex`** into one type with the access
  discipline as a defaulted type parameter — `Shared<T>` (task-local, no
  lock), `Shared<T, Readers>`, `Shared<T, Mutex>`. Three names become one;
  `Cell` and `Mutex` survive as strategies so the familiar words aren't lost.
  Removes a question users can't answer at declaration time, and makes
  sending a task-local value a compile error rather than a race. Contradicts
  `mem.boxes`' closed-family listing, so it's a deliberate change.
- **Take `Heap<T>` out of the storage question.** It answers "stack or
  heap?", which is orthogonal to every other axis — mixing it in is part of
  why the set read as unchooseable.
- **Keep `Atomic<T>` as its own type**, with a lock-free-looking API
  (`add`, `load`, `store`, `compare_swap`) and no `with` blocks. Folding it
  into `Shared` would have dressed a one-instruction operation in lock
  ceremony. It still leaves the *storage* decision — it's a measured
  optimization, documented under concurrency.
- **Keep the rest**, and ship the ordered decision procedure above in
  `DAY_ONE.md` and the boxes spec. The set isn't too large; it was presented
  as a flat menu when it's actually a sequence of yes/no questions.
- **Re-test after the edge/link change lands.** `WeakHandle` already
  disappears, `frozen` and context clauses go with it, and `Pool`→`Rack`
  removes the "when do I reach for a pool?" confusion that the
  interchangeability connotation caused. Some of the current difficulty is
  the old model's, not the set's.
