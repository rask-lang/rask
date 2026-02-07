# Time — Duration and Instant

Two-type system:
- `Duration` — Time span (8 bytes, Copy) stored as nanoseconds
- `Instant` — Monotonic timestamp (opaque, ≤16 bytes) for measuring intervals

Defer `SystemTime` (wall-clock) to later — not needed for game loops, benchmarks, or timeouts.

## Specification

### Types

| Type | Description | Size | Copy? |
|------|-------------|------|-------|
| `Duration` | Time span in nanoseconds | 8 bytes | Yes |
| `Instant` | Monotonic timestamp | ≤16 bytes | Yes |

Both types are value types with no special cleanup — no `@resource` annotation needed.

### Duration API

**Constructors** (static methods):

```rask
Duration.seconds(n: u64) -> Duration
Duration.millis(n: u64) -> Duration
Duration.micros(n: u64) -> Duration
Duration.nanos(n: u64) -> Duration
Duration.from_secs_f64(secs: f64) -> Duration
```

**Conversions** (instance methods):

```rask
duration.as_secs() -> u64
duration.as_millis() -> u64
duration.as_micros() -> u64
duration.as_nanos() -> u64
duration.as_secs_f32() -> f32
duration.as_secs_f64() -> f64
```

**Precision notes:**
- Input precision: nanosecond (10⁻⁹ seconds)
- Range: 0 to ~584 years
- Rounding: fractional nanoseconds are truncated (e.g., `from_secs_f64(0.5000000001)` → 500000000 ns)

### Instant API

**Static constructor:**

```rask
Instant.now() -> Instant
```

**Instance methods:**

```rask
instant.duration_since(earlier: Instant) -> Duration
instant.elapsed() -> Duration  // equivalent to Instant.now().duration_since(instant)
```

**Monotonic guarantee:**
- `Instant.now()` never goes backward within a process
- Platform-specific epoch (arbitrary, not UNIX epoch)
- Cannot serialize/deserialize — use `SystemTime` when that's needed

### Module Functions

```rask
time.sleep(duration: Duration) -> () or string
```

Blocks the current thread for at least the specified duration. May wake early if interrupted by a signal.

**Error cases:**
- Invalid duration (overflow, negative)
- System sleep error (rare, platform-specific)

Returns `string` error for simplicity — detailed error types TBD when error handling is finalized.

### Access Pattern

```rask
import time

// Duration construction
const d1 = time.Duration.seconds(5)
const d2 = time.Duration.millis(100)

// Instant measurement
const start = time.Instant.now()
expensive_operation()
const elapsed = start.elapsed()

// Sleep
time.sleep(time.Duration.millis(16))
```

Note: `time.Instant` and `time.Duration` are accessed through the module. There is no standalone `Instant` or `Duration` in the global scope.

## Examples

### Game Loop Timing

```rask
import time

func run_game() {
    let last_time = time.Instant.now()
    let accumulator = 0.0f32

    loop {
        const current_time = time.Instant.now()
        const frame_time = current_time.duration_since(last_time).as_secs_f32()
        last_time = current_time

        accumulator += frame_time

        // Fixed timestep physics
        const dt = 0.016f32  // 60 FPS
        while accumulator >= dt {
            update_physics(dt)
            accumulator -= dt
        }

        render()

        // Cap frame rate
        const target_frame = time.Duration.millis(16)
        const frame_duration = last_time.elapsed()
        if frame_duration < target_frame {
            const sleep_time = target_frame.as_nanos() - frame_duration.as_nanos()
            time.sleep(time.Duration.nanos(sleep_time))
        }
    }
}
```

### Benchmark

```rask
import time

func benchmark(f: func()) -> f64 {
    const start = time.Instant.now()
    f()
    const elapsed = start.elapsed()
    elapsed.as_secs_f64()
}

func main() {
    const duration = benchmark(|| {
        let sum = 0
        for i in 0..1_000_000 {
            sum += i
        }
    })
    println("Took {duration} seconds")
}
```

### Timeout Pattern

```rask
import time

func wait_with_timeout(timeout: time.Duration) -> bool or string {
    const start = time.Instant.now()

    loop {
        if check_condition() {
            return true
        }

        if start.elapsed().as_nanos() > timeout.as_nanos() {
            return false  // Timed out
        }

        time.sleep(time.Duration.millis(10))
    }
}

func main() -> () or string {
    const timeout = time.Duration.seconds(5)
    const result = try wait_with_timeout(timeout)

    if result {
        println("Condition met")
    } else {
        println("Timed out")
    }
}
```

### Rate Limiting

```rask
import time

struct RateLimiter {
    interval: time.Duration
    last_tick: time.Instant
}

extend RateLimiter {
    func new(interval: time.Duration) -> RateLimiter {
        RateLimiter {
            interval,
            last_tick: time.Instant.now(),
        }
    }

    func wait(self) {
        const elapsed = self.last_tick.elapsed()
        if elapsed.as_nanos() < self.interval.as_nanos() {
            const remaining = self.interval.as_nanos() - elapsed.as_nanos()
            time.sleep(time.Duration.nanos(remaining))
        }
        self.last_tick = time.Instant.now()
    }
}

func main() {
    let limiter = RateLimiter.new(time.Duration.millis(100))

    for i in 0..10 {
        limiter.wait()
        println("Tick {i}")
    }
}
```

## Implementation Notes

### Platform Mapping

| Platform | Duration | Instant |
|----------|----------|---------|
| POSIX | u64 (nanos) | `clock_gettime(CLOCK_MONOTONIC)` |
| Windows | u64 (nanos) | `QueryPerformanceCounter` |
| WASM | u64 (nanos) | `performance.now()` |

### Precision vs Accuracy

- **Precision**: nanosecond (10⁻⁹ s)
- **Accuracy**: platform-dependent, typically microsecond (10⁻⁶ s) to millisecond (10⁻³ s)
- Use `Duration` for precise API, accept that underlying clock may be coarser

### Future Extensions

Deferred to later:
- `SystemTime` — wall-clock time with epoch (UNIX timestamp, serializable)
- `SystemTime.now() -> SystemTime`
- `system_time.duration_since(UNIX_EPOCH) -> Duration`
- Duration arithmetic: `d1 + d2`, `d1 - d2`, `d1 * n`, `d1 / n`
- Duration comparison: `d1 < d2`, `d1 == d2`
- Instant subtraction: `instant1 - instant2 -> Duration` (alternative to `.duration_since()`)

These can be added when needed without breaking changes.

## References

- CORE_DESIGN.md — Value semantics, copy types ≤16 bytes
- specs/memory/value-semantics.md — Value types (Copy types)
- specs/stdlib/collections.md — Pool pattern (not needed — no references)

## Status

**Specified** — ready for implementation in interpreter and type system.
