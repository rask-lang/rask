<!-- id: analysis.storage-consolidation -->
<!-- status: exploration -->
<!-- summary: Can the storage types merge? One real merge, two false ones, and a decision procedure for what's left -->
<!-- depends: memory/boxes.md, analysis/fourth-option.md -->

# Can the Storage Types Consolidate?

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
| `Store` + `Link` | many | this scope | **yes** |
| `Owned<T>` | one | movable, exclusive | no |
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

## The one real merge: `Shared` + `Mutex`

They are the same concept: a box that many tasks reach, holding one value,
serialized by a lock. The split is a *performance* distinction — many-readers
versus exclusive — and asking a user to pick a **type** based on their
expected read/write ratio is asking them to make a benchmarking decision at
declaration time, before the program exists.

Merge into one, with the discipline in the methods:

<!-- test: skip -->
```rask
let config: Shared<Config> = Shared.new(cfg)

let t = config.read().timeout          // shared access
with config.write() as c {             // exclusive access
    c.retries = 5
}
```

`Mutex<T>` disappears; `with mutex as v` becomes `with x.write() as v`. Write-
heavy workloads pay a small cost over a plain mutex, and if that ever matters
it's an annotation on the declaration, not a different type in the language.

**Net: two names become one, and the question "which lock type?" stops
existing.**

## Two merges that look right and aren't

**`Cell` into the lock family.** Tempting: `Cell` is a one-value box for one
task, `Shared` is a one-value box for many tasks, so let the compiler notice
whether it crosses a task boundary and pick the representation.

It breaks on function signatures. `func bump(c: Cell<i32>)` — is that one
locked or not? The answer depends on the caller, so the type needs a sharing
parameter and every signature carries it. That trades two familiar names for
a new generic dimension across the whole language. Worse deal.

**`Store` into `Vec`.** Also tempting: a `Store` with no links declared is
nearly a `Vec`. But the guarantees are opposite — `Vec` gives contiguity and
moves its elements on `push`; `Store` gives stable addresses and never moves
them. Merging means one of those promises is broken, and both are load-bearing
(contiguity for iteration speed, stability for links).

`Owned` into `Store` fails on cost, argued in
[the guide](fourth-option-guide.md): a store node carries a back-pointer per
reference, an AST doesn't need one, and `Owned` can be returned from a
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
3. **…other things reference them, and they can be deleted?** → `Store` +
   `Link`.
4. **Do several accessors share one mutable value?** → `Shared<T>`, plus a
   strategy if it crosses tasks (`Mutex` / `Readers`).

Two orthogonal questions sit *outside* this list, which is why mixing them in
made it unchooseable:

- **Does it need to be on the heap** (recursive, or large and moved often)?
  → wrap it in `Owned<T>`. Independent of every answer above.
- **Is this a contended counter or flag you've measured?** → `Atomic<T>`.
  A concurrency primitive, not a storage choice.

Read as a rule: **plain fields until you have many; `Vec`/`Map` until they
reference each other; `Store` when they do; and the concurrency types only
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
let config = Shared.new(cfg)          // default: many readers at once
let queue  = Shared.mutex(q)          // opt-in: plain lock, write-heavy
```

Everyone writes `Shared.new` and never learns the other exists. The person who
has measured a contended write path goes looking and finds `mutex`. The
distinction stops being a fork in the road every user must take and becomes a
thing you seek out.

(An earlier draft added `Shared.atomic` here too. That's since been rejected —
see below: atomics need an API that doesn't look like locking, so they keep
their own type.)

**Why the type stays uniform.** The strategy is a defaulted type parameter —
`Shared<T, S = Readers>` — resolved at monomorphization, exactly like the
allocator parameter (`mem.alloc/AL4`). So it's zero-cost, and more
importantly a function written `func serve(c: Shared<Config>)` accepts every
variant. Today, `Mutex<T>` and `Shared<T>` being separate types means an API
must pick one and callers must match it; consolidating removes that too.

This is *not* the objection that killed folding `Cell` into the lock family.
There, the parameter would have carried **semantics** — thread-safe or not,
which changes what the code means. Here every variant is equally correct and
they differ only in speed, so a caller can never be wrong, only suboptimal.

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
let config: Shared<Config> = Shared.new(cfg)              // default: concurrent readers
let queue:  Shared<Queue, Mutex> = Shared.mutex(q)        // opt-in: one at a time
```

`Mutex` stays as the **strategy** name — it's the word people already know for
"one holder at a time," and keeping it means the familiar term survives even
though the type doesn't. (An earlier draft called it `Exclusive`, which is
both non-standard and misleading: a plain mutex covers reads *and* writes, so
"exclusive" describes the locking, not what you're allowed to do.)

Write the bare `Shared<Config>` and you get the default; write the parameter
and you've said it out loud. Constructor and annotation agree, and either one
alone is enough for a reader to know what they have.

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
exclusive lock. It's slower than the default variant would be there, never
wrong. Code written against `Shared<T>` compiles and behaves correctly against
every strategy, which is what makes the parameter safe to default.

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

## What `Owned` and `Cell` really are

Both were sitting in the storage table under false pretenses, and stripping
each to its essential job moves one out and folds the other in.

### `Owned<T>` is heap indirection, not a sharing type

Its job, plainly: **put this value on the heap.** That's the whole thing. It
exists for two reasons and neither is about who can reach the data —

- a struct can't contain itself by value (infinite size), so recursion needs
  indirection;
- a large value moves in one pointer instead of a memcpy.

The linearity — consumed exactly once — isn't its purpose, it's how Rask makes
it safe without a destructor. Answering "who can reach this?" for an `Owned`
is trivial: exactly one thing, the owner. It never varies, so it isn't a
question.

So `Owned` belongs with `Atomic` in the reclassified pile: a real type, doing
real work, but answering **"stack or heap?"** rather than "where does my data
live and who sees it?". It should leave the storage decision table for the
same reason `Atomic` did — it's an orthogonal axis, and mixing axes is what
made the table unchooseable.

Rust calls this `Box`, and the name is honest about it: a box you put a thing
in. `Owned` names the linearity instead of the indirection, which may be
naming the mechanism over the purpose — worth revisiting, out of scope here.

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

- **Merge `Shared` and `Mutex`** into one type, with the lock strategy as a
  defaulted type parameter (`Shared<T>` / `Shared<T, Mutex>`) and a matching
  constructor. Removes a question users can't answer at declaration time, and
  lets one signature accept every variant. `Mutex` survives as the strategy
  name so the familiar word isn't lost. Contradicts `mem.boxes`' closed-family
  listing, so it's a deliberate change.
- **Keep `Atomic<T>` as its own type**, with a lock-free-looking API
  (`add`, `load`, `store`, `compare_swap`) and no `with` blocks. Folding it
  into `Shared` would have dressed a one-instruction operation in lock
  ceremony. It still leaves the *storage* decision — it's a measured
  optimization, documented under concurrency.
- **Keep the rest**, and ship the ordered decision procedure above in
  `DAY_ONE.md` and the boxes spec. The set isn't too large; it was presented
  as a flat menu when it's actually a sequence of yes/no questions.
- **Re-test after the edge/link change lands.** `WeakHandle` already
  disappears, `frozen` and context clauses go with it, and `Pool`→`Store`
  removes the "when do I reach for a pool?" confusion that the
  interchangeability connotation caused. Some of the current difficulty is
  the old model's, not the set's.
