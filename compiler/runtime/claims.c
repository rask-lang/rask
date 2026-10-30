// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Claims (type.claims): the choices a claim's generated inputs are built from,
// and the runner that calls a claim once per input.
//
// An input is a list of choices. Sampling reads them from bytes the claim's
// seed produces, shrinking from bytes the CLI cut down, and an exhaustive run
// from the digits of an index. The generated input code never knows which.
// Bytes past the end read as zero, and zero is always the smallest choice, so
// a shorter or lower byte string is a smaller input.

#include "rask_runtime.h"
#include "sim.h"
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// ─── Choices ────────────────────────────────────────────────

#define SAMPLE_BYTES 1024

static uint8_t  ch_bytes[SAMPLE_BYTES];
static size_t   ch_len;
static size_t   ch_pos;
static int      ch_by_index;
static uint64_t ch_index;

static uint8_t next_byte(void) {
    uint8_t b = ch_pos < ch_len ? ch_bytes[ch_pos] : 0;
    ch_pos++;
    return b;
}

uint64_t rask_claim_draw(uint64_t bound) {
    if (bound <= 1) return 0;
    if (ch_by_index) {
        uint64_t digit = ch_index % bound;
        ch_index /= bound;
        return digit;
    }
    uint64_t v = 0;
    for (uint64_t max = bound - 1; max; max >>= 8) {
        v = (v << 8) | next_byte();
    }
    return v % bound;
}

static int64_t zigzag(uint64_t z) {
    return (int64_t)(z >> 1) ^ -(int64_t)(z & 1);
}

static int64_t sign_extend(uint64_t v, int64_t bits) {
    if (bits >= 64) return (int64_t)v;
    uint64_t sign = 1ULL << (bits - 1);
    v &= (sign << 1) - 1;
    return (int64_t)((v ^ sign) - sign);
}

static uint64_t raw_bits(int64_t bits) {
    uint64_t v = 0;
    for (int64_t i = 0; i < bits; i += 8) {
        v = (v << 8) | next_byte();
    }
    return v;
}

// An integer of `bits` width. Mostly small, sometimes anything: a claim about
// `u32` is usually broken first near zero or at the edges.
int64_t rask_claim_int(int64_t bits, int64_t is_signed) {
    if (ch_by_index) {
        uint64_t v = rask_claim_draw(bits >= 64 ? UINT64_MAX : 1ULL << bits);
        return is_signed ? sign_extend(v, bits) : (int64_t)v;
    }
    if (rask_claim_draw(4) < 3) {
        uint64_t z = rask_claim_draw(33);
        return is_signed ? zigzag(z) : (int64_t)z;
    }
    uint64_t v = raw_bits(bits);
    return is_signed ? sign_extend(v, bits) : (int64_t)v;
}

// Finite floats only. NaN and the infinities break every equality law, so a
// claim about floats is written with tolerances and NaN is its own claim.
double rask_claim_float(void) {
    switch (rask_claim_draw(4)) {
    case 0:
        return (double)zigzag(rask_claim_draw(33));
    case 1:
        return (double)zigzag(rask_claim_draw(2001)) / (double)(1 + rask_claim_draw(100));
    case 2: {
        uint64_t bits = raw_bits(64);
        double d;
        memcpy(&d, &bits, sizeof d);
        return isfinite(d) ? d : 0.0;
    }
    default: {
        static const double edges[] = { 0.0, -0.0, 0.5, 1e-9, 1e9, 2.2250738585072014e-308,
                                        1.7976931348623157e308 };
        return edges[rask_claim_draw(sizeof edges / sizeof edges[0])];
    }
    }
}

// How the input prints, written by the generated code as it builds it.
static char  *note_buf;
static size_t note_len;
static size_t note_cap;

void rask_claim_note(const RaskStr *text) {
    int64_t n = rask_string_len(text);
    if (note_len + (size_t)n + 1 > note_cap) {
        size_t cap = note_cap ? note_cap * 2 : 256;
        while (cap < note_len + (size_t)n + 1) cap *= 2;
        char *grown = realloc(note_buf, cap);
        if (!grown) return;
        note_buf = grown;
        note_cap = cap;
    }
    memcpy(note_buf + note_len, rask_string_ptr(text), (size_t)n);
    note_len += (size_t)n;
    note_buf[note_len] = '\0';
}

// The last statement of every claim body calls this (type.claims/CL5): a body
// that returned before it discarded its input.
static int body_done;

void rask_claim_reached_end(void) {
    body_done = 1;
}

// ─── Step budget (type.claims/D2) ───────────────────────────
//
// Counted, never timed, so an input that runs out does so at the same place
// every build. Sim runs one task at a time, so a plain counter is exact.

static int64_t steps;
static int64_t step_budget;
static int     over_budget;

void rask_claim_step(void) {
    if (step_budget > 0 && ++steps > step_budget) {
        // Off before unwinding: `ensure` blocks run instrumented code too.
        step_budget = 0;
        over_budget = 1;
        rask_panic("the input ran past the claim's step budget");
    }
}

// ─── Runner ─────────────────────────────────────────────────

#define DEFAULT_SAMPLES 1000
// An exhaustive run walks index digits, and a sum type's branches leave
// unused digits, so the walk can be longer than the domain it covers.
#define MAX_WALK (1LL << 20)

// Read before sim seals the environment (sim/B4).
static int64_t cfg_samples = DEFAULT_SAMPLES;
static int64_t cfg_shrink_claim = -1;
static int64_t cfg_shrink_input = -1;
static char   *cfg_shrink_path;

__attribute__((constructor)) static void claim_config_take(void) {
    const char *n = getenv("RASK_CLAIM_SAMPLES");
    if (n && *n) cfg_samples = strtoll(n, NULL, 10);
    // `<claim>:<input>:<path>`: run only that claim, on the candidates in
    // <path>, under the sim seed input <input> ran with.
    const char *shrink = getenv("RASK_CLAIM_SHRINK");
    if (shrink && *shrink) {
        char *end;
        cfg_shrink_claim = strtoll(shrink, &end, 10);
        if (*end == ':') cfg_shrink_input = strtoll(end + 1, &end, 10);
        if (*end == ':') cfg_shrink_path = strdup(end + 1);
    }
    unsetenv("RASK_CLAIM_SAMPLES");
    unsetenv("RASK_CLAIM_SHRINK");
}

static uint64_t mix(uint64_t x) {
    x += 0x9e3779b97f4a7c15ULL;
    x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9ULL;
    x = (x ^ (x >> 27)) * 0x94d049bb133111ebULL;
    return x ^ (x >> 31);
}

static uint64_t input_sim_seed(uint64_t claim_seed, int64_t input) {
    return mix(claim_seed ^ mix((uint64_t)input * 2 + 1));
}

static void sample_bytes(uint64_t claim_seed, int64_t input) {
    uint64_t state = mix(claim_seed ^ mix((uint64_t)input * 2));
    for (size_t i = 0; i < SAMPLE_BYTES; i += 8) {
        state = mix(state);
        memcpy(ch_bytes + i, &state, 8);
    }
    ch_len = SAMPLE_BYTES;
}

enum { OUT_PASS, OUT_FAIL, OUT_DISCARD, OUT_BUDGET, OUT_REPEAT };

static const char *outcome_name(int out) {
    switch (out) {
    case OUT_PASS: return "pass";
    case OUT_FAIL: return "fail";
    case OUT_DISCARD: return "discard";
    case OUT_BUDGET: return "budget";
    default: return "repeat";
    }
}

// One input, from choices already in place. `*msg` is malloc'd or NULL.
static int run_input(void (*fn)(void), uint64_t sim_seed, int64_t budget, char **msg) {
    *msg = NULL;
    ch_pos = 0;
    note_len = 0;
    if (note_buf) note_buf[0] = '\0';
    steps = 0;
    step_budget = budget;
    over_budget = 0;
    body_done = 0;
#ifdef RASK_SIM
    if (!rask_sim_reseed(sim_seed)) {
        *msg = strdup("a task the previous input started was still running when this one began; "
                      "a claim has to join every task it spawns");
        return OUT_FAIL;
    }
#else
    (void)sim_seed;
#endif
    int expects_fail = 0;
    int outcome = rask_test_call(fn, msg, &expects_fail);
    step_budget = 0;
    rask_await_detached_tasks();
    if (ch_by_index && ch_index != 0) return OUT_REPEAT;
    if (over_budget) return OUT_BUDGET;
    if (outcome == RASK_TEST_SKIPPED) return OUT_DISCARD;
    if (outcome == RASK_TEST_PASSED && !body_done) return OUT_DISCARD;
    return outcome == RASK_TEST_FAILED ? OUT_FAIL : OUT_PASS;
}

// Every byte the input read, the zeros past the end included: cutting a
// trailing byte off doesn't make an input smaller when it reads it anyway.
static void print_hex_used(void) {
    size_t used = ch_pos < SAMPLE_BYTES ? ch_pos : SAMPLE_BYTES;
    for (size_t i = 0; i < used; i++) printf("%02x", i < ch_len ? ch_bytes[i] : 0);
}

static void print_input_fields(int out, const char *msg) {
    printf("\"result\":\"%s\",\"input\":\"", outcome_name(out));
    rask_test_json_escaped(note_buf ? note_buf : "");
    printf("\",\"error\":\"");
    rask_test_json_escaped(msg ? msg : "");
    printf("\",\"choices\":\"");
    if (!ch_by_index) print_hex_used();
    printf("\"");
}

static int hex_val(char c) {
    if (c >= '0' && c <= '9') return c - '0';
    if (c >= 'a' && c <= 'f') return c - 'a' + 10;
    return -1;
}

// Shrink mode: one record per candidate byte string, one per line in the file.
static int64_t run_candidates(void (*fn)(void), const char *name, uint64_t seed, int64_t budget) {
    FILE *f = fopen(cfg_shrink_path, "r");
    if (!f) {
        fprintf(stderr, "claims: can't read shrink candidates from %s\n", cfg_shrink_path);
        return 1;
    }
    char line[2 * SAMPLE_BYTES + 8];
    int64_t j = 0;
    while (fgets(line, sizeof line, f)) {
        ch_len = 0;
        for (char *p = line; p[0] && p[1] && hex_val(p[0]) >= 0 && hex_val(p[1]) >= 0; p += 2) {
            if (ch_len == SAMPLE_BYTES) break;
            ch_bytes[ch_len++] = (uint8_t)(hex_val(p[0]) * 16 + hex_val(p[1]));
        }
        char *msg;
        int out = run_input(fn, input_sim_seed(seed, cfg_shrink_input), budget, &msg);
        printf("%s{\"claim\":\"", rask_test_record_mark());
        rask_test_json_escaped(name);
        printf("\",\"candidate\":%lld,", (long long)j++);
        print_input_fields(out, msg);
        printf("}\n");
        fflush(stdout);
        free(msg);
    }
    fclose(f);
    return 0;
}

void rask_claim_start(void) {
#ifdef RASK_SIM
    rask_sim_begin(0, 10000000);
#endif
}

int64_t rask_claim_run(void (*fn)(void), const char *name, int64_t seed, int64_t domain,
                       int64_t budget) {
    static int64_t claim_index;
    int64_t me = claim_index++;
    if (cfg_shrink_claim >= 0) {
        if (me != cfg_shrink_claim) return 0;
        ch_by_index = 0;
        return run_candidates(fn, name, (uint64_t)seed, budget);
    }

    int exhaustive = domain > 0 && domain <= MAX_WALK;
    int64_t walk = exhaustive ? domain : cfg_samples;
    int64_t tried = 0, discarded = 0, budget_hits = 0;
    char *budget_input = NULL;
    int failed = 0;

    for (int64_t i = 0; i < walk; i++) {
        if (exhaustive) {
            ch_by_index = 1;
            ch_index = (uint64_t)i;
        } else {
            ch_by_index = 0;
            sample_bytes((uint64_t)seed, i);
        }
        char *msg;
        int out = run_input(fn, input_sim_seed((uint64_t)seed, i), budget, &msg);
        if (out == OUT_REPEAT) {
            free(msg);
            continue;
        }
        tried++;
        if (out == OUT_DISCARD) discarded++;
        if (out == OUT_BUDGET && budget_hits++ == 0 && note_buf) budget_input = strdup(note_buf);
        if (out == OUT_FAIL) {
            printf("%s{\"claim\":\"", rask_test_record_mark());
            rask_test_json_escaped(name);
            printf("\",\"index\":%lld,", (long long)i);
            print_input_fields(out, msg);
            printf("}\n");
            failed = 1;
            free(msg);
            break;
        }
        free(msg);
    }
    ch_by_index = 0;

    printf("%s{\"claim\":\"", rask_test_record_mark());
    rask_test_json_escaped(name);
    printf("\",\"done\":true,\"mode\":\"%s\",\"tried\":%lld,\"discarded\":%lld,"
           "\"over_budget\":%lld,\"budget_input\":\"",
           exhaustive ? "all" : "sampled", (long long)tried, (long long)discarded,
           (long long)budget_hits);
    rask_test_json_escaped(budget_input ? budget_input : "");
    printf("\",\"failed\":%s}\n", failed ? "true" : "false");
    fflush(stdout);
    free(budget_input);
    return failed;
}
