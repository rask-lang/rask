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
float rask_test_seg(Seg s) { return s.a.x + s.a.y * 10 + s.b.x * 100 + s.b.y * 1000; }
float rask_test_tri(Tri t) { return t.v[0] + t.v[1] * 10 + t.w * 100; }
Rect rask_test_make(int w, int h) {
    Rect r;
    r.width = w;
    r.height = h;
    return r;
}
Vec2 rask_test_make_vec2(float x, float y) { Vec2 v = { x, y }; return v; }
Vec2d rask_test_make_vec2d(double x, double y) { Vec2d v = { x, y }; return v; }
Stamp rask_test_make_stamp(double t, long id) { Stamp s = { t, id }; return s; }
Triple rask_test_make_triple(long a, long b, long c) { Triple t = { a, b, c }; return t; }
Rgba rask_test_make_rgba(int base) {
    Rgba c = { (unsigned char)base, (unsigned char)(base + 1), (unsigned char)(base + 2), (unsigned char)(base + 3) };
    return c;
}
Seg rask_test_make_seg(float base) {
    Seg s = { { base, base + 1 }, { base + 2, base + 3 } };
    return s;
}
Tri rask_test_make_tri(float base) {
    Tri t = { { base, base + 1 }, base + 2 };
    return t;
}
