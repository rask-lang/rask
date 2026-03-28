// SPDX-License-Identifier: (MIT OR Apache-2.0)
// Raw pointer operations for unsafe code.
// Element size is passed as the last argument for sized operations.

#include <stdint.h>
#include <string.h>

int64_t rask_ptr_add(int64_t ptr, int64_t n, int64_t elem_size) {
    return ptr + n * elem_size;
}

int64_t rask_ptr_sub(int64_t ptr, int64_t n, int64_t elem_size) {
    return ptr - n * elem_size;
}

int64_t rask_ptr_offset(int64_t ptr, int64_t n, int64_t elem_size) {
    return ptr + n * elem_size;
}

int64_t rask_ptr_read(int64_t ptr, int64_t elem_size) {
    int64_t val = 0;
    memcpy(&val, (void *)(uintptr_t)ptr, (size_t)elem_size);
    return val;
}

void rask_ptr_write(int64_t ptr, int64_t val, int64_t elem_size) {
    memcpy((void *)(uintptr_t)ptr, &val, (size_t)elem_size);
}

int64_t rask_ptr_is_null(int64_t ptr) {
    return ptr == 0;
}

int64_t rask_ptr_is_aligned(int64_t ptr) {
    return (ptr % 8) == 0;
}

int64_t rask_ptr_is_aligned_to(int64_t ptr, int64_t n) {
    return n > 0 && (ptr % n) == 0;
}

int64_t rask_ptr_align_offset(int64_t ptr, int64_t n) {
    if (n <= 0) return 0;
    int64_t rem = ptr % n;
    return rem == 0 ? 0 : n - rem;
}
