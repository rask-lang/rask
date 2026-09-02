// SPDX-License-Identifier: (MIT OR Apache-2.0)
// 128-bit integer operations Cranelift can't lower.
//
// Cranelift's `I128` covers add, sub and mul on x64, and `sadd_overflow` /
// `uadd_overflow` and their sub counterparts have lowering rules — so checked
// `+` and `-` are free. Three things aren't there: `smul_overflow`/`umul_overflow`
// have no rule (the verifier rejects them outright at `I128`), and `sdiv`/`udiv`/
// `srem`/`urem` have no rule either. Those come through here (#762).
//
// Every entry returns a status rather than trapping, so the panic stays on the
// Rask side where the span is: the caller branches on it into the same overflow
// and divide-by-zero panics the narrower widths use (type.overflow OV1–OV4).

#include "rask_runtime.h"

#include <stdio.h>

typedef RaskI128 rask_i128;
typedef RaskU128 rask_u128;

#define RASK_I128_OK 0
#define RASK_I128_DIV_ZERO 1
#define RASK_I128_OVERFLOW 2

int32_t rask_i128_mul(rask_i128 a, rask_i128 b, rask_i128 *out) {
    if (__builtin_mul_overflow(a, b, out)) {
        return RASK_I128_OVERFLOW;
    }
    return RASK_I128_OK;
}

int32_t rask_u128_mul(rask_u128 a, rask_u128 b, rask_u128 *out) {
    if (__builtin_mul_overflow(a, b, out)) {
        return RASK_I128_OVERFLOW;
    }
    return RASK_I128_OK;
}

// The one signed division that overflows: the most negative value over -1 has
// no positive counterpart to land on.
static int i128_div_guard(rask_i128 a, rask_i128 b) {
    if (b == 0) {
        return RASK_I128_DIV_ZERO;
    }
    if (b == -1 && a == (rask_i128)1 << 127) {
        return RASK_I128_OVERFLOW;
    }
    return RASK_I128_OK;
}

int32_t rask_i128_div(rask_i128 a, rask_i128 b, rask_i128 *out) {
    int status = i128_div_guard(a, b);
    if (status != RASK_I128_OK) {
        return (int32_t)status;
    }
    *out = a / b;
    return RASK_I128_OK;
}

int32_t rask_i128_rem(rask_i128 a, rask_i128 b, rask_i128 *out) {
    int status = i128_div_guard(a, b);
    if (status != RASK_I128_OK) {
        return (int32_t)status;
    }
    *out = a % b;
    return RASK_I128_OK;
}

int32_t rask_u128_div(rask_u128 a, rask_u128 b, rask_u128 *out) {
    if (b == 0) {
        return RASK_I128_DIV_ZERO;
    }
    *out = a / b;
    return RASK_I128_OK;
}

int32_t rask_u128_rem(rask_u128 a, rask_u128 b, rask_u128 *out) {
    if (b == 0) {
        return RASK_I128_DIV_ZERO;
    }
    *out = a % b;
    return RASK_I128_OK;
}

// ─── Rendering ───────────────────────────────────────────────
//
// `printf` has no length modifier for `__int128`, so the digits come out by
// repeated division into a buffer written back to front. 39 digits is the
// widest either type reaches (`u128::MAX` is 340282366920938463463374607431768211455),
// plus a sign and a terminator.

#define RASK_I128_DIGITS 44

static char *u128_digits(rask_u128 v, char *end) {
    char *p = end;
    *--p = '\0';
    if (v == 0) {
        *--p = '0';
        return p;
    }
    while (v != 0) {
        *--p = (char)('0' + (int)(v % 10));
        v /= 10;
    }
    return p;
}

void rask_u128_to_string(RaskStr *out, rask_u128 val) {
    char buf[RASK_I128_DIGITS];
    rask_string_from(out, u128_digits(val, buf + RASK_I128_DIGITS));
}

void rask_i128_to_string(RaskStr *out, rask_i128 val) {
    char buf[RASK_I128_DIGITS];
    // Negate in the unsigned domain: `-i128::MIN` has no signed value.
    rask_u128 magnitude = val < 0 ? (rask_u128)0 - (rask_u128)val : (rask_u128)val;
    char *p = u128_digits(magnitude, buf + RASK_I128_DIGITS);
    if (val < 0) {
        *--p = '-';
    }
    rask_string_from(out, p);
}

void rask_print_i128(rask_i128 val) {
    char buf[RASK_I128_DIGITS];
    rask_u128 magnitude = val < 0 ? (rask_u128)0 - (rask_u128)val : (rask_u128)val;
    char *p = u128_digits(magnitude, buf + RASK_I128_DIGITS);
    if (val < 0) {
        *--p = '-';
    }
    fputs(p, stdout);
}

void rask_print_u128(rask_u128 val) {
    char buf[RASK_I128_DIGITS];
    fputs(u128_digits(val, buf + RASK_I128_DIGITS), stdout);
}

// ─── Assertion failures ──────────────────────────────────────
//
// The i64 helper can't take these: narrowing to report the values would print
// exactly the wrong ones, since the values a 128-bit assertion is about are the
// ones that don't fit 64 bits.

void rask_assert_fail_cmp_i128(rask_i128 left, rask_i128 right,
                               const char *op, const char *file,
                               int32_t line, int32_t col) {
    RaskStr ls, rs;
    rask_i128_to_string(&ls, left);
    rask_i128_to_string(&rs, right);
    const char *lbuf = rask_string_ptr(&ls);
    const char *rbuf = rask_string_ptr(&rs);
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assertion failed: %s %s %s (left: %s, right: %s)",
             lbuf, op ? op : "?", rbuf, lbuf, rbuf);
    rask_panic_at(file, line, col, buf);
}

// F3 for the widest ints: the same operand-carrying overflow message the
// narrower widths get. Lives here rather than in runtime.c because printing a
// 128-bit value needs u128_digits — snprintf has no conversion for it.
_Noreturn void rask_panic_overflow_binary_i128(const char *file, int32_t line, int32_t col,
                                              const char *op, const char *tail,
                                              rask_i128 lhs, rask_i128 rhs,
                                              int32_t is_unsigned) {
    RaskStr ls, rs;
    if (is_unsigned) {
        rask_u128_to_string(&ls, (rask_u128)lhs);
        rask_u128_to_string(&rs, (rask_u128)rhs);
    } else {
        rask_i128_to_string(&ls, lhs);
        rask_i128_to_string(&rs, rhs);
    }
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf), "integer overflow: %s %s %s exceeds %s",
             rask_string_ptr(&ls), op ? op : "?", rask_string_ptr(&rs),
             tail ? tail : "range");
    rask_panic_at(file, line, col, buf);
}

_Noreturn void rask_panic_overflow_neg_i128(const char *file, int32_t line, int32_t col,
                                            const char *tail, rask_i128 operand) {
    RaskStr os;
    rask_i128_to_string(&os, operand);
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf), "integer overflow: negating %s exceeds %s",
             rask_string_ptr(&os), tail ? tail : "range");
    rask_panic_at(file, line, col, buf);
}

void rask_assert_fail_cmp_u128(rask_u128 left, rask_u128 right,
                               const char *op, const char *file,
                               int32_t line, int32_t col) {
    RaskStr ls, rs;
    rask_u128_to_string(&ls, left);
    rask_u128_to_string(&rs, right);
    const char *lbuf = rask_string_ptr(&ls);
    const char *rbuf = rask_string_ptr(&rs);
    char buf[RASK_PANIC_MSG_MAX];
    snprintf(buf, sizeof(buf),
             "assertion failed: %s %s %s (left: %s, right: %s)",
             lbuf, op ? op : "?", rbuf, lbuf, rbuf);
    rask_panic_at(file, line, col, buf);
}

// `abs` at 128 bits. `llabs` can't stand in: it takes a `long long`, so the
// value would be truncated before it ever got negated (#762).
//
// `i128::MIN` has no positive counterpart, same as every other width — the
// checked path panics there rather than handing back the negative value C's
// `abs` family returns.
rask_i128 rask_i128_abs(rask_i128 v) {
    if (v == (rask_i128)1 << 127) {
        rask_panic("integer overflow in abs");
    }
    return v < 0 ? -v : v;
}

void rask_eprint_i128(rask_i128 val) {
    char buf[RASK_I128_DIGITS];
    rask_u128 magnitude = val < 0 ? (rask_u128)0 - (rask_u128)val : (rask_u128)val;
    char *p = u128_digits(magnitude, buf + RASK_I128_DIGITS);
    if (val < 0) {
        *--p = '-';
    }
    fputs(p, stderr);
}

void rask_eprint_u128(rask_u128 val) {
    char buf[RASK_I128_DIGITS];
    fputs(u128_digits(val, buf + RASK_I128_DIGITS), stderr);
}
