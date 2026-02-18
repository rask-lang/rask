// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Rask SIMD runtime — scalar fallback implementations.
// Each SIMD vector is a heap-allocated array of elements, passed as i64 (pointer).
// Float operations use double for ABI compatibility (Cranelift passes F64).

#include "rask_runtime.h"

#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <float.h>
#include <math.h>

// ═══════════════════════════════════════════════════════════
// f32x4 — 4-lane single-precision float vector
// ═══════════════════════════════════════════════════════════

int64_t rask_simd_f32x4_splat(double val) {
    float *v = rask_alloc(4 * sizeof(float));
    float fv = (float)val;
    for (int i = 0; i < 4; i++) v[i] = fv;
    return (int64_t)(uintptr_t)v;
}

int64_t rask_simd_f32x4_load(int64_t src) {
    float *v = rask_alloc(4 * sizeof(float));
    memcpy(v, (void *)(uintptr_t)src, 4 * sizeof(float));
    return (int64_t)(uintptr_t)v;
}

void rask_simd_f32x4_store(int64_t vec, int64_t dst) {
    memcpy((void *)(uintptr_t)dst, (void *)(uintptr_t)vec, 4 * sizeof(float));
}

int64_t rask_simd_f32x4_add(int64_t a, int64_t b) {
    float *va = (float *)(uintptr_t)a, *vb = (float *)(uintptr_t)b;
    float *r = rask_alloc(4 * sizeof(float));
    for (int i = 0; i < 4; i++) r[i] = va[i] + vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f32x4_sub(int64_t a, int64_t b) {
    float *va = (float *)(uintptr_t)a, *vb = (float *)(uintptr_t)b;
    float *r = rask_alloc(4 * sizeof(float));
    for (int i = 0; i < 4; i++) r[i] = va[i] - vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f32x4_mul(int64_t a, int64_t b) {
    float *va = (float *)(uintptr_t)a, *vb = (float *)(uintptr_t)b;
    float *r = rask_alloc(4 * sizeof(float));
    for (int i = 0; i < 4; i++) r[i] = va[i] * vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f32x4_div(int64_t a, int64_t b) {
    float *va = (float *)(uintptr_t)a, *vb = (float *)(uintptr_t)b;
    float *r = rask_alloc(4 * sizeof(float));
    for (int i = 0; i < 4; i++) r[i] = va[i] / vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f32x4_scale(int64_t a, double scalar) {
    float *va = (float *)(uintptr_t)a;
    float *r = rask_alloc(4 * sizeof(float));
    float s = (float)scalar;
    for (int i = 0; i < 4; i++) r[i] = va[i] * s;
    return (int64_t)(uintptr_t)r;
}

double rask_simd_f32x4_sum(int64_t a) {
    float *va = (float *)(uintptr_t)a;
    float s = 0;
    for (int i = 0; i < 4; i++) s += va[i];
    return (double)s;
}

double rask_simd_f32x4_product(int64_t a) {
    float *va = (float *)(uintptr_t)a;
    float s = 1;
    for (int i = 0; i < 4; i++) s *= va[i];
    return (double)s;
}

double rask_simd_f32x4_min(int64_t a) {
    float *va = (float *)(uintptr_t)a;
    float m = va[0];
    for (int i = 1; i < 4; i++) if (va[i] < m) m = va[i];
    return (double)m;
}

double rask_simd_f32x4_max(int64_t a) {
    float *va = (float *)(uintptr_t)a;
    float m = va[0];
    for (int i = 1; i < 4; i++) if (va[i] > m) m = va[i];
    return (double)m;
}

double rask_simd_f32x4_get(int64_t vec, int64_t index) {
    float *v = (float *)(uintptr_t)vec;
    return (double)v[index];
}

void rask_simd_f32x4_set(int64_t vec, int64_t index, double val) {
    float *v = (float *)(uintptr_t)vec;
    v[index] = (float)val;
}

// ═══════════════════════════════════════════════════════════
// f32x8 — 8-lane single-precision float vector
// ═══════════════════════════════════════════════════════════

int64_t rask_simd_f32x8_splat(double val) {
    float *v = rask_alloc(8 * sizeof(float));
    float fv = (float)val;
    for (int i = 0; i < 8; i++) v[i] = fv;
    return (int64_t)(uintptr_t)v;
}

int64_t rask_simd_f32x8_load(int64_t src) {
    float *v = rask_alloc(8 * sizeof(float));
    memcpy(v, (void *)(uintptr_t)src, 8 * sizeof(float));
    return (int64_t)(uintptr_t)v;
}

void rask_simd_f32x8_store(int64_t vec, int64_t dst) {
    memcpy((void *)(uintptr_t)dst, (void *)(uintptr_t)vec, 8 * sizeof(float));
}

int64_t rask_simd_f32x8_add(int64_t a, int64_t b) {
    float *va = (float *)(uintptr_t)a, *vb = (float *)(uintptr_t)b;
    float *r = rask_alloc(8 * sizeof(float));
    for (int i = 0; i < 8; i++) r[i] = va[i] + vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f32x8_sub(int64_t a, int64_t b) {
    float *va = (float *)(uintptr_t)a, *vb = (float *)(uintptr_t)b;
    float *r = rask_alloc(8 * sizeof(float));
    for (int i = 0; i < 8; i++) r[i] = va[i] - vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f32x8_mul(int64_t a, int64_t b) {
    float *va = (float *)(uintptr_t)a, *vb = (float *)(uintptr_t)b;
    float *r = rask_alloc(8 * sizeof(float));
    for (int i = 0; i < 8; i++) r[i] = va[i] * vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f32x8_div(int64_t a, int64_t b) {
    float *va = (float *)(uintptr_t)a, *vb = (float *)(uintptr_t)b;
    float *r = rask_alloc(8 * sizeof(float));
    for (int i = 0; i < 8; i++) r[i] = va[i] / vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f32x8_scale(int64_t a, double scalar) {
    float *va = (float *)(uintptr_t)a;
    float *r = rask_alloc(8 * sizeof(float));
    float s = (float)scalar;
    for (int i = 0; i < 8; i++) r[i] = va[i] * s;
    return (int64_t)(uintptr_t)r;
}

double rask_simd_f32x8_sum(int64_t a) {
    float *va = (float *)(uintptr_t)a;
    float s = 0;
    for (int i = 0; i < 8; i++) s += va[i];
    return (double)s;
}

double rask_simd_f32x8_product(int64_t a) {
    float *va = (float *)(uintptr_t)a;
    float s = 1;
    for (int i = 0; i < 8; i++) s *= va[i];
    return (double)s;
}

double rask_simd_f32x8_min(int64_t a) {
    float *va = (float *)(uintptr_t)a;
    float m = va[0];
    for (int i = 1; i < 8; i++) if (va[i] < m) m = va[i];
    return (double)m;
}

double rask_simd_f32x8_max(int64_t a) {
    float *va = (float *)(uintptr_t)a;
    float m = va[0];
    for (int i = 1; i < 8; i++) if (va[i] > m) m = va[i];
    return (double)m;
}

double rask_simd_f32x8_get(int64_t vec, int64_t index) {
    float *v = (float *)(uintptr_t)vec;
    return (double)v[index];
}

void rask_simd_f32x8_set(int64_t vec, int64_t index, double val) {
    float *v = (float *)(uintptr_t)vec;
    v[index] = (float)val;
}

// ═══════════════════════════════════════════════════════════
// f64x2 — 2-lane double-precision float vector
// ═══════════════════════════════════════════════════════════

int64_t rask_simd_f64x2_splat(double val) {
    double *v = rask_alloc(2 * sizeof(double));
    for (int i = 0; i < 2; i++) v[i] = val;
    return (int64_t)(uintptr_t)v;
}

int64_t rask_simd_f64x2_load(int64_t src) {
    double *v = rask_alloc(2 * sizeof(double));
    memcpy(v, (void *)(uintptr_t)src, 2 * sizeof(double));
    return (int64_t)(uintptr_t)v;
}

void rask_simd_f64x2_store(int64_t vec, int64_t dst) {
    memcpy((void *)(uintptr_t)dst, (void *)(uintptr_t)vec, 2 * sizeof(double));
}

int64_t rask_simd_f64x2_add(int64_t a, int64_t b) {
    double *va = (double *)(uintptr_t)a, *vb = (double *)(uintptr_t)b;
    double *r = rask_alloc(2 * sizeof(double));
    for (int i = 0; i < 2; i++) r[i] = va[i] + vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f64x2_sub(int64_t a, int64_t b) {
    double *va = (double *)(uintptr_t)a, *vb = (double *)(uintptr_t)b;
    double *r = rask_alloc(2 * sizeof(double));
    for (int i = 0; i < 2; i++) r[i] = va[i] - vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f64x2_mul(int64_t a, int64_t b) {
    double *va = (double *)(uintptr_t)a, *vb = (double *)(uintptr_t)b;
    double *r = rask_alloc(2 * sizeof(double));
    for (int i = 0; i < 2; i++) r[i] = va[i] * vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f64x2_div(int64_t a, int64_t b) {
    double *va = (double *)(uintptr_t)a, *vb = (double *)(uintptr_t)b;
    double *r = rask_alloc(2 * sizeof(double));
    for (int i = 0; i < 2; i++) r[i] = va[i] / vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f64x2_scale(int64_t a, double scalar) {
    double *va = (double *)(uintptr_t)a;
    double *r = rask_alloc(2 * sizeof(double));
    for (int i = 0; i < 2; i++) r[i] = va[i] * scalar;
    return (int64_t)(uintptr_t)r;
}

double rask_simd_f64x2_sum(int64_t a) {
    double *va = (double *)(uintptr_t)a;
    double s = 0;
    for (int i = 0; i < 2; i++) s += va[i];
    return s;
}

double rask_simd_f64x2_product(int64_t a) {
    double *va = (double *)(uintptr_t)a;
    double s = 1;
    for (int i = 0; i < 2; i++) s *= va[i];
    return s;
}

double rask_simd_f64x2_min(int64_t a) {
    double *va = (double *)(uintptr_t)a;
    double m = va[0];
    for (int i = 1; i < 2; i++) if (va[i] < m) m = va[i];
    return m;
}

double rask_simd_f64x2_max(int64_t a) {
    double *va = (double *)(uintptr_t)a;
    double m = va[0];
    for (int i = 1; i < 2; i++) if (va[i] > m) m = va[i];
    return m;
}

double rask_simd_f64x2_get(int64_t vec, int64_t index) {
    double *v = (double *)(uintptr_t)vec;
    return v[index];
}

void rask_simd_f64x2_set(int64_t vec, int64_t index, double val) {
    double *v = (double *)(uintptr_t)vec;
    v[index] = val;
}

// ═══════════════════════════════════════════════════════════
// f64x4 — 4-lane double-precision float vector
// ═══════════════════════════════════════════════════════════

int64_t rask_simd_f64x4_splat(double val) {
    double *v = rask_alloc(4 * sizeof(double));
    for (int i = 0; i < 4; i++) v[i] = val;
    return (int64_t)(uintptr_t)v;
}

int64_t rask_simd_f64x4_load(int64_t src) {
    double *v = rask_alloc(4 * sizeof(double));
    memcpy(v, (void *)(uintptr_t)src, 4 * sizeof(double));
    return (int64_t)(uintptr_t)v;
}

void rask_simd_f64x4_store(int64_t vec, int64_t dst) {
    memcpy((void *)(uintptr_t)dst, (void *)(uintptr_t)vec, 4 * sizeof(double));
}

int64_t rask_simd_f64x4_add(int64_t a, int64_t b) {
    double *va = (double *)(uintptr_t)a, *vb = (double *)(uintptr_t)b;
    double *r = rask_alloc(4 * sizeof(double));
    for (int i = 0; i < 4; i++) r[i] = va[i] + vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f64x4_sub(int64_t a, int64_t b) {
    double *va = (double *)(uintptr_t)a, *vb = (double *)(uintptr_t)b;
    double *r = rask_alloc(4 * sizeof(double));
    for (int i = 0; i < 4; i++) r[i] = va[i] - vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f64x4_mul(int64_t a, int64_t b) {
    double *va = (double *)(uintptr_t)a, *vb = (double *)(uintptr_t)b;
    double *r = rask_alloc(4 * sizeof(double));
    for (int i = 0; i < 4; i++) r[i] = va[i] * vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f64x4_div(int64_t a, int64_t b) {
    double *va = (double *)(uintptr_t)a, *vb = (double *)(uintptr_t)b;
    double *r = rask_alloc(4 * sizeof(double));
    for (int i = 0; i < 4; i++) r[i] = va[i] / vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_f64x4_scale(int64_t a, double scalar) {
    double *va = (double *)(uintptr_t)a;
    double *r = rask_alloc(4 * sizeof(double));
    for (int i = 0; i < 4; i++) r[i] = va[i] * scalar;
    return (int64_t)(uintptr_t)r;
}

double rask_simd_f64x4_sum(int64_t a) {
    double *va = (double *)(uintptr_t)a;
    double s = 0;
    for (int i = 0; i < 4; i++) s += va[i];
    return s;
}

double rask_simd_f64x4_product(int64_t a) {
    double *va = (double *)(uintptr_t)a;
    double s = 1;
    for (int i = 0; i < 4; i++) s *= va[i];
    return s;
}

double rask_simd_f64x4_min(int64_t a) {
    double *va = (double *)(uintptr_t)a;
    double m = va[0];
    for (int i = 1; i < 4; i++) if (va[i] < m) m = va[i];
    return m;
}

double rask_simd_f64x4_max(int64_t a) {
    double *va = (double *)(uintptr_t)a;
    double m = va[0];
    for (int i = 1; i < 4; i++) if (va[i] > m) m = va[i];
    return m;
}

double rask_simd_f64x4_get(int64_t vec, int64_t index) {
    double *v = (double *)(uintptr_t)vec;
    return v[index];
}

void rask_simd_f64x4_set(int64_t vec, int64_t index, double val) {
    double *v = (double *)(uintptr_t)vec;
    v[index] = val;
}

// ═══════════════════════════════════════════════════════════
// i32x4 — 4-lane 32-bit integer vector
// ═══════════════════════════════════════════════════════════

int64_t rask_simd_i32x4_splat(int64_t val) {
    int32_t *v = rask_alloc(4 * sizeof(int32_t));
    int32_t iv = (int32_t)val;
    for (int i = 0; i < 4; i++) v[i] = iv;
    return (int64_t)(uintptr_t)v;
}

int64_t rask_simd_i32x4_load(int64_t src) {
    int32_t *v = rask_alloc(4 * sizeof(int32_t));
    memcpy(v, (void *)(uintptr_t)src, 4 * sizeof(int32_t));
    return (int64_t)(uintptr_t)v;
}

void rask_simd_i32x4_store(int64_t vec, int64_t dst) {
    memcpy((void *)(uintptr_t)dst, (void *)(uintptr_t)vec, 4 * sizeof(int32_t));
}

int64_t rask_simd_i32x4_add(int64_t a, int64_t b) {
    int32_t *va = (int32_t *)(uintptr_t)a, *vb = (int32_t *)(uintptr_t)b;
    int32_t *r = rask_alloc(4 * sizeof(int32_t));
    for (int i = 0; i < 4; i++) r[i] = va[i] + vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x4_sub(int64_t a, int64_t b) {
    int32_t *va = (int32_t *)(uintptr_t)a, *vb = (int32_t *)(uintptr_t)b;
    int32_t *r = rask_alloc(4 * sizeof(int32_t));
    for (int i = 0; i < 4; i++) r[i] = va[i] - vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x4_mul(int64_t a, int64_t b) {
    int32_t *va = (int32_t *)(uintptr_t)a, *vb = (int32_t *)(uintptr_t)b;
    int32_t *r = rask_alloc(4 * sizeof(int32_t));
    for (int i = 0; i < 4; i++) r[i] = va[i] * vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x4_div(int64_t a, int64_t b) {
    int32_t *va = (int32_t *)(uintptr_t)a, *vb = (int32_t *)(uintptr_t)b;
    int32_t *r = rask_alloc(4 * sizeof(int32_t));
    for (int i = 0; i < 4; i++) r[i] = va[i] / vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x4_scale(int64_t a, int64_t scalar) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int32_t *r = rask_alloc(4 * sizeof(int32_t));
    int32_t s = (int32_t)scalar;
    for (int i = 0; i < 4; i++) r[i] = va[i] * s;
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x4_sum(int64_t a) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int64_t s = 0;
    for (int i = 0; i < 4; i++) s += va[i];
    return s;
}

int64_t rask_simd_i32x4_product(int64_t a) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int64_t s = 1;
    for (int i = 0; i < 4; i++) s *= va[i];
    return s;
}

int64_t rask_simd_i32x4_min(int64_t a) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int32_t m = va[0];
    for (int i = 1; i < 4; i++) if (va[i] < m) m = va[i];
    return (int64_t)m;
}

int64_t rask_simd_i32x4_max(int64_t a) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int32_t m = va[0];
    for (int i = 1; i < 4; i++) if (va[i] > m) m = va[i];
    return (int64_t)m;
}

int64_t rask_simd_i32x4_get(int64_t vec, int64_t index) {
    int32_t *v = (int32_t *)(uintptr_t)vec;
    return (int64_t)v[index];
}

void rask_simd_i32x4_set(int64_t vec, int64_t index, int64_t val) {
    int32_t *v = (int32_t *)(uintptr_t)vec;
    v[index] = (int32_t)val;
}

// ═══════════════════════════════════════════════════════════
// i32x8 — 8-lane 32-bit integer vector
// ═══════════════════════════════════════════════════════════

int64_t rask_simd_i32x8_splat(int64_t val) {
    int32_t *v = rask_alloc(8 * sizeof(int32_t));
    int32_t iv = (int32_t)val;
    for (int i = 0; i < 8; i++) v[i] = iv;
    return (int64_t)(uintptr_t)v;
}

int64_t rask_simd_i32x8_load(int64_t src) {
    int32_t *v = rask_alloc(8 * sizeof(int32_t));
    memcpy(v, (void *)(uintptr_t)src, 8 * sizeof(int32_t));
    return (int64_t)(uintptr_t)v;
}

void rask_simd_i32x8_store(int64_t vec, int64_t dst) {
    memcpy((void *)(uintptr_t)dst, (void *)(uintptr_t)vec, 8 * sizeof(int32_t));
}

int64_t rask_simd_i32x8_add(int64_t a, int64_t b) {
    int32_t *va = (int32_t *)(uintptr_t)a, *vb = (int32_t *)(uintptr_t)b;
    int32_t *r = rask_alloc(8 * sizeof(int32_t));
    for (int i = 0; i < 8; i++) r[i] = va[i] + vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x8_sub(int64_t a, int64_t b) {
    int32_t *va = (int32_t *)(uintptr_t)a, *vb = (int32_t *)(uintptr_t)b;
    int32_t *r = rask_alloc(8 * sizeof(int32_t));
    for (int i = 0; i < 8; i++) r[i] = va[i] - vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x8_mul(int64_t a, int64_t b) {
    int32_t *va = (int32_t *)(uintptr_t)a, *vb = (int32_t *)(uintptr_t)b;
    int32_t *r = rask_alloc(8 * sizeof(int32_t));
    for (int i = 0; i < 8; i++) r[i] = va[i] * vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x8_div(int64_t a, int64_t b) {
    int32_t *va = (int32_t *)(uintptr_t)a, *vb = (int32_t *)(uintptr_t)b;
    int32_t *r = rask_alloc(8 * sizeof(int32_t));
    for (int i = 0; i < 8; i++) r[i] = va[i] / vb[i];
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x8_scale(int64_t a, int64_t scalar) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int32_t *r = rask_alloc(8 * sizeof(int32_t));
    int32_t s = (int32_t)scalar;
    for (int i = 0; i < 8; i++) r[i] = va[i] * s;
    return (int64_t)(uintptr_t)r;
}

int64_t rask_simd_i32x8_sum(int64_t a) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int64_t s = 0;
    for (int i = 0; i < 8; i++) s += va[i];
    return s;
}

int64_t rask_simd_i32x8_product(int64_t a) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int64_t s = 1;
    for (int i = 0; i < 8; i++) s *= va[i];
    return s;
}

int64_t rask_simd_i32x8_min(int64_t a) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int32_t m = va[0];
    for (int i = 1; i < 8; i++) if (va[i] < m) m = va[i];
    return (int64_t)m;
}

int64_t rask_simd_i32x8_max(int64_t a) {
    int32_t *va = (int32_t *)(uintptr_t)a;
    int32_t m = va[0];
    for (int i = 1; i < 8; i++) if (va[i] > m) m = va[i];
    return (int64_t)m;
}

int64_t rask_simd_i32x8_get(int64_t vec, int64_t index) {
    int32_t *v = (int32_t *)(uintptr_t)vec;
    return (int64_t)v[index];
}

void rask_simd_i32x8_set(int64_t vec, int64_t index, int64_t val) {
    int32_t *v = (int32_t *)(uintptr_t)vec;
    v[index] = (int32_t)val;
}
