// SPDX-License-Identifier: (MIT OR Apache-2.0)
// C frames between a Rask caller and a Rask-exported callback. Whether the
// "after" line prints is the whole test: the panic must abort here, not unwind
// past this frame (ctrl.panic/A1).
#include <stdio.h>

long rask_add_one(long n);
void rask_exported_panics(void);

void c_calls_ok(void) {
    printf("C: got %ld\n", rask_add_one(41));
    fflush(stdout);
}

void c_calls_bad(void) {
    printf("C: before callback\n");
    fflush(stdout);
    rask_exported_panics();
    printf("C: after callback\n");
    fflush(stdout);
}
