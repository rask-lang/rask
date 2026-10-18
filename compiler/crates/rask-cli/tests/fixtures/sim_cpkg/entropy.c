// SPDX-License-Identifier: (MIT OR Apache-2.0)
// What a TLS library does at startup: seed itself from the OS and note the
// time. Under sim both answers come from the seed (sim/B7).
#include <stdint.h>
#include <sys/random.h>
#include <time.h>

int64_t entropy_seed(void) {
    uint64_t v = 0;
    if (getrandom(&v, sizeof v, 0) != (ssize_t)sizeof v) return -1;
    return (int64_t)(v >> 1);
}

int64_t entropy_wall_seconds(void) {
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (int64_t)ts.tv_sec;
}
