/* SPDX-License-Identifier: (MIT OR Apache-2.0) */
/* Macros that read memory or call into C: native code calls them through a
   wrapper compiled with this header (struct.c-interop/PP4). */

typedef struct { unsigned flags; int w; } Surf;

struct node { int value; struct node *next; };

union num { int i; float f; };

struct ev { int type; union { int key; float axis; } u; };

int twice(int x);

#define LOCKED(S) (((S)->flags & 1) != 0)
#define WIDTH(S) ((S)->w)
#define NEXT_VALUE(N) ((N)->next->value)
#define NUM_INT(U) ((U)->i)
#define KEY(U) ((U)->key)
#define TWICE_PLUS(x) (twice(x) + 1)
#define SWAP16(x) __builtin_bswap16(x)
