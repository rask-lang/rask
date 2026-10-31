#ifndef C_CONSTRUCTS_H
#define C_CONSTRUCTS_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#define CC_VERSION 3
#define CC_NAME "constructs"
#define CC_FLAG_A 0x01
#define CC_FLAG_B (1 << 1)
#define CC_FLAGS (CC_FLAG_A | CC_FLAG_B)
#define CC_WIDE ((uint32_t)0x80000000)
#define CC_MAX(a, b) ((a) > (b) ? (a) : (b))

typedef struct { int32_t x; int32_t y; } Point;
typedef struct Pair { Point a; Point b; uint8_t tag; double w; } Pair;
typedef struct Opaque Opaque;
typedef union { int32_t i; float f; } Bits;
typedef enum { RED = 1, GREEN = RED << 2, BLUE } Color;
typedef struct { int32_t vals[CC_VERSION + 1]; const char *name; } Table;
typedef struct { unsigned flag : 1; unsigned kind : 3; signed delta : 5; unsigned short tail; } Packed;

int32_t take_point(Point p);
int32_t pair_sum(const Pair *p);
Opaque *opaque_new(int32_t v);
int32_t opaque_get(const Opaque *o);
void opaque_free(Opaque *o);
int32_t bits_int(Bits b);
size_t sizeof_bits(void);
int color_val(Color c);
bool is_even(int32_t n);
int table_sum(const Table *t);
int packed_kind(Packed p);
int packed_delta(Packed p);
void packed_fill(Packed *out);
size_t sizeof_packed(void);
static inline int cc_twice(int n) { return n * 2; }
static inline void cc_scale(Point *p, int k) { p->x *= k; p->y *= k; }
#endif
