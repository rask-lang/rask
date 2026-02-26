<!-- id: std.time -->
<!-- status: decided -->
<!-- summary: Duration, Instant (monotonic), and SystemTime (wall-clock) for time operations -->
<!-- depends: memory/value-semantics.md -->

# Time

Three types: `Duration` for time spans, `Instant` for monotonic timestamps, `SystemTime` for wall-clock time.

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

## SystemTime

| Rule | Description |
|------|-------------|
| **W1: Wall-clock** | `SystemTime` represents a point in wall-clock time. Subject to NTP adjustments — can go backward |
| **W2: UNIX epoch** | Epoch is 1970-01-01 00:00:00 UTC. Serializable (unlike Instant) |
| **W3: Value type** | Copy, <=16 bytes, no `@resource` |

<!-- test: skip -->
```rask
SystemTime.now() -> SystemTime
SystemTime.unix_epoch() -> SystemTime
SystemTime.from_unix_secs(secs: i64) -> SystemTime
SystemTime.from_unix_millis(millis: i64) -> SystemTime
```

<!-- test: skip -->
```rask
extend SystemTime {
    func unix_secs(self) -> i64
    func unix_millis(self) -> i64
    func unix_nanos(self) -> i128
    func duration_since(self, earlier: SystemTime) -> Duration or TimeError
    func elapsed(self) -> Duration or TimeError
}
```

<!-- test: skip -->
```rask
enum TimeError {
    Backwards    // clock went backward (NTP adjustment)
}
```

<!-- test: skip -->
```rask
import time

const now = time.SystemTime.now()
const timestamp = now.unix_secs()         // 1709251200
const millis = now.unix_millis()          // 1709251200000

// Reconstruct from stored timestamp
const restored = time.SystemTime.from_unix_secs(timestamp)

// Duration since epoch
const since_epoch = try now.duration_since(time.SystemTime.unix_epoch())
```

## Arithmetic and Comparison

| Rule | Description |
|------|-------------|
| **A1: Instant shift** | `instant + duration -> Instant`, `instant - duration -> Instant` |
| **A2: Instant difference** | `instant - instant -> Duration` |
| **A3: Duration add/sub** | `duration + duration -> Duration`, `duration - duration -> Duration` |
| **A4: Duration scaling** | `duration * n -> Duration`, `n * duration -> Duration`, `duration / n -> Duration` |
| **A5: Duration ratio** | `duration / duration -> u64` (integer ratio, truncated) |
| **A6: Comparison** | `<`, `<=`, `>`, `>=`, `==` on same-type pairs (Instant, Duration, or SystemTime) |
| **A7: SystemTime shift** | `systemtime + duration -> SystemTime`, `systemtime - duration -> SystemTime` |
| **A8: SystemTime difference** | `systemtime - systemtime -> Duration` (wraps if negative — use `duration_since` for checked) |

Arithmetic is native integer ops on nanosecond values. No overflow checking (wraps).

<!-- test: skip -->
```rask
import time

const start = time.Instant.now()
time.sleep(time.Duration.millis(10))
const end = time.Instant.now()

const elapsed = end - start           // Duration
const later = start + elapsed         // Instant
const d2 = elapsed + elapsed          // Duration
const before = end > start            // true

// Duration scaling
const frame = time.Duration.millis(16)
const half = frame / 2                // 8ms
const triple = frame * 3              // 48ms
const five = 5 * frame                // 80ms
const ratio = triple / frame          // 3

// SystemTime arithmetic
const now = time.SystemTime.now()
const tomorrow = now + time.Duration.seconds(86400)
```

## Edge Cases

| Case | Behavior | Rule |
|------|----------|------|
| `Duration.from_secs_f64(0.5000000001)` | Truncated to 500000000 ns | D5 |
| `Instant` across process restarts | Not comparable — opaque epoch | I2 |
| `Instant` serialization | Not supported — use `SystemTime` | I2 |
| Sleep interrupted by signal | May return early | S1 |
| Duration overflow | Wraps (u64 nanoseconds) | D4 |
| Duration divide by zero | Panic | A4 |
| `SystemTime` before UNIX epoch | Negative `unix_secs()` | W2 |
| `SystemTime` NTP adjustment backward | `duration_since` returns `Err(TimeError.Backwards)` | W1 |
| `SystemTime` serialization | Use `unix_secs()` or `unix_millis()` | W2 |
| `SystemTime` comparison across machines | Only meaningful if clocks are synchronized | W1 |

---

## Appendix (non-normative)

### Rationale

**D1 (Duration as nanos):** Single u64 keeps it Copy and avoids the secs+nanos struct split. 584-year range is sufficient.

**I2 (opaque epoch):** Monotonic clocks have arbitrary epochs. Exposing the epoch invites bugs where people treat Instant as wall-clock time.

**W1 (wall-clock separate from Instant):** Instant is monotonic — always goes forward, perfect for measuring intervals. SystemTime tracks real-world time but can jump (NTP, manual adjustment, leap seconds). Separate types prevent bugs where someone measures a benchmark with wall-clock time or serializes an Instant.

**A4 (Duration scaling):** Scaling is essential for frame timing (`frame_time * frame_count`), timeout adjustment (`base_timeout * retry_count`), and benchmark normalization (`total / iterations`). Deferring this blocked real programs.

### Platform Mapping

| Platform | Duration | Instant | SystemTime |
|----------|----------|---------|------------|
| POSIX | u64 (nanos) | `clock_gettime(CLOCK_MONOTONIC)` | `clock_gettime(CLOCK_REALTIME)` |
| Windows | u64 (nanos) | `QueryPerformanceCounter` | `GetSystemTimePreciseAsFileTime` |
| WASM | u64 (nanos) | `performance.now()` | `Date.now()` |

### Deferred

- Date/time formatting (RFC 3339, ISO 8601)
- Time zone support
- Calendar types (Date, Time, DateTime)
- Leap second handling
- `Timer` / periodic tick

### See Also

- `mem.value-semantics` — Copy types <=16 bytes
- `std.testing` — Benchmarks use Duration/Instant internally
- `std.http` — SystemTime used for HTTP Date headers
- `std.fs` — File timestamps as `u64` (seconds since epoch)
