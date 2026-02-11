<!-- id: conc.select -->
<!-- status: decided -->
<!-- summary: Wait on multiple channel operations simultaneously with random or priority selection -->
<!-- depends: concurrency/async.md -->

# Select and Multiplex

Wait on multiple sources simultaneously.

## Arm Types

| Rule | Description |
|------|-------------|
| **A1: Receive** | `rx -> v: expr` — wait for value, bind to `v` |
| **A2: Send** | `tx <- val: expr` — wait for send completion |
| **A3: Default** | `_: expr` — non-blocking fallback |

```rask
result = select {
    rx1 -> v: handle_v(v),
    rx2 -> v: handle_v(v),
    tx <- msg: sent(),
    Timer.after(5.seconds) -> _: timed_out(),
}
```

Timeouts use `Timer.after(duration)` which returns a receiver that fires once — regular receive arm, no special syntax.

## Selection Policy

| Rule | Description |
|------|-------------|
| **P1: Random default** | `select` picks uniformly at random among ready arms — prevents starvation |
| **P2: Priority opt-in** | `select_priority` evaluates arms in listed order — first ready arm fires |
| **P3: Zero arms** | Select with 0 arms is a compile error |

<!-- test: skip -->
```rask
// Random (default) — fair
select {
    rx1 -> v: handle(v),  // 50% if both ready
    rx2 -> v: handle(v),  // 50% if both ready
}

// Priority — deterministic
select_priority {
    shutdown -> _: return,       // Always checked first
    work -> w: process(w),       // Only if shutdown not ready
}
```

## Ownership

| Rule | Description |
|------|-------------|
| **OW1: Selected arm** | Ownership transfers as normal |
| **OW2: Non-selected send** | Value returned to caller (not consumed) |

<!-- test: skip -->
```rask
result = select {
    tx1 <- msg: "sent to tx1",
    tx2 <- msg: "sent to tx2",  // msg reused if tx1 selected
}
```

## Closed Channels

| Rule | Description |
|------|-------------|
| **CL1: All closed** | If all recv channels closed, immediate return with `Err(Closed)` |
| **CL2: Some closed** | Skip closed channels, wait on remaining |
| **CL3: Send closed** | Send arm returns `Err(Closed)` |

## Timer

<!-- test: skip -->
```rask
const rx = Timer.after(5.seconds)
rx.recv()  // Blocks for 5 seconds, returns ()

select {
    work -> w: process(w),
    Timer.after(1.seconds) -> _: check_health(),
}
```

Properties: returns `Receiver<()>`, single-shot (fires once, then closes), cancellable (drop receiver to cancel).

## Error Messages

```
ERROR [conc.select/P3]: select requires at least one arm
   |
5  |  select { }
   |  ^^^^^^^^^ empty select block
```

## Edge Cases

| Case | Rule | Handling |
|------|------|----------|
| Select with 0 arms | P3 | Compile error |
| All channels closed | CL1 | Returns immediately with `Err(Closed)` |
| Timer in select | A1 | Regular receive arm — `Timer.after()` returns `Receiver<()>` |
| Non-selected send value | OW2 | Value returned to caller, not consumed |

---

## Appendix (non-normative)

### Rationale

**P1 (random default):** Deterministic selection (always first-listed) causes starvation — a fast channel starves a slow one. Random selection guarantees fairness: if an arm is ready on N consecutive iterations, it fires with probability approaching 1 as N grows.

**P2 (priority opt-in):** Some patterns need determinism — shutdown signals must preempt work, graceful shutdown patterns need ordered checking. `select_priority` is the explicit opt-in.

### Patterns

**Timeout:**
<!-- test: skip -->
```rask
result = select {
    rx -> v: Ok(v),
    Timer.after(5.seconds) -> _: Err(Timeout),
}
```

**Fan-in:**
<!-- test: skip -->
```rask
loop {
    select {
        rx1 -> v: process(v),
        rx2 -> v: process(v),
        rx3 -> v: process(v),
    }
}
```

**Try-send with fallback:**
<!-- test: skip -->
```rask
select {
    tx <- msg: log("sent"),
    _: log("channel full, dropping"),
}
```

### Open Issues

1. **Select as macro** — Should select be a macro for flexibility? Currently specified as language construct.

### See Also

- `conc.async` — channels, task spawning
- `conc.sync` — synchronization primitives
