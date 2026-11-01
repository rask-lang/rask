/* SPDX-License-Identifier: (MIT OR Apache-2.0) */
/* Shares its macro names with c_macro_two_b.h; each import keeps its own. */
struct sa { int v; };
#define G(x) ((x) + 1)
#define GETV(P) ((P)->v)
