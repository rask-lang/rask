// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Claims (type.claims): building a claim's inputs, and the runner that calls a
// claim once per input.
//
// Each input is built from the shape its type's call site describes (the same
// shapes json.decode reads, value_shape.h), by making a choice wherever the
// type allows more than one value. Sampling reads those choices from bytes the
// claim's seed produces, shrinking from bytes the CLI cut down, and an
// exhaustive run from the digits of an index. Bytes past the end read as
// zero, and zero is always the smallest choice, so a shorter or lower byte
// string is a smaller input.

#include "rask_runtime.h"
#include "sim.h"
#include "value_shape.h"
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

static uint64_t draw(uint64_t bound) {
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
static int64_t draw_int(int64_t bits, int is_signed) {
    if (ch_by_index) {
        uint64_t v = draw(bits >= 64 ? UINT64_MAX : 1ULL << bits);
        return is_signed ? sign_extend(v, bits) : (int64_t)v;
    }
    if (draw(4) < 3) {
        uint64_t z = draw(33);
        return is_signed ? zigzag(z) : (int64_t)z;
    }
    uint64_t v = raw_bits(bits);
    return is_signed ? sign_extend(v, bits) : (int64_t)v;
}

// Finite floats only. NaN and the infinities break every equality law, so a
// claim about floats is written with tolerances and NaN is its own claim.
static double draw_float(void) {
    switch (draw(4)) {
    case 0:
        return (double)zigzag(draw(33));
    case 1:
        return (double)zigzag(draw(2001)) / (double)(1 + draw(100));
    case 2: {
        uint64_t bits = raw_bits(64);
        double d;
        memcpy(&d, &bits, sizeof d);
        return isfinite(d) ? d : 0.0;
    }
    default: {
        static const double edges[] = { 0.0, -0.0, 0.5, 1e-9, 1e9, 2.2250738585072014e-308,
                                        1.7976931348623157e308 };
        return edges[draw(sizeof edges / sizeof edges[0])];
    }
    }
}

// How the inputs print, one `name = value` per input, in the order the claim
// takes them. Lowering renders each through the type's Debug.
static char  *note_buf;
static size_t note_len;
static size_t note_cap;

static void note_append(const char *text, size_t n) {
    if (note_len + n + 1 > note_cap) {
        size_t cap = note_cap ? note_cap * 2 : 256;
        while (cap < note_len + n + 1) cap *= 2;
        char *grown = realloc(note_buf, cap);
        if (!grown) return;
        note_buf = grown;
        note_cap = cap;
    }
    memcpy(note_buf + note_len, text, n);
    note_len += n;
    note_buf[note_len] = '\0';
}

void rask_claim_note_input(const RaskStr *text) {
    if (note_len > 0) note_append(", ", 2);
    note_append(rask_string_ptr(text), (size_t)rask_string_len(text));
}

// ─── Building a value from its shape ────────────────────────

// Past this depth a list, map, string or optional is empty, so a type that
// holds itself through a `Vec` still ends.
#define MAX_DEPTH 8

static int64_t prim_bits(int64_t kind, int *is_signed) {
    *is_signed = kind >= RASK_JSHAPE_I8 && kind <= RASK_JSHAPE_I64;
    switch (kind) {
    case RASK_JSHAPE_I8: case RASK_JSHAPE_U8: return 8;
    case RASK_JSHAPE_I16: case RASK_JSHAPE_U16: return 16;
    case RASK_JSHAPE_I32: case RASK_JSHAPE_U32: return 32;
    default: return 64;
    }
}

static void store_int(void *dst, int64_t width, int64_t v) {
    switch (width) {
    case 1: { int8_t x = (int8_t)v; memcpy(dst, &x, 1); break; }
    case 2: { int16_t x = (int16_t)v; memcpy(dst, &x, 2); break; }
    case 4: { int32_t x = (int32_t)v; memcpy(dst, &x, 4); break; }
    default: memcpy(dst, &v, 8); break;
    }
}

// A few characters that break text handling, more often than random ones would.
static const uint32_t ALPHABET[] = { 'a', 'b', 'Z', '0', ' ', '\n', 0xE9 /* é */, '"' };
#define ALPHABET_LEN (sizeof ALPHABET / sizeof ALPHABET[0])

static uint32_t draw_char(void) {
    return ALPHABET[draw(ALPHABET_LEN)];
}

// Whether a list, map or string takes another element: one in four stops it.
static int another(int depth) {
    return depth < MAX_DEPTH && draw(4) != 0;
}

static void fill(void *dst, RaskJsonShape *shape, int depth);

static void fill_string(void *dst, int depth) {
    char buf[256];
    size_t n = 0;
    while (n + 4 <= sizeof buf && another(depth)) {
        uint32_t c = draw_char();
        if (c < 0x80) {
            buf[n++] = (char)c;
        } else {
            buf[n++] = (char)(0xC0 | (c >> 6));
            buf[n++] = (char)(0x80 | (c & 0x3F));
        }
    }
    RaskStr s;
    rask_string_from_bytes(&s, buf, (int64_t)n);
    memcpy(dst, &s, sizeof s);
}

static void fill_container_elems(RaskJsonShape *shape, int depth, void *container, int is_map) {
    int64_t slot = shape->slot;
    char stack_slot[64];
    char *cell = slot <= (int64_t)sizeof stack_slot ? stack_slot : (char *)rask_alloc(slot);
    while (another(depth)) {
        memset(cell, 0, (size_t)slot);
        if (is_map) {
            // A string key unless the shape names another; those are
            // primitives, at most 16 bytes.
            _Alignas(16) char key[16] = { 0 };
            if (shape->key) {
                fill(key, shape->key, depth + 1);
            } else {
                fill_string(key, depth + 1);
            }
            fill(cell, shape->elem, depth + 1);
            rask_map_insert((RaskMap *)container, key, cell);
        } else {
            fill(cell, shape->elem, depth + 1);
            rask_vec_push((RaskVec *)container, cell);
        }
    }
    if (cell != stack_slot) rask_free(cell);
}

static void fill(void *dst, RaskJsonShape *shape, int depth) {
    if (!shape) return;
    switch (shape->kind) {
    case RASK_JSHAPE_BOOL:
        store_int(dst, 1, (int64_t)draw(2));
        return;
    case RASK_JSHAPE_F32: {
        float f = (float)draw_float();
        memcpy(dst, &f, sizeof f);
        return;
    }
    case RASK_JSHAPE_F64: {
        double d = draw_float();
        memcpy(dst, &d, sizeof d);
        return;
    }
    case RASK_JSHAPE_CHAR: {
        uint32_t c = draw_char();
        memcpy(dst, &c, sizeof c);
        return;
    }
    case RASK_JSHAPE_STRING:
        fill_string(dst, depth);
        return;
    case RASK_JSHAPE_VEC: {
        RaskVec *v = rask_vec_with_capacity(shape->slot, 0, shape->elem_owned,
                                            shape->n_elem_owned, shape->elem_align);
        fill_container_elems(shape, depth, v, 0);
        memcpy(dst, &v, sizeof v);
        return;
    }
    case RASK_JSHAPE_MAP: {
        RaskMap *m = shape->key
            ? rask_map_new(shape->key->slot, shape->slot, NULL, 0, shape->key->slot,
                           shape->elem_owned, shape->n_elem_owned, shape->elem_align)
            : rask_map_new_string_keys(16, shape->slot, rask_elem_strs_one, 1, 0,
                                       shape->elem_owned, shape->n_elem_owned,
                                       shape->elem_align);
        fill_container_elems(shape, depth, m, 1);
        memcpy(dst, &m, sizeof m);
        return;
    }
    case RASK_JSHAPE_OPT: {
        // An exhaustive walk spends one digit on present-or-not; sampling makes
        // `none` one input in four.
        int present = ch_by_index ? draw(2) == 1 : (depth < MAX_DEPTH && draw(4) != 0);
        *(uint8_t *)dst = present ? RASK_TAG_PRESENT : RASK_TAG_ABSENT;
        if (present) fill((char *)dst + shape->payload_offset, shape->elem, depth + 1);
        return;
    }
    case RASK_JSHAPE_STRUCT:
        for (int32_t i = 0; i < shape->nfields; i++) {
            struct RaskJsonField *f = &shape->fields[i];
            // A field the wire form leaves out takes its default, as in a
            // decode: a claim gets the values a decode could build.
            if (f->flags & RASK_JFIELD_NOT_SERIALIZED) {
                if (f->fill) f->fill((char *)dst + f->offset);
                continue;
            }
            fill((char *)dst + f->offset, f->shape, depth);
        }
        return;
    case RASK_JSHAPE_RESULT: {
        uint64_t failed = draw(2);
        *(uint8_t *)dst = failed ? RASK_TAG_ABSENT : RASK_TAG_PRESENT;
        fill((char *)dst + shape->payload_offset, failed ? shape->err : shape->elem, depth + 1);
        return;
    }
    case RASK_JSHAPE_I128:
    case RASK_JSHAPE_U128: {
        int is_signed = shape->kind == RASK_JSHAPE_I128;
        int64_t low = draw_int(64, is_signed);
        int64_t high = is_signed && low < 0 ? -1 : 0;
        // Now and then a value past 64 bits, where a narrowing bug shows.
        if (!ch_by_index && draw(8) == 0) high = (int64_t)raw_bits(64);
        memcpy(dst, &low, 8);
        memcpy((char *)dst + 8, &high, 8);
        return;
    }
    case RASK_JSHAPE_ENUM: {
        if (shape->nvariants == 0) return;
        struct RaskJsonVariant *v = &shape->variants[draw((uint64_t)shape->nvariants)];
        store_int((char *)dst + shape->tag_offset, shape->tag_width, v->tag);
        fill(dst, v->payload, depth);
        return;
    }
    default: {
        int is_signed;
        int64_t bits = prim_bits(shape->kind, &is_signed);
        store_int(dst, bits / 8, draw_int(bits, is_signed));
        return;
    }
    }
}

// How many index values an exhaustive walk of `shape` takes, 0 when it can't be
// walked. `*distinct` is how many different values those cover: a sum spends a
// digit on the branch and as many as its widest branch after it, so the
// narrower branches leave digits unused (claims.c skips those as repeats).
static uint64_t walk_of(RaskJsonShape *shape, uint64_t *distinct) {
    if (!shape) { *distinct = 1; return 1; }
    switch (shape->kind) {
    case RASK_JSHAPE_BOOL: *distinct = 2; return 2;
    case RASK_JSHAPE_I8: case RASK_JSHAPE_U8: *distinct = 256; return 256;
    case RASK_JSHAPE_OPT: {
        uint64_t d, w = walk_of(shape->elem, &d);
        if (!w) return 0;
        *distinct = d + 1;
        return w > UINT64_MAX / 2 ? 0 : 2 * w;
    }
    case RASK_JSHAPE_STRUCT: {
        uint64_t w = 1, n = 1;
        for (int32_t i = 0; i < shape->nfields; i++) {
            uint64_t d, fw = walk_of(shape->fields[i].shape, &d);
            if (!fw || w > UINT64_MAX / fw) return 0;
            w *= fw;
            n *= d;
        }
        *distinct = n;
        return w;
    }
    case RASK_JSHAPE_RESULT: {
        uint64_t dok, dfail;
        uint64_t wok = walk_of(shape->elem, &dok), wfail = walk_of(shape->err, &dfail);
        if (!wok || !wfail) return 0;
        *distinct = dok + dfail;
        uint64_t widest = wok > wfail ? wok : wfail;
        return widest > UINT64_MAX / 2 ? 0 : 2 * widest;
    }
    case RASK_JSHAPE_ENUM: {
        uint64_t widest = 1, n = 0;
        for (int32_t i = 0; i < shape->nvariants; i++) {
            uint64_t d, vw = walk_of(shape->variants[i].payload, &d);
            if (!vw) return 0;
            if (vw > widest) widest = vw;
            n += d;
        }
        *distinct = n;
        if (widest > UINT64_MAX / (uint64_t)shape->nvariants) return 0;
        return widest * (uint64_t)shape->nvariants;
    }
    default:
        return 0;
    }
}

// The input's walk so far: every `claims.input` multiplies its own in.
static uint64_t input_walk;
static uint64_t input_distinct;

// `claims.input<T>(name)`: build one input of the shape `shape` at `dst`.
void rask_claim_fill(void *dst, RaskJsonShape *shape) {
    uint64_t d, w = walk_of(shape, &d);
    if (!w || !input_walk || input_walk > UINT64_MAX / w) {
        input_walk = 0;
    } else {
        input_walk *= w;
        input_distinct *= d;
    }
    fill(dst, shape, 0);
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
static uint64_t cfg_seed_offset;
static char    *cfg_only;   // ",2,5,": run only these claims; NULL runs all

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
    // `--claim-seeds`: the k-th seed past the build's own.
    const char *offset = getenv("RASK_CLAIM_SEED_OFFSET");
    if (offset && *offset) cfg_seed_offset = strtoull(offset, NULL, 10);
    // The claims whose verdicts weren't cached (type.claims/D4), by index.
    const char *only = getenv("RASK_CLAIM_ONLY");
    if (only && *only) {
        size_t n = strlen(only);
        cfg_only = malloc(n + 3);
        if (cfg_only) snprintf(cfg_only, n + 3, ",%s,", only);
    }
    unsetenv("RASK_CLAIM_ONLY");
    unsetenv("RASK_CLAIM_SAMPLES");
    unsetenv("RASK_CLAIM_SHRINK");
    unsetenv("RASK_CLAIM_SEED_OFFSET");
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

// Whether the input just run met another task, the clock or simulated I/O:
// each of those is a step of sim's scheduler.
static int reached_scheduling_point(void) {
#ifdef RASK_SIM
    return rask_sim_step() > 0;
#else
    return 0;
#endif
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

int64_t rask_claim_run(void (*fn)(void), const char *name, int64_t seed, int64_t budget) {
    static int64_t claim_index;
    int64_t me = claim_index++;
    if (cfg_only) {
        char want[32];
        snprintf(want, sizeof want, ",%lld,", (long long)me);
        if (!strstr(cfg_only, want)) return 0;
    }
    if (cfg_seed_offset) seed = (int64_t)mix((uint64_t)seed ^ mix(cfg_seed_offset));
    if (cfg_shrink_claim >= 0) {
        if (me != cfg_shrink_claim) return 0;
        ch_by_index = 0;
        return run_candidates(fn, name, (uint64_t)seed, budget);
    }

    // The first input is the one every choice at zero builds, run as index 0
    // of an exhaustive walk. Building it measures the walk the claim's inputs
    // take; when that's small enough the walk carries on, and otherwise the
    // rest are sampled.
    int exhaustive = 1;
    int64_t walk = 1;
    uint64_t distinct = 1;
    int64_t tried = 0, discarded = 0, budget_hits = 0;
    char *budget_input = NULL;
    int failed = 0;
    int scheduled = 0;

    for (int64_t i = 0; i < walk; i++) {
        if (exhaustive) {
            ch_by_index = 1;
            ch_index = (uint64_t)i;
        } else {
            ch_by_index = 0;
            sample_bytes((uint64_t)seed, i);
        }
        input_walk = 1;
        input_distinct = 1;
        char *msg;
        int out = run_input(fn, input_sim_seed((uint64_t)seed, i), budget, &msg);
        if (i == 0 && !scheduled) {
            exhaustive = input_walk > 0 && input_walk <= MAX_WALK;
            walk = exhaustive ? (int64_t)input_walk : cfg_samples;
            distinct = input_distinct;
        }
        if (out != OUT_FAIL && reached_scheduling_point()) scheduled = 1;
        // Every input under one schedule isn't every input: the schedule and
        // the faults are choices too, and they're sampled (type.claims/V2).
        if (exhaustive && scheduled && out != OUT_FAIL) {
            free(msg);
            exhaustive = 0;
            walk = cfg_samples;
            tried = discarded = budget_hits = 0;
            free(budget_input);
            budget_input = NULL;
            i = -1;
            continue;
        }
        // Another seed changes nothing about a claim walked in full.
        if (cfg_seed_offset && exhaustive) {
            free(msg);
            break;
        }
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
    printf("\",\"done\":true,\"mode\":\"%s\",\"tried\":%lld,\"distinct\":%llu,\"discarded\":%lld,"
           "\"over_budget\":%lld,\"scheduled\":%s,\"budget_input\":\"",
           exhaustive ? "all" : "sampled", (long long)tried, (unsigned long long)distinct,
           (long long)discarded, (long long)budget_hits, scheduled ? "true" : "false");
    rask_test_json_escaped(budget_input ? budget_input : "");
    printf("\",\"failed\":%s}\n", failed ? "true" : "false");
    fflush(stdout);
    free(budget_input);
    return failed;
}
