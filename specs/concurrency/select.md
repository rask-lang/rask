# Select and Multiplex

Waiting on multiple sources simultaneously.

## Overview

Select allows waiting on multiple channel operations or timeouts.

## Syntax

```rask
result = select {
    rx1 -> v: handle_v(v),
    rx2 -> v: handle_v(v),
    tx <- msg: sent(),
    timeout 5.seconds: timed_out(),
}
```

## Arm Types

| Arm Type | Syntax | Semantics |
|----------|--------|-----------|
| Receive | `rx -> v: expr` | Wait for value, bind to `v` |
| Send | `tx <- val: expr` | Wait for send completion |
| Timeout | `timeout duration: expr` | Fire after duration |
| Default | `_: expr` | Non-blocking fallback |

## Semantics

### Selection Policy

When multiple arms are ready simultaneously:

| Construct | Policy | Rationale |
|-----------|--------|-----------|
| `select` | **Random** among ready arms | Prevents starvation |
| `select_priority` | **First-listed** wins | Deterministic, explicit priority |

#### `select` (Default)

The runtime selects **uniformly at random** among all ready arms. This prevents starvation—no arm can be indefinitely skipped if it's always ready.

```rask
select {
    rx1 -> v: handle(v),  // 50% if both ready
    rx2 -> v: handle(v),  // 50% if both ready
}
```

**Guarantee:** If an arm is ready on N consecutive iterations, it fires with probability approaching 1 as N increases.

#### `select_priority` (Opt-in)

When priority or determinism is required:

```rask
select_priority {
    shutdown -> _: return,   // Always checked first
    work -> w: process(w),   // Only if shutdown not ready
}
```

**Semantics:** Arms evaluated in listed order. First ready arm fires.

**Use cases:**
- Control signals that must preempt work
- Graceful shutdown patterns
- Deterministic testing

### Ownership

**Non-selected send arms:** Value returned to caller (not consumed).

```rask
result = select {
    tx1 <- msg: "sent to tx1",
    tx2 <- msg: "sent to tx2",  // msg reused if tx1 selected
}
// If tx1 selected, msg for tx2 arm is NOT consumed
```

**Selected arm:** Ownership transfers as normal.

### Closed Channels

| Scenario | Behavior |
|----------|----------|
| All recv channels closed | Immediate return with `Err(Closed)` |
| Some recv channels closed | Skip closed, wait on others |
| Send channel closed | Arm returns `Err(Closed)` |

## Examples

### Timeout Pattern

```rask
result = select {
    rx -> v: Ok(v),
    timeout 5.seconds: Err(Timeout),
}
```

### Fan-in (Multiple Sources)

```rask
loop {
    select {
        rx1 -> v: process(v),
        rx2 -> v: process(v),
        rx3 -> v: process(v),
    }
}
```

### Try-send with Fallback

```rask
select {
    tx <- msg: log("sent"),
    _: log("channel full, dropping"),
}
```

## Edge Cases

| Case | Handling |
|------|----------|
| Select with 0 arms | Compile error |
| All channels closed | Returns immediately |
| Timeout of 0 | Equivalent to default |
| Multiple timeouts | First to expire fires |

---

## Remaining Issues

### Low Priority

1. **Select macros**
   - Should select be a macro for flexibility?
   - Currently specified as language construct
