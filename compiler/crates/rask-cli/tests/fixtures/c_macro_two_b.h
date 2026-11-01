/* SPDX-License-Identifier: (MIT OR Apache-2.0) */
/* Shares its macro names with c_macro_two_a.h; each import keeps its own. */
struct sb { int w; int v; };
#define G(x) ((x) * 100)
#define GETV(P) ((P)->v * 100)
