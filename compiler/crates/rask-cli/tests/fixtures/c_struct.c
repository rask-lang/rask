/* SPDX-License-Identifier: (MIT OR Apache-2.0) */
/* The C side of `c_struct.rk`. Each function combines its fields so that a
   field arriving at the wrong offset — or a struct arriving in the wrong
   register class — produces a different number. */

#include "c_struct.h"

int rask_test_area(Rect r) { return r.width * r.height; }
float rask_test_vec2_sum(Vec2 v) { return v.x + v.y; }
double rask_test_vec2d_sum(Vec2d v) { return v.x + v.y; }
double rask_test_stamp(Stamp s) { return s.t + (double)s.id; }
long rask_test_triple(Triple t) { return t.a + t.b * 10 + t.c * 100; }
int rask_test_rgba(Rgba c) { return c.r + c.g * 10 + c.b * 100 + c.a * 1000; }
int rask_test_mixed(Rect r, int k, Vec2d v) {
    return r.width * r.height + k + (int)(v.x + v.y);
}
Rect rask_test_make(int w, int h) {
    Rect r;
    r.width = w;
    r.height = h;
    return r;
}
