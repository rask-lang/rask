// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask time module — Instant and Duration backed by CLOCK_MONOTONIC nanoseconds.

#include "rask_runtime.h"
#include <time.h>

static int64_t clock_monotonic_ns(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (int64_t)ts.tv_sec * 1000000000LL + (int64_t)ts.tv_nsec;
}

// Instant.now() → nanoseconds since some fixed point
int64_t rask_time_Instant_now(void) {
    return clock_monotonic_ns();
}

// instant.elapsed() → Duration (nanoseconds since instant)
int64_t rask_time_Instant_elapsed(int64_t instant_ns) {
    return clock_monotonic_ns() - instant_ns;
}

// instant.duration_since(other) → Duration (nanoseconds)
int64_t rask_time_Instant_duration_since(int64_t self_ns, int64_t other_ns) {
    return self_ns - other_ns;
}

// Duration.from_nanos(n) → identity (duration is already nanoseconds)
int64_t rask_time_Duration_from_nanos(int64_t ns) {
    return ns;
}

// Duration.from_millis(ms) → nanoseconds
int64_t rask_time_Duration_from_millis(int64_t ms) {
    return ms * 1000000LL;
}

// duration.as_nanos() → nanoseconds (identity)
int64_t rask_time_Duration_as_nanos(int64_t duration_ns) {
    return duration_ns;
}

// duration.as_secs() → whole seconds
int64_t rask_time_Duration_as_secs(int64_t duration_ns) {
    return duration_ns / 1000000000LL;
}

// duration.as_secs_f64() → fractional seconds
double rask_time_Duration_as_secs_f64(int64_t duration_ns) {
    return (double)duration_ns / 1000000000.0;
}
