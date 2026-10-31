#include "c_constructs.h"
#include <stdlib.h>
#include <string.h>
struct Opaque { int32_t v; };
int32_t take_point(Point p) { return p.x * 10 + p.y; }
int32_t pair_sum(const Pair *p) { return p->a.x + p->a.y + p->b.x + p->b.y + p->tag + (int32_t)p->w; }
Opaque *opaque_new(int32_t v) { Opaque *o = malloc(sizeof *o); o->v = v; return o; }
int32_t opaque_get(const Opaque *o) { return o->v; }
void opaque_free(Opaque *o) { free(o); }
int32_t bits_int(Bits b) { return b.i; }
size_t sizeof_bits(void) { return sizeof(Bits); }
int color_val(Color c) { return (int)c; }
bool is_even(int32_t n) { return n % 2 == 0; }
int table_sum(const Table *t) { return t->vals[0] + t->vals[1] + t->vals[2] + t->vals[3] + (int)strlen(t->name); }
int packed_kind(Packed p) { return (int)p.kind; }
int packed_delta(Packed p) { return p.delta; }
void packed_fill(Packed *out) { out->flag = 1; out->kind = 6; out->delta = -7; out->tail = 77; }
size_t sizeof_packed(void) { return sizeof(Packed); }
