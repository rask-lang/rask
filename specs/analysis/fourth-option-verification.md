<!-- id: analysis.fourth-option-verification -->
<!-- status: exploration -->
<!-- summary: Three gates before spec work — soundness argument, the strictly-better question answered honestly (no), and the flagship store written both ways and scored -->
<!-- depends: analysis/fourth-option.md, analysis/fourth-option-concurrency.md, specs/METRICS.md -->

# Verification: Soundness, Strictly-Better, and the Flagship Side by Side

Three gates before any `mem.graph` spec work. Gate 2's answer is **no** — and
the reason it's no matters more than the fact.

## Gate 1 — Is the design sound?

Soundness = three invariants, and every operation must preserve all three.

- **I1 (no dangling):** every non-`none` edge points at a live node.
- **I2 (findability):** every non-`none` edge is reachable from its target's
  incoming list.
- **I3 (agreement):** an edge is in exactly the incoming list of the node it
  points at.

| Operation | I1 | I2 | I3 |
|---|---|---|---|
| `insert(node)` | new node has edges only to live nodes (its initializers are borrows of live nodes, A3) | initializer edges register at insert, when the node has its final address | registration writes both sides together |
| `a.f = b` | `b` is live: the expression producing it is a live borrow | unlink from old target's list, link into `b`'s | both sides written in one uninterruptible step (no destructors, no user code) |
| `a.f = none` | trivially | unlink from old list | both sides |
| `delete(n)` | walks n's incoming list, sets each to `none` — after which no edge points at `n`, so freeing is safe | n's own outgoing edges unlink from their targets' lists | each fixup writes both sides |
| batch apply | validated before any mutation (A6/A4); rejected batches mutate nothing | same as the individual ops, applied in sequence | same |

The load-bearing structural facts, each already decided:

1. **Enumerability.** Edges live only in node fields (locals are block-scoped
   borrows), so the incoming list is complete by construction. This is where
   the no-storable-references ban is spent.
2. **No user code during fixup.** Rask has no destructors. A delete's unlink
   walk cannot be observed or interrupted mid-way, so I3 never has an
   observable violation window.
3. **Sync-domain closure** (A2): edges connect only co-owned graphs, and a
   sync boundary encloses the whole ownership root — so fixups never cross a
   lock they don't hold.
4. **Borrow exclusion:** deleting while a node is borrowed is the existing
   W2c-shaped compile error; aliased `with` scopes address-compare and panic
   (A1).

**Verdict: sound, conditional on four things being specced, not assumed** —
(a) the aliasing address-compare, (b) in-flight literal borrows, (c) batch
validation covering required-edge constraints, (d) index backlinks surviving
a `Map` rehash. (d) is the only one with no worked design yet; it is the
first thing a spec draft must solve.

## Gate 2 — Is it strictly better than handles? **No.**

Stated plainly, because a false "yes" here would be the most expensive kind
of wrong. Handles win five things:

| Where handles win | Why | Severity |
|---|---|---|
| Reference **writes** | 1 integer store vs ~4–7 stores + list surgery | Real; matters for rewire-heavy workloads |
| **Delete** cost | O(1) generation bump vs O(in-degree) fixup | Real; matters for hub nodes and delete storms |
| **Escaping** references | a handle is a 12-byte Copy value that goes anywhere — channels, files, other tasks | Structural; edges cannot do this at all |
| **Relocation** (mmap, `to_bytes`) | integers survive being moved; pointers don't | Real, narrow |
| **Snapshot** | shallow memcpy vs pointer translation | Real, narrow |

Edges win: read cost (a plain deref vs a check), the stale-reference bug
class (impossible vs detected), reference-maintenance code volume (~half),
bidirectional memory (16B vs 32B + generations), day-one concept count, and
they retire `using Pool<T>`, `frozen`, and the whole generation-coalescing
pass.

**So the honest claim is not "strictly better." It is: edges dominate on the
axes that dominate real programs (reads, correctness, ergonomics) and lose on
axes that are either rare (hub churn, mmap, snapshot) or served by keeping a
second mechanism (escape → `Key<T>`).** Which is why the design keeps keys
rather than claiming to replace them.

If the bar really is *strictly* better on every axis, the answer is no and
the exploration should stop here. If the bar is "better on balance for
Rask's stated goals" (METRICS), gate 3 measures it.

## Gate 3 — The flagship store, both ways

`examples/validation/store.rk` — tasks in a `Pool<Task>`, referencing each
other by handle through `deps`, indexed by `by_id: Map<TaskId, Handle<Task>>`.

### The bug the handle version actually has

Found while writing this comparison, filed as
[#740](https://github.com/rask-lang/rask/issues/740). `delete_task` drops the
task from the pool and the index but never removes it from other tasks'
`deps`:

<!-- test: skip -->
```rask
func delete_task(mutate self, id: TaskId) -> void or StoreError {
    let h = self.by_id.get(id) ?? return StoreError.NotFound(id.value)
    self.tasks.remove(h)
    self.by_id.remove(id)
    return
}
```

`task_is_blocked` then walks those handles and panics on the stale one.
Reduced and confirmed on the interpreter:

```
before delete: frontend blocked = true
error[R0010]: panic: stale handle
```

Any `GET /tasks/{id}` touching a task whose dependency was deleted takes down
the request. **This is not a hypothetical failure mode from the design
argument — it is the flagship validation program, written carefully, getting
the pattern wrong.** That is the strongest single piece of evidence in this
whole exploration, and it arrived unprompted.

### Side by side

Deletion, handles (correct version — what the code *should* say):

<!-- test: skip -->
```rask
func delete_task(mutate self, id: TaskId) -> void or StoreError {
    let h = self.by_id.get(id) ?? return StoreError.NotFound(id.value)
    for other in self.tasks.handles() {          // O(n) sweep over every task
        self.tasks[other].deps.remove_where(|d| d == h)
    }
    self.tasks.remove(h)
    self.by_id.remove(id)
    return
}
```

Deletion, edges:

<!-- test: skip -->
```rask
func delete_task(mutate self, id: TaskId) -> void or StoreError {
    let t = self.by_id.get(id) ?? return StoreError.NotFound(id.value)
    self.tasks.delete(t)      // deps entries and the by_id row drop out
    return
}
```

The blocked check, handles (correct version) vs edges:

<!-- test: skip -->
```rask
// handles: every reader repeats the staleness dance
func task_is_blocked(h: Handle<Task>) -> bool using frozen Pool<Task> {
    for dep in h.deps {
        if pool.get(dep)? as d {
            if !d.status.is_terminal() { return true }
        }
    }
    return false
}

// edges: deleted deps are not in the list
func task_is_blocked(t: Task) -> bool {
    for dep in t.deps {
        if !dep.status.is_terminal() { return true }
    }
    return false
}
```

### Scoring (per METRICS)

| Metric | Handles | Edges | Note |
|---|---|---|---|
| **MC** (stale refs) | detected at runtime; **the flagship gets it wrong** (#740) | impossible | The bug class stops existing |
| **SN** on the two functions above | ceremony ≈ logic on the blocked check; the sweep is pure ceremony | ~0.2 | Handles cross the 0.3 red line here |
| **ED** (delete path) | 6 lines + an O(n) sweep, or 3 lines and a latent panic | 3 lines | Half, and no wrong-but-compiles variant |
| **UCC** (web services, 30% weight) | 1.0 — expressible | 1.0 | Both express it; edges express it *correctly by default* |
| **PI** | flat costs | delete now O(deps), not O(1) | Small loss |
| **RO** (list endpoint, read-dominated) | generation check per dep per view | plain deref | Edges faster on the hot path this service actually runs |
| **RS** | `Pool`, `Handle`, `using frozen`, the get-dance | `Edges<T>`, a root index | Fewer, and the store's `using frozen` helpers lose their reason to exist |

**Sharpest finding:** the escaping identity in this program is already
`TaskId` — a domain id redeemed through `by_id`. No `Handle` ever crosses the
wire. So under edges this service needs **no keys at all**: `by_id` becomes a
root `Map<TaskId, Edge<Task>>`, and the entire `Handle` vocabulary leaves the
flagship.

## What this establishes, and what it doesn't

Established: the model is internally sound given four specced rules; it is
**not** strictly better than handles, and the places it loses are named; on
the flagship it produces less code, fewer concepts, faster reads, and it
makes the actual shipped bug unrepresentable.

Not established: any measured performance number (nothing is implemented),
behaviour at hub scale, the `Map` rehash backlink design, and whether the
migration cost across specs, compiler, and examples is worth it. Those need
either a prototype or a decision to accept the risk.
