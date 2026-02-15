// SPDX-License-Identifier: (MIT OR Apache-2.0)

// Heap allocator — thin wrappers around malloc/realloc/free.
// Centralizes allocation so we can add tracking or swap allocators later.

#include "rask_runtime.h"
#include <stdlib.h>
#include <stdio.h>
#include <string.h>

void *rask_alloc(int64_t size) {
    if (size <= 0) {
        return NULL;
    }
    void *ptr = malloc((size_t)size);
    if (!ptr) {
        fprintf(stderr, "rask: allocation failed (%lld bytes)\n", (long long)size);
        abort();
    }
    return ptr;
}

void *rask_realloc(void *ptr, int64_t old_size, int64_t new_size) {
    (void)old_size;
    if (new_size <= 0) {
        free(ptr);
        return NULL;
    }
    void *new_ptr = realloc(ptr, (size_t)new_size);
    if (!new_ptr) {
        fprintf(stderr, "rask: reallocation failed (%lld bytes)\n", (long long)new_size);
        abort();
    }
    return new_ptr;
}

void rask_free(void *ptr) {
    free(ptr);
}
