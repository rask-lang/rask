// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask math module — thin wrappers over libm.
//
// The names match what MIR mangles for `math.foo(x)` (`math_foo`), so codegen
// resolves them by symbol with no dispatch table in between.

#include "rask_runtime.h"
#include <math.h>
#include <string.h>

// M_PI is not in strict c11; the build uses -std=c11.
#define RASK_PI 3.14159265358979323846

// Trigonometric
double math_sin(double x) { return sin(x); }
double math_cos(double x) { return cos(x); }
double math_tan(double x) { return tan(x); }
double math_asin(double x) { return asin(x); }
double math_acos(double x) { return acos(x); }
double math_atan(double x) { return atan(x); }
double math_atan2(double y, double x) { return atan2(y, x); }

// Exponential and logarithmic
double math_exp(double x) { return exp(x); }
double math_ln(double x) { return log(x); }
double math_log2(double x) { return log2(x); }
double math_log10(double x) { return log10(x); }

// Multi-argument
double math_hypot(double x, double y) { return hypot(x, y); }

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
