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

## What's left, and how to choose

Six, after the lock merge — and the choice is mechanical if the questions are
asked in the right order:

1. **Is it one value, held by exactly one owner?** → a plain field. Done.
   (Recursive or variable-sized, so it needs the heap? → `Owned<T>`.)
2. **Many values?** → `Vec` or `Map`, unless…
3. **…other things reference them, and they can be deleted?** → `Store` +
   `Link`.
4. **Do several closures in one task share one mutable value?** → `Cell<T>`.
5. **Does it cross tasks?** → `Shared<T>` (`.read()` / `.write()`), or
   `Atomic<T>` for a single word.

Read as a rule: **plain fields until you have many; `Vec`/`Map` until they
reference each other; `Store` when they do; and the concurrency types only
when a second task exists.** Nothing above step 3 is reached by an ordinary
program.

The reason this reads better than the table is that the axes are now
*sequential*. The current set forces a simultaneous three-way judgement,
which is exactly the thing that's hard, and no amount of naming fixes it.

## Recommendation

- **Merge `Shared` and `Mutex`.** Clean win, removes a question users can't
  answer at declaration time. Contradicts `mem.boxes`' closed-family listing,
  so it's a deliberate change, not a clarification.
- **Keep the rest**, and ship the ordered decision procedure above in
  `DAY_ONE.md` and the boxes spec. The set isn't too large; it was presented
  as a flat menu when it's actually a sequence of yes/no questions.
- **Re-test after the edge/link change lands.** `WeakHandle` already
  disappears, `frozen` and context clauses go with it, and `Pool`→`Store`
  removes the "when do I reach for a pool?" confusion that the
  interchangeability connotation caused. Some of the current difficulty is
  the old model's, not the set's.
