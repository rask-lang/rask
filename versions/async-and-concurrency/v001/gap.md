# Gap 1: Channel Buffering and Backpressure

**Priority:** HIGH
**Type:** Underspecified

**Question:** How are channel capacities specified? What happens when a buffered channel is full?

**Current spec:** Shows example `Channel<T>.buffered(1000)` but doesn't specify:
- Unbuffered (rendezvous) channels
- What `send()` does when buffer is full (block? error?)
- Backpressure semantics
- Capacity types (bounded vs unbounded)

**Impact:** Cannot implement channel API or reason about backpressure patterns.
