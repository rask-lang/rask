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
   (Recursive or variable-sized, so it needs the heap? → `Owned<T>`.)
2. **Many values?** → `Vec` or `Map`, unless…
3. **…other things reference them, and they can be deleted?** → `Store` +
   `Link`.
4. **Do several closures in one task share one mutable value?** → `Cell<T>`.
5. **Does it cross tasks?** → `Shared<T>` (`.read()` / `.write()`).

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
let queue  = Shared.exclusive(q)      // opt-in: plain lock, write-heavy
let hits   = Shared.atomic(0i64)      // opt-in: lock-free word
```

Everyone writes `Shared.new` and never learns the others exist. The two who
have measured a contended write path find `exclusive`; the one writing a
metrics counter finds `atomic`. The distinction stops being a fork in the
road every user must take and becomes a thing you go looking for.

**Why the type stays uniform.** The strategy is a defaulted type parameter —
`Shared<T, S = ReadWrite>` — resolved at monomorphization, exactly like the
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

**Where atomics only partly fit.** Simple operations — increment, load,
store — express fine through the normal interface and compile to atomic
instructions under `Shared.atomic`. Compare-and-swap loops and explicit
memory orderings need their own API and their own vocabulary. That's the
honest line: the common atomic operations become an opt-in variant, and the
genuinely exotic ones stay a separate advanced surface for people who already
know they want it.

## The API of the unified `Shared`

### Declaring

The strategy lives in the type, so it's always writable and always
inspectable — a constructor that silently changed behaviour with no type-level
trace would be magic, which is the thing to avoid.

<!-- test: skip -->
```rask
let config: Shared<Config> = Shared.new(cfg)                  // default
let queue:  Shared<Queue, Exclusive> = Shared.exclusive(q)    // plain lock
let hits:   Shared<i64, Atomic> = Shared.atomic(0)            // lock-free word
```

Write the bare `Shared<Config>` and you get the default; write the parameter
and you've said it out loud. The constructor and the annotation agree, and
either one alone is enough for a reader to know what they have.

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

**Uniformity is the point:** `read()` on an `Exclusive` takes the exclusive
lock. It's slower than the default variant would be there, never wrong. Code
written against `Shared<T>` compiles and behaves correctly against every
strategy, which is what makes the parameter safe to default.

### The atomic variant

Simple operations go through the same interface and compile to atomic
instructions:

<!-- test: skip -->
```rask
let n = hits.read()                       // atomic load
with hits.write() as v { v += 1 }         // atomic fetch_add
```

Arbitrary code in a `write` block can't be lock-free, so on the `Atomic`
strategy the block is restricted to one supported operation, and anything
else is a compile error that names the fix:

```
ERROR [conc.sync/SH4]: `write` block on an atomic Shared must be a single
                        supported operation
   |
 4 |  with hits.write() as v {
 5 |      v += compute_delta()
   |      ^^^^^^^^^^^^^^^^^^^^ calls a function; not expressible as one atomic

WHY: `Shared.atomic` is lock-free — it can do add, sub, swap, and store in one
     instruction, and nothing else.

FIX: compute first, then apply:

  let d = compute_delta()
  with hits.write() as v { v += d }

  or use `Shared.new(0)` if the update genuinely needs a lock.
```

Compare-and-swap and explicit memory orderings keep their own methods —
`hits.compare_swap(old, new)` — available only on the `Atomic` strategy,
where the person calling them already knows what they're doing.

## Recommendation

- **Merge `Shared` and `Mutex`** into one type, with the lock strategy as a
  constructor variant (`Shared.new` / `Shared.exclusive`) over a defaulted
  type parameter. Removes a question users can't answer at declaration time,
  and lets one signature accept every variant. Contradicts `mem.boxes`'
  closed-family listing, so it's a deliberate change.
- **Fold the common atomic operations in the same way** (`Shared.atomic`),
  leaving CAS loops and explicit orderings as a separate advanced surface.
  `Atomic` leaves the storage story either way — it answers a performance
  question, not a "where does my data live" one.
- **Keep the rest**, and ship the ordered decision procedure above in
  `DAY_ONE.md` and the boxes spec. The set isn't too large; it was presented
  as a flat menu when it's actually a sequence of yes/no questions.
- **Re-test after the edge/link change lands.** `WeakHandle` already
  disappears, `frozen` and context clauses go with it, and `Pool`→`Store`
  removes the "when do I reach for a pool?" confusion that the
  interchangeability connotation caused. Some of the current difficulty is
  the old model's, not the set's.
