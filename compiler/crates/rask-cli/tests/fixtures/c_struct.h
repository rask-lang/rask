/* SPDX-License-Identifier: (MIT OR Apache-2.0) */
/* Structs crossing the C ABI by value, both ways — one per System V class, so a
   misclassification shows up as a wrong number rather than a crash.

   Rect      8 bytes, one integer piece
   Vec2      8 bytes, one SSE piece (two packed floats)
   Vec2d    16 bytes, two SSE pieces
   Stamp    16 bytes, one SSE and one integer piece
   Triple   24 bytes, too big for registers — copied onto the stack
   Rgba      4 bytes, one integer piece
   Seg      16 bytes, two Vec2 — four floats behind two nested structs
   Tri      12 bytes, a float[2] and a float — three floats behind an array */

typedef struct { int width; int height; } Rect;
typedef struct { float x; float y; } Vec2;
typedef struct { double x; double y; } Vec2d;
typedef struct { double t; long id; } Stamp;
typedef struct { long a; long b; long c; } Triple;
typedef struct { unsigned char r, g, b, a; } Rgba;
typedef struct { Vec2 a; Vec2 b; } Seg;
typedef struct { float v[2]; float w; } Tri;

int rask_test_area(Rect r);
float rask_test_vec2_sum(Vec2 v);
double rask_test_vec2d_sum(Vec2d v);
double rask_test_stamp(Stamp s);
long rask_test_triple(Triple t);
int rask_test_rgba(Rgba c);
int rask_test_mixed(Rect r, int k, Vec2d v);
float rask_test_seg(Seg s);
float rask_test_tri(Tri t);
/* The same classes coming back: a returned struct travels by its own rule
   (registers by class, or a hidden pointer for Triple). */
Rect rask_test_make(int w, int h);
Vec2 rask_test_make_vec2(float x, float y);
Vec2d rask_test_make_vec2d(double x, double y);
Stamp rask_test_make_stamp(double t, long id);
Triple rask_test_make_triple(long a, long b, long c);
Rgba rask_test_make_rgba(int base);
Seg rask_test_make_seg(float base);
Tri rask_test_make_tri(float base);

/* C calling Rask: each takes a function the Rask side wrote and calls it
   with a struct, or reads the struct it returns. */
int rask_test_call_area(int (*f)(Rect));
float rask_test_call_vec2(float (*f)(Vec2));
double rask_test_call_stamp(double (*f)(Stamp));
long rask_test_call_triple(long (*f)(Triple));
float rask_test_call_seg(float (*f)(Seg));
int rask_test_call_make(Rect (*f)(int, int));
float rask_test_call_make_vec2(Vec2 (*f)(float, float));
double rask_test_call_make_stamp(Stamp (*f)(double, long));
long rask_test_call_make_triple(Triple (*f)(long, long, long));
float rask_test_call_make_tri(Tri (*f)(float));
