# Elaboration: Channel Buffering and Backpressure

## Channel Creation API

| Constructor | Capacity | Blocking Behavior |
|------------|----------|-------------------|
| `Channel<T>.unbounded()` | Unlimited | Never blocks on send |
| `Channel<T>.buffered(n)` | Fixed buffer size `n` | Blocks send when full |
| `Channel<T>.rendezvous()` | 0 (no buffer) | Blocks until receiver ready |

## Send Semantics by Channel Type

### Unbounded Channels

```
(tx, rx) = Channel<T>.unbounded()
tx.send(value)?  // Never blocks, may allocate memory
```

| Property | Behavior |
|----------|----------|
| Send blocking | NEVER blocks |
| Memory growth | Unbounded (can exhaust memory) |
| Send error cases | Channel closed only |
| Cost visibility | Allocation visible via `?` (fallible) |

**Send signature:** `fn send(self, value: T) -> Result<(), SendError<T>>`

**Error cases:**
- `Err(SendError::Closed(value))` — channel closed by receiver

### Buffered Channels

```
(tx, rx) = Channel<T>.buffered(100)
tx.send(value)?  // Blocks if buffer full
```

| Property | Behavior |
|----------|----------|
| Send blocking | Blocks when buffer full, unblocks when space available |
| Memory growth | Bounded by capacity |
| Send error cases | Channel closed only (full is blocking, not error) |
| Capacity allocation | Pre-allocated at creation |

**Send signature:** `fn send(self, value: T) -> Result<(), SendError<T>>`

**Blocking behavior:**
- Sync send: OS thread blocks
- Async send: task yields, runtime schedules other tasks

**Error cases:**
- `Err(SendError::Closed(value))` — channel closed while waiting or sending

### Rendezvous Channels

```
(tx, rx) = Channel<T>.rendezvous()
tx.send(value)?  // Blocks until receiver calls recv()
```

| Property | Behavior |
|----------|----------|
| Send blocking | Always blocks until handoff |
| Memory | No buffer allocation |
| Semantics | Synchronous rendezvous point |

**Send signature:** `fn send(self, value: T) -> Result<(), SendError<T>>`

## Receive Semantics (All Channel Types)

| Operation | Blocking | Error Cases |
|-----------|----------|-------------|
| `rx.recv()` | Blocks until value available | `Err(RecvError::Closed)` if closed and empty |
| `rx.try_recv()` | Non-blocking | `Err(RecvError::Empty)` or `Err(RecvError::Closed)` |

**Signatures:**
```
fn recv(self) -> Result<T, RecvError>
fn try_recv(self) -> Result<T, RecvError>
```

**Error cases:**
```
enum RecvError {
    Closed,  // Channel closed and no pending values
    Empty,   // try_recv() only: no value available
}
```

## Backpressure Patterns

### Producer Rate Limiting (Buffered)

```
(tx, rx) = Channel<T>.buffered(100)

// Producer blocks when buffer full
for item in items {
    tx.send(item)?  // Automatically rate-limited by receiver speed
}
```

**Mechanism:** Send blocks create natural backpressure.

### Memory-Bounded Pipeline (Buffered)

```
nursery { |n|
    (tx, rx) = Channel<Item>.buffered(1000)

    n.spawn(rx) { |rx|
        for item in receive_all(rx) {
            process(item)  // Processing speed controls producer
        }
    }

    for item in load_large_dataset() {
        tx.send(item)?  // Blocks when 1000 items buffered
    }
}
```

**Cost visibility:** Buffer size explicit in `buffered(1000)` call.

### High-Throughput (Unbounded)

```
(tx, rx) = Channel<T>.unbounded()

// No backpressure - monitor memory externally
for item in items {
    tx.send(item)?  // Never blocks, may OOM
}
```

**When to use:** Producer must not block (e.g., signal handlers, critical sections).

**Safety:** Programmer responsibility to monitor memory or bound producer rate.

## Edge Cases

| Case | Handling |
|------|----------|
| Send to closed buffered channel | `Err(SendError::Closed(value))`, returns value |
| Receive from closed empty channel | `Err(RecvError::Closed)` |
| try_recv on empty open channel | `Err(RecvError::Empty)` |
| Buffered channel capacity 0 | Same as rendezvous |
| Unbounded send failure (OOM) | `Err(SendError::OutOfMemory(value))` |
| Concurrent sends to buffered channel | All senders may block; fairness implementation-defined |
| Drop sender while blocked send | Send unblocks with `Err(Closed(value))` |
| Drop receiver while blocked send | Send unblocks with `Err(Closed(value))` |

## Integration with Existing Spec

**Affine types:** All channel constructors return `(Sender<T>, Receiver<T>)` tuple. Both are affine.

**Shared channels:** Call `.share()` on either endpoint to convert to refcounted version:
```
tx.share() -> SharedSender<T>   // Supports clone()
rx.share() -> SharedReceiver<T> // Supports clone()
```

**Linear resources:** Cannot send linear types on any channel (existing rule preserved).

## Cost Transparency

| Operation | Visible Cost |
|-----------|--------------|
| `.unbounded()` | Name implies unbounded growth |
| `.buffered(n)` | Capacity explicit in argument |
| `.rendezvous()` | Name implies blocking synchronization |
| `send()` on unbounded | May allocate (fallible) |
| `send()` on buffered | May block (function property) |
| `recv()` on all types | May block (function property) |

All costs visible: capacity at creation, blocking in function semantics, allocation via fallible return.
