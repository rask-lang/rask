// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask random module — xoshiro256++ PRNG.
// Instance type (Rng) and thread-local module convenience functions.

#include "rask_runtime.h"
#include <stdint.h>
#include <time.h>

struct RaskRng {
    uint64_t s[4];
};

// SplitMix64 seed expansion
static uint64_t splitmix64(uint64_t *z) {
    *z += 0x9e3779b97f4a7c15;
    uint64_t r = *z;
    r = (r ^ (r >> 30)) * 0xbf58476d1ce4e5b9;
    r = (r ^ (r >> 27)) * 0x94d049bb133111eb;
    return r ^ (r >> 31);
}

static void rng_seed(RaskRng *rng, uint64_t seed) {
    uint64_t z = seed;
    for (int i = 0; i < 4; i++) {
        rng->s[i] = splitmix64(&z);
    }
}

// xoshiro256++ core
static uint64_t rng_next_u64(RaskRng *rng) {
    uint64_t *s = rng->s;
    uint64_t sum = s[0] + s[3];
    uint64_t result = ((sum << 23) | (sum >> 41)) + s[0];

    uint64_t t = s[1] << 17;
    s[2] ^= s[0];
    s[3] ^= s[1];
    s[1] ^= s[2];
    s[0] ^= s[3];
    s[2] ^= t;
    s[3] = (s[3] << 45) | (s[3] >> 19);

    return result;
}

// ── Rng instance methods ──────────────────────────────────────

RaskRng *rask_rng_new(void) {
    RaskRng *rng = (RaskRng *)rask_alloc(sizeof(RaskRng));
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    uint64_t seed = (uint64_t)ts.tv_sec * 1000000000ULL + (uint64_t)ts.tv_nsec;
    rng_seed(rng, seed);
    return rng;
}

RaskRng *rask_rng_from_seed(int64_t seed) {
    RaskRng *rng = (RaskRng *)rask_alloc(sizeof(RaskRng));
    rng_seed(rng, (uint64_t)seed);
    return rng;
}

int64_t rask_rng_u64(RaskRng *rng) {
    return (int64_t)rng_next_u64(rng);
}

int64_t rask_rng_i64(RaskRng *rng) {
    return (int64_t)rng_next_u64(rng);
}

double rask_rng_f64(RaskRng *rng) {
    return (double)(rng_next_u64(rng) >> 11) / (double)(1ULL << 53);
}

double rask_rng_f32(RaskRng *rng) {
    return (double)((float)(rng_next_u64(rng) >> 40) / (float)(1U << 24));
}

int64_t rask_rng_bool(RaskRng *rng) {
    return (int64_t)(rng_next_u64(rng) & 1);
}

int64_t rask_rng_range(RaskRng *rng, int64_t lo, int64_t hi) {
    if (lo >= hi) {
        rask_panic_fmt("Rng.range: lo (%lld) >= hi (%lld)", (long long)lo, (long long)hi);
    }
    uint64_t range = (uint64_t)(hi - lo);
    return lo + (int64_t)(rng_next_u64(rng) % range);
}

// ── Module-level convenience functions (thread-local PRNG) ───

static __thread RaskRng *tl_random_rng = NULL;

static RaskRng *get_tl_rng(void) {
    if (!tl_random_rng) {
        tl_random_rng = rask_rng_new();
    }
    return tl_random_rng;
}

double  rask_random_f64(void)                   { return rask_rng_f64(get_tl_rng()); }
double  rask_random_f32(void)                   { return rask_rng_f32(get_tl_rng()); }
int64_t rask_random_i64(void)                   { return rask_rng_i64(get_tl_rng()); }
int64_t rask_random_bool(void)                  { return rask_rng_bool(get_tl_rng()); }
int64_t rask_random_range(int64_t lo, int64_t hi) { return rask_rng_range(get_tl_rng(), lo, hi); }
