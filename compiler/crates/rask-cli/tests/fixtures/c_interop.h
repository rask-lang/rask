/* SPDX-License-Identifier: (MIT OR Apache-2.0) */
/* A header for the C interop tests. Nothing links against it — the point is
   what `import c` makes of the declarations, not what the C side does. */
#include <stddef.h>

int c_sum(const int *xs, size_t n);
long c_total(const long *xs, size_t n);
double c_scale(double x, float factor);
void c_copy(void *dst, const void *src, size_t n);
unsigned char c_first_byte(const unsigned char *p);
