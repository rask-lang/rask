<!-- id: mem.cell -->
<!-- status: deprecated -->
<!-- summary: Cell<T> is gone — it was Shared<T> with the lock removed, which is now the default strategy -->
<!-- depends: concurrency/sync.md -->

# Cell (retired)

`Cell<T>` was "one value, reached by several accessors, mutated through a scoped
view, single-task." Strip `Shared<T>` down and you get the same sentence. The
only difference was *which* accessors — closures in one task versus tasks — and
therefore what synchronization is needed: none, versus a lock.

That is exactly what the strategy parameter models, so `Cell` folded into
[`Shared<T, Local>`](../concurrency/sync.md), which is what bare `Shared<T>`
means. See `analysis.storage-consolidation` for the argument.

## Migration

| Was | Is |
|-----|-----|
| `Cell.new(v)` | `Shared.new(v)` |
| `Cell<T>` | `Shared<T>` |
| `with cell as v { … }` | `with s.write() as v { … }` |
| read-only `with cell as v { … }` | `with s.read() as v { … }` |
| `cell.get()` / `cell.set(v)` | unchanged — `s.get()` / `s.set(v)` |
| `cell.replace(v)` / `cell.into_inner()` | unchanged |

The cost, owned: single-task access gains one method call. In exchange,
read-versus-write intent becomes visible at every use site, which `Cell` hid —
and a task-local value sent to another task is now a compile error (`conc.sync/SH7`)
instead of a race.
