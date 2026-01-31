# Async and Concurrency

**This specification has been split into focused modules.**

See [specs/concurrency/](concurrency/) for the complete specification:

| Spec | Purpose |
|------|---------|
| [README.md](concurrency/README.md) | Overview and navigation |
| [sync-concurrency.md](concurrency/sync-concurrency.md) | OS threads, nurseries, channels (Layer 1) |
| [parallel-compute.md](concurrency/parallel-compute.md) | parallel_map, thread pools (Layer 2) |
| [async-runtime.md](concurrency/async-runtime.md) | Green tasks, async/await (Layer 3) |
| [select-and-multiplex.md](concurrency/select-and-multiplex.md) | Select statement (Layer 4) |

## Design Summary

**Decision:** OS threads with structured nurseries, ownership-transfer channels, and opt-in async runtime.

**Rationale:** Sync-first approach covers 80%+ of use cases. Async is an optimization for high-concurrency scenarios (10k+ connections).

## Quick Reference

### Sync (Default)
```
nursery { |n|
    n.spawn { work() }  // OS thread, ~2MB stack
}
```

### Async (Opt-in)
```
async nursery { |n|
    n.async_spawn { work().await }  // Green task, ~4KB
}
```

### Channels
```
(tx, rx) = Channel<T>.buffered(100)
tx.send(value)?  // Ownership transfers
rx.recv()?
```

## Critical Open Issues

1. **Sync nursery blocks async runtime** — See [async-runtime.md](concurrency/async-runtime.md#critical-design-issues)
2. **Linear types + channels** — See [sync-concurrency.md](concurrency/sync-concurrency.md#remaining-issues)
