/* SPDX-License-Identifier: (MIT OR Apache-2.0) */
/* Function-like macros of the shapes real headers have (c_macros.rk). */
#include <stdint.h>

#define NS_PER_MS 1000000ULL
#define NS_TO_MS(NS) ((NS) / NS_PER_MS)
#define BUTTON_MASK(X) (1u << ((X)-1))
#define FOURCC(A, B, C, D) ((((uint32_t)(A)) << 0) | (((uint32_t)(B)) << 8) | (((uint32_t)(C)) << 16) | (((uint32_t)(D)) << 24))
#define HALF(x) ((x) * 0.5f)
#define CLAMP_BYTE(v) ((uint8_t)(v))
#define BELOW(a, b) ((a) < (b))
#define MUSTLOCK(S) (((S)->flags & 1) != 0)
