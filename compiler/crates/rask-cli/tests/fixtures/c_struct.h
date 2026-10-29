/* SPDX-License-Identifier: (MIT OR Apache-2.0) */
/* Structs crossing the C ABI by value — one per System V class, so a
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
Rect rask_test_make(int w, int h);
