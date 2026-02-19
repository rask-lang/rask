// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C baseline: string concat 1k — uses in-place append (matching Rask optimization).

#include <stdint.h>

extern void rask_bench_run(void (*fn)(void), const char *name);

typedef struct RaskString RaskString;
RaskString *rask_string_from(const char *s);
int64_t     rask_string_append(RaskString *s, const RaskString *other);
void        rask_string_free(RaskString *s);

static void work(void) {
    RaskString *s = rask_string_from("");
    RaskString *x = rask_string_from("x");
    for (int64_t i = 0; i < 1000; i++) {
        rask_string_append(s, x);
    }
    rask_string_free(s);
    rask_string_free(x);
}

int main(void) {
    rask_bench_run(work, "string concat 1k");
    return 0;
}
