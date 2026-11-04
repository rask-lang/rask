// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask math module.
//
// The names match what MIR mangles for `math.foo(x)` (`math_foo`), so codegen
// resolves them by symbol with no dispatch table in between. The interpreter
// links this same file (rask-interp/build.rs) and calls the same symbols, so
// the two backends can't disagree on `sin(1.0)`.
//
// Every function here that isn't exact goes to CORE-MATH (`core-math/`), never
// to the platform's libm. glibc, musl and Apple's libm differ in the last bit
// of `sin`, `exp` and `pow`, and glibc has changed its own answers between
// releases, so a program calling libm computes different bits on different
// machines. CORE-MATH is correctly rounded: each result is the one double
// nearest the true value, which is unique, so it's the same on every machine
// and in any other correctly rounded implementation (determinism/D12).
//
// What still goes to libm is exact by definition — `sqrt`, `floor`, `ceil`,
// `round`, `trunc`, `fabs` in stdlib/float.rk — where every conforming libm
// returns the same bits.

#include "rask_runtime.h"
#include <fenv.h>
#include <math.h>
#include <string.h>

// M_PI is not in strict c11; the build uses -std=c11.
#define RASK_PI 3.14159265358979323846

double cr_sin(double), cr_cos(double), cr_tan(double);
double cr_asin(double), cr_acos(double), cr_atan(double), cr_atan2(double, double);
double cr_exp(double), cr_log(double), cr_log2(double), cr_log10(double);
double cr_pow(double, double), cr_hypot(double, double);

// The floating-point environment every Rask thread runs in: round to
// nearest, subnormals kept (no flush-to-zero), exceptions masked. That's
// the default everywhere, and the runtime never changes it, but a C library
// linked in can, and a thread inherits whatever its creator had. x86 and
// aarch64 also disagree on where flush-to-zero decides a value is tiny, so
// a program that turned it on would compute different bits on each.
// Called at the top of `main` and of every thread the runtime starts.
void rask_fp_reset(void) {
    fesetenv(FE_DFL_ENV);
}

// Trigonometric
double math_sin(double x) { return cr_sin(x); }
double math_cos(double x) { return cr_cos(x); }
double math_tan(double x) { return cr_tan(x); }
double math_asin(double x) { return cr_asin(x); }
double math_acos(double x) { return cr_acos(x); }
double math_atan(double x) { return cr_atan(x); }
double math_atan2(double y, double x) { return cr_atan2(y, x); }

// Exponential and logarithmic
double math_exp(double x) { return cr_exp(x); }
double math_ln(double x) { return cr_log(x); }
double math_log2(double x) { return cr_log2(x); }
double math_log10(double x) { return cr_log10(x); }

// Powers. An integer exponent is exact as a double, so `powi` is `pow` with
// the exponent converted — the same correctly rounded answer, not the
// repeated multiplication a compiler's `powi` builtin does, whose rounding
// depends on how it chains the products.
double math_pow(double x, double y) { return cr_pow(x, y); }
double math_powi(double x, int32_t n) { return cr_pow(x, (double)n); }

// Multi-argument
double math_hypot(double x, double y) { return cr_hypot(x, y); }

// f64.fract() — the signed fractional part. No libm symbol for it.
double math_fract(double x) { return x - trunc(x); }

double math_clamp(double x, double min, double max) {
    // NaN propagates rather than collapsing to a bound.
    if (isnan(x)) return x;
    if (x < min) return min;
    if (x > max) return max;
    return x;
}

// Conversion
double math_to_radians(double degrees) { return degrees * (RASK_PI / 180.0); }
double math_to_degrees(double radians) { return radians * (180.0 / RASK_PI); }

// Classification — bool is i8 in the Rask ABI.
int8_t math_is_nan(double x) { return isnan(x) ? 1 : 0; }
int8_t math_is_inf(double x) { return isinf(x) ? 1 : 0; }
int8_t math_is_finite(double x) { return isfinite(x) ? 1 : 0; }

// A float's bit pattern at its own width (std.bits/B4): reinterpret, not
// convert, so 1.5 is 0x3FF8000000000000. Every NaN comes out as one quiet NaN
// — a payload is whatever the hardware left, and a program that keys a map on
// the bits or puts them on the wire shouldn't get a different answer per
// machine. `from_bits` folds a NaN pattern the same way.
uint64_t rask_f64_to_bits(double d) {
    if (d != d) return 0x7ff8000000000000ull;
    uint64_t bits;
    memcpy(&bits, &d, sizeof bits);
    return bits;
}

double rask_f64_from_bits(uint64_t bits) {
    double d;
    memcpy(&d, &bits, sizeof d);
    if (d != d) return NAN;
    return d;
}

uint32_t rask_f32_to_bits(float f) {
    if (f != f) return 0x7fc00000u;
    uint32_t bits;
    memcpy(&bits, &f, sizeof bits);
    return bits;
}

float rask_f32_from_bits(uint32_t bits) {
    float f;
    memcpy(&f, &bits, sizeof f);
    if (f != f) return NAN;
    return f;
}
