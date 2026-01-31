# Select and Multiplex

Waiting on multiple sources simultaneously.

## Overview

Select allows waiting on multiple channel operations or timeouts.

## Syntax

```
result = select {
    case rx1.recv() -> |v| handle_v(v),
    case rx2.recv() -> |v| handle_v(v),
    case tx.send(msg) -> |()| sent(),
    timeout 5.seconds -> timed_out(),
}
```

## Arm Types

| Arm Type | Syntax | Semantics |
|----------|--------|-----------|
| Receive | `case rx.recv() -> \|v\| expr` | Wait for value, bind to `v` |
| Send | `case tx.send(val) -> \|_\| expr` | Wait for send completion |
| Timeout | `timeout duration -> expr` | Fire after duration |
| Default | `default -> expr` | Non-blocking fallback |

## Semantics

### Selection

When multiple arms are ready simultaneously:

| Policy | Behavior |
|--------|----------|
| **First-ready** | Select first arm that becomes ready |
| Random | Select randomly among ready arms |
| Priority | Select in listed order |

**Current status:** Unspecified. Recommendation: First-ready with implementation-defined tie-breaking.

### Ownership

**Non-selected send arms:** Value returned to caller (not consumed).

```
result = select {
    case tx1.send(msg) -> |()| "sent to tx1",
    case tx2.send(msg) -> |()| "sent to tx2",  // msg reused if tx1 selected
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

```
result = select {
    case rx.recv() -> |v| Ok(v),
    timeout 5.seconds -> Err(Timeout),
}
```

### Fan-in (Multiple Sources)

```
loop {
    select {
        case rx1.recv() -> |v| process(v),
        case rx2.recv() -> |v| process(v),
        case rx3.recv() -> |v| process(v),
    }
}
```

### Try-send with Fallback

```
select {
    case tx.send(msg) -> |()| log("sent"),
    default -> log("channel full, dropping"),
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

### High Priority

1. **Arm evaluation order**
   - When multiple arms ready, which fires?
   - Random? First-listed? Implementation-defined?

2. **Async select**
   - Does select work in async context?
   - Same syntax? Different primitive?

### Medium Priority

3. **Select on async operations**
   - Can you select on `.await` instead of channel ops?
   - `case fetch_user(1).await -> |u| ...`

4. **Biased select**
   - Syntax for priority-based selection?
   - `select_biased { ... }`

### Low Priority

5. **Select macros**
   - Should select be a macro for flexibility?
   - Currently specified as language construct
