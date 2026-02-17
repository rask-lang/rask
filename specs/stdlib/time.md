<!-- id: std.time -->
<!-- status: decided -->
<!-- summary: Duration (time span) and Instant (monotonic timestamp) for measuring intervals -->
<!-- depends: memory/value-semantics.md -->

# Time

Two types: `Duration` for time spans, `Instant` for monotonic timestamps. Wall-clock time (`SystemTime`) deferred.

## Types

| Rule | Description |
|------|-------------|
| **D1: Duration** | Time span stored as nanoseconds. 8 bytes, Copy |
| **D2: Instant** | Monotonic timestamp, opaque, <=16 bytes, Copy |
| **D3: Value types** | Both are value types — no special cleanup, no `@resource` |

## Duration API

| Rule | Description |
|------|-------------|
| **D4: Nanosecond precision** | Internal storage is nanosecond. Range: 0 to ~584 years |
| **D5: Truncation** | Fractional nanoseconds from `from_secs_f64` are truncated |

<!-- test: skip -->
```rask
// Constructors
Duration.seconds(n: u64) -> Duration
Duration.millis(n: u64) -> Duration
Duration.micros(n: u64) -> Duration
Duration.nanos(n: u64) -> Duration
Duration.from_secs_f64(secs: f64) -> Duration

// Conversions
duration.as_secs() -> u64
duration.as_millis() -> u64
duration.as_micros() -> u64
duration.as_nanos() -> u64
duration.as_secs_f32() -> f32
duration.as_secs_f64() -> f64
```

## Instant API

| Rule | Description |
|------|-------------|
| **I1: Monotonic** | `Instant.now()` never goes backward within a process |
| **I2: Opaque epoch** | Platform-specific epoch — not UNIX, not serializable |

<!-- test: skip -->
```rask
Instant.now() -> Instant
instant.duration_since(earlier: Instant) -> Duration
instant.elapsed() -> Duration
```

## Module Functions

| Rule | Description |
|------|-------------|
| **S1: Sleep** | `time.sleep(duration)` blocks current thread for at least the given duration. May wake early on signal |

<!-- test: skip -->
```rask
time.sleep(duration: Duration) -> () or string
```

<!-- test: skip -->
```rask
import time

const start = time.Instant.now()
expensive_operation()
const elapsed = start.elapsed()

time.sleep(time.Duration.millis(16))
```

## Error Messages

```
ERROR [std.time/S1]: sleep failed
   |
5  |  try time.sleep(duration)
   |      ^^^^^^^^^^^^^^^^^^^^^ system sleep error

WHY: Platform-specific sleep failure (rare).
```

## Arithmetic and Comparison

| Rule | Description |
|------|-------------|
| **A1: Instant shift** | `instant + duration -> Instant`, `instant - duration -> Instant` |
| **A2: Instant difference** | `instant - instant -> Duration` |
| **A3: Duration arithmetic** | `duration + duration -> Duration`, `duration - duration -> Duration` |
| **A4: Comparison** | `<`, `<=`, `>`, `>=`, `==` on same-type pairs (Instant/Instant or Duration/Duration) |

Both types are nanosecond i64 internally — arithmetic is native integer ops. No overflow checking (wraps).

<!-- test: skip -->
```rask
import time

const start = time.Instant.now()
time.sleep(time.Duration.from_millis(10))
const end = time.Instant.now()

const elapsed = end - start           // Duration
const later = start + elapsed         // Instant
const d2 = elapsed + elapsed          // Duration
const before = end > start            // true
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| `Duration.from_secs_f64(0.5000000001)` | Truncated to 500000000 ns | D5 |
| `Instant` across process restarts | Not comparable — opaque epoch | I2 |
| `Instant` serialization | Not supported — use `SystemTime` when available | I2 |
| Sleep interrupted by signal | May return early | S1 |
| Duration overflow | Wraps (u64 nanoseconds) | D4 |

---

## Appendix (non-normative)

### Rationale

**D1 (Duration as nanos):** Single u64 keeps it Copy and avoids the secs+nanos struct split. 584-year range is sufficient.

**I2 (opaque epoch):** Monotonic clocks have arbitrary epochs. Exposing the epoch invites bugs where people treat Instant as wall-clock time.

**SystemTime deferred:** Game loops, benchmarks, and timeouts only need Duration + Instant. Wall-clock time adds complexity (leap seconds, NTP adjustments) that can wait.

### Platform Mapping

| Platform | Duration | Instant |
|----------|----------|---------|
| POSIX | u64 (nanos) | `clock_gettime(CLOCK_MONOTONIC)` |
| Windows | u64 (nanos) | `QueryPerformanceCounter` |
| WASM | u64 (nanos) | `performance.now()` |

### Deferred

- `SystemTime` — wall-clock with UNIX epoch, serializable
- Duration scaling: `d1 * n`, `d1 / n`

### See Also

- `mem.value-semantics` — Copy types <=16 bytes
- `std.testing` — Benchmarks use Duration/Instant internally
