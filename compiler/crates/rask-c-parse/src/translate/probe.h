#ifndef LIB_H
#define LIB_H
#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#define LIB_VERSION 3
#define LIB_NAME "lib"
#define LIB_FLAG_A 0x01
#define LIB_FLAG_B (1 << 1)
#define LIB_MAX(a, b) ((a) > (b) ? (a) : (b))

typedef struct { int32_t x; int32_t y; } Point;
typedef struct Pair { Point a; Point b; uint8_t tag; double w; } Pair;
typedef struct Opaque Opaque;              /* opaque handle */
typedef union { int32_t i; float f; } Bits;
typedef struct { unsigned flag : 1; unsigned kind : 3; } Packed;
typedef struct { int32_t vals[4]; const char *name; } Table;
typedef enum { RED = 1, GREEN = 5, BLUE = 9 } Color;
typedef int (*cmp_fn)(int, int);

int32_t take_point(Point p);
Point make_point(int32_t x, int32_t y);
int32_t pair_sum(const Pair *p);
Opaque *opaque_new(int32_t v);
int32_t opaque_get(const Opaque *o);
void opaque_free(Opaque *o);
int32_t bits_int(Bits b);
int call_cb(cmp_fn f, int a, int b);
int color_val(Color c);
int fill(int32_t *out, size_t n);
int write_flag(int64_t *flag);
size_t name_len(const char *s);
void *raw_alloc(size_t n);
bool is_even(int32_t n);
int table_sum(const Table *t);
int vprint(const char *fmt, ...);
static inline int twice(int n) { return n * 2; }

size_t sizeof_packed(void);
size_t sizeof_bits(void);
int packed_kind(Packed p);
#endif
