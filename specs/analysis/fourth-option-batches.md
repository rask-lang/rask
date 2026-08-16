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

## B8 — `delete` must not mean two things

A flaw caught after the first pass, and it's one this exploration already
ruled against elsewhere. Under B1, `store.delete(x)` outside a batch happens
immediately and `w.delete(x)` inside one is deferred — the same verb, two
timings, distinguished only by which block encloses it.

That's precisely the objection used to reject implicit delete-locked loops
(`fourth-option.md`, option (b)): *"`store.delete(x)` would mean something
different inside a loop than outside it, with no syntax marking the
difference."* The reasoning doesn't get to apply to loops and not to batches.

Two ways out.

**(a) Deletes only exist inside batches.** Remove the immediate form
entirely. `delete` then always means "at the end of this block" — one
meaning, everywhere, and a reference is *never* invalidated under a holder's
feet because there is no delete that can do it. The delete-locked scope stops
being a property of batches and becomes a property of the language.

The cost is ceremony on single deletes. `cache.rk`'s `evict_one` removes one
node; it would need a batch block around it:

<!-- test: skip -->
```rask
with self.blocks.batch() as b { b.delete(victim) }
```

Three lines where one would do, in a fairly common shape.

**(b) The deferred one gets its own verb.** Keep `store.delete(x)` immediate,
and name the batch's version for what it does — `retire`, `mark_deleted`,
`schedule_delete`. The timing lands in the call rather than in the enclosing
block, which is what the transparency argument asks for.

**Leaning (a).** Uniformity is worth more than the saved lines, and it buys a
property nothing else in the design provides: with no immediate delete, a
node reference can never go stale while you hold it — anywhere, not just
inside a batch. That subsumes the delete-locked scope, makes B7 unnecessary
(there's no "pre-batch world" to distinguish), and removes a whole class of
question about which construct you're in.

The SQL analogy that seemed to license the dual meaning doesn't hold: inside
a transaction *everything* defers, so there's one rule. Here inserts are
immediate and deletes aren't, so a reader has to track which is which — the
asymmetry is what makes the shared verb misleading, and (a) resolves it by
making the deferred side universal rather than contextual.

### Resolved: (a), and the sugar makes the rule uniform

A single delete in its own batch **is** an immediate delete — the block opens
and closes around one call, so the effect lands right there. So the bare form
is sugar, not a second mechanism:

<!-- test: skip -->
```rask
store.delete(x)                                  // sugar for:
with store.batch() as b { b.delete(x) }
```

Which collapses the whole thing to one sentence:

> **A delete takes effect at the end of its enclosing batch. A bare delete is
> its own batch.**

Nesting (B6) makes that hold everywhere: write `store.delete(x)` *inside* an
explicit batch and it flattens into that batch and defers, exactly as the
sentence says. No case is special, and the ergonomics objection to (a)
evaporates — `evict_one` keeps its one-liner.

### The word for the batch-scoped form

With the timing rule uniform, one verb everywhere is defensible: the receiver
already marks it (`w.delete` where `w` is bound by a batch block, versus
`store.delete`), which is how ECS command buffers distinguish the two.

But "the receiver marks it" is a weaker signal than putting it in the verb,
and the honest reading of `w.delete(enemy)` in isolation is still "delete the
enemy, now."

| Candidate | For | Against |
|---|---|---|
| `delete` | one word, uniform rule, receiver marks it | reads as immediate in isolation |
| `schedule_delete` | unambiguous | clunky; compound where Rask prefers a verb |
| `queue_delete` / `defer_delete` | names the mechanism / the timing | same clunkiness |
| **`retire`** | single plain verb; means "take out of service" with no implication of *now*; a retired thing still exists, which is exactly the node's state until apply | slightly soft |
| `condemn` | most precise — a condemned building still stands and is going to come down | too dramatic for a systems language |

**Lean: `retire`.** `w.retire(enemy)` reads honestly, stays a single plain
verb in Rask's style, and its ordinary meaning matches the semantics
precisely: still here, definitely going. `store.delete(x)` keeps the direct
name for the one-shot sugar, where the effect really is immediate.

Not settled — `retire` versus keeping `delete` on both is a judgement about
whether the receiver is marker enough.
