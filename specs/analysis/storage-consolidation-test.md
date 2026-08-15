<!-- id: analysis.storage-consolidation-test -->
<!-- status: exploration -->
<!-- summary: The consolidated storage design run against every storage usage in the example corpus -->
<!-- depends: analysis/storage-type-consolidation.md, analysis/fourth-option.md -->

# Testing the Consolidation Against Real Code

The design can't be executed, so the test is this: take **every** storage
declaration in the example corpus, run the four-question procedure on it, and
record whether the answer is unambiguous, and whether the result is better or
worse than what's there today.

## Every usage in the corpus

| Where | Today | Question it lands on | Becomes | Verdict |
|---|---|---|---|---|
| `validation/store.rk` | `Pool<Task>` + `Map<TaskId, Handle<Task>>` | 3 — they reference each other and get deleted | `Store<Task>` + `Map<TaskId, Link<Task>>` | clean; fixes [#740](https://github.com/rask-lang/rask/issues/740) |
| `validation/store.rk` | `const store = Mutex.new(Store.new())` | 4 — crosses tasks, write-heavy | `Shared<Store, Mutex>` | clean |
| `validation/config.rk` | `const config = Shared.new(…)` | 4 — crosses tasks, read-heavy | `Shared<Config, Readers>` | clean |
| `validation/metrics.rk` | `const metrics = Mutex.new(…)` | 4 — crosses tasks | `Shared<Metrics, Mutex>` | clean |
| `validation/middleware.rk` | `counters: Mutex<Map<string, u64>>` | 4 — crosses tasks | `Shared<Map<…>, Mutex>` | clean |
| `http_api_server.rk` | `const db = Shared.new(…)` | 4 | `Shared<Database, Readers>` | clean |
| `package_manager.rk` | `Pool<ResolvedPkg>` + `Map<string, Handle<…>>` | 3 — dependency graph | `Store` + `Link` | clean |
| `package_manager.rk` | `let counter = Mutex.new(0)` | 4 — cross-task integer counter | `Shared<i32, Mutex>` | **see below** |
| `lsm_database/cache.rk` | `Pool<CacheBlock>` + `Map<string, Handle<…>>` | 3 — LRU cache with an index | `Store` + `Link` | clean; removes real ceremony |
| `text_editor.rk` | `Pool<Line>` + `Vec<Handle<Line>>` | 3 — ordered line view | `Store` + `Vec<Link<Line>>` | clean |
| `game_loop.rk` | `Pool<Entity>` + `Handle<Entity>?` | 3 — entities target each other | `Store` + `Link` | clean |
| `cli_calculator.rk` | `Heap<Expr>` | orthogonal — recursive, needs the heap | `Heap<Expr>` | unchanged |
| anywhere | `Cell<T>` | — | — | **zero usages** |
| anywhere | `Atomic<T>` | — | — | **zero usages** |

Twelve real declarations. Every one lands on exactly one question with no
judgement call required. That's the result the procedure was built for, and
it holds.

## Five findings

### 1. `Cell` is used zero times in the entire corpus

Not once, across the flagship web service, a database, a package manager, a
game loop, a text editor and nineteen tutorial examples. A type nobody
reaches for is pure cost in the "which do I pick?" question — and it means
folding it into `Shared` has **no migration cost at all**. The merge argued
on principle turns out to be free in practice.

### 2. `Atomic` is also used zero times — and there's exactly one place it belongs

`package_manager.rk` wraps a cross-task fetch counter in `Mutex.new(0)`, which
is precisely the atomic case: one integer, incremented from many tasks.

Under the new procedure the author still lands on `Shared<i32, Mutex>`, and
only finds `Atomic` if they go measuring. **That's the design working as
specified** — conservative default, opt-in fast path — and this corpus entry
is the evidence that the conservative default is what real code actually
reaches for. Nobody wrote `Atomic` even where it fit.

### 3. `cache.rk` shows the index-maintenance ceremony concretely

The cache is correct today — unlike the flagship — but pays for it visibly:

<!-- test: skip -->
```rask
func evict_one(mutate self) {
    // …find victim…
    if victim? as h {
        let key = with self.blocks[h] as block {
            BlockCache.cache_key(block.sst_id, block.block_id)   // rebuild the key
        }
        self.blocks.remove(h)
        self.by_key.remove(key)                                    // and unindex
    }
}
```

It has to **reconstruct the index key from the block's own fields** purely so
it can remove the index entry. Under links the whole tail is
`self.blocks.delete(h)` — the `by_key` row drops itself. `invalidate()` does
the same dance in bulk, building a `Vec<(Handle, key)>` to carry both halves.

This is the same shape as #740, caught early enough to be written correctly.
The ceremony is the cost of *remembering* — and forgetting it is the bug.

### 4. `cache.rk` also settles the container name

Its header comment reads: *"Demonstrates `Pool<T>` and `Handle<T>` for
**non-graph** use cases."* The corpus is explicitly documenting that this
container is used for things that aren't graphs — written before any of this
exploration existed. `Graph<CacheBlock>` (an early working name) would have
contradicted a comment already in the tree. `Store<CacheBlock>` reads
correctly.

### 5. `own` already means something else

`Owned<T>` (now `Heap<T>`) is the heap box, but `own` is also the
move-capture keyword:

<!-- test: skip -->
```rask
Expr.Binary(left: own base, …)      // heap-allocate
spawn(own || { … })                 // move-capture a closure
mut opts = parse_args(own args)     // move an argument
```

Three uses of `own` in the corpus, two of which have nothing to do with heap
allocation. This finding fed directly into the rename: `Owned<T>` became
`Heap<T>` with `Heap(expr)` construction, freeing `own` to mean move-capture
and nothing else.

## What the test did not cover

- **Nothing was executed.** No performance claim in the consolidation doc is
  measured, and this test doesn't change that.
- **The `Local` default was not stress-tested.** No corpus example shares a
  mutable value between closures in one task, so the case `Shared<T>`'s
  default exists for is unrepresented — which is itself the finding in §1,
  but it means the default's ergonomics are untested against real code.
- **Twelve declarations is a small sample**, drawn from one author's examples.
  It shows the procedure is unambiguous on the code that exists; it can't show
  it stays unambiguous on code that doesn't.

## Verdict

The procedure resolves all twelve real usages without a judgement call. The
two types being removed from the storage question (`Cell` folded, `Atomic`
reclassified) have zero usages between them, so both changes are free. The
one place `Atomic` genuinely fits was written as a `Mutex` by its author,
which is exactly the behaviour the conservative default predicts.

The consolidation survives contact with the corpus.
