<!-- id: analysis.fourth-option-batches -->
<!-- status: exploration -->
<!-- summary: Design pass on batches — the one open mechanism, carrying four jobs -->
<!-- depends: analysis/fourth-option.md, analysis/fourth-option-concurrency.md -->

# Batches

The last open mechanism, and it ended up carrying four jobs:

1. **Deferred structural mutation** for parallelism — workers enqueue, the
   join applies.
2. **Atomicity** — a failed operation leaves nothing half-done.
3. **Cycle construction** — mutual required links need a transient state where
   the rules are temporarily unmet.
4. **A delete-locked scope** — references stay valid because nothing dies
   while the batch runs.

Anything doing four jobs needs its edges pinned down.

## The shape

<!-- test: skip -->
```rask
with world.batch() as w {
    let a = w.insert(Entity { name: "drone", health: 20 })
    let b = w.insert(Body { x: 0.0, y: 0.0 })
    a.body = b                    // mutual required links: legal in here
    b.owner = a
    w.delete(old_target)
}                                  // applies here
```

A `with` block rather than a closure, for the reason the box family already
uses one (`mem.boxes`): `return`, `try`, `break` and `continue` propagate
naturally, and a closure would swallow them.

## B1 — Only deletes defer

The central simplification, and it took a while to see. The three structural
operations are not symmetric:

| Operation | Timing | Why |
|---|---|---|
| `insert` | **immediate** — the node exists and can be linked at once | nothing can dangle from a node coming into existence |
| link assignment | **immediate** | rewiring invalidates nothing |
| `delete` | **deferred to apply** | this is the only operation that can invalidate a reference |

So a batch is, precisely: **a region in which deletes are deferred.** Not a
general command buffer. That is a much smaller mechanism than the earlier
sketch, and it still serves all four jobs — because deletion is the only
thing any of them needed to control.

Job 4 falls out immediately: nothing dies during the block, so node
references collected into a local `Vec` stay valid for its duration. The
delete-locked scope isn't a separate feature; it's what a batch *is*.

## B2 — Inserted nodes are private until apply

An inserted node is real (allocated, linkable, readable) but is **not
published to the store's iteration** until the batch applies. A `for` over
the store inside the batch doesn't see it.

Without this, a loop that inserts while iterating would visit its own output
— the same reason today's pools snapshot (`mem.pools/PF3`, "elements inserted
during iteration are not visited").

## B3 — Apply happens only on normal completion

Any early exit — `return`, `try`, `break`, `continue` out of the block, or a
panic — **abandons** the batch:

- staged deletes are dropped, and the nodes stay alive;
- nodes inserted during the batch are deleted, running the normal fixup so
  any links into them are nulled.

That gives genuine all-or-nothing without a journal or copy-on-write: there's
nothing to roll back, because deletes hadn't happened and inserts undo by the
same mechanism deletion already uses. Cost is proportional to what the
abandoned batch wired — an error path paying for its own cleanup.

This is the one place Rask gets rollback for free, and it's worth being
precise about why: **the batch never mutated anything a rollback would have
to reconstruct.**

## B4 — Required links are checked at compile time

The earlier sketch said "validate at apply, reject the batch." That turns out
to be unnecessary. Inside a batch, a node literal may omit a required
`Link<T>`, and the compiler tracks — by ordinary definite-assignment analysis
over the block — that every omitted link is assigned before the block ends.

<!-- test: skip -->
```rask
with world.batch() as w {
    let a = w.insert(Entity { name: "drone", health: 20 })   // body omitted
    let b = w.insert(Body { x: 0.0, y: 0.0 })
    a.body = b        // assigned before block end ✓
    b.owner = a
}
```

Leaving one unassigned is a compile error naming the field and the block —
the same shape as using an uninitialised `mut`. So **batches have no
validation step and no rejection path.** They cannot fail; they can only be
abandoned by control flow (B3).

That deletes `BatchError` from the design, and it's a strictly better answer
than the runtime check it replaces.

## B5 — Deleting an already-deleted node is a no-op

Two workers may stage a delete of the same node. At apply, the second is
ignored rather than an error. This keeps per-task buffers independent — no
cross-buffer validation, no ordering requirement between them beyond the
task-order rule that keeps `sim` deterministic (`determinism/D13`).

Consistent with today's `pool.remove` returning `T?` rather than failing.

## B6 — Nested batches flatten

A function may open a batch, and may be called from inside one. The inner
block joins the enclosing batch rather than creating a new boundary: it
doesn't apply at its own end, and abandoning the outer abandons everything.

The alternative — making nesting a compile error — breaks composition, since
a helper would be callable from only one context. Flattening is what nested
transactions do, and the surprise (an inner block completing without
applying) is worth stating in the docs but not worth forbidding
composability over.

## B7 — Reads inside a batch see the pre-batch world

Because deletes haven't applied, a node staged for deletion is still readable
inside the block. That's not a wart, it's B1 restated: the world is stable
for the batch's duration, which is exactly what job 4 needs.

<!-- test: skip -->
```rask
with world.batch() as w {
    w.delete(target)
    let n = target.name        // still readable — the delete hasn't happened
}
```

## What this does *not* provide

- **No rollback of ordinary code.** Local variables, I/O, and anything
  outside the store are untouched by abandonment. A batch is not a
  transaction over your whole program.
- **No cross-task atomicity.** Each task's buffer applies as a unit; two
  tasks' batches are not one transaction.
- **No isolation between concurrent batches** beyond the phase structure —
  they don't observe each other because they run in a parallel phase and
  apply at the join, not because anything enforces isolation.

## Open, after this pass

- **Parallel inserts need an allocation story.** B1 makes inserts immediate,
  which means workers in a parallel phase allocate concurrently. Either the
  store's allocator is per-task with a merge at the join, or inserts are the
  one operation that *does* defer. This is the remaining hole and it's a real
  one.
- **Syntax is placeholder.** `world.batch()` reads acceptably; the name isn't
  settled.
