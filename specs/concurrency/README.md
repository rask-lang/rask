# Concurrency Specifications

This folder contains the concurrency model for Rask, split into focused, independently-designable pieces.

## Design Philosophy

**Sync-first:** Most programs need OS threads + channels. Async is an optimization for 10k+ connections.

**Layered design:** Each layer builds on the previous:

```
Layer 1: sync-concurrency.md    ← Foundation (80% of programs)
Layer 2: parallel-compute.md    ← CPU-bound work (orthogonal)
Layer 3: async-runtime.md       ← High-concurrency optimization
Layer 4: select-and-multiplex.md ← Advanced patterns
```

## Specifications

| Spec | Status | Purpose |
|------|--------|---------|
| [sync-concurrency.md](sync-concurrency.md) | Draft | OS threads, nurseries, channels, task capture |
| [parallel-compute.md](parallel-compute.md) | Draft | parallel_map, thread pools, CPU parallelism |
| [async-runtime.md](async-runtime.md) | Draft | Green tasks, async/await, runtime |
| [select-and-multiplex.md](select-and-multiplex.md) | Draft | Select statement, multiplexing |

## Validation Criteria

Each layer has clear test criteria before moving to the next:

**Layer 1 (sync-concurrency):**
- Can build HTTP server handling 1000 concurrent connections?
- Can build CLI pipeline tool (grep | sort | uniq)?
- Can build producer-consumer with multiple workers?

**Layer 2 (parallel-compute):**
- Can process 1M items across all CPU cores?
- Can handle errors in parallel operations?

**Layer 3 (async-runtime):**
- Can build proxy handling 100k connections?
- Is sync/async interaction well-defined?

**Layer 4 (select-and-multiplex):**
- Can wait on multiple channels with timeout?
- Does select work in both sync and async contexts?

## Critical Design Issues

These issues span multiple specs and need resolution:

1. **Sync nursery in async context** — Does `nursery` block the async runtime? (See async-runtime.md)
2. **Linear types in channels** — Silent cleanup on drop? (See sync-concurrency.md)
3. **Cooperative cancellation** — No forced termination (See sync-concurrency.md)

## Integration

All specs share these principles from CORE_DESIGN.md:
- Tasks own their data (no shared mutable state)
- Channels transfer ownership (move semantics)
- TaskHandle is affine (must be consumed)
- No storable references (captures by move only)
- Local analysis only (no cross-function lifetime tracking)
