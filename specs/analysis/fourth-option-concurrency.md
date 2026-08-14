<!-- id: analysis.fourth-option-concurrency -->
<!-- status: exploration -->
<!-- summary: Lock-free-by-design concurrency for edges — staged structural mutation, three parallel shapes, and the batch that replaces transactions -->
<!-- depends: analysis/fourth-option.md, analysis/fourth-option-adversarial.md, concurrency/sync.md -->

# Edges and Concurrency, Designed In

The adversarial pass answered "cross-graph edges can span two locks" with
"then wrap the ownership root in one lock." That's a patch, and a bad one:
one lock around the world is exactly the design Go beats. Concurrency
performance is a first-class requirement, so the model has to be built for it,
not fenced off from it.

Redone from the data-race surface up.

## What actually races

Four operations, and only two of them touch memory the current task doesn't own:

| Operation | Touches | Parallel-safe? |
|---|---|---|
| Follow an edge (read) | the target node | **Yes** — pure pointer chase, no writes (heal is suppressed in shared contexts) |
| Write a node's own scalar fields | that node | **Yes**, if worker chunks are disjoint |
| Assign an edge | source node **+ target's backlink** | No — cross-node write |
| Delete | node **+ every incoming edge holder** | No — cross-node write |

The race surface is exactly *structural mutation*. Everything a hot loop
spends its time on — reads and local field math — is already
contention-free.

So: **structural mutation gets staged instead of locked.** Workers enqueue
deletes and rewires into their own per-task buffer (no allocation contention,
no atomics, no locks — a plain Vec per task); the buffers apply at the phase
boundary, where exclusive access already exists by structure. The lazy
tombstone model was already halfway here: `flush_deletes()` *is* the apply
point, so this isn't a new mechanism, it's the existing one carrying one more
kind of pending work.

The hot path acquires no lock at all. Not an uncontended lock — none.

## Three shapes, all lock-free

**1. Task-owned world (the Go shape).** A world lives in one task; other
tasks send it messages over channels. Per-connection or per-request worlds
are independent values, so a server with 10k connections has 10k worlds and
zero shared structure. Messages carry `Key<T>` or domain IDs. This is what
pools.md already blessed ("share handles, not data; the pool stays in one
thread") and it's untouched — the common server/CLI case never contends.

**2. Phase parallelism (fork-join over one world).** Inside a phase, workers
run in parallel under a checked discipline:

| Tier | Workers may | Workers may not | Example |
|---|---|---|---|
| **Per-node parallel** | read own chunk, write own nodes' scalar fields | follow edges out of chunk, mutate structure | movement, integration |
| **Frozen parallel** | follow any edge, read anything | write anything | render, queries, collision detection |
| **Staged parallel** | read anything, enqueue structural ops | apply them | spawn/despawn logic, combat |

Tier 3 is the new one and it's what makes the model first-class: a worker
*decides* structurally and defers the *doing*. Applying happens once, at the
join, single-threaded or internally parallelized by the runtime (disjoint
fixup sets can go wide). Determinism falls out if buffers apply in task order —
which matters for `sim` mode (`determinism/D13`).

**3. Sharded worlds (scale-out).** One giant world doesn't parallelize by
locking harder; it shards. N worlds run on N tasks, fully independent, and
cross-shard references are `Key<T>` — checked at redemption, because a
reference the other shard's delete can't reach is exactly the case keys
exist for. This is the database answer (foreign keys inside a shard, IDs
across) and it matches the federation thinking in `projects/allgard`:
domains own their state, cross-domain is by name.

## Performance, claimed honestly

| | Pools today | Edges, staged |
|---|---|---|
| Parallel reads | generation check + Pool's internal `Arc<Mutex>` on structural ops | pointer chase, no atomics, scales linearly |
| Parallel field writes | same lock | disjoint chunks, no coordination |
| Structural ops | serialize on the pool lock | per-task buffer (no contention), applied once at the join |
| Cross-task refs | handles, checked | keys across shards; edges never cross |
| Sync primitives on the hot path | one mutex | **none** |

Pool's hidden `Arc<Mutex>` is the thing being replaced. The staged model is
strictly more parallel than what exists today, and the structure that makes
it safe (phases with a join) is the structure high-performance ECS engines
already impose by convention — here it's checked instead of documented.

### Is the apply a bottleneck? (back of envelope)

Spawn/despawn-heavy frame: 100k entities, 10k structural ops, 8 workers.

*Enqueue* is a push to a per-task Vec — ~5ns, 10k ops spread over 8 threads,
call it microseconds. Not the issue.

*Apply* is the fixup walk: only **deletes** cost anything (nothing points at
a newly inserted node yet). 10k deletes at average in-degree ~3 is ~30k
scattered pointer writes; at 20–50ns each when the holders are cold, roughly
0.6–1.5ms. Real, in a 16ms frame.

Compared against handles, honestly: those 10k removes are O(1) generation
bumps, ~50µs — handles win the *delete*. But the handle program then pays a
generation check on every follow of a stored handle, every frame, dead or
not (200k follows ≈ 400µs/frame, forever), plus the same scattered write when
it lazily clears a stale handle. Total work is lower for edges; the
difference is **shape**: edges concentrate it in a burst, handles smear it.

So the claim narrows honestly: staged edges are more parallel than pools on
reads and field writes (which dominate every real workload), and the apply is
a **latency/jitter** concern, not a throughput one. Three mitigations, all
already in the design: apply is internally parallelizable (disjoint fixup
sets don't conflict), healing amortizes K per insert, and flush points are
chosen by the programmer. Games flush per frame; a delete-storm frame can
spread its apply across two.

### "Isn't that just a GC pause?"

The fair version of the objection: deferred work accumulates and gets paid in
a batch, so the delete you wrote isn't where the cost lands. That is exactly
GC's shape, and three points concede cleanly:

- **The shape is the same.** Deferred batch, latency spike, cost displaced
  from the line that caused it.
- **Backlink maintenance is a write barrier.** Edge assignment costs ~4–8
  stores instead of 1 — structurally what a generational GC's write barrier
  does to every pointer write. Owned.
- **A hub node is a real pause.** Deleting a node with 100k incoming edges
  walks 100k scattered writes. Comparable to tracing 100k objects.

Where the analogy breaks — and these are the properties that make GC
unacceptable for systems work, all absent here:

| | Tracing GC | Flush |
|---|---|---|
| Cost scales with | what you **keep** (live set) | what you **destroyed** (your deletes × in-degree) |
| Predictability | heap biography, collector heuristics, allocation rate | computable from this frame's own deletes and the schema's fan |
| Who schedules it | the runtime | a line you wrote |
| Determinism | no (timing-dependent) | yes — same input, same cost, which `sim` mode requires |
| Mutator interruption | stop-the-world or concurrent barriers | runs at a phase join you already have; parallelizable across disjoint fixup sets |
| What the work *is* | bookkeeping the program never asked for | the exact pointer updates the manual program writes by hand |

A stable world that allocates a lot and deletes nothing costs **zero** here,
forever, while a tracing collector keeps re-scanning it. That's an inversion
of the cost model, not a variant of it.

And the dial GC doesn't have: **the flush is optional.** In lazy mode,
heal-on-read means each reader fixes its own edge as it goes — the fixup work
distributes across readers naturally, and `flush_deletes()` exists to reclaim
memory promptly, not to keep the program correct. So the burst can be
dialed out entirely, trading memory for smoothness; or dialed the other way
with eager mode, which pays at each delete and has no batch at all. For hub
nodes specifically, lazy-only is the right policy: never walk the 100k list,
let the readers who actually show up pay for themselves.

Failure mode differs accordingly: skipping flushes holds memory (a
leak-shaped curve), it never produces an unscheduled pause. The honest
category for this mechanism isn't "collector" — it's the ECS command buffer
every engine already applies at stage boundaries.

## The batch is the transaction — minus rollback

Should Rask have transactions? The useful half, yes; the expensive half, no.

Transactions buy: atomic multi-node mutation, deferred constraint checking,
and a natural undo log. They cost: rollback, which means journaling every
write or copy-on-write — hidden memory and time cost, and a nightmare
interaction with panics. That price is wrong for a systems language.

But **validate-then-apply gets the benefits without rollback.** A staged
batch is checked before anything mutates; a failing batch is rejected having
touched nothing, so there is nothing to roll back. The two-phase delete the
adversarial pass already adopted (A6) is this same shape — the batch just
generalizes it.

<!-- test: skip -->
```rask
world.stage(|w| {
    let e = w.insert(Entity { name: "drone", health: 20 })
    let b = w.bodies.insert(Body { x: 0.0, y: 0.0 })
    e.body = b          // mutual references inside the batch
    b.owner = e
})                       // constraints checked, then applied — or rejected whole
```

Three consequences:

1. **Non-optional edges come back.** A4 killed them because a required cycle
   has no legal first member. Inside a batch there *is* a legal transient
   state, and constraints are checked at apply — deferred constraints,
   exactly the database mechanism. `Edge<T>` (required) and `Edge<T>?`
   (optional) both exist again, and the distinction is meaningful: required
   edges never need a `?` at use sites.
2. **One mechanism, three jobs.** The batch is the concurrency primitive
   (per-task command buffer), the atomicity primitive (validate-then-apply),
   and the construction primitive (build cycles). Not bolted on — the same
   thing under three lights.
3. **Structural undo stays reachable.** A batch's applied fixup set is
   enumerable, so recording batches gives replay/undo without a journal on
   the write path.

What Rask still doesn't get: rollback of *arbitrary* code (no
`ensure`-and-unwind-my-writes), and cross-task distributed transactions.
Both stay out.

## Delta to the earlier docs

- A2's "one lock around the ownership root" is **superseded**. The ownership
  rule still holds for *soundness* (edges connect co-owned graphs only), but
  the concurrency answer is staging, not locking. `Mutex<World>` becomes one
  option among three, not the model.
- A4's kill of non-optional edges is **reversed**, conditional on batches
  landing: required edges are constructible inside a batch.
- The three-tier parallel contract gains tier 3 (staged parallel), which is
  where spawn/despawn-style logic lives.
